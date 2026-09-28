//! The `/sparql` read boundary holds even when the textual query rewriter is
//! fooled — through the real router.
//!
//! `scope_query_to_authorized` re-scopes a caller's query by rewriting its text:
//! it strips the `FROM` / `FROM NAMED` clauses it recognises and injects a
//! prologue naming the graphs the caller may read. Text rewriting cannot be made
//! perfect — a ` WHERE ` inside a string literal mis-anchors the injection so the
//! prologue lands inside the literal and the query is left with no dataset clause
//! (which reads every named graph), and a `FROM NAMED` the scanner does not
//! recognise (no space before `<`) survives untouched. `ensure_query_within_scope`
//! backstops it: the query about to reach the engine is parsed and refused with
//! `403` unless its dataset names only graphs the caller may read.
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
    let state = test_state();
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
/// mis-anchors the rewriter, so the scope prologue lands inside the literal and
/// the query would otherwise read every named graph. The guard refuses it.
#[tokio::test]
async fn literal_spliced_prologue_cannot_read_other_graphs() {
    let app = setup();
    let attack = "SELECT ?g ?o (\"\"\"x WHERE x\"\"\" AS ?z) WHERE { GRAPH ?g { ?s ?p ?o } }";
    let (status, body) = anon_query(&app, attack).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the neutralised-scope query must be refused, not run: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private graph leaked: {body}"
    );
    assert!(!body.contains(TENANT_B_MARKER), "tenant B leaked: {body}");
}

/// The unstripped-`FROM NAMED` bypass: `FROM NAMED<iri>` with no space is not
/// recognised by the scanner, so the caller's own graph name survives beside the
/// injected prologue. The guard refuses a name outside the readable scope.
#[tokio::test]
async fn unstripped_from_named_cannot_name_a_private_graph() {
    let app = setup();
    let attack = format!(
        "SELECT ?o FROM NAMED<{PRIV_GRAPH}> WHERE {{ GRAPH <{PRIV_GRAPH}> {{ ?s ?p ?o }} }}"
    );
    let (status, body) = anon_query(&app, &attack).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "naming a private graph in an unstripped FROM NAMED must be refused: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private graph leaked: {body}"
    );

    // The same trick aimed at another tenant's dataset.
    let attack =
        format!("SELECT ?o FROM NAMED<{B_GRAPH}> WHERE {{ GRAPH <{B_GRAPH}> {{ ?s ?p ?o }} }}");
    let (status, body) = anon_query(&app, &attack).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(!body.contains(TENANT_B_MARKER), "tenant B leaked: {body}");
}
