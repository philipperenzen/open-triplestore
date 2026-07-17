//! Security regression tests for SHACL Studio cross-tenant access control.
//!
//! These cover findings where Studio tooling read or enforced against graphs the
//! caller had no rights to:
//!   * import-shapes copied from a caller-named *source* graph without a read check
//!     (only the destination was manage-gated);
//!   * model-context (`?graphs=`) and derive (`body.graphs`) returned/derived from
//!     caller-named graphs without a read check (the `?dataset=` paths already gate);
//!   * binding a shape graph as an enforcing validator required only *read* on the
//!     shape graph, not *manage*.
//!
//! All run against an in-memory AppState driven via the shared HTTP harness — a
//! plain authenticated (non-admin) user is denied read on any graph lacking a
//! `graph_acl` grant, which is exactly the cross-tenant boundary under test.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::server::AppState;
use serde_json::Value;
use tower::ServiceExt as _;

/// Send a request with an optional bearer token and JSON-ish body, returning the
/// status and decoded body text.
async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: &str,
) -> (StatusCode, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    b = b.header(header::CONTENT_TYPE, "application/json");
    let req = b.body(Body::from(body.to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    (status, text)
}

/// Create a regular (non-admin) user and return a token for them.
fn make_user(state: &AppState, id: &str) -> String {
    state
        .auth_db
        .create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
        .unwrap();
    mint_token(id, id, "user")
}

/// Create a shape graph as `token`'s user with the given visibility; return its id.
async fn create_shape_graph(app: &Router, token: &str, name: &str, visibility: &str) -> String {
    let body = serde_json::json!({ "name": name, "visibility": visibility }).to_string();
    let (st, txt) = send(
        app,
        Method::POST,
        "/api/shacl/shape-graphs",
        Some(token),
        &body,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "shape graph create failed: {txt}");
    let v: Value = serde_json::from_str(&txt).unwrap();
    v["id"].as_str().expect("shape graph id").to_string()
}

// import-shapes must reject a source graph the caller cannot read, even though the
// caller owns (and may manage) the destination shape graph.
#[tokio::test]
async fn import_shapes_denies_unreadable_source_graph() {
    let state = test_state();
    let token = make_user(&state, "alice");
    let app = test_app(state);

    let dest = create_shape_graph(&app, &token, "dest", "private").await;
    let body = serde_json::json!({
        "shapes": [ { "source_graph": "urn:secret:other-tenant", "shape": "http://ex/Shape" } ]
    })
    .to_string();
    let (st, txt) = send(
        &app,
        Method::POST,
        &format!("/api/shacl/shape-graphs/{dest}/import-shapes"),
        Some(&token),
        &body,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "import from an unreadable source graph must be 403, got {st}: {txt}"
    );
}

// model-context with caller-named ?graphs= must reject graphs the caller cannot read.
#[tokio::test]
async fn model_context_denies_unreadable_graph() {
    let state = test_state();
    let token = make_user(&state, "alice");
    let app = test_app(state);

    let (st, txt) = send(
        &app,
        Method::GET,
        &format!(
            "/api/shacl/model-context?graphs={}",
            url_encode("urn:secret:other-tenant")
        ),
        Some(&token),
        "",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "model-context over an unreadable graph must be 403, got {st}: {txt}"
    );
}

// derive with caller-named body.graphs must reject graphs the caller cannot read.
#[tokio::test]
async fn derive_shapes_denies_unreadable_graph() {
    let state = test_state();
    let token = make_user(&state, "alice");
    let app = test_app(state);

    let body = serde_json::json!({ "graphs": ["urn:secret:other-tenant"] }).to_string();
    let (st, txt) = send(&app, Method::POST, "/api/shacl/derive", Some(&token), &body).await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "derive over an unreadable graph must be 403, got {st}: {txt}"
    );
}

// Binding a shape graph as an enforcing validator requires *manage* on it, not
// merely read: a user who can only *access* (a public shape graph they do not own)
// must be refused.
#[tokio::test]
async fn binding_requires_manage_on_shape_graph() {
    let state = test_state();
    let owner_token = make_user(&state, "owner");
    let other_token = make_user(&state, "other");
    let app = test_app(state);

    // owner publishes a *public* shape graph: any authenticated user can read it,
    // but only the owner (or an admin) may manage it.
    let sg = create_shape_graph(&app, &owner_token, "public-shapes", "public").await;

    // The non-owner can read it (sanity: the access gate would have passed before
    // the fix) — GET succeeds.
    let (read_st, _) = send(
        &app,
        Method::GET,
        &format!("/api/shacl/shape-graphs/{sg}"),
        Some(&other_token),
        "",
    )
    .await;
    assert_eq!(
        read_st,
        StatusCode::OK,
        "a public shape graph must be readable by other users"
    );

    // But binding it (which makes it enforce against a target) must require manage.
    let body = serde_json::json!({
        "target": { "kind": "graph", "id": "urn:data:some-graph" },
        "shape_graph_id": sg,
    })
    .to_string();
    let (st, txt) = send(
        &app,
        Method::POST,
        "/api/shacl/bindings",
        Some(&other_token),
        &body,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "binding a shape graph the caller can only read must be 403, got {st}: {txt}"
    );
}
