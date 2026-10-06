//! The `/sparql` read boundary holds against queries that fooled the old
//! textual rewriter — through the real router.
//!
//! `/sparql` used to re-scope a caller's query by rewriting its text: strip the
//! `FROM` / `FROM NAMED` clauses it recognised and inject a prologue naming the
//! graphs the caller may read. Two tricks beat that rewriter: a ` WHERE ` inside a
//! string literal mis-anchored the injection, leaving the query with no dataset
//! clause (which reads every named graph), and a `FROM NAMED` the scanner did
//! not recognise (no space before `<`) survived untouched. The dataset is now set
//! on the parsed query (`scope_query_dataset`), so neither trick changes what the
//! query reads; `ensure_query_within_scope` stays behind it as a backstop, and
//! its own unit tests pin that it refuses both rewritten texts.
//!
//! Here an anonymous caller on a public dataset tries both tricks to read a
//! private graph and a second tenant's private dataset; neither leaks.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use common::*;
use tower::ServiceExt as _;

use open_triplestore::auth::models::{OwnerType, Visibility};

const PUBLIC_MARKER: &str = "PUBLIC_TRIPLE_VISIBLE_MARKER";
const PRIVATE_MARKER: &str = "PRIVATE_TRIPLE_SECRET_MARKER";
const TENANT_B_MARKER: &str = "TENANT_B_SECRET_MARKER";

const PUB_GRAPH: &str = "http://example.org/g/public";
const PRIV_GRAPH: &str = "http://example.org/g/private";
const B_GRAPH: &str = "http://example.org/g/tenant-b";

/// A public dataset with one public and one private graph, plus a *second*,
/// private dataset owned by another user holding a third graph. Every graph
/// carries one distinctively-marked triple.
fn setup() -> Router {
    setup_on(test_state())
}

fn setup_on(state: open_triplestore::server::AppState) -> Router {
    for user in ["alice", "bob"] {
        state
            .auth_db
            .create_user(
                user,
                user,
                &format!("{user}@test.com"),
                "hash",
                open_triplestore::auth::models::SystemRole::User,
            )
            .unwrap();
    }

    let public_ds = state
        .auth_db
        .create_dataset(
            "ds-public",
            "DS public",
            None,
            OwnerType::User,
            "alice",
            Visibility::Public,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph(&public_ds.id, PUB_GRAPH)
        .unwrap();
    state
        .auth_db
        .add_dataset_graph(&public_ds.id, PRIV_GRAPH)
        .unwrap();
    state
        .auth_db
        .set_dataset_graph_private(&public_ds.id, PRIV_GRAPH, true)
        .unwrap();

    // A different tenant's private dataset — anonymous callers may not see it at
    // all, so its graph must never surface through a `GRAPH ?g` enumeration.
    let private_ds = state
        .auth_db
        .create_dataset(
            "ds-tenant-b",
            "DS tenant b",
            None,
            OwnerType::User,
            "bob",
            Visibility::Private,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph(&private_ds.id, B_GRAPH)
        .unwrap();

    for (graph, marker) in [
        (PUB_GRAPH, PUBLIC_MARKER),
        (PRIV_GRAPH, PRIVATE_MARKER),
        (B_GRAPH, TENANT_B_MARKER),
    ] {
        state
            .store
            .update(&format!(
                "INSERT DATA {{ GRAPH <{graph}> {{ <http://example.org/s> <http://example.org/p> \"{marker}\" }} }}"
            ))
            .unwrap();
    }

    test_app(state)
}

/// Anonymous `GET /sparql?query=…`.
async fn anon_query(app: &Router, query: &str) -> (StatusCode, String) {
    let uri = format!("/sparql?query={}", url_encode(query));
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

/// The baseline: an anonymous caller may read the public graph and only it.
#[tokio::test]
async fn anonymous_sees_the_public_graph_but_not_the_private_ones() {
    let app = setup();
    let (status, body) = anon_query(&app, "SELECT ?o WHERE { GRAPH ?g { ?s ?p ?o } }").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains(PUBLIC_MARKER),
        "public triple missing: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private graph leaked: {body}"
    );
    assert!(!body.contains(TENANT_B_MARKER), "tenant B leaked: {body}");
}

/// The prologue-in-a-literal bypass: a ` WHERE ` inside a triple-quoted literal
/// mis-anchored the textual rewriter, so the scope prologue landed inside the
/// literal and the query would have read every named graph. Scoping the parsed
/// query is not fooled: it runs over the caller's readable graphs only.
#[tokio::test]
async fn literal_spliced_prologue_cannot_read_other_graphs() {
    let app = setup();
    let attack = "SELECT ?g ?o (\"\"\"x WHERE x\"\"\" AS ?z) WHERE { GRAPH ?g { ?s ?p ?o } }";
    let (status, body) = anon_query(&app, attack).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains(PUBLIC_MARKER),
        "the readable graph answers: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private graph leaked: {body}"
    );
    assert!(!body.contains(TENANT_B_MARKER), "tenant B leaked: {body}");
}

/// The unstripped-`FROM NAMED` bypass: `FROM NAMED<iri>` with no space was not
/// recognised by the textual scanner, so the caller's own graph name survived
/// beside the injected prologue. The parser reads it like any `FROM NAMED`, and
/// a graph the caller may not read is dropped from the dataset: the query runs
/// and finds nothing.
#[tokio::test]
async fn unstripped_from_named_cannot_name_a_private_graph() {
    let app = setup();
    let attack = format!(
        "SELECT ?o FROM NAMED<{PRIV_GRAPH}> WHERE {{ GRAPH <{PRIV_GRAPH}> {{ ?s ?p ?o }} }}"
    );
    let (status, body) = anon_query(&app, &attack).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private graph leaked: {body}"
    );

    // The same trick aimed at another tenant's dataset.
    let attack =
        format!("SELECT ?o FROM NAMED<{B_GRAPH}> WHERE {{ GRAPH <{B_GRAPH}> {{ ?s ?p ?o }} }}");
    let (status, body) = anon_query(&app, &attack).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.contains(TENANT_B_MARKER), "tenant B leaked: {body}");
}

