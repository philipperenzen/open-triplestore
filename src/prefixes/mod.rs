//! Prefix registry with prefix.cc lookup and local JSON caching.
//!
//! # Design
//!
//! SPARQL queries frequently use well-known namespace prefixes like `foaf:`,
//! `schema:`, or `owl:` without declaring them.  This module transparently
//! resolves those prefixes via the public [prefix.cc](https://prefix.cc) service
//! and stores the results in a local cache so subsequent queries never hit the
//! network.
//!
//! ## How auto-resolution works
//!
//! Before a SPARQL query reaches the store engine, [`find_undeclared_prefixes`]
//! scans the query text for prefix usages that lack a matching `PREFIX` declaration.
//! Each undeclared prefix is looked up in the registry (cache first, then
//! prefix.cc), and any resolved prefix is prepended to the query as a standard
//! `PREFIX label: <IRI>` declaration.
//!
//! ## Security
//!
//! * **Label validation** — prefix labels are checked against
//!   `[A-Za-z][A-Za-z0-9_-]*` (max 64 chars) before any network call,
//!   preventing SSRF via crafted label strings injected into the URL.
//! * **IRI validation** — only `http` and `https` IRIs are accepted from
//!   prefix.cc responses; `file://`, `javascript:`, `data:`, etc. are rejected.
//! * **URL construction** — the `/reverse` endpoint URL is built with
//!   [`url::Url::query_pairs_mut`] so the IRI is always percent-encoded, never
//!   spliced into the URL as a raw string.
//! * **Cache permissions** — the JSON cache file is written with `0o600`
//!   permissions (owner read/write only) on Unix systems.
//! * **Atomic writes** — cache updates are written to a `.tmp` file first,
//!   then atomically renamed, preventing partial/corrupt cache files.
//! * **Circuit breaker** — after [`CIRCUIT_BREAKER_THRESHOLD`] consecutive HTTP
//!   failures for the same label, further network calls are suppressed until the
//!   process restarts.
//! * **rustls** — TLS uses the pure-Rust `rustls` backend so no system OpenSSL
//!   is required in the Docker runtime image.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use url::Url;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Consecutive HTTP failures for a single label that trip the circuit breaker.
const CIRCUIT_BREAKER_THRESHOLD: u32 = 5;

/// Timeout for HTTP requests to prefix.cc.
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum permitted length of a prefix label.
const MAX_LABEL_LENGTH: usize = 64;

/// URI schemes accepted in resolved prefix IRIs.
const ALLOWED_SCHEMES: &[&str] = &["http", "https"];

// ─── Persistent cache ────────────────────────────────────────────────────────

/// Cache data serialized to the JSON file on disk.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct CacheData {
    /// Forward map: prefix label → namespace IRI.
    by_label: HashMap<String, String>,
    /// Reverse map: namespace IRI → prefix label.
    by_iri: HashMap<String, String>,
}

// ─── Runtime state ───────────────────────────────────────────────────────────

/// In-memory state that resets on process restart (not persisted).
#[derive(Default)]
struct RuntimeState {
    cache: CacheData,
    /// Consecutive HTTP failure count per label.
    failures: HashMap<String, u32>,
    /// Labels confirmed absent on prefix.cc (404 responses).
    not_found: HashSet<String>,
}

// ─── PrefixRegistry ──────────────────────────────────────────────────────────

/// Thread-safe prefix registry backed by prefix.cc with local JSON caching.
///
/// # Thread safety
///
/// `PrefixRegistry` is `Send + Sync`.  Internal state is protected by a
/// [`std::sync::Mutex`] that is *never* held across `.await` points, so it is
/// safe to use from an async context.
pub struct PrefixRegistry {
    cache_path: PathBuf,
    client: reqwest::Client,
    state: Mutex<RuntimeState>,
}

