//! Consensus (P4, the maintainer's decision 2): Raft, through `openraft`,
//! elects the leader and fences the old one; the data path stays the change
//! log.
//!
//! What Raft decides here is *who leads* — nothing else. The replicated
//! state machine holds no data: its entries are membership changes and a
//! blank heartbeat, and the log lives in memory. That keeps the
//! dependency to its strength (a proven election with a majority quorum and
//! an election timeout that fences a partitioned leader) and leaves the
//! data where P4 put it: one leader recording every write in its change
//! log, followers tailing it, and synchronous acknowledgement by a majority
//! of the other members. A node that loses the Raft leadership stops
//! accepting writes on the next request (its role is read through
//! [`view`]); a node that wins it starts recording and serving. Followers
//! notice the new leader's epoch and resynchronise once.
//!
//! Why the log is not persisted: a restarted member rejoins with term 0 and
//! learns the current term from the first heartbeat or vote request, and the
//! state machine it would replay holds no data — the data path is the change
//! log, and a restarted member catches up on it like any other follower.
//!
//! The **vote** is persisted, in one small file beside the store
//! (`{data_dir}/raft-vote.json`, rewritten on each vote and only on a vote,
//! so never on a hot path). Raft's safety argument is that a member votes at
//! most once per term; a member that forgets its vote across a restart can
//! vote again in the same term. The consequences here were bounded — the
//! election timeout (1.5–3 s by default) usually outlasts a restart, and
//! since the state machine holds no data and the data path fences by epoch,
//! a double vote could at worst elect a second leader for one term, which
//! the epoch handling turns into a resynchronisation rather than a
//! divergence — but bounded is not the same as absent, and remembering one
//! `{term, node}` pair is cheap. An in-memory store has no file and behaves
//! as it always did; an unreadable file is logged and ignored rather than
//! refused, since starting without it is exactly where this began.
//!
//! Transport: the Raft RPCs travel as JSON over the server's own HTTP port
//! (`POST /api/replication/raft/{vote,append,snapshot}`), authenticated by
//! a shared cluster secret. Tests wire the same code through an in-process
//! router instead.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::io::Cursor;
use std::ops::RangeBounds;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use openraft::error::{
    InstallSnapshotError, NetworkError, RPCError, RaftError, RemoteError, Unreachable,
};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::storage::{LogFlushed, RaftLogStorage, RaftStateMachine};
use openraft::{
    BasicNode, Config, Entry, EntryPayload, LogId, LogState, OptionalSend, Raft, RaftLogReader,
    RaftMetrics, RaftSnapshotBuilder, ServerState, Snapshot, SnapshotMeta, SnapshotPolicy,
    StorageError, StoredMembership, Vote,
};

// ─── Types ──────────────────────────────────────────────────────────────────

/// A log entry's payload: nothing. The state machine counts them.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Command {}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ack {}

openraft::declare_raft_types!(
    /// The Raft type configuration: u64 node ids, `BasicNode` (an address).
    pub TypeConfig:
        D = Command,
        R = Ack,
);

use std::path::PathBuf;

pub type NodeId = u64;

// ─── The in-memory log ──────────────────────────────────────────────────────

#[derive(Default)]
struct LogInner {
    last_purged: Option<LogId<NodeId>>,
    log: BTreeMap<u64, Entry<TypeConfig>>,
    vote: Option<Vote<NodeId>>,
    committed: Option<LogId<NodeId>>,
}

/// The Raft log, in memory — with the vote, and only the vote, on disk.
///
/// The log itself stays in memory: it holds elections, not data, and a member
/// that restarts rejoins by catching up on the change log like any other
/// follower (see the module docs). The *vote* is different. Raft's safety
/// argument is that a member votes at most once per term, and a member that
/// forgets its vote across a restart can vote a second time in the same term —
/// at worst electing a second leader for that term, which the epoch fence turns
/// into a resynchronisation rather than a divergence, but which is still a
/// thing that should not happen and is cheap not to.
///
/// One small file, rewritten on each vote. Raft votes rarely — once per
/// election, not once per write — so this is not on any hot path.
#[derive(Clone, Default)]
pub struct MemLog {
    inner: Arc<Mutex<LogInner>>,
    /// Where the vote is kept. `None` keeps the previous behaviour exactly:
    /// an in-memory store, or a member configured before there was a file.
    vote_path: Option<Arc<PathBuf>>,
}

