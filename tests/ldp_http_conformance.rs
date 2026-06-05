//! LDP 1.0 HTTP-layer conformance tests.
//!
//! The store-level container behaviour is covered by `ldp_conformance.rs`; this
//! suite drives the real `/ldp/*` HTTP handlers: 201 + Location on POST, the
//! `constrainedBy` Link header, OPTIONS advertising Accept-Post, and ETag /
//! If-Match optimistic concurrency (412). Member bodies use absolute IRIs (the
//! handler parses the body without a base, so a relative `<>` subject is rejected).

#![cfg(feature = "ldp")]

mod common;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use common::*;
use tower::ServiceExt as _;

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
    body: &str,
) -> (StatusCode, HeaderMap, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let req = b.body(Body::from(body.to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let hdrs = resp.headers().clone();
    let text = body_text(resp.into_body()).await;
    (status, hdrs, text)
}

/// POST a member into container `/ldp/c1` (auto-created as a Basic Container).
async fn post_member(app: &Router, token: &str, slug: &str) -> (StatusCode, HeaderMap) {
    let (st, h, _b) = send(
        app,
        Method::POST,
        "/ldp/c1",
        Some(token),
        &[("Content-Type", "text/turtle"), ("Slug", slug)],
        "<http://example.org/x> <http://example.org/p> \"v\" .",
    )
    .await;
    (st, h)
}

// POST to a container creates a member and returns 201 with a Location header.
#[tokio::test]
async fn ldp_post_creates_member_with_location() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let (st, hdrs) = post_member(&app, &token, "item1").await;
    assert_eq!(st, StatusCode::CREATED, "POST must create with 201, got {st}");
    assert!(hdrs.contains_key(header::LOCATION), "201 response must carry a Location header");
}

// OPTIONS on an LDP resource advertises Allow + Accept-Post + Accept-Patch.
//
// NOTE: this drives the LDP router directly. In the full application the global
// CORS layer answers OPTIONS (preflight) before the LDP handler runs, so the
// deployed `OPTIONS /ldp/*` returns CORS headers and does NOT surface
// Accept-Post — a known interaction tracked separately. Here we verify the LDP
// handler itself is conformant.
#[tokio::test]
async fn ldp_options_advertises_capabilities() {
    use open_triplestore::ldp::ldp_routes;
    let app: Router = ldp_routes().with_state(test_state());
    let (st, hdrs, _) = send(&app, Method::OPTIONS, "/ldp/c1", None, &[], "").await;
    assert!(st.is_success(), "OPTIONS must succeed, got {st}");
    assert!(hdrs.contains_key("accept-post"), "OPTIONS must advertise Accept-Post");
    assert!(hdrs.contains_key("accept-patch"), "OPTIONS must advertise Accept-Patch");
    assert!(hdrs.contains_key(header::ALLOW), "OPTIONS must advertise Allow");
}

// Every LDP response carries the constrainedBy Link header.
#[tokio::test]
async fn ldp_constrainedby_link_header() {
    let (state, token) = admin_state();
    let app = test_app(state);
    post_member(&app, &token, "x").await;
    let (_st, hdrs, _) = send(&app, Method::GET, "/ldp/c1", Some(&token), &[], "").await;
    let link = hdrs
        .get(header::LINK)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        link.contains("constrainedBy"),
        "responses must carry a constrainedBy Link header, got {link:?}"
    );
}

// ETag / If-Match optimistic concurrency: a stale If-Match yields 412.
#[tokio::test]
async fn ldp_if_match_precondition_failed() {
    let (state, token) = admin_state();
    let app = test_app(state);
    // Create a resource.
    let (st, _, body) = send(
        &app,
        Method::PUT,
        "/ldp/res1",
        Some(&token),
        &[("Content-Type", "text/turtle")],
        "<http://example.org/res1> <http://example.org/p> \"v1\" .",
    )
    .await;
    assert!(
        st.is_success() || st == StatusCode::CREATED,
        "PUT create must succeed, got {st}; body: {body}"
    );
    // PUT with a stale/incorrect If-Match must be rejected with 412.
    let (st, _, _) = send(
        &app,
        Method::PUT,
        "/ldp/res1",
        Some(&token),
        &[("Content-Type", "text/turtle"), ("If-Match", "\"stale-etag\"")],
        "<http://example.org/res1> <http://example.org/p> \"v2\" .",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::PRECONDITION_FAILED,
        "a stale If-Match must yield 412, got {st}"
    );
}
