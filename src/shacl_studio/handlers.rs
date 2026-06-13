//! HTTP handlers for SHACL Studio: shape-graph Library CRUD + revisions, pipeline
//! CRUD + run + history, model-context / derive tooling, and the form platform
//! manifest. Error convention matches the existing SHACL handlers:
//! `Result<_, (StatusCode, String)>`.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use bytes::Bytes;
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{OwnerType, Visibility};
use crate::server::AppState;

use super::access::*;
use super::models::*;
use super::store::ShaclStudioStore;

type ApiErr = (StatusCode, String);

const EMPTY_SHAPES: &str = "# SHACL shapes\nPREFIX sh: <http://www.w3.org/ns/shacl#>\nPREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n";

fn studio(state: &AppState) -> ShaclStudioStore {
    ShaclStudioStore::new(state.auth_db.pool())
}
fn org_ids(state: &AppState, uid: &str) -> Vec<String> {
    state.auth_db.get_user_org_ids(uid).unwrap_or_default()
}
fn e500<E: ToString>(e: E) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
fn parse_visibility(s: &Option<String>) -> Visibility {
    s.as_deref()
        .and_then(Visibility::from_str)
        .unwrap_or(Visibility::Private)
}

/// Resolve the owner for a new artifact from the request, defaulting to the
/// current user. Organisation ownership requires membership.
fn resolve_owner(
    state: &AppState,
    user: &AuthenticatedUser,
    owner_type: &Option<String>,
    owner_id: &Option<String>,
) -> Result<(OwnerType, String), ApiErr> {
    match owner_type.as_deref().and_then(OwnerType::from_str) {
        Some(OwnerType::Organisation) | Some(OwnerType::Group) => {
            let oid = owner_id
                .clone()
                .ok_or((StatusCode::BAD_REQUEST, "owner_id required".into()))?;
            if !user.is_admin() && !org_ids(state, &user.user_id).iter().any(|o| o == &oid) {
                return Err((
                    StatusCode::FORBIDDEN,
                    "Not a member of the owning organisation".into(),
                ));
            }
            Ok((OwnerType::Organisation, oid))
        }
        _ => Ok((OwnerType::User, user.user_id.clone())),
    }
}

// ─── Shape graphs ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateShapeGraphBody {
    pub name: String,
    pub description: Option<String>,
    pub visibility: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub owner_type: Option<String>,
    pub owner_id: Option<String>,
    pub turtle: Option<String>,
    pub source: Option<String>,
}

pub async fn create_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<CreateShapeGraphBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let st = studio(&state);
    let (owner_type, owner_id) = resolve_owner(&state, &user, &body.owner_type, &body.owner_id)?;
    let graph_iri = format!("urn:shapes:{}", Uuid::new_v4());
    let source = body
        .source
        .as_deref()
        .map(ShapeSource::from_str_or_manual)
        .unwrap_or(ShapeSource::Manual);

    let set = st
        .create_shape_graph(
            &body.name,
            body.description.as_deref(),
            owner_type,
            &owner_id,
            parse_visibility(&body.visibility),
            &graph_iri,
            &body.tags,
            source,
            Some(&user.user_id),
        )
        .map_err(e500)?;

    let turtle = body.turtle.unwrap_or_else(|| EMPTY_SHAPES.to_string());
    state
        .store
        .graph_store_put(Some(&graph_iri), &turtle, oxigraph::io::RdfFormat::Turtle)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let (targets, count) = super::run::analyze_shapes_graph(&state.store, &graph_iri);
    let version = st
        .save_shape_graph_revision(
            &set.id,
            &turtle,
            &targets,
            count,
            Some("Created"),
            Some(&user.user_id),
        )
        .map_err(e500)?;
    record_shape_graph_commit(
        &state,
        &set.id,
        &graph_iri,
        version,
        Some("Created"),
        &user.user_id,
    );

    let set = st.get_shape_graph(&set.id).map_err(e500)?;
    Ok((StatusCode::CREATED, Json(set)))
}

pub async fn list_shape_graphs(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiErr> {
    let st = studio(&state);
    let orgs = org_ids(&state, &user.user_id);
    let sets: Vec<ShapeGraph> = st
        .list_shape_graphs()
        .map_err(e500)?
        .into_iter()
        .filter(|s| can_access_set(s, Some(&user.user_id), &orgs))
        .collect();
    Ok(Json(sets))
}

async fn load_set_checked(
    state: &AppState,
    user: &AuthenticatedUser,
    id: &str,
    need_manage: bool,
) -> Result<ShapeGraph, ApiErr> {
    let st = studio(state);
    let set = st
        .get_shape_graph(id)
        .map_err(e500)?
        .ok_or((StatusCode::NOT_FOUND, "Shape graph not found".into()))?;
    let orgs = org_ids(state, &user.user_id);
    let ok = if need_manage {
        can_manage_set(&set, Some(&user.user_id), &orgs, user.is_admin())
    } else {
        can_access_set(&set, Some(&user.user_id), &orgs)
    };
    if !ok {
        return Err((StatusCode::FORBIDDEN, "Access denied".into()));
    }
    Ok(set)
}

pub async fn get_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, false).await?;
    Ok(Json(set))
}

#[derive(Deserialize)]
pub struct UpdateShapeGraphBody {
    pub name: String,
    pub description: Option<String>,
    pub visibility: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

pub async fn update_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateShapeGraphBody>,
) -> Result<impl IntoResponse, ApiErr> {
    load_set_checked(&state, &user, &id, true).await?;
    studio(&state)
        .update_shape_graph_meta(
            &id,
            &body.name,
            body.description.as_deref(),
            parse_visibility(&body.visibility),
            &body.tags,
        )
        .map_err(e500)?;
    let set = studio(&state).get_shape_graph(&id).map_err(e500)?;
    Ok(Json(set))
}

