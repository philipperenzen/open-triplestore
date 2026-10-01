//! A dataset's change feed: the change log's rows for the dataset's graphs
//! (delta note §7).
//!
//! * `GET /api/datasets/:id/changes?since=&limit=&wait_ms=` — rows above
//!   `since`, in commit order, without their quads, plus `next` to resume
//!   from. Only rows of graphs the caller may read are listed (dataset access
//!   does not imply access to its private graphs); an unreadable row is
//!   omitted, never counted. Store-scoped rows — writes whose graphs were not
//!   known — are listed for every dataset: they bound every chain.
//! * `GET /api/datasets/:id/changes/:seq` and `…/:seq/inverse` — one change
//!   as an RDF Patch, or the patch that undoes it. 410 when retention has
//!   removed it, 409 when the row carries no quads (a change over the payload
//!   cap, or one the log could not bound).
//! * `PUT` / `DELETE /api/datasets/:id/changes/cursors/:name` — a named
//!   consumer position. It pins retention until it expires
//!   (`OTS_CURSOR_TTL_DAYS`, refreshed by every PUT) or its owner or an admin
//!   deletes it.
//! * `GET /api/admin/changes/stats` — size accounting: rows and payload bytes
//!   per dataset, pins and cursors with their owners, live materialisations
//!   and versions per kind.
//!
//! Reads are anonymous-capable like every dataset read (404 hides a dataset
//! the caller cannot see); a cursor needs an account. Everything answers from
//! an empty log when change capture is off.

use std::collections::{HashMap, HashSet};

use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::Dataset;
use crate::server::error::AppError;
use crate::server::AppState;
use crate::store::changes::{ChangeRow, Extent};

fn dataset_iri(state: &AppState, id: &str) -> String {
    format!("{}/dataset/{id}", state.base_url.trim_end_matches('/'))
}

fn load_readable(state: &AppState, id: &str, uid: Option<&str>) -> Result<Dataset, AppError> {
    let ds = state
        .auth_db
        .get_dataset(id)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Dataset '{id}' not found")))?;
    if !state
        .auth_db
        .can_access_dataset(uid, &ds)
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Dataset '{id}' not found")));
    }
    Ok(ds)
}

/// The dataset's graphs the caller may read.
fn readable_graphs(
    state: &AppState,
    dataset_id: &str,
    user: Option<&AuthenticatedUser>,
) -> Result<Vec<String>, AppError> {
    let graphs = state
        .auth_db
        .list_dataset_graphs(dataset_id)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if user.is_some_and(|u| u.is_admin()) {
        return Ok(graphs);
    }
    let readable = crate::server::routes::accessible_read_graphs(state, user)?;
    Ok(graphs
        .into_iter()
        .filter(|g| readable.contains(g))
        .collect())
}

#[derive(Debug, Deserialize)]
pub struct FeedParams {
    pub since: Option<i64>,
    pub limit: Option<usize>,
    /// Long-poll: when nothing is above `since`, hold the request up to
    /// this long (at most 30 s) for a row to land.
    pub wait_ms: Option<u64>,
}

/// GET /api/datasets/:dataset_id/changes
pub async fn feed(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Query(p): Query<FeedParams>,
) -> Result<impl IntoResponse, AppError> {
    let user = user.as_deref();
    load_readable(&state, &dataset_id, user.map(|u| u.user_id.as_str()))?;
    let graphs = readable_graphs(&state, &dataset_id, user)?;
    let since = p.since.unwrap_or(0).max(0);
    let limit = p.limit.unwrap_or(500).clamp(1, 5000);
    let log = state.store.changes();
    let mut notified = Box::pin(log.rows_notified());
    notified.as_mut().enable();
    let mut rows = log.rows_after_in_graphs(since, limit, &graphs, false);
    let wait = p.wait_ms.unwrap_or(0).min(30_000);
    if rows.is_empty() && wait > 0 && log.enabled() {
        let _ = tokio::time::timeout(std::time::Duration::from_millis(wait), notified).await;
        rows = log.rows_after_in_graphs(since, limit, &graphs, false);
    }
    let next = rows.last().and_then(|r| r.seq).unwrap_or(since);
    Ok(Json(serde_json::json!({
        "dataset_id": dataset_id,
        "enabled": log.enabled(),
        "epoch": log.epoch(),
        "rows": rows,
        "next": next,
    })))
}

