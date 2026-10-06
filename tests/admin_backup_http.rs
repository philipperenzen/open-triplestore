//! The admin backup API the maintenance page drives: status (with the S3
//! copy and any staged or last restore), create, list, verify, and a restore
//! staged for the next start and cancelled again. super_admin only.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::backup::BackupManager;
use serde_json::Value;
use tower::ServiceExt as _;

async fn call(app: &Router, method: Method, uri: &str, token: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let st = resp.status();
    (st, body_json(resp.into_body()).await)
}

#[tokio::test]
async fn backups_are_created_verified_and_a_restore_is_staged_and_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let (mut state, token) = admin_state();
    state
        .store
        .load_str(
            "<http://example.org/a> <http://example.org/b> <http://example.org/c> .",
            oxigraph::io::RdfFormat::NTriples,
            None,
        )
        .unwrap();
    // The manager backs up a SQLite file; the harness's identity DB is in
    // memory, so give it one of its own.
    let sqlite = dir.path().join("auth.sqlite");
    let _db = open_triplestore::auth::db::AuthDb::open(&sqlite).unwrap();
    state.backup = Some(Arc::new(
        BackupManager::new(
            dir.path().join("backups"),
            sqlite,
            state.store.clone(),
            state.audit.clone(),
            7,
            false,
            None,
        )
        .unwrap(),
    ));
    let app = test_app(state.clone());

    let (st, v) = call(&app, Method::GET, "/api/admin/backup/status", &token).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["configured"], true, "{v}");
    assert_eq!(v["retention"], 7, "{v}");
    assert_eq!(v["encrypted"], false, "{v}");
    assert_eq!(v["count"], 0, "{v}");
    assert_eq!(v["s3"]["enabled"], false, "{v}");
    assert_eq!(v["s3"]["object_store_configured"], false, "{v}");
    assert!(v["pending_restore"].is_null(), "{v}");

    let (st, m) = call(&app, Method::POST, "/api/admin/backup", &token).await;
    assert_eq!(st, StatusCode::OK, "{m}");
    let id = m["id"].as_str().unwrap().to_string();
    assert_eq!(m["rdf_quad_count"], 1, "{m}");
    assert!(m["s3"].is_null(), "S3 is off: {m}");

    let (_, list) = call(&app, Method::GET, "/api/admin/backup", &token).await;
    assert_eq!(list.as_array().map(Vec::len), Some(1), "{list}");
    let (_, v) = call(
        &app,
        Method::POST,
        &format!("/api/admin/backup/{id}/verify"),
        &token,
    )
    .await;
    assert_eq!(v["verified"], true, "{v}");

    let (st, p) = call(
        &app,
        Method::POST,
        &format!("/api/admin/backup/{id}/restore"),
        &token,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{p}");
    assert_eq!(p["id"], id.as_str(), "{p}");
    assert_eq!(p["requested_by"], "adm", "{p}");
    let (_, v) = call(&app, Method::GET, "/api/admin/backup/status", &token).await;
    assert_eq!(v["pending_restore"]["id"], id.as_str(), "{v}");
    assert_eq!(v["latest"]["id"], id.as_str(), "{v}");

    let (st, _) = call(&app, Method::DELETE, "/api/admin/backup/restore", &token).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&app, Method::DELETE, "/api/admin/backup/restore", &token).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, v) = call(&app, Method::GET, "/api/admin/backup/status", &token).await;
    assert!(v["pending_restore"].is_null(), "{v}");

    // A backup that does not exist is refused when staged, not at the restart.
    let (st, _) = call(
        &app,
        Method::POST,
        "/api/admin/backup/backup-19990101T000000Z/restore",
        &token,
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    // An admin who is not a super admin is refused all of it.
    state
        .auth_db
        .create_user("ops", "ops", "ops@test.com", "hash", SystemRole::Admin)
        .unwrap();
    let ops = mint_token("ops", "ops", "admin");
    for (m, uri) in [
        (Method::GET, "/api/admin/backup/status".to_string()),
        (Method::POST, format!("/api/admin/backup/{id}/restore")),
        (Method::DELETE, "/api/admin/backup/restore".to_string()),
    ] {
        let (st, _) = call(&app, m, &uri, &ops).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{uri}");
    }
}

#[tokio::test]
async fn the_status_says_when_backups_are_not_configured() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let (st, v) = call(&app, Method::GET, "/api/admin/backup/status", &token).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["configured"], false, "{v}");
    assert!(v["s3"].is_object(), "{v}");
    let (st, _) = call(
        &app,
        Method::POST,
        "/api/admin/backup/backup-x/restore",
        &token,
    )
    .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
}
