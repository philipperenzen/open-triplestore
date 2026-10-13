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

// ── Graph Store HTTP Protocol — a rejected PUT must not destroy the graph ─────

/// A PUT whose body fails to parse must leave the target graph exactly as it
/// was. The implementation used to `clear_graph()` first and only then parse, so
/// one syntax error returned 4xx *and* left the graph empty with nothing to
/// replace it — silent, unrecoverable data loss on a request the server itself
/// rejected.
#[tokio::test]
async fn gsp_put_with_malformed_body_leaves_graph_intact() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let g = graph_uri("http://example.org/atomic");

    let (st, ..) = send(
        &app,
        Method::PUT,
        g.clone(),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://ex/keep> <http://ex/p> <http://ex/o> .",
    )
    .await;
    assert!(st.is_success(), "seed PUT => {st}");

    // Truncated Turtle: the object and terminating '.' are missing.
    let (st, ..) = send(
        &app,
        Method::PUT,
        g.clone(),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://ex/broken> <http://ex/p> ",
    )
    .await;
    assert!(
        st.is_client_error(),
        "a malformed PUT body must be rejected, got {st}"
    );

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
    assert!(st.is_success(), "GET after rejected PUT => {st}");
    assert!(
        body.contains("keep"),
        "a rejected PUT must not clear the graph; graph is now: {body:?}"
    );
    assert!(
        !body.contains("broken"),
        "no part of the rejected body may be applied: {body:?}"
    );
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
    app_with_shacl_on_write_shapes(
        "@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example.org/> . \
         ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ; \
         sh:property [ sh:path ex:name ; sh:minCount 1 ] .",
    )
    .await
}

