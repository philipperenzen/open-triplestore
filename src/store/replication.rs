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
//! hour, `warm` every minute, `hot` continuously — a long-poll held on the
//! leader up to [`LONG_POLL`] and answered the moment a row lands. **Scope**
//! changes
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
use crate::store::consensus::{self, ClusterConfig};
use crate::store::engine::{StoreError, TripleStore};

// ─── Configuration ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    None,
    Leader,
    Follower,
    /// A member of a Raft cluster: leader or follower as the election
    /// decides (see [`Replication::effective_role`]).
    Cluster,
}

/// How often a follower asks. Nothing else differs between the temperatures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// A disaster-recovery copy: catch up every hour.
    Cold,
    /// A reporting replica: catch up every minute.
    Warm,
    /// A read replica: long-poll the leader continuously; the lag is a
    /// round trip.
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
    /// A hot follower's pacing after a failed catch-up, and its health
    /// interval; the request for rows itself is held for [`LONG_POLL`].
    pub poll: Duration,
    /// How often the follower catches up (the temperature, or an override).
    pub interval: Duration,
    /// Rows per page.
    pub page: usize,
    /// Synchronous replication (a leader): the node ids of the followers
    /// whose acknowledgement a write waits for; empty means asynchronous.
    pub sync_followers: Vec<String>,
    /// How many of them must have applied a write before it returns.
    pub sync_required: usize,
    /// How long a write waits for them before it returns degraded.
    pub sync_timeout: Duration,
    /// How often a follower checks the leader's identity database for a
    /// change (the temperature's interval, at least 5 s, or an override).
    pub identity_interval: Duration,
    /// The Raft cluster this node belongs to, for the `cluster` role.
    pub cluster: Option<ClusterConfig>,
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

