//! HTTP handlers for `/api/sources`, `/api/mappings` and `/api/runs`.
//!
//! Every route here is admin-gated (see [`routes`](super::routes)): a
//! datasource carries a pointer to a production credential, and its runs write
//! instance data.

use axum::extract::{Path, Query, State};
use axum::http::{header::CONTENT_TYPE, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::json;

use crate::auth::middleware::AuthenticatedUser;
use crate::secrets;
use crate::server::AppState;

use super::model::*;
use super::runs::{RunContext, RunError};
use super::{mappings, registry, runs, ValidationError};

type ApiResult<T> = Result<T, (StatusCode, String)>;

fn bad(message: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, message.to_string())
}
fn not_found(kind: &str, id: &str) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, format!("{kind} '{id}' not found"))
}
fn internal(message: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, message.to_string())
}

/// The actor IRI recorded as `prov:wasAttributedTo` / `prov:wasAssociatedWith`.
fn actor_iri(state: &AppState, user: &AuthenticatedUser) -> String {
    format!(
        "{}/users/{}",
        state.base_url.trim_end_matches('/'),
        user.user_id
    )
}

fn ctx<'a>(state: &'a AppState) -> RunContext<'a> {
    RunContext {
        store: &state.store,
        auth_db: &state.auth_db,
        base_url: &state.base_url,
    }
}

// ───────────────────────────── Datasources ─────────────────────────────

/// Build a validated [`SqlSource`] from a request body.
fn to_source(
    state: &AppState,
    body: SourceRequest,
    id: String,
    existing: Option<&SqlSource>,
    actor: &str,
) -> ApiResult<SqlSource> {
    let credential = super::parse_credential(body.credential.as_deref()).map_err(bad)?;
    // An update that omits the credential keeps the one already registered,
    // so re-saving a source from the UI does not need the reference re-typed.
    let credential = credential.or_else(|| {
        body.credential
            .is_none()
            .then(|| existing.and_then(|e| e.credential.clone()))
            .flatten()
    });

    let statement_timeout_ms = match body.statement_timeout_ms {
        Some(ms) => ms,
        None if secrets::posture().is_production() => {
            return Err(bad(ValidationError::MissingTimeout))
        }
        None => existing
            .map(|e| e.statement_timeout_ms)
            .unwrap_or(super::DEFAULT_TIMEOUT_MS),
    };

    if let Some(dataset_id) = &body.dataset {
        let known = state
            .auth_db
            .get_dataset(dataset_id)
            .map_err(internal)?
            .is_some();
        if !known {
            return Err(bad(ValidationError::UnknownDataset(dataset_id.clone())));
        }
    }

    let now = registry::now();
    let source = SqlSource {
        name: body.name.clone().unwrap_or_else(|| id.clone()),
        dialect: body.dialect.trim().to_ascii_lowercase(),
        host: body.host.clone().filter(|h| !h.trim().is_empty()),
        port: body.port,
        database: body.database.clone(),
        username: body.username.clone().filter(|u| !u.trim().is_empty()),
        credential,
        read_only: body.read_only,
        statement_timeout_ms,
        watermark_column: body
            .watermark_column
            .clone()
            .filter(|w| !w.trim().is_empty()),
        allow_model_assist: body.allow_model_assist,
        tls: body.tls,
        options: body.options.clone(),
        dataset: body.dataset.clone(),
        owner: existing.and_then(|e| e.owner.clone()),
        created_by: existing
            .and_then(|e| e.created_by.clone())
            .or_else(|| Some(actor.to_string())),
        created_at: existing
            .map(|e| e.created_at.clone())
            .unwrap_or_else(|| now.clone()),
        updated_at: now,
        production: existing.and_then(|e| e.production.clone()),
        previous: existing.and_then(|e| e.previous.clone()),
        id,
    };
    super::validate_source(&source).map_err(bad)?;
    Ok(source)
}

/// Confirm the source's credential resolves, off the async runtime: a
/// `vault:` reference is an HTTP round trip, and blocking a worker on it
/// starves everything else the runtime is driving.
async fn check_resolvable(source: &SqlSource) -> ApiResult<()> {
    let probe = source.clone();
    tokio::task::spawn_blocking(move || super::resolves(&probe))
        .await
        .map_err(internal)?
        .map_err(bad)
}

