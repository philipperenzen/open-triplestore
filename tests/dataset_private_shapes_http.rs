//! A dataset's private shapes graph is read only by who may read it — through
//! the real router. "May read" is the set `/sparql` scopes a query to: graphs
//! of datasets the caller can access (a private graph only for its dataset's
//! writers) plus graph-ACL read grants; admins read every graph. That holds
//! whatever serves the graph:
//!
//! * `GET /api/datasets/{id}/shapes` and the form manifest, which serve a
//!   dataset's shapes to the dataset's readers;
//! * the SHACL Studio Library, which adopts a dataset's shapes graph in place
//!   when it is used (validation, import, a role change) and serves an entry
//!   to whoever the entry's visibility admits: its Turtle, revisions, clone,
//!   bindings, catalogue and pipelines. An entry adopted before the graph was
//!   made private (or before this rule) follows the graph too;
//! * linking a graph as another dataset's shapes graph, which is a read.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{GraphKind, OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use open_triplestore::store::TripleStore;
use serde_json::{json, Value};
use tower::ServiceExt as _;

/// The shapes-role graph of alice's public dataset `pub-ds`.
const SHAPES: &str = "http://pub.example/shapes";
/// A public data graph of `pub-ds`.
const DATA: &str = "http://pub.example/data";
/// Data in bob's own dataset `bob-ds`.
const BOB_DATA: &str = "http://bob.example/data";