/// True when `OTS_REPLICATION_ROLE=cluster`: the change log stays on (this
/// node may be elected leader at any time).
pub fn cluster_role_configured() -> bool {
    env_opt("OTS_REPLICATION_ROLE")
        .map(|r| r.eq_ignore_ascii_case("cluster"))
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
        let (sync_followers, sync_required, sync_timeout) = Self::parse_sync(
            env_opt("OTS_REPLICATION_SYNC_FOLLOWERS").as_deref(),
            env_opt("OTS_REPLICATION_SYNC_REQUIRED").as_deref(),
            env_opt("OTS_REPLICATION_SYNC_TIMEOUT_MS").as_deref(),
        );
        let mut c = Self::parse(
            env_opt("OTS_REPLICATION_ROLE").as_deref().unwrap_or("none"),
            env_opt("OTS_REPLICATION_MODE").as_deref().unwrap_or("warm"),
            env_opt("OTS_REPLICATION_GRAPHS").as_deref(),
            env_opt("OTS_REPLICATION_DATASETS").as_deref(),
            env_opt("OTS_REPLICATION_LEADER_URL").as_deref(),
            env_opt("OTS_REPLICATION_TOKEN").as_deref(),
            env_opt("OTS_REPLICATION_NODE_ID").as_deref(),
            env_opt("OTS_REPLICATION_POLL_MS").as_deref(),
            env_opt("OTS_REPLICATION_INTERVAL_SECS").as_deref(),
        );
        c.sync_followers = sync_followers;
        c.sync_required = sync_required;
        c.sync_timeout = sync_timeout;
        if let Some(secs) =
            env_opt("OTS_REPLICATION_IDENTITY_INTERVAL_SECS").and_then(|v| v.parse::<u64>().ok())
        {
            c.identity_interval = Duration::from_secs(secs.max(1));
        }
        if c.role == Role::Cluster {
            match ClusterConfig::parse(
                env_opt("OTS_REPLICATION_CLUSTER").as_deref(),
                env_opt("OTS_REPLICATION_CLUSTER_ID").as_deref(),
                env_opt("OTS_REPLICATION_CLUSTER_SECRET").as_deref(),
                env_opt("OTS_REPLICATION_ELECTION_MS").as_deref(),
                env_opt("OTS_REPLICATION_HEARTBEAT_MS").as_deref(),
            ) {
                Some(cluster) => c.apply_cluster(cluster),
                None => {
                    tracing::error!(
                        "replication: OTS_REPLICATION_ROLE=cluster needs OTS_REPLICATION_CLUSTER \
                         (id=url,...), OTS_REPLICATION_CLUSTER_ID (one of them) and \
                         OTS_REPLICATION_CLUSTER_SECRET; running without a role"
                    );
                    c.role = Role::None;
                }
            }
        }
        c
    }

    /// Join a cluster: the node's name is `node-<id>`, the other members
    /// are its synchronous followers, and a majority must acknowledge —
    /// unless the environment set those explicitly.
    pub fn apply_cluster(&mut self, cluster: ClusterConfig) {
        self.role = Role::Cluster;
        if env_opt("OTS_REPLICATION_NODE_ID").is_none() {
            self.node_id = format!("node-{}", cluster.id);
        }
        if self.sync_followers.is_empty() {
            self.sync_followers = cluster.peer_names();
            self.sync_required = cluster.majority_peers().max(1);
        }
        self.mode = Mode::Hot;
        // A member follows hot: its request for rows is held on the leader
        // and asked again the moment it is answered, so its interval is its
        // poll — not the warm minute `parse` gave it before the cluster was
        // known, which left a member blind for the rest of every minute
        // after its hold — unless the environment set an interval on purpose.
        if env_opt("OTS_REPLICATION_INTERVAL_SECS").is_none() {
            self.interval = Mode::Hot.default_interval(self.poll);
            self.identity_interval = self.interval.max(Duration::from_secs(5));
        }
        self.leader_url = None;
        self.cluster = Some(cluster);
    }

    /// `OTS_REPLICATION_SYNC_FOLLOWERS` (node ids), `_SYNC_REQUIRED` (how
    /// many of them, default 1, never more than there are), and
    /// `_SYNC_TIMEOUT_MS` (default 2000, 50–60000).
    pub fn parse_sync(
        followers: Option<&str>,
        required: Option<&str>,
        timeout_ms: Option<&str>,
    ) -> (Vec<String>, usize, Duration) {
        let followers: Vec<String> = followers
            .map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let required = required
            .and_then(|v| {
                if v.trim().eq_ignore_ascii_case("all") {
                    Some(followers.len())
                } else {
                    v.trim().parse::<usize>().ok()
                }
            })
            .unwrap_or(1)
            .clamp(1, followers.len().max(1));
        let timeout = Duration::from_millis(
            timeout_ms
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(2000)
                .clamp(50, 60_000),
        );
        (followers, required, timeout)
    }

    /// Synchronous replication (builder style; tests).
    pub fn with_sync(mut self, followers: &[&str], required: usize, timeout_ms: u64) -> Self {
        let (f, r, t) = Self::parse_sync(
            Some(&followers.join(",")),
            Some(&required.to_string()),
            Some(&timeout_ms.to_string()),
        );
        self.sync_followers = f;
        self.sync_required = r;
        self.sync_timeout = t;
        self
    }

    /// This node's name (builder style; tests).
    pub fn with_node_id(mut self, node_id: &str) -> Self {
        self.node_id = node_id.to_string();
        self
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
            sync_followers: Vec::new(),
            sync_required: 1,
            sync_timeout: Duration::from_millis(2000),
            identity_interval: interval.max(Duration::from_secs(5)),
            cluster: None,
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

/// A synchronous leader's view of its followers.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct SyncStatus {
    pub followers: Vec<String>,
    pub required: usize,
    pub timeout_ms: u64,
    /// The followers whose cursor covered the last write that waited.
    pub acked: Vec<String>,
    /// Set while writes return without the required acknowledgements;
    /// cleared by the first write that gets them again.
    pub degraded_since: Option<String>,
    /// The newest sequence number the required followers confirmed.
    pub last_confirmed_seq: i64,
    pub waits: u64,
    pub degraded_waits: u64,
}

#[derive(Default)]
struct SyncState {
    acked: Vec<String>,
    degraded_since: Option<String>,
    last_confirmed_seq: i64,
    waits: u64,
    degraded_waits: u64,
}

/// What a write got from the synchronous followers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncOutcome {
    /// The required followers applied it within the timeout.
    Synced,
    /// They did not: the write is durable on the leader only.
    Degraded,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Status {
    /// The role played now (a cluster member reports `leader` or `follower`).
    pub role: Role,
    /// The role as configured (`cluster` for a member).
    pub configured_role: Role,
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
    /// Present on a leader with synchronous followers configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncStatus>,
    /// Present on a follower: the identity database's replication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<IdentityStatus>,
    /// Present on a cluster member: who leads, the term, this node's state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster: Option<consensus::View>,
}

