//! Replication on the change log (P4): logical log shipping.
//!
//! A **leader** records every write in its change log
//! (`src/store/changes.rs`; the leader role switches capture on) and serves
//! the rows at `GET /api/admin/changes`, a manifest of its graphs and
//! datasets at `GET /api/replication/manifest`, and every graph through the
//! Graph Store. A **follower** tails the rows with a cursor and applies them:
//!
//! - a `full` row is applied as a delta — the added quads inserted, the
//!   removed ones removed, in one transaction, the count index adjusted by
//!   what actually changed, and the row's `post_count` compared with the
//!   result so a divergence heals itself by fetching the graph;
//! - a `counts` or `unknown` row says "graph X changed at seq N" and the
//!   follower fetches the graph and replaces its copy (a bulk load on the
//!   leader is one such row per graph, not nine million quads in a log);
//! - a store-scoped row, or an epoch that is not the one the follower
//!   adopted (a promoted follower, a restore, a quarantine), resynchronises
//!   every graph in scope from the manifest — and drops the ones the leader
//!   no longer has;
//! - after every page the follower bookmarks its position locally and on the
//!   leader (`PUT /api/admin/changes/cursors/<node>`), so the leader's
//!   retention never sweeps a row the follower still needs.
//!
//! **Temperature** changes only how often the follower asks: `cold` every
//! hour, `warm` every minute, `hot` every poll (500 ms). **Scope** changes
//! what it applies: every graph, a list of graphs, or the graphs of a list of
//! the leader's datasets (resolved through the manifest at each catch-up).
//! A follower is **read-only**: every mutation primitive refuses with
//! [`StoreError::ReadOnly`] (a 503 over HTTP), except what the follower
//! itself applies. The synchronous and consensus variants of "hot" are not
//! built here; see `docs/operations.md (Replication)`.
//!
//! The follower's HTTP client is [`HttpLeader`]; tests drive
//! [`TripleStore::replicate_once`] against an [`InProcessLeader`].

use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use oxigraph::io::RdfFormat;

use crate::store::changes::{self, ChangeRow, Extent, State};
use crate::store::engine::{StoreError, TripleStore};

// ─── Configuration ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    None,
    Leader,
    Follower,
}

/// How often a follower asks. Nothing else differs between the temperatures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// A disaster-recovery copy: catch up every hour.
    Cold,
    /// A reporting replica: catch up every minute.
    Warm,
    /// A read replica: catch up every poll (500 ms), asynchronously.
    Hot,
}

impl Mode {
    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "cold" => Mode::Cold,
            "hot" => Mode::Hot,
            _ => Mode::Warm,
        }
    }

    pub fn default_interval(self, poll: Duration) -> Duration {
        match self {
            Mode::Cold => Duration::from_secs(3600),
            Mode::Warm => Duration::from_secs(60),
            Mode::Hot => poll,
        }
    }
}

/// What a follower applies. Rows for anything else advance the cursor and
/// are otherwise ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    /// Graph IRIs; `None` is the default graph (`default` in the environment).
    Graphs(Vec<Option<String>>),
    /// The leader's dataset ids, resolved to graphs through the manifest.
    Datasets(Vec<String>),
}

