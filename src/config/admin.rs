//! The admin endpoints over the configuration: `GET /api/admin/config` (the
//! effective settings) and `/api/admin/settings` (the runtime ones).

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::Deserialize;

use crate::auth::audit::{self, AuditEventBuilder, AuditEventType, AuditOutcome};
use crate::auth::middleware::AuthenticatedUser;
use crate::server::client_ip::ClientIp;
use crate::server::AppState;

/// Every setting with its effective value and where it came from (`cli`,
/// `env`, `file`, `profile`, `default`, or `admin` for a runtime override).
/// Secret values are redacted; a secret given as a reference shows the
/// reference, which is no secret.
pub async fn admin_get_config(State(state): State<AppState>) -> Json<serde_json::Value> {
    let config = &state.config;
    let db = state.auth_db.clone();
    Json(serde_json::json!({
        "profile": if config.production() { "production" } else { "development" },
        "settings": config.describe_with(|spec| super::runtime::stored(&db, spec)),
        "warnings": config.warnings,
    }))
}

/// `GET /api/admin/settings`: the settings an administrator can change at
/// runtime, each with its effective value, what the configuration says
/// without an override, and whether one is set.
pub async fn admin_list_settings(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "settings": super::runtime::list(&state.auth_db, &state.config),
    }))
}

#[derive(Deserialize)]
pub struct SettingUpdate {
    /// The new value, validated as the environment's would be; `null`
    /// removes the override.
    pub value: Option<String>,
}

/// `PUT /api/admin/settings/{name}`: override a runtime setting, or with
/// `{"value": null}` remove the override so the configured value applies.
/// Audit-logged.
pub async fn admin_set_setting(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(name): Path<String>,
    client_ip: ClientIp,
    headers: HeaderMap,
    Json(req): Json<SettingUpdate>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use super::runtime::SetError;
    let stored =
        super::runtime::set(&state.auth_db, &name, req.value.as_deref()).map_err(|e| match e {
            SetError::NotRuntime(_) => (StatusCode::NOT_FOUND, e.to_string()),
            SetError::Invalid(_) => (StatusCode::BAD_REQUEST, e.to_string()),
            SetError::Storage(_) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        })?;
    let mut b = AuditEventBuilder::new(AuditEventType::SettingChanged, AuditOutcome::Success)
        .actor_id(&current_user.user_id)
        .resource("setting", &name)
        .details(serde_json::json!({ "value": stored, "reset": stored.is_none() }));
    b.ip_address = client_ip.as_string();
    b.user_agent = audit::user_agent(&headers);
    b.request_id = audit::request_id_from_headers(&headers);
    state.audit.log(b);
    let setting = super::runtime::list(&state.auth_db, &state.config)
        .into_iter()
        .find(|s| s.name == name);
    Ok(Json(serde_json::json!({ "setting": setting })))
}