/// The identity database as a follower keeps it: the leader's version it
/// last applied, when, how often, and the last error.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct IdentityStatus {
    pub applied_version: Option<i64>,
    pub applied_at: Option<String>,
    pub applies: u64,
    pub last_error: Option<String>,
    pub interval_secs: f64,
}

fn identity_state() -> &'static Mutex<IdentityStatus> {
    static S: OnceLock<Mutex<IdentityStatus>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(IdentityStatus::default()))
}

/// The identity follower's state (process-wide: one identity database).
pub fn identity_status() -> IdentityStatus {
    identity_state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

pub struct Replication {
    config: ReplicationConfig,
    bookmark: Mutex<Bookmark>,
    path: Option<PathBuf>,
    last: Mutex<LastSync>,
    applied_rows: AtomicU64,
    refetched_graphs: AtomicU64,
    resyncs: AtomicU64,
    sync: Mutex<SyncState>,
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
            sync: Mutex::new(SyncState::default()),
        }
    }

    pub fn disabled() -> Self {
        Self::with_config(ReplicationConfig::none(), None)
    }

    pub fn config(&self) -> &ReplicationConfig {
        &self.config
    }

    /// The configured role.
    pub fn role(&self) -> Role {
        self.config.role
    }

    /// The role this node plays now: for a cluster member, whatever the
    /// election decided — a follower until a leader is known.
    pub fn effective_role(&self) -> Role {
        match self.config.role {
            Role::Cluster => match consensus::view() {
                Some(v) if v.is_leader => Role::Leader,
                _ => Role::Follower,
            },
            r => r,
        }
    }

    /// Where the leader is: the configured URL, or the elected member's.
    pub fn leader_url(&self) -> Option<String> {
        match self.config.role {
            Role::Cluster => consensus::leader_url(),
            _ => self.config.leader_url.clone(),
        }
    }

    /// A follower refuses every write that is not its own replication —
    /// and so does a cluster member that is not the leader right now.
    pub fn read_only(&self) -> bool {
        self.effective_role() == Role::Follower
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

    /// Progress inside a catch-up: the health window restarts from now,
    /// without calling the catch-up complete.
    fn note_progress(&self) {
        let mut l = self.last.lock().unwrap_or_else(|p| p.into_inner());
        l.ok_at = Some(Instant::now());
    }

    fn record_error(&self, e: &str) {
        let mut l = self.last.lock().unwrap_or_else(|p| p.into_inner());
        l.at = Some(now());
        l.error = Some(e.to_string());
    }

    pub fn status(&self) -> Status {
        let b = self.bookmark();
        let l = self.last.lock().unwrap_or_else(|p| p.into_inner());
        let effective = self.effective_role();
        // A hot follower's request is held on the leader for up to
        // `LONG_POLL` when nothing lands, and its last success is that old
        // while it is held; the window allows for the hold.
        let hold = if self.config.mode == Mode::Hot {
            LONG_POLL
        } else {
            Duration::ZERO
        };
        let healthy = match effective {
            Role::Follower => l
                .ok_at
                .map(|t| t.elapsed() <= self.config.interval * 3 + hold + Duration::from_secs(5))
                .unwrap_or(false),
            _ => true,
        };
        Status {
            role: effective,
            configured_role: self.config.role,
            mode: self.config.mode,
            scope: self.config.scope.describe(),
            leader_url: self.leader_url(),
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
            sync: self.sync_status(),
            cluster: self.config.cluster.as_ref().map(|c| {
                consensus::view().unwrap_or_else(|| consensus::View {
                    id: c.id,
                    members: c.members.clone(),
                    state: "starting".to_string(),
                    ..Default::default()
                })
            }),
            identity: (effective == Role::Follower).then(|| {
                let mut s = identity_status();
                s.interval_secs = self.config.identity_interval.as_secs_f64();
                s
            }),
        }
    }

    /// Synchronous replication is configured on this node.
    pub fn sync_configured(&self) -> bool {
        self.effective_role() == Role::Leader && !self.config.sync_followers.is_empty()
    }

    pub fn sync_status(&self) -> Option<SyncStatus> {
        if !self.sync_configured() {
            return None;
        }
        let s = self.sync.lock().unwrap_or_else(|p| p.into_inner());
        Some(SyncStatus {
            followers: self.config.sync_followers.clone(),
            required: self.config.sync_required,
            timeout_ms: self.config.sync_timeout.as_millis() as u64,
            acked: s.acked.clone(),
            degraded_since: s.degraded_since.clone(),
            last_confirmed_seq: s.last_confirmed_seq,
            waits: s.waits,
            degraded_waits: s.degraded_waits,
        })
    }

    /// What a write's response should say about the synchronous followers:
    /// `sync` while the required ones keep up, `degraded` while they do
    /// not, nothing when none are configured. The `X-Replication-Ack`
    /// header of the write routes.
    pub fn ack_state(&self) -> Option<&'static str> {
        if !self.sync_configured() {
            return None;
        }
        let s = self.sync.lock().unwrap_or_else(|p| p.into_inner());
        Some(if s.degraded_since.is_some() {
            "degraded"
        } else {
            "sync"
        })
    }

    /// Called at the end of every outermost write on a leader: wait until
    /// the required followers' cursors cover the write, or the timeout.
    /// `seq_before` is the log's position when the write began. While
    /// degraded, a write does not wait unless a follower has caught up to
    /// `seq_before` — a dead follower costs one lookup per write, not one
    /// timeout; a returning one gets the full wait and clears the flag.
    pub fn after_write(&self, log: &changes::ChangeLog, seq_before: i64) -> Option<SyncOutcome> {
        if !self.sync_configured() || !log.enabled() {
            return None;
        }
        let seq = log.last_seq();
        if seq <= seq_before {
            // The write recorded nothing: nothing to wait for.
            return None;
        }
        let names = &self.config.sync_followers;
        let required = self.config.sync_required;
        let degraded = self
            .sync
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .degraded_since
            .is_some();
        let (acked, ok) = if degraded && log.cursors_at(names, seq_before) < required {
            (log.cursors_at(names, seq), false)
        } else {
            log.wait_for_cursors(names, seq, required, self.config.sync_timeout)
        };
        let mut s = self.sync.lock().unwrap_or_else(|p| p.into_inner());
        s.waits += 1;
        s.acked = names
            .iter()
            .filter(|n| log.cursor(n).is_some_and(|c| c.seq >= seq))
            .cloned()
            .collect();
        if ok {
            s.last_confirmed_seq = seq;
            if s.degraded_since.take().is_some() {
                tracing::info!(
                    "replication: synchronous again at seq {seq} ({acked} of {required} required)"
                );
            }
            Some(SyncOutcome::Synced)
        } else {
            s.degraded_waits += 1;
            if s.degraded_since.is_none() {
                s.degraded_since = Some(now());
                tracing::warn!(
                    "replication: degraded to asynchronous at seq {seq}: {acked} of {required} required followers acknowledged within {:?}",
                    self.config.sync_timeout
                );
            }
            Some(SyncOutcome::Degraded)
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
    /// The identity database's change counter (`PRAGMA data_version` on
    /// the leader), so a follower fetches a snapshot only after a change.
    #[serde(default)]
    pub identity_version: Option<i64>,
}

/// The store half of the manifest; the handler adds the datasets.
pub fn manifest_of(
    store: &TripleStore,
    datasets: Vec<DatasetGraphs>,
    identity_version: Option<i64>,
) -> Manifest {
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
        identity_version,
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
    /// Rows after `after`; when there are none, a source that can wait
    /// holds the request up to `wait` for one to arrive.
    fn changes_after(&self, after: i64, limit: usize, wait: Duration) -> Result<Page, String>;
    /// The graph as N-Triples; `Ok(None)` when the leader does not have it.
    fn graph_ntriples(&self, graph: Option<&str>) -> Result<Option<String>, String>;
    fn set_cursor(&self, name: &str, seq: i64) -> Result<(), String>;
    /// The leader's identity database, whole, as SQLite file bytes.
    fn identity_snapshot(&self) -> Result<Vec<u8>, String>;
}

/// The leader in the same process: tests, and the shape every source must
/// have.
#[cfg(any(test, feature = "test-utils"))]
#[allow(dead_code)]
pub struct InProcessLeader {
    store: TripleStore,
    datasets: Vec<DatasetGraphs>,
    identity: Option<std::sync::Arc<crate::auth::db::AuthDb>>,
}

#[cfg(any(test, feature = "test-utils"))]
#[allow(dead_code)]
impl InProcessLeader {
    pub fn new(store: TripleStore) -> Self {
        Self {
            store,
            datasets: Vec::new(),
            identity: None,
        }
    }

    /// The leader's identity database, for the identity follower.
    pub fn with_identity(mut self, auth: std::sync::Arc<crate::auth::db::AuthDb>) -> Self {
        self.identity = Some(auth);
        self
    }

    pub fn with_datasets(mut self, datasets: Vec<DatasetGraphs>) -> Self {
        self.datasets = datasets;
        self
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl LeaderSource for InProcessLeader {
    fn manifest(&self) -> Result<Manifest, String> {
        Ok(manifest_of(
            &self.store,
            self.datasets.clone(),
            self.identity.as_ref().and_then(|a| a.data_version()),
        ))
    }

    fn changes_after(&self, after: i64, limit: usize, _wait: Duration) -> Result<Page, String> {
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

    fn identity_snapshot(&self) -> Result<Vec<u8>, String> {
        self.identity
            .as_ref()
            .ok_or_else(|| "this leader has no identity database".to_string())?
            .snapshot_bytes()
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

/// How long a hot follower asks the leader to hold its request for rows
/// when none are waiting. Under the leader's 30 s cap on a long-poll
/// (`wait_ms`), and long enough that an idle hot follower — one request
/// for rows and one manifest read per hold — stays well under the leader's
/// per-IP limiter, which sustains one request a second. The leader answers
/// the moment a row lands whatever the hold, so the lag is unchanged.
pub const LONG_POLL: Duration = Duration::from_secs(25);

/// How many `429 Too Many Requests` answers one request waits out before it
/// is given up on.
pub const RETRY_AFTER_ATTEMPTS: usize = 8;

/// The longest a single `Retry-After` is honoured for.
pub const RETRY_AFTER_MAX: Duration = Duration::from_secs(30);

/// The wait a `429` asks for: its `Retry-After` in seconds; a second when
/// the header is missing, unreadable or `0` — the leader's limiter writes
/// whole seconds, truncated, so `0` means "under a second", and at one
/// token a second a second is the shortest wait sure to find one; and
/// never more than [`RETRY_AFTER_MAX`].
pub fn retry_after_wait(header: Option<&str>) -> Duration {
    let secs = header
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(1);
    Duration::from_secs(secs.max(1)).min(RETRY_AFTER_MAX)
}

/// Send the request `build` makes, waiting out the leader's `429`s.
///
/// The leader rate-limits a follower like any other client of its address,
/// and says when to come back (`Retry-After`). A follower that took a `429`
/// as a failed catch-up restarted the catch-up on its next tick — its
/// bootstrap from the first graph again — which is what kept the limiter
/// drained, so a leader with more graphs than the limiter's burst never got
/// a follower past bootstrap. Waiting the header out and sending the *same*
/// request again lets a bootstrap proceed at the limiter's sustained rate
/// instead. Bounded, so a leader that never stops saying `429` is an error
/// rather than a hang; any other failure is returned as it is.
async fn send_waiting_out_429(
    url: &str,
    build: impl Fn() -> reqwest::RequestBuilder,
) -> Result<reqwest::Response, String> {
    let mut waited = 0;
    loop {
        let resp = build().send().await.map_err(|e| format!("{url}: {e}"))?;
        if resp.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Ok(resp);
        }
        if waited == RETRY_AFTER_ATTEMPTS {
            return Err(format!(
                "{url}: HTTP 429 after waiting out {waited} Retry-After answers"
            ));
        }
        // Retrying a `0` at once spent all eight attempts inside a few
        // milliseconds; `retry_after_wait` reads it as a second.
        let wait = retry_after_wait(
            resp.headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
        );
        tracing::debug!("replication: {url} answered 429; waiting {wait:?} as told");
        tokio::time::sleep(wait).await;
        waited += 1;
    }
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

    /// The leader as it is now: configured, or elected.
    pub fn for_current_leader(rep: &Replication) -> Option<Self> {
        rep.leader_url()
            .map(|u| Self::new(&u, rep.config().token.as_deref()))
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
            let build = || {
                let mut req = client()
                    .request(method.clone(), &url)
                    .timeout(timeout)
                    .header("Accept", accept);
                if let Some(t) = &token {
                    req = req.bearer_auth(t);
                }
                if let Some(b) = &body {
                    req = req.json(b);
                }
                req
            };
            let resp = send_waiting_out_429(&url, build).await?;
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

    fn request_bytes(&self, path: &str, accept: &str) -> Result<Vec<u8>, String> {
        let url = format!("{}{}", self.base, path);
        let token = self.token.clone();
        let timeout = self.timeout;
        blocking(async move {
            let build = || {
                let mut req = client().get(&url).timeout(timeout).header("Accept", accept);
                if let Some(t) = &token {
                    req = req.bearer_auth(t);
                }
                req
            };
            let resp = send_waiting_out_429(&url, build).await?;
            let status = resp.status();
            if !status.is_success() {
                return Err(format!("{url}: HTTP {}", status.as_u16()));
            }
            resp.bytes()
                .await
                .map(|b| b.to_vec())
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

    fn changes_after(&self, after: i64, limit: usize, wait: Duration) -> Result<Page, String> {
        self.get_json(&format!(
            "/api/admin/changes?after={after}&limit={limit}&wait_ms={}",
            wait.as_millis()
        ))
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

    fn identity_snapshot(&self) -> Result<Vec<u8>, String> {
        self.request_bytes("/api/replication/identity", "application/vnd.sqlite3")
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
        if rep.effective_role() != Role::Follower {
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
            // A hot follower long-polls: the request returns as soon as a
            // row lands, or after `LONG_POLL`. The hold is long on purpose:
            // it is what keeps an idle hot follower to a request every few
            // tens of seconds — under the leader's per-IP limiter — rather
            // than two a second, which the limiter cut off right after the
            // bootstrap. The lag is the same round trip either way.
            let wait = if config.mode == Mode::Hot {
                LONG_POLL
            } else {
                Duration::ZERO
            };
            let page = source
                .changes_after(after, config.page, wait)
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
                let fetched_before = progress.refetched_graphs;
                self.apply_row(source, row, &selected, &manifest, &mut progress)?;
                after = seq;
                // A row that fetched a graph whole took a round trip — and,
                // under the leader's limiter, a second. Bookmark it at once
                // and let health see the progress: a page of such rows can
                // take minutes, and a position frozen at the page's start
                // read as stale while rows were being applied the whole
                // time. Delta rows apply in microseconds and are bookmarked
                // by the page.
                if progress.refetched_graphs > fetched_before {
                    rep.set_bookmark(Some(manifest.epoch.clone()), after);
                    rep.note_progress();
                }
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
            // Each fetch is a round trip — and, under the leader's limiter, a
            // second — so a resync of many graphs reports its progress the way
            // the row path does, rather than reading as stale until it ends.
            self.replication().note_progress();
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

// ─── The identity database ──────────────────────────────────────────────────

/// One round of the identity follower: read the leader's manifest and, if
/// its identity version moved since the last apply (or is unknown), fetch
/// the snapshot and apply it in place. Returns whether one was applied.
pub fn replicate_identity_once(
    auth: &crate::auth::db::AuthDb,
    source: &dyn LeaderSource,
) -> Result<bool, String> {
    let outcome: Result<(bool, Option<i64>), String> = (|| {
        let m = source.manifest()?;
        let applied = identity_state()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .applied_version;
        if m.identity_version.is_some() && m.identity_version == applied {
            return Ok((false, m.identity_version));
        }
        let bytes = source.identity_snapshot()?;
        auth.apply_snapshot(&bytes).map_err(|e| e.to_string())?;
        Ok((true, m.identity_version))
    })();
    let mut s = identity_state().lock().unwrap_or_else(|p| p.into_inner());
    match outcome {
        Ok((applied, version)) => {
            if applied {
                s.applied_version = version;
                s.applied_at = Some(now());
                s.applies += 1;
            }
            s.last_error = None;
            Ok(applied)
        }
        Err(e) => {
            s.last_error = Some(e.clone());
            Err(e)
        }
    }
}

/// Start the identity follower when the environment configures a follower
/// with a leader URL. Called once by `AuthDb::open`.
pub fn spawn_identity_follower_if_configured(auth: crate::auth::db::AuthDb) {
    let config = ReplicationConfig::from_env();
    if config.role != Role::Follower && config.role != Role::Cluster {
        return;
    }
    if config.role == Role::Follower && config.leader_url.is_none() {
        return;
    }
    let interval = config.identity_interval;
    let rep = Replication::with_config(config, None);
    let spawned = std::thread::Builder::new()
        .name("replication-identity".into())
        .spawn(move || loop {
            // A cluster member that leads has nothing to fetch.
            if rep.effective_role() != Role::Follower {
                std::thread::sleep(interval);
                continue;
            }
            let Some(http) = HttpLeader::for_current_leader(&rep) else {
                std::thread::sleep(interval);
                continue;
            };
            match replicate_identity_once(&auth, &http) {
                Ok(true) => {
                    tracing::info!("replication: identity database applied from the leader")
                }
                Ok(false) => {}
                Err(e) => tracing::warn!("replication: identity database not applied: {e}"),
            }
            std::thread::sleep(interval);
        });
    if let Err(e) = spawned {
        tracing::error!("replication: could not start the identity follower thread: {e}");
    }
}

// ─── The follower thread ────────────────────────────────────────────────────

/// Start the follower loop when the environment configures one. Called once
/// by the store constructors; does nothing for a leader or an unconfigured
/// node, or when the leader URL is missing (then the status says so).
pub(crate) fn spawn_follower_if_configured(store: &TripleStore) {
    let rep = store.replication();
    if let Some(cluster) = &rep.config().cluster {
        // The vote goes beside the bookmark, in the store's own directory. An
        // in-memory store has neither, and keeps the previous behaviour.
        let vote_path = rep
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|d| d.join("raft-vote.json"));
        consensus::spawn(cluster.clone(), vote_path);
    }
    if rep.role() != Role::Follower && rep.role() != Role::Cluster {
        return;
    }
    if rep.role() == Role::Follower && rep.config().leader_url.is_none() {
        rep.record_error("OTS_REPLICATION_LEADER_URL is not set; this follower cannot catch up");
        tracing::warn!("replication: follower without OTS_REPLICATION_LEADER_URL");
        return;
    }
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
                    // A cluster member follows whoever leads now, and a
                    // leader follows nobody.
                    if store.replication().effective_role() != Role::Follower {
                        std::thread::sleep(config.poll.min(Duration::from_secs(1)));
                        continue;
                    }
                    let Some(http) = HttpLeader::for_current_leader(store.replication()) else {
                        std::thread::sleep(Duration::from_secs(1));
                        continue;
                    };
                    let outcome = store.replicate_once(&http);
                    let failed = outcome.is_err();
                    match outcome {
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
                    if config.mode == Mode::Hot && !failed {
                        // The long-poll paced this round; ask again at once.
                        continue;
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

    /// A hot follower's request is held on the leader for tens of seconds
    /// when nothing lands, and its last success is that old while it is
    /// held. Health has to allow for the hold, or an idle hot follower would
    /// read as stale between rows.
    #[test]
    fn a_hot_follower_held_on_a_long_poll_is_still_healthy() {
        let r = Replication::with_config(
            ReplicationConfig::follower("http://leader", Mode::Hot, Scope::All),
            None,
        );
        r.record_ok(0);
        r.last.lock().unwrap().ok_at = Some(Instant::now() - Duration::from_secs(20));
        assert!(r.status().healthy, "20 s into a hold is not stale");
        r.last.lock().unwrap().ok_at = Some(Instant::now() - Duration::from_secs(120));
        assert!(!r.status().healthy, "two minutes is");

        // A warm follower's window is its interval, three times over.
        let w = Replication::with_config(
            ReplicationConfig::follower("http://leader", Mode::Warm, Scope::All),
            None,
        );
        w.record_ok(0);
        w.last.lock().unwrap().ok_at = Some(Instant::now() - Duration::from_secs(120));
        assert!(w.status().healthy);
        w.last.lock().unwrap().ok_at = Some(Instant::now() - Duration::from_secs(300));
        assert!(!w.status().healthy);
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
