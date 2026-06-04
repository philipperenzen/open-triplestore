//! Dataset versioning REST API.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use oxigraph::io::RdfFormat;
use serde_json::json;

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::Dataset;
use crate::data_models::diff::triple_delta;
use crate::server::error::AppError;
use crate::server::AppState;

use super::models::{
    CreateDatasetBranchRequest, CreateDatasetVersionRequest, DatasetBranchInfo, DatasetVersion,
    UpdateDatasetVersionRequest, VersionDataParams, VersionStatus,
};
use super::{registry, snapshot};

// ─── helpers ──────────────────────────────────────────────────────────────

fn load_dataset(state: &AppState, id: &str) -> Result<Dataset, AppError> {
    state
        .auth_db
        .get_dataset(id)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Dataset '{id}' not found")))
}

fn require_read(state: &AppState, ds: &Dataset, uid: Option<&str>) -> Result<(), AppError> {
    if state.auth_db.can_access_dataset(uid, ds).map_err(|e| AppError::Internal(e.to_string()))? {
        Ok(())
    } else {
        Err(AppError::NotFound(format!("Dataset '{}' not found", ds.id)))
    }
}

fn require_write(state: &AppState, ds: &Dataset, uid: &str) -> Result<(), AppError> {
    if state.auth_db.can_write_dataset(uid, ds).map_err(|e| AppError::Internal(e.to_string()))? {
        Ok(())
    } else {
        Err(AppError::Unauthorized("Write access to this dataset required".to_string()))
    }
}

fn validate_version(v: &str) -> Result<(), AppError> {
    if v.trim().is_empty() || v.contains('/') || v.contains(' ') || v.contains('#') {
        return Err(AppError::BadRequest(format!("Invalid version string: '{v}'")));
    }
    Ok(())
}

// ─── list / get ─────────────────────────────────────────────────────────────

/// GET /api/datasets/:id/versions
pub async fn list_versions(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    require_read(&state, &ds, uid)?;
    let versions = registry::list_versions(&state.store, &state.base_url, &id);
    Ok(Json(versions))
}

/// GET /api/datasets/:id/versions/:ver
pub async fn get_version(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    require_read(&state, &ds, uid)?;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    Ok(Json(record))
}

/// GET /api/datasets/:id/versions/:ver/data
pub async fn get_version_data(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((id, ver)): Path<(String, String)>,
    Query(params): Query<VersionDataParams>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    require_read(&state, &ds, uid)?;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;

    let graphs: Vec<String> = match params.graph.as_deref() {
        Some("all") | None => record.snapshot_graphs.clone(),
        Some(suffix) => record.snapshot_graphs.iter().filter(|g| g.ends_with(suffix)).cloned().collect(),
    };

    let single = matches!(params.format.as_deref(), Some("turtle") | Some("ttl") | Some("ntriples") | Some("nt"))
        && graphs.len() == 1;
    let fmt = if single {
        match params.format.as_deref() {
            Some("ntriples") | Some("nt") => RdfFormat::NTriples,
            _ => RdfFormat::Turtle,
        }
    } else {
        RdfFormat::TriG
    };
    let content_type = match fmt {
        RdfFormat::Turtle => "text/turtle",
        RdfFormat::NTriples => "application/n-triples",
        _ => "application/trig",
    };

    let mut out = Vec::new();
    for g in &graphs {
        let data = state.store.graph_store_get(Some(g), fmt).map_err(AppError::from)?;
        out.extend_from_slice(&data);
    }

    use axum::http::HeaderValue;
    use axum::response::Response;
    let mut resp = Response::new(axum::body::Body::from(out));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    Ok(resp)
}

// ─── create draft snapshot ──────────────────────────────────────────────────

