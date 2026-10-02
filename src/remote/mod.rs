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
//! timeout (`OTS_REMOTE_TIMEOUT_SECS`, default 10) and a body limit
//! (`OTS_REMOTE_MAX_BYTES`, default 64 MiB), and a `SERVICE` result a row cap
//! (`OTS_SERVICE_MAX_ROWS`, default 10 000), so a slow or huge remote cannot
//! stall or flood a local query. One query may contact at most
//! `OTS_SERVICE_MAX_ENDPOINTS` endpoints (16) with at most
//! `OTS_SERVICE_MAX_CALLS` requests (64), all within
//! `OTS_SERVICE_DEADLINE_SECS` (30) of its start (`crate::sparql::federation`). Exceeding a limit fails the request; nothing
//! is ever cut short and passed on as if it were the whole answer.
//!
//! Redirects are followed (LDES 1.0 §3.3 and TREE require it), at most
//! [`MAX_REDIRECTS`] hops, and **every hop is checked against the allowlist
//! again**: an allowlisted host cannot bounce a request to one that is not.
//! A hop to another host or port also loses the `Authorization` header
//! (reqwest strips sensitive headers there), so an identity assertion minted
//! for one peer is never shown to another.
//!
//! The allowlist is read on every call rather than cached: tests and operators
//! change it at runtime, and the cost is one environment read.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const ALLOWLIST_ENV: &str = "OTS_REMOTE_ALLOWLIST";
pub const TIMEOUT_ENV: &str = "OTS_REMOTE_TIMEOUT_SECS";
pub const MAX_ROWS_ENV: &str = "OTS_SERVICE_MAX_ROWS";
pub const MAX_BYTES_ENV: &str = "OTS_REMOTE_MAX_BYTES";
pub const MAX_ENDPOINTS_ENV: &str = "OTS_SERVICE_MAX_ENDPOINTS";
pub const MAX_CALLS_ENV: &str = "OTS_SERVICE_MAX_CALLS";
pub const DEADLINE_ENV: &str = "OTS_SERVICE_DEADLINE_SECS";

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

/// Distinct `SERVICE` endpoints one query may contact (default 16).
pub fn max_endpoints() -> usize {
    positive_env(MAX_ENDPOINTS_ENV).unwrap_or(16)
}

/// Remote requests one query may make through `SERVICE` (default 64);
/// answers reused within the query do not count.
pub fn max_calls() -> usize {
    positive_env(MAX_CALLS_ENV).unwrap_or(64)
}

/// How long after it starts a query may still make `SERVICE` requests
/// (default 30 s).
pub fn deadline() -> Duration {
    Duration::from_secs(positive_env(DEADLINE_ENV).unwrap_or(30) as u64)
}

fn positive_env(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
}

pub fn max_bytes() -> usize {
    std::env::var(MAX_BYTES_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(64 * 1024 * 1024)
}

/// The most redirects one request follows before it fails.
pub const MAX_REDIRECTS: usize = 10;

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("remote access to <{0}> is not allowed: it is not in {ALLOWLIST_ENV}")]
    NotAllowed(String),
    #[error("remote <{from}> redirected to <{to}>, which is not in {ALLOWLIST_ENV}")]
    RedirectNotAllowed { from: String, to: String },
    #[error("remote request to <{url}> failed: {reason}")]
    Request { url: String, reason: String },
    #[error("remote <{url}> answered {status}")]
    Status { url: String, status: u16 },
    #[error(
        "remote <{url}> answered more than {limit} bytes; raise {MAX_BYTES_ENV} \
         or narrow the request"
    )]
    TooLarge { url: String, limit: usize },
}