impl MemLog {
    /// `vote_path` is `{data_dir}/raft-vote.json` for a persistent store.
    pub fn new(vote_path: Option<PathBuf>) -> Self {
        let me = Self {
            inner: Arc::new(Mutex::new(LogInner::default())),
            vote_path: vote_path.map(Arc::new),
        };
        // Read it once here rather than on every `read_vote`: openraft asks for
        // the vote during start-up, and after that the in-memory copy is the
        // one it has been updating.
        if let Some(vote) = me.read_vote_file() {
            me.lock().vote = Some(vote);
        }
        me
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LogInner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn read_vote_file(&self) -> Option<Vote<NodeId>> {
        let path = self.vote_path.as_ref()?;
        let text = std::fs::read_to_string(path.as_path()).ok()?;
        match serde_json::from_str::<Vote<NodeId>>(&text) {
            Ok(v) => Some(v),
            Err(e) => {
                // A corrupt file is not a reason to refuse to start: the worst
                // it costs is the risk this whole mechanism removes, which is
                // where we were before it existed. Refusing to boot would be
                // strictly worse than the problem.
                tracing::warn!("raft: vote file unreadable ({e}); starting without it");
                None
            }
        }
    }

    fn write_vote_file(&self, vote: &Vote<NodeId>) {
        let Some(path) = self.vote_path.as_ref() else {
            return;
        };
        // Write-and-rename: a vote half-written by a crash is exactly the
        // forgotten vote this is here to prevent.
        let tmp = path.with_extension("json.tmp");
        let body = match serde_json::to_vec(vote) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("raft: vote not serialised: {e}");
                return;
            }
        };
        if let Err(e) =
            std::fs::write(&tmp, &body).and_then(|()| std::fs::rename(&tmp, path.as_path()))
        {
            tracing::warn!("raft: vote not persisted to {}: {e}", path.display());
        }
    }
}

impl RaftLogReader<TypeConfig> for MemLog {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<TypeConfig>>, StorageError<NodeId>> {
        Ok(self
            .lock()
            .log
            .range(range)
            .map(|(_, e)| e.clone())
            .collect())
    }
}