/// The row `seq` of `dataset_id`, if the caller may read it.
fn readable_row(
    state: &AppState,
    dataset_id: &str,
    seq: i64,
    user: Option<&AuthenticatedUser>,
) -> Result<ChangeRow, AppError> {
    load_readable(state, dataset_id, user.map(|u| u.user_id.as_str()))?;
    let log = state.store.changes();
    let Some(row) = log.row(seq) else {
        if seq >= 1 && seq <= log.last_seq() {
            return Err(AppError::Gone(format!(
                "change {seq} is no longer held: retention removed it"
            )));
        }
        return Err(AppError::NotFound(format!("change {seq} not found")));
    };
    let graphs: HashSet<String> = readable_graphs(state, dataset_id, user)?
        .into_iter()
        .collect();
    let mine = match &row.graph_iri {
        Some(g) => graphs.contains(g),
        // A store-scoped row bears on every dataset, but carries no quads.
        None => row.scope == "store",
    };
    if !mine {
        return Err(AppError::NotFound(format!("change {seq} not found")));
    }
    if row.extent != Extent::Full {
        return Err(AppError::Conflict(serde_json::json!({
            "error": "this change carries no quads: it was larger than a row holds, or the log could not bound it",
            "seq": seq,
            "extent": row.extent,
            "added_n": row.added_n,
            "removed_n": row.removed_n,
        })));
    }
    Ok(row)
}

fn patch_response(text: String) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, crate::rdf_patch::MEDIA_TYPE)],
        text,
    )
        .into_response()
}

/// GET /api/datasets/:dataset_id/changes/:seq
pub async fn change(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, seq)): Path<(String, i64)>,
) -> Result<Response, AppError> {
    let row = readable_row(&state, &dataset_id, seq, user.as_deref())?;
    let iri = dataset_iri(&state, &dataset_id);
    let patch = crate::rdf_patch::change_patch(&row, Some(&iri))
        .ok_or_else(|| AppError::Internal("a full row without a patch".into()))?;
    Ok(patch_response(patch.render()))
}

/// GET /api/datasets/:dataset_id/changes/:seq/inverse
pub async fn change_inverse(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, seq)): Path<(String, i64)>,
) -> Result<Response, AppError> {
    let row = readable_row(&state, &dataset_id, seq, user.as_deref())?;
    let iri = dataset_iri(&state, &dataset_id);
    let patch = crate::rdf_patch::change_inverse_patch(&row, Some(&iri))
        .ok_or_else(|| AppError::Internal("a full row without a patch".into()))?;
    Ok(patch_response(patch.render()))
}

/// The cursor table is shared by every consumer; a dataset's cursors are
/// named under it so two datasets' `replica` cursors never collide.
fn cursor_key(dataset_id: &str, name: &str) -> String {
    format!("dataset:{dataset_id}:{name}")
}

fn valid_cursor_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[derive(Debug, Deserialize)]
pub struct CursorBody {
    pub seq: i64,
}

/// PUT /api/datasets/:dataset_id/changes/cursors/:name
pub async fn put_cursor(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((dataset_id, name)): Path<(String, String)>,
    Json(body): Json<CursorBody>,
) -> Result<impl IntoResponse, AppError> {
    load_readable(&state, &dataset_id, Some(&user.user_id))?;
    if !valid_cursor_name(&name) {
        return Err(AppError::BadRequest(
            "cursor names are 1-64 characters of [A-Za-z0-9._-]".into(),
        ));
    }
    let log = state.store.changes();
    if !log.readable() {
        return Err(AppError::Conflict(serde_json::json!({
            "error": "change capture is off: there is no feed to hold a position in",
        })));
    }
    let last = log.last_seq();
    if body.seq < 0 || body.seq > last {
        return Err(AppError::BadRequest(format!(
            "seq must be between 0 and {last}"
        )));
    }
    let key = cursor_key(&dataset_id, &name);
    if let Some(existing) = log.cursor(&key) {
        if existing.owner.as_deref().is_some_and(|o| o != user.user_id) && !user.is_admin() {
            return Err(AppError::Forbidden(
                "this cursor belongs to someone else".into(),
            ));
        }
    }
    log.set_cursor(&key, body.seq, Some(&user.user_id))
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let c = log
        .cursor(&key)
        .ok_or_else(|| AppError::Internal("cursor not stored".into()))?;
    Ok(Json(serde_json::json!({
        "name": name,
        "seq": c.seq,
        "owner": c.owner,
        "expires_at": c.expires_at,
        "updated_at": c.updated_at,
    })))
}

