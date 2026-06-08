//! HTTP handlers for saved-query management and the parameterised run-as-API
//! endpoints, for each scope (dataset / organisation / group).
//!
//! The scope-generic logic lives in the `*_core` functions; the per-scope
//! wrappers below only extract path parameters and forward the right
//! [`QueryScope`]. Read access follows the owner (dataset visibility, or
//! org/group membership) unless a query is explicitly `public`; management
//! requires owner-admin (or system admin) and a write-scoped credential.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::{header::ACCEPT, HeaderMap},
    response::Response,
    Extension, Json,
};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{Dataset, OwnerType, Role};
use crate::server::error::AppError;
use crate::server::{routes, AppState};

use super::exec::{self, VersionRequest};
use super::models::*;
use super::store::SavedQueryStore;
use super::{llm, metadata, notify, openapi, params};

type OptUser = Option<Extension<AuthenticatedUser>>;

fn internal<E: std::fmt::Display>(e: E) -> AppError {
    AppError::Internal(e.to_string())
}

fn store_of(state: &AppState) -> SavedQueryStore {
    SavedQueryStore::new(state.auth_db.pool())
}

// ─── Access control ──────────────────────────────────────────────────────────

/// Can the principal read/run saved queries in this scope?
fn check_read(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
    sq_visibility: Option<&str>,
) -> Result<(), AppError> {
    if user.map(|u| u.is_admin()).unwrap_or(false) {
        return Ok(());
    }
    if sq_visibility == Some("public") {
        return Ok(());
    }
    let uid = user.map(|u| u.user_id.as_str());
    let ok = match scope {
        QueryScope::Dataset => {
            let ds = state
                .auth_db
                .get_dataset(owner_id)
                .map_err(internal)?
                .ok_or_else(|| AppError::NotFound("dataset not found".to_string()))?;
            state
                .auth_db
                .can_access_dataset(uid, &ds)
                .map_err(internal)?
        }
        QueryScope::Organisation => match uid {
            Some(id) => state
                .auth_db
                .get_org_membership(id, owner_id)
                .map_err(internal)?
                .is_some(),
            None => false,
        },
        QueryScope::Group => match uid {
            Some(id) => state
                .auth_db
                .get_group_membership(id, owner_id)
                .map_err(internal)?
                .is_some(),
            None => false,
        },
    };
    if ok {
        Ok(())
    } else if user.is_none() {
        Err(AppError::Unauthorized(
            "authentication required — provide an API token".to_string(),
        ))
    } else {
        Err(AppError::NotFound("not found".to_string()))
    }
}

/// Whether the principal may write API services in this scope: owner/editor on
/// the dataset, or Admin/Member of the owning org/group (or a system admin),
/// with a write-scoped credential. Non-erroring (used to drive UI state); the
/// gate that produces an error is [`check_write`].
fn can_write_scope(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
) -> bool {
    let Some(u) = user else { return false };
    if !u.write_access {
        return false;
    }
    if u.is_admin() {
        return true;
    }
    match scope {
        QueryScope::Dataset => state
            .auth_db
            .get_dataset(owner_id)
            .ok()
            .flatten()
            .map(|ds| {
                state
                    .auth_db
                    .can_write_dataset(&u.user_id, &ds)
                    .unwrap_or(false)
            })
            .unwrap_or(false),
        QueryScope::Organisation => matches!(
            state
                .auth_db
                .get_org_membership(&u.user_id, owner_id)
                .ok()
                .flatten(),
            Some(Role::Admin) | Some(Role::Member)
        ),
        QueryScope::Group => matches!(
            state
                .auth_db
                .get_group_membership(&u.user_id, owner_id)
                .ok()
                .flatten(),
            Some(Role::Admin) | Some(Role::Member)
        ),
    }
}

