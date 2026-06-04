//! Dataset versioning API routes.

use axum::routing::{get, post};
use axum::Router;

use super::{commit, handlers, reports, share_links};
use crate::server::AppState;

/// Read routes — served with optional_auth so visibility scoping applies.
pub fn dataset_version_public_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/datasets/:dataset_id/versions",
            get(handlers::list_versions),
        )
        .route(
            "/api/datasets/:dataset_id/versions/:ver",
            get(handlers::get_version),
        )
        .route(
            "/api/datasets/:dataset_id/versions/:ver/data",
            get(handlers::get_version_data),
        )
        .route(
            "/api/datasets/:dataset_id/branches",
            get(handlers::list_branches),
        )
        .route(
            "/api/datasets/:dataset_id/validation-reports",
            get(reports::list_reports),
        )
        .route(
            "/api/datasets/:dataset_id/validation-reports/:rid",
            get(reports::get_report),
        )
        // Redeeming a share link is intentionally public (anonymous) — that is the point.
        .route(
            "/api/share-links/redeem",
            post(share_links::redeem_share_link),
        )
}

/// Write routes — require authentication; per-dataset write checks inside handlers.
pub fn dataset_version_auth_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/datasets/validate-and-commit",
            post(commit::validate_and_commit),
        )
        .route(
            "/api/datasets/:dataset_id/share-links",
            post(share_links::mint_share_link),
        )
        .route(
            "/api/datasets/:dataset_id/versions",
            post(handlers::create_version),
        )
        .route(
            "/api/datasets/:dataset_id/versions/:ver",
            axum::routing::patch(handlers::update_version_notes),
        )
        .route(
            "/api/datasets/:dataset_id/versions/:ver/stage",
            post(handlers::stage_version),
        )
        .route(
            "/api/datasets/:dataset_id/versions/:ver/publish",
            post(handlers::publish_version),
        )
        .route(
            "/api/datasets/:dataset_id/versions/:ver/deprecate",
            post(handlers::deprecate_version),
        )
        .route(
            "/api/datasets/:dataset_id/versions/:ver/restore",
            post(handlers::restore_version),
        )
        .route(
            "/api/datasets/:dataset_id/branches",
            post(handlers::create_branch),
        )
}
