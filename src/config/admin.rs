//! `GET /api/admin/config`: the effective settings, for administrators.

use axum::extract::State;
use axum::Json;

use crate::server::AppState;

/// Every setting with its effective value and where it came from (`cli`,
/// `env`, `file`, `profile` or `default`). Secret values are redacted; a
/// secret given as a reference shows the reference, which is no secret.
pub async fn admin_get_config(State(state): State<AppState>) -> Json<serde_json::Value> {
    let config = &state.config;
    Json(serde_json::json!({
        "profile": if config.production() { "production" } else { "development" },
        "settings": config.describe(),
        "warnings": config.warnings,
    }))
}