/// Require write access. Creating, editing/versioning, deleting, acknowledging
/// and LLM-repair all mutate state and go through here; read/run do not (see
/// [`check_read`]), so anonymous callers can still use services on public data.
fn check_write<'a>(
    state: &AppState,
    user: Option<&'a AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
) -> Result<&'a AuthenticatedUser, AppError> {
    let u = user.ok_or_else(|| {
        AppError::Unauthorized(
            "sign in with editor or owner rights to save API services".to_string(),
        )
    })?;
    if !u.write_access {
        return Err(AppError::Forbidden(
            "this API token lacks write scope".to_string(),
        ));
    }
    if can_write_scope(state, Some(u), scope, owner_id) {
        Ok(u)
    } else {
        Err(AppError::Forbidden(
            "editor or owner rights required for this scope".to_string(),
        ))
    }
}

// ─── Listing helpers ─────────────────────────────────────────────────────────

/// Datasets owned by an org/group scope that the caller can access (a public
/// dataset is accessible to everyone). Empty for the dataset scope.
fn accessible_owned_datasets(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
) -> Vec<Dataset> {
    let want = match scope {
        QueryScope::Organisation => OwnerType::Organisation,
        QueryScope::Group => OwnerType::Group,
        QueryScope::Dataset => return Vec::new(),
    };
    let uid = user.map(|u| u.user_id.as_str());
    state
        .auth_db
        .list_datasets()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.owner_type == want && d.owner_id == owner_id)
        .filter(|d| state.auth_db.can_access_dataset(uid, d).unwrap_or(false))
        .collect()
}

/// Full member (admin / org-or-group member) — sees private services too.
fn is_member_or_admin(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
) -> bool {
    if user.map(|u| u.is_admin()).unwrap_or(false) {
        return true;
    }
    let Some(uid) = user.map(|u| u.user_id.as_str()) else {
        return false;
    };
    match scope {
        QueryScope::Organisation => state
            .auth_db
            .get_org_membership(uid, owner_id)
            .ok()
            .flatten()
            .is_some(),
        QueryScope::Group => state
            .auth_db
            .get_group_membership(uid, owner_id)
            .ok()
            .flatten()
            .is_some(),
        QueryScope::Dataset => true,
    }
}

/// The set of services a scope's API-services page should show. For an org/group
/// this is its own org/group-scoped services (members see all; everyone else sees
/// only `public` ones) PLUS the services of every dataset it owns that the caller
/// can access — so a public dataset's services surface in the owning org's view.
/// Each query keeps its own `scope`/`owner_id`, so callers route per-query.
fn gather_listed_queries(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
) -> Result<Vec<SavedQuery>, AppError> {
    let store = store_of(state);
    match scope {
        QueryScope::Dataset => {
            check_read(state, user, scope, owner_id, None)?;
            store.list(scope, owner_id).map_err(internal)
        }
        QueryScope::Organisation | QueryScope::Group => {
            let member = is_member_or_admin(state, user, scope, owner_id);
            let datasets = accessible_owned_datasets(state, user, scope, owner_id);
            let scoped: Vec<SavedQuery> = store
                .list(scope, owner_id)
                .map_err(internal)?
                .into_iter()
                .filter(|q| member || q.visibility.as_deref() == Some("public"))
                .collect();

            // Nothing public to show and not a member → behave like check_read.
            if !member && datasets.is_empty() && scoped.is_empty() {
                return Err(if user.is_none() {
                    AppError::Unauthorized(
                        "authentication required — provide an API token".to_string(),
                    )
                } else {
                    AppError::NotFound("not found".to_string())
                });
            }

            let mut out = scoped;
            for ds in &datasets {
                out.extend(store.list(QueryScope::Dataset, &ds.id).map_err(internal)?);
            }
            Ok(out)
        }
    }
}

/// Describe where a listed query reads its data from (for the UI tag/grouping).
fn reads_from_of(
    q: &SavedQuery,
    dataset_name: &impl Fn(&str) -> Option<String>,
    union_datasets: &[(String, String)],
) -> Value {
    match q.scope {
        QueryScope::Dataset => json!({
            "kind": "dataset",
            "dataset_id": q.owner_id,
            "dataset_name": dataset_name(&q.owner_id),
        }),
        QueryScope::Organisation | QueryScope::Group => {
            let datasets: Vec<Value> = union_datasets
                .iter()
                .map(|(id, name)| json!({ "id": id, "name": name }))
                .collect();
            json!({ "kind": q.scope.as_str(), "datasets": datasets })
        }
    }
}

