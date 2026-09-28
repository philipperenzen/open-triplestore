//! Prefixed Turtle for the SHACL Studio shape-graph endpoint.
//!
//! `GET /api/shacl/shape-graphs/:id/turtle` served a document built by a bare
//! `RdfSerializer::from_format` — no `@prefix` header, every IRI written out in
//! full. That is the text a human edits in the source view, and the text the
//! visual builder scans to discover the prefixes it can offer, so both saw
//! nothing but `<http://…>`. These tests pin the header down: it must be there,
//! it must carry only namespaces the graph actually uses, and the document it
//! heads must still parse back to exactly the same triples.
//!
//! The same endpoint's PUT hard-coded its revision note to "Edited", making the
//! history modal a column of identical rows; a caller-supplied `?message=` is
//! covered here too.
mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::prefixes::PrefixRegistry;
use open_triplestore::server::AppState;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const SH: &str = "http://www.w3.org/ns/shacl#";

/// No blank nodes: a Turtle round trip renames those, and these tests compare
/// the triples themselves.
const SHAPES: &str = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .
ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property ex:NameProperty .
ex:NameProperty a sh:PropertyShape ;
    sh:path ex:name ;
    sh:minCount 1 .
"#;

/// `admin_state()` with a registry that can actually resolve namespaces — the
/// shared harness installs `PrefixRegistry::empty()`, which resolves nothing.
fn studio_state(registry: PrefixRegistry) -> (AppState, String) {
    let (mut state, token) = admin_state();
    state.prefix_registry = Arc::new(registry);
    (state, token)
}

