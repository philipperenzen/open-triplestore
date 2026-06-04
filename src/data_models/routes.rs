//! data-model versioning API.

use axum::routing::{delete, get, patch, post};
use axum::Router;

use super::handlers;
use crate::server::AppState;

/// Public (read-only) routes — served with optional_auth so visibility scoping works.
pub fn data_model_public_routes() -> Router<AppState> {
    Router::new()
        .route("/api/data-models", get(handlers::list_data_models))
        .route("/api/data-models/:id", get(handlers::get_data_model))
        .route(
            "/api/data-models/:id/versions",
            get(handlers::list_versions),
        )
        .route(
            "/api/data-models/:id/versions/:ver",
            get(handlers::get_version),
        )
        .route(
            "/api/data-models/:id/versions/:ver/data",
            get(handlers::get_version_data),
        )
        .route(
            "/api/data-models/:id/latest/data",
            get(handlers::get_latest_data),
        )
        .route("/api/data-models/:id/diff", get(handlers::diff_versions))
        .route(
            "/api/data-models/:id/collaborators",
            get(handlers::list_collaborators),
        )
        .route("/api/data-models/:id/commits", get(handlers::list_commits))
        .route(
            "/api/data-models/:id/branches",
            get(handlers::list_branches).post(handlers::create_branch),
        )
        .route(
            "/api/data-models/:id/merge/preview",
            get(handlers::merge_preview),
        )
        .route(
            "/api/data-models/:id/term",
            get(super::deref::describe_term),
        )
}

/// Write routes — require authentication; fine-grained role checks are done inside handlers.
pub fn data_model_auth_routes() -> Router<AppState> {
    Router::new()
        .route("/api/data-models", post(handlers::create_data_model))
        .route(
            "/api/data-models/:id",
            delete(handlers::delete_data_model).patch(handlers::update_data_model),
        )
        .route(
            "/api/data-models/:id/versions",
            post(handlers::upload_version),
        )
        .route(
            "/api/data-models/:id/versions/:ver",
            patch(handlers::update_version_notes),
        )
        .route(
            "/api/data-models/:id/versions/:ver/data",
            patch(handlers::patch_version_data),
        )
        .route(
            "/api/data-models/:id/versions/:ver/draft",
            post(handlers::create_draft),
        )
        .route(
            "/api/data-models/:id/versions/:ver/rebase",
            post(handlers::rebase_version),
        )
        .route("/api/data-models/:id/merge", post(handlers::merge_apply))
        .route(
            "/api/data-models/:id/versions/:ver/stage",
            post(handlers::stage_version),
        )
        .route(
            "/api/data-models/:id/versions/:ver/publish",
            post(handlers::publish_version),
        )
        .route(
            "/api/data-models/:id/versions/:ver/deprecate",
            post(handlers::deprecate_version),
        )
        .route(
            "/api/data-models/:id/versions/:ver/subgraph/stage",
            post(handlers::stage_sub_graph),
        )
        .route(
            "/api/data-models/:id/versions/:ver/subgraph/publish",
            post(handlers::publish_sub_graph),
        )
        .route(
            "/api/data-models/:id/versions/:ver/subgraph/deprecate",
            post(handlers::deprecate_sub_graph),
        )
}