pub async fn delete_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, true).await?;
    // Best-effort clear of the backing graph, then drop the DB rows.
    let _ = state
        .store
        .update(&format!("CLEAR SILENT GRAPH <{}>", set.graph_iri));
    studio(&state).delete_shape_graph(&id).map_err(e500)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_shape_graph_turtle(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: axum::http::HeaderMap,
) -> Result<Response, ApiErr> {
    let set = load_set_checked(&state, &user, &id, false).await?;
    let want_shaclc = q.get("format").map(|v| v == "shaclc").unwrap_or(false)
        || headers
            .get(ACCEPT)
            .and_then(|v| v.to_str().ok())
            .map(|a| a.contains("text/shaclc"))
            .unwrap_or(false);
    if want_shaclc {
        let shaclc = crate::shaclc::serialize(&state.store, &set.graph_iri).map_err(e500)?;
        return Ok((StatusCode::OK, [(CONTENT_TYPE, "text/shaclc")], shaclc).into_response());
    }
    let data = state
        .store
        .graph_store_get(Some(&set.graph_iri), oxigraph::io::RdfFormat::Turtle)
        .map_err(e500)?;
    Ok((StatusCode::OK, [(CONTENT_TYPE, "text/turtle")], data).into_response())
}

pub async fn put_shape_graph_turtle(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, true).await?;
    let raw = String::from_utf8(body.to_vec())
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid UTF-8".into()))?;
    let ct = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");
    let turtle = if ct.contains("shaclc") {
        crate::shaclc::parse(&raw)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("SHACLC parse error: {e}")))?
    } else {
        raw
    };
    let version = write_shapes_revision(
        &state,
        &set.graph_iri,
        &id,
        &turtle,
        Some("Edited"),
        &user.user_id,
    )?;
    Ok(Json(serde_json::json!({ "version": version })))
}

/// Write Turtle to a shape graph's graph, recompute facets, and snapshot it.
fn write_shapes_revision(
    state: &AppState,
    graph_iri: &str,
    set_id: &str,
    turtle: &str,
    note: Option<&str>,
    by: &str,
) -> Result<i64, ApiErr> {
    state
        .store
        .graph_store_put(Some(graph_iri), turtle, oxigraph::io::RdfFormat::Turtle)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let (targets, count) = super::run::analyze_shapes_graph(&state.store, graph_iri);
    let version = studio(state)
        .save_shape_graph_revision(set_id, turtle, &targets, count, note, Some(by))
        .map_err(e500)?;
    record_shape_graph_commit(state, set_id, graph_iri, version, note, by);
    Ok(version)
}

/// IRI a shape graph is addressed by in the commit trail.
fn shape_graph_subject_iri(base_url: &str, set_id: &str) -> String {
    format!(
        "{}/shacl/shape-graphs/{}",
        base_url.trim_end_matches('/'),
        set_id
    )
}

/// Record a shape-graph change in the shared commit trail (kind = Shapes). The
/// backing shapes graph is the affected graph; the prior version is the parent.
/// Best-effort: a failed commit must never abort the change that persisted.
fn record_shape_graph_commit(
    state: &AppState,
    set_id: &str,
    graph_iri: &str,
    version: i64,
    note: Option<&str>,
    by: &str,
) {
    let msg = match note.map(str::trim) {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => format!("Revision {version}"),
    };
    let mut rec = crate::commit_log::CommitRecord::new(crate::commit_log::CommitKind::Shapes, msg);
    rec.actor_iri = Some(format!("{}/users/{}", state.base_url, by));
    rec.subject_iri = Some(shape_graph_subject_iri(&state.base_url, set_id));
    rec.version = Some(version.to_string());
    rec.revision = Some(version.to_string());
    if version > 1 {
        rec.parent_revision = Some((version - 1).to_string());
    }
    rec.affected_graphs = vec![graph_iri.to_string()];
    if let Err(e) = crate::commit_log::insert_commit(&state.store, &state.base_url, &rec) {
        tracing::warn!("failed to record shape-graph commit for {set_id} v{version}: {e}");
    }
}

pub async fn list_shape_graph_revisions(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    load_set_checked(&state, &user, &id, false).await?;
    let revs = studio(&state)
        .list_shape_graph_revisions(&id)
        .map_err(e500)?;
    Ok(Json(revs))
}

pub async fn get_shape_graph_revision(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((id, rev)): Path<(String, i64)>,
) -> Result<impl IntoResponse, ApiErr> {
    load_set_checked(&state, &user, &id, false).await?;
    let rev = studio(&state)
        .get_shape_graph_revision(&id, rev)
        .map_err(e500)?
        .ok_or((StatusCode::NOT_FOUND, "Revision not found".into()))?;
    Ok(Json(rev))
}

pub async fn restore_shape_graph_revision(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((id, rev)): Path<(String, i64)>,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, true).await?;
    let snapshot = studio(&state)
        .get_shape_graph_revision(&id, rev)
        .map_err(e500)?
        .ok_or((StatusCode::NOT_FOUND, "Revision not found".into()))?;
    let version = write_shapes_revision(
        &state,
        &set.graph_iri,
        &id,
        &snapshot.turtle,
        Some(&format!("Restored revision {rev}")),
        &user.user_id,
    )?;
    Ok(Json(serde_json::json!({ "version": version })))
}

#[derive(Deserialize)]
pub struct CloneShapeGraphBody {
    pub name: Option<String>,
}

