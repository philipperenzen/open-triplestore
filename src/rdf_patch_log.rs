//! RDF Patch Logs (RDF Delta): one log per dataset, named by the dataset id
//! (<https://afs.github.io/rdf-delta/rdf-patch-logs.html>).
//!
//! * `GET  /api/datasets/:id/log` — the log: its version 0 and its entries.
//! * `POST /api/datasets/:id/log` — append a patch. It needs exactly one
//!   `H id` (an IRI the log does not hold yet) and at most one `H prev`,
//!   which must name the log's latest entry; no `H prev` means the log must be
//!   empty. A mismatch is a 409 and changes nothing. The patch is then applied
//!   to the dataset exactly as `POST /api/datasets/:id/patch` applies one
//!   (registered graphs, `?graph=` for triples, the SHACL write gates, `PA` /
//!   `PD` on the prefix table) and appended only when that succeeds.
//! * `GET  /api/datasets/:id/log/init` — version 0: the dataset the log
//!   starts from, as TriG (or N-Quads), with its prefix table.
//! * `GET  /api/datasets/:id/log/current` — the latest patch.
//! * `GET  /api/datasets/:id/log/patch/:ref` — a patch by log version (all
//!   digits) or by id (the full IRI, or the UUID of a `uuid:` / `urn:uuid:`
//!   id).
//!
//! What goes in: the patches appended here, and one entry per version cut
//! from the live graphs — the diff, as a patch, from the state the log's
//! previous version-cut entry reached (version 0 for the first) to the new
//! version, its prefix-table changes included. Other writes (SPARQL Update,
//! the Graph Store, imports, `POST …/patch`) are not journaled: replaying the
//! log redoes what went through it, and each version-cut entry brings the
//! graphs that version holds to the version's contents for every triple that
//! changed since the previous cut.
//!
//! A log starts with its first entry. Started by a version cut, version 0 is
//! the empty dataset. Started by an append, it is the dataset as it stood:
//! empty when its graphs hold nothing, else a draft version (`log-init-…`) cut
//! for the purpose. A version-cut entry is rendered from the immutable
//! snapshot graphs it compares; when a version it reads is deleted, its text
//! is written out first, so an entry never changes once appended.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use oxigraph::io::RdfFormat;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth::db::AuthDb;
use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::Dataset;
use crate::dataset_versions::models::{DatasetVersion, VersionStatus};
use crate::rdf_patch::{self, Patch, Refused};
use crate::server::AppState;
use crate::store::TripleStore;

/// Version 0 of a log: the dataset it starts from. `graphs` is `(live
/// graph, snapshot graph holding its state)`; `version` the dataset version
/// those snapshots belong to.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Init {
    pub version: Option<String>,
    pub graphs: Vec<(String, String)>,
    pub prefixes: Vec<(String, String)>,
}

/// The state the last version-cut entry reached (version 0 before the
/// first): the snapshot holding each live graph's state, and the prefix table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Checkpoint {
    graphs: BTreeMap<String, String>,
    prefixes: Vec<(String, String)>,
}

/// How a version-cut entry renders: [`rdf_patch::render`]'s inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VersionSpec {
    dataset_iri: String,
    to_version: String,
    /// `(live graph, snapshot it was, snapshot it is)`.
    mappings: Vec<(String, Option<String>, Option<String>)>,
    prefix_changes: Vec<(String, Option<String>)>,
}

/// One appended patch.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub version: i64,
    pub id: String,
    pub prev: Option<String>,
    /// `patch` (appended to the log) or `version` (a version cut).
    pub kind: String,
    pub author: Option<String>,
    /// The dataset version a `version` entry reached.
    pub dataset_version: Option<String>,
    /// The registered graph a `patch` entry's triples went to (`?graph=`).
    pub default_graph: Option<String>,
    /// The live graphs the entry changes.
    pub graphs: Vec<String>,
    pub created_at: String,
    #[serde(skip)]
    spec: Option<String>,
    #[serde(skip)]
    text: Option<String>,
}

// ── storage ─────────────────────────────────────────────────────────────────