/// `GET /api/sources`
pub async fn list_sources(State(state): State<AppState>) -> Json<Vec<SourceResponse>> {
    Json(
        registry::list_sources(&state.store)
            .iter()
            .map(SourceResponse::from)
            .collect(),
    )
}

/// `POST /api/sources`
pub async fn create_source(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<SourceRequest>,
) -> ApiResult<Response> {
    let id = body
        .id
        .clone()
        .or_else(|| body.name.clone())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| bad("a datasource needs an id"))?;
    if registry::get_source(&state.store, &id).is_some() {
        return Err((
            StatusCode::CONFLICT,
            format!("datasource '{id}' already exists; update it with PUT"),
        ));
    }
    let actor = actor_iri(&state, &user);
    let source = to_source(&state, body, id, None, &actor)?;
    check_resolvable(&source).await?;
    registry::put_source(&state.store, &source).map_err(internal)?;
    audit(&state, &user, &source, "registered");
    Ok((StatusCode::CREATED, Json(SourceResponse::from(&source))).into_response())
}

/// `GET /api/sources/:id`
pub async fn get_source(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SourceResponse>> {
    registry::get_source(&state.store, &id)
        .map(|s| Json(SourceResponse::from(&s)))
        .ok_or_else(|| not_found("datasource", &id))
}

/// `PUT /api/sources/:id`
pub async fn update_source(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SourceRequest>,
) -> ApiResult<Json<SourceResponse>> {
    let existing =
        registry::get_source(&state.store, &id).ok_or_else(|| not_found("datasource", &id))?;
    let actor = actor_iri(&state, &user);
    let source = to_source(&state, body, id, Some(&existing), &actor)?;
    check_resolvable(&source).await?;
    registry::put_source(&state.store, &source).map_err(internal)?;
    // An update is the moment an operator re-points a credential, so drop the
    // resolution cache: the next run reads the secret store, not a value
    // cached before the rotation.
    secrets::clear_cache();
    audit(&state, &user, &source, "updated");
    Ok(Json(SourceResponse::from(&source)))
}

/// `DELETE /api/sources/:id`
pub async fn delete_source(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let source =
        registry::get_source(&state.store, &id).ok_or_else(|| not_found("datasource", &id))?;
    let mappings = registry::list_mappings(&state.store, Some(&id));
    if !mappings.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "datasource '{id}' still has {} mapping(s); delete them first",
                mappings.len()
            ),
        ));
    }
    // Before the record itself, so a failure here leaves the datasource
    // registered rather than orphaning its profiles: a profile left behind
    // outlives the database it describes, and re-registering the id would
    // continue its version sequence and serve the previous database's
    // code-list values as this datasource's history.
    super::profile::delete_profiles(&state.store, &id).map_err(internal)?;
    // Its re-map tickets go the same way: a ticket about a datasource that no
    // longer exists is noise for whoever registers the id next.
    super::drift::delete_tickets(&state.store, &id).map_err(internal)?;
    registry::delete_source(&state.store, &id).map_err(internal)?;
    audit(&state, &user, &source, "deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/sources/test` — open a connection and throw it away.
///
/// Answers 200 with `{"ok": false, "error": …}` for a reachable-but-failing
/// datasource: whether the database answers is the *result* of the check, not
/// a fault in the request. A malformed registration is still a 400.
pub async fn test_source(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<SourceRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let id = body.id.clone().unwrap_or_else(|| "probe".to_string());
    let actor = actor_iri(&state, &user);
    let source = to_source(&state, body, id, None, &actor)?;

    // Resolution and the connection both happen on the blocking pool.
    let probe = tokio::task::spawn_blocking(move || {
        super::resolves(&source).map_err(|e| e.to_string())?;
        let mut conn = runs::connect(&source)?;
        let version = conn.server_version().ok().flatten();
        let tables = conn
            .introspect()
            .map_err(|e| runs::scrub(&e.to_string(), &source, None))?;
        Ok::<_, String>((version, tables.len()))
    })
    .await
    .map_err(internal)?;

    Ok(Json(match probe {
        Ok((version, tables)) => json!({
            "ok": true,
            "serverVersion": version,
            "tables": tables,
            "allowlisted": true,
        }),
        Err(error) => json!({ "ok": false, "error": error }),
    }))
}

/// `GET /api/sources/:id/introspect`
pub async fn introspect_source(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let source =
        registry::get_source(&state.store, &id).ok_or_else(|| not_found("datasource", &id))?;
    let tables = tokio::task::spawn_blocking(move || {
        let mut conn = runs::connect(&source)?;
        conn.introspect()
            .map_err(|e| runs::scrub(&e.to_string(), &source, None))
    })
    .await
    .map_err(internal)?
    .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;

    Ok(Json(json!({ "tables": tables })))
}

#[derive(Debug, Deserialize)]
pub struct PreviewParams {
    pub table: String,
    pub limit: Option<usize>,
}

/// `GET /api/sources/:id/preview` — the first rows of a table, unmapped.
///
/// This is pre-clean source data, so it is admin-only like the rest of the
/// registry and never cached.
pub async fn preview_source(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<PreviewParams>,
) -> ApiResult<Json<serde_json::Value>> {
    let source =
        registry::get_source(&state.store, &id).ok_or_else(|| not_found("datasource", &id))?;
    let limit = params.limit.unwrap_or(20).clamp(1, 1_000);
    let table = params.table.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let mut conn = runs::connect(&source)?;
        conn.sample(&table, limit)
            .map_err(|e| runs::scrub(&e.to_string(), &source, None))
    })
    .await
    .map_err(internal)?
    .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;

    Ok(Json(
        json!({ "table": params.table, "limit": limit, "rows": rows }),
    ))
}

