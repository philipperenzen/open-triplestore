//! Per-quad change capture with a durable cursor — the first deliverable of
//! the analytical-layer plan and the log a read replica tails
//! (`docs/notes/delta-versioning-design.md` §2 and §4.3,
//! `analytical-mirror-design.md` §5.2).
//!
//! Every engine primitive that mutates the store writes one row per graph it
//! touches into `{data_dir}/changes/changes.db` (an in-memory SQLite for an
//! in-memory store): the *intent* row goes in before the store is touched,
//! the data commits, and inside a short critical section the row is
//! finalised with its payload — the added and removed quads as N-Quads — and
//! a sequence number handed out in commit order. `seq` is the durable cursor
//! oxigraph does not expose; `row` only identifies the intent.
//!
//! The payload is the *net* delta the primitive already holds or can bound:
//! free where the quads are in a local, a `contains` probe where they are
//! not, a before-image scan capped at `OTS_CHANGE_CAPTURE_MAX_SCAN` quads for
//! a non-ground SPARQL update. Beyond a cap the row keeps exact counts
//! (`extent = counts`) or admits it knows nothing (`unknown`); a consumer
//! treats `unknown` as a hard boundary. Nothing here is atomic with the
//! RocksDB commit — a crash between the two leaves a `pending` row, and the
//! next open resolves it by probing the store — so every gap is detectable
//! and none is guessed.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;

use oxigraph::io::{RdfFormat, RdfParser, RdfSerializer};
use oxigraph::model::{NamedOrBlankNode, Quad, Term};
use rusqlite::{params, Connection, OptionalExtension};

use super::StoreError;

pub const DEFAULT_MAX_SCAN: usize = 250_000;
pub const DEFAULT_MAX_PAYLOAD: usize = 250_000;
pub const DEFAULT_RETENTION_DAYS: u64 = 90;
pub const DEFAULT_CURSOR_TTL_DAYS: u64 = 30;
/// Payloads above this many bytes of N-Quads are stored gzipped.
const GZIP_ABOVE: usize = 64 * 1024;
/// Rows between retention sweeps on the write path.
const SWEEP_EVERY_ROWS: u64 = 1_000;

/// `(row, extent, added, removed, scope)` of a row left pending.
type PendingRow = (i64, String, Option<Vec<u8>>, Option<Vec<u8>>, String);

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn env_on(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            )
        })
        .unwrap_or(false)
}

fn env_off(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(false)
}

/// How much of the delta a row carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Extent {
    /// The payload is the complete net delta.
    Full,
    /// Counts are exact, the payload was over the cap and is not stored.
    Counts,
    /// Nothing about the delta is known; a chain cannot cross this row.
    Unknown,
}

impl Extent {
    fn as_str(self) -> &'static str {
        match self {
            Extent::Full => "full",
            Extent::Counts => "counts",
            Extent::Unknown => "unknown",
        }
    }
    fn parse(s: &str) -> Extent {
        match s {
            "full" => Extent::Full,
            "counts" => Extent::Counts,
            _ => Extent::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Pending,
    Committed,
    Aborted,
    /// Committed as far as anyone knows, delta unknown (a resolved crash, a
    /// count that disagreed at open, a primitive that cannot bound itself).
    Unknown,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Committed => "committed",
            State::Aborted => "aborted",
            State::Unknown => "unknown",
        }
    }
    fn parse(s: &str) -> State {
        match s {
            "pending" => State::Pending,
            "committed" => State::Committed,
            "aborted" => State::Aborted,
            _ => State::Unknown,
        }
    }
}

/// One graph's part of a write, as the primitive knows it. `None` graph is
/// the default graph.
#[derive(Debug, Clone)]
pub struct GraphDelta {
    pub graph: Option<String>,
    pub added: Vec<Quad>,
    pub removed: Vec<Quad>,
    pub extent: Extent,
    pub added_n: usize,
    pub removed_n: usize,
    /// The graph's quad count after the write, when the primitive knows it
    /// better than `pre + added − removed` (a replace, a drop).
    pub post_count: Option<usize>,
}

impl GraphDelta {
    /// The complete net delta.
    pub fn full(graph: Option<String>, added: Vec<Quad>, removed: Vec<Quad>) -> Self {
        Self {
            graph,
            added_n: added.len(),
            removed_n: removed.len(),
            added,
            removed,
            extent: Extent::Full,
            post_count: None,
        }
    }

    /// Exact counts, no payload.
    pub fn counts(
        graph: Option<String>,
        added_n: usize,
        removed_n: usize,
        post_count: Option<usize>,
    ) -> Self {
        Self {
            graph,
            added: Vec::new(),
            removed: Vec::new(),
            extent: Extent::Counts,
            added_n,
            removed_n,
            post_count,
        }
    }

    /// Nothing is known about what changed in this graph.
    pub fn unknown(graph: Option<String>) -> Self {
        Self {
            graph,
            added: Vec::new(),
            removed: Vec::new(),
            extent: Extent::Unknown,
            added_n: 0,
            removed_n: 0,
            post_count: None,
        }
    }

    pub fn with_post_count(mut self, n: usize) -> Self {
        self.post_count = Some(n);
        self
    }

    fn has_bnode(&self) -> bool {
        self.added.iter().chain(self.removed.iter()).any(|q| {
            matches!(q.subject, NamedOrBlankNode::BlankNode(_))
                || matches!(q.object, Term::BlankNode(_))
        })
    }
}

/// A write's rows between `begin` and its commit or abort.
#[derive(Debug)]
pub struct Intent {
    txn: String,
    /// `(graph, row id, count before the write)` for the targets known at
    /// entry; a `None` graph key is the default graph.
    rows: Vec<(Option<String>, i64, Option<usize>)>,
    /// The single `scope = store` row when the targets were unknown.
    store_row: Option<i64>,
}