pub async fn clone_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CloneShapeGraphBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let src = load_set_checked(&state, &user, &id, false).await?;
    let st = studio(&state);
    let turtle = state
        .store
        .graph_store_get(Some(&src.graph_iri), oxigraph::io::RdfFormat::Turtle)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| EMPTY_SHAPES.to_string());
    let graph_iri = format!("urn:shapes:{}", Uuid::new_v4());
    let name = body.name.unwrap_or_else(|| format!("{} (copy)", src.name));
    let set = st
        .create_shape_graph(
            &name,
            src.description.as_deref(),
            OwnerType::User,
            &user.user_id,
            Visibility::Private,
            &graph_iri,
            &src.tags,
            ShapeSource::Manual,
            Some(&user.user_id),
        )
        .map_err(e500)?;
    write_shapes_revision(
        &state,
        &graph_iri,
        &set.id,
        &turtle,
        Some(&format!("Cloned from {}", src.id)),
        &user.user_id,
    )?;
    let set = st.get_shape_graph(&set.id).map_err(e500)?;
    Ok((StatusCode::CREATED, Json(set)))
}

// ─── Meta-validation (SHACL-of-SHACL) ────────────────────────────────────────

/// Validate a shape graph's Turtle *as data* against the built-in SHACL-SHACL
/// meta-shapes. Reuses the shared validation plumbing — no pipeline row, no
/// persisted run. Read access to the set is sufficient.
pub async fn validate_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, false).await?;
    let store = state.store.clone();
    let data_graph = set.graph_iri.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        super::run::run_validation(
            &store,
            &[super::seed::SHACL_SHACL_GRAPH.to_string()],
            &[data_graph],
            SeverityThreshold::Violation,
            false,
        )
    })
    .await
    .map_err(e500)?
    .map_err(e500)?;

    Ok(Json(serde_json::json!({
        "shape_graph_id": id,
        "conforms": outcome.report.conforms,
        "passes": outcome.passes,
        "results_count": outcome.report.results_count,
        "violation_count": outcome.violation_count,
        "warning_count": outcome.warning_count,
        "info_count": outcome.info_count,
        "report": outcome.report,
    })))
}

// ─── Lifecycle & history ─────────────────────────────────────────────────────

/// Transition a shape graph's lifecycle status. Gating is independent of status
/// (a Draft set still gates) — these endpoints only express publication intent
/// to consumers (form platform, downstream pipelines). Mirrors the dataset-version
/// lifecycle, minus branches/latest-published pointers (shape graphs have neither).
async fn transition_shape_graph(
    state: &AppState,
    user: &AuthenticatedUser,
    id: &str,
    to: VersionStatus,
) -> Result<Response, ApiErr> {
    let set = load_set_checked(state, user, id, true).await?;
    let allowed = match to {
        VersionStatus::Staged => matches!(set.status, VersionStatus::Draft),
        VersionStatus::Published => {
            matches!(set.status, VersionStatus::Draft | VersionStatus::Staged)
        }
        VersionStatus::Deprecated => !matches!(set.status, VersionStatus::Deprecated),
        VersionStatus::Draft => false,
    };
    if !allowed {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Cannot transition shape graph from {} to {}",
                set.status.as_str(),
                to.as_str()
            ),
        ));
    }
    studio(state).set_shape_graph_status(id, to).map_err(e500)?;
    Ok(Json(serde_json::json!({ "status": to.as_str() })).into_response())
}

pub async fn stage_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    transition_shape_graph(&state, &user, &id, VersionStatus::Staged).await
}

pub async fn publish_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    transition_shape_graph(&state, &user, &id, VersionStatus::Published).await
}

pub async fn deprecate_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    transition_shape_graph(&state, &user, &id, VersionStatus::Deprecated).await
}

/// GET /api/shacl/shape-graphs/:id/commits — the shape graph's slice of the shared
/// commit trail (kind = Shapes), newest first, with actor names resolved.
pub async fn list_shape_graph_commits(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<crate::commit_log::CommitsParams>,
) -> Result<impl IntoResponse, ApiErr> {
    load_set_checked(&state, &user, &id, false).await?;
    let subject = shape_graph_subject_iri(&state.base_url, &id);
    let scope = crate::commit_log::CommitScope::Subject(subject);
    let mut commits = crate::commit_log::list_commits(&state.store, &scope, &params.to_query());
    crate::commit_log::resolve_actors(state.auth_db.as_ref(), &mut commits);
    Ok(Json(commits))
}

// ─── Bindings (the validation layer) ─────────────────────────────────────────

#[derive(Deserialize)]
pub struct BindingBody {
    pub target: ValidationTarget,
    /// The shape graph that validates the target.
    pub shape_graph_id: String,
}

#[derive(Deserialize)]
pub struct BindingQuery {
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    /// Reverse lookup: which targets does this shape graph validate?
    pub shape_graph_id: Option<String>,
}

/// Resolve a target to its validation-layer IRI, checking the caller may bind
/// to it. Dataset → write on the dataset; Graph → graph-level write ACL (same
/// gate as a Graph Store write); ShapeGraph → manage on the shape graph.
async fn resolve_target_for_write(
    state: &AppState,
    user: &AuthenticatedUser,
    target: &ValidationTarget,
) -> Result<String, ApiErr> {
    match target.kind {
        TargetKind::Dataset => {
            let ds = state
                .auth_db
                .get_dataset(&target.id)
                .map_err(e500)?
                .ok_or((StatusCode::NOT_FOUND, "Dataset not found".into()))?;
            if !state
                .auth_db
                .can_write_dataset(&user.user_id, &ds)
                .map_err(e500)?
            {
                return Err((
                    StatusCode::FORBIDDEN,
                    "Write access denied for dataset".into(),
                ));
            }
            Ok(super::bindings::dataset_target_iri(
                &state.base_url,
                &target.id,
            ))
        }
        TargetKind::Graph => {
            if !crate::auth::acl::check_graph_permission(
                Some(user),
                &target.id,
                "write",
                &state.auth_db,
            ) {
                return Err((
                    StatusCode::FORBIDDEN,
                    format!("Write access denied for graph <{}>", target.id),
                ));
            }
            Ok(target.id.clone())
        }
        TargetKind::ShapeGraph => {
            let set = load_set_checked(state, user, &target.id, true).await?;
            Ok(set.graph_iri)
        }
    }
}