/// `GET /api/sources/:id/provenance` — the datasource's PROV-O trail as Turtle.
pub async fn source_provenance(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    if registry::get_source(&state.store, &id).is_none() {
        return Err(not_found("datasource", &id));
    }
    let turtle = runs::source_provenance_turtle(&state.store, &id);
    Ok((StatusCode::OK, [(CONTENT_TYPE, "text/turtle")], turtle).into_response())
}

/// `GET /api/sources/metrics`
pub async fn source_metrics(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(runs::metrics(&state.store))
}

fn audit(state: &AppState, user: &AuthenticatedUser, source: &SqlSource, what: &str) {
    crate::commit_log::record(
        &state.store,
        &state.base_url,
        crate::commit_log::CommitKind::Source,
        format!("datasource '{}' {what}", source.id),
        Some(&user.user_id),
        Some(source.iri()),
        vec![SOURCES_GRAPH.to_string()],
        0,
        0,
        None,
    );
}

// ────────────────────────────── Mappings ──────────────────────────────

fn mapping_response(state: &AppState, m: &MappingRecord) -> MappingResponse {
    // A version whose graph will not parse still lists, with zero structure,
    // rather than taking the whole listing down.
    let rml = mappings::load(&state.store, &m.id, m.version).ok();
    MappingResponse {
        iri: m.iri(),
        version_iri: m.version_iri(),
        id: m.id.clone(),
        title: m.title.clone(),
        source: source_iri(&m.source_id),
        version: m.version,
        state: m.state.as_str().to_string(),
        shapes_graph: m.shapes_graph.clone(),
        model: m.model.clone(),
        model_version: m.model_version.clone(),
        profile_version: m.profile_version,
        triples_maps: rml.as_ref().map(|r| r.triples_maps.len()).unwrap_or(0),
        joins: rml
            .as_ref()
            .map(|r| {
                mappings::join_parents(r)
                    .into_iter()
                    .map(|(child, parent)| MappingJoin { child, parent })
                    .collect()
            })
            .unwrap_or_default(),
        created_at: m.created_at.clone(),
        updated_at: m.updated_at.clone(),
    }
}

#[derive(Debug, Deserialize)]
pub struct MappingListParams {
    pub source: Option<String>,
}

/// `GET /api/mappings` — `?source=` takes the datasource id or its IRI.
pub async fn list_mappings(
    State(state): State<AppState>,
    Query(params): Query<MappingListParams>,
) -> Json<Vec<MappingResponse>> {
    // The Studio sends the IRI (`urn:source:<id>`), a script the bare id;
    // the registry filters by id, so either spelling has to reach it as one.
    let source = params
        .source
        .as_deref()
        .map(|s| s.trim().trim_start_matches("urn:source:").to_string())
        .filter(|s| !s.is_empty());
    Json(
        registry::list_mappings(&state.store, source.as_deref())
            .iter()
            .map(|m| mapping_response(&state, m))
            .collect(),
    )
}