/// DELETE /api/datasets/:dataset_id/changes/cursors/:name — its owner or an
/// admin.
pub async fn delete_cursor(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((dataset_id, name)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    load_readable(&state, &dataset_id, Some(&user.user_id))?;
    let log = state.store.changes();
    let key = cursor_key(&dataset_id, &name);
    let Some(existing) = log.cursor(&key) else {
        return Err(AppError::NotFound("no such cursor".into()));
    };
    if existing.owner.as_deref() != Some(user.user_id.as_str()) && !user.is_admin() {
        return Err(AppError::Forbidden(
            "only the cursor's owner or an admin can delete it".into(),
        ));
    }
    log.delete_cursor(&key);
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/admin/changes/stats — what the change log and the versions
/// built on it hold, per dataset (closing the readiness audit's "no size
/// accounting").
pub async fn admin_stats(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
) -> Result<impl IntoResponse, AppError> {
    match user.as_deref() {
        None => return Err(AppError::Unauthorized("Authentication required".into())),
        Some(u) if !u.is_admin() => return Err(AppError::Forbidden("Admin role required".into())),
        Some(_) => {}
    }
    let st = state.clone();
    let stats = tokio::task::spawn_blocking(move || stats(&st))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))??;
    Ok(Json(stats))
}

fn stats(state: &AppState) -> Result<serde_json::Value, AppError> {
    let log = state.store.changes();
    let status = log.status();
    let owner_of: HashMap<String, String> = state
        .auth_db
        .list_all_dataset_graph_rows()
        .map_err(|e| AppError::Internal(e.to_string()))?
        .into_iter()
        .map(|(ds, g, _)| (g, ds))
        .collect();
    #[derive(Default, serde::Serialize)]
    struct PerDataset {
        rows: i64,
        payload_bytes: i64,
        materialised_quads: i64,
        materialisations: i64,
        checkpoints: i64,
        deltas: i64,
    }
    let mut per: std::collections::BTreeMap<String, PerDataset> = Default::default();
    let (mut unowned_rows, mut unowned_bytes, mut store_scoped) = (0i64, 0i64, 0i64);
    for (graph, rows, bytes) in log.usage_by_graph() {
        match graph.as_deref().and_then(|g| owner_of.get(g)) {
            Some(ds) => {
                let e = per.entry(ds.clone()).or_default();
                e.rows += rows;
                e.payload_bytes += bytes;
            }
            None if graph.is_none() => store_scoped += rows,
            None => {
                unowned_rows += rows;
                unowned_bytes += bytes;
            }
        }
    }
    for m in crate::dataset_versions::materialise::list(&state.store) {
        let e = per.entry(m.dataset_id.clone()).or_default();
        e.materialised_quads += m.quads;
        e.materialisations += 1;
    }
    for ds in per.keys().cloned().collect::<Vec<_>>() {
        let versions =
            crate::dataset_versions::registry::list_versions(&state.store, &state.base_url, &ds);
        let e = per.entry(ds).or_default();
        for v in versions {
            if v.is_delta() {
                e.deltas += 1;
            } else {
                e.checkpoints += 1;
            }
        }
    }
    Ok(serde_json::json!({
        "enabled": status.enabled,
        "readable": status.readable,
        "epoch": status.epoch,
        "size_bytes": status.size_bytes,
        "rows": status.rows,
        "max_deleted_seq": status.max_deleted_seq,
        "first_seq_commit": log.first_seq_commit(),
        "datasets": per,
        "unregistered_graphs": { "rows": unowned_rows, "payload_bytes": unowned_bytes },
        "store_scoped_rows": store_scoped,
        "pins": status.pins,
        "cursors": status.cursors,
    }))
}
