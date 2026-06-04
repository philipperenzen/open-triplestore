//! data-model versioning REST API.

use axum::extract::{Extension, Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use oxigraph::io::RdfFormat;
use serde_json::json;

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::SystemRole;
use crate::server::error::AppError;
use crate::server::AppState;
use crate::store::{escape_sparql_iri, TripleStore};

use super::diff::{collect_triples, compute_diff, triple_delta, version_revision};
use super::merge;
use super::models::{
    CreateDataModelRequest, CreateDraftRequest, DiffParams, PatchVersionRequest,
    SubGraphActionRequest, UpdateDataModelRequest, UpdateVersionRequest, VersionDataParams,
    VersionStatus,
};
use super::registry;
use super::upload;

// ─── Data Model CRUD ──────────────────────────────────────────────────────────

/// GET /api/data-models
pub async fn list_data_models(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
) -> Result<impl IntoResponse, AppError> {
    let records = registry::list_data_models(&state.store);
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    let filtered: Vec<_> = records
        .into_iter()
        .filter(|o| {
            state
                .auth_db
                .can_access_ontology(
                    uid,
                    o.is_public,
                    o.owner_type.as_deref(),
                    o.owner_id.as_deref(),
                )
                .unwrap_or(false)
        })
        .collect();
    Ok(Json(filtered))
}

/// POST /api/data-models
pub async fn create_data_model(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<CreateDataModelRequest>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin() {
        return Err(AppError::Unauthorized("Admin access required".to_string()));
    }
    let title = body.title.trim().to_string();
    let namespace = body.namespace.trim().to_string();
    if title.is_empty() {
        return Err(AppError::BadRequest("title is required".to_string()));
    }
    // Derive a URL-safe id from the title
    let id: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if registry::data_model_exists(&state.store, &state.base_url, &id) {
        return Err(AppError::BadRequest(format!(
            "Data model '{id}' already exists"
        )));
    }

    let now = Utc::now().to_rfc3339();
    let is_public = body.is_public.unwrap_or(false);
    // Default owner to the current user if none provided.
    let owner_type = body.owner_type.as_deref().unwrap_or("user").to_string();
    let owner_id = body
        .owner_id
        .clone()
        .unwrap_or_else(|| user.user_id.clone());
    registry::insert_data_model(
        &state.store,
        &state.base_url,
        &id,
        &title,
        &namespace,
        body.description.as_deref(),
        is_public,
        Some(&owner_type),
        Some(&owner_id),
        Some(&format!("{}/users/{}", state.base_url, user.user_id)),
        &now,
    )
    .map_err(AppError::from)?;

    let record = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::Internal("Failed to retrieve created ontology".to_string()))?;
    Ok((StatusCode::CREATED, Json(record)))
}

/// GET /api/data-models/:id
pub async fn get_data_model(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let record = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_ontology(
            uid,
            record.is_public,
            record.owner_type.as_deref(),
            record.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Data model '{id}' not found")));
    }
    Ok(Json(record))
}

/// DELETE /api/data-models/:id
pub async fn delete_data_model(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if user.role != SystemRole::SuperAdmin {
        return Err(AppError::Unauthorized(
            "Super-admin access required to delete an ontology".to_string(),
        ));
    }

    // Delete all version data graphs in one batched transaction.
    let versions = registry::list_versions(&state.store, &state.base_url, &id);
    let mut all_iris: Vec<String> = Vec::new();
    for ver in &versions {
        if ver.sub_graphs.is_empty() {
            all_iris.push(ver.graph_iri.clone());
        } else {
            all_iris.extend(ver.sub_graphs.iter().cloned());
        }
    }
    let iri_refs: Vec<&str> = all_iris.iter().map(|s| s.as_str()).collect();
    state
        .store
        .bulk_delete_graphs(&iri_refs)
        .map_err(AppError::from)?;

    registry::delete_data_model(&state.store, &state.base_url, &id).map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /api/data-models/:id
pub async fn update_data_model(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateDataModelRequest>,
) -> Result<impl IntoResponse, AppError> {
    let existing = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    if !state
        .auth_db
        .can_write_ontology(
            &user.user_id,
            existing.owner_type.as_deref(),
            existing.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Unauthorized(
            "Write access to this data model required".to_string(),
        ));
    }
    // Owner reassignment (donating the model to another user/org) is restricted to
    // system admins: the write check above runs against the EXISTING owner, so
    // without this an owner could repoint owner_id at an arbitrary org (handing its
    // admins control) or at a bogus principal that orphans the resource.
    let reassigning_owner = (body.owner_type.is_some()
        && body.owner_type.as_deref() != existing.owner_type.as_deref())
        || (body.owner_id.is_some() && body.owner_id.as_deref() != existing.owner_id.as_deref());
    if reassigning_owner && !user.is_admin() {
        return Err(AppError::Forbidden(
            "Only an administrator may change ontology ownership".to_string(),
        ));
    }
    registry::update_data_model(
        &state.store,
        &state.base_url,
        &id,
        body.title.as_deref(),
        body.namespace.as_deref(),
        body.description.as_deref(),
        body.is_public,
        body.owner_type.as_deref(),
        body.owner_id.as_deref(),
    )
    .map_err(AppError::from)?;
    let record = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::Internal("Failed to retrieve updated ontology".to_string()))?;
    Ok(Json(record))
}

// ─── Version listing and metadata ─────────────────────────────────────────────

/// GET /api/data-models/:id/versions
pub async fn list_versions(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let ontology = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_ontology(
            uid,
            ontology.is_public,
            ontology.owner_type.as_deref(),
            ontology.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Data model '{id}' not found")));
    }
    let versions = registry::list_versions(&state.store, &state.base_url, &id);
    Ok(Json(versions))
}

/// GET /api/data-models/:id/versions/:ver
pub async fn get_version(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let ontology = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_ontology(
            uid,
            ontology.is_public,
            ontology.owner_type.as_deref(),
            ontology.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Data model '{id}' not found")));
    }
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    Ok(Json(record))
}

/// GET /api/data-models/:id/collaborators
///
/// Lists who can see/edit this model: the owner (user) or org members, plus any
/// users who authored a version (draft holders). Deduplicated, owner/org first.
pub async fn list_collaborators(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let record = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_ontology(
            uid,
            record.is_public,
            record.owner_type.as_deref(),
            record.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Data model '{id}' not found")));
    }
    let created_by: Vec<String> = registry::list_versions(&state.store, &state.base_url, &id)
        .into_iter()
        .filter_map(|v| v.created_by)
        .collect();
    let list = collaborators_for(
        state.auth_db.as_ref(),
        record.owner_type.as_deref(),
        record.owner_id.as_deref(),
        &created_by,
    );
    Ok(Json(list))
}