impl PrefixRegistry {
    /// Lock the runtime state, recovering the guard if a previous holder panicked.
    ///
    /// The cached prefix map is rebuildable, so a poisoned lock should degrade
    /// gracefully (stale-but-usable state) rather than crash every subsequent
    /// prefix lookup process-wide.
    fn lock_state(&self) -> MutexGuard<'_, RuntimeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Open (or create) a prefix registry backed by `cache_path`.
    ///
    /// If the file exists and is valid JSON it is loaded into memory.
    /// If it does not exist an empty registry is created; the file will be
    /// written when the first prefix is resolved.
    pub fn open(cache_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let cache_path = cache_path.as_ref().to_path_buf();

        let cache = if cache_path.exists() {
            let bytes = std::fs::read(&cache_path)?;
            serde_json::from_slice::<CacheData>(&bytes).unwrap_or_else(|e| {
                warn!(
                    "Prefix cache at {:?} could not be parsed ({}); starting fresh",
                    cache_path, e
                );
                CacheData::default()
            })
        } else {
            CacheData::default()
        };

        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(concat!(
                "open-triplestore/",
                env!("CARGO_PKG_VERSION"),
                " (prefix resolver)"
            ))
            .build()?;

        Ok(Self {
            cache_path,
            client,
            state: Mutex::new(RuntimeState {
                cache,
                ..Default::default()
            }),
        })
    }

    /// Create a no-op, purely in-memory registry with no backing file (for tests).
    pub fn empty() -> Self {
        Self {
            cache_path: std::path::PathBuf::new(),
            client: reqwest::Client::new(),
            state: Mutex::new(RuntimeState::default()),
        }
    }

    // ── Forward lookup ───────────────────────────────────────────────────────

