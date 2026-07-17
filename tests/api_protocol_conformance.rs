//! SPARQL 1.1 Protocol + Graph Store HTTP Protocol conformance (HTTP layer).
//!
//! Drives the real Axum router in-process. Covers content negotiation, error
//! codes, write authorization, and the Graph Store Protocol PUT-replaces /
//! POST-merges semantics (research sparql11-cx-11), with the verifier's
//! correction that CSV/TSV apply to SELECT results only (not ASK booleans).

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use tower::ServiceExt as _;

/// Send one request against a fresh clone of the router (shared `AppState`/store).
async fn send(
    app: &Router,
    method: Method,
    uri: String,
    token: Option<&str>,
    content_type: Option<&str>,
    accept: Option<&str>,
    body: &str,
) -> (StatusCode, String, Option<String>) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(c) = content_type {
        b = b.header(header::CONTENT_TYPE, c);
    }
    if let Some(a) = accept {
        b = b.header(header::ACCEPT, a);
    }
    let req = b.body(Body::from(body.to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let ctype = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let text = body_text(resp.into_body()).await;
    (status, text, ctype)
}

fn graph_uri(g: &str) -> String {
    format!("/store?graph={}", url_encode(g))
}

// ── Graph Store HTTP Protocol — PUT replaces, POST merges (cx-11) ──────────────

#[tokio::test]
async fn gsp_put_replaces_post_merges() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let g = graph_uri("http://example.org/g");

    // PUT s1
    let (st, ..) = send(
        &app,
        Method::PUT,
        g.clone(),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://ex/s1> <http://ex/p> <http://ex/o1> .",
    )
    .await;
    assert!(st.is_success(), "PUT 1 => {st}");

    // POST s2 — RDF merge: both present
    let (st, ..) = send(
        &app,
        Method::POST,
        g.clone(),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://ex/s2> <http://ex/p> <http://ex/o2> .",
    )
    .await;
    assert!(st.is_success(), "POST => {st}");
    let (st, body, _) = send(
        &app,
        Method::GET,
        g.clone(),
        Some(&token),
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    assert!(st.is_success(), "GET after merge => {st}");
    assert!(
        body.contains("s1") && body.contains("s2"),
        "POST merges: {body}"
    );

    // PUT s3 — replaces the whole graph
    let (st, ..) = send(
        &app,
        Method::PUT,
        g.clone(),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://ex/s3> <http://ex/p> <http://ex/o3> .",
    )
    .await;
    assert!(st.is_success(), "PUT 2 => {st}");
    let (st, body, _) = send(
        &app,
        Method::GET,
        g.clone(),
        Some(&token),
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    assert!(st.is_success());
    assert!(
        body.contains("s3") && !body.contains("s1") && !body.contains("s2"),
        "PUT replaces (no merge): {body}"
    );

    // DELETE the graph, then it is empty/absent.
    let (st, ..) = send(
        &app,
        Method::DELETE,
        g.clone(),
        Some(&token),
        None,
        None,
        "",
    )
    .await;
    assert!(st.is_success(), "DELETE => {st}");
    let (_st, body, _) = send(
        &app,
        Method::GET,
        g.clone(),
        Some(&token),
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    assert!(
        !body.contains("s3"),
        "after DELETE the graph is empty: {body}"
    );
}

// GSP writes require authorization.
#[tokio::test]
async fn gsp_write_requires_auth() {
    let (state, _token) = admin_state();
    let app = test_app(state);
    let (st, ..) = send(
        &app,
        Method::PUT,
        graph_uri("http://example.org/g"),
        None,
        Some("text/turtle"),
        None,
        "<http://ex/s> <http://ex/p> <http://ex/o> .",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "unauthenticated GSP PUT must be 401, got {st}"
    );
}

// ── SPARQL 1.1 Protocol — query forms + content negotiation ───────────────────

/// Load data into a named graph via GSP, then query it.
async fn app_with_data() -> (Router, String) {
    let (state, token) = admin_state();
    let app = test_app(state);
    let (st, ..) = send(
        &app,
        Method::PUT,
        graph_uri("http://ex/data"),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://ex/alice> <http://ex/name> \"Alice\" . <http://ex/bob> <http://ex/name> \"Bob\" .",
    )
    .await;
    assert!(st.is_success(), "seed PUT => {st}");
    (app, token)
}

#[tokio::test]
async fn sparql_select_content_negotiation() {
    let (app, token) = app_with_data().await;
    let q = url_encode("SELECT ?n WHERE { GRAPH <http://ex/data> { ?s <http://ex/name> ?n } }");

    // SELECT → JSON
    let (st, body, ct) = send(
        &app,
        Method::GET,
        format!("/sparql?query={q}"),
        Some(&token),
        None,
        Some("application/sparql-results+json"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "json select => {st}");
    assert!(
        ct.as_deref().unwrap_or("").contains("json"),
        "json content-type, got {ct:?}"
    );
    assert!(
        body.contains("Alice") && body.contains("Bob"),
        "json body: {body}"
    );

    // SELECT → XML
    let (st, _b, ct) = send(
        &app,
        Method::GET,
        format!("/sparql?query={q}"),
        Some(&token),
        None,
        Some("application/sparql-results+xml"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        ct.as_deref().unwrap_or("").contains("xml"),
        "xml content-type, got {ct:?}"
    );

    // SELECT → CSV
    let (st, body, ct) = send(
        &app,
        Method::GET,
        format!("/sparql?query={q}"),
        Some(&token),
        None,
        Some("text/csv"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        ct.as_deref().unwrap_or("").contains("csv"),
        "csv content-type, got {ct:?}"
    );
    assert!(body.contains("Alice"), "csv body: {body}");
}

#[tokio::test]
async fn sparql_construct_returns_rdf() {
    let (app, token) = app_with_data().await;
    let q = url_encode(
        "CONSTRUCT { ?s <http://ex/label> ?n } WHERE { GRAPH <http://ex/data> { ?s <http://ex/name> ?n } }",
    );
    let (st, body, ct) = send(
        &app,
        Method::GET,
        format!("/sparql?query={q}"),
        Some(&token),
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "construct => {st}");
    assert!(
        ct.as_deref().unwrap_or("").contains("turtle"),
        "turtle content-type, got {ct:?}"
    );
    assert!(body.contains("label"), "construct body: {body}");
}

#[tokio::test]
async fn sparql_ask_returns_json_boolean() {
    let (app, token) = app_with_data().await;
    let q = url_encode("ASK { GRAPH <http://ex/data> { ?s <http://ex/name> \"Alice\" } }");
    let (st, body, _ct) = send(
        &app,
        Method::GET,
        format!("/sparql?query={q}"),
        Some(&token),
        None,
        Some("application/sparql-results+json"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "ask => {st}");
    assert!(body.contains("true"), "ASK boolean json: {body}");
}

#[tokio::test]
async fn sparql_query_via_post_body() {
    let (app, token) = app_with_data().await;
    let (st, body, _) = send(
        &app,
        Method::POST,
        "/sparql".to_string(),
        Some(&token),
        Some("application/sparql-query"),
        Some("application/sparql-results+json"),
        "SELECT ?n WHERE { GRAPH <http://ex/data> { ?s <http://ex/name> ?n } }",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "post-body query => {st}");
    assert!(body.contains("Alice"), "post-body json: {body}");
}

#[tokio::test]
async fn sparql_malformed_query_is_400() {
    let (app, token) = app_with_data().await;
    let q = url_encode("SELECT ?x WHERE { this is not sparql");
    let (st, ..) = send(
        &app,
        Method::GET,
        format!("/sparql?query={q}"),
        Some(&token),
        None,
        Some("application/sparql-results+json"),
        "",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "malformed query must be 400, got {st}"
    );
}

// ── SPARQL 1.1 Protocol — update authorization ────────────────────────────────

#[tokio::test]
async fn sparql_update_requires_auth() {
    let (state, _token) = admin_state();
    let app = test_app(state);
    let (st, ..) = send(
        &app,
        Method::POST,
        "/sparql".to_string(),
        None,
        Some("application/sparql-update"),
        None,
        "INSERT DATA { <http://ex/a> <http://ex/b> <http://ex/c> }",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "unauthenticated UPDATE must be 401, got {st}"
    );
}

#[tokio::test]
async fn sparql_update_authenticated_succeeds() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let (st, ..) = send(
        &app,
        Method::POST,
        "/sparql".to_string(),
        Some(&token),
        Some("application/sparql-update"),
        None,
        "INSERT DATA { GRAPH <http://ex/g> { <http://ex/a> <http://ex/b> <http://ex/c> } }",
    )
    .await;
    assert!(
        st.is_success(),
        "authenticated UPDATE must succeed, got {st}"
    );
}

// ── SHACL-on-write: a dataset with shacl_on_write rejects violating writes ─────

/// Build an app whose dataset `d1` validates writes to `urn:data:d1` against a
/// blank-node property shape requiring `ex:name` on every `ex:Person`.
async fn app_with_shacl_on_write() -> (Router, String) {
    use open_triplestore::auth::models::{OwnerType, Visibility};
    let (state, token) = admin_state();
    state
        .auth_db
        .create_organisation("o1", "Acme", "acme", None, None)
        .unwrap();
    state
        .auth_db
        .create_dataset(
            "d1",
            "DS",
            None,
            OwnerType::Organisation,
            "o1",
            Visibility::Public,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("d1", "urn:data:d1")
        .unwrap();
    state
        .auth_db
        .update_dataset_shacl("d1", true, Some("urn:shapes:d1"))
        .unwrap();
    // Shapes use the standard blank-node property-shape idiom (loader handles it).
    state
        .store
        .load_str(
            "@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example.org/> . \
             ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ; \
             sh:property [ sh:path ex:name ; sh:minCount 1 ] .",
            oxigraph::io::RdfFormat::Turtle,
            Some("urn:shapes:d1"),
        )
        .unwrap();
    (test_app(state), token)
}

#[tokio::test]
async fn shacl_on_write_rejects_violation_422() {
    let (app, token) = app_with_shacl_on_write().await;
    // ex:p1 is a Person with no ex:name -> violates sh:minCount 1.
    let (st, body, _) = send(
        &app,
        Method::PUT,
        graph_uri("urn:data:d1"),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://example.org/p1> a <http://example.org/Person> .",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "violating write must be rejected with 422, got {st}; body: {body}"
    );
}

#[tokio::test]
async fn shacl_on_write_accepts_conforming() {
    let (app, token) = app_with_shacl_on_write().await;
    let (st, ..) = send(&app, Method::PUT, graph_uri("urn:data:d1"), Some(&token),
        Some("text/turtle"), None,
        "<http://example.org/p2> a <http://example.org/Person> ; <http://example.org/name> \"Bob\" .").await;
    assert!(st.is_success(), "conforming write must succeed, got {st}");
}