/// GET /api/data-models/:id/commits — provenance trail for this model.
pub async fn list_commits(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
    Query(params): Query<crate::commit_log::CommitsParams>,
) -> Result<impl IntoResponse, AppError> {
    let record = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_ontology(
            uid,
            record.is_public,
            record.owner_type.as_deref(),
            record.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Data model '{id}' not found")));
    }
    let subject = format!("{}/data-model/{}", state.base_url, id);
    let scope = crate::commit_log::CommitScope::Subject(subject);
    let mut commits = crate::commit_log::list_commits(&state.store, &scope, &params.to_query());
    crate::commit_log::resolve_actors(state.auth_db.as_ref(), &mut commits);
    Ok(Json(commits))
}

/// PATCH /api/data-models/:id/versions/:ver
pub async fn update_version_notes(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
    Json(body): Json<UpdateVersionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let parent = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    if !state
        .auth_db
        .can_write_ontology(
            &user.user_id,
            parent.owner_type.as_deref(),
            parent.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Unauthorized(
            "Write access to this data model required".to_string(),
        ));
    }
    if registry::get_version(&state.store, &state.base_url, &id, &ver).is_none() {
        return Err(AppError::NotFound(format!("Version '{ver}' not found")));
    }
    registry::update_version_notes(
        &state.store,
        &state.base_url,
        &id,
        &ver,
        body.notes.as_deref(),
    )
    .map_err(AppError::from)?;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::Internal("Failed to retrieve updated version".to_string()))?;
    Ok(Json(record))
}

// ─── Version data download ────────────────────────────────────────────────────

/// GET /api/data-models/:id/versions/:ver/data
pub async fn get_version_data(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((id, ver)): Path<(String, String)>,
    Query(params): Query<VersionDataParams>,
) -> Result<impl IntoResponse, AppError> {
    // Enforce visibility: private ontologies require access rights
    let ontology = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_ontology(
            uid,
            ontology.is_public,
            ontology.owner_type.as_deref(),
            ontology.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Data model '{id}' not found")));
    }
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;

    let format = match params.format.as_deref().unwrap_or("trig") {
        "turtle" | "ttl" => RdfFormat::Turtle,
        "ntriples" | "nt" => RdfFormat::NTriples,
        "nquads" | "nq" => RdfFormat::NQuads,
        "jsonld" | "json-ld" => RdfFormat::NQuads, // fallback
        _ => RdfFormat::TriG,
    };

    let content_type = match format {
        RdfFormat::Turtle => "text/turtle",
        RdfFormat::NTriples => "application/n-triples",
        RdfFormat::NQuads => "application/n-quads",
        RdfFormat::TriG => "application/trig",
        _ => "application/octet-stream",
    };

    // Determine which graphs to dump
    let graphs_to_dump: Vec<String> = if let Some(suffix) = &params.graph {
        if suffix == "all" {
            record.sub_graphs.clone()
        } else {
            record
                .sub_graphs
                .iter()
                .filter(|g| g.ends_with(suffix.as_str()))
                .cloned()
                .collect()
        }
    } else {
        record.sub_graphs.clone()
    };

    // For a single graph (Turtle/NTriples), dump just that graph.
    // For multiple graphs, dump as TriG/NQuads.
    let dump_format =
        if graphs_to_dump.len() == 1 && matches!(format, RdfFormat::Turtle | RdfFormat::NTriples) {
            format
        } else if graphs_to_dump.len() > 1 {
            RdfFormat::TriG
        } else {
            format
        };

    let mut output = Vec::new();
    if graphs_to_dump.is_empty() {
        // Dump the base graph
        let data = state
            .store
            .graph_store_get(Some(&record.graph_iri), dump_format)
            .map_err(AppError::from)?;
        output.extend_from_slice(&data);
    } else if graphs_to_dump.len() == 1 {
        let data = state
            .store
            .graph_store_get(Some(&graphs_to_dump[0]), dump_format)
            .map_err(AppError::from)?;
        output.extend_from_slice(&data);
    } else {
        // Dump multiple graphs: concatenate their TriG serialization
        for g in &graphs_to_dump {
            let data = state
                .store
                .graph_store_get(Some(g), RdfFormat::TriG)
                .map_err(AppError::from)?;
            output.extend_from_slice(&data);
        }
    }

    let filename = format!("{}-{}.trig", id, ver);
    let content_disposition = format!("attachment; filename=\"{filename}\"");
    // Opaque content revision over the whole version, used for optimistic concurrency.
    let etag = format!(
        "\"{}\"",
        version_revision(&state.store, &version_graphs(&record))
    );
    use axum::http::HeaderValue;
    use axum::response::Response;
    let mut resp = Response::new(axum::body::Body::from(output));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::try_from(content_disposition)
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    if let Ok(v) = HeaderValue::from_str(&etag) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    Ok(resp)
}

// ─── Latest published data shortcut ───────────────────────────────────────────

/// GET /api/data-models/:id/latest/data
pub async fn get_latest_data(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
    Query(params): Query<VersionDataParams>,
) -> Result<impl IntoResponse, AppError> {
    let ontology = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_ontology(
            uid,
            ontology.is_public,
            ontology.owner_type.as_deref(),
            ontology.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Data model '{id}' not found")));
    }
    let ver = ontology
        .latest_published
        .ok_or_else(|| AppError::NotFound("No published version exists".to_string()))?;
    get_version_data(State(state), user, Path((id, ver)), Query(params)).await
}

// ─── Upload a new version ─────────────────────────────────────────────────────