const ENTRY_COLUMNS: &str = "version, patch_id, prev_id, kind, author, dataset_version, \
     default_graph, graphs, created_at, spec, text";

fn read_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let graphs: String = r.get(7)?;
    Ok(Entry {
        version: r.get(0)?,
        id: r.get(1)?,
        prev: r.get(2)?,
        kind: r.get(3)?,
        author: r.get(4)?,
        dataset_version: r.get(5)?,
        default_graph: r.get(6)?,
        graphs: serde_json::from_str(&graphs).unwrap_or_default(),
        created_at: r.get(8)?,
        spec: r.get(9)?,
        text: r.get(10)?,
    })
}

/// The log's version 0 and checkpoint; `None` before its first entry.
fn meta(db: &AuthDb, dataset_id: &str) -> anyhow::Result<Option<(Init, Checkpoint)>> {
    let conn = db.pool().get()?;
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT init, checkpoint FROM dataset_patch_logs WHERE dataset_id = ?1",
            [dataset_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(i, c)| Ok((serde_json::from_str(&i)?, serde_json::from_str(&c)?)))
        .transpose()
}

fn latest(db: &AuthDb, dataset_id: &str) -> anyhow::Result<Option<Entry>> {
    let conn = db.pool().get()?;
    Ok(conn
        .query_row(
            &format!(
                "SELECT {ENTRY_COLUMNS} FROM dataset_patch_log_entries
                 WHERE dataset_id = ?1 ORDER BY version DESC LIMIT 1"
            ),
            [dataset_id],
            read_entry,
        )
        .optional()?)
}

fn entry_by_version(db: &AuthDb, dataset_id: &str, version: i64) -> anyhow::Result<Option<Entry>> {
    let conn = db.pool().get()?;
    Ok(conn
        .query_row(
            &format!(
                "SELECT {ENTRY_COLUMNS} FROM dataset_patch_log_entries
                 WHERE dataset_id = ?1 AND version = ?2"
            ),
            params![dataset_id, version],
            read_entry,
        )
        .optional()?)
}

fn entry_by_id(db: &AuthDb, dataset_id: &str, id: &str) -> anyhow::Result<Option<Entry>> {
    let conn = db.pool().get()?;
    Ok(conn
        .query_row(
            &format!(
                "SELECT {ENTRY_COLUMNS} FROM dataset_patch_log_entries
                 WHERE dataset_id = ?1 AND patch_id = ?2"
            ),
            params![dataset_id, id],
            read_entry,
        )
        .optional()?)
}

fn entries(db: &AuthDb, dataset_id: &str, after: i64, limit: usize) -> anyhow::Result<Vec<Entry>> {
    let conn = db.pool().get()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {ENTRY_COLUMNS} FROM dataset_patch_log_entries
         WHERE dataset_id = ?1 AND version > ?2 ORDER BY version LIMIT ?3"
    ))?;
    let rows = stmt
        .query_map(params![dataset_id, after, limit as i64], read_entry)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// What [`append`] writes.
struct NewEntry {
    id: String,
    kind: &'static str,
    author: Option<String>,
    dataset_version: Option<String>,
    default_graph: Option<String>,
    graphs: Vec<String>,
    spec: Option<String>,
    text: Option<String>,
    /// The checkpoint a version-cut entry moves the log to.
    checkpoint: Option<Checkpoint>,
}