/// Read-side target resolution: Dataset/ShapeGraph require access; a graph's
/// bindings are low-sensitivity metadata, so any authenticated caller may read.
async fn resolve_target_for_read(
    state: &AppState,
    user: &AuthenticatedUser,
    target: &ValidationTarget,
) -> Result<String, ApiErr> {
    match target.kind {
        TargetKind::Dataset => {
            let ds = state
                .auth_db
                .get_dataset(&target.id)
                .map_err(e500)?
                .ok_or((StatusCode::NOT_FOUND, "Dataset not found".into()))?;
            if !state
                .auth_db
                .can_access_dataset(Some(&user.user_id), &ds)
                .map_err(e500)?
            {
                return Err((StatusCode::FORBIDDEN, "Access denied".into()));
            }
            Ok(super::bindings::dataset_target_iri(
                &state.base_url,
                &target.id,
            ))
        }
        TargetKind::Graph => Ok(target.id.clone()),
        TargetKind::ShapeGraph => {
            let set = load_set_checked(state, user, &target.id, false).await?;
            Ok(set.graph_iri)
        }
    }
}

/// Record a binding change in the shape graph's commit history (the binding lives
/// in the validation-layer graph). Best-effort.
fn record_binding_commit(
    state: &AppState,
    set: &ShapeGraph,
    target_iri: &str,
    added: bool,
    by: &str,
) {
    let verb = if added { "Bound to" } else { "Unbound from" };
    let mut rec = crate::commit_log::CommitRecord::new(
        crate::commit_log::CommitKind::Shapes,
        format!("{verb} {target_iri}"),
    );
    rec.actor_iri = Some(format!("{}/users/{}", state.base_url, by));
    rec.subject_iri = Some(shape_graph_subject_iri(&state.base_url, &set.id));
    rec.affected_graphs = vec![super::bindings::VALIDATION_GRAPH.to_string()];
    if let Err(e) = crate::commit_log::insert_commit(&state.store, &state.base_url, &rec) {
        tracing::warn!("failed to record binding commit for set {}: {e}", set.id);
    }
}

/// POST /api/shacl/bindings — bind a shape graph to a target (dataset/graph/shape
/// set). Idempotent. The caller must be able to write the target and *manage* the
/// shape graph (a binding makes it an enforcing validator).
pub async fn create_binding(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<BindingBody>,
) -> Result<impl IntoResponse, ApiErr> {
    // Binding makes this shape graph an enforcing validator of the target, so the
    // caller must be able to *manage* the shape graph (not merely read it).
    let set = load_set_checked(&state, &user, &body.shape_graph_id, true).await?;
    let target_iri = resolve_target_for_write(&state, &user, &body.target).await?;
    super::bindings::add_binding(&state.store, &target_iri, &set.graph_iri).map_err(e500)?;
    record_binding_commit(&state, &set, &target_iri, true, &user.user_id);
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "target": target_iri,
            "shape_graph_id": set.id,
            "shape_graph_graph": set.graph_iri,
        })),
    ))
}

/// DELETE /api/shacl/bindings — remove a binding. Same access rules as create.
pub async fn delete_binding(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<BindingBody>,
) -> Result<impl IntoResponse, ApiErr> {
    // Same authority as create_binding: managing the enforcing validator.
    let set = load_set_checked(&state, &user, &body.shape_graph_id, true).await?;
    let target_iri = resolve_target_for_write(&state, &user, &body.target).await?;
    super::bindings::remove_binding(&state.store, &target_iri, &set.graph_iri).map_err(e500)?;
    record_binding_commit(&state, &set, &target_iri, false, &user.user_id);
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/shacl/bindings — list bindings. With `?target_kind=&target_id=` it
/// returns the shape graphs bound to that target; with `?shape_graph_id=` it returns
/// the target IRIs that shape graph validates (reverse, for impact display).
pub async fn list_bindings(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Query(q): Query<BindingQuery>,
) -> Result<impl IntoResponse, ApiErr> {
    if let Some(set_id) = q.shape_graph_id {
        let set = load_set_checked(&state, &user, &set_id, false).await?;
        let targets = super::bindings::targets_for_shape_graph(&state.store, &set.graph_iri);
        return Ok(Json(
            serde_json::json!({ "shape_graph_id": set.id, "targets": targets }),
        ));
    }
    let (Some(kind), Some(id)) = (q.target_kind, q.target_id) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Provide either shape_graph_id or target_kind+target_id".into(),
        ));
    };
    let target = ValidationTarget {
        kind: TargetKind::from_str_or_dataset(&kind),
        id,
    };
    let target_iri = resolve_target_for_read(&state, &user, &target).await?;
    let st = studio(&state);
    let orgs = org_ids(&state, &user.user_id);
    // Resolve bound shape-graph graphs back to records the caller may access.
    let sets: Vec<ShapeGraph> = super::bindings::bindings_for_target(&state.store, &target_iri)
        .into_iter()
        .filter_map(|giri| st.get_shape_graph_by_iri(&giri).ok().flatten())
        .filter(|s| can_access_set(s, Some(&user.user_id), &orgs))
        .collect();
    Ok(Json(
        serde_json::json!({ "target": target_iri, "shape_graphs": sets }),
    ))
}

/// GET /api/datasets/:id/effective-shapes — the shape graphs that effectively
/// apply to a dataset (its own bindings ∪ each contained graph's bindings).
pub async fn dataset_effective_shapes(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(e500)?
        .ok_or((StatusCode::NOT_FOUND, "Dataset not found".into()))?;
    if !state
        .auth_db
        .can_access_dataset(Some(&user.user_id), &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".into()));
    }
    let sets = super::bindings::effective_shape_graphs_for_dataset(
        &state.store,
        &state.auth_db,
        &studio(&state),
        &state.base_url,
        &ds,
    );
    Ok(Json(sets))
}