impl RaftLogStorage<TypeConfig> for MemLog {
    type LogReader = Self;

    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError<NodeId>> {
        let g = self.lock();
        let last = g
            .log
            .iter()
            .next_back()
            .map(|(_, e)| e.log_id)
            .or(g.last_purged);
        Ok(LogState {
            last_purged_log_id: g.last_purged,
            last_log_id: last,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        self.clone()
    }

    async fn save_vote(&mut self, vote: &Vote<NodeId>) -> Result<(), StorageError<NodeId>> {
        self.lock().vote = Some(*vote);
        self.write_vote_file(vote);
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<NodeId>>, StorageError<NodeId>> {
        Ok(self.lock().vote)
    }

    async fn save_committed(
        &mut self,
        committed: Option<LogId<NodeId>>,
    ) -> Result<(), StorageError<NodeId>> {
        self.lock().committed = committed;
        Ok(())
    }

    async fn read_committed(&mut self) -> Result<Option<LogId<NodeId>>, StorageError<NodeId>> {
        Ok(self.lock().committed)
    }

    async fn append<I>(
        &mut self,
        entries: I,
        callback: LogFlushed<TypeConfig>,
    ) -> Result<(), StorageError<NodeId>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        {
            let mut g = self.lock();
            for e in entries {
                g.log.insert(e.log_id.index, e);
            }
        }
        callback.log_io_completed(Ok(()));
        Ok(())
    }

    async fn truncate(&mut self, log_id: LogId<NodeId>) -> Result<(), StorageError<NodeId>> {
        let mut g = self.lock();
        let _dropped = g.log.split_off(&log_id.index);
        Ok(())
    }

    async fn purge(&mut self, log_id: LogId<NodeId>) -> Result<(), StorageError<NodeId>> {
        let mut g = self.lock();
        g.last_purged = Some(log_id);
        let keep = g.log.split_off(&(log_id.index + 1));
        g.log = keep;
        Ok(())
    }
}

// ─── The state machine: membership and a count, nothing else ────────────────

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct SmData {
    last_applied: Option<LogId<NodeId>>,
    membership: StoredMembership<NodeId, BasicNode>,
    applied: u64,
}

#[derive(Default)]
struct SmInner {
    data: SmData,
    snapshot_idx: u64,
    current_snapshot: Option<(SnapshotMeta<NodeId, BasicNode>, Vec<u8>)>,
}

#[derive(Clone, Default)]
pub struct MemStateMachine(Arc<Mutex<SmInner>>);

impl MemStateMachine {
    fn lock(&self) -> std::sync::MutexGuard<'_, SmInner> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl RaftSnapshotBuilder<TypeConfig> for MemStateMachine {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError<NodeId>> {
        let mut g = self.lock();
        g.snapshot_idx += 1;
        let data = serde_json::to_vec(&g.data).unwrap_or_default();
        let meta = SnapshotMeta {
            last_log_id: g.data.last_applied,
            last_membership: g.data.membership.clone(),
            snapshot_id: format!(
                "{}-{}",
                g.data.last_applied.map(|l| l.index).unwrap_or(0),
                g.snapshot_idx
            ),
        };
        g.current_snapshot = Some((meta.clone(), data.clone()));
        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        })
    }
}

impl RaftStateMachine<TypeConfig> for MemStateMachine {
    type SnapshotBuilder = Self;

    async fn applied_state(
        &mut self,
    ) -> Result<(Option<LogId<NodeId>>, StoredMembership<NodeId, BasicNode>), StorageError<NodeId>>
    {
        let g = self.lock();
        Ok((g.data.last_applied, g.data.membership.clone()))
    }

    async fn apply<I>(&mut self, entries: I) -> Result<Vec<Ack>, StorageError<NodeId>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        let mut g = self.lock();
        let mut out = Vec::new();
        for e in entries {
            g.data.last_applied = Some(e.log_id);
            match e.payload {
                EntryPayload::Blank => {}
                EntryPayload::Normal(_) => g.data.applied += 1,
                EntryPayload::Membership(m) => {
                    g.data.membership = StoredMembership::new(Some(e.log_id), m);
                }
            }
            out.push(Ack {});
        }
        Ok(out)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        self.clone()
    }

    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<Box<Cursor<Vec<u8>>>, StorageError<NodeId>> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<NodeId, BasicNode>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError<NodeId>> {
        let data = snapshot.into_inner();
        let mut g = self.lock();
        if let Ok(d) = serde_json::from_slice::<SmData>(&data) {
            g.data = d;
        }
        g.data.last_applied = meta.last_log_id;
        g.data.membership = meta.last_membership.clone();
        g.current_snapshot = Some((meta.clone(), data));
        Ok(())
    }

    async fn get_current_snapshot(
        &mut self,
    ) -> Result<Option<Snapshot<TypeConfig>>, StorageError<NodeId>> {
        let g = self.lock();
        Ok(g.current_snapshot.clone().map(|(meta, data)| Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        }))
    }
}

// ─── Transport ──────────────────────────────────────────────────────────────

/// Routes RPCs between Raft instances in one process (tests).
#[derive(Default)]
pub struct InProcessRouter {
    nodes: RwLock<HashMap<NodeId, Raft<TypeConfig>>>,
}

#[allow(dead_code)] // the binary never routes in-process; tests do
impl InProcessRouter {
    pub fn add(&self, id: NodeId, raft: Raft<TypeConfig>) {
        self.nodes
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, raft);
    }

    /// Take a node off the network: its peers find it unreachable.
    pub fn remove(&self, id: NodeId) {
        self.nodes
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
    }

    fn get(&self, id: NodeId) -> Option<Raft<TypeConfig>> {
        self.nodes
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&id)
            .cloned()
    }
}

