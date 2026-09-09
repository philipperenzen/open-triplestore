//! A dataset's *conformance layer*: which of its graphs play which role, which
//! model version and shape graphs it declares conformance to
//! (`dct:conformsTo` in the DCAT catalogue), and — derived from that — which
//! graphs reasoning should read and which shapes validation should apply.
//!
//! This is the TBox/ABox separation made explicit and domain-neutral: an
//! instance dataset points at the model layer it conforms to, and the
//! reasoners and validators work on *that* layer instead of the whole store.
//! Nothing here knows about any particular domain profile.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Serialize;

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{Dataset, GraphKind};
use crate::server::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct GraphLayer {
    pub graph_iri: String,
    pub role: Option<GraphKind>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelLayer {
    pub id: String,
    pub version: String,
    pub status: String,
    pub graph_iri: String,
    pub sub_graphs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShapeLayer {
    pub id: String,
    pub name: String,
    pub graph_iri: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConformanceLayer {
    pub dataset_id: String,
    pub dataset_role: Option<GraphKind>,
    pub graphs: Vec<GraphLayer>,
    /// The model version this dataset declares conformance to, resolved from
    /// `conforms_to_model` / `conforms_to_version` (the model's latest
    /// published version when no version is declared).
    pub conforms_to_model: Option<ModelLayer>,
    /// Declared but unresolvable (unknown model, unknown version, or no
    /// published version yet) — surfaced rather than silently dropped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unresolved_model: Option<String>,
    /// The shape graphs bound to the dataset (dataset-level and inherited
    /// per-graph bindings).
    pub shape_graphs: Vec<ShapeLayer>,
    /// What a reasoner should read for this dataset: its data-bearing graphs
    /// (instances, model, vocabulary, domain values, linksets, unclassified)
    /// plus the conformed model version's graphs. Shapes, entailment, system,
    /// catalog and provenance graphs are not premises.
    pub reasoning_sources: Vec<String>,
    /// What validation should apply: the bound shape graphs' IRIs.
    pub validation_shapes: Vec<String>,
}

/// Graph roles that carry premises for reasoning. Unclassified graphs count:
/// most datasets never set a role.
fn is_reasoning_source(role: Option<GraphKind>) -> bool {
    matches!(
        role,
        None | Some(
            GraphKind::Instances
                | GraphKind::Model
                | GraphKind::Vocabulary
                | GraphKind::DomainValues
                | GraphKind::Linkset
        )
    )
}

/// Resolve the conformance layer of `ds`. Never fails: an unreadable registry
/// yields an empty layer, an unresolvable model is reported in
/// `unresolved_model`.
pub fn resolve(state: &AppState, ds: &Dataset) -> ConformanceLayer {
    let base = state.base_url.as_str();
    let entries = state
        .auth_db
        .list_dataset_graph_entries(&ds.id)
        .unwrap_or_default();
    let graphs: Vec<GraphLayer> = entries
        .iter()
        .map(|e| GraphLayer {
            graph_iri: e.graph_iri.clone(),
            role: e.graph_role,
        })
        .collect();
    let mut sources: Vec<String> = entries
        .iter()
        .filter(|e| is_reasoning_source(e.graph_role))
        .map(|e| e.graph_iri.clone())
        .collect();

    let mut model = None;
    let mut unresolved = None;
    if let Some(mid) = ds.conforms_to_model.as_deref().filter(|s| !s.is_empty()) {
        let record = crate::data_models::registry::get_data_model(&state.store, base, mid);
        let version = ds
            .conforms_to_version
            .clone()
            .filter(|v| !v.is_empty())
            .or_else(|| record.as_ref().and_then(|r| r.latest_published.clone()));
        let resolved = version
            .as_deref()
            .and_then(|v| crate::data_models::registry::get_version(&state.store, base, mid, v));
        match resolved {
            Some(v) => {
                sources.push(v.graph_iri.clone());
                sources.extend(v.sub_graphs.iter().cloned());
                let status = serde_json::to_value(v.status)
                    .ok()
                    .and_then(|x| x.as_str().map(str::to_string))
                    .unwrap_or_default();
                model = Some(ModelLayer {
                    id: mid.to_string(),
                    version: v.version.clone(),
                    status,
                    graph_iri: v.graph_iri.clone(),
                    sub_graphs: v.sub_graphs.clone(),
                });
            }
            None => {
                unresolved = Some(match version {
                    Some(v) => format!("{mid}@{v}"),
                    None => format!("{mid} (no published version)"),
                });
            }
        }
    }

    let studio = crate::shacl_studio::store::ShaclStudioStore::new(state.auth_db.pool());
    let shapes = crate::shacl_studio::bindings::effective_shape_graphs_for_dataset(
        &state.store,
        &state.auth_db,
        &studio,
        base,
        ds,
    );

    sources.sort();
    sources.dedup();
    ConformanceLayer {
        dataset_id: ds.id.clone(),
        dataset_role: ds.graph_role,
        graphs,
        conforms_to_model: model,
        unresolved_model: unresolved,
        shape_graphs: shapes
            .iter()
            .map(|s| ShapeLayer {
                id: s.id.clone(),
                name: s.name.clone(),
                graph_iri: s.graph_iri.clone(),
            })
            .collect(),
        validation_shapes: shapes.iter().map(|s| s.graph_iri.clone()).collect(),
        reasoning_sources: sources,
    }
}

/// Whether `iri` is a graph of a data-model version the user may read: the
/// version graph `{base}/data-model/{id}/version/{ver}` or one of its
/// sub-graphs. Model graphs live in the model registry, not in a dataset, so
/// the dataset-graph read set never contains them; this is the registry's own
/// visibility rule (public, or owned by the caller / their organisation).
pub fn model_graph_readable(state: &AppState, user_id: Option<&str>, iri: &str) -> bool {
    let base = state.base_url.trim_end_matches('/');
    let Some(rest) = iri.strip_prefix(&format!("{base}/data-model/")) else {
        return false;
    };
    let Some(model_id) = rest.split('/').next().filter(|s| !s.is_empty()) else {
        return false;
    };
    let Some(model) = crate::data_models::registry::get_data_model(&state.store, base, model_id)
    else {
        return false;
    };
    state
        .auth_db
        .can_access_ontology(
            user_id,
            model.is_public,
            model.owner_type.as_deref(),
            model.owner_id.as_deref(),
        )
        .unwrap_or(false)
}

/// GET /api/datasets/:dataset_id/conformance
pub async fn get_dataset_conformance(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ds = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let visible = state
        .auth_db
        .can_access_dataset(uid, &ds)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !visible {
        // Same answer as for a dataset that does not exist: no existence leak.
        return Err((StatusCode::NOT_FOUND, "Dataset not found".to_string()));
    }
    Ok(Json(resolve(&state, &ds)))
}