/// Append `e` after the log's latest entry, in one transaction: `prev` is
/// the latest entry's id, the log version the next number. `init` starts a
/// log that has no entries yet. Returns `(version, prev)`.
fn append(
    db: &AuthDb,
    dataset_id: &str,
    init: Option<&Init>,
    e: &NewEntry,
) -> anyhow::Result<(i64, Option<String>)> {
    let mut conn = db.pool().get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let now = chrono::Utc::now().to_rfc3339();
    let has_log = tx
        .query_row(
            "SELECT 1 FROM dataset_patch_logs WHERE dataset_id = ?1",
            [dataset_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !has_log {
        let init = init.cloned().unwrap_or_default();
        let checkpoint = Checkpoint {
            graphs: init.graphs.iter().cloned().collect(),
            prefixes: init.prefixes.clone(),
        };
        tx.execute(
            "INSERT INTO dataset_patch_logs (dataset_id, init, checkpoint, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                dataset_id,
                serde_json::to_string(&init)?,
                serde_json::to_string(&checkpoint)?,
                now
            ],
        )?;
    }
    let last: Option<(i64, String)> = tx
        .query_row(
            "SELECT version, patch_id FROM dataset_patch_log_entries
             WHERE dataset_id = ?1 ORDER BY version DESC LIMIT 1",
            [dataset_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (version, prev) = match last {
        Some((v, id)) => (v + 1, Some(id)),
        None => (1, None),
    };
    tx.execute(
        "INSERT INTO dataset_patch_log_entries
             (dataset_id, version, patch_id, prev_id, kind, author, dataset_version,
              default_graph, graphs, spec, text, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            dataset_id,
            version,
            e.id,
            prev,
            e.kind,
            e.author,
            e.dataset_version,
            e.default_graph,
            serde_json::to_string(&e.graphs)?,
            e.spec,
            e.text,
            now
        ],
    )?;
    if let Some(c) = &e.checkpoint {
        tx.execute(
            "UPDATE dataset_patch_logs SET checkpoint = ?2 WHERE dataset_id = ?1",
            params![dataset_id, serde_json::to_string(c)?],
        )?;
    }
    tx.commit()?;
    Ok((version, prev))
}

// ── appends in flight ───────────────────────────────────────────────────────
//
// An append checks `H prev`, applies the patch (which may take a while) and
// only then writes the entry. A second append in that window is refused
// (409: retry against the new latest entry). A version cut in that window
// waits its turn: its entry is computed and written right after the append's,
// so the chain stays linear without holding a lock across the apply.

/// A version cut waiting behind an append.
struct WaitingCut {
    base_url: String,
    record: DatasetVersion,
    author: Option<String>,
    prefixes: Vec<(String, String)>,
}

/// Logs with an append in flight, and the version cuts waiting behind it.
/// Keyed by [`log_key`]: one process may run several instances (the tests do).
static IN_FLIGHT: LazyLock<Mutex<HashMap<String, Vec<WaitingCut>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn in_flight() -> std::sync::MutexGuard<'static, HashMap<String, Vec<WaitingCut>>> {
    IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner())
}

/// A log's key in [`IN_FLIGHT`]: its instance (the identity database it
/// lives in) and its dataset.
fn log_key(db: &AuthDb, dataset_id: &str) -> String {
    format!("{:p}/{dataset_id}", db as *const AuthDb)
}

/// An append's claim on a log. Dropping it — the append done, refused or
/// unwound — releases the log and journals the cuts that waited.
struct Claim {
    state: AppState,
    dataset_id: String,
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut flight = in_flight();
        let key = log_key(&self.state.auth_db, &self.dataset_id);
        for cut in flight.remove(&key).unwrap_or_default() {
            journal_cut(&self.state.auth_db, &cut);
        }
    }
}

// ── version cuts ────────────────────────────────────────────────────────────

/// A version was cut from the dataset's live graphs: record its prefix
/// table, and append to the dataset's log the diff from the log's checkpoint
/// to it. Best-effort — the version stands either way; a failure is logged
/// and the next cut's entry covers the same changes.
pub fn on_version_cut(db: &AuthDb, base_url: &str, record: &DatasetVersion, author: Option<&str>) {
    let ds = &record.dataset_id;
    let prefixes = match db.snapshot_dataset_version_prefixes(ds, &record.version) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                "version {ds} {}: prefix table not recorded: {e}",
                record.version
            );
            return;
        }
    };
    let cut = WaitingCut {
        base_url: base_url.to_string(),
        record: record.clone(),
        author: author.map(str::to_string),
        prefixes,
    };
    let mut flight = in_flight();
    match flight.get_mut(&log_key(db, ds)) {
        Some(waiting) => waiting.push(cut),
        None => journal_cut(db, &cut),
    }
}