/// POST /api/data-models/:id/versions  (multipart)
pub async fn upload_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    let parent = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    if !state
        .auth_db
        .can_write_ontology(
            &user.user_id,
            parent.owner_type.as_deref(),
            parent.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Unauthorized(
            "Write access to this data model required".to_string(),
        ));
    }

    let mut file_bytes: Option<Vec<u8>> = None;
    let mut content_type_field = String::from("application/trig");
    let mut filename_field = String::from("upload.trig");
    let mut version_override: Option<String> = None;
    let mut notes: Option<String> = None;
    let mut message: Option<String> = None;
    let mut kind_override: Option<String> = None;
    let mut merge = false;
    let mut is_public = false;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                if let Some(ct) = field.content_type() {
                    content_type_field = ct.to_string();
                }
                if let Some(fname) = field.file_name() {
                    filename_field = fname.to_string();
                    // Infer content type from extension if not set explicitly
                    if content_type_field == "application/octet-stream"
                        || content_type_field.is_empty()
                    {
                        content_type_field = infer_mime(&filename_field);
                    }
                }
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Failed to read file: {e}")))?;
                file_bytes = Some(bytes.to_vec());
            }
            "version" => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                let v = v.trim().to_string();
                if !v.is_empty() {
                    version_override = Some(v);
                }
            }
            "kind" => {
                let k = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                let k = k.trim().to_string();
                if !k.is_empty() {
                    kind_override = Some(k);
                }
            }
            "notes" => {
                let n = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                let n = n.trim().to_string();
                if !n.is_empty() {
                    notes = Some(n);
                }
            }
            "message" => {
                let m = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                let m = m.trim().to_string();
                if !m.is_empty() {
                    message = Some(m);
                }
            }
            "merge" => {
                let val = field.text().await.unwrap_or_default();
                merge = val.trim() == "true" || val.trim() == "1";
            }
            "is_public" => {
                let val = field.text().await.unwrap_or_default();
                is_public = val.trim() == "true" || val.trim() == "1";
            }
            _ => {
                // Drain unknown fields
                let _ = field.bytes().await;
            }
        }
    }

    let bytes = file_bytes.ok_or_else(|| AppError::BadRequest("No file provided".to_string()))?;

    // Parse and load is CPU-intensive and calls synchronous Oxigraph APIs.
    // Run it on the blocking thread pool so we don't stall the Tokio executor.
    let store_clone = state.store.clone();
    let base_url_clone = state.base_url.clone();
    let id_clone = id.clone();
    let (result, detected) = tokio::task::spawn_blocking(move || {
        // Parse RDF first
        let quads = upload::parse_rdf(&bytes, &content_type_field, &filename_field)?;

        // Auto-detect kind from RDF content
        let detected = crate::kind_detector::detect(&quads);

        // Validate kind if override provided
        if let Some(ref kind_str) = kind_override {
            crate::kind_detector::parse_kind_override(kind_str).ok_or_else(|| {
                format!(
                    "Invalid kind override: '{}'. Expected: data-model, vocabulary",
                    kind_str
                )
            })?;
        }

        // Proceed with loading
        let result = upload::parse_and_load(
            &store_clone,
            &base_url_clone,
            &id_clone,
            version_override.as_deref(),
            &bytes,
            &content_type_field,
            &filename_field,
            merge,
        )?;

        Ok::<_, String>((result, detected))
    })
    .await
    .map_err(|e| AppError::Internal(format!("Upload task failed: {e}")))?
    .map_err(AppError::BadRequest)?;

    if registry::version_exists(&state.store, &state.base_url, &id, &result.version) {
        return Err(AppError::BadRequest(format!(
            "Version '{}' already exists. Delete it first or use a different version string.",
            result.version
        )));
    }

    let now = Utc::now().to_rfc3339();
    let graph_iri = format!(
        "{}/data-model/{}/version/{}",
        state.base_url, id, result.version
    );

    use super::models::{DataModelVersion, DataModelVersionWithDetection};
    let record = DataModelVersion {
        data_model_id: id.clone(),
        version: result.version.clone(),
        status: if is_public {
            VersionStatus::Published
        } else {
            VersionStatus::Draft
        },
        graph_iri,
        sub_graphs: result.sub_graphs,
        created_at: now,
        created_by: Some(format!("{}/users/{}", state.base_url, user.user_id)),
        derived_from: None,
        notes,
        branch: None,
        sub_graph_status: Vec::new(),
    };

    registry::insert_version(&state.store, &state.base_url, &record).map_err(AppError::from)?;
    if is_public {
        registry::update_latest_published(&state.store, &state.base_url, &id, &result.version)
            .map_err(AppError::from)?;
    }

    // Record the upload in the provenance trail.
    let mut affected = vec![record.graph_iri.clone()];
    affected.extend(record.sub_graphs.iter().cloned());
    record_patch_commit(
        &state,
        crate::commit_log::CommitKind::DataModel,
        &format!("{}/data-model/{}", state.base_url, id),
        &user,
        &result.version,
        None,
        affected,
        0,
        0,
        None,
        None,
        message.as_deref().or(record.notes.as_deref()),
        None,
    );

    let response = DataModelVersionWithDetection {
        version: record,
        detected: detected.primary,
        mixed: detected.mixed,
        evidence: detected.evidence,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

fn infer_mime(filename: &str) -> String {
    match filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "ttl" | "turtle" => "text/turtle".to_string(),
        "nt" => "application/n-triples".to_string(),
        "nq" => "application/n-quads".to_string(),
        "trig" => "application/trig".to_string(),
        "rdf" | "xml" | "owl" => "application/rdf+xml".to_string(),
        "jsonld" | "json" => "application/ld+json".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

// ─── Patch (edit draft) ───────────────────────────────────────────────────────

/// PATCH /api/data-models/:id/versions/:ver/data
pub async fn patch_version_data(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
    headers: axum::http::HeaderMap,
    Json(body): Json<PatchVersionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let parent = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    if !state
        .auth_db
        .can_write_ontology(
            &user.user_id,
            parent.owner_type.as_deref(),
            parent.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Unauthorized(
            "Write access to this data model required".to_string(),
        ));
    }

    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;

    if record.status != VersionStatus::Draft {
        return Err(AppError::BadRequest(
            "Only Draft versions can be edited".to_string(),
        ));
    }

    // Optimistic concurrency: if the client sent If-Match, reject when the draft
    // has moved on since they last read it (another collaborator edited it).
    let current_revision = version_revision(&state.store, &version_graphs(&record));
    if let Some(if_match) = headers
        .get(axum::http::header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        let expected = if_match.trim().trim_matches('"');
        if expected != "*" && expected != current_revision {
            return Err(AppError::Conflict(json!({
                "error": "stale_revision",
                "message": "This draft was modified since you loaded it. Reload or start a branch.",
                "currentRevision": current_revision,
            })));
        }
    }

    // Determine default target graph (used when a triple has no graph override)
    let default_graph = {
        let suffix = body.graph.as_deref().unwrap_or("");
        if suffix.is_empty() {
            record.graph_iri.clone()
        } else {
            record
                .sub_graphs
                .iter()
                .find(|g| g.ends_with(suffix))
                .cloned()
                .unwrap_or_else(|| format!("{}/{}", record.graph_iri, suffix))
        }
    };

    let mut affected: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // Group removes by target graph (per-triple graph overrides the request-level default)
    for (graph, triples) in group_by_graph(&body.remove, &default_graph, &record) {
        let block = sparql_triple_block(&triples);
        if !block.is_empty() {
            let q = format!("DELETE DATA {{ GRAPH <{graph}> {{ {block} }} }}");
            state
                .store
                .update(&q)
                .map_err(|e| AppError::BadRequest(e.to_string()))?;
            affected.insert(graph);
        }
    }

    // Group adds by target graph
    for (graph, triples) in group_by_graph(&body.add, &default_graph, &record) {
        let block = sparql_triple_block(&triples);
        if !block.is_empty() {
            let q = format!("INSERT DATA {{ GRAPH <{graph}> {{ {block} }} }}");
            state
                .store
                .update(&q)
                .map_err(|e| AppError::BadRequest(e.to_string()))?;
            affected.insert(graph);
        }
    }

    // Return the post-edit revision so the client can advance its If-Match token.
    let new_revision = version_revision(&state.store, &version_graphs(&record));

    // Record a commit in the provenance trail (best-effort: never fail the write).
    record_patch_commit(
        &state,
        crate::commit_log::CommitKind::DataModel,
        &format!("{}/data-model/{}", state.base_url, id),
        &user,
        &ver,
        record.branch.as_deref(),
        affected.into_iter().collect(),
        body.add.len(),
        body.remove.len(),
        Some(current_revision),
        Some(new_revision.clone()),
        body.message.as_deref(),
        body.metadata.clone(),
    );
    use axum::http::HeaderValue;
    use axum::response::Response;
    let mut resp = Response::new(axum::body::Body::from(
        json!({ "currentRevision": new_revision }).to_string(),
    ));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("\"{new_revision}\"")) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    Ok(resp)
}