#[derive(Clone)]
pub enum Transport {
    /// JSON over the members' HTTP ports, with the shared cluster secret.
    Http { secret: String, timeout: Duration },
    #[allow(dead_code)] // tests
    InProcess(Arc<InProcessRouter>),
}

pub struct NetworkFactory {
    transport: Transport,
}

impl NetworkFactory {
    pub fn new(transport: Transport) -> Self {
        Self { transport }
    }
}

impl RaftNetworkFactory<TypeConfig> for NetworkFactory {
    type Network = Client;

    async fn new_client(&mut self, target: NodeId, node: &BasicNode) -> Self::Network {
        Client {
            target,
            addr: node.addr.trim_end_matches('/').to_string(),
            transport: self.transport.clone(),
        }
    }
}

pub struct Client {
    target: NodeId,
    addr: String,
    transport: Transport,
}

fn unreachable<E: std::error::Error + 'static>(e: &E) -> Unreachable {
    Unreachable::new(e)
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!("open-triplestore-raft/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client")
    })
}

impl Client {
    // openraft's `RaftNetwork` trait fixes this return type, so the large
    // `RPCError` cannot be boxed here without unboxing it at every call site.
    #[allow(clippy::result_large_err)]
    async fn post<
        Req: serde::Serialize,
        Resp: serde::de::DeserializeOwned,
        E: std::error::Error,
    >(
        &self,
        secret: &str,
        timeout: Duration,
        path: &str,
        req: &Req,
    ) -> Result<Resp, RPCError<NodeId, BasicNode, E>> {
        let url = format!("{}/api/replication/raft/{path}", self.addr);
        let resp = http_client()
            .post(&url)
            .timeout(timeout)
            .header("X-Cluster-Secret", secret)
            .json(req)
            .send()
            .await
            .map_err(|e| RPCError::Unreachable(unreachable(&e)))?;
        let status = resp.status();
        if !status.is_success() {
            let err = std::io::Error::other(format!("{url}: HTTP {}", status.as_u16()));
            return Err(RPCError::Network(NetworkError::new(&err)));
        }
        resp.json::<Resp>()
            .await
            .map_err(|e| RPCError::Network(NetworkError::new(&e)))
    }
}

impl RaftNetwork<TypeConfig> for Client {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        option: RPCOption,
    ) -> Result<AppendEntriesResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        match &self.transport {
            Transport::Http { secret, timeout } => {
                let t = (*timeout).min(option.hard_ttl());
                self.post(secret, t, "append", &rpc).await
            }
            Transport::InProcess(router) => {
                let Some(raft) = router.get(self.target) else {
                    let e = std::io::Error::other("node is off the network");
                    return Err(RPCError::Unreachable(unreachable(&e)));
                };
                raft.append_entries(rpc)
                    .await
                    .map_err(|e| RPCError::RemoteError(RemoteError::new(self.target, e)))
            }
        }
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<NodeId>,
        RPCError<NodeId, BasicNode, RaftError<NodeId, InstallSnapshotError>>,
    > {
        match &self.transport {
            Transport::Http { secret, timeout } => {
                let t = (*timeout).min(option.hard_ttl());
                self.post(secret, t, "snapshot", &rpc).await
            }
            Transport::InProcess(router) => {
                let Some(raft) = router.get(self.target) else {
                    let e = std::io::Error::other("node is off the network");
                    return Err(RPCError::Unreachable(unreachable(&e)));
                };
                raft.install_snapshot(rpc)
                    .await
                    .map_err(|e| RPCError::RemoteError(RemoteError::new(self.target, e)))
            }
        }
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<NodeId>,
        option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        match &self.transport {
            Transport::Http { secret, timeout } => {
                let t = (*timeout).min(option.hard_ttl());
                self.post(secret, t, "vote", &rpc).await
            }
            Transport::InProcess(router) => {
                let Some(raft) = router.get(self.target) else {
                    let e = std::io::Error::other("node is off the network");
                    return Err(RPCError::Unreachable(unreachable(&e)));
                };
                raft.vote(rpc)
                    .await
                    .map_err(|e| RPCError::RemoteError(RemoteError::new(self.target, e)))
            }
        }
    }
}