/// The RML a request supplies, in whichever authoring form.
///
/// YARRRML is translated here and only RML is stored, so the store has one
/// mapping representation to version, diff, gate and execute. `source_hint` is
/// the datasource the mapping is being registered against, which a YARRRML
/// document may leave implicit.
fn rml_of(body: &MappingRequest, source_hint: Option<&str>) -> ApiResult<String> {
    let yarrrml = body
        .yarrrml
        .as_deref()
        .map(str::trim)
        .filter(|y| !y.is_empty());
    let rml = body.rml.as_deref().map(str::trim).filter(|r| !r.is_empty());
    match (yarrrml, rml) {
        (Some(_), Some(_)) => Err(bad(mappings::MappingError::BothForms)),
        (Some(y), None) => super::yarrrml::to_rml(y, source_hint)
            .map_err(|e| bad(mappings::MappingError::Yarrrml(e.to_string()))),
        (None, Some(r)) => Ok(r.to_string()),
        (None, None) => Err(bad(
            "a mapping needs its RML as Turtle in 'rml', or YARRRML in 'yarrrml'",
        )),
    }
}

/// `POST /api/mappings`
pub async fn create_mapping(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<MappingRequest>,
) -> ApiResult<Response> {
    let id = body
        .id
        .clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("a mapping needs an id"))?;
    if !valid_id(&id) {
        return Err(bad(ValidationError::Id(id)));
    }
    if registry::get_mapping(&state.store, &id).is_some() {
        return Err((
            StatusCode::CONFLICT,
            format!("mapping '{id}' already exists; update it with PUT"),
        ));
    }
    let source_hint = body
        .source
        .as_deref()
        .map(|s| s.trim_start_matches("urn:source:"));
    let rml = rml_of(&body, source_hint)?;
    let parsed =
        crate::rml::parse_rml(&rml).map_err(|e| bad(mappings::MappingError::Invalid(e)))?;
    // The RML already names the datasource it reads, so `source` is optional:
    // supplying it only pins what the mapping says, and a disagreement is an
    // error rather than a silent re-binding.
    let source_id = body
        .source
        .clone()
        .map(|s| s.trim_start_matches("urn:source:").to_string())
        .or_else(|| {
            parsed
                .datasources()
                .first()
                .map(|iri| iri.trim_start_matches("urn:source:").to_string())
        })
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad(mappings::MappingError::NoSource))?;
    let source = registry::get_source(&state.store, &source_id)
        .ok_or_else(|| not_found("datasource", &source_id))?;
    mappings::check(&parsed, &source.id).map_err(bad)?;

    let now = registry::now();
    let record = MappingRecord {
        title: body.title.clone().unwrap_or_else(|| id.clone()),
        source_id: source.id.clone(),
        version: 1,
        state: body
            .state
            .as_deref()
            .and_then(MappingState::parse)
            .unwrap_or(MappingState::Draft),
        shapes_graph: body.shapes_graph.clone().filter(|s| !s.trim().is_empty()),
        model: body.model.clone(),
        model_version: body.model_version.clone(),
        // The profile the mapping was written against: what a drift check
        // compares the newest profile with.
        profile_version: super::profile::latest_version(&state.store, &source.id),
        created_by: Some(actor_iri(&state, &user)),
        created_at: now.clone(),
        updated_at: now,
        id,
    };
    mappings::store_version(&state.store, &record.id, 1, &rml).map_err(internal)?;
    registry::put_mapping(&state.store, &record).map_err(internal)?;
    commit_mapping(&state, &user, &record, "registered");
    Ok((StatusCode::CREATED, Json(mapping_response(&state, &record))).into_response())
}

