//! `GET /api/catalog` — DCAT catalog of published data-models and vocabularies.

use axum::extract::{Extension, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;

use crate::dcat::catalog::CatalogOptions;
use crate::server::content_negotiation::negotiate_graph_format;
use crate::server::error::AppError;
use crate::server::AppState;

use super::builder::build_registry_catalog;

pub fn catalog_routes() -> Router<AppState> {
    Router::new()
        .route("/api/catalog", get(serve_catalog))
        .route(
            "/api/public/catalog",
            get(super::public::serve_public_catalog),
        )
}

async fn serve_catalog(
    State(state): State<AppState>,
    user: Option<Extension<crate::auth::middleware::AuthenticatedUser>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    let opts = CatalogOptions::from_env(&state.base_url);
    let triples = build_registry_catalog(&opts, &state.store, &state.auth_db, user_id);

    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");
    let format = negotiate_graph_format(accept);
    let body = crate::dcat::graph::serialize(&triples, format.to_rdf_format())
        .map_err(|e| AppError::Internal(format!("Catalog serialize failed: {e}")))?;

    let mut resp = Response::new(axum::body::Body::from(body));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(format.content_type()),
    );
    Ok(resp)
}
