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
use oxigraph::sparql::QueryResults;
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

/// LDP PATCH takes an arbitrary SPARQL UPDATE body. It used to run that body
/// verbatim via `store.update()`, so any authenticated caller — every test in
/// this file uses an admin token, which hid it — could `DROP ALL` or delete
/// another tenant's named graph, bypassing every per-graph ACL. PATCH now goes
/// through the same gate as `POST /sparql`, which admin-gates all-graph and
/// variable-graph operations.
#[tokio::test]
async fn ldp_patch_cannot_drop_all_as_non_admin() {
    let (state, admin) = admin_state();
    state
        .auth_db
        .create_user(
            "mallory",
            "mallory",
            "mallory@test.com",
            "hash",
            open_triplestore::auth::models::SystemRole::User,
        )
        .unwrap();
    let mallory = mint_token("mallory", "mallory", "user");
    let app = test_app(state.clone());

    let (st, _) = post_member(&app, &admin, "victim").await;
    assert_eq!(st, StatusCode::CREATED, "seed member");

    let before = state.store.query("SELECT * WHERE { ?s ?p ?o }").is_ok();
    assert!(before, "store is queryable before the PATCH");

    // A whole-store wipe, in both the all-graph and variable-graph forms.
    for evil in [
        "DROP ALL",
        "DELETE { GRAPH ?g { ?s ?p ?o } } WHERE { GRAPH ?g { ?s ?p ?o } }",
    ] {
        let (st, _, body) = send(
            &app,
            Method::PATCH,
            "/ldp/c1/victim",
            Some(&mallory),
            &[("Content-Type", "application/sparql-update")],
            evil,
        )
        .await;
        assert!(
            st == StatusCode::FORBIDDEN || st == StatusCode::UNAUTHORIZED,
            "non-admin PATCH `{evil}` must be refused, got {st}: {body}"
        );
    }

    // The store still holds the member's triples: a refused PATCH must not have
    // deleted anything.
    let survived = matches!(
        state
            .store
            .query("ASK { <http://example.org/x> <http://example.org/p> \"v\" }"),
        Ok(QueryResults::Boolean(true))
    );
    assert!(survived, "a refused PATCH must not delete store contents");

    let (st, ..) = send(
        &app,
        Method::GET,
        "/ldp/c1/victim",
        Some(&admin),
        &[("Accept", "text/turtle")],
        "",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::OK,
        "member must survive the refused PATCHes"
    );
}

// POST to a container creates a member and returns 201 with a Location header.
#[tokio::test]
async fn ldp_post_creates_member_with_location() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let (st, hdrs) = post_member(&app, &token, "item1").await;
    assert_eq!(
        st,
        StatusCode::CREATED,
        "POST must create with 201, got {st}"
    );
    assert!(
        hdrs.contains_key(header::LOCATION),
        "201 response must carry a Location header"
    );
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
    assert!(
        hdrs.contains_key("accept-post"),
        "OPTIONS must advertise Accept-Post"
    );
    assert!(
        hdrs.contains_key("accept-patch"),
        "OPTIONS must advertise Accept-Patch"
    );
    assert!(
        hdrs.contains_key(header::ALLOW),
        "OPTIONS must advertise Allow"
    );
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
        &[
            ("Content-Type", "text/turtle"),
            ("If-Match", "\"stale-etag\""),
        ],
        "<http://example.org/res1> <http://example.org/p> \"v2\" .",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::PRECONDITION_FAILED,
        "a stale If-Match must yield 412, got {st}"
    );
}

// ─── Security regressions ───────────────────────────────────────────────────────

// SECURITY: the LDP router must require authentication. Mounted unauthenticated, an
// anonymous PATCH applied a raw SPARQL Update (e.g. `DROP ALL`) to the shared store.
#[tokio::test]
async fn ldp_requires_authentication() {
    let (state, _token) = admin_state();
    let app = test_app(state);
    // Unauthenticated POST (member creation) must be rejected.
    let (st, _h, _b) = send(
        &app,
        Method::POST,
        "/ldp/c1",
        None,
        &[("Content-Type", "text/turtle"), ("Slug", "x")],
        "<http://example.org/x> <http://example.org/p> \"v\" .",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "unauthenticated LDP POST must be 401, got {st}"
    );
    // Unauthenticated PATCH (raw SPARQL Update) must be rejected before it runs.
    let (st, _h, _b) = send(
        &app,
        Method::PATCH,
        "/ldp/c1",
        None,
        &[("Content-Type", "application/sparql-update")],
        "DROP ALL",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "unauthenticated LDP PATCH must be 401, got {st}"
    );
}

