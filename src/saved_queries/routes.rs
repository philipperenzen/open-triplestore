//! Route tables for saved queries, split into read routes (mounted with
//! `optional_auth` so anonymous callers can reach public scopes) and management
//! routes (mounted with `require_auth`).
//!
//! Group-scoped queries are exposed at a top-level `/api/groups/:group_id/…`
//! (group ids are globally unique) to keep the path shape uniform across scopes.

use axum::routing::{get, post, put};
use axum::Router;

use super::handlers::{dataset, group, org};
use crate::server::AppState;

/// Read + run routes — `optional_auth`; handlers enforce per-scope read access.
pub fn saved_query_public_routes() -> Router<AppState> {
    Router::new()
        // ── Dataset scope ──
        .route("/api/datasets/:dataset_id/api-services", get(dataset::list))
        .route("/api/datasets/:dataset_id/api-services/:slug", get(dataset::get))
        .route("/api/datasets/:dataset_id/api-services/:slug/revisions", get(dataset::revisions))
        .route("/api/datasets/:dataset_id/api-services/:slug/tests", get(dataset::tests))
        .route(
            "/api/datasets/:dataset_id/api-services/:slug/run",
            get(dataset::run_get).post(dataset::run_post),
        )
        .route("/api/datasets/:dataset_id/openapi.json", get(dataset::openapi))
        // ── Organisation scope ──
        .route("/api/organisations/:org_id/api-services", get(org::list))
        .route("/api/organisations/:org_id/api-services/:slug", get(org::get))
        .route("/api/organisations/:org_id/api-services/:slug/revisions", get(org::revisions))
        .route("/api/organisations/:org_id/api-services/:slug/tests", get(org::tests))
        .route(
            "/api/organisations/:org_id/api-services/:slug/run",
            get(org::run_get).post(org::run_post),
        )
        .route("/api/organisations/:org_id/openapi.json", get(org::openapi))
        // ── Group scope ──
        .route("/api/groups/:group_id/api-services", get(group::list))
        .route("/api/groups/:group_id/api-services/:slug", get(group::get))
        .route("/api/groups/:group_id/api-services/:slug/revisions", get(group::revisions))
        .route("/api/groups/:group_id/api-services/:slug/tests", get(group::tests))
        .route(
            "/api/groups/:group_id/api-services/:slug/run",
            get(group::run_get).post(group::run_post),
        )
        .route("/api/groups/:group_id/openapi.json", get(group::openapi))
}

/// Management routes — `require_auth`; handlers enforce owner-admin + write scope.
pub fn saved_query_auth_routes() -> Router<AppState> {
    Router::new()
        // ── Dataset scope ──
        .route("/api/datasets/:dataset_id/api-services", post(dataset::create))
        .route(
            "/api/datasets/:dataset_id/api-services/:slug",
            put(dataset::update).delete(dataset::delete),
        )
        .route("/api/datasets/:dataset_id/api-services/:slug/repair", post(dataset::repair))
        .route(
            "/api/datasets/:dataset_id/api-services/:slug/tests/:test_id/ack",
            post(dataset::ack),
        )
        // ── Organisation scope ──
        .route("/api/organisations/:org_id/api-services", post(org::create))
        .route(
            "/api/organisations/:org_id/api-services/:slug",
            put(org::update).delete(org::delete),
        )
        .route("/api/organisations/:org_id/api-services/:slug/repair", post(org::repair))
        .route(
            "/api/organisations/:org_id/api-services/:slug/tests/:test_id/ack",
            post(org::ack),
        )
        // ── Group scope ──
        .route("/api/groups/:group_id/api-services", post(group::create))
        .route(
            "/api/groups/:group_id/api-services/:slug",
            put(group::update).delete(group::delete),
        )
        .route("/api/groups/:group_id/api-services/:slug/repair", post(group::repair))
        .route(
            "/api/groups/:group_id/api-services/:slug/tests/:test_id/ack",
            post(group::ack),
        )
}