// ─── Shapes catalog & compose ────────────────────────────────────────────────

/// GET /api/shacl/shapes — the **graph-first** shapes catalog. Real stores hold
/// tens of thousands of shapes (e.g. an large information model), so without a
/// `?graph=` parameter this returns a cheap *summary* of the graphs that contain
/// shapes (`{ "graphs": [...] }`, each with node/property counts + registration);
/// with `?graph=<iri>` it returns that one graph's shapes (`{ "graph", "shapes" }`).
/// Either way, graphs that are registered shape graphs the caller cannot access
/// are hidden.
pub async fn list_shapes_catalog(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiErr> {
    let st = studio(&state);
    let orgs = org_ids(&state, &user.user_id);
    let mut reg_all: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut reg_access: HashMap<String, (String, String)> = HashMap::new();
    for s in st.list_shape_graphs().map_err(e500)? {
        reg_all.insert(s.graph_iri.clone());
        if can_access_set(&s, Some(&user.user_id), &orgs) {
            reg_access.insert(s.graph_iri.clone(), (s.id.clone(), s.name.clone()));
        }
    }
    let hidden = |g: &str| reg_all.contains(g) && !reg_access.contains_key(g);

    // Drill-down: one graph's shapes.
    if let Some(graph) = params.get("graph") {
        if hidden(graph) {
            return Err((
                StatusCode::FORBIDDEN,
                "Access denied for that shape graph".into(),
            ));
        }
        let reg = reg_access.get(graph);
        let shapes: Vec<serde_json::Value> = super::catalog::catalog_shapes(&state.store, graph)
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "graph": s.graph, "shape": s.shape, "kind": s.kind, "label": s.label,
                    "target_classes": s.target_classes, "path": s.path,
                    "registered": reg.is_some(),
                    "shape_graph_id": reg.map(|(id, _)| id.clone()),
                    "shape_graph_name": reg.map(|(_, n)| n.clone()),
                })
            })
            .collect();
        return Ok(Json(
            serde_json::json!({ "graph": graph, "shapes": shapes }),
        ));
    }

    // Default: the cheap graph summary.
    let graphs: Vec<serde_json::Value> = super::catalog::catalog_graph_summary(&state.store)
        .into_iter()
        .filter(|g| !hidden(&g.graph))
        .map(|g| {
            let reg = reg_access.get(&g.graph);
            serde_json::json!({
                "graph": g.graph,
                "node_count": g.node_count,
                "property_count": g.property_count,
                "total": g.node_count + g.property_count,
                "registered": reg.is_some(),
                "shape_graph_id": reg.map(|(id, _)| id.clone()),
                "shape_graph_name": reg.map(|(_, n)| n.clone()),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "graphs": graphs })))
}

#[derive(Deserialize)]
pub struct ImportShapeRef {
    pub source_graph: String,
    pub shape: String,
}

#[derive(Deserialize)]
pub struct ImportShapesBody {
    pub shapes: Vec<ImportShapeRef>,
    pub note: Option<String>,
}

/// POST /api/shacl/shape-graphs/:id/import-shapes — copy existing shapes (each
/// with its full closure) into this shape graph. The picked shapes become part
/// of the graph (copy semantics). Records a new revision + Shapes commit.
pub async fn import_shapes(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ImportShapesBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, true).await?;
    if body.shapes.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "No shapes to import".into()));
    }
    // Copying a shape reads its source graph's triples into the destination, so
    // the caller must have read on every source graph — mirror the read-ACL gate
    // in resolve_target_for_read / register_shape_graph. Manage on the
    // destination (above) is not sufficient to pull from an arbitrary graph.
    for r in &body.shapes {
        if !crate::auth::acl::check_graph_permission(
            Some(&user),
            &r.source_graph,
            "read",
            &state.auth_db,
        ) {
            return Err((
                StatusCode::FORBIDDEN,
                format!("Read access denied for graph <{}>", r.source_graph),
            ));
        }
    }
    let refs: Vec<(String, String)> = body
        .shapes
        .iter()
        .map(|r| (r.source_graph.clone(), r.shape.clone()))
        .collect();
    let copied =
        super::catalog::import_shapes_into(&state.store, &set.graph_iri, &refs).map_err(e500)?;

    // Snapshot the post-import Turtle as a new revision (+ commit).
    let turtle = state
        .store
        .graph_store_get(Some(&set.graph_iri), oxigraph::io::RdfFormat::Turtle)
        .map_err(e500)?;
    let turtle = String::from_utf8(turtle).map_err(|_| e500("graph is not valid UTF-8"))?;
    let note = body
        .note
        .unwrap_or_else(|| format!("Imported {copied} shape(s)"));
    let (targets, count) = super::run::analyze_shapes_graph(&state.store, &set.graph_iri);
    let version = studio(&state)
        .save_shape_graph_revision(
            &set.id,
            &turtle,
            &targets,
            count,
            Some(&note),
            Some(&user.user_id),
        )
        .map_err(e500)?;
    record_shape_graph_commit(
        &state,
        &set.id,
        &set.graph_iri,
        version,
        Some(&note),
        &user.user_id,
    );
    let updated = studio(&state).get_shape_graph(&set.id).map_err(e500)?;
    Ok(Json(
        serde_json::json!({ "imported": copied, "version": version, "shape_graph": updated }),
    ))
}

#[derive(Deserialize)]
pub struct RegisterShapeGraphBody {
    pub graph_iri: String,
    pub name: String,
    pub description: Option<String>,
    pub visibility: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub owner_type: Option<String>,
    pub owner_id: Option<String>,
}

