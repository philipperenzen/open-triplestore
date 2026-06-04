//! vocabulary versioning API.

use axum::routing::{delete, get, post, patch};
use axum::Router;

use crate::server::AppState;
use super::handlers;

/// Public (read-only) routes — served with optional_auth so visibility scoping works.
pub fn vocabulary_public_routes() -> Router<AppState> {
    Router::new()
        .route("/api/vocabularies", get(handlers::list_vocabularies))
        .route("/api/vocabularies/:id", get(handlers::get_vocabulary))
        .route("/api/vocabularies/:id/versions", get(handlers::list_versions))
        .route(
            "/api/vocabularies/:id/versions/:ver",
            get(handlers::get_version),
        )
        .route(
            "/api/vocabularies/:id/versions/:ver/data",
            get(handlers::get_version_data),
        )
        .route(
            "/api/vocabularies/:id/latest/data",
            get(handlers::get_latest_data),
        )
        .route("/api/vocabularies/:id/diff", get(handlers::diff_versions))
        .route("/api/vocabularies/:id/collaborators", get(handlers::list_collaborators))
        .route("/api/vocabularies/:id/commits", get(handlers::list_commits))
        .route("/api/vocabularies/:id/branches", get(handlers::list_branches).post(handlers::create_branch))
        .route("/api/vocabularies/:id/merge/preview", get(handlers::merge_preview))
        .route("/api/vocabularies/:id/concept", get(super::deref::describe_concept))
}

/// Write routes — require authentication; fine-grained role checks are done inside handlers.
pub fn vocabulary_auth_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/vocabularies",
            post(handlers::create_vocabulary),
        )
        .route(
            "/api/vocabularies/:id",
            delete(handlers::delete_vocabulary).patch(handlers::update_vocabulary),
        )
        .route(
            "/api/vocabularies/:id/versions",
            post(handlers::upload_version),
        )
        .route(
            "/api/vocabularies/:id/versions/:ver",
            patch(handlers::update_version_notes),
        )
        .route(
            "/api/vocabularies/:id/versions/:ver/data",
            patch(handlers::patch_version_data),
        )
        .route(
            "/api/vocabularies/:id/versions/:ver/draft",
            post(handlers::create_draft),
        )
        .route(
            "/api/vocabularies/:id/versions/:ver/rebase",
            post(handlers::rebase_version),
        )
        .route(
            "/api/vocabularies/:id/merge",
            post(handlers::merge_apply),
        )
        .route(
            "/api/vocabularies/:id/versions/:ver/stage",
            post(handlers::stage_version),
        )
        .route(
            "/api/vocabularies/:id/versions/:ver/publish",
            post(handlers::publish_version),
        )
        .route(
            "/api/vocabularies/:id/versions/:ver/deprecate",
            post(handlers::deprecate_version),
        )
        .route(
            "/api/vocabularies/:id/versions/:ver/subgraph/stage",
            post(handlers::stage_sub_graph),
        )
        .route(
            "/api/vocabularies/:id/versions/:ver/subgraph/publish",
            post(handlers::publish_sub_graph),
        )
        .route(
            "/api/vocabularies/:id/versions/:ver/subgraph/deprecate",
            post(handlers::deprecate_sub_graph),
        )
}