/// JSON view of one collaborator on a model/vocabulary.
fn collaborator_json(u: &crate::auth::models::User, role: &str, source: &str) -> serde_json::Value {
    json!({
        "user_id": u.id,
        "username": u.username,
        "display_name": u.display_name,
        "email": u.email,
        "role": role,
        "source": source,
    })
}

/// Build the collaborator list for a model/vocabulary given its owner and the
/// set of version `created_by` IRIs (version/draft authors). Deduplicated by
/// user; owner/org members are listed first, then any extra draft authors.
///
/// Shared by both the data-model and vocabulary collaborators endpoints.
pub fn collaborators_for(
    auth_db: &crate::auth::db::AuthDb,
    owner_type: Option<&str>,
    owner_id: Option<&str>,
    created_by_iris: &[String],
) -> Vec<serde_json::Value> {
    use std::collections::HashSet;
    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    match (owner_type, owner_id) {
        (Some("user"), Some(oid)) => {
            if let Ok(Some(u)) = auth_db.get_user_by_id(oid) {
                seen.insert(u.id.clone());
                out.push(collaborator_json(&u, "owner", "owner"));
            }
        }
        (Some("organisation"), Some(oid)) => {
            if let Ok(members) = auth_db.list_org_members(oid) {
                for (u, role) in members {
                    if seen.insert(u.id.clone()) {
                        out.push(collaborator_json(&u, role.as_str(), "organisation"));
                    }
                }
            }
        }
        _ => {}
    }

    for iri in created_by_iris {
        let uid = iri.rsplit('/').next().unwrap_or(iri);
        if uid.is_empty() || seen.contains(uid) {
            continue;
        }
        if let Ok(Some(u)) = auth_db.get_user_by_id(uid) {
            seen.insert(u.id.clone());
            out.push(collaborator_json(&u, "contributor", "draft"));
        }
    }

    out
}

/// Record a commit in the provenance trail for a registry write. Shared by the
/// data-model and vocabulary handlers. Best-effort: a failure to record the
/// commit is logged but never fails the mutation the user already made.
#[allow(clippy::too_many_arguments)]
pub fn record_patch_commit(
    state: &AppState,
    kind: crate::commit_log::CommitKind,
    subject_iri: &str,
    user: &AuthenticatedUser,
    version: &str,
    branch: Option<&str>,
    affected_graphs: Vec<String>,
    added: usize,
    removed: usize,
    parent_revision: Option<String>,
    revision: Option<String>,
    message: Option<&str>,
    metadata: Option<serde_json::Value>,
) {
    let msg = match message.map(str::trim) {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => format!("Edited {version} (+{added}/-{removed})"),
    };
    let mut rec = crate::commit_log::CommitRecord::new(kind, msg);
    rec.actor_iri = Some(format!("{}/users/{}", state.base_url, user.user_id));
    rec.subject_iri = Some(subject_iri.to_string());
    rec.version = Some(version.to_string());
    rec.branch = branch.map(str::to_string);
    rec.affected_graphs = affected_graphs;
    rec.added = added;
    rec.removed = removed;
    rec.parent_revision = parent_revision;
    rec.revision = revision;
    rec.metadata = metadata;
    if let Err(e) = crate::commit_log::insert_commit(&state.store, &state.base_url, &rec) {
        tracing::warn!("failed to record commit for {subject_iri} v{version}: {e}");
    }
}

/// Registry-agnostic view of a version used to compute branch tips. Shared by
/// the data-model and vocabulary `list_branches` endpoints.
pub struct BranchVersionView {
    pub branch: Option<String>,
    pub version: String,
    pub status: String,
    pub derived_from: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub sub_graphs: Vec<String>,
}

/// Group versions by branch (None → "main"), pick each branch's tip (newest),
/// and compute ahead/behind triple counts against the branch's base version.
///
/// `views` must be ordered newest-first (as `list_versions` returns them).
pub fn build_branches(
    store: &TripleStore,
    views: &[BranchVersionView],
) -> Vec<super::models::BranchInfo> {
    use std::collections::BTreeMap;
    let by_version: std::collections::HashMap<&str, &BranchVersionView> =
        views.iter().map(|v| (v.version.as_str(), v)).collect();

    // First view per branch is the tip (input is newest-first).
    let mut tips: BTreeMap<String, &BranchVersionView> = BTreeMap::new();
    for v in views {
        let b = v.branch.clone().unwrap_or_else(|| "main".to_string());
        tips.entry(b).or_insert(v);
    }

    tips.into_iter()
        .map(|(branch, tip)| {
            let (mut ahead, mut behind) = (0usize, 0usize);
            if let Some(base_ver) = tip.derived_from.as_deref() {
                if let Some(base) = by_version.get(base_ver) {
                    let (a, b) = triple_delta(store, &base.sub_graphs, &tip.sub_graphs);
                    ahead = a;
                    behind = b;
                }
            }
            super::models::BranchInfo {
                branch,
                tip_version: tip.version.clone(),
                status: tip.status.clone(),
                base_version: tip.derived_from.clone(),
                owner: tip.created_by.clone(),
                created_at: tip.created_at.clone(),
                ahead,
                behind,
            }
        })
        .collect()
}

/// The named graphs that make up a version's content (sub-graphs, or the base
/// graph when none are recorded). Used for revision hashing.
fn version_graphs(record: &super::models::DataModelVersion) -> Vec<String> {
    if record.sub_graphs.is_empty() {
        vec![record.graph_iri.clone()]
    } else {
        record.sub_graphs.clone()
    }
}