/// POST /api/shacl/register-shape-graph — adopt an existing named graph that
/// already holds SHACL as a first-class shape graph, *in place* (no copy): the
/// record points at the graph. Powers "this existing shapes graph should be in
/// the Library too". Idempotent: re-registering returns the existing record.
pub async fn register_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<RegisterShapeGraphBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let st = studio(&state);
    if let Some(existing) = st.get_shape_graph_by_iri(&body.graph_iri).map_err(e500)? {
        return Ok((StatusCode::OK, Json(existing)));
    }
    if !crate::auth::acl::check_graph_permission(
        Some(&user),
        &body.graph_iri,
        "read",
        &state.auth_db,
    ) {
        return Err((
            StatusCode::FORBIDDEN,
            format!("Read access denied for graph <{}>", body.graph_iri),
        ));
    }
    let (targets, count) = super::run::analyze_shapes_graph(&state.store, &body.graph_iri);
    if count == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Graph contains no SHACL shapes".into(),
        ));
    }
    let (owner_type, owner_id) = resolve_owner(&state, &user, &body.owner_type, &body.owner_id)?;
    let set = st
        .create_shape_graph(
            &body.name,
            body.description.as_deref(),
            owner_type,
            &owner_id,
            parse_visibility(&body.visibility),
            &body.graph_iri,
            &body.tags,
            ShapeSource::Imported,
            Some(&user.user_id),
        )
        .map_err(e500)?;
    // Seed revision 1 from the graph's current Turtle (adopt in place — no PUT).
    let turtle = state
        .store
        .graph_store_get(Some(&body.graph_iri), oxigraph::io::RdfFormat::Turtle)
        .map_err(e500)?;
    let turtle = String::from_utf8(turtle).map_err(|_| e500("graph is not valid UTF-8"))?;
    let version = st
        .save_shape_graph_revision(
            &set.id,
            &turtle,
            &targets,
            count,
            Some("Registered existing graph"),
            Some(&user.user_id),
        )
        .map_err(e500)?;
    record_shape_graph_commit(
        &state,
        &set.id,
        &body.graph_iri,
        version,
        Some("Registered existing graph"),
        &user.user_id,
    );
    let set = st.get_shape_graph(&set.id).map_err(e500)?.ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "shape graph vanished after create".into(),
    ))?;
    Ok((StatusCode::CREATED, Json(set)))
}

// ─── Pipelines ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PipelineBody {
    pub name: String,
    pub description: Option<String>,
    pub visibility: Option<String>,
    pub owner_type: Option<String>,
    pub owner_id: Option<String>,
    #[serde(default)]
    pub targets: Vec<ValidationTarget>,
    #[serde(default)]
    pub dataset_ids: Vec<String>,
    #[serde(default)]
    pub graph_iris: Vec<String>,
    #[serde(default)]
    pub target_classes: Vec<String>,
    #[serde(default)]
    pub shape_graph_ids: Vec<String>,
    pub severity_threshold: Option<String>,
    #[serde(default)]
    pub run_inference: bool,
    pub max_results: Option<i64>,
    #[serde(default)]
    pub trigger_on_write: bool,
    pub schedule_cron: Option<String>,
    #[serde(default)]
    pub gate_writes: bool,
    pub retention: Option<i64>,
    #[serde(default)]
    pub inferred_target: Option<WriteTarget>,
    #[serde(default)]
    pub inferred_target_graph: Option<String>,
    #[serde(default)]
    pub results_target: Option<ResultsTarget>,
    #[serde(default)]
    pub results_target_graph: Option<String>,
}

fn validate_cron(cron: &Option<String>) -> Result<(), ApiErr> {
    if let Some(c) = cron.as_deref().filter(|s| !s.is_empty()) {
        if c.split_whitespace().count() != 5 {
            return Err((
                StatusCode::BAD_REQUEST,
                "schedule_cron must be a 5-field cron expression".into(),
            ));
        }
    }
    Ok(())
}

/// Authorize a pipeline's *write* surface against the acting user before it is stored/updated.
///
/// Admins bypass. Otherwise every graph the pipeline would WRITE must be writable by the caller,
/// using the same authority as the SPARQL/Graph-Store write path (`check_graph_permission(write)`):
/// - an explicit (caller-supplied) inferred/results target graph, and
/// - every data graph in scope when `run_inference` is set — SHACL-AF inference
///   materialises triples *in place* into those graphs, so it is a write to them.
///
/// The pipeline's own auto-namespaced `urn:system:*:{id}` report/inference graphs are server-owned
/// and exempt. `exec::owner_can_write` re-checks the same authority at run time (covering the
/// scheduler's otherwise-ambient authority and any later change to the owner's grants).
fn authorize_pipeline_targets(
    state: &AppState,
    user: &AuthenticatedUser,
    pipeline: &ValidationPipeline,
) -> Result<(), ApiErr> {
    if user.is_admin() {
        return Ok(());
    }
    use crate::auth::acl::check_graph_permission;

    let mut write_targets: Vec<String> = Vec::new();
    if pipeline.run_inference {
        if let Some(g) = pipeline.inferred_target_graph.as_deref() {
            if !g.trim().is_empty() {
                write_targets.push(g.to_string());
            }
        }
    }
    if matches!(pipeline.results_target, ResultsTarget::NewGraph) {
        if let Some(g) = pipeline.results_target_graph.as_deref() {
            if !g.trim().is_empty() {
                write_targets.push(g.to_string());
            }
        }
    }
    // In-place inference writes into the data graphs themselves.
    if pipeline.run_inference {
        let st = studio(state);
        write_targets.extend(super::exec::resolve_data_graphs(
            &state.auth_db,
            &st,
            pipeline,
        ));
    }
    write_targets.sort();
    write_targets.dedup();
    for g in &write_targets {
        if !check_graph_permission(Some(user), g, "write", &state.auth_db) {
            return Err((
                StatusCode::FORBIDDEN,
                format!("Write access denied for graph <{g}>"),
            ));
        }
    }
    Ok(())
}

