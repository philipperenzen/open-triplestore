//! Routes for bulk import endpoints.

use axum::routing::post;
use axum::Router;

use super::handlers;
use crate::server::AppState;

pub fn bulk_import_routes() -> Router<AppState> {
    Router::new()
        .route("/api/import/bulk", post(handlers::bulk_import))
        .route("/api/import/analyze", post(handlers::analyze_import))
}
