//! The production posture (`OTS_ENV=production`) is fail-closed: a raw secret
//! in an API body or in configuration is refused, a statement timeout is
//! mandatory, a networked datasource must be on the egress allowlist, and a
//! SQLite datasource must live under `OTS_SOURCES_DIR`.
//!
//! Kept in its own test binary: the posture is read from the process
//! environment, so it cannot be toggled per test inside `sources_http`.

mod common;

use std::path::PathBuf;
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt as _;

fn setup() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::set_var("OTS_ENV", "production");
        std::env::set_var("OTS_TEST_DB_PASSWORD", "hunter2-hunter2");
        std::env::set_var("OTS_TEST_CLIENT_SECRET", "oidc-client-secret-value");
        let dir = std::env::temp_dir().join(format!("ots-sources-prod-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("OTS_SOURCES_DIR", &dir);
        let db = dir.join("assets.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT); INSERT INTO t VALUES (1, 'a');")
            .unwrap();
        dir
    })
}

async fn req(app: &Router, method: Method, uri: &str, token: &str, body: Value) -> (StatusCode, Value, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = if body.is_null() {
        Body::empty()
    } else {
        b = b.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

fn sqlite_body(id: &str, db: &PathBuf, credential: &str) -> Value {
    json!({
        "id": id, "name": id, "dialect": "sqlite",
        "database": db.to_string_lossy(),
        "credential": credential,
        "readOnly": true, "statementTimeoutMs": 5000,
    })
}

#[tokio::test]
async fn raw_password_in_a_production_registration_is_a_400() {
    let dir = setup();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = dir.join("assets.db");

    let (st, _, txt) = req(&app, Method::POST, "/api/sources", &token, sqlite_body("raw", &db, "hunter2-hunter2")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(!txt.contains("hunter2-hunter2"), "the refused value is not echoed: {txt}");

    // The reference form is accepted in the same posture.
    let (st, v, txt) = req(&app, Method::POST, "/api/sources", &token, sqlite_body("ok", &db, "env:OTS_TEST_DB_PASSWORD")).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(v["credential"], "env:OTS_TEST_DB_PASSWORD");
    assert_eq!(v["allowlisted"], true, "a local file source is allowlisted by definition: {txt}");
}

#[tokio::test]
async fn statement_timeout_is_mandatory_in_production() {
    let dir = setup();
    let (state, token) = admin_state();
    let app = test_app(state);
    let mut body = sqlite_body("no-timeout", &dir.join("assets.db"), "env:OTS_TEST_DB_PASSWORD");
    body.as_object_mut().unwrap().remove("statementTimeoutMs");
    let (st, _, txt) = req(&app, Method::POST, "/api/sources", &token, body).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.to_lowercase().contains("timeout"), "{txt}");
}

#[tokio::test]
async fn sqlite_outside_sources_dir_is_refused_in_production() {
    setup();
    let (state, token) = admin_state();
    let app = test_app(state);
    let outside = std::env::temp_dir().join(format!("ots-outside-{}.db", uuid::Uuid::new_v4()));
    rusqlite::Connection::open(&outside).unwrap();
    let (st, _, txt) = req(&app, Method::POST, "/api/sources", &token, sqlite_body("outside", &outside, "env:OTS_TEST_DB_PASSWORD")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(!txt.contains(&outside.to_string_lossy().to_string()), "path not echoed: {txt}");
}

#[tokio::test]
async fn networked_source_must_be_on_the_egress_allowlist() {
    setup();
    let (state, token) = admin_state();
    let app = test_app(state);
    let body = json!({
        "id": "pg", "name": "pg", "dialect": "postgresql",
        "host": "db.internal", "port": 5432, "database": "assets", "username": "reader",
        "credential": "env:OTS_TEST_DB_PASSWORD",
        "readOnly": true, "statementTimeoutMs": 30000,
    });
    let (st, _, txt) = req(&app, Method::POST, "/api/sources", &token, body).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    // Either the dialect is not compiled in, or the host is not allowlisted —
    // both are registration errors, and neither names a credential.
    assert!(!txt.contains("hunter2"), "{txt}");
}

#[tokio::test]
async fn oidc_provider_secret_must_be_a_reference_in_production() {
    setup();
    let (state, token) = admin_state();
    let app = test_app(state);
    let provider = |secret: &str| {
        json!({
            "name": "corp-idp", "slug": "corp-idp", "provider_type": "oidc", "client_id": "ots",
            "client_secret": secret,
            "discovery_url": "https://idp.example.org/.well-known/openid-configuration",
            "auto_provision": false,
            "is_active": false,
            "default_role": "user",
            "scopes": "openid profile email",
        })
    };
    let (st, _, txt) = req(&app, Method::POST, "/api/admin/oauth/providers", &token, provider("oidc-client-secret-value")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "raw client secret: {txt}");
    assert!(!txt.contains("oidc-client-secret-value"), "{txt}");
    let (st, v, txt) = req(&app, Method::POST, "/api/admin/oauth/providers", &token, provider("env:OTS_TEST_CLIENT_SECRET")).await;
    assert_eq!(st, StatusCode::CREATED, "referenced client secret: {txt}");
    assert!(v.get("client_secret_enc").is_none() && !txt.contains("oidc-client-secret-value"), "{txt}");
    let (st, _, txt) = req(&app, Method::POST, "/api/admin/oauth/providers", &token, provider("env:OTS_NOT_SET_ANYWHERE_123")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "unresolvable reference: {txt}");
}