/// Append a version cut's entry: the diff from the log's checkpoint (read
/// now) to the version. The caller holds the in-flight lock.
fn journal_cut(db: &AuthDb, cut: &WaitingCut) {
    let record = &cut.record;
    let ds = &record.dataset_id;
    let checkpoint = match meta(db, ds) {
        Ok(m) => m.map(|(_, c)| c).unwrap_or_default(),
        Err(e) => {
            tracing::warn!(
                "patch log {ds}: not read, version {} not journaled: {e}",
                record.version
            );
            return;
        }
    };
    let mappings: Vec<(String, Option<String>, Option<String>)> = record
        .source_map
        .iter()
        .map(|m| {
            (
                m.source_graph.clone(),
                checkpoint.graphs.get(&m.source_graph).cloned(),
                Some(m.snapshot_graph.clone()),
            )
        })
        .collect();
    let mut next = checkpoint.clone();
    for m in &record.source_map {
        next.graphs
            .insert(m.source_graph.clone(), m.snapshot_graph.clone());
    }
    next.prefixes = cut.prefixes.clone();
    let spec = VersionSpec {
        dataset_iri: format!("{}/dataset/{ds}", cut.base_url.trim_end_matches('/')),
        to_version: record.version.clone(),
        prefix_changes: rdf_patch::prefix_changes(&checkpoint.prefixes, &cut.prefixes),
        mappings,
    };
    let entry = NewEntry {
        id: format!("urn:uuid:{}", uuid::Uuid::new_v4()),
        kind: "version",
        author: cut.author.clone(),
        dataset_version: Some(record.version.clone()),
        default_graph: None,
        graphs: record
            .source_map
            .iter()
            .map(|m| m.source_graph.clone())
            .collect(),
        spec: serde_json::to_string(&spec).ok(),
        text: None,
        checkpoint: Some(next),
    };
    if let Err(e) = append(db, ds, None, &entry) {
        tracing::warn!(
            "patch log {ds}: version {} not journaled: {e}",
            record.version
        );
    }
}

/// Before version `record`'s snapshot graphs are dropped: write out the text
/// of every version-cut entry that reads them, so the entry stays servable
/// and unchanged, and forget the version's prefix table.
pub fn before_version_purge(store: &TripleStore, db: &AuthDb, record: &DatasetVersion) {
    let ds = &record.dataset_id;
    let gone: HashSet<&str> = record.snapshot_graphs.iter().map(String::as_str).collect();
    let pending = unrendered_version_entries(db, ds);
    match pending {
        Ok(rows) => {
            for e in rows {
                let Some(spec) = e
                    .spec
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<VersionSpec>(s).ok())
                else {
                    continue;
                };
                let reads = spec.mappings.iter().any(|(_, from, to)| {
                    [from, to]
                        .into_iter()
                        .flatten()
                        .any(|g| gone.contains(g.as_str()))
                });
                if !reads {
                    continue;
                }
                let text = render_version_entry(store, &e, &spec);
                let written = db.pool().get().map_err(anyhow::Error::from).and_then(|c| {
                    c.execute(
                        "UPDATE dataset_patch_log_entries SET text = ?3
                         WHERE dataset_id = ?1 AND version = ?2",
                        params![ds, e.version, text],
                    )
                    .map_err(Into::into)
                });
                if let Err(err) = written {
                    tracing::warn!("patch log {ds}: entry {} not written out: {err}", e.version);
                }
            }
        }
        Err(err) => tracing::warn!("patch log {ds}: entries not read before a purge: {err}"),
    }
    if let Err(err) = db.delete_dataset_version_prefixes(ds, &record.version) {
        tracing::warn!(
            "version {ds} {}: prefix table not removed: {err}",
            record.version
        );
    }
}

/// The version-cut entries whose text is still rendered from snapshots.
fn unrendered_version_entries(db: &AuthDb, dataset_id: &str) -> anyhow::Result<Vec<Entry>> {
    let conn = db.pool().get()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {ENTRY_COLUMNS} FROM dataset_patch_log_entries
         WHERE dataset_id = ?1 AND kind = 'version' AND text IS NULL"
    ))?;
    let rows = stmt
        .query_map([dataset_id], read_entry)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn render_version_entry(store: &TripleStore, e: &Entry, spec: &VersionSpec) -> String {
    rdf_patch::render(
        store,
        &e.id,
        e.prev.as_deref(),
        &[
            ("dataset", spec.dataset_iri.as_str()),
            ("to", spec.to_version.as_str()),
        ],
        &spec.prefix_changes,
        &spec.mappings,
    )
}

