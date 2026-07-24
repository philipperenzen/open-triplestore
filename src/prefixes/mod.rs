//! Internal prefix registry with a bundled prefix.cc snapshot.
//!
//! # Design
//!
//! SPARQL queries frequently use well-known namespace prefixes like `foaf:`,
//! `schema:`, or `owl:` without declaring them.  This module resolves those
//! prefixes **locally**: a compile-time embedded snapshot of the full
//! prefix.cc dataset merged with the LOV catalog prefixes (see [`dataset`]),
//! overlaid with the prefixes of models/vocabularies registered on this
//! instance.  The public prefix.cc service is only contacted when the
//! operator explicitly opts in (`PREFIX_CC_FALLBACK=true`) and a label is
//! unknown to every local tier — by default the platform makes no third-party
//! calls for prefix resolution.
//!
//! ## Resolution order
//!
//! 1. **Platform** — prefixes derived from the model/vocabulary registry
//!    (kept fresh by the registry seed and model mutations).
//! 2. **Bundled dataset** — the prefix.cc + LOV snapshot (~3.7k prefixes).
//! 3. **Local cache** — mappings confirmed earlier (persisted JSON).
//! 4. **prefix.cc network fallback** — opt-in only.
//!
//! ## How auto-resolution works
//!
//! Before a SPARQL query reaches the store engine, [`find_undeclared_prefixes`]
//! scans the query text for prefix usages that lack a matching `PREFIX`
//! declaration.  Each undeclared prefix is looked up in the registry and any
//! resolved prefix is prepended to the query as a standard
//! `PREFIX label: <IRI>` declaration.
//!
//! ## Security
//!
//! * **Label validation** — prefix labels are checked against
//!   `[A-Za-z][A-Za-z0-9_-]*` (max 64 chars) before any lookup,
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

pub mod dataset;
pub mod routes;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use url::Url;

pub use dataset::{PrefixDataset, PrefixSource};

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

/// Registry-derived prefixes (models/vocabularies on this instance).
#[derive(Default)]
struct PlatformPrefixes {
    by_label: HashMap<String, String>,
    by_iri: HashMap<String, String>,
}

/// A resolved prefix with its provenance, as served by the HTTP API.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedPrefix {
    pub prefix: String,
    pub namespace: String,
    pub source: PrefixSource,
}

// ─── PrefixRegistry ──────────────────────────────────────────────────────────