// ─── sh:SPARQLFunction definitions never reach /sparql ─────────────────────
//
// Any writer of a graph can store a `sh:SPARQLFunction`. If `/sparql`
// registered every one it found, a tenant could redefine an `xsd:` cast or a
// GeoSPARQL function for every other caller's queries, or plant a function
// under an IRI other tenants call. Functions belong to the shapes runs of the
// graph that declares them; `/sparql` sees only the server's own functions
// and those in the admin-designated graphs (`OTS_SPARQL_FUNCTION_GRAPHS`).

const SECRET_FN_MARKER: &str = "TENANT_B_FUNCTION_MARKER";

fn function_ttl(iri: &str, select: &str) -> String {
    format!(
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
         <{iri}> a sh:SPARQLFunction ;\n\
           sh:parameter [ sh:path <http://example.org/x> ; sh:order 0 ] ;\n\
           sh:parameter [ sh:path <http://example.org/y> ; sh:order 1 ] ;\n\
           sh:select \"\"\"{select}\"\"\" .\n"
    )
}

/// The tenant-B fixture, with tenant B's private graph also defining
/// `xsd:integer`, `geof:sfWithin` and a function of its own.
fn setup_with_tenant_functions() -> (Router, open_triplestore::server::AppState) {
    let state = test_state();
    let app = setup_on(state.clone());
    for (iri, select) in [
        (
            "http://www.w3.org/2001/XMLSchema#integer",
            "SELECT (0 AS ?r) WHERE {}".to_string(),
        ),
        (
            "http://www.opengis.net/def/function/geosparql/sfWithin",
            "SELECT (true AS ?r) WHERE {}".to_string(),
        ),
        (
            "http://example.org/fn/secret",
            format!("SELECT (\"{SECRET_FN_MARKER}\" AS ?r) WHERE {{}}"),
        ),
    ] {
        state
            .store
            .load_str(
                &function_ttl(iri, &select),
                oxigraph::io::RdfFormat::Turtle,
                Some(B_GRAPH),
            )
            .unwrap();
    }
    (app, state)
}

/// A cast and a GeoSPARQL function mean what the server defines, whatever a
/// tenant stored.
#[tokio::test]
async fn a_tenant_cannot_redefine_a_builtin_for_other_callers() {
    let (app, _) = setup_with_tenant_functions();

    let (status, body) = anon_query(
        &app,
        "SELECT (<http://www.w3.org/2001/XMLSchema#integer>(\"7\") AS ?v) WHERE {}",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"7\""), "xsd:integer was redefined: {body}");

    let within = "SELECT (<http://www.opengis.net/def/function/geosparql/sfWithin>(\
        \"POINT(0 0)\"^^<http://www.opengis.net/ont/geosparql#wktLiteral>, \
        \"POLYGON((10 10, 11 10, 11 11, 10 11, 10 10))\"^^<http://www.opengis.net/ont/geosparql#wktLiteral>) AS ?v) WHERE {}";
    let (status, body) = anon_query(&app, within).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("false"),
        "geof:sfWithin was redefined: {body}"
    );
    assert!(
        !body.contains("\"true\""),
        "geof:sfWithin was redefined: {body}"
    );
}

