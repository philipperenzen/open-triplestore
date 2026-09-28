//! The endpoint ACL must apply to the routes operators write rules for.
//!
//! `endpoint_acl_guard` was mounted on exactly one router group — the six
//! `/api/browse/*` routes — while the admin UI and `docs/security.md` present
//! it as general endpoint access control over any path pattern. A deny rule on
//! `/sparql` or `/api/admin/**` was accepted, listed back, and silently did
//! nothing. Anonymous callers were also exempt unconditionally, so a rule aimed
//! at public access had no effect on the `optional_auth` routes where public
//! access is exactly what one would want to restrict.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::SystemRole;
use tower::ServiceExt as _;

fn reader(state: &AppStateAlias) -> String {
    state
        .auth_db
        .create_user("reader", "reader", "reader@t.com", "hash", SystemRole::User)
        .unwrap();
    mint_token("reader", "reader", "user")
}

// `common` re-exports AppState; alias it so the helper above reads clearly.
type AppStateAlias = open_triplestore::server::AppState;

/// A deny rule on `/sparql` is enforced.
#[tokio::test]
async fn deny_rule_on_sparql_is_enforced() {
    let (state, _admin) = admin_state();
    let token = reader(&state);
    state
        .auth_db
        .create_endpoint_acl_rule("r1", "user", "reader", "/sparql", "GET", "deny", 100, "adm")
        .unwrap();

    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sparql?query={}",
                    url_encode("SELECT * WHERE { ?s ?p ?o } LIMIT 1")
                ))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a deny rule on /sparql must be enforced"
    );
}

/// A wildcard deny on the admin surface is enforced.
#[tokio::test]
async fn deny_rule_on_admin_wildcard_is_enforced() {
    let (state, admin) = admin_state();
    state
        .auth_db
        .create_endpoint_acl_rule(
            "r1",
            "user",
            "adm",
            "/api/admin/**",
            "*",
            "deny",
            100,
            "adm",
        )
        .unwrap();

    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/admin/users")
                .header(header::AUTHORIZATION, format!("Bearer {admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a deny rule on /api/admin/** must be enforced, even for an admin"
    );
}

/// With no matching rule the request proceeds — the default stays open, so
/// mounting the guard everywhere is not itself a behaviour change.
#[tokio::test]
async fn no_matching_rule_still_allows() {
    let (state, _admin) = admin_state();
    let token = reader(&state);
    state
        .auth_db
        .create_endpoint_acl_rule(
            "r1",
            "user",
            "reader",
            "/api/something-else",
            "*",
            "deny",
            100,
            "adm",
        )
        .unwrap();

    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sparql?query={}",
                    url_encode("SELECT * WHERE { ?s ?p ?o } LIMIT 1")
                ))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "an unrelated deny rule must not affect this request"
    );
}

/// The reserved `('role', 'public')` principal restricts ANONYMOUS callers.
/// That branch previously returned allow unconditionally, so no rule could ever
/// restrict public access.
#[tokio::test]
async fn public_deny_rule_restricts_anonymous_callers() {
    let (state, _admin) = admin_state();
    state
        .auth_db
        .create_endpoint_acl_rule(
            "r1",
            "role",
            "public",
            "/api/browse/**",
            "*",
            "deny",
            100,
            "adm",
        )
        .unwrap();

    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/browse/graphs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a public deny rule must restrict an anonymous caller"
    );
}
