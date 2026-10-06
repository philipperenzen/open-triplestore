//! A dataset's own prefix table: part of its data, as RDF Patch has it
//! ("Prefixes … are changes to the data the patch is applied to"). An
//! applied patch's `PA` / `PD` rows change it, a version records it and a
//! restore brings it back, and the dataset's Turtle and TriG exports declare
//! it ahead of the instance's prefix registry.
//!
//! * `GET    /api/datasets/:id/prefixes` — the table (dataset read access)
//! * `PUT    /api/datasets/:id/prefixes` — replace it, body `{label: namespace}`
//! * `PUT    /api/datasets/:id/prefixes/:label` — set one, body `{namespace}`
//! * `DELETE /api/datasets/:id/prefixes/:label` — remove one
//!
//! A label is a Turtle `PN_PREFIX` (letters first; letters, digits, `_`,
//! `-` and inner `.`), or empty for the default prefix, which only the whole-
//! table `PUT` and patches can name. A namespace is any absolute IRI.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;

use crate::auth::middleware::AuthenticatedUser;
use crate::dataset_versions::handlers as vh;
use crate::server::error::AppError;
use crate::server::AppState;

fn internal(e: anyhow::Error) -> AppError {
    AppError::Internal(e.to_string())
}

fn writer(
    state: &AppState,
    user: Option<Extension<AuthenticatedUser>>,
    dataset_id: &str,
) -> Result<AuthenticatedUser, AppError> {
    let Some(Extension(user)) = user else {
        return Err(AppError::Unauthorized("Authentication required".into()));
    };
    let ds = vh::load_dataset(state, dataset_id)?;
    vh::require_read(state, &ds, Some(&user.user_id))?;
    vh::require_write(state, &ds, &user.user_id)?;
    Ok(user)
}

fn check(label: &str, namespace: &str) -> Result<(), AppError> {
    if !crate::rdf_patch::is_prefix_name(label) {
        return Err(AppError::BadRequest(format!(
            "{label:?} is not a prefix label: a letter first, then letters, digits, '_', '-' \
             or '.' (not last); or empty for the default prefix"
        )));
    }
    oxigraph::model::NamedNode::new(namespace)
        .map_err(|e| AppError::BadRequest(format!("{namespace:?} is not an absolute IRI: {e}")))?;
    Ok(())
}

/// GET /api/datasets/:id/prefixes
pub async fn list(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<Response, AppError> {
    let ds = vh::load_dataset(&state, &dataset_id)?;
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    vh::require_read(&state, &ds, uid)?;
    let rows = state
        .auth_db
        .list_dataset_prefixes(&dataset_id)
        .map_err(internal)?;
    Ok(Json(rows).into_response())
}

/// PUT /api/datasets/:id/prefixes — replace the table.
pub async fn replace(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Json(body): Json<BTreeMap<String, String>>,
) -> Result<Response, AppError> {
    let user = writer(&state, user, &dataset_id)?;
    for (label, namespace) in &body {
        check(label, namespace)?;
    }
    let pairs: Vec<(String, String)> = body.into_iter().collect();
    state
        .auth_db
        .replace_dataset_prefixes(&dataset_id, &pairs, Some(&user.user_id))
        .map_err(internal)?;
    let rows = state
        .auth_db
        .list_dataset_prefixes(&dataset_id)
        .map_err(internal)?;
    Ok(Json(rows).into_response())
}

#[derive(Deserialize)]
pub struct PutPrefix {
    namespace: String,
}

/// PUT /api/datasets/:id/prefixes/:label — set or repoint one prefix.
pub async fn put_one(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, label)): Path<(String, String)>,
    Json(body): Json<PutPrefix>,
) -> Result<Response, AppError> {
    let user = writer(&state, user, &dataset_id)?;
    check(&label, &body.namespace)?;
    let created = state
        .auth_db
        .put_dataset_prefix(&dataset_id, &label, &body.namespace, Some(&user.user_id))
        .map_err(internal)?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(serde_json::json!({ "label": label, "namespace": body.namespace })),
    )
        .into_response())
}

/// DELETE /api/datasets/:id/prefixes/:label
pub async fn delete_one(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, label)): Path<(String, String)>,
) -> Result<Response, AppError> {
    writer(&state, user, &dataset_id)?;
    if !state
        .auth_db
        .delete_dataset_prefix(&dataset_id, &label)
        .map_err(internal)?
    {
        return Err(AppError::NotFound(format!(
            "dataset {dataset_id} has no prefix {label:?}"
        )));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}