    /// Return the namespace IRI for a prefix `label`.
    ///
    /// Resolution order:
    /// 1. In-memory cache
    /// 2. `https://prefix.cc/{label}.file.json`
    ///
    /// Returns `None` when the label is invalid, unknown, or all network
    /// attempts have failed.
    pub async fn lookup_prefix(&self, label: &str) -> Option<String> {
        // Security: validate label before any network call.
        if !is_valid_label(label) {
            debug!("Skipping invalid prefix label {:?}", label);
            return None;
        }

        // Fast path: check in-memory cache.
        {
            let state = self.lock_state();
            if let Some(iri) = state.cache.by_label.get(label) {
                debug!("Cache hit: prefix '{}' → {}", label, iri);
                return Some(iri.clone());
            }
            if state.not_found.contains(label) {
                return None;
            }
            if *state.failures.get(label).unwrap_or(&0) >= CIRCUIT_BREAKER_THRESHOLD {
                debug!("Circuit breaker open for prefix '{}'", label);
                return None;
            }
        } // mutex released before await

        // Slow path: fetch from prefix.cc.
        let url = format!("https://prefix.cc/{}.file.json", label);
        debug!("Fetching prefix '{}' from {}", label, url);

        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                warn!("HTTP error looking up prefix '{}': {}", label, e);
                let mut state = self.lock_state();
                *state.failures.entry(label.to_string()).or_insert(0) += 1;
                return None;
            }
        };

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            debug!("prefix.cc: '{}' not found", label);
            self.lock_state().not_found.insert(label.to_string());
            return None;
        }

        if !resp.status().is_success() {
            warn!(
                "prefix.cc returned {} for prefix '{}'",
                resp.status(),
                label
            );
            let mut state = self.lock_state();
            *state.failures.entry(label.to_string()).or_insert(0) += 1;
            return None;
        }

        // Parse JSON response: {"label": "IRI"}
        let json: serde_json::Value = match resp.json().await {
            Ok(j) => j,
            Err(e) => {
                warn!("Bad prefix.cc response for '{}': {}", label, e);
                let mut state = self.lock_state();
                *state.failures.entry(label.to_string()).or_insert(0) += 1;
                return None;
            }
        };

        let iri = match json
            .as_object()
            .and_then(|m| m.values().next())
            .and_then(|v| v.as_str())
        {
            Some(s) => s.to_string(),
            None => {
                warn!("Unexpected prefix.cc JSON shape for '{}'", label);
                let mut state = self.lock_state();
                *state.failures.entry(label.to_string()).or_insert(0) += 1;
                return None;
            }
        };

        // Security: only store http/https IRIs.
        if !is_valid_iri(&iri) {
            warn!(
                "prefix.cc returned disallowed IRI for '{}': {:?}",
                label, iri
            );
            self.lock_state().not_found.insert(label.to_string());
            return None;
        }

        debug!("Resolved prefix '{}' → {}", label, iri);

        {
            let mut state = self.lock_state();
            state.cache.by_label.insert(label.to_string(), iri.clone());
            state.cache.by_iri.insert(iri.clone(), label.to_string());
            state.failures.remove(label);
        }

        self.persist_cache();
        Some(iri)
    }

    // ── Reverse lookup ───────────────────────────────────────────────────────

    /// Return the prefix label for a namespace `iri`.
    ///
    /// Resolution order:
    /// 1. In-memory cache
    /// 2. `https://prefix.cc/reverse?uri=<iri>&format=json`
    ///
    /// Returns `None` when the IRI is invalid, unknown, or the request fails.
    pub async fn reverse_lookup(&self, iri: &str) -> Option<(String, String)> {
        // Security: validate IRI before any network call.
        if !is_valid_iri(iri) {
            debug!("Skipping invalid IRI for reverse lookup: {:?}", iri);
            return None;
        }

        // Fast path: check in-memory cache.
        {
            let state = self.lock_state();
            if let Some(label) = state.cache.by_iri.get(iri) {
                debug!("Cache hit: IRI '{}' → prefix '{}'", iri, label);
                return Some((label.clone(), iri.to_string()));
            }
        } // mutex released before await

        // Build URL safely — query_pairs_mut percent-encodes the IRI, so it
        // can never be interpreted as part of the path or inject extra params.
        let mut lookup_url = Url::parse("https://prefix.cc/reverse").ok()?;
        lookup_url
            .query_pairs_mut()
            .append_pair("uri", iri)
            .append_pair("format", "json");

        debug!("Reverse lookup '{}' → {}", iri, lookup_url);

        let resp = match self.client.get(lookup_url.as_str()).send().await {
            Ok(r) => r,
            Err(e) => {
                warn!("HTTP error during reverse lookup for '{}': {}", iri, e);
                return None;
            }
        };

        if !resp.status().is_success() {
            debug!("prefix.cc reverse returned {} for '{}'", resp.status(), iri);
            return None;
        }

        // Parse JSON response: {"label": "IRI"}
        let json: serde_json::Value = match resp.json().await {
            Ok(j) => j,
            Err(e) => {
                warn!("Bad prefix.cc reverse response for '{}': {}", iri, e);
                return None;
            }
        };

        let (label, resolved_iri) = json
            .as_object()
            .and_then(|m| m.iter().next())
            .and_then(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))?;

        // Security: validate returned data before caching.
        if !is_valid_label(&label) || !is_valid_iri(&resolved_iri) {
            warn!(
                "prefix.cc reverse returned invalid data for '{}': label={:?}, iri={:?}",
                iri, label, resolved_iri
            );
            return None;
        }

        debug!("Resolved IRI '{}' → prefix '{}'", iri, label);

        {
            let mut state = self.lock_state();
            state
                .cache
                .by_label
                .insert(label.clone(), resolved_iri.clone());
            state
                .cache
                .by_iri
                .insert(resolved_iri.clone(), label.clone());
        }

        self.persist_cache();
        Some((label, resolved_iri))
    }

    // ── Cache persistence ────────────────────────────────────────────────────

    /// Write the cache to disk atomically.
    ///
    /// Writes to `{cache_path}.tmp`, sets `0o600` permissions on Unix, then
    /// renames to the real path to prevent partial/corrupt writes.
    fn persist_cache(&self) {
        // Serialize while holding the lock, then release before doing I/O.
        let json = {
            let state = self.lock_state();
            match serde_json::to_string_pretty(&state.cache) {
                Ok(j) => j,
                Err(e) => {
                    warn!("Failed to serialize prefix cache: {}", e);
                    return;
                }
            }
        };

        let tmp = self.cache_path.with_extension("json.tmp");

        if let Err(e) = std::fs::write(&tmp, json.as_bytes()) {
            warn!("Failed to write prefix cache temp file: {}", e);
            return;
        }

        // Restrict to owner-only read/write (Unix only; no-op on Windows).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)) {
                warn!("Failed to set prefix cache permissions: {}", e);
            }
        }

        if let Err(e) = std::fs::rename(&tmp, &self.cache_path) {
            warn!("Failed to atomically rename prefix cache: {}", e);
        } else {
            debug!("Prefix cache persisted to {:?}", self.cache_path);
        }
    }
}

