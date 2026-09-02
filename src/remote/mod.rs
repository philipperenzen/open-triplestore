//! Outbound HTTP from the store, behind an operator allowlist.
//!
//! Two features reach out to other servers on a user's behalf: SPARQL
//! federation (`SERVICE <endpoint> { … }`) and LDES client sync. Both are
//! server-side request forgery vectors if any URL may be named, so neither
//! may contact an endpoint unless its prefix is listed in
//! `OTS_REMOTE_ALLOWLIST` (comma-separated URL prefixes; empty or unset means
//! "no remote access at all", which is the default). Every request also gets a
//! timeout (`OTS_REMOTE_TIMEOUT_SECS`, default 10) and a result cap
//! (`OTS_SERVICE_MAX_ROWS`, default 10 000) so a slow or huge remote cannot
//! stall or flood a local query.
//!
//! The allowlist is read on every call rather than cached: tests and operators
//! change it at runtime, and the cost is one environment read.

use std::sync::OnceLock;
use std::time::Duration;

pub const ALLOWLIST_ENV: &str = "OTS_REMOTE_ALLOWLIST";
pub const TIMEOUT_ENV: &str = "OTS_REMOTE_TIMEOUT_SECS";
pub const MAX_ROWS_ENV: &str = "OTS_SERVICE_MAX_ROWS";

/// The allowlisted URL prefixes, trimmed, empty entries dropped.
pub fn allowlist() -> Vec<String> {
    std::env::var(ALLOWLIST_ENV)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether remote access is configured at all.
pub fn enabled() -> bool {
    !allowlist().is_empty()
}

/// Whether `url` may be contacted: an absolute http(s) URL that starts with one
/// of the allowlisted prefixes. Prefix matching is on the whole string, so an
/// entry of `https://sparql.example.org/` does not admit
/// `https://sparql.example.org.evil.net/` (the trailing slash is part of the
/// prefix) — operators should list origins with their trailing slash.
pub fn is_allowed(url: &str) -> bool {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return false;
    }
    allowlist().iter().any(|p| url.starts_with(p.as_str()))
}

pub fn timeout() -> Duration {
    let secs = std::env::var(TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(10);
    Duration::from_secs(secs)
}

pub fn max_rows() -> usize {
    std::env::var(MAX_ROWS_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(10_000)
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("remote access to <{0}> is not allowed: it is not in {ALLOWLIST_ENV}")]
    NotAllowed(String),
    #[error("remote request to <{url}> failed: {reason}")]
    Request { url: String, reason: String },
    #[error("remote <{url}> answered {status}")]
    Status { url: String, status: u16 },
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("remote runtime")
    })
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!("open-triplestore/", env!("CARGO_PKG_VERSION")))
            // No redirects: an allowlisted host must not be able to bounce a
            // request to one that is not.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client")
    })
}

/// A blocking request from synchronous code (the SPARQL evaluator runs on a
/// blocking thread). It runs on the module's own runtime, on a scoped OS
/// thread, so it is safe to call from inside or outside a tokio runtime.
fn blocking<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    std::thread::scope(|s| {
        s.spawn(|| runtime().block_on(fut))
            .join()
            .expect("remote call thread")
    })
}

/// `POST` a SPARQL query to `endpoint` (SPARQL 1.1 Protocol, direct POST) and
/// return the body as `application/sparql-results+json` text.
pub fn post_sparql_blocking(endpoint: &str, query: &str) -> Result<String, RemoteError> {
    if !is_allowed(endpoint) {
        return Err(RemoteError::NotAllowed(endpoint.to_string()));
    }
    let endpoint = endpoint.to_string();
    let query = query.to_string();
    blocking(async move {
        let resp = client()
            .post(&endpoint)
            .timeout(timeout())
            .header("Content-Type", "application/sparql-query")
            .header("Accept", "application/sparql-results+json")
            .body(query)
            .send()
            .await
            .map_err(|e| RemoteError::Request {
                url: endpoint.clone(),
                reason: e.to_string(),
            })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(RemoteError::Status {
                url: endpoint.clone(),
                status: status.as_u16(),
            });
        }
        resp.text().await.map_err(|e| RemoteError::Request {
            url: endpoint,
            reason: e.to_string(),
        })
    })
}

/// `GET` a URL with an RDF `Accept` header (LDES client). Returns
/// `(content-type, body)`.
#[allow(dead_code)] // the LDES client (6.1) is its caller; the binary sees it before then
pub fn get_rdf_blocking(url: &str) -> Result<(String, String), RemoteError> {
    if !is_allowed(url) {
        return Err(RemoteError::NotAllowed(url.to_string()));
    }
    let url = url.to_string();
    blocking(async move {
        let resp = client()
            .get(&url)
            .timeout(timeout())
            .header(
                "Accept",
                "text/turtle, application/n-triples;q=0.9, application/ld+json;q=0.8",
            )
            .send()
            .await
            .map_err(|e| RemoteError::Request {
                url: url.clone(),
                reason: e.to_string(),
            })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(RemoteError::Status {
                url: url.clone(),
                status: status.as_u16(),
            });
        }
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/turtle")
            .to_string();
        let body = resp.text().await.map_err(|e| RemoteError::Request {
            url,
            reason: e.to_string(),
        })?;
        Ok((ct, body))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_is_prefix_based_and_scheme_checked() {
        std::env::set_var(
            ALLOWLIST_ENV,
            "https://sparql.example.org/, http://127.0.0.1:9999/",
        );
        assert!(is_allowed("https://sparql.example.org/query"));
        assert!(is_allowed("http://127.0.0.1:9999/sparql"));
        assert!(!is_allowed("https://sparql.example.org.evil.net/"));
        assert!(!is_allowed("ftp://sparql.example.org/"));
        assert!(!is_allowed("https://other.example.org/"));
        std::env::set_var(ALLOWLIST_ENV, "");
        assert!(!is_allowed("https://sparql.example.org/query"));
        assert!(!enabled());
    }
}