/// An entry's patch text: as appended, or rendered from its snapshots.
fn entry_text(store: &TripleStore, e: &Entry) -> Option<String> {
    if let Some(t) = &e.text {
        return Some(t.clone());
    }
    let spec: VersionSpec = serde_json::from_str(e.spec.as_deref()?).ok()?;
    Some(render_version_entry(store, e, &spec))
}

// ── HTTP ────────────────────────────────────────────────────────────────────

fn refuse(status: StatusCode, message: impl Into<String>) -> Refused {
    (status, Json(serde_json::json!({ "error": message.into() })))
        .into_response()
        .into()
}

fn e500(e: impl std::fmt::Display) -> Refused {
    refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn conflict(message: String, latest: Option<&Entry>) -> Refused {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "error": message,
            "latest": latest.map(|e| serde_json::json!({ "version": e.version, "id": e.id })),
        })),
    )
        .into_response()
        .into()
}

/// The dataset, when `uid` may read it, and the live graphs hidden from
/// them (private graphs, for a caller who may not write the dataset).
fn readable(
    state: &AppState,
    dataset_id: &str,
    uid: Option<&str>,
) -> Result<(Dataset, HashSet<String>), Refused> {
    use crate::dataset_versions::handlers as vh;
    let ds = vh::load_dataset(state, dataset_id).map_err(Refused::from)?;
    vh::require_read(state, &ds, uid).map_err(Refused::from)?;
    let hidden = vh::private_source_filter(state, &ds, uid)
        .map_err(Refused::from)?
        .unwrap_or_default();
    Ok((ds, hidden))
}

fn withheld(e: &Entry, hidden: &HashSet<String>) -> bool {
    e.graphs.iter().any(|g| hidden.contains(g))
        || e.default_graph.as_ref().is_some_and(|g| hidden.contains(g))
}

#[derive(Debug, Default, Deserialize)]
pub struct ListParams {
    /// Entries after this log version.
    pub after: Option<i64>,
    pub limit: Option<usize>,
}

/// GET /api/datasets/:id/log — the log: version 0 and its entries (without
/// their text; fetch one at `…/log/patch/{version}`).
pub async fn describe_log(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Query(q): Query<ListParams>,
) -> Result<Response, Refused> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let (_, hidden) = readable(&state, &dataset_id, uid)?;
    let limit = q.limit.unwrap_or(1000).clamp(1, 10_000);
    let init = meta(&state.auth_db, &dataset_id)
        .map_err(e500)?
        .map(|(i, _)| i);
    let last = latest(&state.auth_db, &dataset_id).map_err(e500)?;
    let rows = entries(&state.auth_db, &dataset_id, q.after.unwrap_or(0), limit).map_err(e500)?;
    let rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|e| {
            if withheld(e, &hidden) {
                serde_json::json!({
                    "version": e.version, "id": e.id, "prev": e.prev, "kind": e.kind,
                    "withheld": true,
                })
            } else {
                serde_json::to_value(e).unwrap_or_default()
            }
        })
        .collect();
    Ok(Json(serde_json::json!({
        "name": dataset_id,
        "init": init.map(|i| serde_json::json!({
            "version": i.version,
            "graphs": i.graphs.iter().filter(|(g, _)| !hidden.contains(g)).map(|(g, _)| g).collect::<Vec<_>>(),
            "prefixes": i.prefixes.len(),
        })),
        "latest": last.map(|e| serde_json::json!({ "version": e.version, "id": e.id })),
        "entries": rows,
    }))
    .into_response())
}