pub async fn create_pipeline(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<PipelineBody>,
) -> Result<impl IntoResponse, ApiErr> {
    validate_cron(&body.schedule_cron)?;
    let (owner_type, owner_id) = resolve_owner(&state, &user, &body.owner_type, &body.owner_id)?;
    let now = chrono::Utc::now().to_rfc3339();
    let schedule = body.schedule_cron.filter(|s| !s.is_empty());
    let pipeline = ValidationPipeline {
        id: Uuid::new_v4().to_string(),
        name: body.name,
        description: body.description,
        owner_type,
        owner_id,
        visibility: parse_visibility(&body.visibility),
        targets: body.targets,
        dataset_ids: body.dataset_ids,
        graph_iris: body.graph_iris,
        target_classes: body.target_classes,
        shape_graph_ids: body.shape_graph_ids,
        severity_threshold: body
            .severity_threshold
            .as_deref()
            .map(SeverityThreshold::from_str_or_default)
            .unwrap_or(SeverityThreshold::Violation),
        run_inference: body.run_inference,
        max_results: body.max_results,
        trigger_on_write: body.trigger_on_write,
        schedule_cron: schedule,
        gate_writes: body.gate_writes,
        retention: body.retention.unwrap_or(50).clamp(1, 500),
        inferred_target: body.inferred_target.unwrap_or_default(),
        inferred_target_graph: body
            .inferred_target_graph
            .clone()
            .filter(|s| !s.trim().is_empty()),
        results_target: body.results_target.unwrap_or_default(),
        results_target_graph: body
            .results_target_graph
            .clone()
            .filter(|s| !s.trim().is_empty()),
        last_run_at: None,
        last_conforms: None,
        created_by: Some(user.user_id.clone()),
        created_at: now.clone(),
        updated_at: now,
    };
    authorize_pipeline_targets(&state, &user, &pipeline)?;
    studio(&state).insert_pipeline(&pipeline).map_err(e500)?;
    Ok((StatusCode::CREATED, Json(pipeline)))
}

pub async fn list_pipelines(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiErr> {
    let orgs = org_ids(&state, &user.user_id);
    let pipelines: Vec<ValidationPipeline> = studio(&state)
        .list_pipelines()
        .map_err(e500)?
        .into_iter()
        .filter(|p| can_access_pipeline(p, Some(&user.user_id), &orgs))
        .collect();
    Ok(Json(pipelines))
}

async fn load_pipeline_checked(
    state: &AppState,
    user: &AuthenticatedUser,
    id: &str,
    need_manage: bool,
) -> Result<ValidationPipeline, ApiErr> {
    let p = studio(state)
        .get_pipeline(id)
        .map_err(e500)?
        .ok_or((StatusCode::NOT_FOUND, "Pipeline not found".into()))?;
    let orgs = org_ids(state, &user.user_id);
    let ok = if need_manage {
        can_manage_pipeline(&p, Some(&user.user_id), &orgs, user.is_admin())
    } else {
        can_access_pipeline(&p, Some(&user.user_id), &orgs)
    };
    if !ok {
        return Err((StatusCode::FORBIDDEN, "Access denied".into()));
    }
    Ok(p)
}

pub async fn get_pipeline(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let p = load_pipeline_checked(&state, &user, &id, false).await?;
    Ok(Json(p))
}

pub async fn update_pipeline(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PipelineBody>,
) -> Result<impl IntoResponse, ApiErr> {
    validate_cron(&body.schedule_cron)?;
    let existing = load_pipeline_checked(&state, &user, &id, true).await?;
    let updated = ValidationPipeline {
        name: body.name,
        description: body.description,
        visibility: parse_visibility(&body.visibility),
        targets: body.targets,
        dataset_ids: body.dataset_ids,
        graph_iris: body.graph_iris,
        target_classes: body.target_classes,
        shape_graph_ids: body.shape_graph_ids,
        severity_threshold: body
            .severity_threshold
            .as_deref()
            .map(SeverityThreshold::from_str_or_default)
            .unwrap_or(existing.severity_threshold),
        run_inference: body.run_inference,
        max_results: body.max_results,
        trigger_on_write: body.trigger_on_write,
        schedule_cron: body.schedule_cron.filter(|s| !s.is_empty()),
        gate_writes: body.gate_writes,
        retention: body.retention.unwrap_or(existing.retention).clamp(1, 500),
        inferred_target: body.inferred_target.unwrap_or(existing.inferred_target),
        inferred_target_graph: body
            .inferred_target_graph
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| existing.inferred_target_graph.clone()),
        results_target: body.results_target.unwrap_or(existing.results_target),
        results_target_graph: body
            .results_target_graph
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| existing.results_target_graph.clone()),
        ..existing
    };
    authorize_pipeline_targets(&state, &user, &updated)?;
    studio(&state).update_pipeline(&updated).map_err(e500)?;
    Ok(Json(updated))
}

