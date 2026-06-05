//! Per-term dereference for a model's classes / properties / shapes / SKOS concepts.
//!
//! `GET /api/models/:id/term?iri=<term IRI>` runs a SPARQL `DESCRIBE`
//! over the latest published version's named graphs and returns the
//! Concise Bounded Description in the client's preferred RDF format. When the
//! term is a SKOS concept the enclosing `skos:ConceptScheme` is pulled in too;
//! for non-SKOS terms that extra clause simply binds nothing.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use oxigraph::model::NamedNodeRef;
use serde::Deserialize;

use crate::auth::middleware::AuthenticatedUser;
use crate::server::content_negotiation::{negotiate_graph_format, serialize_graph};
use crate::server::error::AppError;
use crate::server::AppState;

use super::registry;

#[derive(Debug, Deserialize)]
pub struct TermQuery {
    pub iri: String,
    /// Optional explicit version; defaults to the latest published version.
    pub version: Option<String>,
}

pub async fn describe_term(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
    Query(params): Query<TermQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    // Validate term IRI before letting it near a SPARQL string.
    NamedNodeRef::new(&params.iri)
        .map_err(|e| AppError::BadRequest(format!("Invalid term IRI: {e}")))?;

    let data_model = registry::get_data_model(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Data model '{id}' not found")))?;

    let uid = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_ontology(
            uid,
            data_model.is_public,
            data_model.owner_type.as_deref(),
            data_model.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Data model '{id}' not found")));
    }

    let version = params
        .version
        .clone()
        .or_else(|| data_model.latest_published.clone())
        .ok_or_else(|| AppError::NotFound("No published version exists".to_string()))?;

    let record = registry::get_version(&state.store, &state.base_url, &id, &version)
        .ok_or_else(|| AppError::NotFound(format!("Version '{version}' not found")))?;

    let graphs: Vec<String> = if record.sub_graphs.is_empty() {
        vec![record.graph_iri.clone()]
    } else {
        record.sub_graphs.clone()
    };

    let from_clauses: String = graphs
        .iter()
        .map(|g| format!("FROM <{g}>"))
        .collect::<Vec<_>>()
        .join("\n");

    // Describe the term and, for SKOS concepts, the scheme it belongs to. The
    // OPTIONAL is inert for ontology classes/properties (it binds nothing).
    let q = format!(
        r#"
        DESCRIBE <{iri}> ?scheme
        {from_clauses}
        WHERE {{
          OPTIONAL {{ <{iri}> <http://www.w3.org/2004/02/skos/core#inScheme> ?scheme }}
        }}
        "#,
        iri = params.iri,
    );
    let results = state
        .store
        .query(&q)
        .map_err(|e| AppError::Internal(format!("DESCRIBE failed: {e}")))?;

    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");
    let format = negotiate_graph_format(accept);
    let body = serialize_graph(results, format)
        .map_err(|e| AppError::Internal(format!("Serialization failed: {e}")))?;

    let mut resp = Response::new(axum::body::Body::from(body));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(format.content_type()),
    );
    Ok(resp)
}