/// Group a slice of `RdfTriple` by their effective target graph.
/// A triple's `graph` field overrides the request-level `default_graph`; if the
/// override is a suffix it is matched against `record.sub_graphs`.
fn group_by_graph<'a>(
    triples: &'a [super::models::RdfTriple],
    default_graph: &str,
    record: &super::models::DataModelVersion,
) -> Vec<(String, Vec<&'a super::models::RdfTriple>)> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, Vec<&super::models::RdfTriple>> = BTreeMap::new();
    for t in triples {
        let graph = match t.graph.as_deref() {
            None | Some("") => default_graph.to_string(),
            Some(g) => {
                if g.starts_with("http://") || g.starts_with("https://") || g.starts_with("urn:") {
                    g.to_string()
                } else {
                    record
                        .sub_graphs
                        .iter()
                        .find(|sg| sg.ends_with(g))
                        .cloned()
                        .unwrap_or_else(|| format!("{}/{}", record.graph_iri, g))
                }
            }
        };
        map.entry(graph).or_default().push(t);
    }
    map.into_iter().collect()
}

fn sparql_triple_block(triples: &[&super::models::RdfTriple]) -> String {
    triples
        .iter()
        .map(|t| {
            let s = term_to_sparql(&t.s);
            let p = term_to_sparql(&t.p);
            let o = rdf_value_to_sparql(&t.o);
            format!("{s} {p} {o} .")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn term_to_sparql(t: &str) -> String {
    if (t.starts_with('<') && t.ends_with('>')) || t.starts_with("_:") {
        t.to_string()
    } else {
        format!("<{t}>")
    }
}

fn rdf_value_to_sparql(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => term_to_sparql(s),
        serde_json::Value::Object(map) => {
            let value = map.get("value").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(lang) = map.get("lang").and_then(|v| v.as_str()) {
                format!("\"{value}\"@{lang}")
            } else if let Some(dt) = map.get("datatype").and_then(|v| v.as_str()) {
                format!("\"{value}\"^^<{dt}>")
            } else {
                format!("\"{value}\"")
            }
        }
        other => format!("\"{other}\""),
    }
}

// ─── Draft creation ───────────────────────────────────────────────────────────

/// POST /api/data-models/:id/versions/:ver/draft
pub async fn create_draft(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, source_ver)): Path<(String, String)>,
    Json(body): Json<CreateDraftRequest>,
) -> Result<impl IntoResponse, AppError> {
    let parent = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    if !state
        .auth_db
        .can_write_ontology(
            &user.user_id,
            parent.owner_type.as_deref(),
            parent.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Unauthorized(
            "Write access to this data model required".to_string(),
        ));
    }

    let _source = registry::get_version(&state.store, &state.base_url, &id, &source_ver)
        .ok_or_else(|| AppError::NotFound(format!("Source version '{source_ver}' not found")))?;

    let target_ver = body.target_version.trim().to_string();
    if target_ver.is_empty() {
        return Err(AppError::BadRequest(
            "targetVersion is required".to_string(),
        ));
    }

    if registry::version_exists(&state.store, &state.base_url, &id, &target_ver) {
        return Err(AppError::BadRequest(format!(
            "Version '{}' already exists",
            target_ver
        )));
    }

    // Clone graphs
    let draft_sub_graphs =
        upload::clone_graphs_as_draft(&state.store, &state.base_url, &id, &source_ver, &target_ver)
            .map_err(AppError::from)?;

    let now = Utc::now().to_rfc3339();
    let graph_iri = format!(
        "{}/data-model/{}/version/{}",
        state.base_url, id, target_ver
    );

    use super::models::DataModelVersion;
    let record = DataModelVersion {
        data_model_id: id.clone(),
        version: target_ver.clone(),
        status: VersionStatus::Draft,
        graph_iri,
        sub_graphs: draft_sub_graphs,
        created_at: now,
        created_by: Some(format!("{}/users/{}", state.base_url, user.user_id)),
        derived_from: Some(source_ver.clone()),
        notes: None,
        branch: None,
        sub_graph_status: Vec::new(),
    };

    registry::insert_version(&state.store, &state.base_url, &record).map_err(AppError::from)?;
    registry::update_latest_draft(&state.store, &state.base_url, &id, &target_ver)
        .map_err(AppError::from)?;

    // Record the draft creation in the commit log (a clone, so no triple delta).
    let mut affected = vec![record.graph_iri.clone()];
    affected.extend(record.sub_graphs.iter().cloned());
    let msg = body
        .message
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Created draft {target_ver} from {source_ver}"));
    record_patch_commit(
        &state,
        crate::commit_log::CommitKind::DataModel,
        &format!("{}/data-model/{}", state.base_url, id),
        &user,
        &target_ver,
        None,
        affected,
        0,
        0,
        Some(source_ver.clone()),
        None,
        Some(&msg),
        None,
    );

    Ok((StatusCode::CREATED, Json(record)))
}

/// POST /api/data-models/:id/branches — create a named branch as a new draft
/// forked from `from_version`. Unlike a plain draft this carries a `branch`
/// label and does not move the model's `latestDraft` pointer.
pub async fn create_branch(
    State(state): State<AppState>,
    user_ext: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
    Json(body): Json<super::models::CreateBranchRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user = user_ext
        .ok_or_else(|| AppError::Unauthorized("Authentication required".to_string()))?
        .0;
    if !user.is_publisher() {
        return Err(AppError::Forbidden("Publisher access required".to_string()));
    }
    let parent = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    if !state
        .auth_db
        .can_write_ontology(
            &user.user_id,
            parent.owner_type.as_deref(),
            parent.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Unauthorized(
            "Write access to this data model required".to_string(),
        ));
    }

    let branch = body.branch.trim().to_string();
    if branch.is_empty() || branch == "main" {
        return Err(AppError::BadRequest(
            "branch must be a non-empty name other than 'main'".to_string(),
        ));
    }
    registry::get_version(&state.store, &state.base_url, &id, &body.from_version).ok_or_else(
        || AppError::NotFound(format!("Source version '{}' not found", body.from_version)),
    )?;

    let target_ver = body
        .target_version
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}-{}", body.from_version, branch));

    if registry::version_exists(&state.store, &state.base_url, &id, &target_ver) {
        return Err(AppError::BadRequest(format!(
            "Version '{target_ver}' already exists"
        )));
    }

    let draft_sub_graphs = upload::clone_graphs_as_draft(
        &state.store,
        &state.base_url,
        &id,
        &body.from_version,
        &target_ver,
    )
    .map_err(AppError::from)?;

    let now = Utc::now().to_rfc3339();
    let graph_iri = format!(
        "{}/data-model/{}/version/{}",
        state.base_url, id, target_ver
    );
    let record = super::models::DataModelVersion {
        data_model_id: id.clone(),
        version: target_ver.clone(),
        status: VersionStatus::Draft,
        graph_iri,
        sub_graphs: draft_sub_graphs,
        created_at: now,
        created_by: Some(format!("{}/users/{}", state.base_url, user.user_id)),
        derived_from: Some(body.from_version.clone()),
        notes: None,
        branch: Some(branch.clone()),
        sub_graph_status: Vec::new(),
    };
    registry::insert_version(&state.store, &state.base_url, &record).map_err(AppError::from)?;

    let msg = body
        .message
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Created branch '{branch}' from {}", body.from_version));
    let mut affected = vec![record.graph_iri.clone()];
    affected.extend(record.sub_graphs.iter().cloned());
    record_patch_commit(
        &state,
        crate::commit_log::CommitKind::DataModel,
        &format!("{}/data-model/{}", state.base_url, id),
        &user,
        &target_ver,
        Some(&branch),
        affected,
        0,
        0,
        Some(body.from_version.clone()),
        None,
        Some(&msg),
        None,
    );

    Ok((StatusCode::CREATED, Json(record)))
}

