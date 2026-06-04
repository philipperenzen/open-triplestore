//! `GET /api/catalog` — DCAT catalog of published data-models and vocabularies.

use axum::extract::{Extension, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use oxigraph::io::{RdfFormat, RdfParser, RdfSerializer};
use std::io::BufReader;

use crate::server::content_negotiation::{negotiate_graph_format, GraphFormat};
use crate::server::error::AppError;
use crate::server::AppState;

use super::builder::build_catalog_turtle;

pub fn catalog_routes() -> Router<AppState> {
    Router::new()
        .route("/api/catalog", get(serve_catalog))
        .route("/api/public/catalog", get(super::public::serve_public_catalog))
}

async fn serve_catalog(
    State(state): State<AppState>,
    user: Option<Extension<crate::auth::middleware::AuthenticatedUser>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    let turtle = build_catalog_turtle(&state.store, &state.base_url, &state.auth_db, user_id);

    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");
    let format = negotiate_graph_format(accept);

    let body = if matches!(format, GraphFormat::Turtle) {
        turtle.into_bytes()
    } else {
        // Reparse the Turtle and re-serialize to the negotiated format.
        let parser = RdfParser::from_format(RdfFormat::Turtle);
        let mut serializer = RdfSerializer::from_format(format.to_rdf_format())
            .for_writer(Vec::<u8>::new());
        for q in parser.for_reader(BufReader::new(turtle.as_bytes())) {
            let q = q.map_err(|e| AppError::Internal(format!("Catalog reparse failed: {e}")))?;
            serializer
                .serialize_quad(&q)
                .map_err(|e| AppError::Internal(format!("Catalog serialize failed: {e}")))?;
        }
        serializer
            .finish()
            .map_err(|e| AppError::Internal(format!("Catalog finish failed: {e}")))?
    };

    let mut resp = Response::new(axum::body::Body::from(body));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(format.content_type()),
    );
    Ok(resp)
}