/// `GET /api/mappings/:id`
pub async fn get_mapping(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<MappingResponse>> {
    registry::get_mapping(&state.store, &id)
        .map(|m| Json(mapping_response(&state, &m)))
        .ok_or_else(|| not_found("mapping", &id))
}

/// `PUT /api/mappings/:id` — freeze a new version.
///
/// A version is immutable once written: runs point at it by IRI, so editing
/// one in place would rewrite history. Every save mints the next version.
pub async fn update_mapping(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<MappingRequest>,
) -> ApiResult<Json<MappingResponse>> {
    let existing =
        registry::get_mapping(&state.store, &id).ok_or_else(|| not_found("mapping", &id))?;
    let source_id = body
        .source
        .clone()
        .map(|s| s.trim_start_matches("urn:source:").to_string())
        .unwrap_or_else(|| existing.source_id.clone());

    let mut record = MappingRecord {
        title: body.title.clone().unwrap_or_else(|| existing.title.clone()),
        source_id,
        state: body
            .state
            .as_deref()
            .and_then(MappingState::parse)
            .unwrap_or(existing.state),
        shapes_graph: body
            .shapes_graph
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| existing.shapes_graph.clone()),
        model: body.model.clone().or_else(|| existing.model.clone()),
        model_version: body
            .model_version
            .clone()
            .or_else(|| existing.model_version.clone()),
        updated_at: registry::now(),
        ..existing.clone()
    };

    // Only new RML mints a version; a metadata-only edit keeps the current one.
    let new_rml = body.rml.is_some() || body.yarrrml.is_some();
    if new_rml {
        let rml = rml_of(&body, Some(record.source_id.as_str()))?;
        mappings::validate_rml(&rml, &record.source_id).map_err(bad)?;
        record.version = existing.version + 1;
        mappings::store_version(&state.store, &record.id, record.version, &rml)
            .map_err(internal)?;
    }
    // New RML, or an approval, was written against the profile of the moment:
    // that becomes the drift baseline. A metadata edit keeps the old one.
    let approved_now =
        record.state == MappingState::Approved && existing.state != MappingState::Approved;
    if new_rml || approved_now {
        record.profile_version = super::profile::latest_version(&state.store, &record.source_id)
            .or(record.profile_version);
    }
    registry::put_mapping(&state.store, &record).map_err(internal)?;
    commit_mapping(&state, &user, &record, "updated");
    Ok(Json(mapping_response(&state, &record)))
}

/// `DELETE /api/mappings/:id`
pub async fn delete_mapping(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let mapping =
        registry::get_mapping(&state.store, &id).ok_or_else(|| not_found("mapping", &id))?;
    let used_by: Vec<String> = registry::list_runs(&state.store, None)
        .into_iter()
        .filter(|r| r.mapping_id == mapping.id)
        .map(|r| r.id)
        .collect();
    if !used_by.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "mapping '{id}' is used by {} run(s); deleting it would leave their provenance \
                 pointing at nothing — delete the runs first",
                used_by.len()
            ),
        ));
    }
    registry::delete_mapping(&state.store, &mapping).map_err(internal)?;
    commit_mapping(&state, &user, &mapping, "deleted");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct VersionParam {
    pub version: Option<u32>,
}

/// `GET /api/mappings/:id/rml` — a version's RML as Turtle.
pub async fn get_mapping_rml(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<VersionParam>,
) -> ApiResult<Response> {
    let mapping =
        registry::get_mapping(&state.store, &id).ok_or_else(|| not_found("mapping", &id))?;
    let version = params.version.unwrap_or(mapping.version);
    if version == 0 || version > mapping.version {
        return Err(bad(format!(
            "mapping '{id}' has versions 1..{}",
            mapping.version
        )));
    }
    let turtle = mappings::turtle(&state.store, &id, version, |ns| {
        state.prefix_registry.declaration_for(ns)
    })
    .ok_or_else(|| not_found("mapping version", &format!("{id} v{version}")))?;
    Ok((StatusCode::OK, [(CONTENT_TYPE, "text/turtle")], turtle).into_response())
}

fn commit_mapping(state: &AppState, user: &AuthenticatedUser, m: &MappingRecord, what: &str) {
    crate::commit_log::record(
        &state.store,
        &state.base_url,
        crate::commit_log::CommitKind::Source,
        format!("mapping '{}' v{} {what}", m.id, m.version),
        Some(&user.user_id),
        Some(m.iri()),
        vec![m.version_iri()],
        0,
        0,
        Some(m.version.to_string()),
    );
}

