//! Validate-and-commit orchestration (Unified Accounts plan, Phase 4).
//!
//! `POST /api/datasets/validate-and-commit` runs SHACL validation on an external
//! validation platform (forwarding the caller's bearer token, on-behalf-of) and,
//! **only if the data conforms**, imports it and snapshots a new dataset version
//! with a commit message — into a brand-new private dataset (default) or a
//! caller-specified existing dataset (ACL-checked). Commit is gated on `conforms`
//! so the operation never half-applies.

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use chrono::Utc;
use oxigraph::io::RdfFormat;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::audit::{AuditEventBuilder, AuditEventType, AuditOutcome};
use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{Dataset, OwnerType, Role, Visibility};
use crate::server::error::AppError;
use crate::server::AppState;

use super::models::{DatasetVersion, VersionStatus};
use super::{registry, reports, snapshot};

/// A graph supplied inline (`ttl`) or by OTS reference — forwarded verbatim to
/// the Validation Platform, whose contract uses these exact (snake_case) fields.
#[derive(Debug, Deserialize, Serialize)]
pub struct GraphSourceIn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_dataset_shapes: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct CommitOnValid {
    /// "new" (default private dataset) | "dataset" (commit into an existing one).
    pub target: String,
    #[serde(default)]
    pub dataset_id: Option<String>,
    #[serde(default)]
    pub org_id: Option<String>,
    #[serde(default)]
    pub graph: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ValidateAndCommitRequest {
    pub data: GraphSourceIn,
    pub shapes: GraphSourceIn,
    #[serde(default)]
    pub inference: Option<String>,
    pub commit_on_valid: CommitOnValid,
}

#[derive(Serialize)]
struct ValidatorRequest<'a> {
    data: &'a GraphSourceIn,
    shapes: &'a GraphSourceIn,
    inference: &'a str,
}

#[derive(Deserialize)]
struct ValidatorResponse {
    conforms: bool,
    #[serde(default)]
    report: String,
}

/// Call the Validation Platform `/validate`, forwarding the caller's token.
async fn run_validation(
    bearer: &str,
    body: &ValidateAndCommitRequest,
) -> Result<ValidatorResponse, AppError> {
    let base = std::env::var("VALIDATION_API_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            AppError::BadRequest(
                "Validation Platform not configured (set VALIDATION_API_URL)".to_string(),
            )
        })?;
    let req = ValidatorRequest {
        data: &body.data,
        shapes: &body.shapes,
        inference: body.inference.as_deref().unwrap_or("none"),
    };
    let resp = reqwest::Client::new()
        .post(format!("{}/validate", base.trim_end_matches('/')))
        .header(header::AUTHORIZATION, bearer)
        .json(&req)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("validator call failed: {e}")))?;
    if !resp.status().is_success() {
        let code = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Internal(format!(
            "validator returned {code}: {}",
            text.chars().take(300).collect::<String>()
        )));
    }
    resp.json::<ValidatorResponse>()
        .await
        .map_err(|e| AppError::Internal(format!("bad validator response: {e}")))
}

