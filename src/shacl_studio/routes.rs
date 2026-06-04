//! Router builders for SHACL Studio. The caller (`server::mod::build_router`)
//! wraps each in the appropriate auth middleware: most endpoints require auth;
//! the `form-manifest` route uses optional auth so form platform can load public
//! datasets without a token.

use axum::routing::{get, post};
use axum::Router;

use crate::server::AppState;

use super::handlers;

/// Endpoints that require authentication: shape graphs, pipelines, runs,
/// model-context, derive.
pub fn studio_auth_routes() -> Router<AppState> {
    Router::new()
        // Shape graphs
        .route(
            "/api/shacl/shape-graphs",
            get(handlers::list_shape_graphs).post(handlers::create_shape_graph),
        )
        .route(
            "/api/shacl/shape-graphs/:id",
            get(handlers::get_shape_graph)
                .put(handlers::update_shape_graph)
                .delete(handlers::delete_shape_graph),
        )
        .route(
            "/api/shacl/shape-graphs/:id/turtle",
            get(handlers::get_shape_graph_turtle).put(handlers::put_shape_graph_turtle),
        )
        .route(
            "/api/shacl/shape-graphs/:id/revisions",
            get(handlers::list_shape_graph_revisions),
        )
        .route(
            "/api/shacl/shape-graphs/:id/revisions/:rev",
            get(handlers::get_shape_graph_revision),
        )
        .route(
            "/api/shacl/shape-graphs/:id/restore/:rev",
            post(handlers::restore_shape_graph_revision),
        )
        .route(
            "/api/shacl/shape-graphs/:id/clone",
            post(handlers::clone_shape_graph),
        )
        .route(
            "/api/shacl/shape-graphs/:id/import-shapes",
            post(handlers::import_shapes),
        )
        .route(
            "/api/shacl/shape-graphs/:id/validate",
            post(handlers::validate_shape_graph),
        )
        .route(
            "/api/shacl/shape-graphs/:id/commits",
            get(handlers::list_shape_graph_commits),
        )
        .route(
            "/api/shacl/shape-graphs/:id/stage",
            post(handlers::stage_shape_graph),
        )
        .route(
            "/api/shacl/shape-graphs/:id/publish",
            post(handlers::publish_shape_graph),
        )
        .route(
            "/api/shacl/shape-graphs/:id/deprecate",
            post(handlers::deprecate_shape_graph),
        )
        // Shapes catalog (discover shapes across all graphs) + adopt-in-place
        .route("/api/shacl/shapes", get(handlers::list_shapes_catalog))
        .route(
            "/api/shacl/register-shape-graph",
            post(handlers::register_shape_graph),
        )
        // Bindings (the validation layer)
        .route(
            "/api/shacl/bindings",
            get(handlers::list_bindings)
                .post(handlers::create_binding)
                .delete(handlers::delete_binding),
        )
        .route(
            "/api/datasets/:id/effective-shapes",
            get(handlers::dataset_effective_shapes),
        )
        // Pipelines
        .route(
            "/api/shacl/pipelines",
            get(handlers::list_pipelines).post(handlers::create_pipeline),
        )
        .route(
            "/api/shacl/pipelines/:id",
            get(handlers::get_pipeline)
                .put(handlers::update_pipeline)
                .delete(handlers::delete_pipeline),
        )
        .route("/api/shacl/pipelines/:id/run", post(handlers::run_pipeline))
        .route(
            "/api/shacl/pipelines/:id/runs",
            get(handlers::list_pipeline_runs),
        )
        .route(
            "/api/shacl/pipelines/:id/runs/:run_id",
            get(handlers::get_pipeline_run),
        )
        .route(
            "/api/shacl/pipelines/latest",
            post(handlers::list_latest_pipeline_runs),
        )
        // Tooling
        .route("/api/shacl/model-context", get(handlers::model_context))
        .route("/api/shacl/derive", post(handlers::derive_shapes))
}

/// Endpoints using optional auth (anonymous callers may read when the dataset
/// is public): currently the form-manifest.
pub fn studio_optional_auth_routes() -> Router<AppState> {
    Router::new().route(
        "/api/datasets/:dataset_id/form-manifest",
        get(handlers::form_manifest),
    )
}
