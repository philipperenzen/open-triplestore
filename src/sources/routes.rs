//! Router for the datasource registry, mapping registry and runs.
//!
//! One group, admin-gated as a whole. A datasource holds a pointer to a
//! production credential and its runs write instance data, so there is no
//! read-only tier here: `GET /api/sources` already tells a caller which
//! databases this deployment reaches.

use axum::routing::{get, post};
use axum::Router;

use crate::server::AppState;

use super::handlers;

pub fn source_routes() -> Router<AppState> {
    Router::new()
        // `/metrics` and `/test` are static siblings of `/:id`; the router
        // prefers the literal segment, so a datasource may still be called
        // "metrics" without shadowing them.
        .route("/api/sources/metrics", get(handlers::source_metrics))
        .route("/api/sources/test", post(handlers::test_source))
        .route(
            "/api/sources",
            get(handlers::list_sources).post(handlers::create_source),
        )
        .route(
            "/api/sources/:id",
            get(handlers::get_source)
                .put(handlers::update_source)
                .delete(handlers::delete_source),
        )
        .route(
            "/api/sources/:id/introspect",
            get(handlers::introspect_source),
        )
        .route("/api/sources/:id/preview", get(handlers::preview_source))
        .route(
            "/api/sources/:id/provenance",
            get(handlers::source_provenance),
        )
        .route(
            "/api/sources/:id/runs",
            get(handlers::list_source_runs).post(handlers::create_run),
        )
        .route(
            "/api/mappings",
            get(handlers::list_mappings).post(handlers::create_mapping),
        )
        .route(
            "/api/mappings/:id",
            get(handlers::get_mapping)
                .put(handlers::update_mapping)
                .delete(handlers::delete_mapping),
        )
        .route("/api/mappings/:id/rml", get(handlers::get_mapping_rml))
        .route("/api/runs/:id", get(handlers::get_run).delete(handlers::delete_run))
        .route("/api/runs/:id/provenance", get(handlers::run_provenance))
        .route("/api/runs/:id/rollback", post(handlers::rollback_run))
}