/// As [`app_with_shacl_on_write`], gating writes to `urn:data:d1` on `shapes`
/// (Turtle) instead of the default minCount shape.
async fn app_with_shacl_on_write_shapes(shapes: &str) -> (Router, String) {
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
            shapes,
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

/// A POST merges, so the gate must validate the graph's POST-MERGE state.
///
/// Validation staged only the request payload, so `sh:minCount 1` was evaluated
/// against a node stripped of every property the payload did not repeat. Adding
/// one property to a node that already conforms was therefore REJECTED — the
/// gate blocked a write that produces a conforming graph.
#[tokio::test]
async fn shacl_on_write_post_merge_sees_existing_triples() {
    let (app, token) = app_with_shacl_on_write().await;

    // Seed a conforming person.
    let (st, ..) = send(
        &app,
        Method::PUT,
        graph_uri("urn:data:d1"),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://example.org/p3> a <http://example.org/Person> ; \
         <http://example.org/name> \"Ada\" .",
    )
    .await;
    assert!(st.is_success(), "seed PUT must succeed, got {st}");

    // Merge an extra property onto the SAME node. The payload restates the
    // type — so the shape's targetClass matches inside the staged graph — but
    // not ex:name, which already lives in the store. The merged graph conforms;
    // the payload alone does not.
    let (st, body, _) = send(
        &app,
        Method::POST,
        graph_uri("urn:data:d1"),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://example.org/p3> a <http://example.org/Person> ; \
         <http://example.org/nickname> \"A\" .",
    )
    .await;
    assert!(
        st.is_success(),
        "merging a property onto an already-conforming node must be allowed, got {st}: {body}"
    );
}

/// The merge gate must still REJECT a payload that makes the graph violate.
#[tokio::test]
async fn shacl_on_write_post_merge_still_rejects_violations() {
    let (app, token) = app_with_shacl_on_write().await;
    // A brand-new Person with no ex:name — nothing in the store supplies it.
    let (st, body, _) = send(
        &app,
        Method::POST,
        graph_uri("urn:data:d1"),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://example.org/p4> a <http://example.org/Person> .",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a merge that introduces a violation must still be rejected, got {st}: {body}"
    );
}

/// The audit's sharper merge probe: a POST that adds a SECOND `ex:name` to a
/// node that already has one must be rejected under `sh:maxCount 1`. Only a
/// gate that validates the merged future state (existing graph + payload) can
/// see the violation — the payload alone carries one name and conforms.
#[tokio::test]
async fn shacl_on_write_post_second_value_under_max_count_1_is_422() {
    let (app, token) = app_with_shacl_on_write_shapes(
        "@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example.org/> . \
         ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ; \
         sh:property [ sh:path ex:name ; sh:maxCount 1 ] .",
    )
    .await;
    let (st, body, _) = send(
        &app,
        Method::PUT,
        graph_uri("urn:data:d1"),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://example.org/p5> a <http://example.org/Person> ; \
         <http://example.org/name> \"Ada\" .",
    )
    .await;
    assert!(st.is_success(), "seed PUT must succeed, got {st}: {body}");

    let (st, body, _) = send(
        &app,
        Method::POST,
        graph_uri("urn:data:d1"),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://example.org/p5> a <http://example.org/Person> ; \
         <http://example.org/name> \"Augusta\" .",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a merge that adds a second value under sh:maxCount 1 must be rejected, got {st}: {body}"
    );
    assert_eq!(body_json_value(&body)["conforms"], false, "{body}");

    let (st, graph, _) = send(
        &app,
        Method::GET,
        graph_uri("urn:data:d1"),
        Some(&token),
        None,
        Some("application/n-triples"),
        "",
    )
    .await;
    assert!(st.is_success(), "GET => {st}");
    assert!(
        graph.contains("Ada") && !graph.contains("Augusta"),
        "a rejected POST must leave the graph unchanged: {graph:?}"
    );
}

/// A gate that cannot be evaluated must refuse the write. A `sh:sparql`
/// constraint whose `sh:select` does not parse used to yield no violations
/// (`if let Ok(..) = store.query(..)` swallowed the error), so the graph
/// conformed by accident and the write went through with 204.
#[tokio::test]
async fn shacl_on_write_gate_with_malformed_sparql_constraint_fails_closed_422() {
    let (app, token) = app_with_shacl_on_write_shapes(
        "@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example.org/> . \
         ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ; \
         sh:sparql [ sh:message \"unbalanced\" ; \
                     sh:select \"\"\"SELECT $this WHERE { $this ex:name ?n FILTER( \"\"\" ] .",
    )
    .await;
    let (st, body, _) = send(
        &app,
        Method::PUT,
        graph_uri("urn:data:d1"),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://example.org/p6> a <http://example.org/Person> ; \
         <http://example.org/name> \"Bob\" .",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a gate whose SPARQL constraint cannot be evaluated must fail closed, got {st}: {body}"
    );
    assert_eq!(body_json_value(&body)["conforms"], false, "{body}");

    let (st, graph, _) = send(
        &app,
        Method::GET,
        graph_uri("urn:data:d1"),
        Some(&token),
        None,
        Some("application/n-triples"),
        "",
    )
    .await;
    assert!(st.is_success(), "GET => {st}");
    assert!(
        !graph.contains("p6"),
        "a refused PUT must not land in the graph: {graph:?}"
    );
}

fn body_json_value(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or(serde_json::Value::Null)
}

/// A DB failure while checking for triple security labels must refuse the
/// read, not serve the graph unfiltered: the gate used to be
/// `has_triple_security_labels(..).unwrap_or(false)`.
#[tokio::test]
async fn gsp_get_fails_closed_when_the_label_table_cannot_be_read() {
    let (state, token) = admin_state();
    let db = state.auth_db.clone();
    let app = test_app(state);
    let g = graph_uri("http://example.org/labelled");

    let (st, ..) = send(
        &app,
        Method::PUT,
        g.clone(),
        Some(&token),
        Some("text/turtle"),
        None,
        "<http://ex/s> <http://ex/p> <http://ex/o> .",
    )
    .await;
    assert!(st.is_success(), "seed PUT => {st}");
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
    assert!(
        st.is_success() && body.contains("http://ex/s"),
        "control read => {st}"
    );

    // Break the label table underneath the handler.
    db.pool()
        .get()
        .unwrap()
        .execute_batch("DROP TABLE triple_security_labels;")
        .unwrap();

    let (st, body, _) = send(
        &app,
        Method::GET,
        g,
        Some(&token),
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::SERVICE_UNAVAILABLE,
        "a failed label check must refuse the read, got {st}: {body}"
    );
    assert!(
        !body.contains("http://ex/s"),
        "no graph data may be served when labels cannot be checked"
    );
}

// ── Literals keep their lexical form and datatype ────────────────────────────

const LEXICAL_TTL: &str = "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
    <http://ex/s> <http://ex/v> \"1\"^^xsd:boolean , \"05\"^^xsd:integer , \
    \"5\"^^xsd:nonNegativeInteger , \"7\"^^xsd:int , \"1.50\"^^xsd:decimal , \
    \"2020-01-01T00:00:00+00:00\"^^xsd:dateTime , \
    \"2020-01-01T00:00:00Z\"^^xsd:dateTimeStamp .";

const LEXICAL_NT: [&str; 7] = [
    "\"1\"^^<http://www.w3.org/2001/XMLSchema#boolean>",
    "\"05\"^^<http://www.w3.org/2001/XMLSchema#integer>",
    "\"5\"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger>",
    "\"7\"^^<http://www.w3.org/2001/XMLSchema#int>",
    "\"1.50\"^^<http://www.w3.org/2001/XMLSchema#decimal>",
    "\"2020-01-01T00:00:00+00:00\"^^<http://www.w3.org/2001/XMLSchema#dateTime>",
    "\"2020-01-01T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTimeStamp>",
];

/// A typed literal written through the Graph Store Protocol comes back as
/// written, lexical form and datatype, through a SPARQL SELECT (JSON results)
/// and a Graph Store GET; a graph pattern matches the term as written, a
/// FILTER the value.
#[tokio::test]
async fn gsp_and_sparql_return_typed_literals_as_written() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let g = graph_uri("http://example.org/lexical");
    let (st, body, _) = send(
        &app,
        Method::PUT,
        g.clone(),
        Some(&token),
        Some("text/turtle"),
        None,
        LEXICAL_TTL,
    )
    .await;
    assert!(st.is_success(), "PUT => {st}: {body}");

    let (st, nt, _) = send(
        &app,
        Method::GET,
        g,
        Some(&token),
        None,
        Some("application/n-triples"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{nt}");
    for lit in LEXICAL_NT {
        assert!(
            nt.contains(&format!("<http://ex/s> <http://ex/v> {lit} .")),
            "GET lost {lit}:\n{nt}"
        );
    }

    let select = |q: &str| {
        format!(
            "/sparql?query={}",
            url_encode(&format!(
                "SELECT ?o WHERE {{ GRAPH <http://example.org/lexical> {{ {q} }} }}"
            ))
        )
    };
    let (st, json, _) = send(
        &app,
        Method::GET,
        select("<http://ex/s> <http://ex/v> ?o"),
        Some(&token),
        None,
        Some("application/sparql-results+json"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{json}");
    let v = body_json_value(&json);
    let mut got: Vec<(String, String)> = v["results"]["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["o"]["value"].as_str().unwrap().to_string(),
                b["o"]["datatype"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    got.sort();
    let xsd = |l: &str, d: &str| {
        (
            l.to_string(),
            format!("http://www.w3.org/2001/XMLSchema#{d}"),
        )
    };
    let mut want = vec![
        xsd("1", "boolean"),
        xsd("05", "integer"),
        xsd("5", "nonNegativeInteger"),
        xsd("7", "int"),
        xsd("1.50", "decimal"),
        xsd("2020-01-01T00:00:00+00:00", "dateTime"),
        xsd("2020-01-01T00:00:00Z", "dateTimeStamp"),
    ];
    want.sort();
    assert_eq!(got, want);

    // A pattern with the canonical value does not find "05"; a FILTER does.
    let count = |q: String| {
        let app = app.clone();
        let token = token.clone();
        async move {
            let (st, json, _) = send(
                &app,
                Method::GET,
                q,
                Some(&token),
                None,
                Some("application/sparql-results+json"),
                "",
            )
            .await;
            assert_eq!(st, StatusCode::OK, "{json}");
            body_json_value(&json)["results"]["bindings"]
                .as_array()
                .unwrap()
                .len()
        }
    };
    assert_eq!(
        count(select("BIND(5 AS ?o) <http://ex/s> <http://ex/v> ?o")).await,
        0
    );
    assert_eq!(
        count(select(
            "<http://ex/s> <http://ex/v> ?o FILTER(isNumeric(?o) && ?o = 5)"
        ))
        .await,
        2,
        "\"05\"^^xsd:integer and \"5\"^^xsd:nonNegativeInteger equal 5 by value"
    );
}

/// A write gate whose shape says `sh:datatype xsd:nonNegativeInteger` accepts a
/// valid `"5"^^xsd:nonNegativeInteger` (the store used to read it back as
/// `xsd:integer`, so the gate answered 422 on valid data) and still refuses an
/// `xsd:integer`, an out-of-range value and a `xsd:dateTimeStamp` without a
/// time zone.
#[tokio::test]
async fn shacl_on_write_accepts_valid_derived_datatypes() {
    let (app, token) = app_with_shacl_on_write_shapes(
        "@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example.org/> . \
         @prefix xsd: <http://www.w3.org/2001/XMLSchema#> . \
         ex:CountShape a sh:NodeShape ; sh:targetClass ex:Item ; \
         sh:property [ sh:path ex:count ; sh:datatype xsd:nonNegativeInteger ] ; \
         sh:property [ sh:path ex:small ; sh:datatype xsd:byte ] ; \
         sh:property [ sh:path ex:at ; sh:datatype xsd:dateTimeStamp ] .",
    )
    .await;
    let put = |ttl: &'static str| {
        let app = app.clone();
        let token = token.clone();
        async move {
            send(
                &app,
                Method::PUT,
                graph_uri("urn:data:d1"),
                Some(&token),
                Some("text/turtle"),
                None,
                &format!(
                    "@prefix ex: <http://example.org/> . \
                     @prefix xsd: <http://www.w3.org/2001/XMLSchema#> . {ttl}"
                ),
            )
            .await
        }
    };
    let (st, body, _) = put(
        "ex:i a ex:Item ; ex:count \"5\"^^xsd:nonNegativeInteger ; ex:small \"12\"^^xsd:byte ; \
         ex:at \"2020-01-01T00:00:00Z\"^^xsd:dateTimeStamp .",
    )
    .await;
    assert!(
        st.is_success(),
        "valid derived-type data must pass: {st} {body}"
    );
    for (bad, why) in [
        (
            "ex:i a ex:Item ; ex:count 5 .",
            "xsd:integer is not xsd:nonNegativeInteger",
        ),
        (
            "ex:i a ex:Item ; ex:small \"300\"^^xsd:byte .",
            "300 is out of xsd:byte's range",
        ),
        (
            "ex:i a ex:Item ; ex:at \"2020-01-01T00:00:00\"^^xsd:dateTimeStamp .",
            "xsd:dateTimeStamp needs a time zone",
        ),
    ] {
        let (st, body, _) = put(bad).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{why}: {body}");
    }
}

// ── SPARQL 1.1 Protocol — the RDF dataset of a /sparql query ──────────────────
//
// SPARQL 1.1 Query §13 and Protocol §2.1.4: `FROM` / `FROM NAMED` (or the
// protocol's `default-graph-uri` / `named-graph-uri`, which take precedence)
// define the dataset; this server confines it to the graphs the caller may read,
// and only a request that names no dataset gets the union of those graphs.

const DS_A: &str = "http://example.org/ds/a";
const DS_B: &str = "http://example.org/ds/b";
const DS_C: &str = "http://example.org/ds/c"; // another tenant's, unreadable

/// `alice` reads `DS_A` and `DS_B` (her private dataset), which share one triple;
/// `bob`'s private dataset holds `DS_C`. Returns the app, alice's and an admin's
/// token.
fn dataset_app() -> (Router, String, String) {
    use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
    let (state, admin) = admin_state();
    for user in ["alice", "bob"] {
        state
            .auth_db
            .create_user(
                user,
                user,
                &format!("{user}@test.com"),
                "hash",
                SystemRole::User,
            )
            .unwrap();
    }
    for (id, owner, graphs) in [
        ("ds-alice", "alice", vec![DS_A, DS_B]),
        ("ds-bob", "bob", vec![DS_C]),
    ] {
        let ds = state
            .auth_db
            .create_dataset(
                id,
                id,
                None,
                OwnerType::User,
                owner,
                Visibility::Private,
                None,
            )
            .unwrap();
        for g in graphs {
            state.auth_db.add_dataset_graph(&ds.id, g).unwrap();
        }
    }
    state
        .store
        .update(&format!(
            r#"INSERT DATA {{
                 GRAPH <{DS_A}> {{ <http://ex/a> <http://ex/v> "a-only" . <http://ex/x> <http://ex/v> "shared" }}
                 GRAPH <{DS_B}> {{ <http://ex/b> <http://ex/v> "b-only" . <http://ex/x> <http://ex/v> "shared" }}
                 GRAPH <{DS_C}> {{ <http://ex/c> <http://ex/v> "c-secret" }}
               }}"#
        ))
        .unwrap();
    (test_app(state), mint_token("alice", "alice", "user"), admin)
}

/// Run a query (GET, extra URL parameters appended) and return the values of
/// `?o` (or `?g`, or the ASK boolean as "true"/"false"), sorted.
async fn values(app: &Router, token: &str, query: &str, extra: &str) -> (StatusCode, Vec<String>) {
    let uri = format!("/sparql?query={}{extra}", url_encode(query));
    let (st, body, _) = send(
        app,
        Method::GET,
        uri,
        Some(token),
        None,
        Some("application/sparql-results+json"),
        "",
    )
    .await;
    let json = body_json_value(&body);
    let mut out: Vec<String> = if let Some(b) = json.get("boolean") {
        vec![b.to_string()]
    } else {
        json["results"]["bindings"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|r| {
                        let vars = ["n", "o", "g"];
                        vars.iter()
                            .filter_map(|v| r[*v]["value"].as_str())
                            .collect::<Vec<_>>()
                            .join("|")
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    out.sort();
    (st, out)
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// No dataset named: the default graph is the RDF merge of every readable graph
/// (the triple held in both counts once), and the readable graphs are the named
/// graphs; another tenant's graph is in neither.
#[tokio::test]
async fn sparql_dataset_union_default_only_when_none_is_named() {
    let (app, alice, _) = dataset_app();
    let (st, v) = values(&app, &alice, "SELECT ?o WHERE { ?s <http://ex/v> ?o }", "").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v, strs(&["a-only", "b-only", "shared"]));
    let (_, v) = values(
        &app,
        &alice,
        "SELECT (COUNT(*) AS ?n) WHERE { ?s <http://ex/v> ?o }",
        "",
    )
    .await;
    assert_eq!(v, strs(&["3"]), "the union default graph is a set");
    let (_, v) = values(
        &app,
        &alice,
        "SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s <http://ex/v> ?o } }",
        "",
    )
    .await;
    assert_eq!(v, strs(&[DS_A, DS_B]));
}

/// `FROM <a>` alone: `<a>` is the default graph and there are no named graphs
/// (it used to become a named graph as well).
#[tokio::test]
async fn sparql_dataset_from_alone_names_no_graph() {
    let (app, alice, _) = dataset_app();
    let (_, v) = values(
        &app,
        &alice,
        &format!("SELECT ?o FROM <{DS_A}> WHERE {{ ?s <http://ex/v> ?o }}"),
        "",
    )
    .await;
    assert_eq!(v, strs(&["a-only", "shared"]));
    let (_, v) = values(
        &app,
        &alice,
        &format!("ASK FROM <{DS_A}> WHERE {{ GRAPH ?g {{ ?s ?p ?o }} }}"),
        "",
    )
    .await;
    assert_eq!(v, strs(&["false"]));
    // Two FROM graphs merge as a set.
    let (_, v) = values(
        &app,
        &alice,
        &format!("SELECT (COUNT(*) AS ?n) FROM <{DS_A}> FROM <{DS_B}> WHERE {{ ?s ?p ?o }}"),
        "",
    )
    .await;
    assert_eq!(v, strs(&["3"]));
}

/// `FROM NAMED <b>` alone: an empty default graph, `<b>` the only named graph
/// (it used to become the default graph as well).
#[tokio::test]
async fn sparql_dataset_from_named_alone_leaves_the_default_graph_empty() {
    let (app, alice, _) = dataset_app();
    let (_, v) = values(
        &app,
        &alice,
        &format!("SELECT ?o FROM NAMED <{DS_B}> WHERE {{ ?s <http://ex/v> ?o }}"),
        "",
    )
    .await;
    assert!(v.is_empty(), "{v:?}");
    let (_, v) = values(
        &app,
        &alice,
        &format!("SELECT ?g ?o FROM NAMED <{DS_B}> WHERE {{ GRAPH ?g {{ ?s <http://ex/v> ?o }} }}"),
        "",
    )
    .await;
    assert_eq!(
        v,
        strs(&[&format!("b-only|{DS_B}"), &format!("shared|{DS_B}")])
    );
}

/// A graph the caller may not read is dropped from the dataset it names, as if
/// it were empty: no rows, no error that would reveal it.
#[tokio::test]
async fn sparql_dataset_drops_an_unreadable_graph() {
    let (app, alice, _) = dataset_app();
    let (st, v) = values(
        &app,
        &alice,
        &format!("SELECT ?o FROM <{DS_C}> FROM NAMED <{DS_C}> WHERE {{ {{ ?s ?p ?o }} UNION {{ GRAPH ?g {{ ?s ?p ?o }} }} }}"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(v.is_empty(), "{v:?}");
}

/// An admin's `FROM` is the dataset as written: every registered graph is no
/// longer added to it.
#[tokio::test]
async fn sparql_dataset_admin_from_is_not_widened() {
    let (app, _, admin) = dataset_app();
    let (_, v) = values(
        &app,
        &admin,
        &format!("SELECT ?o FROM <{DS_C}> WHERE {{ ?s <http://ex/v> ?o }}"),
        "",
    )
    .await;
    assert_eq!(v, strs(&["c-secret"]));
    let (_, v) = values(&app, &admin, "SELECT ?o WHERE { ?s <http://ex/v> ?o }", "").await;
    assert_eq!(v, strs(&["a-only", "b-only", "c-secret", "shared"]));
}

/// A query whose outermost group has no `WHERE` keyword and holds a sub-select:
/// the dataset used to be spliced into the sub-select (a 400).
#[tokio::test]
async fn sparql_dataset_subquery_without_outer_where() {
    let (app, alice, _) = dataset_app();
    let (st, v) = values(
        &app,
        &alice,
        "SELECT ?o { { SELECT ?o WHERE { ?s <http://ex/v> ?o } } }",
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v, strs(&["a-only", "b-only", "shared"]));
}

/// `default-graph-uri` / `named-graph-uri` (GET, POST form, POST query body)
/// define the dataset and take precedence over the query's own `FROM`.
#[tokio::test]
async fn sparql_protocol_dataset_parameters_are_honoured() {
    let (app, alice, _) = dataset_app();
    let q = "SELECT ?o WHERE { ?s <http://ex/v> ?o }";
    let dg = |g: &str| format!("&default-graph-uri={}", url_encode(g));
    let ng = |g: &str| format!("&named-graph-uri={}", url_encode(g));
    let (_, v) = values(&app, &alice, q, &dg(DS_B)).await;
    assert_eq!(v, strs(&["b-only", "shared"]));
    // Precedence over the query's FROM.
    let (_, v) = values(
        &app,
        &alice,
        &format!("SELECT ?o FROM <{DS_A}> WHERE {{ ?s <http://ex/v> ?o }}"),
        &dg(DS_B),
    )
    .await;
    assert_eq!(v, strs(&["b-only", "shared"]));
    // named-graph-uri alone: an empty default graph.
    let (_, v) = values(&app, &alice, q, &ng(DS_A)).await;
    assert!(v.is_empty(), "{v:?}");
    let (_, v) = values(
        &app,
        &alice,
        "SELECT ?g WHERE { GRAPH ?g { <http://ex/a> ?p ?o } }",
        &format!("{}{}", ng(DS_A), ng(DS_B)),
    )
    .await;
    assert_eq!(v, strs(&[DS_A]));
    // An unreadable graph is dropped here too.
    let (st, v) = values(&app, &alice, q, &dg(DS_C)).await;
    assert_eq!(st, StatusCode::OK);
    assert!(v.is_empty(), "{v:?}");
    // Not an absolute IRI: 400.
    let (st, _) = values(&app, &alice, q, "&default-graph-uri=not-an-iri").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // POST, form-encoded: the parameters ride in the body.
    let form = format!(
        "query={}&default-graph-uri={}",
        url_encode(q),
        url_encode(DS_A)
    );
    let (st, body, _) = send(
        &app,
        Method::POST,
        "/sparql".to_string(),
        Some(&alice),
        Some("application/x-www-form-urlencoded"),
        Some("application/sparql-results+json"),
        &form,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(
        body.contains("a-only") && !body.contains("b-only"),
        "{body}"
    );
    // POST, query in the body: the parameters ride in the URL.
    let (st, body, _) = send(
        &app,
        Method::POST,
        format!("/sparql?default-graph-uri={}", url_encode(DS_B)),
        Some(&alice),
        Some("application/sparql-query"),
        Some("application/sparql-results+json"),
        q,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(
        body.contains("b-only") && !body.contains("a-only"),
        "{body}"
    );
}

/// `using-graph-uri` / `using-named-graph-uri` (Protocol §2.2.3) set the dataset
/// of an update's `WHERE`; combined with the update's own `USING` / `WITH` they
/// are a 400.
#[tokio::test]
async fn sparql_update_protocol_using_parameters() {
    let (app, _, admin) = dataset_app();
    let copy = "INSERT { GRAPH <http://ex/out> { ?s <http://ex/copied> ?o } } WHERE { ?s <http://ex/v> ?o }";
    let (st, body, _) = send(
        &app,
        Method::POST,
        format!("/sparql?using-graph-uri={}", url_encode(DS_A)),
        Some(&admin),
        Some("application/sparql-update"),
        None,
        copy,
    )
    .await;
    assert!(st.is_success(), "{st}: {body}");
    let (_, v) = values(
        &app,
        &admin,
        "SELECT ?o FROM NAMED <http://ex/out> WHERE { GRAPH <http://ex/out> { ?s <http://ex/copied> ?o } }",
        "",
    )
    .await;
    assert_eq!(v, strs(&["a-only", "shared"]));

    // using-named-graph-uri, form-encoded: the WHERE sees DS_B as a named graph.
    let form = format!(
        "update={}&using-named-graph-uri={}",
        url_encode(
            "INSERT { GRAPH <http://ex/out2> { ?s <http://ex/from> ?g } } WHERE { GRAPH ?g { ?s <http://ex/v> ?o } }"
        ),
        url_encode(DS_B)
    );
    let (st, body, _) = send(
        &app,
        Method::POST,
        "/sparql".to_string(),
        Some(&admin),
        Some("application/x-www-form-urlencoded"),
        None,
        &form,
    )
    .await;
    assert!(st.is_success(), "{st}: {body}");
    let (_, v) = values(
        &app,
        &admin,
        "SELECT DISTINCT ?g FROM NAMED <http://ex/out2> WHERE { GRAPH <http://ex/out2> { ?s <http://ex/from> ?g } }",
        "",
    )
    .await;
    assert_eq!(v, strs(&[DS_B]));

    // The update's own WITH plus the protocol parameter: 400, nothing written.
    let (st, body, _) = send(
        &app,
        Method::POST,
        format!("/sparql?using-graph-uri={}", url_encode(DS_A)),
        Some(&admin),
        Some("application/sparql-update"),
        None,
        &format!(
            "WITH <{DS_B}> INSERT {{ ?s <http://ex/bad> ?o }} WHERE {{ ?s <http://ex/v> ?o }}"
        ),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    let (_, v) = values(
        &app,
        &admin,
        "ASK { GRAPH ?g { ?s <http://ex/bad> ?o } }",
        "",
    )
    .await;
    assert_eq!(v, strs(&["false"]));
}
