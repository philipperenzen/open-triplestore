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
use open_triplestore::auth::models::{
    GraphKind, OwnerType, ResourceRole, Role, SystemRole, Visibility,
};
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
    fixture_owned_by_org(false)
}

/// [`fixture`], with `pub-ds` owned by the organisation `acme` (alice its
/// administrator) when `by_org`.
fn fixture_owned_by_org(by_org: bool) -> (AppState, String, String, String) {
    let (state, admin) = admin_state();
    let alice = make_user(&state, "alice");
    let bob = make_user(&state, "bob");
    let pub_owner = if by_org {
        state
            .auth_db
            .create_organisation("acme", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .add_org_member("alice", "acme", Role::Admin)
            .unwrap();
        (OwnerType::Organisation, "acme")
    } else {
        (OwnerType::User, "alice")
    };
    for (id, (owner_type, owner), vis) in [
        ("pub-ds", pub_owner, Visibility::Public),
        ("bob-ds", (OwnerType::User, "bob"), Visibility::Private),
    ] {
        state
            .auth_db
            .create_dataset(id, id, None, owner_type, owner, vis, None)
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
    // carry its target classes; the catalogue its shapes. (Once `pub-ds` is
    // deleted, the routes about it answer 404.)
    let pub_ds = state.auth_db.get_dataset("pub-ds").unwrap().is_some();
    let of_pub_ds = if pub_ds {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    for (uri, expected) in [
        ("/api/datasets/pub-ds/effective-shapes", of_pub_ds),
        (
            "/api/shacl/bindings?target_kind=dataset&target_id=pub-ds",
            of_pub_ds,
        ),
        ("/api/shacl/shapes", StatusCode::OK),
    ] {
        let (st, text) = get(state, uri, bob).await;
        assert_eq!(st, expected, "{uri}: {text}");
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
        assert_eq!(st, of_pub_ds, "{text}");
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

// ─── After the graph leaves its dataset ──────────────────────────────────────
//
// The flag is a column of the registration. A detach removes the registration
// and, for a graph the dataset may not delete (it did not create it, another
// dataset uses it, it is the dataset's own shapes graph), leaves the graph in
// the store. Whatever else names the graph still does: a Library entry, a link
// from another dataset, the dataset's own shapes setting. The graph left as a
// private one, and it stays one for all of them.

/// Detach `SHAPES` from `pub-ds` as `token`, through the route a writer uses.
async fn detach_shapes(state: &AppState, token: &str) {
    let (st, text) = send(
        state,
        Method::DELETE,
        "/api/datasets/pub-ds/graphs",
        Some(token),
        json!({ "graph_iri": SHAPES }),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "detach: {text}");
    assert!(
        !state.auth_db.dataset_has_graph("pub-ds", SHAPES).unwrap(),
        "the registration is gone"
    );
}

/// Whether the store still holds the shapes graph.
fn shapes_stored(state: &AppState) -> bool {
    matches!(
        state
            .store
            .query(&format!("ASK {{ GRAPH <{SHAPES}> {{ ?s ?p ?o }} }}")),
        Ok(QueryResults::Boolean(true))
    )
}

/// A Library entry adopted the graph in place while it was public, so the
/// entry is public; the graph was then made private. The owner detaches it:
/// she did not create it, so it stays in the store, and the entry still
/// points at it. The entry goes on following the graph. A viewer of the
/// dataset gets nothing of it, by any path the Library serves.
#[tokio::test]
async fn a_detached_private_shapes_graph_stays_private_in_the_library() {
    let (state, admin, alice, bob) = fixture();
    set_private(&state, false);
    let entry = adopt(&state, &alice).await;
    let id = entry["id"].as_str().unwrap().to_string();
    assert_eq!(entry["visibility"], json!("public"), "{entry}");
    set_private(&state, true);
    assert_library_withholds(&state, &bob, &id).await;

    detach_shapes(&state, &alice).await;
    assert!(shapes_stored(&state), "alice did not create the graph");
    assert_library_withholds(&state, &bob, &id).await;

    // An administrator still reads the graph itself.
    let by_name = format!("/store?graph={}", url_encode(SHAPES));
    let (st, text) = get(&state, &by_name, &admin).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(text.contains("SecretShape"), "{text}");
    let (st, text) = get(&state, &by_name, &bob).await;
    assert_ne!(st, StatusCode::OK, "{text}");
    assert!(!leaks(&text), "{text}");

    // Attached again it is private again, and its dataset's writers read the
    // entry as before.
    let (st, text) = send(
        &state,
        Method::POST,
        "/api/datasets/pub-ds/graphs",
        Some(&admin),
        json!({ "graph_iri": SHAPES }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "attach again: {text}");
    assert_library_withholds(&state, &bob, &id).await;
    let (st, text) = get(
        &state,
        &format!("/api/shacl/shape-graphs/{id}/turtle"),
        &alice,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(text.contains("SecretShape"), "{text}");
}

/// The dataset names the graph as its shapes graph (`shapes_graph_iri`), so
/// no detach deletes it, an administrator's included. `GET …/shapes` and the
/// form manifest (anonymous, for a public dataset) keep it from the dataset's
/// viewers after the detach as before it.
#[tokio::test]
async fn a_detached_private_shapes_graph_is_not_served_as_the_datasets_shapes() {
    for by_admin in [false, true] {
        let (state, admin, alice, bob) = fixture();
        state
            .auth_db
            .update_dataset_shacl("pub-ds", false, Some(SHAPES))
            .unwrap();
        let viewer_reads = |when: &'static str| {
            let (state, bob) = (state.clone(), bob.clone());
            async move {
                for uri in [
                    "/api/datasets/pub-ds/shapes",
                    "/api/datasets/pub-ds/shapes?format=shaclc",
                ] {
                    let (st, text) = get(&state, uri, &bob).await;
                    assert!(!leaks(&text), "{when}, {uri} to a viewer: {text}");
                    assert_eq!(st, StatusCode::NOT_FOUND, "{when}, {uri}: {text}");
                }
                let manifest = "/api/datasets/pub-ds/form-manifest";
                for token in [None, Some(bob.as_str())] {
                    let (st, text) = send(&state, Method::GET, manifest, token, Value::Null).await;
                    assert_eq!(st, StatusCode::OK, "{when}: {text}");
                    assert!(!leaks(&text), "{when}, manifest ({token:?}): {text}");
                }
                let (st, text) = send(
                    &state,
                    Method::POST,
                    "/api/datasets/pub-ds/validate?test=true",
                    Some(&bob),
                    Value::Null,
                )
                .await;
                assert!(!leaks(&text), "{when}, a viewer's test run: {st}: {text}");
            }
        };
        viewer_reads("attached").await;

        detach_shapes(&state, if by_admin { &admin } else { &alice }).await;
        assert!(
            shapes_stored(&state),
            "the dataset's own shapes graph is never deleted by a detach"
        );
        viewer_reads(if by_admin {
            "detached by an admin"
        } else {
            "detached by the owner"
        })
        .await;

        // An administrator reads every graph.
        let (st, text) = get(&state, "/api/datasets/pub-ds/shapes", &admin).await;
        assert_eq!(st, StatusCode::OK, "{text}");
        assert!(text.contains("SecretShape"), "{text}");

        // The dataset's writers read its graphs, and this is no longer one
        // of them: the owner gets it here no more than over `/sparql`. Nor
        // may she attach it again, as it holds data she may not write
        // directly. An administrator may, and it comes back private: hers
        // to read again, and still not a viewer's.
        let (st, text) = get(&state, "/api/datasets/pub-ds/shapes", &alice).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "the owner: {text}");
        assert!(!leaks(&text), "{text}");
        let attach = json!({ "graph_iri": SHAPES });
        let graphs = "/api/datasets/pub-ds/graphs";
        let (st, text) = send(&state, Method::POST, graphs, Some(&alice), attach.clone()).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "the owner attaches: {text}");
        let (st, text) = send(&state, Method::POST, graphs, Some(&admin), attach).await;
        assert_eq!(st, StatusCode::CREATED, "an admin attaches: {text}");
        let (st, text) = get(&state, "/api/datasets/pub-ds/shapes", &alice).await;
        assert_eq!(st, StatusCode::OK, "attached again, the owner: {text}");
        assert!(text.contains("SecretShape"), "{text}");
        viewer_reads("attached again").await;
    }
}

/// Alice linked the private graph into her other public dataset while she
/// could read it. Detached from `pub-ds`, it stays in the store (`alice-pub`
/// uses it) and stays private: `alice-pub`'s viewers get neither the graph nor
/// a report that names its shapes, exactly as before the detach.
#[tokio::test]
async fn a_detached_private_shapes_graph_is_not_read_through_a_link() {
    let (state, _admin, alice, bob) = fixture();
    alice_pub(&state);
    link_shapes_into_alice_pub(&state, &alice).await;
    let run = validate_alice_pub(&state, &alice).await;
    let viewer_reads = |when: &'static str| {
        let (state, bob, run) = (state.clone(), bob.clone(), run.clone());
        async move {
            let (st, text) = get(&state, "/api/datasets/alice-pub/shapes", &bob).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{when}: {text}");
            assert!(!leaks(&text), "{when}: {text}");
            for uri in [
                "/api/datasets/alice-pub/validation/latest".to_string(),
                format!("/api/datasets/alice-pub/validation/runs/{run}"),
            ] {
                let (st, text) = get(&state, &uri, &bob).await;
                assert_eq!(st, StatusCode::OK, "{when}, {uri}: {text}");
                assert!(!leaks(&text), "{when}, {uri}: {text}");
            }
            let manifest = "/api/datasets/alice-pub/form-manifest";
            let (st, text) = send(&state, Method::GET, manifest, None, Value::Null).await;
            assert_eq!(st, StatusCode::OK, "{when}: {text}");
            assert!(!leaks(&text), "{when}, anonymous manifest: {text}");
            let (st, text) = send(
                &state,
                Method::POST,
                "/api/datasets/alice-pub/validate?test=true",
                Some(&bob),
                Value::Null,
            )
            .await;
            assert!(!leaks(&text), "{when}, a viewer's test run: {st}: {text}");
        }
    };
    viewer_reads("attached").await;

    detach_shapes(&state, &alice).await;
    assert!(shapes_stored(&state), "alice-pub still uses the graph");
    viewer_reads("detached").await;
}

// ─── A private graph that a dataset delete keeps ─────────────────────────────
//
// Deleting a dataset deletes its registrations, and with them the flag and
// the records of the graphs it detached while private. It keeps a stored
// graph it did not create, or one another dataset uses
// (`graphs_deletable_with_dataset`); deleting an organisation deletes its
// datasets the same way. Whatever else names the kept graph still does: a
// Library entry, a link from another dataset. The graph was private when its
// dataset went, and it stays private for all of them.

/// Who deletes `pub-ds`, and how.
#[derive(Clone, Copy, Debug)]
enum Deleter {
    /// Its owner alice, through `DELETE /api/datasets/pub-ds`.
    Owner,
    /// An administrator, through the same route.
    Admin,
    /// Alice deletes the organisation that owns it
    /// ([`fixture_owned_by_org`]).
    Organisation,
}

/// `who` deletes `pub-ds`.
async fn delete_pub_ds(state: &AppState, who: Deleter, admin: &str, alice: &str) {
    let (uri, token) = match who {
        Deleter::Owner => ("/api/datasets/pub-ds", alice),
        Deleter::Admin => ("/api/datasets/pub-ds", admin),
        Deleter::Organisation => ("/api/organisations/acme", alice),
    };
    let (st, text) = send(state, Method::DELETE, uri, Some(token), Value::Null).await;
    assert!(st.is_success(), "{who:?} deletes: {st}: {text}");
    assert!(
        state.auth_db.get_dataset("pub-ds").unwrap().is_none(),
        "{who:?}: pub-ds is gone"
    );
}

/// Everything an anonymous caller could reach Library entry `id` by: all
/// refused or silent.
async fn assert_library_withholds_anonymously(state: &AppState, id: &str) {
    let (st, text) = send(
        state,
        Method::GET,
        "/api/shacl/shape-graphs",
        None,
        Value::Null,
    )
    .await;
    assert!(!text.contains(SHAPES), "listed anonymously: {st}: {text}");
    for uri in [
        format!("/api/shacl/shape-graphs/{id}"),
        format!("/api/shacl/shape-graphs/{id}/turtle"),
        format!("/api/shacl/shape-graphs/{id}/turtle?format=shaclc"),
        format!("/api/shacl/shape-graphs/{id}/revisions"),
        format!("/api/shacl/shape-graphs/{id}/revisions/1"),
    ] {
        let (st, text) = send(state, Method::GET, &uri, None, Value::Null).await;
        assert_ne!(st, StatusCode::OK, "{uri} anonymously: {text}");
        assert!(!leaks(&text), "{uri} anonymously: {text}");
    }
}

/// Alice linked the private graph into her other public dataset while she
/// could read it. Then `pub-ds` goes: she deletes it, an administrator does,
/// or she deletes its organisation. `alice-pub` uses the graph, so it stays in
/// the store, and it stays private: `alice-pub`'s viewers and anonymous
/// callers get neither the graph nor a stored report that names its shapes,
/// exactly as before the delete. An administrator and a graph-ACL reader
/// still read it. Its former owner no longer does here, as over `/sparql`.
#[tokio::test]
async fn a_private_shapes_graph_kept_by_a_dataset_delete_is_not_read_through_a_link() {
    for who in [Deleter::Owner, Deleter::Admin, Deleter::Organisation] {
        let (state, admin, alice, bob) = fixture_owned_by_org(matches!(who, Deleter::Organisation));
        alice_pub(&state);
        link_shapes_into_alice_pub(&state, &alice).await;
        let run = validate_alice_pub(&state, &alice).await;
        let withheld = |when: String| {
            let (state, bob, run) = (state.clone(), bob.clone(), run.clone());
            async move {
                for token in [Some(bob.as_str()), None] {
                    // The shapes and the validation runs need a signed-in
                    // caller; a public dataset's form manifest does not.
                    let signed_in = |served: StatusCode| match token {
                        Some(_) => served,
                        None => StatusCode::UNAUTHORIZED,
                    };
                    for (uri, expected) in [
                        (
                            "/api/datasets/alice-pub/shapes".to_string(),
                            signed_in(StatusCode::NOT_FOUND),
                        ),
                        (
                            "/api/datasets/alice-pub/shapes?format=shaclc".to_string(),
                            signed_in(StatusCode::NOT_FOUND),
                        ),
                        (
                            "/api/datasets/alice-pub/validation/latest".to_string(),
                            signed_in(StatusCode::OK),
                        ),
                        (
                            format!("/api/datasets/alice-pub/validation/runs/{run}"),
                            signed_in(StatusCode::OK),
                        ),
                        (
                            "/api/datasets/alice-pub/form-manifest".to_string(),
                            StatusCode::OK,
                        ),
                    ] {
                        let (st, text) = send(&state, Method::GET, &uri, token, Value::Null).await;
                        assert!(!leaks(&text), "{when}, {uri} ({token:?}): {text}");
                        assert_eq!(st, expected, "{when}, {uri} ({token:?}): {text}");
                    }
                }
                let (st, text) = send(
                    &state,
                    Method::POST,
                    "/api/datasets/alice-pub/validate?test=true",
                    Some(&bob),
                    Value::Null,
                )
                .await;
                assert!(!leaks(&text), "{when}, a viewer's test run: {st}: {text}");
            }
        };
        withheld("attached".to_string()).await;

        delete_pub_ds(&state, who, &admin, &alice).await;
        assert!(shapes_stored(&state), "{who:?}: alice-pub uses the graph");
        withheld(format!("deleted by {who:?}")).await;

        let (st, text) = get(&state, "/api/datasets/alice-pub/shapes", &admin).await;
        assert_eq!(st, StatusCode::OK, "{who:?}, an admin: {text}");
        assert!(text.contains("SecretShape"), "{who:?}, an admin: {text}");
        let (st, text) = get(&state, "/api/datasets/alice-pub/shapes", &alice).await;
        assert_eq!(
            st,
            StatusCode::NOT_FOUND,
            "{who:?}, the former owner: {text}"
        );
        assert!(!leaks(&text), "{who:?}, the former owner: {text}");

        state
            .auth_db
            .grant_graph_permission("rule-1", SHAPES, "user", "bob", "read", "adm")
            .unwrap();
        let (st, text) = get(&state, "/api/datasets/alice-pub/shapes", &bob).await;
        assert_eq!(st, StatusCode::OK, "{who:?}, a graph-ACL reader: {text}");
        assert!(text.contains("SecretShape"), "{who:?}: {text}");
    }
}

/// A Library entry adopted the graph in place while it was public, so the
/// entry is public; the graph was then made private. Alice deletes `pub-ds`,
/// or its organisation: she did not create the graph, so it stays in the
/// store, and the entry still points at it. The entry goes on following the
/// graph: a viewer and an anonymous caller get nothing of it, by any path the
/// Library serves. An administrator still reads the graph itself; its former
/// owner reads it no more than over `/sparql`.
#[tokio::test]
async fn a_private_shapes_graph_kept_by_a_dataset_delete_stays_private_in_the_library() {
    for who in [Deleter::Owner, Deleter::Organisation] {
        let (state, admin, alice, bob) = fixture_owned_by_org(matches!(who, Deleter::Organisation));
        set_private(&state, false);
        let entry = adopt(&state, &alice).await;
        let id = entry["id"].as_str().unwrap().to_string();
        assert_eq!(entry["visibility"], json!("public"), "{who:?}: {entry}");
        set_private(&state, true);
        assert_library_withholds(&state, &bob, &id).await;
        assert_library_withholds_anonymously(&state, &id).await;

        delete_pub_ds(&state, who, &admin, &alice).await;
        assert!(
            shapes_stored(&state),
            "{who:?}: alice did not create the graph"
        );
        assert_library_withholds(&state, &bob, &id).await;
        assert_library_withholds_anonymously(&state, &id).await;

        let by_name = format!("/store?graph={}", url_encode(SHAPES));
        let (st, text) = get(&state, &by_name, &admin).await;
        assert_eq!(st, StatusCode::OK, "{who:?}, an admin: {text}");
        assert!(text.contains("SecretShape"), "{who:?}: {text}");
        let turtle = format!("/api/shacl/shape-graphs/{id}/turtle");
        let (st, text) = get(&state, &turtle, &alice).await;
        assert_eq!(
            st,
            StatusCode::FORBIDDEN,
            "{who:?}, the former owner: {text}"
        );
        assert!(!leaks(&text), "{who:?}, the former owner: {text}");
    }
}

/// Shapes with nothing of `PRIVATE_SHAPES` in them, for an admin's pipeline.
const LABEL_SHAPES: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
    @prefix ex: <http://ex.org/> .\n\
    ex:LabelShape a sh:NodeShape ; sh:targetClass ex:Thing ;\n\
      sh:property [ sh:path ex:label ; sh:minCount 1 ] .\n";

/// A SHACL Studio pipeline writes its report into a graph its owner names
/// (`results_target_graph`) and attaches that graph to the dataset holding
/// the data it ran over. A graph on record as private (detached while
/// private, or kept by the delete of the dataset that held it as private)
/// comes back private when a dataset registers it, and the pipeline's attach
/// is such a registration: it must not clear the flag the registration got
/// back. Here an administrator's pipeline writes its report into the private
/// shapes graph; the dataset it lands in holds it as private, so that
/// dataset's viewers and anonymous callers read nothing of it.
#[tokio::test]
async fn a_pipeline_attaches_a_graph_on_record_as_private_as_private() {
    for deleted in [false, true] {
        let (state, admin, alice, bob) = fixture();
        alice_pub(&state);
        let (home, data) = if deleted {
            delete_pub_ds(&state, Deleter::Owner, &admin, &alice).await;
            ("alice-pub", ALICE_DATA)
        } else {
            detach_shapes(&state, &alice).await;
            ("pub-ds", DATA)
        };
        assert!(shapes_stored(&state), "{home}: alice did not create it");

        let (st, text) = send(
            &state,
            Method::POST,
            "/api/shacl/shape-graphs",
            Some(&admin),
            json!({ "name": "labels", "turtle": LABEL_SHAPES }),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{text}");
        let sg: Value = serde_json::from_str(&text).unwrap();
        let (st, text) = send(
            &state,
            Method::POST,
            "/api/shacl/pipelines",
            Some(&admin),
            json!({ "name": "report", "graph_iris": [data], "shape_graph_ids": [sg["id"]],
                    "results_target": "new_graph", "results_target_graph": SHAPES }),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{text}");
        let pipeline: Value = serde_json::from_str(&text).unwrap();
        let run = format!(
            "/api/shacl/pipelines/{}/run",
            pipeline["id"].as_str().unwrap()
        );
        let (st, text) = send(&state, Method::POST, &run, Some(&admin), Value::Null).await;
        assert_eq!(st, StatusCode::OK, "{text}");

        let flag = state
            .auth_db
            .list_dataset_graph_entries(home)
            .unwrap()
            .into_iter()
            .find(|e| e.graph_iri == SHAPES)
            .map(|e| e.private);
        assert_eq!(flag, Some(true), "{home}: attached, and private");

        let by_name = format!("/store?graph={}", url_encode(SHAPES));
        for token in [Some(bob.as_str()), None] {
            let (st, text) = send(&state, Method::GET, &by_name, token, Value::Null).await;
            assert!(!leaks(&text), "{home} ({token:?}): {text}");
            assert_ne!(st, StatusCode::OK, "{home} ({token:?}): {text}");
        }
        let (st, text) = get(&state, &by_name, &admin).await;
        assert_eq!(st, StatusCode::OK, "{home}, an admin: {text}");
        assert!(text.contains("SecretShape"), "{home}: {text}");
    }
}

// ─── Bindings a dataset delete leaves behind ─────────────────────────────────
//
// SHACL Studio bindings are triples keyed by IRI: a dataset's by
// `{base}/dataset/{id}`, a graph's by the graph's own name. A dataset id is a
// slug of its name, free again once the dataset is deleted, and its own
// graphs live under `{base}/dataset/{id}/`. So the bindings a delete left
// behind passed to the next dataset of that name: shapes its owner never
// chose, that shape its runs and whose Turtle its form manifest serves to
// its readers.

/// alice's private Library entry, holding `PRIVATE_SHAPES`; returns
/// `(id, graph_iri)`.
async fn alice_private_entry(state: &AppState, alice: &str) -> (String, String) {
    let (st, text) = send(
        state,
        Method::POST,
        "/api/shacl/shape-graphs",
        Some(alice),
        json!({ "name": "alice rules", "visibility": "private", "turtle": PRIVATE_SHAPES }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{text}");
    let entry: Value = serde_json::from_str(&text).unwrap();
    (
        entry["id"].as_str().unwrap().to_string(),
        entry["graph_iri"].as_str().unwrap().to_string(),
    )
}

/// `owner` creates a dataset named `name` through the route; returns its id.
async fn create_dataset_named(
    state: &AppState,
    token: &str,
    owner: &str,
    name: &str,
    visibility: &str,
) -> String {
    let (st, text) = send(
        state,
        Method::POST,
        "/api/datasets",
        Some(token),
        json!({ "name": name, "owner_type": "user", "owner_id": owner,
                "visibility": visibility }),
    )
    .await;
    assert!(st.is_success(), "create {name}: {st}: {text}");
    let ds: Value = serde_json::from_str(&text).unwrap();
    ds["id"].as_str().unwrap().to_string()
}

/// `user` registers the empty graph `graph` to dataset `id` (so the dataset
/// created it) and writes `ttl` into it through the Graph Store, which a
/// non-admin writes by a graph-ACL grant.
async fn write_own_graph(
    state: &AppState,
    (user, token): (&str, &str),
    id: &str,
    graph: &str,
    ttl: &str,
) {
    state
        .auth_db
        .grant_graph_permission(&format!("w-{user}"), graph, "user", user, "write", "adm")
        .unwrap();
    let (st, text) = send(
        state,
        Method::POST,
        &format!("/api/datasets/{id}/graphs"),
        Some(token),
        json!({ "graph_iri": graph }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register {graph}: {text}");
    let (st, text) = put_data(state, token, graph, ttl).await;
    assert!(st.is_success(), "write {graph}: {st}: {text}");
    assert!(!leaks(&text), "{text}");
}

async fn bind(state: &AppState, token: &str, entry: &str, kind: &str, target: &str) {
    let (st, text) = send(
        state,
        Method::POST,
        "/api/shacl/bindings",
        Some(token),
        json!({ "shape_graph_id": entry, "target": { "kind": kind, "id": target } }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "bind {kind} {target}: {text}");
}

/// alice binds her private entry to her private dataset and to a graph of
/// its own, then deletes the dataset. bob gets a dataset of the same name,
/// which gets the same id: a private one he creates himself, or a public one
/// an administrator creates for him (creating a public dataset takes the
/// publisher role). He writes a graph under the name alice's had. Neither
/// binding comes with them: bob, and an anonymous caller on the public one,
/// get nothing of alice's entry, and her shapes neither refuse his data nor
/// report on it.
#[tokio::test]
async fn a_deleted_datasets_bindings_do_not_pass_to_the_next_dataset_of_its_name() {
    for public in [false, true] {
        let (state, admin, alice, bob) = fixture();
        let (entry, entry_graph) = alice_private_entry(&state, &alice).await;
        let id = create_dataset_named(&state, &alice, "alice", "Secret Rules", "private").await;
        let own = format!(
            "{}/dataset/{id}/records",
            state.base_url.trim_end_matches('/')
        );
        write_own_graph(
            &state,
            ("alice", &alice),
            &id,
            &own,
            "<http://ex.org/r1> <http://ex.org/p> \"r\" .",
        )
        .await;
        bind(&state, &alice, &entry, "dataset", &id).await;
        bind(&state, &alice, &entry, "graph", &own).await;

        let uri = format!("/api/datasets/{id}");
        let (st, text) = send(&state, Method::DELETE, &uri, Some(&alice), Value::Null).await;
        assert_eq!(st, StatusCode::NO_CONTENT, "{text}");

        let (creator, visibility) = if public {
            (&admin, "public")
        } else {
            (&bob, "private")
        };
        let reused = create_dataset_named(&state, creator, "bob", "Secret Rules", visibility).await;
        assert_eq!(reused, id, "the name's slug is free again");
        let withheld = |when: &'static str| {
            let (state, bob, id, entry_graph) =
                (state.clone(), bob.clone(), id.clone(), entry_graph.clone());
            async move {
                let uri = format!("/api/datasets/{id}/effective-shapes");
                let (st, text) = get(&state, &uri, &bob).await;
                assert_eq!(st, StatusCode::OK, "{visibility}, {when}: {text}");
                assert!(
                    !text.contains(&entry_graph),
                    "{visibility}, {when}, {uri}: {text}"
                );
                let manifest = format!("/api/datasets/{id}/form-manifest");
                for token in [Some(bob.as_str()), None] {
                    let (st, text) = send(&state, Method::GET, &manifest, token, Value::Null).await;
                    assert!(
                        !leaks(&text),
                        "{visibility}, {when}, manifest ({token:?}): {text}"
                    );
                    assert!(
                        !text.contains(&entry_graph),
                        "{visibility}, {when}, manifest ({token:?}): {text}"
                    );
                    if token.is_some() || public {
                        assert_eq!(st, StatusCode::OK, "{visibility}, {when}: {text}");
                    }
                }
                let run = format!("/api/datasets/{id}/validate?test=true");
                let (st, text) = send(&state, Method::POST, &run, Some(&bob), Value::Null).await;
                assert!(
                    !leaks(&text),
                    "{visibility}, {when}, bob's test run: {st}: {text}"
                );
            }
        };
        withheld("the same id").await;

        // A Thing breaks alice's shapes, so they would refuse or report it.
        write_own_graph(
            &state,
            ("bob", &bob),
            &id,
            &own,
            "<http://ex.org/b1> a <http://ex.org/Thing> .",
        )
        .await;
        withheld("bob's graph under the old name").await;
    }
}

/// Bindings left by a dataset deleted before deletes removed them go with
/// the admin sweep (`POST /api/admin/versions/sweep`): the dataset's own, and
/// those of graphs in its namespace that hold no data and no dataset holds.
/// A graph in that namespace another dataset still holds keeps its
/// bindings, and so does a dataset that exists.
#[tokio::test]
async fn the_admin_sweep_removes_bindings_a_deleted_dataset_left() {
    use open_triplestore::shacl_studio::bindings;
    let (state, admin, alice, bob) = fixture();
    let (_, entry_graph) = alice_private_entry(&state, &alice).await;
    let base = state.base_url.trim_end_matches('/').to_string();
    let gone = format!("{base}/dataset/gone-ds");
    let empty = format!("{base}/dataset/gone-ds/records");
    let held = format!("{base}/dataset/gone-ds/shared");
    let live = format!("{base}/dataset/pub-ds");
    load(
        &state.store,
        &held,
        "<http://ex.org/k> <http://ex.org/p> \"k\" .",
    );
    state.auth_db.add_dataset_graph("bob-ds", &held).unwrap();
    for target in [&gone, &empty, &held, &live] {
        bindings::add_binding(&state.store, target, &entry_graph).unwrap();
    }

    let sweep = "/api/admin/versions/sweep";
    let (st, text) = send(&state, Method::POST, sweep, Some(&bob), Value::Null).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{text}");
    let (st, text) = send(&state, Method::POST, sweep, Some(&admin), Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let report: Value = serde_json::from_str(&text).unwrap();
    let mut removed: Vec<String> =
        serde_json::from_value(report["bindings_removed"].clone()).unwrap_or_default();
    removed.sort();
    assert_eq!(removed, [gone.clone(), empty.clone()], "{text}");
    for target in [&gone, &empty] {
        assert!(bindings::bindings_for_target(&state.store, target).is_empty());
    }
    for target in [&held, &live] {
        assert_eq!(
            bindings::bindings_for_target(&state.store, target),
            std::slice::from_ref(&entry_graph),
            "{target}"
        );
    }
}

// ─── The dataset record ──────────────────────────────────────────────────────

/// `shapes_graph_iri` of dataset `id` as `token` (`None`: an anonymous
/// caller) reads it in `GET /api/datasets/{id}` and in the `GET /api/datasets`
/// listing. The two agree, the field is there either way, and a caller who
/// is not given the IRI in it is given it nowhere in either answer.
/// `shapes_graph_withheld` says so exactly when the dataset has a shapes
/// graph stored and the caller is served none.
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
    let configured = state
        .auth_db
        .get_dataset(id)
        .unwrap()
        .unwrap()
        .shapes_graph_iri
        .is_some_and(|s| !s.is_empty());
    let withheld = json!(configured && record["shapes_graph_iri"].is_null());
    for (what, r) in [("the record", &record), ("the listing", listed)] {
        assert_eq!(r["shapes_graph_withheld"], withheld, "{what}: {r}");
    }
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

    // A dataset with no shapes graph says it has none, not that one is
    // withheld (`record_shapes` checks the flag against what is stored).
    let shapes = record_shapes(&state, "bob-ds", Some(&bob)).await;
    assert_eq!(shapes, Value::Null);

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
    assert_eq!(record["shapes_graph_withheld"], json!(true), "{text}");
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
    assert_eq!(record["shapes_graph_withheld"], json!(false), "{text}");
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
    // Her record now says there is none (the empty IRI as stored), not that
    // one is withheld.
    let shapes = record_shapes(&state, "alice-pub", Some(&carol)).await;
    assert!(shapes.as_str().is_none_or(str::is_empty), "{shapes}");

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