/// Read a response body as UTF-8 text, a chunk at a time, failing as soon as
/// it exceeds `OTS_REMOTE_MAX_BYTES` — before it is all in memory, and before
/// anything downstream can mistake a prefix for the whole document. RDF and
/// SPARQL results formats are UTF-8; invalid sequences are replaced, as
/// `Response::text` would.
async fn body_text(url: &str, mut resp: reqwest::Response) -> Result<String, RemoteError> {
    let limit = max_bytes();
    let too_large = || RemoteError::TooLarge {
        url: url.to_string(),
        limit,
    };
    if resp.content_length().is_some_and(|n| n > limit as u64) {
        return Err(too_large());
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| RemoteError::Request {
        url: url.to_string(),
        reason: e.to_string(),
    })? {
        if buf.len() + chunk.len() > limit {
            return Err(too_large());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(match String::from_utf8(buf) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
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

/// The reason a redirect was refused, carried through reqwest's error chain
/// so [`request_error`] can report it as [`RemoteError::RedirectNotAllowed`].
#[derive(Debug, thiserror::Error)]
#[error("redirect to <{0}> is not in the remote allowlist")]
struct RefusedRedirect(String);

/// Follow a redirect only to a URL the allowlist covers, re-checked on every
/// hop (the allowlist is read afresh, like everywhere else in this module).
fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            attempt.error(format!("more than {MAX_REDIRECTS} redirects"))
        } else if is_allowed(attempt.url().as_str()) {
            attempt.follow()
        } else {
            let to = attempt.url().to_string();
            attempt.error(RefusedRedirect(to))
        }
    })
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!("open-triplestore/", env!("CARGO_PKG_VERSION")))
            .redirect(redirect_policy())
            .build()
            .expect("reqwest client")
    })
}

/// A transport error as a [`RemoteError`]: a redirect the allowlist refused
/// is reported as such, anything else as a failed request.
fn request_error(url: &str, e: reqwest::Error) -> RemoteError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&e);
    while let Some(err) = source {
        if let Some(RefusedRedirect(to)) = err.downcast_ref::<RefusedRedirect>() {
            return RemoteError::RedirectNotAllowed {
                from: url.to_string(),
                to: to.clone(),
            };
        }
        source = err.source();
    }
    RemoteError::Request {
        url: url.to_string(),
        reason: e.to_string(),
    }
}

/// A blocking request from synchronous code (the SPARQL evaluator runs on a
/// blocking thread). It runs on the module's own runtime, on a scoped OS
/// thread, so it is safe to call from inside or outside a tokio runtime.
pub(crate) fn blocking<F, T>(fut: F) -> T
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

/// How an outbound SPARQL request identifies itself.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Auth<'a> {
    None,
    /// A federation identity assertion, or a token the caller holds.
    Bearer(&'a str),
    /// HTTP Basic — what a virtual datasource's account is, resolved from
    /// its secret reference at the moment of use and never kept.
    Basic(&'a str, &'a str),
}

/// `POST` a SPARQL query to `endpoint` and return the body as text in the
/// format `accept` asks for — results JSON for a SELECT/ASK, N-Triples for
/// a CONSTRUCT. Behind the allowlist, with the module's timeout and body
/// limit.
pub(crate) fn post_sparql_blocking(
    endpoint: &str,
    query: &str,
    accept: &str,
    auth: Auth<'_>,
) -> Result<String, RemoteError> {
    post_sparql_blocking_within(endpoint, query, accept, auth, timeout())
}