/// A function a tenant stored in a dataset graph is not callable over
/// `/sparql` at all, by anyone.
#[tokio::test]
async fn a_dataset_graph_function_is_not_callable_from_sparql() {
    let (app, _) = setup_with_tenant_functions();
    let (_, body) = anon_query(
        &app,
        "SELECT (<http://example.org/fn/secret>(1, 2) AS ?v) WHERE {}",
    )
    .await;
    assert!(
        !body.contains(SECRET_FN_MARKER),
        "a dataset graph's function ran in /sparql: {body}"
    );
}

/// An admin-designated function graph (`OTS_SPARQL_FUNCTION_GRAPHS`) makes its
/// functions callable from `/sparql` — but not at a reserved IRI either, and
/// only a `urn:system:functions` graph can be designated.
#[tokio::test]
async fn a_designated_function_graph_serves_sparql_but_not_builtins() {
    let (app, state) = setup_with_tenant_functions();
    assert!(
        state
            .store
            .set_function_graphs(&[B_GRAPH.to_string()])
            .is_err(),
        "a dataset's graph must not be designatable"
    );
    state
        .store
        .set_function_graphs(&["urn:system:functions".to_string()])
        .unwrap();
    for (iri, select) in [
        (
            "http://example.org/fn/sum",
            "SELECT ($x + $y AS ?r) WHERE {}",
        ),
        (
            "http://www.w3.org/2001/XMLSchema#integer",
            "SELECT (0 AS ?r) WHERE {}",
        ),
    ] {
        state
            .store
            .load_str(
                &function_ttl(iri, select),
                oxigraph::io::RdfFormat::Turtle,
                Some("urn:system:functions"),
            )
            .unwrap();
    }

    let (status, body) = anon_query(
        &app,
        "SELECT (<http://example.org/fn/sum>(20, 22) AS ?v) WHERE {}",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("\"42\""),
        "designated function missing: {body}"
    );

    let (status, body) = anon_query(
        &app,
        "SELECT (<http://www.w3.org/2001/XMLSchema#integer>(\"7\") AS ?v) WHERE {}",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("\"7\""),
        "a designated graph redefined xsd:integer: {body}"
    );

    // The tenant's own function is still not callable.
    let (_, body) = anon_query(
        &app,
        "SELECT (<http://example.org/fn/secret>(1, 2) AS ?v) WHERE {}",
    )
    .await;
    assert!(!body.contains(SECRET_FN_MARKER), "{body}");
}

// ─── A shapes run's function bodies read that run's data graphs only ───────
//
// A function body is SPARQL that any writer of a shapes graph uploads. It
// reads data now (it used to see an empty store), so it is confined like
// every other query of the run: to the run's data graphs, whatever `FROM`,
// `FROM NAMED` or `GRAPH` the body names.

const B_SHAPES: &str = "http://example.org/g/tenant-b-shapes";

/// Bob's own dataset gets a shapes graph whose function concatenates every
/// literal it can reach — through the default graph, `FROM <alice's private
/// graph>` and `GRAPH ?g` — and reports it for one focus node.
#[tokio::test]
async fn a_function_body_cannot_read_outside_the_validated_dataset() {
    use open_triplestore::auth::models::GraphKind;
    let state = test_state();
    let app = setup_on(state.clone());
    let peek = format!(
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
         <http://example.org/fn/peek> a sh:SPARQLFunction ;\n\
           sh:select \"\"\"SELECT (GROUP_CONCAT(STR(?v)) AS ?r) FROM <{PRIV_GRAPH}> FROM NAMED <{PRIV_GRAPH}> FROM NAMED <{PUB_GRAPH}>\n\
             WHERE {{ {{ ?s ?p ?v }} UNION {{ GRAPH ?g {{ ?s ?p ?v }} }} FILTER(isLiteral(?v)) }}\"\"\" .\n\
         <http://example.org/S> a sh:NodeShape ; sh:targetNode <http://example.org/probe> ;\n\
           sh:sparql [ sh:select \"\"\"SELECT $this ?value WHERE {{ BIND(<http://example.org/fn/peek>() AS ?value) }}\"\"\" ] .\n"
    );
    state
        .store
        .load_str(&peek, oxigraph::io::RdfFormat::Turtle, Some(B_SHAPES))
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("ds-tenant-b", B_SHAPES)
        .unwrap();
    state
        .auth_db
        .set_dataset_graph_role("ds-tenant-b", B_SHAPES, Some(GraphKind::Shapes))
        .unwrap();

    let bob = mint_token("bob", "bob", "user");
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/datasets/ds-tenant-b/validate?test=true")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {bob}"))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = body_text(resp.into_body()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains(TENANT_B_MARKER),
        "the body reads the run's own data graph: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "a function body read another tenant's private graph: {body}"
    );
    assert!(
        !body.contains(PUBLIC_MARKER),
        "a function body read a graph outside the run: {body}"
    );
}