/// A row as a consumer reads it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChangeRow {
    pub row: i64,
    pub seq: Option<i64>,
    pub epoch: String,
    pub txn: String,
    /// `graph` or `store`.
    pub scope: String,
    pub graph_iri: Option<String>,
    pub origin: String,
    pub kind: Option<String>,
    pub commit_iri: Option<String>,
    pub actor_iri: Option<String>,
    pub extent: Extent,
    /// N-Quads text, present for `full` rows.
    pub added: Option<String>,
    pub removed: Option<String>,
    pub added_n: i64,
    pub removed_n: i64,
    pub post_count: Option<i64>,
    pub has_bnode: bool,
    pub state: State,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Cursor {
    pub name: String,
    pub seq: i64,
    pub owner: Option<String>,
    pub expires_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Status {
    pub enabled: bool,
    pub epoch: String,
    pub next_seq: i64,
    pub rows: i64,
    pub committed: i64,
    pub pending: i64,
    pub unknown: i64,
    pub oldest_seq: Option<i64>,
    pub newest_seq: Option<i64>,
    pub cursors: Vec<Cursor>,
    pub max_scan: usize,
    pub max_payload: usize,
    pub retention_days: u64,
    pub size_bytes: Option<u64>,
}

// ─── Who is writing, and how deep ───────────────────────────────────────────

/// Actor, commit and kind for the rows of the write running on this thread.
/// Set inside the blocking closure that calls the primitive, never across an
/// `.await` (a thread-local does not follow a task).
#[derive(Debug, Clone, Default)]
pub struct WriteContext {
    pub actor_iri: Option<String>,
    pub commit_iri: Option<String>,
    pub kind: Option<String>,
}

thread_local! {
    static WRITE_DEPTH: Cell<u32> = const { Cell::new(0) };
    static RECORDED: Cell<bool> = const { Cell::new(false) };
    static CONTEXT: RefCell<Option<WriteContext>> = const { RefCell::new(None) };
}

pub struct WriteContextGuard(Option<WriteContext>);

impl WriteContextGuard {
    pub fn set(ctx: WriteContext) -> Self {
        Self(CONTEXT.with(|c| c.replace(Some(ctx))))
    }
}

impl Drop for WriteContextGuard {
    fn drop(&mut self) {
        CONTEXT.with(|c| *c.borrow_mut() = self.0.take());
    }
}

fn context() -> WriteContext {
    CONTEXT.with(|c| c.borrow().clone().unwrap_or_default())
}

/// The write guard entered a primitive (possibly nested in another). The
/// outermost entry starts clean: nothing of this write is recorded yet.
pub(crate) fn enter_write() {
    WRITE_DEPTH.with(|d| {
        if d.get() == 0 {
            RECORDED.with(|r| r.set(false));
        }
        d.set(d.get() + 1);
    });
}

/// The write guard left; the outermost leave forgets who recorded.
pub(crate) fn leave_write() {
    WRITE_DEPTH.with(|d| {
        let n = d.get().saturating_sub(1);
        d.set(n);
        if n == 0 {
            RECORDED.with(|r| r.set(false));
        }
    });
}

/// The first primitive of a write to ask gets to record it; a primitive a
/// recording one delegates to is told no, so a Graph Store PUT that loads
/// through the bulk path is one row, not two.
fn claim() -> bool {
    // Outside a write guard (a direct caller, a test) there is no nesting
    // to suppress, and nothing to remember.
    if WRITE_DEPTH.with(|d| d.get()) == 0 {
        return true;
    }
    RECORDED.with(|r| !r.replace(true))
}

// ─── The log ────────────────────────────────────────────────────────────────

pub struct ChangeLog {
    conn: Option<Mutex<Connection>>,
    epoch: String,
    /// The next sequence number. Handed out under `section`; persisted in
    /// `meta` before a sweep (which may empty the table) and by the open-time
    /// repair, and recomputed from `MAX(seq)` at open.
    next_seq: AtomicI64,
    /// Brackets the data commit and the row finalisation, so `seq` order is
    /// commit order without serialising evaluation (§4.3).
    section: Mutex<()>,
    max_scan: usize,
    max_payload: usize,
    retention_days: u64,
    cursor_ttl_days: u64,
    rows_since_sweep: AtomicU64,
    path: Option<std::path::PathBuf>,
    /// Woken after every finalised write: the long-poll on the change
    /// endpoint waits on it.
    rows_notify: tokio::sync::Notify,
    /// Bumped on every cursor move: a synchronous leader waits on it.
    cursor_moves: (Mutex<u64>, std::sync::Condvar),
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS changes (
        row INTEGER PRIMARY KEY AUTOINCREMENT,
        seq INTEGER UNIQUE,
        epoch TEXT NOT NULL,
        txn TEXT NOT NULL,
        gen INTEGER NOT NULL DEFAULT 0,
        scope TEXT NOT NULL,
        graph_iri TEXT,
        origin TEXT NOT NULL,
        kind TEXT,
        commit_iri TEXT,
        actor_iri TEXT,
        extent TEXT NOT NULL,
        added BLOB,
        removed BLOB,
        added_n INTEGER NOT NULL DEFAULT 0,
        removed_n INTEGER NOT NULL DEFAULT 0,
        post_count INTEGER,
        has_bnode INTEGER NOT NULL DEFAULT 0,
        state TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_changes_seq ON changes(seq);
    CREATE INDEX IF NOT EXISTS idx_changes_state ON changes(state);
    CREATE INDEX IF NOT EXISTS idx_changes_graph ON changes(graph_iri, seq);
    CREATE TABLE IF NOT EXISTS graph_state (
        graph_key TEXT PRIMARY KEY,
        last_seq INTEGER,
        post_count INTEGER
    );
    CREATE TABLE IF NOT EXISTS cursors (
        name TEXT PRIMARY KEY,
        seq INTEGER NOT NULL,
        owner TEXT,
        expires_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
";

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn graph_key(graph: Option<&str>) -> String {
    graph.unwrap_or("").to_string()
}

fn sql_err(e: rusqlite::Error) -> StoreError {
    StoreError::Other(format!("change log: {e}"))
}

/// Execute through the connection's statement cache: the write path runs
/// the same handful of statements on every write.
fn exec(conn: &Connection, sql: &str, params: &[&dyn rusqlite::ToSql]) -> rusqlite::Result<usize> {
    conn.prepare_cached(sql)?.execute(params)
}

impl ChangeLog {
    /// Open the log beside a persistent store (`{dir}/changes/changes.db`),
    /// or in memory when `dir` is `None`. Capture is **on by default**
    /// (the maintainer's decision, 2026-09-16): every write pays for its
    /// row (docs/versioning.md has the measured cost) and the log is there
    /// the day a follower, the history or an audit needs it.
    /// `OTS_CHANGE_CAPTURE=off` turns it off; a replication leader keeps it
    /// on regardless (its followers read this log); a follower keeps it
    /// off unless `OTS_CHANGE_CAPTURE=on` (its log is not a source).
    pub fn open(dir: Option<&Path>) -> Result<Self, StoreError> {
        let (leader, follower) = (
            crate::store::replication::leader_role_configured()
                || crate::store::replication::cluster_role_configured(),
            crate::store::replication::follower_role_configured(),
        );
        let on = if leader {
            true
        } else if follower {
            env_on("OTS_CHANGE_CAPTURE")
        } else {
            !env_off("OTS_CHANGE_CAPTURE")
        };
        if !on {
            return Ok(Self::disabled());
        }
        Self::open_at(dir)
    }

    /// An in-memory log that is on regardless of the environment (the
    /// builder, tests).
    pub fn in_memory() -> Result<Self, StoreError> {
        Self::open_at(None)
    }

    fn open_at(dir: Option<&Path>) -> Result<Self, StoreError> {
        let (conn, path) = match dir {
            Some(dir) => {
                let sub = dir.join("changes");
                std::fs::create_dir_all(&sub)
                    .map_err(|e| StoreError::Other(format!("change log dir: {e}")))?;
                let path = sub.join("changes.db");
                (Connection::open(&path).map_err(sql_err)?, Some(path))
            }
            None => (Connection::open_in_memory().map_err(sql_err)?, None),
        };
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;",
        )
        .map_err(sql_err)?;
        conn.execute_batch(SCHEMA).map_err(sql_err)?;
        let epoch: String = match conn
            .query_row("SELECT value FROM meta WHERE key = 'epoch'", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(sql_err)?
        {
            Some(e) => e,
            None => {
                let e = uuid::Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('epoch', ?1), ('next_seq', '1')",
                    params![e],
                )
                .map_err(sql_err)?;
                e
            }
        };
        let meta_next: i64 = conn
            .query_row("SELECT value FROM meta WHERE key = 'next_seq'", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let max_seq: i64 = conn
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM changes", [], |r| {
                r.get(0)
            })
            .map_err(sql_err)?;
        Ok(Self {
            conn: Some(Mutex::new(conn)),
            epoch,
            next_seq: AtomicI64::new(meta_next.max(max_seq + 1)),
            section: Mutex::new(()),
            max_scan: env_usize("OTS_CHANGE_CAPTURE_MAX_SCAN", DEFAULT_MAX_SCAN),
            max_payload: env_usize("OTS_CHANGE_CAPTURE_MAX_PAYLOAD", DEFAULT_MAX_PAYLOAD),
            retention_days: env_u64("OTS_CHANGE_RETENTION_DAYS", DEFAULT_RETENTION_DAYS),
            cursor_ttl_days: env_u64("OTS_CURSOR_TTL_DAYS", DEFAULT_CURSOR_TTL_DAYS),
            rows_since_sweep: AtomicU64::new(0),
            path,
            rows_notify: tokio::sync::Notify::new(),
            cursor_moves: (Mutex::new(0), std::sync::Condvar::new()),
        })
    }

    /// A log that records nothing (the off switch).
    pub fn disabled() -> Self {
        Self {
            conn: None,
            epoch: String::new(),
            next_seq: AtomicI64::new(0),
            section: Mutex::new(()),
            max_scan: 0,
            max_payload: 0,
            retention_days: 0,
            cursor_ttl_days: 0,
            rows_since_sweep: AtomicU64::new(0),
            path: None,
            rows_notify: tokio::sync::Notify::new(),
            cursor_moves: (Mutex::new(0), std::sync::Condvar::new()),
        }
    }

    /// Override the caps (builder style; tests).
    pub fn with_caps(mut self, max_scan: usize, max_payload: usize) -> Self {
        self.max_scan = max_scan;
        self.max_payload = max_payload;
        self
    }

    pub fn enabled(&self) -> bool {
        self.conn.is_some()
    }

    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// The before-image scan cap: a primitive scans its targets only when
    /// their summed counts are at or below this.
    pub fn max_scan(&self) -> usize {
        self.max_scan
    }

    /// The payload cap in quads: above it a row keeps exact counts only.
    pub fn max_payload(&self) -> usize {
        self.max_payload
    }

    fn lock(&self) -> Option<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .as_ref()
            .map(|m| m.lock().unwrap_or_else(|p| p.into_inner()))
    }

    // ── Recording ───────────────────────────────────────────────────────────

    /// Open the intent rows for a write about to happen: one per known
    /// target (`Some(targets)`), or one store-scoped row when the targets
    /// cannot be known before the write. `pre_count` reads the count index.
    /// `None` when the log is off or another primitive of this write already
    /// records it.
    pub fn begin(
        &self,
        origin: &'static str,
        targets: Option<&[Option<String>]>,
        pre_count: &dyn Fn(Option<&str>) -> Option<usize>,
    ) -> Option<Intent> {
        // The commit trail's own graph is never recorded: a row per commit
        // would double every handler write, and the trail is derived data.
        if targets.is_some_and(|t| {
            !t.is_empty()
                && t.iter()
                    .all(|g| g.as_deref() == Some(crate::commit_log::COMMIT_GRAPH))
        }) {
            return None;
        }
        let conn = self.lock()?;
        if !claim() {
            return None;
        }
        let ctx = context();
        let txn = uuid::Uuid::new_v4().to_string();
        let created = now();
        let insert = |graph: Option<&str>, scope: &str| -> Option<i64> {
            exec(&conn,
                "INSERT INTO changes (epoch, txn, scope, graph_iri, origin, kind, commit_iri, actor_iri, extent, state, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'unknown', 'pending', ?9)",
                params![self.epoch, txn, scope, graph, origin, ctx.kind, ctx.commit_iri, ctx.actor_iri, created],
            )
            .ok()?;
            Some(conn.last_insert_rowid())
        };
        let mut rows = Vec::new();
        let mut store_row = None;
        match targets {
            Some(targets) => {
                let mut seen: Vec<Option<String>> = Vec::new();
                for g in targets {
                    if seen.contains(g) {
                        continue;
                    }
                    seen.push(g.clone());
                    let row = insert(g.as_deref(), "graph")?;
                    rows.push((g.clone(), row, pre_count(g.as_deref())));
                }
            }
            None => {
                store_row = insert(None, "store");
            }
        }
        Some(Intent {
            txn,
            rows,
            store_row,
        })
    }

    /// Run the data commit inside the sequence section and finalise the rows
    /// with `deltas` — one per graph; a graph with an intent row but no delta
    /// is committed unchanged. A commit error aborts the rows and is passed
    /// through. Rows for graphs the intent did not know are added.
    pub fn commit_with<E>(
        &self,
        intent: Option<Intent>,
        deltas: Vec<GraphDelta>,
        commit: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        let Some(intent) = intent else {
            return commit();
        };
        let _section = self.section.lock().unwrap_or_else(|p| p.into_inner());
        match commit() {
            Ok(()) => {
                self.finalise(intent, deltas);
                Ok(())
            }
            Err(e) => {
                self.abort(intent);
                Err(e)
            }
        }
    }

    /// As [`Self::commit_with`] for a primitive that cannot bound its delta:
    /// every row becomes `unknown`, per graph when `graphs` names them, else
    /// the store-scoped row.
    pub fn commit_unknown<E>(
        &self,
        intent: Option<Intent>,
        graphs: Option<Vec<Option<String>>>,
        commit: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        let deltas = graphs
            .map(|gs| gs.into_iter().map(GraphDelta::unknown).collect())
            .unwrap_or_default();
        self.commit_with(intent, deltas, commit)
    }

    /// The write failed before its commit.
    pub fn abort(&self, intent: Intent) {
        let Some(conn) = self.lock() else { return };
        let rows: Vec<i64> = intent
            .rows
            .iter()
            .map(|(_, r, _)| *r)
            .chain(intent.store_row)
            .collect();
        for r in rows {
            let _ = conn.execute(
                "UPDATE changes SET state = 'aborted' WHERE row = ?1 AND state = 'pending'",
                params![r],
            );
        }
    }

    fn finalise(&self, intent: Intent, mut deltas: Vec<GraphDelta>) {
        let Some(mut conn) = self.lock() else { return };
        let Ok(tx) = conn.transaction() else { return };
        let mut next: i64 = self.next_seq.load(Ordering::Relaxed);
        let mut written = 0u64;

        // A store-scoped intent finalises as one unknown row unless the
        // primitive learnt its graphs after the fact.
        if let Some(store_row) = intent.store_row {
            if deltas.is_empty() {
                let _ = exec(&tx,
                    "UPDATE changes SET seq = ?2, extent = 'unknown', state = 'unknown' WHERE row = ?1",
                    params![store_row, next],
                );
                next += 1;
                written += 1;
            } else {
                let _ = exec(
                    &tx,
                    "UPDATE changes SET state = 'aborted' WHERE row = ?1",
                    params![store_row],
                );
            }
        }

        // Every intent row: its delta, or "unchanged".
        let mut pending: Vec<(Option<String>, i64, Option<usize>)> = intent.rows;
        for (graph, row, pre) in pending.drain(..) {
            let delta = match deltas.iter().position(|d| d.graph == graph) {
                Some(i) => deltas.swap_remove(i),
                None => GraphDelta::full(graph.clone(), Vec::new(), Vec::new()),
            };
            self.write_row(&tx, row, &delta, pre, next);
            next += 1;
            written += 1;
        }
        // Deltas for graphs the intent did not know (a primitive that
        // discovered its targets while running).
        for delta in deltas {
            let ctx = context();
            if tx
                .execute(
                    "INSERT INTO changes (epoch, txn, scope, graph_iri, origin, kind, commit_iri, actor_iri, extent, state, created_at) \
                     VALUES (?1, ?2, 'graph', ?3, 'late', ?4, ?5, ?6, 'unknown', 'pending', ?7)",
                    params![self.epoch, intent.txn, delta.graph, ctx.kind, ctx.commit_iri, ctx.actor_iri, now()],
                )
                .is_err()
            {
                continue;
            }
            let row = tx.last_insert_rowid();
            self.write_row(&tx, row, &delta, None, next);
            next += 1;
            written += 1;
        }
        let _ = tx.commit();
        self.next_seq.store(next, Ordering::Relaxed);
        drop(conn);
        self.rows_notify.notify_waiters();
        if self.rows_since_sweep.fetch_add(written, Ordering::Relaxed) + written >= SWEEP_EVERY_ROWS
        {
            self.rows_since_sweep.store(0, Ordering::Relaxed);
            let _ = self.sweep();
        }
    }

    fn write_row(
        &self,
        tx: &rusqlite::Transaction<'_>,
        row: i64,
        delta: &GraphDelta,
        pre: Option<usize>,
        seq: i64,
    ) {
        let (extent, added, removed) = match delta.extent {
            Extent::Full if delta.added.len() + delta.removed.len() <= self.max_payload => (
                Extent::Full,
                encode_quads(&delta.added),
                encode_quads(&delta.removed),
            ),
            Extent::Full => (Extent::Counts, None, None),
            other => (other, None, None),
        };
        // An unknown row keeps only a count the primitive stated outright.
        let post_count: Option<i64> = match extent {
            Extent::Unknown => delta.post_count.map(|n| n as i64),
            _ => delta
                .post_count
                .map(|n| n as i64)
                .or_else(|| pre.map(|p| p as i64 + delta.added_n as i64 - delta.removed_n as i64)),
        };
        let state = if extent == Extent::Unknown {
            State::Unknown
        } else {
            State::Committed
        };
        let _ = exec(tx,
            "UPDATE changes SET seq = ?2, extent = ?3, added = ?4, removed = ?5, added_n = ?6, removed_n = ?7, \
             post_count = ?8, has_bnode = ?9, state = ?10 WHERE row = ?1",
            params![
                row,
                seq,
                extent.as_str(),
                added,
                removed,
                delta.added_n as i64,
                delta.removed_n as i64,
                post_count,
                delta.has_bnode() as i32,
                state.as_str()
            ],
        );
        let _ = exec(
            tx,
            "INSERT INTO graph_state (graph_key, last_seq, post_count) VALUES (?1, ?2, ?3) \
             ON CONFLICT(graph_key) DO UPDATE SET last_seq = excluded.last_seq, \
             post_count = COALESCE(excluded.post_count, graph_state.post_count)",
            params![graph_key(delta.graph.as_deref()), seq, post_count],
        );
    }

    // ── Waiting ─────────────────────────────────────────────────────────────

    /// A future that resolves when a write is finalised after the call
    /// (`enable` it before checking for rows, so a row that lands in
    /// between is not missed). The long-poll of `GET /api/admin/changes`.
    pub fn rows_notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.rows_notify.notified()
    }

    /// How many of `names` have a cursor at or beyond `seq`.
    pub fn cursors_at(&self, names: &[String], seq: i64) -> usize {
        names
            .iter()
            .filter(|n| self.cursor(n).is_some_and(|c| c.seq >= seq))
            .count()
    }

    /// Wait until `required` of `names` have a cursor at or beyond `seq`,
    /// or `timeout` passes. Returns how many had, and whether enough did.
    /// Woken by every cursor move, so a follower's acknowledgement is seen
    /// as soon as it arrives.
    pub fn wait_for_cursors(
        &self,
        names: &[String],
        seq: i64,
        required: usize,
        timeout: std::time::Duration,
    ) -> (usize, bool) {
        let deadline = std::time::Instant::now() + timeout;
        let (lock, cv) = &self.cursor_moves;
        let mut seen = lock.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            let acked = self.cursors_at(names, seq);
            if acked >= required {
                return (acked, true);
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return (acked, false);
            }
            let (guard, _) = cv
                .wait_timeout(seen, deadline - now)
                .unwrap_or_else(|p| p.into_inner());
            seen = guard;
        }
    }

    // ── Reading ─────────────────────────────────────────────────────────────

    /// Rows with a sequence number above `after`, in commit order.
    pub fn rows_after(&self, after: i64, limit: usize) -> Vec<ChangeRow> {
        let Some(conn) = self.lock() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT row, seq, epoch, txn, scope, graph_iri, origin, kind, commit_iri, actor_iri, extent, \
                    added, removed, added_n, removed_n, post_count, has_bnode, state, created_at \
             FROM changes WHERE seq > ?1 ORDER BY seq LIMIT ?2",
        ) else {
            return Vec::new();
        };
        stmt.query_map(params![after, limit as i64], row_from_sql)
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// Rows above `after` for one graph — plus every store-scoped row, which
    /// a reader of any graph must see — or all rows when `graph` is `None`.
    pub fn rows_after_in(&self, after: i64, limit: usize, graph: Option<&str>) -> Vec<ChangeRow> {
        let Some(graph) = graph else {
            return self.rows_after(after, limit);
        };
        let Some(conn) = self.lock() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT row, seq, epoch, txn, scope, graph_iri, origin, kind, commit_iri, actor_iri, extent, \
                    added, removed, added_n, removed_n, post_count, has_bnode, state, created_at \
             FROM changes WHERE seq > ?1 AND (graph_iri = ?3 OR scope = 'store') \
             ORDER BY seq LIMIT ?2",
        ) else {
            return Vec::new();
        };
        stmt.query_map(params![after, limit as i64, graph], row_from_sql)
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// One row by sequence number.
    pub fn row(&self, seq: i64) -> Option<ChangeRow> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT row, seq, epoch, txn, scope, graph_iri, origin, kind, commit_iri, actor_iri, extent, \
                    added, removed, added_n, removed_n, post_count, has_bnode, state, created_at \
             FROM changes WHERE seq = ?1",
            params![seq],
            row_from_sql,
        )
        .optional()
        .ok()
        .flatten()
    }

    /// The last sequence number handed out (0 before the first write). From
    /// the in-memory counter, so the write path can ask on every write.
    pub fn last_seq(&self) -> i64 {
        if self.conn.is_none() {
            return 0;
        }
        (self.next_seq.load(Ordering::Relaxed) - 1).max(0)
    }

    pub fn status(&self) -> Status {
        let Some(conn) = self.lock() else {
            return Status {
                enabled: false,
                epoch: String::new(),
                next_seq: 0,
                rows: 0,
                committed: 0,
                pending: 0,
                unknown: 0,
                oldest_seq: None,
                newest_seq: None,
                cursors: Vec::new(),
                max_scan: 0,
                max_payload: 0,
                retention_days: 0,
                size_bytes: None,
            };
        };
        let count = |state: &str| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM changes WHERE state = ?1",
                params![state],
                |r| r.get(0),
            )
            .unwrap_or(0)
        };
        let next_seq = self.next_seq.load(Ordering::Relaxed);
        let (oldest, newest): (Option<i64>, Option<i64>) = conn
            .query_row("SELECT MIN(seq), MAX(seq) FROM changes", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap_or((None, None));
        let cursors = cursors_of(&conn);
        let size_bytes = self
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len());
        Status {
            enabled: true,
            epoch: self.epoch.clone(),
            next_seq,
            rows: conn
                .query_row("SELECT COUNT(*) FROM changes", [], |r| r.get(0))
                .unwrap_or(0),
            committed: count("committed"),
            pending: count("pending"),
            unknown: count("unknown"),
            oldest_seq: oldest,
            newest_seq: newest,
            cursors,
            max_scan: self.max_scan,
            max_payload: self.max_payload,
            retention_days: self.retention_days,
            size_bytes,
        }
    }

    // ── Cursors ─────────────────────────────────────────────────────────────

    /// Record where a consumer is. Refreshes the expiry.
    pub fn set_cursor(&self, name: &str, seq: i64, owner: Option<&str>) -> Result<(), StoreError> {
        let Some(conn) = self.lock() else {
            return Ok(());
        };
        let expires =
            (chrono::Utc::now() + chrono::Duration::days(self.cursor_ttl_days as i64)).to_rfc3339();
        conn.execute(
            "INSERT INTO cursors (name, seq, owner, expires_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(name) DO UPDATE SET seq = excluded.seq, owner = COALESCE(excluded.owner, cursors.owner), \
             expires_at = excluded.expires_at, updated_at = excluded.updated_at",
            params![name, seq, owner, expires, now()],
        )
        .map_err(sql_err)?;
        {
            let (lock, cv) = &self.cursor_moves;
            let mut moves = lock.lock().unwrap_or_else(|p| p.into_inner());
            *moves += 1;
            cv.notify_all();
        }
        Ok(())
    }

    pub fn cursor(&self, name: &str) -> Option<Cursor> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT name, seq, owner, expires_at, updated_at FROM cursors WHERE name = ?1",
            params![name],
            cursor_from_sql,
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn delete_cursor(&self, name: &str) -> bool {
        self.lock()
            .and_then(|c| {
                c.execute("DELETE FROM cursors WHERE name = ?1", params![name])
                    .ok()
            })
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn cursors(&self) -> Vec<Cursor> {
        self.lock().map(|c| cursors_of(&c)).unwrap_or_default()
    }

    // ── Retention ───────────────────────────────────────────────────────────

    /// Delete rows older than the retention window that no live cursor pins:
    /// payloads first, then the rows. Expired cursors stop pinning. Returns
    /// the number of rows deleted.
    pub fn sweep(&self) -> Result<usize, StoreError> {
        let Some(conn) = self.lock() else {
            return Ok(0);
        };
        let now_s = now();
        // The sequence must survive a sweep that empties the table.
        let _ = conn.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'next_seq'",
            params![self.next_seq.load(Ordering::Relaxed).to_string()],
        );
        conn.execute("DELETE FROM cursors WHERE expires_at < ?1", params![now_s])
            .map_err(sql_err)?;
        let lowest_pin: Option<i64> = conn
            .query_row("SELECT MIN(seq) FROM cursors", [], |r| r.get(0))
            .map_err(sql_err)?;
        let cutoff =
            (chrono::Utc::now() - chrono::Duration::days(self.retention_days as i64)).to_rfc3339();
        // Rows a cursor still needs are never touched; below the lowest pin,
        // age decides.
        let pin = lowest_pin.unwrap_or(i64::MAX);
        let deleted = conn
            .execute(
                "DELETE FROM changes WHERE state != 'pending' AND created_at < ?1 \
                 AND (seq IS NULL OR seq < ?2)",
                params![cutoff, pin],
            )
            .map_err(sql_err)?;
        Ok(deleted)
    }

    // ── Open-time repair ────────────────────────────────────────────────────

    /// Resolve every `pending` row left by a crash between the data commit and
    /// the row finalisation, by probing the store: a full-extent row whose
    /// added quads are all present and removed quads all absent committed
    /// (and is sequenced now); the inverse aborted; anything else — a partial
    /// state, a row without a payload, a store-scoped row — is `unknown`, a
    /// boundary no chain crosses. Returns `(committed, aborted, unknown)`.
    pub fn resolve_pending(
        &self,
        contains: &dyn Fn(&Quad) -> bool,
    ) -> Result<(usize, usize, usize), StoreError> {
        let Some(mut conn) = self.lock() else {
            return Ok((0, 0, 0));
        };
        let pending: Vec<PendingRow> = {
            let mut stmt = conn
                .prepare("SELECT row, extent, added, removed, scope FROM changes WHERE state = 'pending' ORDER BY row")
                .map_err(sql_err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })
                .map_err(sql_err)?;
            rows.flatten().collect()
        };
        if pending.is_empty() {
            return Ok((0, 0, 0));
        }
        let mut next: i64 = self.next_seq.load(Ordering::Relaxed);
        let tx = conn.transaction().map_err(sql_err)?;
        let (mut committed, mut aborted, mut unknown) = (0, 0, 0);
        // Rows sequenced here, per graph: two on one graph cannot be ordered
        // against each other, so the committed ones become unknown as well;
        // a store-scoped row breaks every chain.
        let mut sequenced: HashMap<Option<String>, Vec<(i64, bool)>> = HashMap::new();
        let mut store_row_sequenced = false;
        for (row, extent, added, removed, scope) in pending {
            let verdict = if scope == "graph" && extent == "full" {
                let added = decode_quads(added.as_deref());
                let removed = decode_quads(removed.as_deref());
                let all_added = added.iter().all(contains);
                let none_added = added.iter().all(|q| !contains(q));
                let none_removed = removed.iter().all(|q| !contains(q));
                let all_removed = removed.iter().all(contains);
                if (added.is_empty() && removed.is_empty()) || (all_added && none_removed) {
                    State::Committed
                } else if none_added && all_removed {
                    State::Aborted
                } else {
                    State::Unknown
                }
            } else {
                State::Unknown
            };
            let graph: Option<String> = tx
                .query_row(
                    "SELECT graph_iri FROM changes WHERE row = ?1",
                    params![row],
                    |r| r.get(0),
                )
                .ok()
                .flatten();
            match verdict {
                State::Committed => {
                    sequenced
                        .entry(graph.clone())
                        .or_default()
                        .push((row, true));
                    let _ = tx.execute(
                        "UPDATE changes SET state = 'committed', seq = ?2 WHERE row = ?1",
                        params![row, next],
                    );
                    next += 1;
                    committed += 1;
                }
                State::Aborted => {
                    let _ = tx.execute(
                        "UPDATE changes SET state = 'aborted' WHERE row = ?1",
                        params![row],
                    );
                    aborted += 1;
                }
                _ => {
                    if scope == "store" {
                        store_row_sequenced = true;
                    } else {
                        sequenced
                            .entry(graph.clone())
                            .or_default()
                            .push((row, false));
                    }
                    let _ = tx.execute(
                        "UPDATE changes SET state = 'unknown', extent = 'unknown', seq = ?2 WHERE row = ?1",
                        params![row, next],
                    );
                    next += 1;
                    unknown += 1;
                }
            }
        }
        for rows in sequenced.into_values() {
            if rows.len() < 2 && !store_row_sequenced {
                continue;
            }
            for (row, was_committed) in rows {
                if was_committed {
                    let _ = tx.execute(
                        "UPDATE changes SET state = 'unknown', extent = 'unknown' WHERE row = ?1",
                        params![row],
                    );
                    committed -= 1;
                    unknown += 1;
                }
            }
        }
        let _ = tx.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'next_seq'",
            params![next.to_string()],
        );
        tx.commit().map_err(sql_err)?;
        self.next_seq.store(next, Ordering::Relaxed);
        Ok((committed, aborted, unknown))
    }

    /// Compare the count each graph had after its last recorded write with the
    /// count the store has now; every disagreement — and every graph the log
    /// never saw but the store holds — gets a synthetic `unknown` row, so a
    /// write that produced no row at all still closes the chains it broke.
    /// Blind to a swap that keeps the count; the intent rows cover the crash
    /// case, this covers the bug case. Returns the graphs reconciled.
    pub fn reconcile_counts(&self, live: &[(Option<String>, usize)]) -> Vec<Option<String>> {
        let Some(mut conn) = self.lock() else {
            return Vec::new();
        };
        let recorded: HashMap<String, Option<i64>> = {
            let Ok(mut stmt) = conn.prepare("SELECT graph_key, post_count FROM graph_state") else {
                return Vec::new();
            };
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default()
        };
        let mut disagree: Vec<Option<String>> = Vec::new();
        for (graph, n) in live {
            match recorded.get(&graph_key(graph.as_deref())) {
                Some(Some(recorded_n)) if *recorded_n == *n as i64 => {}
                Some(None) => {}
                // A graph the log never saw and the store does not hold
                // either (the default graph of a fresh store) is no chain.
                None if *n == 0 => {}
                _ => disagree.push(graph.clone()),
            }
        }
        // Graphs recorded with a count but no longer in the store.
        for (key, post) in &recorded {
            if post.is_some() && !live.iter().any(|(g, _)| graph_key(g.as_deref()) == *key) {
                disagree.push((!key.is_empty()).then(|| key.clone()));
            }
        }
        if disagree.is_empty() {
            return disagree;
        }
        let Ok(tx) = conn.transaction() else {
            return Vec::new();
        };
        let mut next: i64 = self.next_seq.load(Ordering::Relaxed);
        for graph in &disagree {
            let post = live
                .iter()
                .find(|(g, _)| g == graph)
                .map(|(_, n)| *n as i64);
            let _ = tx.execute(
                "INSERT INTO changes (epoch, txn, scope, graph_iri, origin, extent, post_count, state, created_at, seq) \
                 VALUES (?1, ?2, 'graph', ?3, 'reconcile', 'unknown', ?4, 'unknown', ?5, ?6)",
                params![self.epoch, uuid::Uuid::new_v4().to_string(), graph, post, now(), next],
            );
            let _ = tx.execute(
                "INSERT INTO graph_state (graph_key, last_seq, post_count) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(graph_key) DO UPDATE SET last_seq = excluded.last_seq, post_count = excluded.post_count",
                params![graph_key(graph.as_deref()), next, post],
            );
            next += 1;
        }
        let _ = tx.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'next_seq'",
            params![next.to_string()],
        );
        let _ = tx.commit();
        self.next_seq.store(next, Ordering::Relaxed);
        disagree
    }

    /// Mint a new epoch: the store was replaced wholesale (a quarantine and
    /// rebuild), so no chain from before it may be applied.
    pub fn renew_epoch(&mut self) {
        let epoch = uuid::Uuid::new_v4().to_string();
        if let Some(conn) = self.lock() {
            let _ = conn.execute(
                "UPDATE meta SET value = ?1 WHERE key = 'epoch'",
                params![epoch],
            );
        }
        self.epoch = epoch;
    }
}