/// GET /api/data-models/:id/branches — list branch tips (one per branch name).
pub async fn list_branches(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let record = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_ontology(
            uid,
            record.is_public,
            record.owner_type.as_deref(),
            record.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Data model '{id}' not found")));
    }
    let views: Vec<BranchVersionView> = registry::list_versions(&state.store, &state.base_url, &id)
        .into_iter()
        .map(|v| BranchVersionView {
            branch: v.branch,
            version: v.version,
            status: v.status.as_str().to_string(),
            derived_from: v.derived_from,
            created_by: v.created_by,
            created_at: v.created_at,
            sub_graphs: if v.sub_graphs.is_empty() {
                vec![v.graph_iri]
            } else {
                v.sub_graphs
            },
        })
        .collect();
    Ok(Json(build_branches(&state.store, &views)))
}

// ─── Merge ──────────────────────────────────────────────────────────────────

/// Walk `prov:wasDerivedFrom` from `version` upward, returning [self, parent, …].
fn ancestor_chain(store: &TripleStore, base_url: &str, id: &str, version: &str) -> Vec<String> {
    let mut chain = Vec::new();
    let mut cur = Some(version.to_string());
    let mut guard = 0;
    while let Some(v) = cur {
        if chain.contains(&v) || guard > 200 {
            break;
        }
        chain.push(v.clone());
        cur = registry::get_version(store, base_url, id, &v).and_then(|r| r.derived_from);
        guard += 1;
    }
    chain
}

/// GET /api/data-models/:id/merge/preview?from=X&into=Y
pub async fn merge_preview(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
    Query(params): Query<merge::MergeParams>,
) -> Result<impl IntoResponse, AppError> {
    let parent = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_ontology(
            uid,
            parent.is_public,
            parent.owner_type.as_deref(),
            parent.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Data model '{id}' not found")));
    }
    let preview = compute_merge_preview(
        &state.store,
        &state.base_url,
        &id,
        &params.from,
        &params.into,
    )?;
    Ok(Json(preview))
}

/// POST /api/data-models/:id/merge — apply resolutions, write a new draft.
pub async fn merge_apply(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Json(body): Json<merge::MergeRequest>,
) -> Result<impl IntoResponse, AppError> {
    let parent = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    if !state
        .auth_db
        .can_write_ontology(
            &user.user_id,
            parent.owner_type.as_deref(),
            parent.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Unauthorized(
            "Write access to this data model required".to_string(),
        ));
    }

    let from_rec = registry::get_version(&state.store, &state.base_url, &id, &body.from)
        .ok_or_else(|| AppError::NotFound(format!("Version '{}' not found", body.from)))?;
    let into_rec = registry::get_version(&state.store, &state.base_url, &id, &body.into)
        .ok_or_else(|| AppError::NotFound(format!("Version '{}' not found", body.into)))?;

    let base_ver = merge::lca(
        &ancestor_chain(&state.store, &state.base_url, &id, &body.from),
        &ancestor_chain(&state.store, &state.base_url, &id, &body.into),
    );
    let base = base_ver
        .as_deref()
        .and_then(|v| registry::get_version(&state.store, &state.base_url, &id, v))
        .map(|r| collect_triples(&state.store, &version_graphs(&r)))
        .unwrap_or_default();
    let ours = collect_triples(&state.store, &version_graphs(&from_rec));
    let theirs = collect_triples(&state.store, &version_graphs(&into_rec));

    let merged = merge::resolve(&base, &ours, &theirs, &body.resolutions);

    // If an explicit branch is provided in the request, use it.
    // Otherwise inherit the branch label from the `into` version so that merging
    // main into a branch keeps the branch alive, while merging a branch into
    // main (into has no branch) correctly produces a main-line draft.
    let branch = body.branch.clone().or_else(|| into_rec.branch.clone());
    let target_ver = body
        .target_version
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}-merge-{}", body.into, body.from));
    if registry::version_exists(&state.store, &state.base_url, &id, &target_ver) {
        return Err(AppError::BadRequest(format!(
            "Version '{target_ver}' already exists"
        )));
    }

    let graph_iri = format!(
        "{}/data-model/{}/version/{}",
        state.base_url, id, target_ver
    );
    write_merged_graph(&state.store, &graph_iri, &merged).map_err(AppError::from)?;

    let record = super::models::DataModelVersion {
        data_model_id: id.clone(),
        version: target_ver.clone(),
        status: VersionStatus::Draft,
        graph_iri: graph_iri.clone(),
        sub_graphs: vec![graph_iri],
        created_at: Utc::now().to_rfc3339(),
        created_by: Some(format!("{}/users/{}", state.base_url, user.user_id)),
        derived_from: Some(body.into.clone()),
        notes: Some(format!("Merge of {} into {}", body.from, body.into)),
        branch,
        sub_graph_status: Vec::new(),
    };
    registry::insert_version(&state.store, &state.base_url, &record).map_err(AppError::from)?;

    // Record the merge in the commit log, with the triple delta vs the `into` parent.
    let new_graphs = version_graphs(&record);
    let (added, removed) = triple_delta(&state.store, &version_graphs(&into_rec), &new_graphs);
    let msg = body
        .message
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Merged {} into {}", body.from, body.into));
    record_patch_commit(
        &state,
        crate::commit_log::CommitKind::DataModel,
        &format!("{}/data-model/{}", state.base_url, id),
        &user,
        &target_ver,
        record.branch.as_deref(),
        new_graphs,
        added,
        removed,
        Some(body.into.clone()),
        None,
        Some(&msg),
        None,
    );

    Ok((StatusCode::CREATED, Json(record)))
}

