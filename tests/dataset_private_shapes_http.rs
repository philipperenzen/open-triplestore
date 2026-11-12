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
//! * linking a graph as another dataset's shapes graph, which is a read;
//! * a validation report, which names its shapes, their paths and messages:
//!   a write gate's refusal, a stored run (the dataset's writers included,
//!   and a graph made private after the run too), the report graph, and a
//!   pipeline run over a dataset the graph is bound to;
//! * the model profile, which profiles every shapes graph bound to a model;
//! * inference, which writes what a shapes graph's rules derive into the
//!   dataset it runs over;
//! * the dataset record (`GET /api/datasets`, `GET /api/datasets/{id}`, the
//!   answer to `PUT /api/datasets/{id}`), which carries the shapes graph the
//!   dataset is configured with, and what else names that graph to a reader
//!   of the dataset: the DCAT catalogue, the Validation page's shape-graph
//!   selector and the conformance layer.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{GraphKind, OwnerType, ResourceRole, SystemRole, Visibility};
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::registry as dmr;
use open_triplestore::server::AppState;
use open_triplestore::store::TripleStore;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

/// The shapes-role graph of alice's public dataset `pub-ds`.
const SHAPES: &str = "http://pub.example/shapes";
/// A public data graph of `pub-ds`.
const DATA: &str = "http://pub.example/data";
/// Data in bob's own dataset `bob-ds`.
const BOB_DATA: &str = "http://bob.example/data";
/// Data in alice's other public dataset `alice-pub`.
const ALICE_DATA: &str = "http://alice.example/data";

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

/// Alice's PUBLIC dataset `alice-pub`, holding one public data graph that
/// `PRIVATE_SHAPES` finds fault with.
fn alice_pub(state: &AppState) {
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
}

/// Alice, who may read `SHAPES`, links it as `alice-pub`'s shapes graph.
async fn link_shapes_into_alice_pub(state: &AppState, alice: &str) {
    let (st, text) = send(
        state,
        Method::PUT,
        "/api/datasets/alice-pub/shacl",
        Some(alice),
        json!({ "shacl_on_write": false, "shapes_graph_iri": SHAPES }),
    )
    .await;
    assert!(
        st.is_success(),
        "alice may read it, so link it: {st}: {text}"
    );
}