/// POST /api/datasets/validate-and-commit
pub async fn validate_and_commit(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    headers: HeaderMap,
    Json(body): Json<ValidateAndCommitRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Commit needs the actual triples, so the data must be supplied inline.
    let data_ttl = body
        .data
        .ttl
        .clone()
        .ok_or_else(|| AppError::BadRequest("commit requires inline `data.ttl`".to_string()))?;

    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // 1. Validate. Commit only proceeds on `conforms`.
    let result = run_validation(&bearer, &body).await?;
    if !result.conforms {
        return Ok((
            StatusCode::OK,
            Json(json!({ "conforms": false, "committed": false, "report": result.report })),
        ));
    }

    // 2. Resolve (or create) the target dataset + graph, with ACL checks.
    let c = &body.commit_on_valid;
    let (dataset, graph_iri): (Dataset, String) = match c.target.as_str() {
        "new" => {
            let ds_id = Uuid::new_v4().to_string();
            let (owner_type, owner_id) = match c.org_id.as_deref().filter(|s| !s.is_empty()) {
                Some(org) => {
                    // Only org members (admin/member, not viewer) may create org-owned datasets.
                    match state
                        .auth_db
                        .get_org_membership(&user.user_id, org)
                        .map_err(|e| AppError::Internal(e.to_string()))?
                    {
                        Some(Role::Admin) | Some(Role::Member) => {
                            (OwnerType::Organisation, org.to_string())
                        }
                        _ => {
                            return Err(AppError::Unauthorized(
                                "Membership of the target organisation is required".to_string(),
                            ))
                        }
                    }
                }
                None => (OwnerType::User, user.user_id.clone()),
            };
            let name = c
                .name
                .clone()
                .unwrap_or_else(|| "Validated import".to_string());
            let ds = state
                .auth_db
                .create_dataset(
                    &ds_id,
                    &name,
                    None,
                    owner_type,
                    &owner_id,
                    Visibility::Private,
                    None,
                )
                .map_err(|e| AppError::Internal(e.to_string()))?;
            let graph_iri = c
                .graph
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{}/dataset/{}/graph/imported", state.base_url, ds_id));
            state
                .auth_db
                .add_dataset_graph(&ds_id, &graph_iri)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            (ds, graph_iri)
        }
        "dataset" => {
            let ds_id = c.dataset_id.clone().ok_or_else(|| {
                AppError::BadRequest("target 'dataset' requires datasetId".to_string())
            })?;
            let ds = state
                .auth_db
                .get_dataset(&ds_id)
                .map_err(|e| AppError::Internal(e.to_string()))?
                .ok_or_else(|| AppError::NotFound(format!("Dataset '{ds_id}' not found")))?;
            if !state
                .auth_db
                .can_write_dataset(&user.user_id, &ds)
                .map_err(|e| AppError::Internal(e.to_string()))?
            {
                return Err(AppError::Unauthorized(
                    "Write access to this dataset required".to_string(),
                ));
            }
            let registered = state
                .auth_db
                .list_dataset_graphs(&ds_id)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            let graph_iri = match c.graph.clone().filter(|s| !s.is_empty()) {
                Some(g) => g,
                None => registered.first().cloned().unwrap_or_else(|| {
                    format!("{}/dataset/{}/graph/imported", state.base_url, ds_id)
                }),
            };
            if !registered.contains(&graph_iri) {
                state
                    .auth_db
                    .add_dataset_graph(&ds_id, &graph_iri)
                    .map_err(|e| AppError::Internal(e.to_string()))?;
            }
            (ds, graph_iri)
        }
        other => {
            return Err(AppError::BadRequest(format!(
                "unknown commit target '{other}'"
            )))
        }
    };

    // 3. Enforce the dataset's effective shapes (Studio write-gates + bindings
    // + legacy shacl_on_write) so a commit cannot bypass graph-attached shapes,
    // even if it passed the request-supplied shapes above.
    crate::server::routes::validate_on_write(
        &state,
        Some(&graph_iri),
        &data_ttl,
        RdfFormat::Turtle,
    )?;

    // 4. Import (replace) the validated data into the target graph.
    state
        .store
        .graph_store_put(Some(&graph_iri), &data_ttl, RdfFormat::Turtle)
        .map_err(|e| AppError::Internal(format!("import failed: {e}")))?;
    #[cfg(feature = "text-search")]
    state.mark_text_dirty();

    // 5. Snapshot a new draft version carrying the commit message.
    let version = c
        .version
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| Utc::now().format("%Y%m%d%H%M%S").to_string());
    let source_graphs = vec![graph_iri.clone()];
    let source_map = snapshot::snapshot_graphs(
        &state.store,
        &state.base_url,
        &dataset.id,
        &version,
        &source_graphs,
    )
    .map_err(AppError::from)?;
    let snapshot_graphs: Vec<String> = source_map
        .iter()
        .map(|m| m.snapshot_graph.clone())
        .collect();
    let record = DatasetVersion {
        dataset_id: dataset.id.clone(),
        version: version.clone(),
        status: VersionStatus::Draft,
        graph_iri: format!(
            "{}/dataset/{}/version/{}",
            state.base_url, dataset.id, version
        ),
        snapshot_graphs,
        source_map,
        created_at: Utc::now().to_rfc3339(),
        created_by: Some(format!("{}/users/{}", state.base_url, user.user_id)),
        derived_from: None,
        notes: c.message.clone(),
        branch: None,
    };
    registry::insert_version(&state.store, &state.base_url, &record).map_err(AppError::from)?;
    registry::update_latest_draft(&state.store, &state.base_url, &dataset.id, &version)
        .map_err(AppError::from)?;
    // Snapshot the validation layer alongside the committed data (best-effort).
    if let Err(e) = crate::shacl_studio::bindings::snapshot_dataset_bindings(
        &state.store,
        &state.base_url,
        &dataset.id,
        &version,
        &source_graphs,
    ) {
        tracing::warn!(
            "failed to snapshot validation bindings for {} v{version}: {e}",
            dataset.id
        );
    }
    // Re-test this dataset's saved queries against the freshly committed version.
    crate::saved_queries::testing::spawn_version_tests(&state, &dataset.id, &version);

    // Persist the conforming report with provenance (best-effort: the commit has
    // already succeeded, so a storage hiccup must not fail the request).
    let shapes_ref = c
        .graph
        .clone()
        .or_else(|| body.shapes.graph.clone())
        .or_else(|| {
            if body.shapes.use_dataset_shapes == Some(true) {
                body.shapes.dataset_id.clone()
            } else {
                None
            }
        });
    if let Err(e) = reports::persist_report(
        &state,
        &dataset.id,
        Some(&version),
        true,
        &result.report,
        Some("inline"),
        shapes_ref.as_deref(),
        "platform",
        Some(&user.user_id),
    ) {
        tracing::warn!("failed to persist validation report: {e}");
    }

    state.audit.log(
        AuditEventBuilder::new(AuditEventType::GraphCreated, AuditOutcome::Success)
            .actor_id(user.user_id.clone())
            .resource("dataset", dataset.id.clone())
            .action("validate-and-commit")
            .details(json!({ "version": version, "graph": graph_iri })),
    );

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "conforms": true,
            "committed": true,
            "datasetId": dataset.id,
            "version": version,
            "graph": graph_iri,
            "report": result.report,
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::models::SystemRole;
    use crate::server::AppState;
    use crate::store::TripleStore;

    fn empty_source() -> GraphSourceIn {
        GraphSourceIn {
            ttl: None,
            format: None,
            dataset_id: None,
            graph: None,
            use_dataset_shapes: None,
        }
    }

    fn test_user() -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: "u1".to_string(),
            role: SystemRole::User,
            can_publish: false,
            write_access: true,
        }
    }

    /// Commit needs the actual triples → a request without inline `data.ttl`
    /// must be rejected up front, before any validator call or write.
    #[tokio::test]
    async fn missing_inline_data_is_bad_request() {
        let state = AppState::test_default_with_store(TripleStore::in_memory().unwrap());
        let body = ValidateAndCommitRequest {
            data: empty_source(),
            shapes: empty_source(),
            inference: None,
            commit_on_valid: CommitOnValid {
                target: "new".to_string(),
                dataset_id: None,
                org_id: None,
                graph: None,
                message: Some("msg".to_string()),
                version: None,
                name: None,
            },
        };
        let res = validate_and_commit(
            State(state),
            Extension(test_user()),
            HeaderMap::new(),
            Json(body),
        )
        .await;
        assert!(matches!(res, Err(AppError::BadRequest(_))));
    }
}