// ─── Payloads ───────────────────────────────────────────────────────────────

fn encode_quads(quads: &[Quad]) -> Option<Vec<u8>> {
    if quads.is_empty() {
        return Some(Vec::new());
    }
    let mut buf = Vec::new();
    {
        let mut w = RdfSerializer::from_format(RdfFormat::NQuads).for_writer(&mut buf);
        for q in quads {
            if w.serialize_quad(q.as_ref()).is_err() {
                return None;
            }
        }
        if w.finish().is_err() {
            return None;
        }
    }
    if buf.len() > GZIP_ABOVE {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        if enc.write_all(&buf).is_err() {
            return Some(buf);
        }
        return enc.finish().ok().or(Some(buf));
    }
    Some(buf)
}

fn payload_text(blob: Option<&[u8]>) -> Option<String> {
    let blob = blob?;
    if blob.starts_with(&[0x1f, 0x8b]) {
        let mut out = String::new();
        flate2::read::GzDecoder::new(blob)
            .read_to_string(&mut out)
            .ok()?;
        Some(out)
    } else {
        String::from_utf8(blob.to_vec()).ok()
    }
}

/// The quads of a payload; an unreadable payload is empty.
pub fn decode_quads(blob: Option<&[u8]>) -> Vec<Quad> {
    match payload_text(blob) {
        Some(text) => parse_nquads(&text),
        None => Vec::new(),
    }
}