// SECURITY: a Slug with SPARQL/IRI metacharacters must be sanitised, not injected
// into the member triples' `<member_iri>` context.
#[tokio::test]
async fn ldp_slug_injection_is_sanitised() {
    let (state, token) = admin_state();
    let app = test_app(state);
    // Seed a member so we can detect a destructive injection.
    let (st, _h) = post_member(&app, &token, "keep").await;
    assert_eq!(st, StatusCode::CREATED);

    let malicious = "evil> } ; DROP ALL ; INSERT DATA { <urn:x> <urn:y> <urn:z";
    let (st, hdrs) = post_member(&app, &token, malicious).await;
    assert_eq!(
        st,
        StatusCode::CREATED,
        "crafted Slug should create a sanitised member, got {st}"
    );
    let loc = hdrs
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        !loc.contains('>') && !loc.contains(' ') && !loc.contains(';') && !loc.contains('{'),
        "member Location must be a sanitised IRI, got {loc:?}"
    );

    // The seeded member must still be present — proves no DROP ALL executed.
    let (st, _h, body) = send(&app, Method::GET, "/ldp/c1", Some(&token), &[], "").await;
    assert!(
        st.is_success(),
        "container must still be readable, got {st}"
    );
    assert!(
        body.contains("c1/keep"),
        "seeded member must survive the injection attempt; body: {body}"
    );
}

// SECURITY: a Non-RDF Source uploaded as text/html must not be reflected as
// text/html on GET (stored XSS); it degrades to a safe type with nosniff.
#[tokio::test]
async fn ldp_binary_content_type_is_sanitised() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let (st, hdrs, _b) = send(
        &app,
        Method::POST,
        "/ldp/c1",
        Some(&token),
        &[("Content-Type", "text/html"), ("Slug", "xss")],
        "<script>alert(1)</script>",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CREATED,
        "binary POST should create, got {st}"
    );
    let loc = hdrs
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let path = loc.strip_prefix("http://localhost:7878").unwrap_or(&loc);

    let (st, hdrs, _b) = send(&app, Method::GET, path, Some(&token), &[], "").await;
    assert!(st.is_success(), "GET binary must succeed, got {st}");
    let ct = hdrs
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        !ct.contains("text/html"),
        "served Content-Type must not be text/html, got {ct:?}"
    );
    assert_eq!(
        hdrs.get("x-content-type-options")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        "nosniff",
        "must set X-Content-Type-Options: nosniff"
    );
}

// ─── Spec deviations closed in the readiness program ─────────────────────────

const BASE: &str = "http://localhost:7878";
const LDP: &str = "http://www.w3.org/ns/ldp#";

/// An ETag read from GET must satisfy If-Match on PUT. GET used to hash the
/// re-serialised, Prefer-filtered body while PUT and PATCH compared If-Match
/// against the raw DESCRIBE hash, so the documented read→modify→write round
/// trip ALWAYS ended in 412 — optimistic concurrency was unusable by any client
/// (the existing 412 test only ever sent a made-up ETag, which hid it).
#[tokio::test]
async fn ldp_etag_from_get_satisfies_if_match_on_put() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let iri = format!("{BASE}/ldp/rt1");
    let (st, _, body) = send(
        &app,
        Method::PUT,
        "/ldp/rt1",
        Some(&token),
        &[("Content-Type", "text/turtle")],
        &format!("<{iri}> <http://example.org/p> \"v1\" ."),
    )
    .await;
    assert!(st.is_success(), "create: {st} {body}");

    let (st, hdrs, _) = send(
        &app,
        Method::GET,
        "/ldp/rt1",
        Some(&token),
        &[("Accept", "text/turtle")],
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let etag = hdrs
        .get("etag")
        .expect("GET carries an ETag")
        .to_str()
        .unwrap()
        .to_string();

    let (st, hdrs2, body) = send(
        &app,
        Method::PUT,
        "/ldp/rt1",
        Some(&token),
        &[("Content-Type", "text/turtle"), ("If-Match", &etag)],
        &format!("<{iri}> <http://example.org/p> \"v2\" ."),
    )
    .await;
    assert!(
        st.is_success(),
        "the ETag GET returned must satisfy If-Match, got {st}: {body}"
    );
    // The ETag identifies STATE: it changes once the state has.
    let new_etag = hdrs2
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_ne!(new_etag, etag, "a changed resource must carry a new ETag");
}