// ─── Scope-generic core handlers ─────────────────────────────────────────────

async fn list_core(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
) -> Result<Json<Value>, AppError> {
    let queries = gather_listed_queries(state, user, scope, owner_id)?;

    // Dataset-name lookup + the union of datasets an org/group query reads.
    let all_datasets = state.auth_db.list_datasets().map_err(internal)?;
    let name_of = |id: &str| {
        all_datasets
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.name.clone())
    };
    let union_datasets: Vec<(String, String)> = if matches!(scope, QueryScope::Dataset) {
        Vec::new()
    } else {
        accessible_owned_datasets(state, user, scope, owner_id)
            .into_iter()
            .map(|d| (d.id, d.name))
            .collect()
    };

    // Serialize each query and attach its source descriptor + per-query write flag
    // (an org page may list a dataset service the caller cannot edit).
    let mut out: Vec<Value> = Vec::with_capacity(queries.len());
    for q in &queries {
        let mut v = serde_json::to_value(q).map_err(internal)?;
        if let Value::Object(ref mut m) = v {
            m.insert(
                "reads_from".to_string(),
                reads_from_of(q, &name_of, &union_datasets),
            );
            m.insert(
                "can_write".to_string(),
                json!(can_write_scope(state, user, q.scope, &q.owner_id)),
            );
        }
        out.push(v);
    }

    let can_write = can_write_scope(state, user, scope, owner_id);
    Ok(Json(json!({ "queries": out, "can_write": can_write })))
}

async fn create_core(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
    req: CreateSavedQueryRequest,
) -> Result<Json<SavedQuery>, AppError> {
    let u = check_write(state, user, scope, owner_id)?;
    if req.name.trim().is_empty() || req.sparql.trim().is_empty() {
        return Err(AppError::BadRequest(
            "name and sparql are required".to_string(),
        ));
    }
    params::declared_check(&req.sparql, &req.parameters)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let sq = store_of(state)
        .create(scope, owner_id, &req, &u.user_id)
        .map_err(internal)?;
    // Project immutable linked-data metadata (service node + revision 1).
    metadata::record_service(&state.store, &state.base_url, &sq);
    metadata::record_revision(
        &state.store,
        &state.base_url,
        &sq.id,
        sq.current_revision,
        req.version_name.as_deref(),
        req.note.as_deref(),
        sq.sparql.as_deref().unwrap_or(&req.sparql),
        "manual",
        &sq.created_by,
        &sq.created_at,
    );
    Ok(Json(sq))
}

async fn get_core(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
    slug: &str,
) -> Result<Json<SavedQuery>, AppError> {
    let sq = store_of(state)
        .get_by_slug(scope, owner_id, slug)
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound("saved query not found".to_string()))?;
    check_read(state, user, scope, owner_id, sq.visibility.as_deref())?;
    Ok(Json(sq))
}

async fn update_core(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
    slug: &str,
    req: UpdateSavedQueryRequest,
) -> Result<Json<SavedQuery>, AppError> {
    let u = check_write(state, user, scope, owner_id)?;
    let store = store_of(state);
    let sq = store
        .get_by_slug(scope, owner_id, slug)
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound("saved query not found".to_string()))?;
    if let Some(ref s) = req.sparql {
        let specs = req
            .parameters
            .clone()
            .unwrap_or_else(|| sq.parameters.clone());
        params::declared_check(s, &specs).map_err(|e| AppError::BadRequest(e.to_string()))?;
    }
    let updated = store
        .update(&sq.id, &req, &u.user_id)
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound("saved query not found".to_string()))?;
    // A new SPARQL body created a new revision → record it immutably.
    if req.sparql.is_some() {
        metadata::record_revision(
            &state.store,
            &state.base_url,
            &updated.id,
            updated.current_revision,
            req.version_name.as_deref(),
            req.note.as_deref(),
            updated.sparql.as_deref().unwrap_or_default(),
            "manual",
            &u.user_id,
            &updated.updated_at,
        );
    }
    Ok(Json(updated))
}