/// The quads of a row's N-Quads text (`added` / `removed`); `None` is empty.
pub fn decode_text(text: Option<&str>) -> Vec<Quad> {
    text.map(parse_nquads).unwrap_or_default()
}

/// Parse a row's N-Quads text back into quads.
pub fn parse_nquads(text: &str) -> Vec<Quad> {
    RdfParser::from_format(RdfFormat::NQuads)
        .for_reader(text.as_bytes())
        .flatten()
        .collect()
}

fn row_from_sql(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChangeRow> {
    let added: Option<Vec<u8>> = r.get(11)?;
    let removed: Option<Vec<u8>> = r.get(12)?;
    let extent: String = r.get(10)?;
    let state: String = r.get(17)?;
    Ok(ChangeRow {
        row: r.get(0)?,
        seq: r.get(1)?,
        epoch: r.get(2)?,
        txn: r.get(3)?,
        scope: r.get(4)?,
        graph_iri: r.get(5)?,
        origin: r.get(6)?,
        kind: r.get(7)?,
        commit_iri: r.get(8)?,
        actor_iri: r.get(9)?,
        extent: Extent::parse(&extent),
        added: payload_text(added.as_deref()),
        removed: payload_text(removed.as_deref()),
        added_n: r.get(13)?,
        removed_n: r.get(14)?,
        post_count: r.get(15)?,
        has_bnode: r.get::<_, i32>(16)? != 0,
        state: State::parse(&state),
        created_at: r.get(18)?,
    })
}

fn cursor_from_sql(r: &rusqlite::Row<'_>) -> rusqlite::Result<Cursor> {
    Ok(Cursor {
        name: r.get(0)?,
        seq: r.get(1)?,
        owner: r.get(2)?,
        expires_at: r.get(3)?,
        updated_at: r.get(4)?,
    })
}

fn cursors_of(conn: &Connection) -> Vec<Cursor> {
    let Ok(mut stmt) =
        conn.prepare("SELECT name, seq, owner, expires_at, updated_at FROM cursors ORDER BY name")
    else {
        return Vec::new();
    };
    stmt.query_map([], cursor_from_sql)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Net difference of two quad sets: `(in after but not before, in before but
/// not after)` — the before-image diff a non-ground update needs.
pub fn diff_quads(before: &[Quad], after: &[Quad]) -> (Vec<Quad>, Vec<Quad>) {
    let before_set: std::collections::HashSet<&Quad> = before.iter().collect();
    let after_set: std::collections::HashSet<&Quad> = after.iter().collect();
    let added = after
        .iter()
        .filter(|q| !before_set.contains(q))
        .cloned()
        .collect();
    let removed = before
        .iter()
        .filter(|q| !after_set.contains(q))
        .cloned()
        .collect();
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::model::{GraphName, NamedNode};

    fn q(n: u32, g: Option<&str>) -> Quad {
        Quad::new(
            NamedNode::new_unchecked(format!("http://ex/s{n}")),
            NamedNode::new_unchecked("http://ex/p"),
            NamedNode::new_unchecked(format!("http://ex/o{n}")),
            match g {
                Some(g) => GraphName::NamedNode(NamedNode::new_unchecked(g)),
                None => GraphName::DefaultGraph,
            },
        )
    }

    fn log() -> ChangeLog {
        ChangeLog::in_memory().unwrap()
    }

    #[test]
    fn a_write_is_one_row_per_graph_sequenced_in_commit_order() {
        let log = log();
        let targets = vec![
            Some("http://g/a".to_string()),
            Some("http://g/b".to_string()),
        ];
        let intent = log
            .begin("test", Some(&targets), &|_| Some(10))
            .expect("recorded");
        leave_write();
        let deltas = vec![
            GraphDelta::full(
                Some("http://g/a".into()),
                vec![q(1, Some("http://g/a"))],
                vec![],
            ),
            GraphDelta::full(
                Some("http://g/b".into()),
                vec![],
                vec![q(2, Some("http://g/b")), q(3, Some("http://g/b"))],
            ),
        ];
        log.commit_with::<()>(Some(intent), deltas, || Ok(()))
            .unwrap();
        let rows = log.rows_after(0, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].seq, Some(1));
        assert_eq!(rows[1].seq, Some(2));
        assert_eq!(rows[0].graph_iri.as_deref(), Some("http://g/a"));
        assert_eq!(rows[0].added_n, 1);
        assert_eq!(rows[0].post_count, Some(11));
        assert!(rows[0].added.as_deref().unwrap().contains("<http://ex/s1>"));
        assert_eq!(rows[1].removed_n, 2);
        assert_eq!(rows[1].post_count, Some(8));
        assert_eq!(rows[0].txn, rows[1].txn, "one write, one txn");
        assert_eq!(rows[0].state, State::Committed);
        assert_eq!(log.last_seq(), 2);
        assert_eq!(log.status().next_seq, 3);
    }

    #[test]
    fn a_failed_commit_aborts_its_rows_and_hands_out_no_sequence() {
        let log = log();
        let targets = vec![Some("http://g/a".to_string())];
        let intent = log.begin("test", Some(&targets), &|_| None).unwrap();
        leave_write();
        let r: Result<(), &str> = log.commit_with(Some(intent), vec![], || Err("boom"));
        assert_eq!(r, Err("boom"));
        assert!(log.rows_after(0, 10).is_empty());
        let s = log.status();
        assert_eq!(s.rows, 1);
        assert_eq!(s.pending, 0);
        assert_eq!(s.next_seq, 1);
    }

    #[test]
    fn only_the_first_primitive_of_a_write_records_it() {
        let log = log();
        enter_write();
        let outer = log.begin("outer", Some(&[None]), &|_| Some(0));
        assert!(outer.is_some());
        enter_write();
        let inner = log.begin("inner", Some(&[None]), &|_| Some(0));
        assert!(inner.is_none(), "the nested primitive is the same write");
        leave_write();
        leave_write();
        let again = log.begin("next", Some(&[None]), &|_| Some(0));
        assert!(again.is_some(), "the next write records again");
        leave_write();
        let _ = outer;
        let _ = again;
    }

    #[test]
    fn unknown_targets_are_one_store_scoped_unknown_row() {
        let log = log();
        let intent = log.begin("update", None, &|_| None).unwrap();
        leave_write();
        log.commit_unknown::<()>(Some(intent), None, || Ok(()))
            .unwrap();
        let rows = log.rows_after(0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scope, "store");
        assert_eq!(rows[0].extent, Extent::Unknown);
        assert_eq!(rows[0].state, State::Unknown);
        assert_eq!(rows[0].seq, Some(1));
    }

    #[test]
    fn a_payload_over_the_cap_keeps_exact_counts_and_drops_the_quads() {
        std::env::set_var("OTS_CHANGE_CAPTURE_MAX_PAYLOAD", "2");
        let log = log();
        std::env::remove_var("OTS_CHANGE_CAPTURE_MAX_PAYLOAD");
        let intent = log.begin("test", Some(&[None]), &|_| Some(0)).unwrap();
        leave_write();
        let big = (0..3).map(|i| q(i, None)).collect();
        log.commit_with::<()>(
            Some(intent),
            vec![GraphDelta::full(None, big, vec![])],
            || Ok(()),
        )
        .unwrap();
        let row = &log.rows_after(0, 1)[0];
        assert_eq!(row.extent, Extent::Counts);
        assert_eq!(row.added_n, 3);
        assert!(row.added.is_none());
        assert_eq!(row.post_count, Some(3));
    }

    #[test]
    fn large_payloads_round_trip_through_gzip() {
        let quads: Vec<Quad> = (0..5_000).map(|i| q(i, Some("http://g/z"))).collect();
        let blob = encode_quads(&quads).unwrap();
        assert!(
            blob.starts_with(&[0x1f, 0x8b]),
            "gzipped above the threshold"
        );
        let back = decode_quads(Some(&blob));
        assert_eq!(back.len(), 5_000);
        assert_eq!(back[4_999], quads[4_999]);
    }

    #[test]
    fn pending_rows_are_resolved_by_probing_the_store() {
        let log = log();
        let present = q(1, Some("http://g/a"));
        let absent = q(2, Some("http://g/a"));
        // Three crashed writes: one that landed (its added quad is present,
        // its removed one absent), one that did not, one half-visible.
        for (added, removed) in [
            (vec![present.clone()], vec![absent.clone()]),
            (vec![absent.clone()], vec![present.clone()]),
            (vec![present.clone(), absent.clone()], vec![]),
        ] {
            let intent = log
                .begin("test", Some(&[Some("http://g/a".to_string())]), &|_| None)
                .unwrap();
            leave_write();
            // Write the payload but leave the row pending, as a crash would.
            let conn = log.lock().unwrap();
            conn.execute(
                "UPDATE changes SET extent = 'full', added = ?2, removed = ?3 WHERE row = ?1",
                params![
                    intent.rows[0].1,
                    encode_quads(&added),
                    encode_quads(&removed)
                ],
            )
            .unwrap();
        }
        let (c, a, u) = log.resolve_pending(&|quad| *quad == present).unwrap();
        // The two rows that could be sequenced touch the same graph, so their
        // order is unrecoverable and both are unknown.
        assert_eq!((c, a, u), (0, 1, 2));
        let rows = log.rows_after(0, 10);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.state == State::Unknown));
        assert_eq!(log.status().pending, 0);
    }

    #[test]
    fn a_count_the_log_did_not_predict_becomes_an_unknown_row() {
        let log = log();
        let g = Some("http://g/a".to_string());
        let intent = log
            .begin("test", Some(std::slice::from_ref(&g)), &|_| Some(4))
            .unwrap();
        leave_write();
        log.commit_with::<()>(
            Some(intent),
            vec![GraphDelta::full(
                g.clone(),
                vec![q(1, Some("http://g/a"))],
                vec![],
            )],
            || Ok(()),
        )
        .unwrap();
        // Agrees: 4 + 1 = 5.
        assert!(log.reconcile_counts(&[(g.clone(), 5)]).is_empty());
        // Disagrees, and a graph the log never saw.
        let fixed = log.reconcile_counts(&[(g.clone(), 7), (Some("http://g/new".into()), 3)]);
        assert_eq!(fixed.len(), 2);
        let rows = log.rows_after(1, 10);
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|r| r.origin == "reconcile" && r.extent == Extent::Unknown));
        assert_eq!(rows[0].post_count, Some(7));
    }

    #[test]
    fn retention_deletes_only_below_the_lowest_live_cursor() {
        std::env::set_var("OTS_CHANGE_RETENTION_DAYS", "0");
        let log = log();
        std::env::remove_var("OTS_CHANGE_RETENTION_DAYS");
        for i in 0..5 {
            let intent = log.begin("test", Some(&[None]), &|_| Some(0)).unwrap();
            leave_write();
            log.commit_with::<()>(
                Some(intent),
                vec![GraphDelta::full(None, vec![q(i, None)], vec![])],
                || Ok(()),
            )
            .unwrap();
        }
        // Rows are "older than 0 days" only once the clock has moved on.
        std::thread::sleep(std::time::Duration::from_millis(20));
        log.set_cursor("replica", 3, Some("adm")).unwrap();
        assert_eq!(log.sweep().unwrap(), 2, "seq 1 and 2 are below the pin");
        assert_eq!(log.rows_after(0, 10).len(), 3);
        log.delete_cursor("replica");
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(log.sweep().unwrap(), 3);
        assert!(log.rows_after(0, 10).is_empty());
        // The sequence keeps counting from where it was.
        assert_eq!(log.status().next_seq, 6);
    }

    #[test]
    fn the_off_switch_records_nothing() {
        // On is the default; an explicit off is off.
        std::env::remove_var("OTS_CHANGE_CAPTURE");
        std::env::remove_var("OTS_REPLICATION_ROLE");
        assert!(ChangeLog::open(None).unwrap().enabled());
        std::env::set_var("OTS_CHANGE_CAPTURE", "off");
        let log = ChangeLog::open(None).unwrap();
        std::env::remove_var("OTS_CHANGE_CAPTURE");
        assert!(!log.enabled());
        assert!(log.begin("test", Some(&[None]), &|_| None).is_none());
        leave_write();
        assert!(log.rows_after(0, 10).is_empty());
        assert!(!log.status().enabled);
    }

    #[test]
    fn the_context_guard_stamps_actor_and_kind() {
        let log = log();
        {
            let _ctx = WriteContextGuard::set(WriteContext {
                actor_iri: Some("http://ex/users/alice".into()),
                commit_iri: Some("http://ex/commits/1".into()),
                kind: Some("Import".into()),
            });
            let intent = log.begin("test", Some(&[None]), &|_| Some(0)).unwrap();
            leave_write();
            log.commit_with::<()>(Some(intent), vec![], || Ok(()))
                .unwrap();
        }
        let row = &log.rows_after(0, 1)[0];
        assert_eq!(row.actor_iri.as_deref(), Some("http://ex/users/alice"));
        assert_eq!(row.kind.as_deref(), Some("Import"));
        assert_eq!(
            row.added_n, 0,
            "an intent row without a delta committed unchanged"
        );
        // Restored.
        let intent = log.begin("test", Some(&[None]), &|_| Some(0)).unwrap();
        leave_write();
        log.commit_with::<()>(Some(intent), vec![], || Ok(()))
            .unwrap();
        assert!(log.rows_after(1, 1)[0].actor_iri.is_none());
    }

    #[test]
    fn diff_is_the_net_change() {
        let before = vec![q(1, None), q(2, None)];
        let after = vec![q(2, None), q(3, None)];
        let (added, removed) = diff_quads(&before, &after);
        assert_eq!(added, vec![q(3, None)]);
        assert_eq!(removed, vec![q(1, None)]);
    }

    #[test]
    fn the_commit_trail_graph_is_never_recorded() {
        let log = log();
        let trail = vec![Some(crate::commit_log::COMMIT_GRAPH.to_string())];
        assert!(log.begin("update", Some(&trail), &|_| None).is_none());
        // Nor was the write claimed: a data primitive still records.
        let data = vec![Some("http://g/a".to_string())];
        assert!(log.begin("update", Some(&data), &|_| None).is_some());
        leave_write();
        assert_eq!(log.status().pending, 1);
    }
}
