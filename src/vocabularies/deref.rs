//! Per-concept dereference for SKOS concept schemes.
//!
//! `GET /api/vocabularies/:id/concept?iri=<concept IRI>` runs SPARQL
//! `DESCRIBE` over the latest published version's named graphs and returns
//! the Concise Bounded Description of the concept with content negotiation.

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
pub struct ConceptQuery {
    pub iri: String,
    pub version: Option<String>,
}

pub async fn describe_concept(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
    Query(params): Query<ConceptQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    NamedNodeRef::new(&params.iri)
        .map_err(|e| AppError::BadRequest(format!("Invalid concept IRI: {e}")))?;

    let vocab = registry::get_vocabulary(&state.store, &state.base_url, &id)
        .ok_or_else(|| AppError::NotFound(format!("Vocabulary '{id}' not found")))?;

    let uid = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_ontology(
            uid,
            vocab.is_public,
            vocab.owner_type.as_deref(),
            vocab.owner_id.as_deref(),
        )
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Vocabulary '{id}' not found")));
    }

    let version = params
        .version
        .clone()
        .or_else(|| vocab.latest_published.clone())
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

    // Pull in the scheme context alongside the concept's CBD.
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