async fn delete_core(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
    slug: &str,
) -> Result<Json<Value>, AppError> {
    check_write(state, user, scope, owner_id)?;
    let store = store_of(state);
    let sq = store
        .get_by_slug(scope, owner_id, slug)
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound("saved query not found".to_string()))?;
    store.delete(&sq.id).map_err(internal)?;
    // The operational row is gone, but the linked-data metadata is immutable —
    // keep the record and append a retirement marker.
    metadata::record_retired(
        &state.store,
        &state.base_url,
        &sq.id,
        &chrono::Utc::now().to_rfc3339(),
    );
    Ok(Json(json!({ "deleted": true, "id": sq.id })))
}

async fn revisions_core(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
    slug: &str,
) -> Result<Json<Value>, AppError> {
    let store = store_of(state);
    let sq = store
        .get_by_slug(scope, owner_id, slug)
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound("saved query not found".to_string()))?;
    check_read(state, user, scope, owner_id, sq.visibility.as_deref())?;
    let revisions = store.list_revisions(&sq.id).map_err(internal)?;
    Ok(Json(json!({ "revisions": revisions })))
}

async fn tests_core(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
    slug: &str,
) -> Result<Json<Value>, AppError> {
    let store = store_of(state);
    let sq = store
        .get_by_slug(scope, owner_id, slug)
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound("saved query not found".to_string()))?;
    check_read(state, user, scope, owner_id, sq.visibility.as_deref())?;
    let tests = store.list_tests(&sq.id).map_err(internal)?;
    Ok(Json(json!({ "tests": tests })))
}

async fn ack_core(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
    slug: &str,
    test_id: &str,
) -> Result<Json<Value>, AppError> {
    let u = check_write(state, user, scope, owner_id)?;
    let store = store_of(state);
    let sq = store
        .get_by_slug(scope, owner_id, slug)
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound("saved query not found".to_string()))?;
    let t = store
        .get_test(test_id)
        .map_err(internal)?
        .filter(|t| t.query_id == sq.id)
        .ok_or_else(|| AppError::NotFound("test not found".to_string()))?;
    store
        .acknowledge_test(&t.id, &u.user_id)
        .map_err(internal)?;
    Ok(Json(json!({ "acknowledged": true, "id": t.id })))
}

#[derive(Debug, Default, Deserialize)]
pub struct RepairBody {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    schema_hint: Option<String>,
    #[serde(default)]
    model: Option<String>,
    /// Override the text to repair (defaults to the query's head revision).
    #[serde(default)]
    sparql: Option<String>,
    /// Persist the repaired query as a new revision.
    #[serde(default)]
    save: bool,
}

async fn repair_core(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
    slug: &str,
    body: RepairBody,
) -> Result<Json<Value>, AppError> {
    let u = check_write(state, user, scope, owner_id)?;
    let store = store_of(state);
    let sq = store
        .get_by_slug(scope, owner_id, slug)
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound("saved query not found".to_string()))?;
    let broken = body
        .sparql
        .or_else(|| sq.sparql.clone())
        .unwrap_or_default();
    let error = body
        .error
        .or_else(|| {
            store.list_tests(&sq.id).ok().and_then(|ts| {
                ts.into_iter()
                    .find(|t| t.status == "error")
                    .and_then(|t| t.error_message)
            })
        })
        .unwrap_or_else(|| "the query no longer returns the expected results".to_string());
    let res = llm::repair_query(
        &broken,
        &error,
        body.schema_hint.as_deref(),
        body.model.as_deref(),
    )
    .await?;
    let mut saved_revision = None;
    if body.save {
        let rev = store
            .add_revision(
                &sq.id,
                &res.sparql,
                Some("LLM repair"),
                None,
                "llm_repair",
                &u.user_id,
            )
            .map_err(internal)?;
        metadata::record_revision(
            &state.store,
            &state.base_url,
            &sq.id,
            rev,
            Some("LLM repair"),
            None,
            &res.sparql,
            "llm_repair",
            &u.user_id,
            &chrono::Utc::now().to_rfc3339(),
        );
        saved_revision = Some(rev);
    }
    Ok(Json(json!({
        "sparql": res.sparql,
        "model": res.model,
        "savedRevision": saved_revision,
    })))
}