pub async fn delete_pipeline(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    load_pipeline_checked(&state, &user, &id, true).await?;
    studio(&state).delete_pipeline(&id).map_err(e500)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn run_pipeline(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiErr> {
    let pipeline = load_pipeline_checked(&state, &user, &id, false).await?;
    let store = state.store.clone();
    let auth_db = state.auth_db.clone();
    let base_url = state.base_url.to_string();
    let actor = user.user_id.clone();
    // A test run validates but records nothing — no run row, no last-run update.
    let test = q
        .get("test")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    let dataset_ids = pipeline.dataset_ids.clone();
    // Validation is blocking — run it off the async runtime.
    let run = tokio::task::spawn_blocking(move || {
        let st = ShaclStudioStore::new(auth_db.pool());
        if test {
            super::exec::execute_pipeline_dry(
                &store,
                &auth_db,
                &st,
                &base_url,
                &pipeline,
                Some(&actor),
            )
        } else {
            super::exec::execute_pipeline(
                &store,
                &auth_db,
                &st,
                &base_url,
                &pipeline,
                "manual",
                Some(&actor),
            )
        }
    })
    .await
    .map_err(e500)?
    .map_err(e500)?;

    // Best-effort private usage telemetry. Only real (non-test) runs count.
    if !test {
        for ds in &dataset_ids {
            let _ = state
                .auth_db
                .record_dataset_usage(ds, Some(&user.user_id), "pipeline");
        }
    }
    Ok(Json(run))
}

pub async fn list_pipeline_runs(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiErr> {
    load_pipeline_checked(&state, &user, &id, false).await?;
    let limit = q
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(50);
    let runs = studio(&state)
        .list_pipeline_runs(&id, limit)
        .map_err(e500)?;
    Ok(Json(runs))
}

pub async fn get_pipeline_run(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((id, run_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiErr> {
    load_pipeline_checked(&state, &user, &id, false).await?;
    let run = studio(&state)
        .get_pipeline_run(&run_id)
        .map_err(e500)?
        .filter(|r| r.pipeline_id == id)
        .ok_or((StatusCode::NOT_FOUND, "Run not found".into()))?;
    Ok(Json(run))
}

#[derive(Deserialize)]
pub struct LatestRunsBody {
    pub pipeline_ids: Vec<String>,
}

pub async fn list_latest_pipeline_runs(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<LatestRunsBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let st = studio(&state);
    let orgs = org_ids(&state, &user.user_id);
    // Only include pipelines the caller may access.
    let mut allowed = Vec::new();
    for id in &body.pipeline_ids {
        if let Ok(Some(p)) = st.get_pipeline(id) {
            if can_access_pipeline(&p, Some(&user.user_id), &orgs) {
                allowed.push(id.clone());
            }
        }
    }
    let runs = st.list_latest_runs(&allowed).map_err(e500)?;
    Ok(Json(runs))
}

// ─── Introspection / tooling ─────────────────────────────────────────────────

/// Resolve a `?dataset=` (with access check) or `?graphs=a,b` selector to graph IRIs.
async fn resolve_scope(
    state: &AppState,
    user: &AuthenticatedUser,
    q: &HashMap<String, String>,
) -> Result<Vec<String>, ApiErr> {
    if let Some(ds_id) = q.get("dataset") {
        let ds = state
            .auth_db
            .get_dataset(ds_id)
            .map_err(e500)?
            .ok_or((StatusCode::NOT_FOUND, "Dataset not found".into()))?;
        if !state
            .auth_db
            .can_access_dataset(Some(&user.user_id), &ds)
            .map_err(e500)?
        {
            return Err((StatusCode::FORBIDDEN, "Access denied".into()));
        }
        return state.auth_db.list_dataset_graphs(ds_id).map_err(e500);
    }
    if let Some(graphs) = q.get("graphs") {
        let graphs: Vec<String> = graphs
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        // The `?dataset=` branch above gates on can_access_dataset; caller-named
        // graphs must be gated too, or this leaks model structure from graphs the
        // caller can't read. Require read on each (same gate as a Graph Store read).
        for g in &graphs {
            if !crate::auth::acl::check_graph_permission(Some(user), g, "read", &state.auth_db) {
                return Err((
                    StatusCode::FORBIDDEN,
                    format!("Read access denied for graph <{g}>"),
                ));
            }
        }
        return Ok(graphs);
    }
    Ok(vec![])
}

pub async fn model_context(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiErr> {
    let graphs = resolve_scope(&state, &user, &q).await?;
    let store = state.store.clone();
    let ctx =
        tokio::task::spawn_blocking(move || super::introspect::model_context(&store, &graphs))
            .await
            .map_err(e500)?;
    Ok(Json(ctx))
}

#[derive(Deserialize)]
pub struct DeriveBody {
    pub dataset_id: Option<String>,
    #[serde(default)]
    pub graphs: Vec<String>,
    #[serde(default)]
    pub target_classes: Vec<String>,
}

pub async fn derive_shapes(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<DeriveBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let graphs = if let Some(ds_id) = &body.dataset_id {
        let ds = state
            .auth_db
            .get_dataset(ds_id)
            .map_err(e500)?
            .ok_or((StatusCode::NOT_FOUND, "Dataset not found".into()))?;
        if !state
            .auth_db
            .can_access_dataset(Some(&user.user_id), &ds)
            .map_err(e500)?
        {
            return Err((StatusCode::FORBIDDEN, "Access denied".into()));
        }
        state.auth_db.list_dataset_graphs(ds_id).map_err(e500)?
    } else {
        // The dataset branch above gates on can_access_dataset; caller-named graphs
        // must be gated too, or derivation reads structure/values from graphs the
        // caller can't read. Require read on each (same gate as a Graph Store read).
        for g in &body.graphs {
            if !crate::auth::acl::check_graph_permission(Some(&user), g, "read", &state.auth_db) {
                return Err((
                    StatusCode::FORBIDDEN,
                    format!("Read access denied for graph <{g}>"),
                ));
            }
        }
        body.graphs.clone()
    };
    let targets = body.target_classes.clone();
    let store = state.store.clone();
    let (turtle, stats) = tokio::task::spawn_blocking(move || {
        super::introspect::derive_shapes(&store, &graphs, &targets)
    })
    .await
    .map_err(e500)?;
    Ok(Json(
        serde_json::json!({ "turtle": turtle, "stats": stats }),
    ))
}

// ─── form-manifest (optional auth — public datasets load anonymously) ────

pub async fn form_manifest(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|u| u.user_id.as_str());
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(e500)?
        .ok_or((StatusCode::NOT_FOUND, "Dataset not found".into()))?;
    if !state
        .auth_db
        .can_access_dataset(uid, &dataset)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".into()));
    }
    let manifest = super::manifest::build_manifest(
        &state.store,
        &state.auth_db,
        &state.base_url,
        &studio(&state),
        &dataset,
    );
    Ok(Json(manifest))
}