/// POST /api/datasets/:id/versions
pub async fn create_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Json(body): Json<CreateDatasetVersionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    validate_version(&body.version)?;
    if registry::version_exists(&state.store, &state.base_url, &id, &body.version) {
        return Err(AppError::BadRequest(format!("Version '{}' already exists", body.version)));
    }

    // Determine source graphs: requested subset, else all registered graphs.
    let all_graphs: Vec<String> = state
        .auth_db
        .list_dataset_graphs(&id)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let source_graphs: Vec<String> = if body.graphs.is_empty() {
        all_graphs
    } else {
        body.graphs.iter().filter(|g| all_graphs.contains(g)).cloned().collect()
    };
    if source_graphs.is_empty() {
        return Err(AppError::BadRequest("Dataset has no graphs to snapshot".to_string()));
    }

    let source_map = snapshot::snapshot_graphs(&state.store, &state.base_url, &id, &body.version, &source_graphs)
        .map_err(AppError::from)?;
    let snapshot_graphs: Vec<String> = source_map.iter().map(|m| m.snapshot_graph.clone()).collect();

    let record = DatasetVersion {
        dataset_id: id.clone(),
        version: body.version.clone(),
        status: VersionStatus::Draft,
        graph_iri: format!("{}/dataset/{}/version/{}", state.base_url, id, body.version),
        snapshot_graphs,
        source_map,
        created_at: Utc::now().to_rfc3339(),
        created_by: Some(format!("{}/users/{}", state.base_url, user.user_id)),
        derived_from: None,
        notes: body.notes.clone(),
        branch: body.branch.clone(),
    };
    registry::insert_version(&state.store, &state.base_url, &record).map_err(AppError::from)?;
    if record.branch.is_none() {
        registry::update_latest_draft(&state.store, &state.base_url, &id, &body.version).map_err(AppError::from)?;
    }
    // Snapshot the validation layer alongside the data (best-effort: the version
    // already persisted, so a binding-snapshot hiccup must not fail the request).
    if let Err(e) = crate::shacl_studio::bindings::snapshot_dataset_bindings(
        &state.store, &state.base_url, &id, &body.version, &source_graphs,
    ) {
        tracing::warn!("failed to snapshot validation bindings for {id} v{}: {e}", body.version);
    }
    // Re-test this dataset's saved queries against the new version (background).
    crate::saved_queries::testing::spawn_version_tests(&state, &id, &body.version);
    Ok((StatusCode::CREATED, Json(record)))
}

/// PATCH /api/datasets/:id/versions/:ver — update notes.
pub async fn update_version_notes(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
    Json(body): Json<UpdateDatasetVersionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    registry::update_version_notes(&state.store, &state.base_url, &id, &ver, body.notes.as_deref())
        .map_err(AppError::from)?;
    Ok(Json(json!({ "ok": true })))
}

// ─── lifecycle ────────────────────────────────────────────────────────────

/// POST /api/datasets/:id/versions/:ver/stage
pub async fn stage_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    if record.status != VersionStatus::Draft {
        return Err(AppError::BadRequest("Only Draft versions can be staged".to_string()));
    }
    registry::update_version_status(&state.store, &state.base_url, &id, &ver, VersionStatus::Staged)
        .map_err(AppError::from)?;
    if record.branch.is_none() {
        registry::clear_latest_draft(&state.store, &state.base_url, &id).map_err(AppError::from)?;
    }
    Ok(Json(json!({ "status": "staged", "version": ver })))
}

/// POST /api/datasets/:id/versions/:ver/publish
pub async fn publish_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    if !matches!(record.status, VersionStatus::Staged | VersionStatus::Draft) {
        return Err(AppError::BadRequest("Only Staged or Draft versions can be published".to_string()));
    }
    // Deprecate the prior latest-published main version.
    let (latest_pub, _) = registry::get_pointers(&state.store, &state.base_url, &id);
    if let Some(old) = latest_pub {
        if old != ver {
            registry::update_version_status(&state.store, &state.base_url, &id, &old, VersionStatus::Deprecated)
                .map_err(AppError::from)?;
        }
    }
    registry::update_version_status(&state.store, &state.base_url, &id, &ver, VersionStatus::Published)
        .map_err(AppError::from)?;
    registry::update_latest_published(&state.store, &state.base_url, &id, &ver).map_err(AppError::from)?;
    Ok(Json(json!({ "status": "published", "version": ver })))
}

/// POST /api/datasets/:id/versions/:ver/deprecate
pub async fn deprecate_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    if record.status == VersionStatus::Deprecated {
        return Err(AppError::BadRequest("Version is already deprecated".to_string()));
    }
    registry::update_version_status(&state.store, &state.base_url, &id, &ver, VersionStatus::Deprecated)
        .map_err(AppError::from)?;
    Ok(Json(json!({ "status": "deprecated", "version": ver })))
}