/// Shared: compute a 3-way merge preview for two versions of one model/vocab.
pub fn compute_merge_preview(
    store: &TripleStore,
    base_url: &str,
    id: &str,
    from: &str,
    into: &str,
) -> Result<merge::MergePreview, AppError> {
    let from_rec = registry::get_version(store, base_url, id, from)
        .ok_or_else(|| AppError::NotFound(format!("Version '{from}' not found")))?;
    let into_rec = registry::get_version(store, base_url, id, into)
        .ok_or_else(|| AppError::NotFound(format!("Version '{into}' not found")))?;
    let base_ver = merge::lca(
        &ancestor_chain(store, base_url, id, from),
        &ancestor_chain(store, base_url, id, into),
    );
    let base = base_ver
        .as_deref()
        .and_then(|v| registry::get_version(store, base_url, id, v))
        .map(|r| collect_triples(store, &version_graphs(&r)))
        .unwrap_or_default();
    let ours = collect_triples(store, &version_graphs(&from_rec));
    let theirs = collect_triples(store, &version_graphs(&into_rec));
    Ok(merge::preview(base_ver, &base, &ours, &theirs))
}

/// Shared: write a merged triple set into a fresh named graph (chunked INSERT DATA).
pub fn write_merged_graph(
    store: &TripleStore,
    graph_iri: &str,
    triples: &[(String, String, String)],
) -> Result<(), crate::store::engine::StoreError> {
    // Only the graph IRI is escaped; the s/p/o components are already-serialized
    // N-Triples terms carrying their own delimiters, so they must not be touched.
    let graph_iri = escape_sparql_iri(graph_iri);
    for chunk in triples.chunks(1000) {
        let block: String = chunk
            .iter()
            .map(|(s, p, o)| format!("{s} {p} {o} ."))
            .collect::<Vec<_>>()
            .join("\n");
        if !block.is_empty() {
            store.update(&format!(
                "INSERT DATA {{ GRAPH <{graph_iri}> {{ {block} }} }}"
            ))?;
        }
    }
    Ok(())
}

// ─── Rebase ───────────────────────────────────────────────────────────────────

/// POST /api/data-models/:id/versions/:ver/rebase
///
/// Rebase a branch version onto a newer base (default: latest published).
/// Performs a three-way merge: (LCA, branch_tip, onto) and creates a new
/// Draft with the same branch label whose `derived_from` is `onto`.
/// Returns the new version record plus a `clean` flag and conflict count.
pub async fn rebase_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
    Json(body): Json<super::models::RebaseRequest>,
) -> Result<impl IntoResponse, AppError> {
    let parent = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    if !state
        .auth_db
        .can_write_ontology(
            &user.user_id,
            parent.owner_type.as_deref(),
            parent.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Unauthorized(
            "Write access to this data model required".to_string(),
        ));
    }

    let branch_rec = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;

    let branch_name = branch_rec.branch.clone().ok_or_else(|| {
        AppError::BadRequest("Version has no branch label; use merge instead".to_string())
    })?;

    // Determine target base: explicit `onto` or latest published.
    let onto_ver = body
        .onto
        .clone()
        .or_else(|| parent.latest_published.clone())
        .ok_or_else(|| {
            AppError::BadRequest(
                "No 'onto' version specified and no published version exists".to_string(),
            )
        })?;

    if onto_ver == ver {
        return Err(AppError::BadRequest(
            "Branch tip and rebase target are the same version".to_string(),
        ));
    }

    let onto_rec = registry::get_version(&state.store, &state.base_url, &id, &onto_ver)
        .ok_or_else(|| AppError::NotFound(format!("Rebase target '{onto_ver}' not found")))?;

    // Compute lowest common ancestor.
    let base_ver = merge::lca(
        &ancestor_chain(&state.store, &state.base_url, &id, &ver),
        &ancestor_chain(&state.store, &state.base_url, &id, &onto_ver),
    );
    let base = base_ver
        .as_deref()
        .and_then(|v| registry::get_version(&state.store, &state.base_url, &id, v))
        .map(|r| collect_triples(&state.store, &version_graphs(&r)))
        .unwrap_or_default();

    // ours = branch tip, theirs = new base (onto).
    let ours = collect_triples(&state.store, &version_graphs(&branch_rec));
    let theirs = collect_triples(&state.store, &version_graphs(&onto_rec));

    // Compute the preview to expose conflict count and clean flag.
    let preview = merge::preview(base_ver.clone(), &base, &ours, &theirs);

    // Resolve (unresolved conflicts default to "ours" to avoid data loss).
    let merged = merge::resolve(&base, &ours, &theirs, &[]);

    let target_ver = body
        .target_version
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}-rebase-{}", ver, onto_ver));

    if registry::version_exists(&state.store, &state.base_url, &id, &target_ver) {
        return Err(AppError::BadRequest(format!(
            "Version '{target_ver}' already exists"
        )));
    }

    let graph_iri = format!(
        "{}/data-model/{}/version/{}",
        state.base_url, id, target_ver
    );
    write_merged_graph(&state.store, &graph_iri, &merged).map_err(AppError::from)?;

    let record = super::models::DataModelVersion {
        data_model_id: id.clone(),
        version: target_ver.clone(),
        status: VersionStatus::Draft,
        graph_iri: graph_iri.clone(),
        sub_graphs: vec![graph_iri],
        created_at: Utc::now().to_rfc3339(),
        created_by: Some(format!("{}/users/{}", state.base_url, user.user_id)),
        derived_from: Some(onto_ver.clone()),
        notes: Some(format!("Rebase of {} onto {}", ver, onto_ver)),
        branch: Some(branch_name),
        sub_graph_status: Vec::new(),
    };
    registry::insert_version(&state.store, &state.base_url, &record).map_err(AppError::from)?;

    // Record the rebase in the commit log, with the triple delta vs the `onto` base.
    let new_graphs = version_graphs(&record);
    let (added, removed) = triple_delta(&state.store, &version_graphs(&onto_rec), &new_graphs);
    let msg = body
        .message
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Rebased {ver} onto {onto_ver}"));
    record_patch_commit(
        &state,
        crate::commit_log::CommitKind::DataModel,
        &format!("{}/data-model/{}", state.base_url, id),
        &user,
        &target_ver,
        record.branch.as_deref(),
        new_graphs,
        added,
        removed,
        Some(onto_ver.clone()),
        None,
        Some(&msg),
        None,
    );

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "version": record,
            "clean": preview.clean,
            "conflicts": preview.conflicts.len(),
            "auto_added": preview.auto_added,
            "auto_removed": preview.auto_removed,
        })),
    ))
}

// ─── Stage ────────────────────────────────────────────────────────────────────

/// POST /api/data-models/:id/versions/:ver/stage
pub async fn stage_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let parent = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    if !state
        .auth_db
        .can_write_ontology(
            &user.user_id,
            parent.owner_type.as_deref(),
            parent.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Unauthorized(
            "Write access to this data model required".to_string(),
        ));
    }

    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;

    if record.status != VersionStatus::Draft {
        return Err(AppError::BadRequest(
            "Only Draft versions can be staged".to_string(),
        ));
    }

    registry::update_version_status(
        &state.store,
        &state.base_url,
        &id,
        &ver,
        VersionStatus::Staged,
    )
    .map_err(AppError::from)?;
    // Only clear the model's latestDraft pointer when staging the main-line draft;
    // branch versions have their own tip and must not disturb the main draft pointer.
    if record.branch.is_none() {
        registry::clear_latest_draft(&state.store, &state.base_url, &id).map_err(AppError::from)?;
    }

    Ok(Json(json!({ "status": "staged", "version": ver })))
}