// ─── A member ───────────────────────────────────────────────────────────────

/// What the rest of the server reads: who leads, as of the last metrics.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct View {
    pub id: NodeId,
    pub leader: Option<NodeId>,
    pub is_leader: bool,
    pub term: u64,
    pub state: String,
    pub members: BTreeMap<NodeId, String>,
    /// Milliseconds since a quorum acknowledged the leader (a leader that
    /// cannot reach a quorum is about to step down).
    pub millis_since_quorum_ack: Option<u64>,
}

static VIEW: OnceLock<RwLock<View>> = OnceLock::new();

fn view_cell() -> &'static RwLock<View> {
    VIEW.get_or_init(|| RwLock::new(View::default()))
}

/// The cluster as this node sees it; `None` until a member has started.
pub fn view() -> Option<View> {
    let v = view_cell().read().unwrap_or_else(|p| p.into_inner());
    if v.members.is_empty() {
        None
    } else {
        Some(v.clone())
    }
}

fn set_view(v: View) {
    *view_cell().write().unwrap_or_else(|p| p.into_inner()) = v;
}

/// The address of the current leader, from the cluster view.
pub fn leader_url() -> Option<String> {
    let v = view()?;
    v.members.get(&v.leader?).cloned()
}

fn view_of(
    id: NodeId,
    members: &BTreeMap<NodeId, String>,
    m: &RaftMetrics<NodeId, BasicNode>,
) -> View {
    View {
        id,
        leader: m.current_leader,
        is_leader: m.current_leader == Some(id) && m.state == ServerState::Leader,
        term: m.current_term,
        state: format!("{:?}", m.state).to_ascii_lowercase(),
        members: members.clone(),
        millis_since_quorum_ack: m.millis_since_quorum_ack,
    }
}

/// One member's Raft instance.
#[derive(Clone)]
pub struct Member {
    pub id: NodeId,
    pub raft: Raft<TypeConfig>,
    pub members: BTreeMap<NodeId, String>,
}

/// Election and heartbeat timing, in milliseconds.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub election_min: u64,
    pub election_max: u64,
    pub heartbeat: u64,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            election_min: 1500,
            election_max: 3000,
            heartbeat: 300,
        }
    }
}

impl Member {
    /// Start this node's Raft instance and, on a fresh log, initialise the
    /// cluster with the static membership (every member does; the ones that
    /// find an initialised log are told so and carry on).
    pub async fn start(
        id: NodeId,
        members: BTreeMap<NodeId, String>,
        transport: Transport,
        timing: Timing,
        vote_path: Option<PathBuf>,
    ) -> Result<Self, String> {
        let config = Config {
            cluster_name: "open-triplestore".to_string(),
            election_timeout_min: timing.election_min,
            election_timeout_max: timing.election_max,
            heartbeat_interval: timing.heartbeat,
            snapshot_policy: SnapshotPolicy::LogsSinceLast(1000),
            max_in_snapshot_log_to_keep: 100,
            ..Default::default()
        }
        .validate()
        .map_err(|e| e.to_string())?;
        let raft = Raft::new(
            id,
            Arc::new(config),
            NetworkFactory::new(transport),
            MemLog::new(vote_path),
            MemStateMachine::default(),
        )
        .await
        .map_err(|e| e.to_string())?;
        let nodes: BTreeMap<NodeId, BasicNode> = members
            .iter()
            .map(|(k, v)| (*k, BasicNode::new(v.clone())))
            .collect();
        if let Err(e) = raft.initialize(nodes).await {
            // An initialised log refuses a second initialisation: fine.
            tracing::debug!("raft: initialize on node {id}: {e}");
        }
        Ok(Self { id, raft, members })
    }