impl Scope {
    fn describe(&self) -> String {
        match self {
            Scope::All => "all".to_string(),
            Scope::Graphs(g) => format!("graphs: {}", g.len()),
            Scope::Datasets(d) => format!("datasets: {}", d.join(",")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReplicationConfig {
    pub role: Role,
    pub mode: Mode,
    pub scope: Scope,
    pub leader_url: Option<String>,
    pub token: Option<String>,
    /// This node's name: the cursor it keeps on the leader.
    pub node_id: String,
    /// The hot poll period.
    pub poll: Duration,
    /// How often the follower catches up (the temperature, or an override).
    pub interval: Duration,
    /// Rows per page.
    pub page: usize,
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// True when `OTS_REPLICATION_ROLE=leader`: the change log stays on.
pub fn leader_role_configured() -> bool {
    env_opt("OTS_REPLICATION_ROLE")
        .map(|r| r.eq_ignore_ascii_case("leader") || r.eq_ignore_ascii_case("primary"))
        .unwrap_or(false)
}

/// True when `OTS_REPLICATION_ROLE=follower`: the change log stays off
/// unless asked for (a follower's log is not a source).
pub fn follower_role_configured() -> bool {
    env_opt("OTS_REPLICATION_ROLE")
        .map(|r| r.eq_ignore_ascii_case("follower") || r.eq_ignore_ascii_case("replica"))
        .unwrap_or(false)
}

impl ReplicationConfig {
    pub fn none() -> Self {
        Self::parse("none", "warm", None, None, None, None, None, None, None)
    }

    pub fn leader() -> Self {
        Self::parse("leader", "warm", None, None, None, None, None, None, None)
    }

    /// A follower of `leader_url` (tests; the environment does the same).
    pub fn follower(leader_url: &str, mode: Mode, scope: Scope) -> Self {
        let mut c = Self::parse(
            "follower",
            match mode {
                Mode::Cold => "cold",
                Mode::Warm => "warm",
                Mode::Hot => "hot",
            },
            None,
            None,
            Some(leader_url),
            None,
            None,
            None,
            None,
        );
        c.scope = scope;
        c
    }

    /// `OTS_REPLICATION_ROLE`, `_MODE`, `_GRAPHS`, `_DATASETS`, `_LEADER_URL`,
    /// `_TOKEN`, `_NODE_ID`, `_POLL_MS`, `_INTERVAL_SECS`.
    pub fn from_env() -> Self {
        Self::parse(
            env_opt("OTS_REPLICATION_ROLE").as_deref().unwrap_or("none"),
            env_opt("OTS_REPLICATION_MODE").as_deref().unwrap_or("warm"),
            env_opt("OTS_REPLICATION_GRAPHS").as_deref(),
            env_opt("OTS_REPLICATION_DATASETS").as_deref(),
            env_opt("OTS_REPLICATION_LEADER_URL").as_deref(),
            env_opt("OTS_REPLICATION_TOKEN").as_deref(),
            env_opt("OTS_REPLICATION_NODE_ID").as_deref(),
            env_opt("OTS_REPLICATION_POLL_MS").as_deref(),
            env_opt("OTS_REPLICATION_INTERVAL_SECS").as_deref(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn parse(
        role: &str,
        mode: &str,
        graphs: Option<&str>,
        datasets: Option<&str>,
        leader_url: Option<&str>,
        token: Option<&str>,
        node_id: Option<&str>,
        poll_ms: Option<&str>,
        interval_secs: Option<&str>,
    ) -> Self {
        let role = match role.trim().to_ascii_lowercase().as_str() {
            "leader" | "primary" => Role::Leader,
            "follower" | "replica" => Role::Follower,
            _ => Role::None,
        };
        let mode = Mode::parse(mode);
        let list = |s: &str| -> Vec<String> {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        };
        let scope = match (datasets, graphs) {
            (Some(d), _) if !list(d).is_empty() => Scope::Datasets(list(d)),
            (_, Some(g)) if !g.trim().eq_ignore_ascii_case("all") && !list(g).is_empty() => {
                Scope::Graphs(
                    list(g)
                        .into_iter()
                        .map(|g| {
                            if g.eq_ignore_ascii_case("default") {
                                None
                            } else {
                                Some(g)
                            }
                        })
                        .collect(),
                )
            }
            _ => Scope::All,
        };
        let poll = Duration::from_millis(
            poll_ms
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(500)
                .clamp(50, 60_000),
        );
        let interval = interval_secs
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or_else(|| mode.default_interval(poll));
        let node_id = node_id
            .map(str::to_string)
            .or_else(|| {
                std::env::var("HOSTNAME")
                    .ok()
                    .filter(|h| !h.trim().is_empty())
            })
            .unwrap_or_else(|| "follower".to_string());
        Self {
            role,
            mode,
            scope,
            leader_url: leader_url.map(|u| u.trim().trim_end_matches('/').to_string()),
            token: token.map(str::to_string),
            node_id,
            poll,
            interval,
            page: 500,
        }
    }
}

// ─── The follower's state ───────────────────────────────────────────────────

/// Where the follower stands: the leader epoch it adopted and the last
/// sequence number it applied. Persisted beside a persistent store.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Bookmark {
    pub epoch: Option<String>,
    pub applied_seq: i64,
    pub synced_at: Option<String>,
}

#[derive(Default)]
struct LastSync {
    at: Option<String>,
    ok_at: Option<Instant>,
    error: Option<String>,
    leader_newest_seq: Option<i64>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Status {
    pub role: Role,
    pub mode: Mode,
    pub scope: String,
    pub leader_url: Option<String>,
    pub node_id: String,
    pub read_only: bool,
    pub epoch: Option<String>,
    pub applied_seq: i64,
    pub leader_newest_seq: Option<i64>,
    /// Rows the leader has that this node has not applied (as of the last
    /// contact).
    pub lag_rows: Option<i64>,
    pub last_sync_at: Option<String>,
    pub last_error: Option<String>,
    pub applied_rows: u64,
    pub refetched_graphs: u64,
    pub resyncs: u64,
    pub interval_secs: f64,
    /// A leader or an unconfigured node is healthy; a follower is healthy
    /// when its last successful catch-up is younger than three intervals.
    pub healthy: bool,
}

pub struct Replication {
    config: ReplicationConfig,
    bookmark: Mutex<Bookmark>,
    path: Option<PathBuf>,
    last: Mutex<LastSync>,
    applied_rows: AtomicU64,
    refetched_graphs: AtomicU64,
    resyncs: AtomicU64,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

impl Replication {
    /// From the environment; the bookmark from `{dir}/replication.json` when
    /// the store is persistent.
    pub fn from_env(dir: Option<&Path>) -> Self {
        Self::with_config(ReplicationConfig::from_env(), dir)
    }

    pub fn with_config(config: ReplicationConfig, dir: Option<&Path>) -> Self {
        let path = dir.map(|d| d.join("replication.json"));
        let bookmark = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Bookmark>(&s).ok())
            .unwrap_or_default();
        Self {
            config,
            bookmark: Mutex::new(bookmark),
            path,
            last: Mutex::new(LastSync::default()),
            applied_rows: AtomicU64::new(0),
            refetched_graphs: AtomicU64::new(0),
            resyncs: AtomicU64::new(0),
        }
    }

    pub fn disabled() -> Self {
        Self::with_config(ReplicationConfig::none(), None)
    }

    pub fn config(&self) -> &ReplicationConfig {
        &self.config
    }

    pub fn role(&self) -> Role {
        self.config.role
    }

    /// A follower refuses every write that is not its own replication.
    pub fn read_only(&self) -> bool {
        self.config.role == Role::Follower
    }

    pub fn bookmark(&self) -> Bookmark {
        self.bookmark
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn set_bookmark(&self, epoch: Option<String>, applied_seq: i64) {
        let snapshot = {
            let mut b = self.bookmark.lock().unwrap_or_else(|p| p.into_inner());
            b.epoch = epoch;
            b.applied_seq = applied_seq;
            b.synced_at = Some(now());
            b.clone()
        };
        if let Some(path) = &self.path {
            // Atomic on the same filesystem: write, then rename over.
            let tmp = path.with_extension("json.tmp");
            if let Ok(text) = serde_json::to_string_pretty(&snapshot) {
                if std::fs::write(&tmp, text).is_ok() {
                    let _ = std::fs::rename(&tmp, path);
                }
            }
        }
    }

    fn record_ok(&self, leader_newest_seq: i64) {
        let mut l = self.last.lock().unwrap_or_else(|p| p.into_inner());
        l.at = Some(now());
        l.ok_at = Some(Instant::now());
        l.error = None;
        l.leader_newest_seq = Some(leader_newest_seq);
    }

    fn record_error(&self, e: &str) {
        let mut l = self.last.lock().unwrap_or_else(|p| p.into_inner());
        l.at = Some(now());
        l.error = Some(e.to_string());
    }

    pub fn status(&self) -> Status {
        let b = self.bookmark();
        let l = self.last.lock().unwrap_or_else(|p| p.into_inner());
        let healthy = match self.config.role {
            Role::Follower => l
                .ok_at
                .map(|t| t.elapsed() <= self.config.interval * 3 + Duration::from_secs(5))
                .unwrap_or(false),
            _ => true,
        };
        Status {
            role: self.config.role,
            mode: self.config.mode,
            scope: self.config.scope.describe(),
            leader_url: self.config.leader_url.clone(),
            node_id: self.config.node_id.clone(),
            read_only: self.read_only(),
            epoch: b.epoch.clone(),
            applied_seq: b.applied_seq,
            leader_newest_seq: l.leader_newest_seq,
            lag_rows: l.leader_newest_seq.map(|n| (n - b.applied_seq).max(0)),
            last_sync_at: l.at.clone(),
            last_error: l.error.clone(),
            applied_rows: self.applied_rows.load(Ordering::Relaxed),
            refetched_graphs: self.refetched_graphs.load(Ordering::Relaxed),
            resyncs: self.resyncs.load(Ordering::Relaxed),
            interval_secs: self.config.interval.as_secs_f64(),
            healthy,
        }
    }
}

// ─── Passing the read-only check ────────────────────────────────────────────

thread_local! {
    static APPLYING: Cell<bool> = const { Cell::new(false) };
}

/// While alive on this thread, the follower's own writes pass the read-only
/// check. Set by the replication primitives, never by a handler.
pub struct ApplyGuard(bool);

impl ApplyGuard {
    pub fn enter() -> Self {
        Self(APPLYING.with(|a| a.replace(true)))
    }
}

impl Drop for ApplyGuard {
    fn drop(&mut self) {
        APPLYING.with(|a| a.set(self.0));
    }
}

pub(crate) fn applying() -> bool {
    APPLYING.with(|a| a.get())
}

// ─── What a follower reads from its leader ──────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DatasetGraphs {
    pub id: String,
    pub graphs: Vec<String>,
}

/// The leader as a follower sees it: its epoch and position, every graph it
/// holds (`null` is the default graph), and its datasets with their graphs.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub epoch: String,
    pub newest_seq: i64,
    pub capture_enabled: bool,
    pub graphs: Vec<Option<String>>,
    pub datasets: Vec<DatasetGraphs>,
}

/// The store half of the manifest; the handler adds the datasets.
pub fn manifest_of(store: &TripleStore, datasets: Vec<DatasetGraphs>) -> Manifest {
    let log = store.changes();
    let mut graphs: Vec<Option<String>> =
        store.graph_counts().into_iter().map(|(g, _)| g).collect();
    graphs.sort();
    graphs.dedup();
    Manifest {
        epoch: log.epoch().to_string(),
        newest_seq: log.last_seq(),
        capture_enabled: log.enabled(),
        graphs,
        datasets,
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Page {
    pub epoch: String,
    pub rows: Vec<ChangeRow>,
    pub next_after: i64,
}

/// A leader, however reached.
pub trait LeaderSource: Send + Sync {
    fn manifest(&self) -> Result<Manifest, String>;
    fn changes_after(&self, after: i64, limit: usize) -> Result<Page, String>;
    /// The graph as N-Triples; `Ok(None)` when the leader does not have it.
    fn graph_ntriples(&self, graph: Option<&str>) -> Result<Option<String>, String>;
    fn set_cursor(&self, name: &str, seq: i64) -> Result<(), String>;
}

/// The leader in the same process: tests, and the shape every source must
/// have.
#[cfg(any(test, feature = "test-utils"))]
#[allow(dead_code)]
pub struct InProcessLeader {
    store: TripleStore,
    datasets: Vec<DatasetGraphs>,
}

#[cfg(any(test, feature = "test-utils"))]
#[allow(dead_code)]
impl InProcessLeader {
    pub fn new(store: TripleStore) -> Self {
        Self {
            store,
            datasets: Vec::new(),
        }
    }

    pub fn with_datasets(mut self, datasets: Vec<DatasetGraphs>) -> Self {
        self.datasets = datasets;
        self
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl LeaderSource for InProcessLeader {
    fn manifest(&self) -> Result<Manifest, String> {
        Ok(manifest_of(&self.store, self.datasets.clone()))
    }

    fn changes_after(&self, after: i64, limit: usize) -> Result<Page, String> {
        let rows = self.store.changes().rows_after(after, limit);
        let next_after = rows.last().and_then(|r| r.seq).unwrap_or(after);
        Ok(Page {
            epoch: self.store.changes().epoch().to_string(),
            rows,
            next_after,
        })
    }

    fn graph_ntriples(&self, graph: Option<&str>) -> Result<Option<String>, String> {
        if graph.is_some() && self.store.graph_count_cached(graph).is_none() {
            return Ok(None);
        }
        let bytes = self
            .store
            .graph_store_get(graph, RdfFormat::NTriples)
            .map_err(|e| e.to_string())?;
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    fn set_cursor(&self, name: &str, seq: i64) -> Result<(), String> {
        self.store
            .changes()
            .set_cursor(name, seq, Some("in-process"))
            .map_err(|e| e.to_string())
    }
}

// ─── The leader over HTTP ───────────────────────────────────────────────────

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("replication runtime")
    })
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!(
                "open-triplestore-replica/",
                env!("CARGO_PKG_VERSION")
            ))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client")
    })
}

/// Run a request to completion from any thread — the follower thread, or a
/// blocking task inside the server's runtime.
fn blocking<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    std::thread::scope(|s| {
        s.spawn(|| runtime().block_on(fut))
            .join()
            .expect("replication request thread")
    })
}

fn encode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The leader named by `OTS_REPLICATION_LEADER_URL`, called with
/// `OTS_REPLICATION_TOKEN` (an admin API token minted on the leader).
pub struct HttpLeader {
    base: String,
    token: Option<String>,
    timeout: Duration,
}

impl HttpLeader {
    pub fn new(base: &str, token: Option<&str>) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            token: token.map(str::to_string),
            timeout: crate::remote::timeout().max(Duration::from_secs(30)),
        }
    }

    pub fn from_config(config: &ReplicationConfig) -> Option<Self> {
        config
            .leader_url
            .as_deref()
            .map(|u| Self::new(u, config.token.as_deref()))
    }

    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        accept: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Option<String>, String> {
        let url = format!("{}{}", self.base, path);
        let token = self.token.clone();
        let timeout = self.timeout;
        blocking(async move {
            let mut req = client()
                .request(method, &url)
                .timeout(timeout)
                .header("Accept", accept);
            if let Some(t) = &token {
                req = req.bearer_auth(t);
            }
            if let Some(b) = body {
                req = req.json(&b);
            }
            let resp = req.send().await.map_err(|e| format!("{url}: {e}"))?;
            let status = resp.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            if !status.is_success() {
                return Err(format!("{url}: HTTP {}", status.as_u16()));
            }
            resp.text()
                .await
                .map(Some)
                .map_err(|e| format!("{url}: {e}"))
        })
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let text = self
            .request(reqwest::Method::GET, path, "application/json", None)?
            .ok_or_else(|| format!("{}{path}: HTTP 404", self.base))?;
        serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
    }
}

impl LeaderSource for HttpLeader {
    fn manifest(&self) -> Result<Manifest, String> {
        self.get_json("/api/replication/manifest")
    }

    fn changes_after(&self, after: i64, limit: usize) -> Result<Page, String> {
        self.get_json(&format!("/api/admin/changes?after={after}&limit={limit}"))
    }

    fn graph_ntriples(&self, graph: Option<&str>) -> Result<Option<String>, String> {
        let path = match graph {
            Some(g) => format!("/store?graph={}", encode(g)),
            None => "/store?default".to_string(),
        };
        self.request(reqwest::Method::GET, &path, "application/n-triples", None)
    }

    fn set_cursor(&self, name: &str, seq: i64) -> Result<(), String> {
        self.request(
            reqwest::Method::PUT,
            &format!("/api/admin/changes/cursors/{}", encode(name)),
            "application/json",
            Some(serde_json::json!({ "seq": seq })),
        )
        .map(|_| ())
    }
}

// ─── Applying ───────────────────────────────────────────────────────────────

/// What one catch-up did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// The first catch-up of a fresh follower: every graph in scope fetched.
    pub bootstrapped: bool,
    /// Full resynchronisations (an epoch change, a store-scoped row).
    pub resyncs: usize,
    /// Rows applied as deltas.
    pub applied_rows: usize,
    /// Graphs fetched whole (`counts` / `unknown` rows, a healed divergence).
    pub refetched_graphs: usize,
    pub applied_seq: i64,
    /// No more rows on the leader when the catch-up ended.
    pub caught_up: bool,
}

/// At most this many pages per catch-up, so a call stays bounded.
const MAX_PAGES: usize = 200;

impl TripleStore {
    /// One catch-up against `source`: adopt the leader's epoch (resynchronise
    /// on a change), then apply pages of rows until caught up. Errors leave
    /// the bookmark where the last complete page put it.
    pub fn replicate_once(&self, source: &dyn LeaderSource) -> Result<Progress, StoreError> {
        let rep = self.replication();
        if rep.role() != Role::Follower {
            return Err(StoreError::Other(
                "replicate_once: this node is not a follower".to_string(),
            ));
        }
        match self.replicate_inner(source) {
            Ok(p) => Ok(p),
            Err(e) => {
                rep.record_error(&e.to_string());
                Err(e)
            }
        }
    }

    fn replicate_inner(&self, source: &dyn LeaderSource) -> Result<Progress, StoreError> {
        let rep = self.replication();
        let config = rep.config().clone();
        let mut progress = Progress::default();

        let manifest = source.manifest().map_err(StoreError::Other)?;
        if !manifest.capture_enabled || manifest.epoch.is_empty() {
            return Err(StoreError::Other(
                "the leader has change capture off (set OTS_REPLICATION_ROLE=leader or OTS_CHANGE_CAPTURE=on there)"
                    .to_string(),
            ));
        }
        let selected = selected_graphs(&config.scope, &manifest);

        // The epoch: adopt it, resynchronising when it is not the one we hold.
        let bookmark = rep.bookmark();
        if bookmark.epoch.as_deref() != Some(manifest.epoch.as_str()) {
            progress.bootstrapped = bookmark.epoch.is_none();
            progress.resyncs += 1;
            rep.resyncs.fetch_add(1, Ordering::Relaxed);
            let fetched = self.resync_all(source, &manifest, &selected)?;
            progress.refetched_graphs += fetched;
            rep.set_bookmark(Some(manifest.epoch.clone()), manifest.newest_seq);
            let _ = source.set_cursor(&config.node_id, manifest.newest_seq);
        }

        // Pages, in sequence order.
        let mut after = rep.bookmark().applied_seq;
        let mut pages = 0;
        loop {
            let page = source
                .changes_after(after, config.page)
                .map_err(StoreError::Other)?;
            if page.epoch != manifest.epoch {
                // The leader changed under us; the next catch-up adopts it.
                rep.set_bookmark(None, 0);
                return Err(StoreError::Other(format!(
                    "leader epoch changed during catch-up ({} -> {}); resynchronising next time",
                    manifest.epoch, page.epoch
                )));
            }
            let n = page.rows.len();
            for row in &page.rows {
                let Some(seq) = row.seq else { continue };
                self.apply_row(source, row, &selected, &manifest, &mut progress)?;
                after = seq;
            }
            if n > 0 {
                rep.set_bookmark(Some(manifest.epoch.clone()), after);
                let _ = source.set_cursor(&config.node_id, after);
            }
            pages += 1;
            if n < config.page {
                progress.caught_up = true;
                break;
            }
            if pages >= MAX_PAGES {
                break;
            }
        }
        progress.applied_seq = after;
        rep.applied_rows
            .fetch_add(progress.applied_rows as u64, Ordering::Relaxed);
        rep.refetched_graphs
            .fetch_add(progress.refetched_graphs as u64, Ordering::Relaxed);
        rep.record_ok(manifest.newest_seq.max(after));
        Ok(progress)
    }

    fn apply_row(
        &self,
        source: &dyn LeaderSource,
        row: &ChangeRow,
        selected: &Option<HashSet<Option<String>>>,
        manifest: &Manifest,
        progress: &mut Progress,
    ) -> Result<(), StoreError> {
        if row.state == State::Aborted || row.state == State::Pending {
            return Ok(());
        }
        if row.scope == "store" {
            // Anything may have changed: every graph in scope, again, from a
            // fresh manifest (the graph list may have changed too).
            let fresh = source.manifest().map_err(StoreError::Other)?;
            let selected = selected_graphs(&self.replication().config().scope, &fresh);
            progress.resyncs += 1;
            self.replication().resyncs.fetch_add(1, Ordering::Relaxed);
            progress.refetched_graphs += self.resync_all(source, &fresh, &selected)?;
            return Ok(());
        }
        let graph = row.graph_iri.clone();
        if let Some(sel) = selected {
            if !sel.contains(&graph) {
                return Ok(());
            }
        }
        let _ = manifest;
        if row.extent == Extent::Full && row.state == State::Committed {
            let added = changes::decode_text(row.added.as_deref());
            let removed = changes::decode_text(row.removed.as_deref());
            self.apply_delta_replicated(graph.as_deref(), &added, &removed)?;
            progress.applied_rows += 1;
            // The row says what the graph should hold now; disagree, and the
            // graph is fetched whole.
            if let Some(expected) = row.post_count {
                let local = self.graph_count_cached(graph.as_deref()).unwrap_or(0) as i64;
                if local != expected {
                    self.refetch_graph(source, graph.as_deref())?;
                    progress.refetched_graphs += 1;
                }
            }
        } else {
            self.refetch_graph(source, graph.as_deref())?;
            progress.refetched_graphs += 1;
        }
        Ok(())
    }

    /// Every graph in scope, fetched whole from the leader; local graphs in
    /// scope the leader no longer lists are dropped. Returns the fetch count.
    fn resync_all(
        &self,
        source: &dyn LeaderSource,
        manifest: &Manifest,
        selected: &Option<HashSet<Option<String>>>,
    ) -> Result<usize, StoreError> {
        let in_scope = |g: &Option<String>| selected.as_ref().is_none_or(|s| s.contains(g));
        let leader_graphs: Vec<Option<String>> = manifest
            .graphs
            .iter()
            .filter(|g| in_scope(g))
            .cloned()
            .collect();
        let mut fetched = 0;
        for g in &leader_graphs {
            self.refetch_graph(source, g.as_deref())?;
            fetched += 1;
        }
        let local: Vec<Option<String>> = self.graph_counts().into_iter().map(|(g, _)| g).collect();
        for g in local {
            if in_scope(&g) && !leader_graphs.contains(&g) {
                let _apply = ApplyGuard::enter();
                self.graph_store_delete(g.as_deref())?;
            }
        }
        Ok(fetched)
    }

    /// Replace the local copy of one graph with the leader's, or drop it when
    /// the leader no longer has it.
    fn refetch_graph(
        &self,
        source: &dyn LeaderSource,
        graph: Option<&str>,
    ) -> Result<(), StoreError> {
        let _apply = ApplyGuard::enter();
        match source.graph_ntriples(graph).map_err(StoreError::Other)? {
            Some(text) => self.graph_store_put(graph, &text, RdfFormat::NTriples),
            None => {
                if graph.is_none() || self.graph_count_cached(graph).is_some() {
                    self.graph_store_delete(graph)
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// The graphs a scope selects, given the leader's manifest; `None` is all.
fn selected_graphs(scope: &Scope, manifest: &Manifest) -> Option<HashSet<Option<String>>> {
    match scope {
        Scope::All => None,
        Scope::Graphs(g) => Some(g.iter().cloned().collect()),
        Scope::Datasets(ids) => Some(
            manifest
                .datasets
                .iter()
                .filter(|d| ids.contains(&d.id))
                .flat_map(|d| d.graphs.iter().map(|g| Some(g.clone())))
                .collect(),
        ),
    }
}

// ─── The follower thread ────────────────────────────────────────────────────

/// Start the follower loop when the environment configures one. Called once
/// by the store constructors; does nothing for a leader or an unconfigured
/// node, or when the leader URL is missing (then the status says so).
pub(crate) fn spawn_follower_if_configured(store: &TripleStore) {
    let rep = store.replication();
    if rep.role() != Role::Follower {
        return;
    }
    let Some(http) = HttpLeader::from_config(rep.config()) else {
        rep.record_error("OTS_REPLICATION_LEADER_URL is not set; this follower cannot catch up");
        tracing::warn!("replication: follower without OTS_REPLICATION_LEADER_URL");
        return;
    };
    let store = store.clone();
    let config = rep.config().clone();
    let spawned = std::thread::Builder::new()
        .name("replication-follower".into())
        .spawn(move || {
            tracing::info!(
                "replication: follower of {} ({:?}, every {:?}, scope {})",
                config.leader_url.as_deref().unwrap_or("?"),
                config.mode,
                config.interval,
                config.scope.describe()
            );
            let mut last_attempt: Option<Instant> = None;
            loop {
                let due = last_attempt
                    .map(|t| t.elapsed() >= config.interval)
                    .unwrap_or(true);
                if due {
                    last_attempt = Some(Instant::now());
                    match store.replicate_once(&http) {
                        Ok(p) if p.applied_rows > 0 || p.refetched_graphs > 0 || p.resyncs > 0 => {
                            tracing::info!(
                                "replication: seq {} ({} rows, {} graphs fetched, {} resyncs)",
                                p.applied_seq,
                                p.applied_rows,
                                p.refetched_graphs,
                                p.resyncs
                            );
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("replication: catch-up failed: {e}"),
                    }
                }
                std::thread::sleep(config.poll.min(Duration::from_secs(1)));
            }
        });
    if let Err(e) = spawned {
        tracing::error!("replication: could not start the follower thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_selects_graphs_through_the_manifest() {
        let m = Manifest {
            datasets: vec![
                DatasetGraphs {
                    id: "a".into(),
                    graphs: vec!["http://g/1".into(), "http://g/2".into()],
                },
                DatasetGraphs {
                    id: "b".into(),
                    graphs: vec!["http://g/3".into()],
                },
            ],
            ..Default::default()
        };
        assert!(selected_graphs(&Scope::All, &m).is_none());
        let some = selected_graphs(&Scope::Datasets(vec!["a".into()]), &m).unwrap();
        assert_eq!(some.len(), 2);
        assert!(some.contains(&Some("http://g/2".to_string())));
        let graphs = selected_graphs(&Scope::Graphs(vec![None]), &m).unwrap();
        assert!(graphs.contains(&None));
    }

    #[test]
    fn the_bookmark_survives_a_reopen_beside_a_persistent_store() {
        let dir = tempfile::tempdir().unwrap();
        let r = Replication::with_config(
            ReplicationConfig::follower("http://leader", Mode::Warm, Scope::All),
            Some(dir.path()),
        );
        r.set_bookmark(Some("e1".into()), 42);
        let again = Replication::with_config(
            ReplicationConfig::follower("http://leader", Mode::Warm, Scope::All),
            Some(dir.path()),
        );
        let b = again.bookmark();
        assert_eq!((b.epoch.as_deref(), b.applied_seq), (Some("e1"), 42));
        assert!(b.synced_at.is_some());
    }

    #[test]
    fn a_follower_is_unhealthy_until_it_has_caught_up_once() {
        let r = Replication::with_config(
            ReplicationConfig::follower("http://leader", Mode::Hot, Scope::All),
            None,
        );
        assert!(!r.status().healthy);
        r.record_ok(7);
        let s = r.status();
        assert!(s.healthy);
        assert_eq!(s.lag_rows, Some(7));
        assert!(Replication::disabled().status().healthy);
    }

    #[test]
    fn the_apply_guard_is_scoped_to_the_thread_and_nests() {
        assert!(!applying());
        {
            let _a = ApplyGuard::enter();
            assert!(applying());
            {
                let _b = ApplyGuard::enter();
                assert!(applying());
            }
            assert!(applying());
        }
        assert!(!applying());
    }
}