// ─── Publish ──────────────────────────────────────────────────────────────────

/// POST /api/data-models/:id/versions/:ver/publish
pub async fn publish_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin() {
        return Err(AppError::Unauthorized("Admin access required".to_string()));
    }

    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;

    if !matches!(record.status, VersionStatus::Staged | VersionStatus::Draft) {
        return Err(AppError::BadRequest(
            "Only Staged or Draft versions can be published".to_string(),
        ));
    }

    let data_model = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;

    // Deprecate the previous latest published version
    let prior_version_iri = if let Some(old_ver) = &data_model.latest_published {
        if old_ver != &ver {
            registry::update_version_status(
                &state.store,
                &state.base_url,
                &id,
                old_ver,
                VersionStatus::Deprecated,
            )
            .map_err(AppError::from)?;
            Some(super::version_iri::build_version_iri(
                &data_model.namespace,
                old_ver,
            ))
        } else {
            None
        }
    } else {
        None
    };

    registry::update_version_status(
        &state.store,
        &state.base_url,
        &id,
        &ver,
        VersionStatus::Published,
    )
    .map_err(AppError::from)?;
    registry::update_latest_published(&state.store, &state.base_url, &id, &ver)
        .map_err(AppError::from)?;

    // Mint owl:versionIRI into the published version's named graph.
    let version_iri = super::version_iri::mint(
        &state.store,
        &record.graph_iri,
        &data_model.namespace,
        &ver,
        prior_version_iri.as_deref(),
    )
    .map_err(AppError::from)?;

    Ok(Json(json!({
        "status": "published",
        "version": ver,
        "versionIRI": version_iri,
    })))
}

// ─── Deprecate ────────────────────────────────────────────────────────────────

/// POST /api/data-models/:id/versions/:ver/deprecate
pub async fn deprecate_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin() {
        return Err(AppError::Unauthorized("Admin access required".to_string()));
    }

    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;

    if record.status == VersionStatus::Deprecated {
        return Err(AppError::BadRequest(
            "Version is already deprecated".to_string(),
        ));
    }

    registry::update_version_status(
        &state.store,
        &state.base_url,
        &id,
        &ver,
        VersionStatus::Deprecated,
    )
    .map_err(AppError::from)?;

    Ok(Json(json!({ "status": "deprecated", "version": ver })))
}

// ─── Per-subgraph lifecycle (Phase 6) ──────────────────────────────────────────

/// Resolve the body's `graph` (full IRI or trailing suffix) to one of the
/// version's actual subgraph IRIs.
fn resolve_sub_graph(record: &super::models::DataModelVersion, wanted: &str) -> Option<String> {
    if record.sub_graphs.iter().any(|g| g == wanted) {
        return Some(wanted.to_string());
    }
    record
        .sub_graphs
        .iter()
        .find(|g| g.ends_with(wanted))
        .cloned()
}

/// Shared implementation for subgraph stage/publish/deprecate.
async fn transition_sub_graph(
    state: AppState,
    user: AuthenticatedUser,
    id: String,
    ver: String,
    graph: String,
    new_status: VersionStatus,
    require_admin: bool,
) -> Result<impl IntoResponse, AppError> {
    let parent = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    if require_admin {
        if !user.is_admin() {
            return Err(AppError::Unauthorized("Admin access required".to_string()));
        }
    } else if !state
        .auth_db
        .can_write_ontology(
            &user.user_id,
            parent.owner_type.as_deref(),
            parent.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Unauthorized(
            "Write access to this data model required".to_string(),
        ));
    }

    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    let sub_graph_iri = resolve_sub_graph(&record, &graph).ok_or_else(|| {
        AppError::BadRequest(format!("Subgraph '{graph}' not found in version '{ver}'"))
    })?;

    registry::set_sub_graph_status(
        &state.store,
        &state.base_url,
        &id,
        &ver,
        &sub_graph_iri,
        Some(new_status),
    )
    .map_err(AppError::from)?;

    Ok(Json(json!({
        "status": new_status.as_str(),
        "version": ver,
        "graph": sub_graph_iri,
    })))
}

/// POST /api/data-models/:id/versions/:ver/subgraph/stage
pub async fn stage_sub_graph(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
    Json(body): Json<SubGraphActionRequest>,
) -> Result<impl IntoResponse, AppError> {
    transition_sub_graph(
        state,
        user,
        id,
        ver,
        body.graph,
        VersionStatus::Staged,
        false,
    )
    .await
}

/// POST /api/data-models/:id/versions/:ver/subgraph/publish
pub async fn publish_sub_graph(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
    Json(body): Json<SubGraphActionRequest>,
) -> Result<impl IntoResponse, AppError> {
    transition_sub_graph(
        state,
        user,
        id,
        ver,
        body.graph,
        VersionStatus::Published,
        true,
    )
    .await
}

/// POST /api/data-models/:id/versions/:ver/subgraph/deprecate
pub async fn deprecate_sub_graph(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
    Json(body): Json<SubGraphActionRequest>,
) -> Result<impl IntoResponse, AppError> {
    transition_sub_graph(
        state,
        user,
        id,
        ver,
        body.graph,
        VersionStatus::Deprecated,
        true,
    )
    .await
}

// ─── Diff ─────────────────────────────────────────────────────────────────────

/// GET /api/data-models/:id/diff?from=X&to=Y&graph=suffix
pub async fn diff_versions(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
    Query(params): Query<DiffParams>,
) -> Result<impl IntoResponse, AppError> {
    let parent = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_ontology(
            uid,
            parent.is_public,
            parent.owner_type.as_deref(),
            parent.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Data model '{id}' not found")));
    }
    let from_record = registry::get_version(&state.store, &state.base_url, &id, &params.from)
        .ok_or_else(|| AppError::NotFound(format!("Version '{}' not found", params.from)))?;

    let to_record = registry::get_version(&state.store, &state.base_url, &id, &params.to)
        .ok_or_else(|| AppError::NotFound(format!("Version '{}' not found", params.to)))?;

    let from_graphs = if from_record.sub_graphs.is_empty() {
        vec![from_record.graph_iri]
    } else {
        from_record.sub_graphs
    };
    let to_graphs = if to_record.sub_graphs.is_empty() {
        vec![to_record.graph_iri]
    } else {
        to_record.sub_graphs
    };

    let result = compute_diff(
        &state.store,
        &from_graphs,
        &to_graphs,
        params.graph.as_deref(),
    );

    Ok(Json(result))
}
