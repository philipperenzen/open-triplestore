//! Feedback reports over HTTP: who may send, who reads what.
//!
//! A reporter reads back only their own reports and the admins' reply, never
//! the internal note or anyone else's report; the inbox and triage are admin
//! only; submissions are validated and capped per user per day.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::SystemRole;
use serde_json::{json, Value};
use tower::ServiceExt as _;

async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    b = b.header(header::USER_AGENT, "feedback-test/1.0");
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// `(app, admin, alice, bob)` tokens.
fn setup() -> (Router, String, String, String) {
    std::env::set_var("RATE_LIMIT_DISABLED", "1");
    let (state, admin) = admin_state();
    for (id, name) in [("alice", "alice"), ("bob", "bob")] {
        state
            .auth_db
            .create_user(
                id,
                name,
                &format!("{name}@test.com"),
                "hash",
                SystemRole::User,
            )
            .unwrap();
    }
    let app = test_app(state);
    (
        app,
        admin,
        mint_token("alice", "alice", "user"),
        mint_token("bob", "bob", "user"),
    )
}

fn report(title: &str) -> Value {
    json!({
        "kind": "bug",
        "title": title,
        "body": "1. open /sparql\n2. run the query\nexpected rows, got an error",
        "page": "/sparql",
        "include_browser": true,
    })
}

#[tokio::test]
async fn anonymous_callers_are_refused() {
    let (app, ..) = setup();
    for (m, uri) in [
        (Method::POST, "/api/feedback"),
        (Method::GET, "/api/feedback/mine"),
        (Method::GET, "/api/admin/feedback"),
    ] {
        let body = (m == Method::POST).then(|| report("x"));
        let (s, _) = call(&app, m.clone(), uri, None, body).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{m} {uri}");
    }
}

#[tokio::test]
async fn reporters_see_their_own_reports_and_the_reply_but_not_the_note() {
    let (app, admin, alice, bob) = setup();

    let (s, created) = call(
        &app,
        Method::POST,
        "/api/feedback",
        Some(&alice),
        Some(report("Query fails")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(created["status"], "open");
    assert_eq!(created["page"], "/sparql");
    let id = created["id"].as_str().unwrap().to_string();

    // Bob sees none of Alice's reports and cannot reach the inbox.
    let (_, bobs) = call(&app, Method::GET, "/api/feedback/mine", Some(&bob), None).await;
    assert_eq!(bobs.as_array().unwrap().len(), 0);
    let (s, _) = call(&app, Method::GET, "/api/admin/feedback", Some(&bob), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let uri = format!("/api/admin/feedback/{id}");
    let (s, _) = call(
        &app,
        Method::PATCH,
        &uri,
        Some(&bob),
        Some(json!({"status": "closed"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(&app, Method::DELETE, &uri, Some(&bob), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // The admin sees who sent it, from where, with which browser.
    let (s, inbox) = call(&app, Method::GET, "/api/admin/feedback", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK);
    let row = &inbox.as_array().unwrap()[0];
    assert_eq!(row["username"], "alice");
    assert_eq!(row["user_agent"], "feedback-test/1.0");
    assert_eq!(row["app_version"], env!("CARGO_PKG_VERSION"));

    let (s, updated) = call(
        &app,
        Method::PATCH,
        &uri,
        Some(&admin),
        Some(json!({"status": "resolved", "admin_response": "Fixed, thanks", "admin_note": "dup of #12"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(updated["status"], "resolved");

    let (_, mine) = call(&app, Method::GET, "/api/feedback/mine", Some(&alice), None).await;
    let mine = &mine.as_array().unwrap()[0];
    assert_eq!(mine["status"], "resolved");
    assert_eq!(mine["admin_response"], "Fixed, thanks");
    assert!(
        mine.get("admin_note").is_none(),
        "internal note leaked: {mine}"
    );
    assert!(mine.get("user_agent").is_none());

    // Filters narrow the inbox; unknown values are refused.
    let (_, open) = call(
        &app,
        Method::GET,
        "/api/admin/feedback?status=open",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(open.as_array().unwrap().len(), 0);
    let (s, _) = call(
        &app,
        Method::GET,
        "/api/admin/feedback?status=spam",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // A report filed under the wrong type can be re-labelled, to a known type only.
    let (s, relabelled) = call(
        &app,
        Method::PATCH,
        &uri,
        Some(&admin),
        Some(json!({"kind": "feature"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(relabelled["kind"], "feature");
    assert_eq!(relabelled["status"], "resolved");
    let (s, _) = call(
        &app,
        Method::PATCH,
        &uri,
        Some(&admin),
        Some(json!({"kind": "spam"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, _) = call(&app, Method::DELETE, &uri, Some(&admin), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(&app, Method::DELETE, &uri, Some(&admin), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn browser_is_recorded_only_when_allowed() {
    let (app, admin, alice, _) = setup();
    let mut r = report("No UA");
    r["include_browser"] = json!(false);
    r["page"] = Value::Null;
    let (s, _) = call(&app, Method::POST, "/api/feedback", Some(&alice), Some(r)).await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, inbox) = call(&app, Method::GET, "/api/admin/feedback", Some(&admin), None).await;
    let row = &inbox.as_array().unwrap()[0];
    assert!(row["user_agent"].is_null());
    assert!(row["page"].is_null());
}

#[tokio::test]
async fn submissions_are_validated() {
    let (app, _, alice, _) = setup();
    let cases = [
        json!({"kind": "spam", "title": "t", "body": "b"}),
        json!({"kind": "bug", "title": "   ", "body": "b"}),
        json!({"kind": "bug", "title": "t", "body": ""}),
        json!({"kind": "bug", "title": "x".repeat(201), "body": "b"}),
        json!({"kind": "bug", "title": "t", "body": "x".repeat(10_001)}),
        json!({"kind": "bug", "title": "t", "body": "b", "page": "x".repeat(501)}),
    ];
    for c in cases {
        let (s, _) = call(
            &app,
            Method::POST,
            "/api/feedback",
            Some(&alice),
            Some(c.clone()),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{c}");
    }
}

#[tokio::test]
async fn a_user_is_capped_per_day() {
    let (app, _, alice, bob) = setup();
    let limit = open_triplestore::feedback::DAILY_LIMIT;
    for i in 0..limit {
        let (s, _) = call(
            &app,
            Method::POST,
            "/api/feedback",
            Some(&alice),
            Some(report(&format!("r{i}"))),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
    }
    let (s, _) = call(
        &app,
        Method::POST,
        "/api/feedback",
        Some(&alice),
        Some(report("one more")),
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    // The cap is per user, not global.
    let (s, _) = call(
        &app,
        Method::POST,
        "/api/feedback",
        Some(&bob),
        Some(report("bob's")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
}
