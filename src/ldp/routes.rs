//! Axum router for the LDP HTTP layer.

use axum::routing::{delete, get, head, options, patch, post, put};
use axum::Router;

use super::handler;
use crate::server::AppState;

/// Build the LDP router, to be merged under `/ldp`.
pub fn ldp_routes() -> Router<AppState> {
    Router::new()
        .route("/ldp/*path", get(handler::ldp_get))
        .route("/ldp/*path", head(handler::ldp_head))
        .route("/ldp/*path", post(handler::ldp_post))
        .route("/ldp/*path", put(handler::ldp_put))
        .route("/ldp/*path", patch(handler::ldp_patch))
        .route("/ldp/*path", delete(handler::ldp_delete))
        .route("/ldp/*path", options(handler::ldp_options))
        // Root container (no trailing wildcard path param)
        .route("/ldp/", get(handler::ldp_get))
        .route("/ldp/", post(handler::ldp_post))
        .route("/ldp/", options(handler::ldp_options_root))
}