/// RFC 9110 §9.3.2: HEAD returns the headers GET would. HEAD used to be a
/// separate implementation that hard-coded application/n-triples, hashed the
/// unfiltered DESCRIBE and omitted Vary, so HEAD and GET disagreed on
/// Content-Type and ETag for the same resource.
#[tokio::test]
async fn ldp_head_headers_match_get() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let iri = format!("{BASE}/ldp/rt2");
    let (st, _, _) = send(
        &app,
        Method::PUT,
        "/ldp/rt2",
        Some(&token),
        &[("Content-Type", "text/turtle")],
        &format!("<{iri}> <http://example.org/p> \"v\" ."),
    )
    .await;
    assert!(st.is_success());

    let accept = [("Accept", "text/turtle")];
    let (gst, get, gbody) = send(&app, Method::GET, "/ldp/rt2", Some(&token), &accept, "").await;
    let (hst, head, hbody) = send(&app, Method::HEAD, "/ldp/rt2", Some(&token), &accept, "").await;
    assert_eq!((gst, hst), (StatusCode::OK, StatusCode::OK));
    assert!(
        !gbody.is_empty() && hbody.is_empty(),
        "HEAD carries no body"
    );
    for name in ["content-type", "etag", "vary", "link"] {
        assert_eq!(
            get.get(name).map(|v| v.to_str().unwrap().to_string()),
            head.get(name).map(|v| v.to_str().unwrap().to_string()),
            "HEAD's {name} must equal GET's"
        );
    }
    assert!(
        get.get("content-type")
            .map(|v| v.to_str().unwrap().starts_with("text/turtle"))
            .unwrap_or(false),
        "negotiated type is honoured: {:?}",
        get.get("content-type")
    );
    assert!(
        get.get("vary")
            .map(|v| v.to_str().unwrap().contains("Accept"))
            .unwrap_or(false),
        "the representation varies with Accept"
    );
}

/// Every LDP response advertises `/ldp/constraints` as its constrainedBy
/// document — and nothing served it (404), so the pointer led nowhere.
#[tokio::test]
async fn ldp_constraints_document_is_served() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let (st, hdrs, body) = send(&app, Method::GET, "/ldp/constraints", Some(&token), &[], "").await;
    assert_eq!(
        st,
        StatusCode::OK,
        "the constrainedBy target must exist: {body}"
    );
    assert!(
        body.contains("constrainedBy") && body.contains("Slug"),
        "it is the constraints document, not a resource: {body}"
    );
    assert!(hdrs.contains_key("content-type"));
}

/// LDP 1.0 §5.2.3.4: a client creates a Direct container by POSTing with
/// `Link: <ldp:DirectContainer>; rel="type"` and the membership configuration in
/// the body. The Link header was never read, so every POSTed member became a
/// plain RDFSource and Direct/Indirect creation was reachable from Rust only.
#[tokio::test]
async fn ldp_post_with_link_type_creates_a_direct_container() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let dc = format!("{BASE}/ldp/c1/dc1");
    let (st, hdrs, body) = send(
        &app,
        Method::POST,
        "/ldp/c1",
        Some(&token),
        &[
            ("Content-Type", "text/turtle"),
            ("Slug", "dc1"),
            (
                "Link",
                "<http://www.w3.org/ns/ldp#DirectContainer>; rel=\"type\"",
            ),
        ],
        &format!(
            "<{dc}> <{LDP}membershipResource> <http://example.org/parent> ;\n\
             <{LDP}hasMemberRelation> <http://example.org/hasChild> ."
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(
        hdrs.get("location").and_then(|v| v.to_str().ok()),
        Some(dc.as_str())
    );

    // It IS a Direct container now …
    let (st, hdrs, _) = send(&app, Method::GET, "/ldp/c1/dc1", Some(&token), &[], "").await;
    assert_eq!(st, StatusCode::OK);
    let links: Vec<String> = hdrs
        .get_all("link")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert!(
        links.iter().any(|l| l.contains("ldp#DirectContainer")),
        "the new resource must be typed DirectContainer: {links:?}"
    );

    // … and behaves like one: a member POSTed into it yields the membership triple.
    let (st, _, body) = send(
        &app,
        Method::POST,
        "/ldp/c1/dc1",
        Some(&token),
        &[("Content-Type", "text/turtle"), ("Slug", "kid")],
        "<http://example.org/x> <http://example.org/p> \"v\" .",
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    let ask =
        format!("ASK {{ <http://example.org/parent> <http://example.org/hasChild> <{dc}/kid> }}");
    assert!(
        matches!(state.store.query(&ask), Ok(QueryResults::Boolean(true))),
        "membership triple must be materialised for the Direct container"
    );
}

/// A Direct container without its membership configuration is a constraint
/// violation (4xx pointing at the constraints document), not a silently typed
/// container that can never produce a membership triple.
#[tokio::test]
async fn ldp_direct_container_without_membership_config_is_rejected() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let (st, _, body) = send(
        &app,
        Method::POST,
        "/ldp/c1",
        Some(&token),
        &[
            ("Content-Type", "text/turtle"),
            ("Slug", "dc-bad"),
            (
                "Link",
                "<http://www.w3.org/ns/ldp#DirectContainer>; rel=\"type\"",
            ),
        ],
        "<http://example.org/x> <http://example.org/p> \"no membership config\" .",
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("membershipResource") && body.contains("constraints"),
        "the error names the missing property and the constraints doc: {body}"
    );
}