/// Thread-safe, local-first prefix registry.
///
/// # Thread safety
///
/// `PrefixRegistry` is `Send + Sync`.  Internal state is protected by locks
/// that are *never* held across `.await` points, so it is safe to use from an
/// async context.
pub struct PrefixRegistry {
    cache_path: PathBuf,
    client: reqwest::Client,
    state: Mutex<RuntimeState>,
    dataset: PrefixDataset,
    platform: RwLock<PlatformPrefixes>,
    /// Contact live prefix.cc for unknown labels (PREFIX_CC_FALLBACK=true).
    allow_network: bool,
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
    /// Loads the bundled prefix dataset and, if the cache file exists and is
    /// valid JSON, the persisted lookup cache.  `allow_network` enables the
    /// live prefix.cc fallback for labels unknown to every local tier.
    pub fn open(cache_path: impl AsRef<Path>, allow_network: bool) -> anyhow::Result<Self> {
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
            dataset: PrefixDataset::bundled(),
            platform: RwLock::new(PlatformPrefixes::default()),
            allow_network,
        })
    }

    /// Create a no-op, purely in-memory registry with no backing file, no
    /// bundled dataset and no network (for tests that expect lookups to fail).
    pub fn empty() -> Self {
        Self {
            cache_path: std::path::PathBuf::new(),
            client: reqwest::Client::new(),
            state: Mutex::new(RuntimeState::default()),
            dataset: PrefixDataset::empty(),
            platform: RwLock::new(PlatformPrefixes::default()),
            allow_network: false,
        }
    }

    /// In-memory registry with the bundled dataset but no cache file and no
    /// network — the standard constructor for tests exercising local lookups.
    pub fn bundled_only() -> Self {
        Self {
            cache_path: std::path::PathBuf::new(),
            client: reqwest::Client::new(),
            state: Mutex::new(RuntimeState::default()),
            dataset: PrefixDataset::bundled(),
            platform: RwLock::new(PlatformPrefixes::default()),
            allow_network: false,
        }
    }

    /// Number of prefixes in the bundled dataset.
    pub fn dataset_len(&self) -> usize {
        self.dataset.len()
    }

    fn read_platform(&self) -> std::sync::RwLockReadGuard<'_, PlatformPrefixes> {
        self.platform.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Replace the platform prefix overlay with `pairs` (label, namespace).
    ///
    /// Invalid labels/namespaces are dropped so downstream lookups never see
    /// entries the SPARQL auto-resolver would reject anyway.
    pub fn set_platform_prefixes(&self, pairs: impl IntoIterator<Item = (String, String)>) {
        let mut by_label = HashMap::new();
        let mut by_iri = HashMap::new();
        for (label, iri) in pairs {
            if !is_valid_label(&label) || !is_valid_iri(&iri) {
                continue;
            }
            by_iri.entry(iri.clone()).or_insert_with(|| label.clone());
            by_label.insert(label, iri);
        }
        let mut platform = self.platform.write().unwrap_or_else(|e| e.into_inner());
        *platform = PlatformPrefixes { by_label, by_iri };
    }

    // ── Local resolution ─────────────────────────────────────────────────────

    /// Resolve `label` from the local tiers only (platform → dataset → cache).
    pub fn lookup_local(&self, label: &str) -> Option<ResolvedPrefix> {
        if !is_valid_label(label) {
            return None;
        }
        if let Some(iri) = self.read_platform().by_label.get(label) {
            return Some(ResolvedPrefix {
                prefix: label.to_string(),
                namespace: iri.clone(),
                source: PrefixSource::Platform,
            });
        }
        if let Some(entry) = self.dataset.lookup(label) {
            return Some(ResolvedPrefix {
                prefix: entry.prefix.clone(),
                namespace: entry.namespace.clone(),
                source: entry.source,
            });
        }
        let state = self.lock_state();
        state.cache.by_label.get(label).map(|iri| ResolvedPrefix {
            prefix: label.to_string(),
            namespace: iri.clone(),
            source: PrefixSource::Cache,
        })
    }

    /// Resolve a namespace IRI from the local tiers only.
    pub fn reverse_local(&self, iri: &str) -> Option<ResolvedPrefix> {
        if !is_valid_iri(iri) {
            return None;
        }
        if let Some(label) = self.read_platform().by_iri.get(iri) {
            return Some(ResolvedPrefix {
                prefix: label.clone(),
                namespace: iri.to_string(),
                source: PrefixSource::Platform,
            });
        }
        if let Some(entry) = self.dataset.reverse(iri) {
            return Some(ResolvedPrefix {
                prefix: entry.prefix.clone(),
                namespace: entry.namespace.clone(),
                source: entry.source,
            });
        }
        let state = self.lock_state();
        state.cache.by_iri.get(iri).map(|label| ResolvedPrefix {
            prefix: label.clone(),
            namespace: iri.to_string(),
            source: PrefixSource::Cache,
        })
    }

    /// Ranked prefix search over platform + bundled entries.
    pub fn search(&self, query: &str, limit: usize) -> Vec<ResolvedPrefix> {
        let q = query.trim().to_ascii_lowercase();
        let mut out: Vec<ResolvedPrefix> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        {
            let platform = self.read_platform();
            let mut platform_hits: Vec<(&String, &String)> = platform
                .by_label
                .iter()
                .filter(|(label, iri)| {
                    q.is_empty()
                        || label.to_ascii_lowercase().contains(&q)
                        || iri.to_ascii_lowercase().contains(&q)
                })
                .collect();
            platform_hits.sort_by(|a, b| a.0.cmp(b.0));
            for (label, iri) in platform_hits.into_iter().take(limit) {
                seen.insert(label.clone());
                out.push(ResolvedPrefix {
                    prefix: label.clone(),
                    namespace: iri.clone(),
                    source: PrefixSource::Platform,
                });
            }
        }

        for entry in self.dataset.search(&q, limit + out.len()) {
            if out.len() >= limit {
                break;
            }
            if seen.contains(&entry.prefix) {
                continue;
            }
            out.push(ResolvedPrefix {
                prefix: entry.prefix.clone(),
                namespace: entry.namespace.clone(),
                source: entry.source,
            });
        }
        out
    }

    /// All known prefixes (platform overlay first, then the bundled dataset,
    /// then cache-confirmed extras), deduplicated by label.
    pub fn all_prefixes(&self) -> Vec<ResolvedPrefix> {
        let mut out = Vec::with_capacity(self.dataset.len() + 16);
        let mut seen: HashSet<String> = HashSet::new();
        {
            let platform = self.read_platform();
            let mut labels: Vec<_> = platform.by_label.iter().collect();
            labels.sort_by(|a, b| a.0.cmp(b.0));
            for (label, iri) in labels {
                seen.insert(label.clone());
                out.push(ResolvedPrefix {
                    prefix: label.clone(),
                    namespace: iri.clone(),
                    source: PrefixSource::Platform,
                });
            }
        }
        for entry in self.dataset.entries() {
            if seen.insert(entry.prefix.clone()) {
                out.push(ResolvedPrefix {
                    prefix: entry.prefix.clone(),
                    namespace: entry.namespace.clone(),
                    source: entry.source,
                });
            }
        }
        {
            let state = self.lock_state();
            let mut cached: Vec<_> = state.cache.by_label.iter().collect();
            cached.sort_by(|a, b| a.0.cmp(b.0));
            for (label, iri) in cached {
                if seen.insert(label.clone()) {
                    out.push(ResolvedPrefix {
                        prefix: label.clone(),
                        namespace: iri.clone(),
                        source: PrefixSource::Cache,
                    });
                }
            }
        }
        out
    }

    /// Expand a CURIE like `foaf:name` to a full IRI using local tiers.
    pub fn expand_curie(&self, curie: &str) -> Option<String> {
        let (label, local) = curie.split_once(':')?;
        if local.contains(['<', '>', '"', ' ', '\n', '\t']) {
            return None;
        }
        let resolved = self.lookup_local(label)?;
        Some(format!("{}{}", resolved.namespace, local))
    }

    /// Shrink a full IRI to `prefix:localName` using the longest known
    /// namespace (platform overlay wins over the bundled dataset).
    pub fn shrink_iri(&self, iri: &str) -> Option<(ResolvedPrefix, String)> {
        if !is_valid_iri(iri) {
            return None;
        }
        let mut best: Option<(ResolvedPrefix, String)> = None;
        {
            let platform = self.read_platform();
            for (ns, label) in platform.by_iri.iter() {
                if iri.starts_with(ns.as_str()) && iri.len() > ns.len() {
                    let better = match &best {
                        Some((b, _)) => ns.len() > b.namespace.len(),
                        None => true,
                    };
                    if better {
                        best = Some((
                            ResolvedPrefix {
                                prefix: label.clone(),
                                namespace: ns.clone(),
                                source: PrefixSource::Platform,
                            },
                            iri[ns.len()..].to_string(),
                        ));
                    }
                }
            }
        }
        if let Some((entry, local)) = self.dataset.shrink(iri) {
            let better = match &best {
                Some((b, _)) => entry.namespace.len() > b.namespace.len(),
                None => true,
            };
            if better {
                best = Some((
                    ResolvedPrefix {
                        prefix: entry.prefix.clone(),
                        namespace: entry.namespace.clone(),
                        source: entry.source,
                    },
                    local,
                ));
            }
        }
        best
    }

    // ── Forward lookup ───────────────────────────────────────────────────────

    /// Return the namespace IRI for a prefix `label`.
    ///
    /// Resolution order: platform → bundled dataset → cache → live prefix.cc
    /// (only when the registry was opened with `allow_network`).
    ///
    /// Returns `None` when the label is invalid or unknown to every tier.
    pub async fn lookup_prefix(&self, label: &str) -> Option<String> {
        // Security: validate label before any lookup.
        if !is_valid_label(label) {
            debug!("Skipping invalid prefix label {:?}", label);
            return None;
        }

        if let Some(resolved) = self.lookup_local(label) {
            return Some(resolved.namespace);
        }

        if !self.allow_network {
            return None;
        }

        // Network fallback path (opt-in).
        {
            let state = self.lock_state();
            if state.not_found.contains(label) {
                return None;
            }
            if *state.failures.get(label).unwrap_or(&0) >= CIRCUIT_BREAKER_THRESHOLD {
                debug!("Circuit breaker open for prefix '{}'", label);
                return None;
            }
        } // mutex released before await

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
    /// Resolution order: platform → bundled dataset → cache → live prefix.cc
    /// (only when opened with `allow_network`).
    pub async fn reverse_lookup(&self, iri: &str) -> Option<(String, String)> {
        // Security: validate IRI before any lookup.
        if !is_valid_iri(iri) {
            debug!("Skipping invalid IRI for reverse lookup: {:?}", iri);
            return None;
        }

        if let Some(resolved) = self.reverse_local(iri) {
            return Some((resolved.prefix, resolved.namespace));
        }

        if !self.allow_network {
            return None;
        }

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
        if self.cache_path.as_os_str().is_empty() {
            return;
        }
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

    // ── Local resolution tiers ───────────────────────────────────────────────

    #[tokio::test]
    async fn bundled_lookup_without_network() {
        let reg = PrefixRegistry::bundled_only();
        assert_eq!(
            reg.lookup_prefix("foaf").await.as_deref(),
            Some("http://xmlns.com/foaf/0.1/")
        );
        assert_eq!(
            reg.reverse_lookup("http://xmlns.com/foaf/0.1/")
                .await
                .map(|(l, _)| l)
                .as_deref(),
            Some("foaf")
        );
        // Unknown label: no network, so None.
        assert!(reg.lookup_prefix("zzz-not-a-prefix").await.is_none());
    }

    #[tokio::test]
    async fn empty_registry_resolves_nothing() {
        let reg = PrefixRegistry::empty();
        assert!(reg.lookup_prefix("foaf").await.is_none());
    }

    #[test]
    fn platform_overlay_wins_over_dataset() {
        let reg = PrefixRegistry::bundled_only();
        reg.set_platform_prefixes([(
            "foaf".to_string(),
            "https://example.org/custom-foaf#".to_string(),
        )]);
        let hit = reg.lookup_local("foaf").expect("resolves");
        assert_eq!(hit.namespace, "https://example.org/custom-foaf#");
        assert_eq!(hit.source, PrefixSource::Platform);
    }

    #[test]
    fn platform_overlay_rejects_invalid_entries() {
        // Empty registry: only the platform overlay can answer, so a rejected
        // entry resolving would be unambiguous (bundled_only() would mask it —
        // prefix.cc knows an "ok" prefix).
        let reg = PrefixRegistry::empty();
        reg.set_platform_prefixes([
            ("bad label".to_string(), "http://example.org/".to_string()),
            ("ok".to_string(), "javascript:alert(1)".to_string()),
            ("good".to_string(), "http://example.org/good#".to_string()),
        ]);
        assert!(reg.lookup_local("good").is_some());
        assert!(reg.lookup_local("ok").is_none());
        assert!(reg.lookup_local("bad label").is_none());
    }

    #[test]
    fn expand_and_shrink_roundtrip() {
        let reg = PrefixRegistry::bundled_only();
        assert_eq!(
            reg.expand_curie("foaf:name").as_deref(),
            Some("http://xmlns.com/foaf/0.1/name")
        );
        let (resolved, local) = reg.shrink_iri("http://xmlns.com/foaf/0.1/name").unwrap();
        assert_eq!(resolved.prefix, "foaf");
        assert_eq!(local, "name");
        assert!(reg.expand_curie("foaf:na me").is_none());
        assert!(reg.expand_curie("nocolon").is_none());
    }

    #[test]
    fn search_returns_ranked_hits() {
        let reg = PrefixRegistry::bundled_only();
        let hits = reg.search("foaf", 5);
        assert_eq!(hits[0].prefix, "foaf");
        // Platform entries surface ahead of bundled ones.
        reg.set_platform_prefixes([(
            "foafx".to_string(),
            "https://example.org/foafx#".to_string(),
        )]);
        let hits = reg.search("foaf", 5);
        assert_eq!(hits[0].source, PrefixSource::Platform);
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
