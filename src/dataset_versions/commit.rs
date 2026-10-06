//! Validate-and-commit orchestration (Unified Accounts plan, Phase 4).
//!
//! `POST /api/datasets/validate-and-commit` runs SHACL validation on an external
//! validation service (forwarding the caller's bearer token, on-behalf-of) and,
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
use super::{registry, reports};

/// A graph supplied inline (`ttl`) or by OTS reference — forwarded verbatim to
/// the external validation service, whose contract uses these exact (snake_case) fields.
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

/// Call the external validation service's `/validate`, forwarding the caller's token.
async fn run_validation(
    bearer: &str,
    body: &ValidateAndCommitRequest,
) -> Result<ValidatorResponse, AppError> {
    let base = std::env::var("VALIDATION_API_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            AppError::BadRequest(
                "External validation service not configured (set VALIDATION_API_URL)".to_string(),
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

/// Refuse a commit target graph that belongs to another dataset, to the
/// system or to the model registry. Both commit branches register the
/// caller-supplied graph and then `graph_store_put` (replace) it, so an
/// unchecked graph name is a whole-graph overwrite of whoever owns it. A
/// model-registry graph is refused for everyone, admins included (this path
/// runs none of the registry's licence checks); admins are otherwise
/// unrestricted, matching the same gate on the mapping-execution path in
/// `server::routes`.
fn authorize_target_graph(
    state: &AppState,
    user: &AuthenticatedUser,
    dataset_id: &str,
    graph_iri: &str,
) -> Result<crate::auth::dataset_graph::GraphClaim, AppError> {
    crate::auth::dataset_graph::gate_dataset_graph_target(
        &state.store,
        &state.auth_db,
        &state.base_url,
        dataset_id,
        graph_iri,
        user,
    )
    .map_err(AppError::Forbidden)
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
            // A brand-new dataset may not claim a graph another dataset already
            // owns: the graph is REPLACED by the graph_store_put below, so
            // without this the "create a dataset, name someone else's graph"
            // path is a whole-graph overwrite.
            let claim = authorize_target_graph(&state, &user, &ds_id, &graph_iri)?;
            crate::auth::dataset_graph::register_claimed_graph(
                &state.auth_db,
                &ds_id,
                &graph_iri,
                claim,
            )
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
                // Never a graph the server keeps for the dataset's history
                // (statement provenance, property states): the commit
                // replaces the graph it lands in.
                None => registered
                    .iter()
                    .find(|g| {
                        !crate::provenance::is_any_statement_provenance_graph(g)
                            && **g != crate::property_states::states_graph(&ds_id)
                    })
                    .cloned()
                    .unwrap_or_else(|| {
                        format!("{}/dataset/{}/graph/imported", state.base_url, ds_id)
                    }),
            };
            if !registered.contains(&graph_iri) {
                // The caller-supplied graph was accepted verbatim, registered to
                // this dataset, and then REPLACED by the graph_store_put below —
                // the register-then-overwrite bypass that every other write path
                // gates. `can_write_dataset` above only proves the caller owns
                // *this* dataset, not that the graph is theirs to claim.
                let claim = authorize_target_graph(&state, &user, &ds_id, &graph_iri)?;
                crate::auth::dataset_graph::register_claimed_graph(
                    &state.auth_db,
                    &ds_id,
                    &graph_iri,
                    claim,
                )
                .map_err(|e| AppError::Internal(e.to_string()))?;
            } else {
                // Already registered: still never replace a model-registry graph
                // (a registration made before such graphs were refused).
                crate::auth::dataset_graph::refuse_model_registry_graph(
                    &state.store,
                    &state.base_url,
                    &ds_id,
                    &graph_iri,
                )
                .map_err(AppError::Forbidden)?;
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
    // The commit below is a graph_store_put, i.e. a replace — so the payload is
    // the graph's whole future state.
    crate::server::routes::validate_on_write(
        &state,
        Some(&user),
        Some(&graph_iri),
        &data_ttl,
        RdfFormat::Turtle,
        crate::shacl_studio::gate::WriteMode::Replace,
    )?;

    // 4. Import (replace) the validated data into the target graph.
    state
        .store
        .graph_store_put(Some(&graph_iri), &data_ttl, RdfFormat::Turtle)
        .map_err(|e| AppError::Internal(format!("import failed: {e}")))?;
    #[cfg(feature = "text-search")]
    state.mark_text_dirty();

    // 5. Cut a new draft version carrying the commit message: a delta when
    //    one can be chained from a checkpoint, a checkpoint otherwise.
    let requested = c.version.clone().filter(|s| !s.is_empty());
    let source_graphs = vec![graph_iri.clone()];
    let version_lock = crate::dataset_versions::lock::dataset(&dataset.id);
    let _held = version_lock.lock().await;
    let version = match requested {
        Some(v) => {
            if registry::version_exists(&state.store, &state.base_url, &dataset.id, &v) {
                return Err(AppError::BadRequest(format!(
                    "Version '{v}' already exists"
                )));
            }
            v
        }
        // A timestamp label is per second: a second commit within it gets a
        // suffix instead of overwriting the first one's snapshot.
        None => {
            let stamp = Utc::now().format("%Y%m%d%H%M%S").to_string();
            let mut label = stamp.clone();
            let mut n = 1;
            while registry::version_exists(&state.store, &state.base_url, &dataset.id, &label) {
                label = format!("{stamp}-{n}");
                n += 1;
            }
            label
        }
    };
    let req = crate::dataset_versions::cut::CutRequest {
        dataset_id: dataset.id.clone(),
        version: version.clone(),
        source_graphs: source_graphs.clone(),
        status: VersionStatus::Draft,
        created_by: Some(format!("{}/users/{}", state.base_url, user.user_id)),
        notes: c.message.clone(),
        branch: None,
        derived_from: None,
        force_checkpoint: false,
        shared: crate::dataset_versions::cut::shared_graphs(
            &state.auth_db,
            &dataset.id,
            &source_graphs,
        ),
        conforms_to_model: dataset.conforms_to_model.clone().filter(|m| !m.is_empty()),
        conforms_to_version: dataset
            .conforms_to_version
            .clone()
            .filter(|v| !v.is_empty()),
    };
    let st = state.clone();
    let record: DatasetVersion = tokio::task::spawn_blocking(move || {
        let made = crate::dataset_versions::cut::cut(&st.store, &st.base_url, &req)?;
        crate::dataset_versions::after_cut(
            &st.store,
            &st.base_url,
            &made.record,
            &req.source_graphs,
        );
        Ok::<_, String>(made.record)
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .map_err(AppError::Internal)?;
    crate::rdf_patch_log::on_version_cut(
        &state.auth_db,
        &state.base_url,
        &record,
        Some(&user.user_id),
    );
    drop(_held);
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
            "versionKind": record.kind,
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
            can_mint_api_tokens: true,
            scopes: Vec::new(),
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