async fn openapi_core(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
) -> Result<Json<Value>, AppError> {
    let queries = gather_listed_queries(state, user, scope, owner_id)?;
    let spec = openapi::build_spec(state, scope, owner_id, &queries);
    Ok(Json(spec))
}

/// The shared run path: load the query, check access, prepare (inject params +
/// resolve version graphs), execute through the scoped streaming executor.
#[allow(clippy::too_many_arguments)] // cohesive run inputs; a struct adds churn
async fn run_core(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    scope: QueryScope,
    owner_id: &str,
    slug: &str,
    version: Option<String>,
    provided: HashMap<String, String>,
    accept: &str,
) -> Result<Response, AppError> {
    let sq_store = store_of(state);
    let sq = sq_store
        .get_by_slug(scope, owner_id, slug)
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound("saved query not found".to_string()))?;
    check_read(state, user, scope, owner_id, sq.visibility.as_deref())?;
    if !sq.is_active {
        return Err(AppError::NotFound("saved query is inactive".to_string()));
    }

    let req = VersionRequest::parse(version.as_deref());
    let uid = user.map(|u| u.user_id.as_str());
    let prep = exec::prepare_run(state, &sq_store, &sq, uid, &provided, &req)?;
    let mut result = routes::run_scoped_sparql(state, &prep.query, &prep.graphs, accept).await;

    // Tell the caller which dataset version actually served the response.
    if let Ok(ref mut resp) = result {
        let label = prep.version_label.clone().unwrap_or_else(|| {
            if prep.is_live {
                "latest".to_string()
            } else {
                "none".to_string()
            }
        });
        if let Ok(hv) = axum::http::HeaderValue::from_str(&label) {
            resp.headers_mut().insert("x-ots-dataset-version", hv);
        }
    }

    // A broken `?version=latest` is surfaced to the caller AND reported to the
    // owner (deduped against an already-open report). Only authenticated callers can
    // trigger the owner notification, so an anonymous caller of a public query can't
    // drive owner-directed emails by forcing `?version=latest` to error.
    if req.is_latest() && user.is_some() {
        if let Err(ref e) = result {
            report_latest_break(state, &sq_store, &sq, owner_id, &e.message()).await;
        }
    }
    result
}

async fn report_latest_break(
    state: &AppState,
    sq_store: &SavedQueryStore,
    sq: &SavedQuery,
    owner_id: &str,
    message: &str,
) {
    let already_open = sq_store
        .latest_test_for_version(&sq.id, "latest")
        .ok()
        .flatten()
        .map(|t| t.status == "error" && !t.acknowledged)
        .unwrap_or(false);

    let test = QueryTest {
        id: Uuid::new_v4().to_string(),
        query_id: sq.id.clone(),
        revision: sq.current_revision,
        dataset_id: owner_id.to_string(),
        dataset_version: "latest".to_string(),
        prev_version: None,
        status: "error".to_string(),
        result_hash: None,
        result_rowcount: None,
        error_message: Some(message.to_string()),
        acknowledged: false,
        acknowledged_by: None,
        acknowledged_at: None,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let _ = sq_store.insert_test(&test);

    if !already_open {
        let dataset_id = match sq.scope {
            QueryScope::Dataset => Some(owner_id),
            _ => None,
        };
        notify::notify_query_broken(state, sq, dataset_id, "latest", message).await;
    }
}

// ─── Request bodies / helpers for run ────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub struct RunBody {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    parameters: HashMap<String, String>,
}

fn accept_of(headers: &HeaderMap) -> String {
    headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/sparql-results+json")
        .to_string()
}