/// GET /api/datasets/:id/log/init — version 0, the dataset the log starts
/// from: TriG by default (`?format=nquads` or `Accept: application/n-quads`
/// for N-Quads), each graph under its live name, the prefix table declared.
pub async fn get_init(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, Refused> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let (_, hidden) = readable(&state, &dataset_id, uid)?;
    let Some((init, _)) = meta(&state.auth_db, &dataset_id).map_err(e500)? else {
        return Err(refuse(
            StatusCode::NOT_FOUND,
            "the dataset's patch log is empty",
        ));
    };
    if let Some(v) = &init.version {
        let held = crate::dataset_versions::registry::version_exists(
            &state.store,
            &state.base_url,
            &dataset_id,
            v,
        );
        if !held {
            return Err(refuse(
                StatusCode::GONE,
                format!("version 0 was dataset version {v}, which has been deleted"),
            ));
        }
    }
    let nquads = q.get("format").is_some_and(|f| f == "nquads" || f == "nq")
        || headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("n-quads"));
    let (fmt, ct) = if nquads {
        (RdfFormat::NQuads, "application/n-quads")
    } else {
        (RdfFormat::TriG, "application/trig")
    };
    let graphs: Vec<(String, String)> = init
        .graphs
        .iter()
        .filter(|(live, _)| !hidden.contains(live))
        .map(|(live, snap)| (snap.clone(), live.clone()))
        .collect();
    let registry = state.prefix_registry.clone();
    let store = state.store.clone();
    let prefixes = init.prefixes.clone();
    let body = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        store
            .dump_graphs_with_prefixes_to_writer(&mut out, fmt, &graphs, &prefixes, |ns| {
                registry.declaration_for(ns)
            })
            .map(|_| out)
    })
    .await
    .map_err(e500)?
    .map_err(e500)?;
    Ok(([(header::CONTENT_TYPE, ct)], body).into_response())
}

fn patch_response(state: &AppState, e: &Entry) -> Result<Response, Refused> {
    let text = entry_text(&state.store, e)
        .ok_or_else(|| e500(format!("log entry {} has no text", e.version)))?;
    let mut resp = ([(header::CONTENT_TYPE, rdf_patch::MEDIA_TYPE)], text).into_response();
    if let Ok(v) = HeaderValue::from_str(&e.version.to_string()) {
        resp.headers_mut().insert("x-patch-log-version", v);
    }
    if let Some(g) = e
        .default_graph
        .as_deref()
        .and_then(|g| HeaderValue::from_str(g).ok())
    {
        resp.headers_mut().insert("x-patch-default-graph", g);
    }
    Ok(resp)
}

fn serve_entry(
    state: &AppState,
    e: Option<Entry>,
    hidden: &HashSet<String>,
    missing: &str,
) -> Result<Response, Refused> {
    let e = e.ok_or_else(|| refuse(StatusCode::NOT_FOUND, missing))?;
    if withheld(&e, hidden) {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "this patch changes graphs of the dataset you may not read",
        ));
    }
    patch_response(state, &e)
}

/// GET /api/datasets/:id/log/current — the latest patch.
pub async fn get_current(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<Response, Refused> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let (_, hidden) = readable(&state, &dataset_id, uid)?;
    let e = latest(&state.auth_db, &dataset_id).map_err(e500)?;
    serve_entry(&state, e, &hidden, "the dataset's patch log is empty")
}

/// GET /api/datasets/:id/log/patch/:ref — a patch by log version or id.
pub async fn get_patch(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, reference)): Path<(String, String)>,
) -> Result<Response, Refused> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let (_, hidden) = readable(&state, &dataset_id, uid)?;
    let db = &state.auth_db;
    let e = if !reference.is_empty() && reference.bytes().all(|b| b.is_ascii_digit()) {
        let v: i64 = reference
            .parse()
            .map_err(|_| refuse(StatusCode::NOT_FOUND, "no such log version"))?;
        entry_by_version(db, &dataset_id, v).map_err(e500)?
    } else {
        let mut found = None;
        for id in [
            reference.clone(),
            format!("urn:uuid:{reference}"),
            format!("uuid:{reference}"),
        ] {
            found = entry_by_id(db, &dataset_id, &id).map_err(e500)?;
            if found.is_some() {
                break;
            }
        }
        found
    };
    serve_entry(&state, e, &hidden, "no such patch in the dataset's log")
}