/// An official run of `alice-pub` by alice; returns its id.
async fn validate_alice_pub(state: &AppState, alice: &str) -> String {
    let (st, text) = send(
        state,
        Method::POST,
        "/api/datasets/alice-pub/validate",
        Some(alice),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(leaks(&text), "alice's run is shaped by it: {text}");
    let j: Value = serde_json::from_str(&text).unwrap();
    j["run_id"].as_str().expect("an official run").to_string()
}

fn report_messages() -> String {
    let q = "SELECT ?m WHERE { GRAPH ?g { ?r <http://www.w3.org/ns/shacl#resultMessage> ?m } }";
    format!("/sparql?query={}", url_encode(q))
}

/// A writer of `pub-ds` reads its private shapes graph, so may link it into a
/// public dataset of theirs, and it shapes their official runs there. Those
/// runs' reports carry its messages and paths, so a stored run is no more
/// readable than the shapes graph, and its viewers get neither the graph nor
/// its entry. The report graph is not written at all: it would be readable
/// by `alice-pub`'s writers, and they need not be readers of `pub-ds`.
#[tokio::test]
async fn a_private_shapes_graph_linked_by_a_writer_stays_with_its_readers() {
    let (state, _admin, alice, bob) = fixture();
    alice_pub(&state);
    link_shapes_into_alice_pub(&state, &alice).await;
    let run = validate_alice_pub(&state, &alice).await;

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
    for (who, token) in [("bob", &bob), ("alice", &alice)] {
        let (st, text) = get(&state, &report_messages(), token).await;
        assert_eq!(st, StatusCode::OK, "{text}");
        assert!(!leaks(&text), "the report graph, to {who}: {text}");
    }

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

// ─── Where else a private shapes graph is named ──────────────────────────────

async fn put_data(state: &AppState, token: &str, graph: &str, ttl: &str) -> (StatusCode, String) {
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/store?graph={}", url_encode(graph)))
                .header(header::CONTENT_TYPE, "text/turtle")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(ttl.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

fn make_writer(state: &AppState, dataset: &str, user: &str) {
    state
        .auth_db
        .set_resource_grant(
            "dataset",
            dataset,
            "user",
            user,
            ResourceRole::Editor,
            "adm",
        )
        .unwrap();
    state.auth_db.invalidate_accessible_graphs_cache();
}

/// A gate that refuses a write tells the writer what their data broke, so its
/// report carries the gate's shapes: their IRIs, paths and messages. From a
/// private shapes graph they may not read, a writer learns only that the write
/// was refused, whether the graph gates as the dataset's `shacl_on_write`
/// shapes graph or through a validation-layer binding.
#[tokio::test]
async fn a_write_refused_by_a_private_shapes_graph_does_not_show_it() {
    let (state, _admin, alice, bob) = fixture();
    for who in ["alice", "bob"] {
        state
            .auth_db
            .grant_graph_permission(&format!("w-{who}"), DATA, "user", who, "write", "adm")
            .unwrap();
    }
    let broken = "<http://ex.org/t9> a <http://ex.org/Thing> .";
    let fixed = "<http://ex.org/t9> a <http://ex.org/Thing> ; <http://ex.org/secretCode> \"x\" .";

    let refusals = |gate: &'static str| {
        let (state, alice, bob) = (&state, &alice, &bob);
        async move {
            let (st, text) = put_data(state, bob, DATA, broken).await;
            assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{gate}: {text}");
            assert!(!leaks(&text), "{gate}: bob's refusal: {text}");
            let (st, text) = put_data(state, alice, DATA, broken).await;
            assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{gate}: {text}");
            assert!(leaks(&text), "{gate}: alice reads the shapes: {text}");
            let (st, text) = put_data(state, bob, DATA, fixed).await;
            assert!(
                st.is_success(),
                "{gate}: conforming data is written: {st}: {text}"
            );
        }
    };

    // The legacy gate: the dataset's shapes graph, with `shacl_on_write`.
    state
        .auth_db
        .update_dataset_shacl("pub-ds", true, Some(SHAPES))
        .unwrap();
    refusals("shacl_on_write").await;

    // A binding: the one alice's validation run makes when it adopts the
    // graph (over data it finds fault with again), legacy gate off.
    state
        .auth_db
        .update_dataset_shacl("pub-ds", false, None)
        .unwrap();
    let (st, text) = put_data(&state, &alice, DATA, broken).await;
    assert!(st.is_success(), "no gate yet: {st}: {text}");
    adopt(&state, &alice).await;
    refusals("binding").await;
}

/// The model profile profiles every shapes graph bound to a model version:
/// its shapes, paths and value sets. Any user may mint the `sources:read`
/// token that reads it, so a private dataset graph bound to a public model is
/// profiled only for who may read the graph.
#[tokio::test]
async fn a_private_shapes_graph_bound_to_a_model_is_not_profiled() {
    let (state, admin, _alice, bob) = fixture();
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "m-pub",
        "Public model",
        "http://example.org/m-pub#",
        None,
        true,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    let version_graph = format!("{base}/data-model/m-pub/version/1.0.0");
    dmr::insert_version(
        &state.store,
        &base,
        &DataModelVersion {
            data_model_id: "m-pub".to_string(),
            version: "1.0.0".to_string(),
            status: VersionStatus::Published,
            graph_iri: version_graph.clone(),
            sub_graphs: vec![version_graph.clone()],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: vec![],
        },
    )
    .unwrap();
    open_triplestore::shacl_studio::bindings::add_binding(&state.store, &version_graph, SHAPES)
        .unwrap();

    let (st, text) = send(
        &state,
        Method::POST,
        "/api/auth/tokens",
        Some(&bob),
        json!({ "name": "bob's reader", "scopes": ["sources:read"] }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{text}");
    let bob_reader = serde_json::from_str::<Value>(&text).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let uri = "/api/models/m-pub/versions/1.0.0/profile";
    let (st, text) = get(&state, uri, &bob_reader).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(!leaks(&text), "profiled for bob: {text}");
    assert!(!text.contains(SHAPES), "listed for bob: {text}");
    let (st, text) = get(&state, uri, &admin).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(text.contains("secretCode"), "the admin's profile: {text}");
}

/// A stored run's report carries its shapes: it is withheld from whoever may
/// not read a private shapes graph the run used, the dataset's writers too (a
/// writer of `alice-pub` need not read `pub-ds`'s private graph), and from
/// then on when a shapes graph is made private after the run.
#[tokio::test]
async fn a_stored_run_is_no_more_readable_than_the_shapes_it_used() {
    let (state, _admin, alice, bob) = fixture();
    let carol = make_user(&state, "carol");
    alice_pub(&state);
    make_writer(&state, "alice-pub", "carol");
    link_shapes_into_alice_pub(&state, &alice).await;
    let run = validate_alice_pub(&state, &alice).await;
    for uri in [
        "/api/datasets/alice-pub/validation/latest".to_string(),
        format!("/api/datasets/alice-pub/validation/runs/{run}"),
    ] {
        let (st, text) = get(&state, &uri, &carol).await;
        assert_eq!(st, StatusCode::OK, "{uri}: {text}");
        assert!(!leaks(&text), "{uri}: to a writer of alice-pub: {text}");
    }
    let (st, text) = get(&state, &report_messages(), &carol).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(!leaks(&text), "the report graph, to carol: {text}");

    // Linked while public, made private after the run.
    set_private(&state, false);
    let run = validate_alice_pub(&state, &alice).await;
    let uri = format!("/api/datasets/alice-pub/validation/runs/{run}");
    let (_, text) = get(&state, &uri, &bob).await;
    assert!(leaks(&text), "public shapes: the report is bob's: {text}");
    let (_, text) = get(&state, &report_messages(), &bob).await;
    assert!(leaks(&text), "and so is the report graph: {text}");
    let (st, text) = send(
        &state,
        Method::PATCH,
        "/api/datasets/pub-ds/graphs",
        Some(&alice),
        json!({ "graph_iri": SHAPES, "private": true }),
    )
    .await;
    assert!(st.is_success(), "{st}: {text}");
    for (who, token) in [("bob", &bob), ("carol", &carol)] {
        let (_, text) = get(&state, &uri, token).await;
        assert!(!leaks(&text), "the stored run, to {who}: {text}");
        let (_, text) = get(&state, &report_messages(), token).await;
        assert!(!leaks(&text), "the report graph, to {who}: {text}");
    }
}

/// The report graph an official run wrote follows the graphs it reports on:
/// a data graph made private after the run makes the report graph private.
#[tokio::test]
async fn a_data_graph_made_private_after_a_run_takes_the_report_graph_with_it() {
    let (state, _admin, alice, bob) = fixture();
    set_private(&state, false);
    let (st, text) = send(
        &state,
        Method::POST,
        "/api/datasets/pub-ds/validate",
        Some(&alice),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let q = "SELECT ?f WHERE { GRAPH ?g { ?r <http://www.w3.org/ns/shacl#focusNode> ?f } }";
    let focus = format!("/sparql?query={}", url_encode(q));
    let (_, text) = get(&state, &focus, &bob).await;
    assert!(
        text.contains("http://ex.org/t1"),
        "all public so far: {text}"
    );

    let (st, text) = send(
        &state,
        Method::PATCH,
        "/api/datasets/pub-ds/graphs",
        Some(&alice),
        json!({ "graph_iri": DATA, "private": true }),
    )
    .await;
    assert!(st.is_success(), "{st}: {text}");
    let (_, text) = get(&state, &focus, &bob).await;
    assert!(
        !text.contains("http://ex.org/t1"),
        "the report graph: {text}"
    );
    let (_, text) = get(&state, &focus, &alice).await;
    assert!(text.contains("http://ex.org/t1"), "still alice's: {text}");
}

/// A pipeline over a dataset validates it with the shapes bound to it. A
/// private shapes graph linked into `alice-pub` is bound to it, so bob, who
/// may read all of `alice-pub` but not that graph, may not define a pipeline
/// over it: its reports would carry that graph's shapes.
#[tokio::test]
async fn a_pipeline_is_not_shaped_by_a_private_graph_bound_to_its_target() {
    let (state, _admin, alice, bob) = fixture();
    alice_pub(&state);
    link_shapes_into_alice_pub(&state, &alice).await;
    let body = json!({ "name": "p", "targets": [{ "kind": "dataset", "id": "alice-pub" }] });
    let (st, text) = send(
        &state,
        Method::POST,
        "/api/shacl/pipelines",
        Some(&bob),
        body.clone(),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{text}");
    assert!(!leaks(&text), "{text}");
    let (st, text) = send(
        &state,
        Method::POST,
        "/api/shacl/pipelines",
        Some(&alice),
        body,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "alice may read it all: {text}");
}

// ─── Inference ───────────────────────────────────────────────────────────────

/// A SHACL-AF rule of the private shapes graph: every `ex:Thing` is given the
/// secret code, so what it derives carries the graph's constants.
const PRIVATE_RULE: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
    @prefix ex: <http://ex.org/> .\n\
    ex:SecretShape sh:rule [ a sh:TripleRule ; sh:subject sh:this ;\n\
      sh:predicate ex:secretCode ; sh:object \"SECRET-RULE-7\" ] .\n";

/// A shapes-role graph of `alice-pub`'s own, which its writers read.
const OWN_SHAPES: &str = "http://alice.example/shapes";
const OWN_RULE: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
    @prefix ex: <http://ex.org/> .\n\
    ex:OwnShape a sh:NodeShape ; sh:targetClass ex:Thing ;\n\
      sh:rule [ a sh:TripleRule ; sh:subject sh:this ;\n\
                sh:predicate ex:checked ; sh:object true ] .\n";

/// Whether the store holds a triple with `predicate`, in any graph.
fn derived(state: &AppState, predicate: &str) -> bool {
    let q = format!(
        "ASK {{ {{ ?s <{predicate}> ?o }} UNION {{ GRAPH ?g {{ ?s <{predicate}> ?o }} }} }}"
    );
    matches!(state.store.query(&q), Ok(QueryResults::Boolean(true)))
}

async fn infer_alice_pub(state: &AppState, token: &str) -> (StatusCode, String) {
    send(
        state,
        Method::POST,
        "/api/datasets/alice-pub/infer",
        Some(token),
        Value::Null,
    )
    .await
}

/// Inference runs a dataset's rules over its data and writes what they derive
/// into the dataset, where its writers and readers read the rules' constants
/// and structure back. A private shapes graph linked into `alice-pub` is no
/// rule of a run by one of its writers who may not read that graph: with no
/// other shapes graph the run is refused, and with one it runs that one and
/// says it left some out (`partial`). Alice reads it, so her run applies it.
#[tokio::test]
async fn a_private_shapes_graph_linked_by_a_writer_infers_only_for_its_readers() {
    let (state, _admin, alice, _bob) = fixture();
    let carol = make_user(&state, "carol");
    load(&state.store, SHAPES, PRIVATE_RULE);
    alice_pub(&state);
    make_writer(&state, "alice-pub", "carol");
    link_shapes_into_alice_pub(&state, &alice).await;
    let secret = "http://ex.org/secretCode";

    let (st, text) = infer_alice_pub(&state, &carol).await;
    assert!(
        !derived(&state, secret),
        "carol's run applied the private rule"
    );
    assert!(!leaks(&text), "{text}");
    assert_eq!(st, StatusCode::BAD_REQUEST, "{text}");

    load(&state.store, OWN_SHAPES, OWN_RULE);
    state
        .auth_db
        .add_dataset_graph("alice-pub", OWN_SHAPES)
        .unwrap();
    state
        .auth_db
        .set_dataset_graph_role("alice-pub", OWN_SHAPES, Some(GraphKind::Shapes))
        .unwrap();
    state.auth_db.invalidate_accessible_graphs_cache();
    let (st, text) = infer_alice_pub(&state, &carol).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let j: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(j["partial"], json!(true), "{text}");
    assert!(derived(&state, "http://ex.org/checked"), "her own rule ran");
    assert!(
        !derived(&state, secret),
        "carol's run applied the private rule"
    );

    let (st, text) = infer_alice_pub(&state, &alice).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let j: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(j["partial"], json!(false), "{text}");
    assert!(derived(&state, secret), "alice's run applies it");
}

// ─── The dataset record ──────────────────────────────────────────────────────

/// `shapes_graph_iri` of dataset `id` as `token` (`None`: an anonymous
/// caller) reads it in `GET /api/datasets/{id}` and in the `GET /api/datasets`
/// listing. The two agree, the field is there either way, and a caller who
/// is not given the IRI in it is given it nowhere in either answer.
async fn record_shapes(state: &AppState, id: &str, token: Option<&str>) -> Value {
    let uri = format!("/api/datasets/{id}");
    let (st, one) = send(state, Method::GET, &uri, token, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{uri}: {one}");
    let (st, all) = send(state, Method::GET, "/api/datasets", token, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "/api/datasets: {all}");
    let record: Value = serde_json::from_str(&one).unwrap();
    let listing: Value = serde_json::from_str(&all).unwrap();
    let listed = listing
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == json!(id))
        .unwrap_or_else(|| panic!("{id} is listed: {all}"));
    for (what, r) in [("the record", &record), ("the listing", listed)] {
        assert!(
            r.as_object().unwrap().contains_key("shapes_graph_iri"),
            "{what} keeps the field: {r}"
        );
        // The rest of the record is the caller's as before.
        assert_eq!(r["name"], json!(id), "{what}: {r}");
        assert!(r["shacl_on_write"].is_boolean(), "{what}: {r}");
        assert!(r["can_write"].is_boolean(), "{what}: {r}");
    }
    assert_eq!(
        record["shapes_graph_iri"], listed["shapes_graph_iri"],
        "the record and the listing differ: {one} / {listed}"
    );
    if record["shapes_graph_iri"].is_null() {
        assert!(!one.contains(SHAPES), "{uri}: {one}");
        assert!(!all.contains(SHAPES), "/api/datasets: {all}");
    }
    record["shapes_graph_iri"].clone()
}

/// The dataset record carries the dataset's configured shapes graph. When
/// that is a private graph of the dataset, the record names it to who may
/// read it (the dataset's writers, admins, graph-ACL readers) and carries
/// `null` for anyone else who may read the dataset, anonymous callers of a
/// public dataset included.
#[tokio::test]
async fn a_dataset_record_names_its_private_shapes_graph_only_to_who_may_read_it() {
    let (state, admin, alice, bob) = fixture();
    let carol = make_user(&state, "carol");
    make_writer(&state, "pub-ds", "carol");
    state
        .auth_db
        .update_dataset_shacl("pub-ds", true, Some(SHAPES))
        .unwrap();

    for (who, token) in [("an anonymous caller", None), ("a viewer", Some(&bob))] {
        let shapes = record_shapes(&state, "pub-ds", token.map(String::as_str)).await;
        assert_eq!(shapes, Value::Null, "named to {who}");
    }
    for (who, token) in [
        ("its owner", &alice),
        ("a writer", &carol),
        ("an admin", &admin),
    ] {
        let shapes = record_shapes(&state, "pub-ds", Some(token)).await;
        assert_eq!(shapes, json!(SHAPES), "withheld from {who}");
    }

    // A graph-ACL read grant reads the graph over /sparql, so is told of it.
    state
        .auth_db
        .grant_graph_permission("rule-1", SHAPES, "user", "bob", "read", "adm")
        .unwrap();
    let shapes = record_shapes(&state, "pub-ds", Some(&bob)).await;
    assert_eq!(shapes, json!(SHAPES), "withheld from a graph-ACL reader");
}

/// Another dataset's private graph linked as `alice-pub`'s shapes graph is
/// named in `alice-pub`'s record to who may read that graph. A writer of
/// `alice-pub` need not be one: carol is withheld it like a viewer, in the
/// record she reads and in the one her own update is answered with.
#[tokio::test]
async fn another_datasets_private_graph_linked_as_shapes_is_named_only_to_its_readers() {
    let (state, admin, alice, bob) = fixture();
    let carol = make_user(&state, "carol");
    alice_pub(&state);
    make_writer(&state, "alice-pub", "carol");
    link_shapes_into_alice_pub(&state, &alice).await;

    for (who, token) in [
        ("an anonymous caller", None),
        ("a viewer", Some(&bob)),
        ("a writer of alice-pub", Some(&carol)),
    ] {
        let shapes = record_shapes(&state, "alice-pub", token.map(String::as_str)).await;
        assert_eq!(shapes, Value::Null, "named to {who}");
    }
    for (who, token) in [("a writer of both", &alice), ("an admin", &admin)] {
        let shapes = record_shapes(&state, "alice-pub", Some(token)).await;
        assert_eq!(shapes, json!(SHAPES), "withheld from {who}");
    }

    let update = json!({ "name": "alice-pub", "visibility": "public" });
    let (st, text) = send(
        &state,
        Method::PUT,
        "/api/datasets/alice-pub",
        Some(&carol),
        update.clone(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(!text.contains(SHAPES), "carol's update is answered: {text}");
    let record: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(record["shapes_graph_iri"], Value::Null, "{text}");
    assert_eq!(record["name"], json!("alice-pub"), "{text}");
    let (st, text) = send(
        &state,
        Method::PUT,
        "/api/datasets/alice-pub",
        Some(&alice),
        update,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let record: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(record["shapes_graph_iri"], json!(SHAPES), "{text}");
}

/// What `alice-pub` is configured with, as stored.
fn stored_shacl(state: &AppState) -> (bool, Option<String>) {
    let ds = state.auth_db.get_dataset("alice-pub").unwrap().unwrap();
    (
        ds.shacl_on_write,
        ds.shapes_graph_iri.filter(|s| !s.is_empty()),
    )
}

/// A client saves the on-write setting with the shapes graph its record
/// carried. A writer whose record carried none, because the graph is
/// withheld from them, does not unlink it that way (the gate would be on
/// with nothing to validate against): from them a request naming no graph
/// leaves the link as it is. An empty IRI is still the unlink, and from
/// someone who reads the graph so is `null`.
#[tokio::test]
async fn a_writer_withheld_the_shapes_graph_does_not_unlink_it_by_sending_back_their_record() {
    let (state, _admin, alice, _bob) = fixture();
    let carol = make_user(&state, "carol");
    alice_pub(&state);
    make_writer(&state, "alice-pub", "carol");
    link_shapes_into_alice_pub(&state, &alice).await;
    let put = |token: &str, body: Value| {
        let (state, token) = (&state, token.to_string());
        async move {
            let uri = "/api/datasets/alice-pub/shacl";
            let (st, text) = send(state, Method::PUT, uri, Some(&token), body).await;
            assert!(st.is_success(), "{st}: {text}");
            assert!(!text.contains(SHAPES), "{text}");
        }
    };

    put(
        &carol,
        json!({ "shacl_on_write": true, "shapes_graph_iri": null }),
    )
    .await;
    assert_eq!(stored_shacl(&state), (true, Some(SHAPES.to_string())));
    put(&carol, json!({ "shacl_on_write": false })).await;
    assert_eq!(stored_shacl(&state), (false, Some(SHAPES.to_string())));

    // Unlinking is a write of her dataset: hers to make, said outright.
    put(
        &carol,
        json!({ "shacl_on_write": false, "shapes_graph_iri": "" }),
    )
    .await;
    assert_eq!(stored_shacl(&state), (false, None));

    // Alice reads the graph, so her record names it and her `null` means none.
    link_shapes_into_alice_pub(&state, &alice).await;
    assert_eq!(stored_shacl(&state), (false, Some(SHAPES.to_string())));
    put(
        &alice,
        json!({ "shacl_on_write": false, "shapes_graph_iri": null }),
    )
    .await;
    assert_eq!(stored_shacl(&state), (false, None));
}

/// The DCAT catalogue as N-Triples.
async fn catalogue(state: &AppState, token: Option<&str>) -> String {
    let uri = "/.well-known/void?format=ntriples";
    let (st, text) = send(state, Method::GET, uri, token, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    text
}

/// The catalogue's statement that a dataset conforms to the shapes graph.
fn conforms_to_shapes(dataset: &str) -> String {
    format!("/dataset/{dataset}> <http://purl.org/dc/terms/conformsTo> <{SHAPES}>")
}

/// The configured shapes graph is named in three more places a reader of the
/// dataset is served: the DCAT catalogue (`dct:conformsTo`, for a dataset
/// validated on write), the shape-graph selector of the Validation page, and
/// the dataset's conformance layer, which lists the dataset's graphs and the
/// shape graphs bound to it. Each names a private one to who may read it
/// only.
#[tokio::test]
async fn a_private_shapes_graph_is_named_nowhere_else_to_who_may_not_read_it() {
    let (state, admin, alice, bob) = fixture();
    let carol = make_user(&state, "carol");
    alice_pub(&state);
    make_writer(&state, "alice-pub", "carol");
    link_shapes_into_alice_pub(&state, &alice).await;
    for id in ["pub-ds", "alice-pub"] {
        state
            .auth_db
            .update_dataset_shacl(id, true, Some(SHAPES))
            .unwrap();
    }
    let selector = "/api/shacl/dataset-shape-graphs";
    // `pub-ds` holds the graph, `alice-pub` has it bound as shapes.
    let conformance = [
        "/api/datasets/pub-ds/conformance",
        "/api/datasets/alice-pub/conformance",
    ];

    for (who, token) in [
        ("an anonymous caller", None),
        ("a viewer", Some(&bob)),
        ("a writer of alice-pub", Some(&carol)),
    ] {
        let token = token.map(String::as_str);
        let text = catalogue(&state, token).await;
        assert!(!text.contains(SHAPES), "the catalogue, to {who}: {text}");
        for uri in conformance {
            let (st, text) = send(&state, Method::GET, uri, token, Value::Null).await;
            assert_eq!(st, StatusCode::OK, "{text}");
            assert!(!text.contains(SHAPES), "{uri}, to {who}: {text}");
            // The graphs they may read stay in the layer.
            let data = if uri.contains("pub-ds") {
                DATA
            } else {
                ALICE_DATA
            };
            assert!(text.contains(data), "{uri}, to {who}: {text}");
        }
        if let Some(token) = token {
            let (st, text) = get(&state, selector, token).await;
            assert_eq!(st, StatusCode::OK, "{text}");
            assert!(!text.contains(SHAPES), "the selector, to {who}: {text}");
            // The dataset's other shapes stay listed.
            assert!(text.contains("shape_graphs"), "{text}");
        }
    }
    for (who, token) in [("a writer of both", &alice), ("an admin", &admin)] {
        let text = catalogue(&state, Some(token)).await;
        for id in ["pub-ds", "alice-pub"] {
            assert!(
                text.contains(&conforms_to_shapes(id)),
                "the catalogue's {id}, to {who}: {text}"
            );
        }
        for uri in conformance {
            let (st, text) = get(&state, uri, token).await;
            assert_eq!(st, StatusCode::OK, "{text}");
            assert!(text.contains(SHAPES), "{uri}, to {who}: {text}");
        }
        let (st, text) = get(&state, selector, token).await;
        assert_eq!(st, StatusCode::OK, "{text}");
        let listed: Value = serde_json::from_str(&text).unwrap();
        for id in ["pub-ds", "alice-pub"] {
            assert!(
                listed["shape_graphs"].as_array().unwrap().iter().any(|g| {
                    g["dataset_id"] == json!(id) && g["shapes_graph_iri"] == json!(SHAPES)
                }),
                "the selector's {id}, to {who}: {text}"
            );
        }
    }
}

/// The control: a shapes graph no dataset holds as private is named to every
/// reader of the dataset, in its record, the catalogue, the selector and the
/// conformance layer, whichever dataset links it.
#[tokio::test]
async fn a_public_shapes_graph_is_named_to_every_reader_of_the_dataset() {
    let (state, admin, alice, bob) = fixture();
    set_private(&state, false);
    let carol = make_user(&state, "carol");
    alice_pub(&state);
    make_writer(&state, "alice-pub", "carol");
    link_shapes_into_alice_pub(&state, &alice).await;
    for id in ["pub-ds", "alice-pub"] {
        state
            .auth_db
            .update_dataset_shacl(id, true, Some(SHAPES))
            .unwrap();
    }

    for (who, token) in [
        ("an anonymous caller", None),
        ("a viewer", Some(&bob)),
        ("a writer of alice-pub", Some(&carol)),
        ("a writer of both", Some(&alice)),
        ("an admin", Some(&admin)),
    ] {
        let token = token.map(String::as_str);
        for id in ["pub-ds", "alice-pub"] {
            let shapes = record_shapes(&state, id, token).await;
            assert_eq!(shapes, json!(SHAPES), "{id}'s record, to {who}");
        }
        let text = catalogue(&state, token).await;
        for id in ["pub-ds", "alice-pub"] {
            assert!(
                text.contains(&conforms_to_shapes(id)),
                "the catalogue's {id}, to {who}: {text}"
            );
        }
        for id in ["pub-ds", "alice-pub"] {
            let uri = format!("/api/datasets/{id}/conformance");
            let (st, text) = send(&state, Method::GET, &uri, token, Value::Null).await;
            assert_eq!(st, StatusCode::OK, "{text}");
            assert!(text.contains(SHAPES), "{uri}, to {who}: {text}");
        }
        if let Some(token) = token {
            let (st, text) = get(&state, "/api/shacl/dataset-shape-graphs", token).await;
            assert_eq!(st, StatusCode::OK, "{text}");
            assert!(text.contains(SHAPES), "the selector, to {who}: {text}");
        }
    }
}

/// When the private flags cannot be read, nobody is shown to be a reader of
/// the shapes graph: the record carries none, the catalogue and the selector
/// do not name it. An admin reads every graph and needs no lookup.
#[tokio::test]
async fn the_shapes_graph_is_withheld_when_the_private_flags_cannot_be_read() {
    let (state, admin, alice, bob) = fixture();
    // Public, so that only the failed lookup stands between bob and the IRI.
    set_private(&state, false);
    state
        .auth_db
        .update_dataset_shacl("pub-ds", true, Some(SHAPES))
        .unwrap();
    let shapes = record_shapes(&state, "pub-ds", Some(&bob)).await;
    assert_eq!(shapes, json!(SHAPES), "public and readable so far");

    // Every query on the dataset-graph table now fails, as a database error
    // would: the in-memory pool holds one connection.
    state
        .auth_db
        .pool()
        .get()
        .unwrap()
        .execute_batch("DROP TABLE dataset_graphs")
        .unwrap();
    state.auth_db.invalidate_accessible_graphs_cache();

    for (who, token) in [
        ("an anonymous caller", None),
        ("a viewer", Some(&bob)),
        ("its owner", Some(&alice)),
    ] {
        let token = token.map(String::as_str);
        let shapes = record_shapes(&state, "pub-ds", token).await;
        assert_eq!(shapes, Value::Null, "named to {who}");
        let text = catalogue(&state, token).await;
        assert!(!text.contains(SHAPES), "the catalogue, to {who}: {text}");
        if let Some(token) = token {
            let (_, text) = get(&state, "/api/shacl/dataset-shape-graphs", token).await;
            assert!(!text.contains(SHAPES), "the selector, to {who}: {text}");
        }
    }
    let shapes = record_shapes(&state, "pub-ds", Some(&admin)).await;
    assert_eq!(shapes, json!(SHAPES), "withheld from an admin");
}