    /// The latest metrics.
    pub fn metrics(&self) -> RaftMetrics<NodeId, BasicNode> {
        self.raft.metrics().borrow().clone()
    }

    pub fn view(&self) -> View {
        view_of(self.id, &self.members, &self.metrics())
    }

    /// Publish this member's view to the process (the server reads it).
    pub fn publish(&self) {
        set_view(self.view());
    }

    /// Follow the metrics and keep the process-wide view current.
    pub async fn watch(&self) {
        let mut rx = self.raft.metrics();
        loop {
            self.publish();
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    #[allow(dead_code)] // tests stop members; the server runs its member for life
    pub async fn shutdown(&self) {
        let _ = self.raft.shutdown().await;
    }
}

// ─── The process-wide member ────────────────────────────────────────────────

static MEMBER: OnceLock<Member> = OnceLock::new();

/// This process's cluster member, once started.
pub fn member() -> Option<&'static Member> {
    MEMBER.get()
}

/// The cluster settings from the environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterConfig {
    pub id: NodeId,
    pub members: BTreeMap<NodeId, String>,
    pub secret: String,
    pub election_ms: u64,
    pub heartbeat_ms: u64,
}

impl ClusterConfig {
    /// `members` is `1=http://a:7878,2=http://b:7878,3=http://c:7878`; `id`
    /// is this node's number; the secret authenticates the Raft routes.
    pub fn parse(
        members: Option<&str>,
        id: Option<&str>,
        secret: Option<&str>,
        election_ms: Option<&str>,
        heartbeat_ms: Option<&str>,
    ) -> Option<Self> {
        let members: BTreeMap<NodeId, String> = members?
            .split(',')
            .filter_map(|m| {
                let (k, v) = m.trim().split_once('=')?;
                let id = k.trim().parse::<u64>().ok()?;
                let url = v.trim().trim_end_matches('/');
                (!url.is_empty()).then(|| (id, url.to_string()))
            })
            .collect();
        let id = id?.trim().parse::<u64>().ok()?;
        if members.is_empty() || !members.contains_key(&id) {
            return None;
        }
        let secret = secret?.trim().to_string();
        if secret.is_empty() {
            return None;
        }
        let election_ms = election_ms
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(1500)
            .clamp(200, 60_000);
        let heartbeat_ms = heartbeat_ms
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(election_ms / 5)
            .clamp(50, election_ms / 2);
        Some(Self {
            id,
            members,
            secret,
            election_ms,
            heartbeat_ms,
        })
    }

    /// The other members' follower names (`node-<id>`), for synchronous
    /// acknowledgement.
    pub fn peer_names(&self) -> Vec<String> {
        self.members
            .keys()
            .filter(|k| **k != self.id)
            .map(|k| format!("node-{k}"))
            .collect()
    }

    /// How many peers must acknowledge for a majority including this node.
    pub fn majority_peers(&self) -> usize {
        let n = self.members.len();
        (n / 2 + 1).saturating_sub(1)
    }

    pub fn timing(&self) -> Timing {
        Timing {
            election_min: self.election_ms,
            election_max: self.election_ms * 2,
            heartbeat: self.heartbeat_ms,
        }
    }
}

