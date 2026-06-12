//! Shared HTTP test harness for API/protocol conformance suites.
//!
//! Builds an in-memory `AppState` and drives the real Axum router via
//! `tower::ServiceExt::oneshot` (no network bind). Mirrors the proven harness in
//! `api_comprehensive_test.rs`.
#![allow(dead_code)]

use std::sync::Arc;

use axum::body::Body;
use axum::Router;
use http_body_util::BodyExt as _;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::Value;

use open_triplestore::{
    auth::{
        db::AuthDb,
        jwt::{issue_access_token, JwtConfig},
        models::SystemRole,
        oauth::new_session_store,
    },
    prefixes::PrefixRegistry,
    server::{build_router, AppState},
    storage::ObjectStore,
    store::TripleStore,
};

pub const JWT_SECRET: &str = "test_secret_must_be_32_chars_abcd";

/// Fresh in-memory `AppState`.
pub fn test_state() -> AppState {
    let auth_db = Arc::new(AuthDb::in_memory().unwrap());
    let audit = Arc::new(open_triplestore::auth::audit::AuditLogger::new(
        auth_db.pool(),
    ));
    AppState {
        store: TripleStore::in_memory().unwrap(),
        prefix_registry: Arc::new(PrefixRegistry::empty()),
        auth_db,
        audit,
        backup: None,
        jwt_config: Arc::new(JwtConfig::new(JWT_SECRET.to_string(), 30, 30)),
        object_store: Arc::new(ObjectStore::noop()),
        mailer: Arc::new(open_triplestore::email::Mailer::log_only("http://localhost:7878")),
        base_url: Arc::new("http://localhost:7878".to_string()),
        oauth_sessions: new_session_store(),
        passkey_sessions: open_triplestore::auth::passkey::new_session_store(),
        auth_ext: Arc::new(open_triplestore::auth::oidc_rs::AuthExt::disabled()),
        query_timeout_secs: 30,
        secure_cookies: false,
        browse_semaphore: Arc::new(tokio::sync::Semaphore::new(64)),
        expensive_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
        #[cfg(feature = "text-search")]
        text_index: None,
        #[cfg(feature = "text-search")]
        text_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    }
}

pub fn test_app(state: AppState) -> Router {
    build_router(state, "", vec![])
}

pub fn mint_token(user_id: &str, username: &str, role: &str) -> String {
    issue_access_token(
        &JwtConfig::new(JWT_SECRET.to_string(), 30, 30),
        user_id,
        username,
        role,
    )
    .unwrap()
}

/// `(state, super_admin_token)` — the user `adm` is created as a SuperAdmin.
pub fn admin_state() -> (AppState, String) {
    let state = test_state();
    state
        .auth_db
        .create_user(
            "adm",
            "admin",
            "admin@test.com",
            "hash",
            SystemRole::SuperAdmin,
        )
        .unwrap();
    let token = mint_token("adm", "admin", "super_admin");
    (state, token)
}

pub async fn body_text(body: Body) -> String {
    let bytes = body.collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub async fn body_json(body: Body) -> Value {
    let text = body_text(body).await;
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

pub fn url_encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}
