//! A write-scoped caller cannot smuggle triples into an arbitrary named graph —
//! through the real router.
//!
//! The SPARQL UPDATE path resolves and ACL-checks every named-graph write. Two
//! data-loading paths did not, because they loaded the request body keeping its
//! embedded graph names:
//!
//! * the Graph Store Protocol default-graph write (`PUT`/`POST /store` with no
//!   `?graph`) accepted TriG / N-Quads / JSON-LD and kept their graph names;
//! * an LDP RDF Source loaded from `application/ld+json` kept the body's JSON-LD
//!   named graphs.
//!
//! So any write-scoped user — the default graph and their own LDP resource are
//! writable without a per-graph grant — could write into *any* named graph,
//! another tenant's private dataset graph or a `urn:system:*` graph, bypassing
//! the graph ACL. Both paths now reject a body that names a graph of its own.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::server::AppState;
use oxigraph::io::RdfFormat;
use tower::ServiceExt as _;

const VICTIM_GRAPH: &str = "http://victim.example/private";
const SEED_MARKER: &str = "SEED_STAYS_MARKER";
const INJECT_MARKER: &str = "INJECTED_MARKER";

/// `(state, writer_token)` — a non-admin, write-scoped user, plus a victim graph
/// seeded with one triple. The writer holds no grant on the victim graph.
fn setup() -> (AppState, String) {
    let (state, _admin) = admin_state();
    state
        .auth_db
        .create_user("writer", "writer", "w@t.com", "hash", SystemRole::User)
        .unwrap();
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{VICTIM_GRAPH}> {{ <http://s/seed> <http://p/> \"{SEED_MARKER}\" }} }}"
        ))
        .unwrap();
    (state.clone(), mint_token("writer", "writer", "user"))
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    content_type: &str,
    body: String,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

/// The victim graph's current triples, dumped straight from the store.
fn victim_dump(state: &AppState) -> String {
    String::from_utf8(
        state
            .store
            .dump(RdfFormat::NTriples, Some(VICTIM_GRAPH))
            .unwrap(),
    )
    .unwrap()
}

/// Escape 2: `POST /store` with no `?graph` (the default graph) and an N-Quads
/// body naming the victim graph.
#[tokio::test]
async fn gsp_default_graph_post_cannot_name_another_graph() {
    let (state, token) = setup();
    let app = test_app(state.clone());

    let nquads = format!("<http://s/x> <http://p/> \"{INJECT_MARKER}\" <{VICTIM_GRAPH}> .\n");
    let (status, resp) = send(
        &app,
        Method::POST,
        "/store",
        &token,
        "application/n-quads",
        nquads,
    )
    .await;

    assert!(
        !status.is_success(),
        "a default-graph POST naming another graph must be refused, got {status}: {resp}"
    );
    let dump = victim_dump(&state);
    assert!(
        dump.contains(SEED_MARKER),
        "victim graph was clobbered: {dump}"
    );
    assert!(
        !dump.contains(INJECT_MARKER),
        "triples were smuggled into the victim graph: {dump}"
    );
}

/// Escape 2, PUT form: `PUT /store` with no `?graph` and a TriG body naming the
/// victim graph.
#[tokio::test]
async fn gsp_default_graph_put_cannot_name_another_graph() {
    let (state, token) = setup();
    let app = test_app(state.clone());

    let trig = format!("<{VICTIM_GRAPH}> {{ <http://s/y> <http://p/> \"{INJECT_MARKER}\" }}\n");
    let (status, resp) = send(
        &app,
        Method::PUT,
        "/store",
        &token,
        "application/trig",
        trig,
    )
    .await;

    assert!(
        !status.is_success(),
        "a default-graph PUT naming another graph must be refused, got {status}: {resp}"
    );
    let dump = victim_dump(&state);
    assert!(
        dump.contains(SEED_MARKER),
        "victim graph was clobbered: {dump}"
    );
    assert!(
        !dump.contains(INJECT_MARKER),
        "triples were smuggled into the victim graph: {dump}"
    );
}

/// A plain default-graph write (no embedded graph names) still works — the guard
/// does not over-reach.
#[tokio::test]
async fn gsp_default_graph_post_of_plain_triples_still_works() {
    let (state, token) = setup();
    let app = test_app(state.clone());

    let (status, resp) = send(
        &app,
        Method::POST,
        "/store",
        &token,
        "application/n-triples",
        "<http://s/ok> <http://p/> \"plain\" .\n".to_string(),
    )
    .await;
    assert!(
        status.is_success(),
        "a plain-triples default-graph write must still succeed, got {status}: {resp}"
    );
}

/// Escape 1: LDP `POST /ldp/` with a JSON-LD body whose top-level `@id` names the
/// victim graph.
#[tokio::test]
async fn ldp_jsonld_body_cannot_name_another_graph() {
    let (state, token) = setup();
    let app = test_app(state.clone());

    let jsonld = format!(
        r#"{{"@id":"{VICTIM_GRAPH}","@graph":[{{"@id":"http://s/z","http://p/":"{INJECT_MARKER}"}}]}}"#
    );
    let (status, resp) = send(
        &app,
        Method::POST,
        "/ldp/",
        &token,
        "application/ld+json",
        jsonld,
    )
    .await;

    assert!(
        !status.is_success(),
        "an LDP JSON-LD body naming another graph must be refused, got {status}: {resp}"
    );
    let dump = victim_dump(&state);
    assert!(
        dump.contains(SEED_MARKER),
        "victim graph was clobbered: {dump}"
    );
    assert!(
        !dump.contains(INJECT_MARKER),
        "triples were smuggled into the victim graph via LDP JSON-LD: {dump}"
    );
}