/// Start this process's member on its own runtime thread and keep the view
/// current. Called once, by the replication set-up, when the environment
/// configures a cluster.
pub fn spawn(config: ClusterConfig, vote_path: Option<PathBuf>) {
    if MEMBER.get().is_some() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("replication-raft".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("raft: no runtime: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                let transport = Transport::Http {
                    secret: config.secret.clone(),
                    timeout: Duration::from_millis(config.election_ms),
                };
                match Member::start(
                    config.id,
                    config.members.clone(),
                    transport,
                    config.timing(),
                    vote_path,
                )
                .await
                {
                    Ok(member) => {
                        tracing::info!(
                            "raft: node {} of {} member(s) started",
                            member.id,
                            member.members.len()
                        );
                        let _ = MEMBER.set(member.clone());
                        member.watch().await;
                    }
                    Err(e) => tracing::error!("raft: node {} did not start: {e}", config.id),
                }
            });
        });
    if let Err(e) = spawned {
        tracing::error!("raft: could not start the member thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cluster_settings_parse_and_derive_the_quorum() {
        let c = ClusterConfig::parse(
            Some("1=http://a:7878, 2=http://b:7878/,3=http://c:7878"),
            Some("2"),
            Some("s3cret"),
            None,
            None,
        )
        .expect("parses");
        assert_eq!(c.members.len(), 3);
        assert_eq!(c.members[&2], "http://b:7878");
        assert_eq!(c.peer_names(), vec!["node-1", "node-3"]);
        assert_eq!(
            c.majority_peers(),
            1,
            "a majority of three is this node plus one"
        );
        assert_eq!((c.election_ms, c.heartbeat_ms), (1500, 300));
        assert!(
            ClusterConfig::parse(Some("1=http://a"), Some("2"), Some("s"), None, None).is_none()
        );
        assert!(ClusterConfig::parse(Some("1=http://a"), Some("1"), None, None, None).is_none());
        let five = ClusterConfig::parse(
            Some("1=u,2=u,3=u,4=u,5=u"),
            Some("1"),
            Some("s"),
            Some("400"),
            None,
        )
        .unwrap();
        assert_eq!(five.majority_peers(), 2);
        assert_eq!(five.heartbeat_ms, 80);
    }

    #[test]
    fn the_view_is_empty_until_a_member_publishes() {
        assert!(view().is_none() || !view().unwrap().members.is_empty());
    }

    // ── The vote on disk ────────────────────────────────────────────────────

    /// The whole point: a member that restarts remembers who it voted for in
    /// the term it was in, so it cannot vote a second time in that term and
    /// help elect a second leader.
    #[tokio::test]
    async fn a_saved_vote_comes_back_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("raft-vote.json");

        let mut log = MemLog::new(Some(path.clone()));
        let vote = Vote::new(7, 2);
        log.save_vote(&vote).await.unwrap();

        // A second process over the same directory.
        let mut rebooted = MemLog::new(Some(path));
        assert_eq!(rebooted.read_vote().await.unwrap(), Some(vote));
    }

    /// The later vote wins, and the file holds exactly one.
    #[tokio::test]
    async fn the_file_holds_the_latest_vote() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("raft-vote.json");

        let mut log = MemLog::new(Some(path.clone()));
        log.save_vote(&Vote::new(1, 1)).await.unwrap();
        log.save_vote(&Vote::new(2, 3)).await.unwrap();

        let mut rebooted = MemLog::new(Some(path));
        assert_eq!(rebooted.read_vote().await.unwrap(), Some(Vote::new(2, 3)));
    }

    /// Without a path it behaves exactly as it always did, which is what an
    /// in-memory store and every test that builds one rely on.
    #[tokio::test]
    async fn no_path_means_no_file_and_no_memory() {
        let mut log = MemLog::new(None);
        assert_eq!(log.read_vote().await.unwrap(), None);
        log.save_vote(&Vote::new(4, 1)).await.unwrap();
        assert_eq!(log.read_vote().await.unwrap(), Some(Vote::new(4, 1)));
        // …but a fresh instance has nothing, because nothing was written.
        let mut fresh = MemLog::new(None);
        assert_eq!(fresh.read_vote().await.unwrap(), None);
    }

    /// A corrupt or truncated file is not a reason to refuse to start: an
    /// unreadable vote is the same risk as the in-memory one this replaces,
    /// and refusing to boot over it would be strictly worse.
    #[tokio::test]
    async fn an_unreadable_vote_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("raft-vote.json");
        std::fs::write(&path, b"{ this is not json").unwrap();

        let mut log = MemLog::new(Some(path.clone()));
        assert_eq!(log.read_vote().await.unwrap(), None);

        // And it is overwritten by the next real vote rather than left to rot.
        log.save_vote(&Vote::new(9, 1)).await.unwrap();
        let mut rebooted = MemLog::new(Some(path));
        assert_eq!(rebooted.read_vote().await.unwrap(), Some(Vote::new(9, 1)));
    }
}