/// Every data node of class `ex:Thing` breaks the private shape, so a report
/// on public data carries the shape's IRI, path and message.
const PRIVATE_SHAPES: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
    @prefix ex: <http://ex.org/> .\n\
    ex:SecretShape a sh:NodeShape ; sh:targetClass ex:Thing ;\n\
      sh:property [ sh:path ex:secretCode ; sh:minCount 1 ;\n\
                    sh:message \"SECRET-RULE-7\" ] .\n";

/// Whether `body` carries anything of the private shapes graph.
fn leaks(body: &str) -> bool {
    ["SecretShape", "secretCode", "SECRET-RULE-7"]
        .iter()
        .any(|m| body.contains(m))
}

async fn send(
    state: &AppState,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = test_app(state.clone())
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

async fn get(state: &AppState, uri: &str, token: &str) -> (StatusCode, String) {
    send(state, Method::GET, uri, Some(token), Value::Null).await
}

fn make_user(state: &AppState, id: &str) -> String {
    state
        .auth_db
        .create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
        .unwrap();
    mint_token(id, id, "user")
}

fn load(store: &TripleStore, graph: &str, ttl: &str) {
    store
        .load_str(ttl, oxigraph::io::RdfFormat::Turtle, Some(graph))
        .unwrap();
}

fn set_private(state: &AppState, private: bool) {
    state
        .auth_db
        .set_dataset_graph_private("pub-ds", SHAPES, private)
        .unwrap();
}

/// Alice's PUBLIC dataset `pub-ds`: a PRIVATE shapes-role graph and a public
/// data graph. bob, a plain signed-in user, views `pub-ds` and owns the
/// private dataset `bob-ds`. Returns `(state, admin, alice, bob)`.
fn fixture() -> (AppState, String, String, String) {
    let (state, admin) = admin_state();
    let alice = make_user(&state, "alice");
    let bob = make_user(&state, "bob");
    for (id, owner, vis) in [
        ("pub-ds", "alice", Visibility::Public),
        ("bob-ds", "bob", Visibility::Private),
    ] {
        state
            .auth_db
            .create_dataset(id, id, None, OwnerType::User, owner, vis, None)
            .unwrap();
    }
    load(&state.store, SHAPES, PRIVATE_SHAPES);
    load(
        &state.store,
        DATA,
        "<http://ex.org/t1> a <http://ex.org/Thing> .",
    );
    load(
        &state.store,
        BOB_DATA,
        "<http://ex.org/b1> a <http://ex.org/Thing> .",
    );
    for g in [SHAPES, DATA] {
        state.auth_db.add_dataset_graph("pub-ds", g).unwrap();
    }
    state.auth_db.add_dataset_graph("bob-ds", BOB_DATA).unwrap();
    state
        .auth_db
        .set_dataset_graph_role("pub-ds", SHAPES, Some(GraphKind::Shapes))
        .unwrap();
    set_private(&state, true);
    (state, admin, alice, bob)
}

/// Alice validates `pub-ds`, which adopts its shapes graph into the Library;
/// returns the entry.
async fn adopt(state: &AppState, alice: &str) -> Value {
    let (st, text) = send(
        state,
        Method::POST,
        "/api/datasets/pub-ds/validate",
        Some(alice),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(leaks(&text), "alice's run uses her private shapes: {text}");
    let (st, text) = get(state, "/api/shacl/shape-graphs", alice).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let entries: Value = serde_json::from_str(&text).unwrap();
    entries
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["graph_iri"] == json!(SHAPES))
        .unwrap_or_else(|| panic!("the shapes graph was adopted: {text}"))
        .clone()
}

/// Everything bob could reach Library entry `id` or the adopted graph by: all
/// refused or silent. `id` is alice's, so a pass says nothing leaked.
async fn assert_library_withholds(state: &AppState, bob: &str, id: &str) {
    let (st, text) = get(state, "/api/shacl/shape-graphs", bob).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(!text.contains(SHAPES), "listed to bob: {text}");
    for uri in [
        format!("/api/shacl/shape-graphs/{id}"),
        format!("/api/shacl/shape-graphs/{id}/turtle"),
        format!("/api/shacl/shape-graphs/{id}/turtle?format=shaclc"),
        format!("/api/shacl/shape-graphs/{id}/revisions"),
        format!("/api/shacl/shape-graphs/{id}/revisions/1"),
    ] {
        let (st, text) = get(state, &uri, bob).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{uri}: {text}");
        assert!(!leaks(&text), "{uri}: {text}");
    }
    let (st, text) = send(
        state,
        Method::POST,
        &format!("/api/shacl/shape-graphs/{id}/clone"),
        Some(bob),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "clone: {text}");

    // Nor as the shapes of a pipeline over data bob may read.
    let (st, text) = send(
        state,
        Method::POST,
        "/api/shacl/pipelines",
        Some(bob),
        json!({ "name": "p", "shape_graph_ids": [id],
                "targets": [{ "kind": "graph", "id": DATA }] }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "pipeline: {text}");

    // The dataset's bindings and effective shapes name the entry's graph and
    // carry its target classes; the catalogue its shapes.
    for uri in [
        "/api/datasets/pub-ds/effective-shapes",
        "/api/shacl/bindings?target_kind=dataset&target_id=pub-ds",
        "/api/shacl/shapes",
    ] {
        let (st, text) = get(state, uri, bob).await;
        assert_eq!(st, StatusCode::OK, "{uri}: {text}");
        assert!(!text.contains(SHAPES), "{uri}: {text}");
    }
    let drill = format!("/api/shacl/shapes?graph={}", url_encode(SHAPES));
    let (st, text) = get(state, &drill, bob).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{text}");
    assert!(!leaks(&text), "{text}");

    // The form manifest carries the Turtle of every shapes graph bound to
    // the dataset, anonymously for a public one.
    let manifest = "/api/datasets/pub-ds/form-manifest";
    for token in [None, Some(bob)] {
        let (st, text) = send(state, Method::GET, manifest, token, Value::Null).await;
        assert_eq!(st, StatusCode::OK, "{text}");
        assert!(!leaks(&text), "manifest ({token:?}): {text}");
        assert!(!text.contains(SHAPES), "manifest ({token:?}): {text}");
    }
}

/// `GET /api/datasets/{id}/shapes` serves a private shapes graph to the
/// dataset's writers, admins and graph-ACL readers, never to its viewers.
#[tokio::test]
async fn the_shapes_endpoint_serves_a_private_shapes_graph_only_to_who_may_read_it() {
    let (state, admin, alice, bob) = fixture();

    for uri in [
        "/api/datasets/pub-ds/shapes",
        "/api/datasets/pub-ds/shapes?format=shaclc",
    ] {
        let (st, text) = get(&state, uri, &bob).await;
        assert!(!leaks(&text), "{uri} to a viewer: {text}");
        assert_eq!(st, StatusCode::NOT_FOUND, "{uri}: {text}");

        for (who, token) in [("alice", &alice), ("admin", &admin)] {
            let (st, text) = get(&state, uri, token).await;
            assert_eq!(st, StatusCode::OK, "{uri} to {who}: {text}");
            assert!(text.contains("SecretShape"), "{uri} to {who}: {text}");
        }
    }

    // A viewer's validation run is not shaped by it either.
    let (st, text) = send(
        &state,
        Method::POST,
        "/api/datasets/pub-ds/validate?test=true",
        Some(&bob),
        Value::Null,
    )
    .await;
    assert!(!leaks(&text), "{st}: {text}");

    // A graph-ACL read grant reads it, as over /sparql.
    state
        .auth_db
        .grant_graph_permission("rule-1", SHAPES, "user", "bob", "read", "adm")
        .unwrap();
    let (st, text) = get(&state, "/api/datasets/pub-ds/shapes", &bob).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(text.contains("SecretShape"), "{text}");
}

/// Adopting a private shapes graph into the Library keeps it from the
/// dataset's viewers, by every path the Library serves an entry's graph.
#[tokio::test]
async fn an_adopted_private_shapes_graph_stays_private_in_the_library() {
    let (state, admin, alice, bob) = fixture();
    let entry = adopt(&state, &alice).await;
    let id = entry["id"].as_str().unwrap().to_string();
    assert_eq!(
        entry["visibility"],
        json!("private"),
        "not the public dataset's visibility: {entry}"
    );

    assert_library_withholds(&state, &bob, &id).await;

    // Its readers keep it. (A Library read never let an admin past an
    // entry's visibility; they read the graph itself.)
    let (st, text) = get(
        &state,
        &format!("/api/shacl/shape-graphs/{id}/turtle"),
        &alice,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(text.contains("SecretShape"), "{text}");
    let (st, text) = get(&state, "/api/datasets/pub-ds/shapes", &admin).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(text.contains("SecretShape"), "{text}");
    let (st, text) = get(&state, "/api/datasets/pub-ds/form-manifest", &alice).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(text.contains("SecretShape"), "alice's manifest: {text}");
}

/// An entry adopted while its graph was public (or before adoption kept a
/// private graph private) follows the graph: made private, it is withheld
/// from the dataset's viewers whatever the entry's own visibility says, and
/// made public again, it is theirs again.
#[tokio::test]
async fn an_entry_adopted_while_its_graph_was_public_follows_it_when_made_private() {
    let (state, _admin, alice, bob) = fixture();
    set_private(&state, false);
    let entry = adopt(&state, &alice).await;
    let id = entry["id"].as_str().unwrap().to_string();
    assert_eq!(entry["visibility"], json!("public"), "{entry}");
    let turtle = format!("/api/shacl/shape-graphs/{id}/turtle");
    let (st, text) = get(&state, &turtle, &bob).await;
    assert_eq!(
        st,
        StatusCode::OK,
        "a public graph is bob's to read: {text}"
    );
    assert!(text.contains("SecretShape"), "{text}");

    set_private(&state, true);
    assert_library_withholds(&state, &bob, &id).await;

    set_private(&state, false);
    let (st, text) = get(&state, &turtle, &bob).await;
    assert_eq!(st, StatusCode::OK, "public again: {text}");
    assert!(text.contains("SecretShape"), "{text}");
}

/// Linking a graph as a dataset's shapes graph is a read of it: bob may not
/// link `pub-ds`'s private shapes graph into his own dataset, and a link made
/// before this rule serves him nothing through his dataset.
#[tokio::test]
async fn another_datasets_private_shapes_graph_is_not_read_through_a_link() {
    let (state, _admin, _alice, bob) = fixture();

    let (st, text) = send(
        &state,
        Method::PUT,
        "/api/datasets/bob-ds/shacl",
        Some(&bob),
        json!({ "shacl_on_write": false, "shapes_graph_iri": SHAPES }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "link: {text}");
    assert!(!leaks(&text), "{text}");

    // A link that exists all the same.
    state
        .auth_db
        .update_dataset_shacl("bob-ds", false, Some(SHAPES))
        .unwrap();
    let (st, text) = get(&state, "/api/datasets/bob-ds/shapes", &bob).await;
    assert!(!leaks(&text), "{st}: {text}");
    let (st, text) = send(
        &state,
        Method::POST,
        "/api/datasets/bob-ds/validate",
        Some(&bob),
        Value::Null,
    )
    .await;
    assert!(!leaks(&text), "validation shaped by it: {st}: {text}");
    let (st, text) = get(&state, "/api/datasets/bob-ds/form-manifest", &bob).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(!leaks(&text), "manifest: {text}");

    // And bob's validation run did not adopt it into a Library entry of his.
    let (st, text) = get(&state, "/api/shacl/shape-graphs", &bob).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(!text.contains(SHAPES), "{text}");
}

/// A writer of `pub-ds` reads its private shapes graph, so may link it into a
/// public dataset of theirs, and it shapes their official runs there. Those
/// runs' reports carry its messages and paths, so a stored run and the report
/// graph are no more readable than the shapes graph, and its viewers get
/// neither the graph nor its entry.
#[tokio::test]
async fn a_private_shapes_graph_linked_by_a_writer_stays_with_its_readers() {
    let (state, _admin, alice, bob) = fixture();
    const ALICE_DATA: &str = "http://alice.example/data";
    state
        .auth_db
        .create_dataset(
            "alice-pub",
            "alice-pub",
            None,
            OwnerType::User,
            "alice",
            Visibility::Public,
            None,
        )
        .unwrap();
    load(
        &state.store,
        ALICE_DATA,
        "<http://ex.org/a1> a <http://ex.org/Thing> .",
    );
    state
        .auth_db
        .add_dataset_graph("alice-pub", ALICE_DATA)
        .unwrap();

    let (st, text) = send(
        &state,
        Method::PUT,
        "/api/datasets/alice-pub/shacl",
        Some(&alice),
        json!({ "shacl_on_write": false, "shapes_graph_iri": SHAPES }),
    )
    .await;
    assert!(
        st.is_success(),
        "alice may read it, so link it: {st}: {text}"
    );
    let (st, text) = send(
        &state,
        Method::POST,
        "/api/datasets/alice-pub/validate",
        Some(&alice),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let j: Value = serde_json::from_str(&text).unwrap();
    let run = j["run_id"].as_str().expect("an official run").to_string();
    assert!(leaks(&text), "alice's run is shaped by it: {text}");

    for uri in [
        "/api/datasets/alice-pub/validation/latest".to_string(),
        format!("/api/datasets/alice-pub/validation/runs/{run}"),
    ] {
        let (st, text) = get(&state, &uri, &bob).await;
        assert_eq!(st, StatusCode::OK, "{uri}: {text}");
        assert!(!leaks(&text), "{uri}: {text}");
        let (_, text) = get(&state, &uri, &alice).await;
        assert!(leaks(&text), "alice reads her run in full: {uri}: {text}");
    }
    let q = "SELECT ?m WHERE { GRAPH ?g { ?r <http://www.w3.org/ns/shacl#resultMessage> ?m } }";
    let sparql = format!("/sparql?query={}", url_encode(q));
    let (st, text) = get(&state, &sparql, &bob).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(!leaks(&text), "the report graph: {text}");
    let (_, text) = get(&state, &sparql, &alice).await;
    assert!(leaks(&text), "the report graph is alice's to read: {text}");

    let (st, text) = get(&state, "/api/datasets/alice-pub/shapes", &bob).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{text}");
    assert!(!leaks(&text), "{text}");
    let (st, text) = send(
        &state,
        Method::GET,
        "/api/datasets/alice-pub/form-manifest",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(!leaks(&text), "{text}");
    let (st, text) = get(&state, "/api/shacl/shape-graphs", &bob).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(!text.contains(SHAPES), "{text}");
}