// ─────────────────────────────── Runs ───────────────────────────────

fn run_error(e: RunError) -> (StatusCode, String) {
    match e {
        RunError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
        RunError::Failed(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        RunError::Gate { .. } => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "the SHACL write gate refused the run".to_string(),
        ),
    }
}

/// `POST /api/sources/:id/runs`
pub async fn create_run(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RunRequest>,
) -> ApiResult<Response> {
    // Bound concurrent expensive operations, as SHACL, reasoning and RML do.
    let _permit = state.expensive_semaphore.acquire().await.map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Server overloaded".to_string(),
        )
    })?;

    let source =
        registry::get_source(&state.store, &id).ok_or_else(|| not_found("datasource", &id))?;
    let mapping_id = body.mapping.trim_start_matches("urn:mapping:").to_string();
    let mapping = registry::get_mapping(&state.store, &mapping_id)
        .ok_or_else(|| not_found("mapping", &mapping_id))?;
    let mode = match body
        .mode
        .as_deref()
        .unwrap_or("full")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "full" => RunMode::Full,
        "watermark" | "incremental" => RunMode::Watermark,
        other => {
            return Err(bad(format!(
                "unknown run mode '{other}'; expected full or watermark"
            )))
        }
    };
    let batch_size = body.batch_size.unwrap_or(1_000);
    let model_version = body.model_version.clone();
    let actor = actor_iri(&state, &user);

    let blocking_state = state.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        runs::execute(
            ctx(&blocking_state),
            &source,
            &mapping,
            mode,
            model_version,
            batch_size,
            Some(&actor),
        )
    })
    .await
    .map_err(internal)?;

    match outcome {
        Ok(record) => Ok((
            StatusCode::CREATED,
            Json(RunResponse::of(&state.store, &record)),
        )
            .into_response()),
        Err(RunError::Gate { run, report }) => Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "the SHACL write gate refused the run; production is unchanged and the \
                          candidate graph is kept for inspection",
                "run": RunResponse::of(&state.store, run.as_ref()),
                "report": {
                    "conforms": report.conforms,
                    "results_count": report.results_count,
                    "results": report.results,
                },
            })),
        )
            .into_response()),
        Err(e) => Err(run_error(e)),
    }
}

/// `GET /api/sources/:id/runs`
pub async fn list_source_runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<RunResponse>>> {
    if registry::get_source(&state.store, &id).is_none() {
        return Err(not_found("datasource", &id));
    }
    Ok(Json(
        registry::list_runs(&state.store, Some(&id))
            .iter()
            .map(|r| RunResponse::of(&state.store, r))
            .collect(),
    ))
}

/// `GET /api/runs/:id`
pub async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<RunResponse>> {
    registry::get_run(&state.store, &id)
        .map(|r| Json(RunResponse::of(&state.store, &r)))
        .ok_or_else(|| not_found("run", &id))
}

/// `GET /api/runs/:id/provenance` — the run's PROV-O trail as Turtle.
pub async fn run_provenance(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let run = registry::get_run(&state.store, &id).ok_or_else(|| not_found("run", &id))?;
    let turtle = runs::provenance_turtle(&state.store, &run);
    Ok((StatusCode::OK, [(CONTENT_TYPE, "text/turtle")], turtle).into_response())
}

/// `POST /api/runs/:id/rollback`
pub async fn rollback_run(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SourceResponse>> {
    let run = registry::get_run(&state.store, &id).ok_or_else(|| not_found("run", &id))?;
    let source = registry::get_source(&state.store, &run.source_id)
        .ok_or_else(|| not_found("datasource", &run.source_id))?;
    let actor = actor_iri(&state, &user);
    let updated = runs::rollback(ctx(&state), &source, &run, Some(&actor)).map_err(run_error)?;
    Ok(Json(SourceResponse::from(&updated)))
}

/// `DELETE /api/runs/:id`
pub async fn delete_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let run = registry::get_run(&state.store, &id).ok_or_else(|| not_found("run", &id))?;
    match runs::delete(ctx(&state), &run) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(RunError::BadRequest(m)) => Err((StatusCode::CONFLICT, m)),
        Err(e) => Err(run_error(e)),
    }
}