/// Map a `?format=` value to a concrete media type (alternative to the Accept
/// header, handy for shareable links and the docs' return-format examples).
fn format_to_accept(fmt: &str) -> Option<&'static str> {
    match fmt.trim().to_ascii_lowercase().as_str() {
        "json" | "sparql-json" => Some("application/sparql-results+json"),
        "xml" | "sparql-xml" => Some("application/sparql-results+xml"),
        "csv" => Some("text/csv"),
        "tsv" => Some("text/tab-separated-values"),
        "turtle" | "ttl" => Some("text/turtle"),
        "ntriples" | "nt" | "n-triples" => Some("application/n-triples"),
        "nquads" | "nq" | "n-quads" => Some("application/n-quads"),
        "trig" => Some("application/trig"),
        "jsonld" | "json-ld" | "ld+json" => Some("application/ld+json"),
        "rdfxml" | "rdf-xml" | "rdf+xml" => Some("application/rdf+xml"),
        _ => None,
    }
}

/// Pull the reserved `version` and `format` keys out of a flat query-string map.
/// The remaining entries are the query's own parameters.
fn split_reserved(
    mut q: HashMap<String, String>,
) -> (Option<String>, Option<String>, HashMap<String, String>) {
    let version = q.remove("version");
    let format = q.remove("format");
    (version, format, q)
}

// ─── Per-scope wrappers ──────────────────────────────────────────────────────
//
// Each forwards the matched scope + owner id to a `*_core` function above.

macro_rules! scope_handlers {
    ($modname:ident, $scope:expr) => {
        pub mod $modname {
            use super::*;

            pub async fn list(
                State(state): State<AppState>,
                user: OptUser,
                Path(owner): Path<String>,
            ) -> Result<Json<Value>, AppError> {
                list_core(&state, user.as_deref(), $scope, &owner).await
            }

            pub async fn create(
                State(state): State<AppState>,
                user: OptUser,
                Path(owner): Path<String>,
                Json(req): Json<CreateSavedQueryRequest>,
            ) -> Result<Json<SavedQuery>, AppError> {
                create_core(&state, user.as_deref(), $scope, &owner, req).await
            }

            pub async fn get(
                State(state): State<AppState>,
                user: OptUser,
                Path((owner, slug)): Path<(String, String)>,
            ) -> Result<Json<SavedQuery>, AppError> {
                get_core(&state, user.as_deref(), $scope, &owner, &slug).await
            }

            pub async fn update(
                State(state): State<AppState>,
                user: OptUser,
                Path((owner, slug)): Path<(String, String)>,
                Json(req): Json<UpdateSavedQueryRequest>,
            ) -> Result<Json<SavedQuery>, AppError> {
                update_core(&state, user.as_deref(), $scope, &owner, &slug, req).await
            }

            pub async fn delete(
                State(state): State<AppState>,
                user: OptUser,
                Path((owner, slug)): Path<(String, String)>,
            ) -> Result<Json<Value>, AppError> {
                delete_core(&state, user.as_deref(), $scope, &owner, &slug).await
            }

            pub async fn revisions(
                State(state): State<AppState>,
                user: OptUser,
                Path((owner, slug)): Path<(String, String)>,
            ) -> Result<Json<Value>, AppError> {
                revisions_core(&state, user.as_deref(), $scope, &owner, &slug).await
            }

            pub async fn tests(
                State(state): State<AppState>,
                user: OptUser,
                Path((owner, slug)): Path<(String, String)>,
            ) -> Result<Json<Value>, AppError> {
                tests_core(&state, user.as_deref(), $scope, &owner, &slug).await
            }

            pub async fn ack(
                State(state): State<AppState>,
                user: OptUser,
                Path((owner, slug, test_id)): Path<(String, String, String)>,
            ) -> Result<Json<Value>, AppError> {
                ack_core(&state, user.as_deref(), $scope, &owner, &slug, &test_id).await
            }

            pub async fn repair(
                State(state): State<AppState>,
                user: OptUser,
                Path((owner, slug)): Path<(String, String)>,
                Json(body): Json<RepairBody>,
            ) -> Result<Json<Value>, AppError> {
                repair_core(&state, user.as_deref(), $scope, &owner, &slug, body).await
            }

            pub async fn run_get(
                State(state): State<AppState>,
                user: OptUser,
                Path((owner, slug)): Path<(String, String)>,
                Query(qs): Query<HashMap<String, String>>,
                headers: HeaderMap,
            ) -> Result<Response, AppError> {
                let (version, format, provided) = split_reserved(qs);
                let accept = format
                    .as_deref()
                    .and_then(format_to_accept)
                    .map(str::to_string)
                    .unwrap_or_else(|| accept_of(&headers));
                run_core(
                    &state,
                    user.as_deref(),
                    $scope,
                    &owner,
                    &slug,
                    version,
                    provided,
                    &accept,
                )
                .await
            }

            pub async fn run_post(
                State(state): State<AppState>,
                user: OptUser,
                Path((owner, slug)): Path<(String, String)>,
                Query(qs): Query<HashMap<String, String>>,
                headers: HeaderMap,
                bytes: Bytes,
            ) -> Result<Response, AppError> {
                let (qs_version, format, mut provided) = split_reserved(qs);
                let accept = format
                    .as_deref()
                    .and_then(format_to_accept)
                    .map(str::to_string)
                    .unwrap_or_else(|| accept_of(&headers));
                let body: RunBody = if bytes.is_empty() {
                    RunBody::default()
                } else {
                    serde_json::from_slice(&bytes)
                        .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))?
                };
                for (k, v) in body.parameters {
                    provided.insert(k, v);
                }
                let version = body.version.or(qs_version);
                run_core(
                    &state,
                    user.as_deref(),
                    $scope,
                    &owner,
                    &slug,
                    version,
                    provided,
                    &accept,
                )
                .await
            }

            pub async fn openapi(
                State(state): State<AppState>,
                user: OptUser,
                Path(owner): Path<String>,
            ) -> Result<Json<Value>, AppError> {
                openapi_core(&state, user.as_deref(), $scope, &owner).await
            }
        }
    };
}

