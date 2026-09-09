//! Outbound HTTP from the store, behind an operator allowlist.
//!
//! Two features reach out to other servers on a user's behalf: SPARQL
//! federation (`SERVICE <endpoint> { … }`) and LDES client sync. Both are
//! server-side request forgery vectors if any URL may be named, so neither
//! may contact an endpoint unless it is covered by an entry in
//! `OTS_REMOTE_ALLOWLIST` (comma-separated URLs; empty or unset means "no
//! remote access at all", which is the default). An entry covers a URL when
//! the parsed origins agree — scheme, host and port — and the entry's path is
//! a segment-boundary prefix of the URL's path; the URL may not carry
//! credentials. Matching is never on the raw string, so an entry of
//! `https://sparql.example.org` admits neither
//! `https://sparql.example.org.evil.net/` nor
//! `https://sparql.example.org@evil.net/`. Every request also gets a
//! timeout (`OTS_REMOTE_TIMEOUT_SECS`, default 10) and a result cap
//! (`OTS_SERVICE_MAX_ROWS`, default 10 000) so a slow or huge remote cannot
//! stall or flood a local query.
//!
//! The allowlist is read on every call rather than cached: tests and operators
//! change it at runtime, and the cost is one environment read.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const ALLOWLIST_ENV: &str = "OTS_REMOTE_ALLOWLIST";
pub const TIMEOUT_ENV: &str = "OTS_REMOTE_TIMEOUT_SECS";
pub const MAX_ROWS_ENV: &str = "OTS_SERVICE_MAX_ROWS";

/// The raw allowlist entries, trimmed, empty entries dropped.
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

/// One allowlist entry, parsed. A URL is covered when its scheme, host and
/// port equal the entry's and its path lies under the entry's path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AllowEntry {
    scheme: String,
    /// Lower-cased; hosts are case-insensitive.
    host: String,
    /// The explicit port or the scheme's default, so `https://h` and
    /// `https://h:443` are the same origin.
    port: Option<u16>,
    path: String,
}

impl AllowEntry {
    /// `None` for anything that is not an absolute http(s) URL with a host
    /// and without credentials.
    fn parse(entry: &str) -> Option<Self> {
        let u = url::Url::parse(entry).ok()?;
        if !matches!(u.scheme(), "http" | "https") || has_userinfo(&u) {
            return None;
        }
        Some(Self {
            scheme: u.scheme().to_string(),
            host: u.host_str()?.to_ascii_lowercase(),
            port: u.port_or_known_default(),
            path: u.path().to_string(),
        })
    }