async fn get_turtle(app: &Router, token: &str, id: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/shacl/shape-graphs/{id}/turtle"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::ACCEPT, "text/turtle")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

async fn put_turtle(
    app: &Router,
    token: &str,
    uri: &str,
    turtle: &str,
) -> (StatusCode, Value, String) {
    let req = Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "text/turtle")
        .body(Body::from(turtle.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

async fn json_req(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

async fn create_shape_graph(app: &Router, token: &str) -> String {
    let (st, v, txt) = json_req(
        app,
        Method::POST,
        "/api/shacl/shape-graphs",
        token,
        json!({ "name": "people", "visibility": "private", "turtle": SHAPES }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create shape graph: {txt}");
    v["id"].as_str().expect("shape graph id").to_string()
}

/// The document's triples as sorted N-Triples — identity that survives
/// serialization choices (prefixes, ordering, literal shorthand).
fn triples(turtle: &str) -> Vec<String> {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(turtle, RdfFormat::Turtle, Some("urn:parsed"))
        .unwrap_or_else(|e| panic!("response is not valid Turtle: {e}\n{turtle}"));
    let nt =
        String::from_utf8(store.dump(RdfFormat::NTriples, Some("urn:parsed")).unwrap()).unwrap();
    let mut lines: Vec<String> = nt.lines().map(str::to_string).collect();
    lines.sort();
    lines
}

/// The notes of a shape graph's revisions, newest first.
async fn revision_notes(app: &Router, token: &str, id: &str) -> Vec<String> {
    let (st, v, txt) = json_req(
        app,
        Method::GET,
        &format!("/api/shacl/shape-graphs/{id}/revisions"),
        token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "revisions: {txt}");
    v.as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|r| r["note"].as_str().unwrap_or_default().to_string())
        .collect()
}

// ─── The header ──────────────────────────────────────────────────────────────

/// The endpoint must serve CURIEs, not a wall of full IRIs — and the document
/// it serves must still be Turtle a parser accepts.
#[tokio::test]
async fn shape_graph_turtle_is_served_with_a_prefix_header() {
    let (state, token) = studio_state(PrefixRegistry::bundled_only());
    let app = test_app(state);
    let id = create_shape_graph(&app, &token).await;

    let (st, ttl) = get_turtle(&app, &token, &id).await;
    assert_eq!(st, StatusCode::OK, "{ttl}");
    assert!(ttl.contains(&format!("@prefix sh: <{SH}>")), "{ttl}");
    assert!(ttl.contains("sh:NodeShape"), "{ttl}");
    assert!(
        !ttl.contains(&format!("<{SH}NodeShape>")),
        "a declared namespace must not also be written out in full: {ttl}"
    );
    assert_eq!(
        triples(&ttl),
        triples(SHAPES),
        "prefixing must not change the graph"
    );
}

/// Only namespaces the graph draws terms from get a line. A registry knows
/// thousands; a header of unused declarations is worse than no header.
#[tokio::test]
async fn only_namespaces_the_graph_uses_are_declared() {
    let registry = PrefixRegistry::bundled_only();
    registry.set_platform_prefixes([
        (
            "unused".to_string(),
            "http://example.org/unused#".to_string(),
        ),
        ("ex".to_string(), "http://example.org/".to_string()),
    ]);
    let (state, token) = studio_state(registry);
    let app = test_app(state);
    let id = create_shape_graph(&app, &token).await;

    let (st, ttl) = get_turtle(&app, &token, &id).await;
    assert_eq!(st, StatusCode::OK, "{ttl}");
    assert!(ttl.contains("@prefix ex:"), "a used namespace: {ttl}");
    assert!(
        !ttl.contains("@prefix unused:"),
        "nothing in the graph lives in that namespace: {ttl}"
    );
    // xsd resolves too, and every literal here carries an xsd datatype — but
    // Turtle writes them all implicitly (`"Bob"`, `1`), so no xsd CURIE ever
    // reaches the document and the declaration would be dead weight.
    assert!(!ttl.contains("@prefix xsd:"), "{ttl}");
}

/// Every `example.org` IRI here sits one path segment below the registered
/// namespace, so the namespace a Turtle writer splits them at
/// (`http://example.org/shapes/`) is the only one the header sees, and no
/// registry knows it. Nothing in the graph pulls `http://example.org/` into the
/// header on its own — that is what makes this the hard case.
const NESTED_SHAPES: &str = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
<http://example.org/shapes/PersonShape> a sh:NodeShape ;
    sh:targetClass <http://example.org/shapes/Person> .
"#;

async fn create_named_shape_graph(app: &Router, token: &str, name: &str, ttl: &str) -> String {
    let (st, v, txt) = json_req(
        app,
        Method::POST,
        "/api/shacl/shape-graphs",
        token,
        json!({ "name": name, "visibility": "private", "turtle": ttl }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create shape graph: {txt}");
    v["id"].as_str().expect("shape graph id").to_string()
}

/// The registry's namespaces are not always the ones a writer splits at.
/// `http://example.org/shapes/PersonShape` splits at `.../shapes/`, which the
/// registry has never heard of — before the fallback it was served as a full
/// IRI even though `ex: <http://example.org/>` covers it. Turtle escapes the
/// `/` that remains in the local name (`ex:shapes\/PersonShape`), so the CURIE
/// is legal and the document still round trips.
#[tokio::test]
async fn an_iri_below_a_registered_namespace_still_gets_a_curie() {
    let registry = PrefixRegistry::bundled_only();
    registry.set_platform_prefixes([("ex".to_string(), "http://example.org/".to_string())]);
    let (state, token) = studio_state(registry);
    let app = test_app(state);
    let id = create_named_shape_graph(&app, &token, "nested", NESTED_SHAPES).await;

    let (st, ttl) = get_turtle(&app, &token, &id).await;
    assert_eq!(st, StatusCode::OK, "{ttl}");
    // The declaration is the *registered* namespace, not the derived one: a
    // `@prefix ex: <http://example.org/shapes/>` line would silently re-point
    // every other ex: CURIE in the document.
    assert!(
        ttl.contains("@prefix ex: <http://example.org/>"),
        "the registered namespace must be the one declared: {ttl}"
    );
    assert!(
        !ttl.contains("@prefix ex: <http://example.org/shapes/>"),
        "the derived namespace must not be declared under the registry's label: {ttl}"
    );
    assert!(
        ttl.contains(r"ex:shapes\/PersonShape"),
        "the nested shape IRI must shorten: {ttl}"
    );
    assert!(
        !ttl.contains("<http://example.org/shapes/PersonShape>"),
        "a shortened IRI must not also appear in full: {ttl}"
    );
    assert_eq!(
        triples(&ttl),
        triples(NESTED_SHAPES),
        "prefixing must not change the graph"
    );
}

// ─── Round trip ──────────────────────────────────────────────────────────────

/// What the editor GETs, it PUTs back. The prefixed form must survive that
/// unchanged — a header the endpoint cannot read back would corrupt a save.
#[tokio::test]
async fn prefixed_turtle_survives_a_get_put_get() {
    let (state, token) = studio_state(PrefixRegistry::bundled_only());
    let app = test_app(state);
    let id = create_shape_graph(&app, &token).await;
    let uri = format!("/api/shacl/shape-graphs/{id}/turtle");

    let (_, first) = get_turtle(&app, &token, &id).await;
    let (st, _, txt) = put_turtle(&app, &token, &uri, &first).await;
    assert!(st.is_success(), "PUT the served Turtle back: {st} {txt}");

    let (st, second) = get_turtle(&app, &token, &id).await;
    assert_eq!(st, StatusCode::OK, "{second}");
    assert_eq!(triples(&second), triples(&first), "round trip lost triples");
    assert!(second.contains("@prefix sh:"), "{second}");
}

// ─── Revision notes ──────────────────────────────────────────────────────────

/// The note the caller sends is the note the history shows; with no message the
/// old default stands.
#[tokio::test]
async fn a_saved_revision_carries_the_callers_message() {
    let (state, token) = studio_state(PrefixRegistry::bundled_only());
    let app = test_app(state);
    let id = create_shape_graph(&app, &token).await;
    let uri = format!("/api/shacl/shape-graphs/{id}/turtle");
    let edited = SHAPES.replace("sh:minCount 1", "sh:minCount 1 ;\n    sh:maxCount 1");

    let (st, _, txt) = put_turtle(
        &app,
        &token,
        &format!("{uri}?message={}", url_encode("Bounded the name count")),
        &edited,
    )
    .await;
    assert!(st.is_success(), "PUT with message: {st} {txt}");
    let notes = revision_notes(&app, &token, &id).await;
    assert!(
        notes.contains(&"Bounded the name count".to_string()),
        "the history must show what the author wrote: {notes:?}"
    );

    // No message: the default note, and a successful save.
    let (st, v, txt) = put_turtle(&app, &token, &uri, &edited).await;
    assert!(st.is_success(), "PUT without message: {st} {txt}");
    assert!(v["version"].as_i64().unwrap_or(0) >= 2, "{txt}");
    let notes = revision_notes(&app, &token, &id).await;
    assert_eq!(
        notes.first().map(String::as_str),
        Some("Edited"),
        "{notes:?}"
    );

    // Whitespace-only is no message at all.
    let (st, _, txt) = put_turtle(&app, &token, &format!("{uri}?message=%20%20"), &edited).await;
    assert!(st.is_success(), "{st} {txt}");
    assert_eq!(
        revision_notes(&app, &token, &id)
            .await
            .first()
            .map(String::as_str),
        Some("Edited")
    );
}

/// A note is copied into the commit log's RDF, so an over-long or
/// control-character-bearing one must be cut down rather than stored as sent.
#[tokio::test]
async fn an_oversized_revision_note_is_bounded() {
    let (state, token) = studio_state(PrefixRegistry::bundled_only());
    let app = test_app(state.clone());
    let id = create_shape_graph(&app, &token).await;
    let uri = format!("/api/shacl/shape-graphs/{id}/turtle");

    let huge = format!("{}\n\"quoted\" \\ end", "x".repeat(4096));
    let (st, _, txt) = put_turtle(
        &app,
        &token,
        &format!("{uri}?message={}", url_encode(&huge)),
        SHAPES,
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");

    let notes = revision_notes(&app, &token, &id).await;
    let note = notes.first().expect("a revision");
    assert!(note.len() <= 200, "note not bounded: {} chars", note.len());
    assert!(
        !note.contains('\n'),
        "control characters must not reach the note: {note:?}"
    );
    // The store is still queryable, so the commit log the note landed in parsed.
    assert!(state.store.count_graph(None).is_ok());
}