// ─── SPARQL prefix scanner ────────────────────────────────────────────────────

/// Return prefix labels that are used but not declared in `sparql`.
///
/// The scanner handles:
/// - `PREFIX label: <IRI>` declarations (case-insensitive)
/// - `label:localname` usages
/// - Skips content inside IRI literals (`<…>`), single/double/triple-quoted
///   string literals, and `#` line comments to avoid false positives.
///
/// Only labels that pass [`is_valid_label`] are returned, so the result set
/// is always safe to pass to [`PrefixRegistry::lookup_prefix`].
pub fn find_undeclared_prefixes(sparql: &str) -> Vec<String> {
    let declared = scan_declared(sparql);
    let used = scan_used(sparql);

    used.into_iter()
        .filter(|l| !declared.contains(l.as_str()) && is_valid_label(l))
        .collect()
}

/// Extract all prefix labels declared with `PREFIX label:` (case-insensitive).
fn scan_declared(sparql: &str) -> HashSet<String> {
    let mut result = HashSet::new();
    let bytes = sparql.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Quick check for 'P'/'p' before the heavier comparison.
        if (bytes[i] == b'P' || bytes[i] == b'p') && i + 6 <= len {
            let candidate = &sparql[i..i + 6];
            if candidate.eq_ignore_ascii_case("PREFIX") {
                // Verify word boundary before (not part of a longer identifier).
                let before_ok =
                    i == 0 || (!bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_');
                // Verify word boundary after.
                let after_ok =
                    i + 6 >= len || (!bytes[i + 6].is_ascii_alphanumeric() && bytes[i + 6] != b'_');

                if before_ok && after_ok {
                    let mut j = i + 6;
                    // Skip whitespace after PREFIX.
                    while j < len
                        && (bytes[j] == b' '
                            || bytes[j] == b'\t'
                            || bytes[j] == b'\n'
                            || bytes[j] == b'\r')
                    {
                        j += 1;
                    }
                    // Read the label (may be empty for the default namespace).
                    let label_start = j;
                    while j < len
                        && (bytes[j].is_ascii_alphanumeric()
                            || bytes[j] == b'_'
                            || bytes[j] == b'-')
                    {
                        j += 1;
                    }
                    // Confirm a colon immediately follows the label.
                    if j < len && bytes[j] == b':' {
                        result.insert(sparql[label_start..j].to_string());
                    }
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }

    result
}

/// Extract all prefix labels used as `label:localname` in a SPARQL query.
///
/// Skips IRIs (`<…>`), string literals, and `#` comments to avoid false
/// positives.  Common URI schemes (`http`, `https`, `ftp`, `urn`, `mailto`,
/// `file`, `data`) are filtered out because they are not SPARQL prefix labels.
fn scan_used(sparql: &str) -> HashSet<String> {
    /// URI schemes that look like `word:` but are not prefix labels.
    const URI_SCHEMES: &[&str] = &["http", "https", "ftp", "urn", "mailto", "file", "data"];

    let mut result = HashSet::new();
    let bytes = sparql.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // ── Skip line comments ────────────────────────────────────────────
        if bytes[i] == b'#' {
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // ── Skip triple-quoted string literals ────────────────────────────
        if i + 2 < len
            && ((bytes[i] == b'"' && bytes[i + 1] == b'"' && bytes[i + 2] == b'"')
                || (bytes[i] == b'\'' && bytes[i + 1] == b'\'' && bytes[i + 2] == b'\''))
        {
            let q = bytes[i];
            i += 3;
            while i + 2 < len {
                if bytes[i] == q && bytes[i + 1] == q && bytes[i + 2] == q {
                    i += 3;
                    break;
                }
                if bytes[i] == b'\\' {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }

        // ── Skip single/double-quoted string literals ─────────────────────
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let q = bytes[i];
            i += 1;
            while i < len {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == q {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // ── Skip IRI literals <…> ─────────────────────────────────────────
        if bytes[i] == b'<' {
            i += 1;
            while i < len && bytes[i] != b'>' {
                i += 1;
            }
            if i < len {
                i += 1;
            }
            continue;
        }

        // ── Identifier start ──────────────────────────────────────────────
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < len
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
            {
                i += 1;
            }
            // Record if immediately followed by ':'.
            if i < len && bytes[i] == b':' {
                let label = &sparql[start..i];
                let lower = label.to_ascii_lowercase();
                if !URI_SCHEMES.contains(&lower.as_str()) && !label.is_empty() {
                    result.insert(label.to_string());
                }
            }
            continue;
        }

        i += 1;
    }

    result
}

// ─── Security helpers ─────────────────────────────────────────────────────────

/// Validate a prefix label: `[A-Za-z][A-Za-z0-9_-]*`, max 64 characters.
///
/// This is the primary SSRF guard — only labels that pass this check are ever
/// used to construct a prefix.cc URL.
pub fn is_valid_label(label: &str) -> bool {
    if label.is_empty() || label.len() > MAX_LABEL_LENGTH {
        return false;
    }
    let mut chars = label.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Validate that an IRI uses an allowed scheme (`http` or `https`).
///
/// Blocks `file://`, `javascript:`, `data:`, etc. from being cached or used.
pub fn is_valid_iri(iri: &str) -> bool {
    match Url::parse(iri) {
        Ok(u) => ALLOWED_SCHEMES.contains(&u.scheme()),
        Err(_) => false,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Label validation ─────────────────────────────────────────────────────

    #[test]
    fn valid_labels() {
        for label in &["foaf", "owl", "schema", "ex", "my-ns", "rdf_11"] {
            assert!(is_valid_label(label), "expected valid: {}", label);
        }
    }

    #[test]
    fn invalid_labels() {
        assert!(!is_valid_label(""));
        assert!(!is_valid_label("123abc")); // starts with digit
        assert!(!is_valid_label("a b")); // contains space
        assert!(!is_valid_label("a/b")); // contains slash
        assert!(!is_valid_label("a..b")); // contains dots
        assert!(!is_valid_label(&"x".repeat(65))); // too long
        assert!(!is_valid_label("_private")); // starts with underscore
    }

    // ── IRI validation ───────────────────────────────────────────────────────

    #[test]
    fn valid_iris() {
        assert!(is_valid_iri("http://xmlns.com/foaf/0.1/"));
        assert!(is_valid_iri("https://schema.org/"));
    }

    #[test]
    fn invalid_iris() {
        assert!(!is_valid_iri("file:///etc/passwd"));
        assert!(!is_valid_iri("javascript:alert(1)"));
        assert!(!is_valid_iri("data:text/plain,hello"));
        assert!(!is_valid_iri("not-a-url"));
        assert!(!is_valid_iri("ftp://example.com/")); // ftp not in ALLOWED_SCHEMES
    }

    // ── Prefix scanner ───────────────────────────────────────────────────────

    #[test]
    fn scanner_finds_undeclared() {
        let sparql = "SELECT ?name WHERE { ?s foaf:name ?name }";
        let undeclared = find_undeclared_prefixes(sparql);
        assert!(undeclared.contains(&"foaf".to_string()), "{:?}", undeclared);
    }

    #[test]
    fn scanner_respects_declarations() {
        let sparql = "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\n\
                      SELECT ?name WHERE { ?s foaf:name ?name }";
        let undeclared = find_undeclared_prefixes(sparql);
        assert!(
            !undeclared.contains(&"foaf".to_string()),
            "foaf should be declared: {:?}",
            undeclared
        );
    }

    #[test]
    fn scanner_multiple_undeclared() {
        let sparql = "SELECT * WHERE { ?s foaf:knows ?o . ?o schema:name ?n }";
        let undeclared = find_undeclared_prefixes(sparql);
        assert!(undeclared.contains(&"foaf".to_string()), "{:?}", undeclared);
        assert!(
            undeclared.contains(&"schema".to_string()),
            "{:?}",
            undeclared
        );
    }

    #[test]
    fn scanner_skips_http_scheme() {
        // `http:` in an IRI literal must not be treated as a prefix.
        let sparql = "SELECT * WHERE { ?s <http://example.org/p> ?o }";
        let undeclared = find_undeclared_prefixes(sparql);
        assert!(
            !undeclared.contains(&"http".to_string()),
            "{:?}",
            undeclared
        );
    }

    #[test]
    fn scanner_skips_string_content() {
        // Prefix-like text inside string literals must not be extracted.
        let sparql = r#"SELECT * WHERE { ?s ?p "foaf:name is not a prefix here" }"#;
        let undeclared = find_undeclared_prefixes(sparql);
        assert!(
            !undeclared.contains(&"foaf".to_string()),
            "foaf in string should be ignored: {:?}",
            undeclared
        );
    }

    #[test]
    fn scanner_skips_iri_content() {
        // Prefix-like labels inside `<…>` must not be extracted.
        let sparql = "SELECT * WHERE { ?s <http://xmlns.com/foaf/0.1/knows> ?o }";
        let undeclared = find_undeclared_prefixes(sparql);
        assert!(
            !undeclared.contains(&"foaf".to_string()),
            "foaf inside IRI should be ignored: {:?}",
            undeclared
        );
    }

    #[test]
    fn scanner_skips_comment_content() {
        let sparql = "# foaf:name used here\nSELECT * WHERE { ?s ?p ?o }";
        let undeclared = find_undeclared_prefixes(sparql);
        assert!(
            !undeclared.contains(&"foaf".to_string()),
            "foaf in comment should be ignored: {:?}",
            undeclared
        );
    }

    #[test]
    fn scanner_handles_update() {
        let sparql = "INSERT DATA { <http://example.org/s> foaf:name \"Alice\" }";
        let undeclared = find_undeclared_prefixes(sparql);
        assert!(undeclared.contains(&"foaf".to_string()), "{:?}", undeclared);
    }

    #[test]
    fn scanner_partial_decl() {
        // PREFIX ex: declared but foaf: not declared.
        let sparql = "PREFIX ex: <http://example.org/>\n\
                      SELECT * WHERE { ex:Alice foaf:knows ex:Bob }";
        let undeclared = find_undeclared_prefixes(sparql);
        assert!(
            !undeclared.contains(&"ex".to_string()),
            "ex is declared: {:?}",
            undeclared
        );
        assert!(undeclared.contains(&"foaf".to_string()), "{:?}", undeclared);
    }
}