scope_handlers!(dataset, QueryScope::Dataset);
scope_handlers!(org, QueryScope::Organisation);
scope_handlers!(group, QueryScope::Group);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::models::{OwnerType, SystemRole, Visibility};
    use crate::store::TripleStore;

    fn principal(id: &str, write: bool) -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: id.to_string(),
            role: SystemRole::User,
            can_publish: false,
            write_access: write,
        }
    }

    fn state_with_dataset(id: &str, vis: Visibility) -> AppState {
        let state = AppState::test_default_with_store(TripleStore::in_memory().unwrap());
        state
            .auth_db
            .create_dataset(id, "DS", None, OwnerType::User, "owner1", vis, None)
            .unwrap();
        state
    }

    #[test]
    fn anonymous_can_read_public_dataset_queries() {
        let state = state_with_dataset("pub", Visibility::Public);
        assert!(check_read(&state, None, QueryScope::Dataset, "pub", None).is_ok());
    }

    #[test]
    fn anonymous_cannot_read_private_dataset_queries() {
        let state = state_with_dataset("priv", Visibility::Private);
        assert!(matches!(
            check_read(&state, None, QueryScope::Dataset, "priv", None),
            Err(AppError::Unauthorized(_))
        ));
    }

    #[test]
    fn writing_requires_auth_editor_or_owner_and_write_scope() {
        let state = state_with_dataset("pub", Visibility::Public);
        // anonymous → unauthorized (must sign in)
        assert!(matches!(
            check_write(&state, None, QueryScope::Dataset, "pub"),
            Err(AppError::Unauthorized(_))
        ));
        // owner with a write-scoped credential → allowed
        let owner = principal("owner1", true);
        assert!(check_write(&state, Some(&owner), QueryScope::Dataset, "pub").is_ok());
        // a signed-in stranger (only public viewer) → forbidden
        let stranger = principal("u2", true);
        assert!(matches!(
            check_write(&state, Some(&stranger), QueryScope::Dataset, "pub"),
            Err(AppError::Forbidden(_))
        ));
        // even the owner, with a read-only API token → forbidden
        let owner_readonly = principal("owner1", false);
        assert!(matches!(
            check_write(&state, Some(&owner_readonly), QueryScope::Dataset, "pub"),
            Err(AppError::Forbidden(_))
        ));
    }
}