/// As [`post_sparql_blocking`], with an explicit timeout (a `SERVICE` call
/// gets what is left of its query's deadline when that is shorter).
pub(crate) fn post_sparql_blocking_within(
    endpoint: &str,
    query: &str,
    accept: &str,
    auth: Auth<'_>,
    timeout: Duration,
) -> Result<String, RemoteError> {
    if !is_allowed(endpoint) {
        return Err(RemoteError::NotAllowed(endpoint.to_string()));
    }
    let endpoint = endpoint.to_string();
    let query = query.to_string();
    let accept = accept.to_string();
    let auth = match auth {
        Auth::None => None,
        Auth::Bearer(b) => Some((b.to_string(), None)),
        Auth::Basic(u, p) => Some((u.to_string(), Some(p.to_string()))),
    };
    blocking(async move {
        let mut req = client().post(&endpoint).timeout(timeout);
        match &auth {
            Some((bearer, None)) => req = req.bearer_auth(bearer),
            Some((user, Some(password))) => req = req.basic_auth(user, Some(password)),
            None => {}
        }
        let resp = req
            .header("Content-Type", "application/sparql-query")
            .header("Accept", accept)
            .body(query)
            .send()
            .await
            .map_err(|e| request_error(&endpoint, e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(RemoteError::Status {
                url: endpoint.clone(),
                status: status.as_u16(),
            });
        }
        body_text(&endpoint, resp).await
    })
}

pub const RETRIES_ENV: &str = "OTS_REMOTE_RETRIES";
pub const MAX_RETRY_WAIT_ENV: &str = "OTS_REMOTE_MAX_RETRY_WAIT_SECS";

/// The statuses LDES 1.0 §3.3 says a client retries with back-off.
pub const RETRYABLE_STATUSES: [u16; 7] = [408, 425, 429, 500, 502, 503, 504];

/// How many times a document fetch is retried after a retryable status
/// (default 4, so five attempts at most; 0 turns retrying off).
pub fn retries() -> u32 {
    std::env::var(RETRIES_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(4)
        .min(16)
}

/// The longest single wait before a retry (default 60 s). A `Retry-After`
/// asking for longer fails the fetch instead of holding the sync that long.
pub fn max_retry_wait() -> Duration {
    let secs = std::env::var(MAX_RETRY_WAIT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(60);
    Duration::from_secs(secs)
}

/// The wait a `Retry-After` header asks for: delay-seconds or an HTTP-date
/// (RFC 9110 §10.2.3). A date in the past is no wait at all.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let v = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = chrono::DateTime::parse_from_rfc2822(v).ok()?;
    let ms = (at.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_milliseconds();
    Some(Duration::from_millis(ms.max(0) as u64))
}

/// Exponential back-off with jitter for retry `attempt` (0-based) when the
/// server named no wait: 250 ms doubling per attempt, capped at 8 s, and a
/// random half to full of that, so clients that failed together do not
/// retry together.
fn backoff(attempt: u32) -> Duration {
    let ceiling = (250u64 << attempt.min(5)).min(8_000);
    let jittered = ceiling / 2 + rand::random_range(0..=ceiling / 2);
    Duration::from_millis(jittered)
}

/// One RDF document fetched for the LDES client.
#[derive(Debug, Clone)]
pub struct RdfDocument {
    /// The URL the document was served from after any redirects: its base
    /// IRI (LDES 1.0 §3.1 "the base IRI (after redirect)").
    pub url: String,
    /// `None` when the server answered `304 Not Modified`.
    pub body: Option<String>,
    pub content_type: String,
    pub etag: Option<String>,
    /// `Cache-Control` says `immutable` (LDES 1.0 §3.2).
    pub immutable: bool,
    /// Retries it took (retryable statuses answered first).
    pub retries: u32,
}

/// The Accept header of a document fetch: every format LDES 1.0 §3.3 says a
/// client supports, quad formats first so a member's named graph survives.
pub const RDF_ACCEPT: &str = "application/trig, application/n-quads, text/turtle;q=0.9, \
     application/n-triples;q=0.8, application/ld+json;q=0.7";

/// `GET` an RDF document (LDES client), behind the allowlist, following
/// allowlisted redirects. `if_none_match` is an ETag kept from an earlier
/// fetch; a `304` comes back as a document without a body. A retryable
/// status ([`RETRYABLE_STATUSES`]) is retried up to [`retries`] times,
/// waiting what `Retry-After` asks or an exponential back-off with jitter.
pub fn get_rdf_blocking(
    url: &str,
    if_none_match: Option<&str>,
) -> Result<RdfDocument, RemoteError> {
    get_rdf_blocking_with_auth(
        url,
        if_none_match,
        crate::federation::assertion_for(url).as_deref(),
    )
}

pub fn get_rdf_blocking_with_auth(
    url: &str,
    if_none_match: Option<&str>,
    bearer: Option<&str>,
) -> Result<RdfDocument, RemoteError> {
    if !is_allowed(url) {
        return Err(RemoteError::NotAllowed(url.to_string()));
    }
    let url = url.to_string();
    let bearer = bearer.map(str::to_string);
    let if_none_match = if_none_match.map(str::to_string);
    let (max_retries, max_wait) = (retries(), max_retry_wait());
    blocking(async move {
        let mut attempt = 0u32;
        loop {
            let mut req = client()
                .get(&url)
                .timeout(timeout())
                .header("Accept", RDF_ACCEPT);
            if let Some(b) = &bearer {
                req = req.bearer_auth(b);
            }
            if let Some(tag) = &if_none_match {
                req = req.header("If-None-Match", tag);
            }
            let resp = req.send().await.map_err(|e| request_error(&url, e))?;
            let status = resp.status().as_u16();
            if RETRYABLE_STATUSES.contains(&status) && attempt < max_retries {
                let wait = retry_after(resp.headers()).unwrap_or_else(|| backoff(attempt));
                if wait <= max_wait {
                    attempt += 1;
                    tracing::debug!(%url, status, ?wait, attempt, "remote: retrying");
                    tokio::time::sleep(wait).await;
                    continue;
                }
            }
            let headers = resp.headers();
            let header = |name: reqwest::header::HeaderName| {
                headers
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            };
            let etag = header(reqwest::header::ETAG);
            let immutable = header(reqwest::header::CACHE_CONTROL).is_some_and(|cc| {
                cc.split(',')
                    .any(|d| d.trim().eq_ignore_ascii_case("immutable"))
            });
            let content_type =
                header(reqwest::header::CONTENT_TYPE).unwrap_or_else(|| "text/turtle".to_string());
            let final_url = resp.url().to_string();
            if status == 304 {
                return Ok(RdfDocument {
                    url: final_url,
                    body: None,
                    content_type,
                    etag: etag.or(if_none_match),
                    immutable,
                    retries: attempt,
                });
            }
            if !resp.status().is_success() {
                return Err(RemoteError::Status { url, status });
            }
            let body = body_text(&url, resp).await?;
            return Ok(RdfDocument {
                url: final_url,
                body: Some(body),
                content_type,
                etag,
                immutable,
                retries: attempt,
            });
        }
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
    fn retry_after_reads_seconds_and_http_dates() {
        let mut h = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after(&h), None);
        h.insert(reqwest::header::RETRY_AFTER, "7".parse().unwrap());
        assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
        h.insert(
            reqwest::header::RETRY_AFTER,
            "Sun, 06 Nov 1994 08:49:37 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after(&h), Some(Duration::ZERO), "a past date: now");
        let soon = (chrono::Utc::now() + chrono::Duration::seconds(30)).to_rfc2822();
        h.insert(reqwest::header::RETRY_AFTER, soon.parse().unwrap());
        let wait = retry_after(&h).unwrap();
        assert!(wait > Duration::from_secs(25) && wait <= Duration::from_secs(30));
        h.insert(reqwest::header::RETRY_AFTER, "soon".parse().unwrap());
        assert_eq!(retry_after(&h), None);
    }

    #[test]
    fn backoff_grows_with_jitter_and_is_capped() {
        for attempt in 0..20 {
            let ceiling = (250u64 << attempt.min(5)).min(8_000);
            let d = backoff(attempt).as_millis() as u64;
            assert!(d >= ceiling / 2 && d <= ceiling, "{attempt}: {d}");
        }
        assert!(backoff(30) <= Duration::from_secs(8));
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