    fn allows(&self, u: &url::Url) -> bool {
        // A URL with userinfo is refused outright: `https://allowed.org@evil.net/`
        // parses with `allowed.org` as the *username* and `evil.net` as the
        // host, and there is no legitimate reason for the store to send
        // credentials embedded in a SERVICE or LDES URL.
        if has_userinfo(u) || u.scheme() != self.scheme {
            return false;
        }
        let same_host = u
            .host_str()
            .is_some_and(|h| h.eq_ignore_ascii_case(&self.host));
        if !same_host || u.port_or_known_default() != self.port {
            return false;
        }
        let path = u.path();
        if self.path.ends_with('/') {
            path.starts_with(&self.path)
        } else {
            // A slash-less entry path is a directory prefix on segment
            // boundaries: `/sparql` covers `/sparql` and `/sparql/x`, not
            // `/sparqlx`.
            path.strip_prefix(self.path.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        }
    }
}

fn has_userinfo(u: &url::Url) -> bool {
    !u.username().is_empty() || u.password().is_some()
}

/// Warn about an entry that cannot be used, once per distinct entry: the
/// allowlist is re-read on every call, and a bad entry should not flood the
/// log every time a query runs.
fn warn_bad_entry(entry: &str) {
    static REPORTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut seen = REPORTED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if seen.insert(entry.to_string()) {
        tracing::warn!(
            entry,
            "{ALLOWLIST_ENV}: ignoring an entry that is not an absolute http(s) URL \
             without credentials"
        );
    }
}

fn parsed_allowlist() -> Vec<AllowEntry> {
    allowlist()
        .iter()
        .filter_map(|e| {
            let parsed = AllowEntry::parse(e);
            if parsed.is_none() {
                warn_bad_entry(e);
            }
            parsed
        })
        .collect()
}

/// Whether `url` may be contacted: an absolute http(s) URL, without
/// credentials, on the exact origin of one of the allowlisted entries and
/// under that entry's path. See the module docs for what that rules out.
pub fn is_allowed(url: &str) -> bool {
    allowed_by(&parsed_allowlist(), url)
}

fn allowed_by(entries: &[AllowEntry], url: &str) -> bool {
    let Ok(u) = url::Url::parse(url.trim()) else {
        return false;
    };
    if !matches!(u.scheme(), "http" | "https") {
        return false;
    }
    entries.iter().any(|e| e.allows(&u))
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
/// return the body as `application/sparql-results+json` text, optionally as
/// the bearer of `bearer` (a federation identity assertion).
pub fn post_sparql_blocking_with_auth(
    endpoint: &str,
    query: &str,
    bearer: Option<&str>,
) -> Result<String, RemoteError> {
    if !is_allowed(endpoint) {
        return Err(RemoteError::NotAllowed(endpoint.to_string()));
    }
    let endpoint = endpoint.to_string();
    let query = query.to_string();
    let bearer = bearer.map(str::to_string);
    blocking(async move {
        let mut req = client().post(&endpoint).timeout(timeout());
        if let Some(b) = &bearer {
            req = req.bearer_auth(b);
        }
        let resp = req
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
    get_rdf_blocking_with_auth(url, crate::federation::assertion_for(url).as_deref())
}

pub fn get_rdf_blocking_with_auth(
    url: &str,
    bearer: Option<&str>,
) -> Result<(String, String), RemoteError> {
    if !is_allowed(url) {
        return Err(RemoteError::NotAllowed(url.to_string()));
    }
    let url = url.to_string();
    let bearer = bearer.map(str::to_string);
    blocking(async move {
        let mut req = client().get(&url).timeout(timeout());
        if let Some(b) = &bearer {
            req = req.bearer_auth(b);
        }
        let resp = req
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

    fn entries(list: &str) -> Vec<AllowEntry> {
        list.split(',')
            .map(str::trim)
            .filter_map(AllowEntry::parse)
            .collect()
    }

    #[test]
    fn allowlist_is_origin_based_and_scheme_checked() {
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

    /// The matcher used to be `url.starts_with(entry)` on the raw string, so an
    /// entry without a trailing slash — which the docs merely *recommended* —
    /// admitted any host that continued the string. Each case below then
    /// minted an identity assertion for the attacker's origin and posted it
    /// there with bearer auth.
    #[test]
    fn a_slashless_entry_cannot_be_extended_into_another_host() {
        let e = entries("https://sparql.example.org");
        assert!(
            !allowed_by(&e, "https://sparql.example.org@evil.net/"),
            "userinfo bypass: the entry is the username, evil.net is the host"
        );
        assert!(
            !allowed_by(&e, "https://sparql.example.org.evil.net/"),
            "subdomain-suffix bypass"
        );
        assert!(
            !allowed_by(&e, "https://sparql.example.org:8443/sparql"),
            "a different port is a different origin"
        );
        assert!(
            !allowed_by(&e, "http://sparql.example.org/sparql"),
            "a different scheme is a different origin"
        );
        // …while the entry still covers every path on its own origin.
        assert!(allowed_by(&e, "https://sparql.example.org"));
        assert!(allowed_by(&e, "https://sparql.example.org/"));
        assert!(allowed_by(&e, "https://sparql.example.org/sparql?query=x"));
        assert!(allowed_by(&e, "https://sparql.example.org:443/sparql"));
    }

    #[test]
    fn hosts_match_case_insensitively() {
        let e = entries("https://sparql.example.org/");
        assert!(allowed_by(&e, "https://SPARQL.Example.ORG/sparql"));
        let e = entries("https://SPARQL.example.org/");
        assert!(allowed_by(&e, "https://sparql.example.org/sparql"));
    }

    #[test]
    fn entry_paths_are_prefixes_on_segment_boundaries() {
        let e = entries("https://h.example/sparql");
        assert!(allowed_by(&e, "https://h.example/sparql"));
        assert!(allowed_by(&e, "https://h.example/sparql/"));
        assert!(allowed_by(&e, "https://h.example/sparql/x"));
        assert!(!allowed_by(&e, "https://h.example/sparqlx"));
        assert!(!allowed_by(&e, "https://h.example/"));
        assert!(!allowed_by(&e, "https://h.example/other"));
        assert!(
            !allowed_by(&e, "https://h.example/sparql/../admin"),
            "dot segments are resolved before matching"
        );

        let e = entries("https://h.example/sparql/");
        assert!(allowed_by(&e, "https://h.example/sparql/"));
        assert!(allowed_by(&e, "https://h.example/sparql/x"));
        assert!(!allowed_by(&e, "https://h.example/sparql"));
        assert!(!allowed_by(&e, "https://h.example/sparqlx"));
    }

    #[test]
    fn a_url_with_credentials_is_never_allowed() {
        let e = entries("https://h.example/");
        assert!(!allowed_by(&e, "https://user:pw@h.example/sparql"));
        assert!(!allowed_by(&e, "https://user@h.example/sparql"));
    }

    #[test]
    fn unparsable_entries_and_urls_are_ignored() {
        assert!(AllowEntry::parse("not a url").is_none());
        assert!(AllowEntry::parse("ftp://h.example/").is_none());
        assert!(AllowEntry::parse("https://").is_none());
        assert!(
            AllowEntry::parse("https://user@h.example/").is_none(),
            "an entry with credentials is refused rather than half-applied"
        );
        let e = entries("garbage, https://h.example/");
        assert_eq!(e.len(), 1, "the bad entry is dropped, the good one kept");
        assert!(allowed_by(&e, "https://h.example/sparql"));
        assert!(!allowed_by(&e, "not a url"));
        assert!(!allowed_by(&e, ""));
        assert!(!allowed_by(&[], "https://h.example/sparql"));
    }
}