/// POST /api/datasets/:id/versions/:ver/restore — copy the snapshot back onto
/// the dataset's live graphs.
pub async fn restore_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    if record.source_map.is_empty() {
        return Err(AppError::BadRequest("Version has no graph mapping to restore".to_string()));
    }
    let restored = snapshot::restore(&state.store, &record.source_map).map_err(AppError::from)?;
    // Ensure restored graphs are registered to the dataset.
    for g in &restored {
        let _ = state.auth_db.add_dataset_graph(&id, g);
    }
    // Re-apply the version's validation-layer bindings (best-effort, tolerant of
    // shape graphs deleted since the snapshot).
    let studio = crate::shacl_studio::store::ShaclStudioStore::new(state.auth_db.pool());
    match crate::shacl_studio::bindings::restore_dataset_bindings(&state.store, &studio, &state.base_url, &id, &ver) {
        Ok(n) if n > 0 => tracing::info!("restored {n} validation binding(s) for {id} v{ver}"),
        Ok(_) => {}
        Err(e) => tracing::warn!("failed to restore validation bindings for {id} v{ver}: {e}"),
    }
    Ok(Json(json!({ "restored": restored, "version": ver })))
}

// ─── branches ─────────────────────────────────────────────────────────────

/// GET /api/datasets/:id/branches
pub async fn list_branches(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    require_read(&state, &ds, uid)?;

    let versions = registry::list_versions(&state.store, &state.base_url, &id);
    let (latest_pub, _) = registry::get_pointers(&state.store, &state.base_url, &id);
    let base_record = latest_pub.as_deref().and_then(|v| versions.iter().find(|x| x.version == v));

    use std::collections::HashMap;
    let mut tips: HashMap<String, &DatasetVersion> = HashMap::new();
    for v in &versions {
        let key = v.branch.clone().unwrap_or_else(|| "main".to_string());
        match tips.get(&key) {
            Some(existing) if existing.created_at >= v.created_at => {}
            _ => { tips.insert(key, v); }
        }
    }

    let mut out: Vec<DatasetBranchInfo> = tips
        .into_iter()
        .map(|(branch, tip)| {
            let (ahead, behind) = match base_record {
                Some(base) if base.version != tip.version => {
                    triple_delta(&state.store, &base.snapshot_graphs, &tip.snapshot_graphs)
                }
                _ => (0, 0),
            };
            DatasetBranchInfo {
                branch,
                tip_version: tip.version.clone(),
                status: tip.status.as_str().to_string(),
                base_version: tip.derived_from.clone(),
                owner: tip.created_by.clone(),
                created_at: tip.created_at.clone(),
                ahead,
                behind,
            }
        })
        .collect();
    out.sort_by(|a, b| a.branch.cmp(&b.branch));
    Ok(Json(out))
}

/// POST /api/datasets/:id/branches
pub async fn create_branch(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Json(body): Json<CreateDatasetBranchRequest>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    let branch = body.branch.trim().to_string();
    if branch.is_empty() {
        return Err(AppError::BadRequest("branch name is required".to_string()));
    }

    let from = registry::get_version(&state.store, &state.base_url, &id, &body.from_version)
        .ok_or_else(|| AppError::NotFound(format!("Version '{}' not found", body.from_version)))?;

    let target_ver = body.target_version.clone()
        .unwrap_or_else(|| format!("{}-{}", body.from_version, branch));
    validate_version(&target_ver)?;
    if registry::version_exists(&state.store, &state.base_url, &id, &target_ver) {
        return Err(AppError::BadRequest(format!("Version '{target_ver}' already exists")));
    }

    let new_map = snapshot::clone_version(&state.store, &state.base_url, &id, &from.source_map, &target_ver)
        .map_err(AppError::from)?;
    let snapshot_graphs: Vec<String> = new_map.iter().map(|m| m.snapshot_graph.clone()).collect();

    let record = DatasetVersion {
        dataset_id: id.clone(),
        version: target_ver.clone(),
        status: VersionStatus::Draft,
        graph_iri: format!("{}/dataset/{}/version/{}", state.base_url, id, target_ver),
        snapshot_graphs,
        source_map: new_map,
        created_at: Utc::now().to_rfc3339(),
        created_by: Some(format!("{}/users/{}", state.base_url, user.user_id)),
        derived_from: Some(body.from_version.clone()),
        notes: None,
        branch: Some(branch),
    };
    registry::insert_version(&state.store, &state.base_url, &record).map_err(AppError::from)?;
    Ok((StatusCode::CREATED, Json(record)))
}