/// POST /api/datasets/:id/log — append a patch: check its `H id` / `H prev`
/// against the log, apply it to the dataset, append it.
pub async fn append_patch(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Query(params): Query<rdf_patch::PatchParams>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Refused> {
    let Some(Extension(user)) = user else {
        return Err(refuse(StatusCode::UNAUTHORIZED, "Authentication required"));
    };
    rdf_patch::writable_dataset(&state, &user, &dataset_id)?;
    let text = rdf_patch::patch_text(&headers, &body)?;
    let patch = rdf_patch::parse(&text)
        .map_err(|e| refuse(StatusCode::BAD_REQUEST, format!("invalid RDF Patch: {e}")))?;
    let id = log_id(&patch).map_err(|m| refuse(StatusCode::BAD_REQUEST, m))?;
    let prev = patch.header_values("prev").next().map(str::to_string);

    // Claim the log: one append at a time, checked against its latest entry.
    {
        let key = log_key(&state.auth_db, &dataset_id);
        let mut flight = in_flight();
        if flight.contains_key(&key) {
            return Err(conflict(
                "another patch is being appended to this log; retry against its new latest entry"
                    .into(),
                None,
            ));
        }
        let db = &state.auth_db;
        let last = latest(db, &dataset_id).map_err(e500)?;
        if entry_by_id(db, &dataset_id, &id).map_err(e500)?.is_some() {
            return Err(conflict(
                format!("the log already holds a patch with id <{id}>"),
                last.as_ref(),
            ));
        }
        match (&prev, &last) {
            (None, Some(l)) => {
                return Err(conflict(
                    format!(
                        "the patch has no H prev, so it must be the log's first, but the log's latest is <{}>",
                        l.id
                    ),
                    Some(l),
                ))
            }
            (Some(p), None) => {
                return Err(conflict(
                    format!("H prev <{p}> does not match: the log is empty"),
                    None,
                ))
            }
            (Some(p), Some(l)) if *p != l.id => {
                return Err(conflict(
                    format!("H prev <{p}> does not match the log's latest patch <{}>", l.id),
                    Some(l),
                ))
            }
            _ => {}
        }
        flight.insert(key, Vec::new());
    }
    let _claim = Claim {
        state: state.clone(),
        dataset_id: dataset_id.clone(),
    };
    append_claimed(&state, &user, &dataset_id, &patch, &text, &id, &params).await
}

/// The patch's one `H id`, which must be an IRI.
fn log_id(patch: &Patch) -> Result<String, String> {
    let ids: Vec<&str> = patch.header_values("id").collect();
    let [id] = ids.as_slice() else {
        return Err(format!(
            "a patch appended to a log needs exactly one H id header, found {}",
            ids.len()
        ));
    };
    oxigraph::model::NamedNode::new(*id).map_err(|e| format!("H id must be an IRI: {id}: {e}"))?;
    let prevs: Vec<&str> = patch.header_values("prev").collect();
    if prevs.len() > 1 {
        return Err(format!(
            "a patch has at most one H prev header, found {}",
            prevs.len()
        ));
    }
    Ok(id.to_string())
}

/// Append once the log is claimed: capture version 0 for a new log, apply,
/// write the entry.
async fn append_claimed(
    state: &AppState,
    user: &AuthenticatedUser,
    dataset_id: &str,
    patch: &Patch,
    text: &str,
    id: &str,
    params: &rdf_patch::PatchParams,
) -> Result<Response, Refused> {
    let db = &state.auth_db;
    let init = if meta(db, dataset_id).map_err(e500)?.is_none() {
        Some(capture_init(state, user, dataset_id).await?)
    } else {
        None
    };
    let applied =
        rdf_patch::apply_to_dataset(state, user, dataset_id, patch, params.graph.as_deref()).await;
    let applied = match applied {
        Ok(a) => a,
        Err(r) => {
            // Version 0 is captured with the first entry; a refused first
            // patch leaves no log, so drop the version cut for it.
            if let Some(Init {
                version: Some(v), ..
            }) = &init
            {
                drop_init_version(state, dataset_id, v);
            }
            return Err(r);
        }
    };
    let entry = NewEntry {
        id: id.to_string(),
        kind: "patch",
        author: Some(user.user_id.clone()),
        dataset_version: None,
        default_graph: params.graph.clone(),
        graphs: applied.graphs.clone(),
        spec: None,
        text: Some(text.to_string()),
        checkpoint: None,
    };
    let (version, prev) = append(db, dataset_id, init.as_ref(), &entry).map_err(|e| {
        e500(format!(
            "the patch was applied but could not be appended to the log: {e}"
        ))
    })?;
    let mut out = rdf_patch::applied_json(patch, id, &applied);
    out["version"] = serde_json::json!(version);
    out["prev"] = serde_json::json!(prev);
    Ok(Json(out).into_response())
}

/// Version 0 of a log an append starts: the empty dataset when its graphs
/// hold nothing, else a draft version cut now.
async fn capture_init(
    state: &AppState,
    user: &AuthenticatedUser,
    dataset_id: &str,
) -> Result<Init, Refused> {
    let db = &state.auth_db;
    let prefixes = db.dataset_prefix_pairs(dataset_id).map_err(e500)?;
    let graphs = db.list_dataset_graphs(dataset_id).map_err(e500)?;
    let held: Vec<String> = graphs
        .into_iter()
        .filter(|g| state.store.count_graph(Some(g.as_str())).unwrap_or(0) > 0)
        .collect();
    if held.is_empty() {
        return Ok(Init {
            version: None,
            graphs: Vec::new(),
            prefixes,
        });
    }
    let existing: BTreeSet<String> =
        crate::dataset_versions::registry::list_versions(&state.store, &state.base_url, dataset_id)
            .into_iter()
            .map(|v| v.version)
            .collect();
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    let mut name = format!("log-init-{stamp}");
    let mut n = 2;
    while existing.contains(&name) {
        name = format!("log-init-{stamp}-{n}");
        n += 1;
    }
    let st = state.clone();
    let ds = dataset_id.to_string();
    let ver = name.clone();
    let by = format!("{}/users/{}", state.base_url, user.user_id);
    let record = tokio::task::spawn_blocking(move || {
        crate::dataset_versions::snapshot_as_version(
            &st.store,
            &st.base_url,
            &ds,
            &ver,
            &held,
            VersionStatus::Draft,
            Some(&by),
            Some("Version 0 of the dataset's RDF Patch log: the data as it stood before the log's first patch"),
        )
    })
    .await
    .map_err(e500)?
    .map_err(e500)?;
    db.snapshot_dataset_version_prefixes(dataset_id, &name)
        .map_err(e500)?;
    Ok(Init {
        version: Some(name),
        graphs: record
            .source_map
            .iter()
            .map(|m| (m.source_graph.clone(), m.snapshot_graph.clone()))
            .collect(),
        prefixes,
    })
}

fn drop_init_version(state: &AppState, dataset_id: &str, version: &str) {
    let Some(record) = crate::dataset_versions::registry::get_version(
        &state.store,
        &state.base_url,
        dataset_id,
        version,
    ) else {
        return;
    };
    let refs: Vec<&str> = record.snapshot_graphs.iter().map(String::as_str).collect();
    if let Err(e) = state.store.bulk_delete_graphs(&refs) {
        tracing::warn!("patch log {dataset_id}: unused version 0 {version} not dropped: {e}");
        return;
    }
    let _ = crate::dataset_versions::registry::delete_version(
        &state.store,
        &state.base_url,
        dataset_id,
        version,
    );
    let _ = state
        .auth_db
        .delete_dataset_version_prefixes(dataset_id, version);
}

/// The id a version diff gets, so the diffs between consecutive versions
/// chain through `H prev`: a name-based UUID of the dataset, both sides and
/// the graphs the caller sees.
pub fn diff_id(dataset_iri: &str, from: &str, to: &str, graphs: &BTreeSet<String>) -> String {
    let name = format!(
        "{dataset_iri}\n{from}\n{to}\n{}",
        graphs.iter().cloned().collect::<Vec<_>>().join("\n")
    );
    format!(
        "urn:uuid:{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, name.as_bytes())
    )
}
