//! What SHACL Studio shows or validates follows what the caller may read —
//! through the real router. "May read" is the set `/sparql` scopes a query
//! to: graphs of datasets the caller can access (a private graph only for its
//! dataset's writers) plus graph-ACL read grants; admins read every graph.
//!
//! * `GET /api/shacl/shapes` lists, and drills into, only graphs the caller
//!   may read. A graph registered in the Library stays governed by its entry.
//! * A validation pipeline reads its data and shape graphs and hands the
//!   report (focus nodes, values) to whoever runs it or opens a run. Creating,
//!   updating and running a pipeline, and opening a run's report, need read
//!   access to every dataset, data graph and shape graph in its scope, checked
//!   each time, so a grant since revoked gives nothing.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use open_triplestore::store::TripleStore;
use serde_json::{json, Value};
use tower::ServiceExt as _;

/// In alice's private dataset: shapes embedded with the data they describe.
const PRIVATE_GRAPH: &str = "http://alice.example/private";
/// Loaded by an admin, registered to no dataset.
const ADMIN_GRAPH: &str = "http://admin.example/shapes";
/// In a public dataset.
const PUBLIC_GRAPH: &str = "http://pub.example/shapes";
/// In the same public dataset, but marked private: only its writers read it.
const PUBLIC_DS_PRIVATE_GRAPH: &str = "http://pub.example/private-shapes";
/// Readable to mallory through a graph-ACL read grant only.
const GRANTED_GRAPH: &str = "http://granted.example/shapes";
/// Mallory's own data, in her own dataset.
const MALLORY_GRAPH: &str = "http://mallory.example/data";

const SECRET_VALUE: &str = "123-45-6789";

fn secret_ttl() -> String {
    format!(
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         @prefix ex: <http://ex.org/> .\n\
         ex:SecretShape a sh:NodeShape ;\n\
           rdfs:label \"Alice's secret shape\" ;\n\
           sh:targetClass ex:SecretClass ;\n\
           sh:property ex:SecretShape-code .\n\
         ex:SecretShape-code a sh:PropertyShape ; sh:path ex:secretCode .\n\
         ex:patient1 a ex:Patient ; ex:ssn \"{SECRET_VALUE}\" .\n"
    )
}

fn shapes_ttl(class: &str) -> String {
    format!(
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
         <http://ex.org/{class}Shape> a sh:NodeShape ;\n\
           sh:targetClass <http://ex.org/{class}> ;\n\
           sh:property [ sh:path <http://ex.org/name> ; sh:minCount 1 ] .\n"
    )
}

/// Shapes whose report echoes every `ex:ssn` value: none may be longer than 3.
const SSN_SHAPES: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
    @prefix ex: <http://ex.org/> .\n\
    ex:PatientShape a sh:NodeShape ; sh:targetClass ex:Patient ;\n\
      sh:property [ sh:path ex:ssn ; sh:maxLength 3 ] .\n";

async fn send(
    state: &AppState,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, String) {
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

async fn get(state: &AppState, uri: &str, token: &str) -> (StatusCode, String) {
    send(state, Method::GET, uri, token, Value::Null).await
}

fn make_user(state: &AppState, id: &str) -> String {
    state
        .auth_db
        .create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
        .unwrap();
    mint_token(id, id, "user")
}

fn make_dataset(state: &AppState, id: &str, owner: &str, vis: Visibility) {
    state
        .auth_db
        .create_dataset(id, id, None, OwnerType::User, owner, vis, None)
        .unwrap();
}

fn load(store: &TripleStore, graph: &str, ttl: &str) {
    store
        .load_str(ttl, oxigraph::io::RdfFormat::Turtle, Some(graph))
        .unwrap();
}

/// Alice's private dataset holding `PRIVATE_GRAPH`, and mallory's own
/// private dataset holding `MALLORY_GRAPH`. Returns `(alice, mallory)`.
fn tenants(state: &AppState) -> (String, String) {
    let alice = make_user(state, "alice");
    let mallory = make_user(state, "mallory");
    make_dataset(state, "alice-ds", "alice", Visibility::Private);
    load(&state.store, PRIVATE_GRAPH, &secret_ttl());
    state
        .auth_db
        .add_dataset_graph("alice-ds", PRIVATE_GRAPH)
        .unwrap();
    make_dataset(state, "mallory-ds", "mallory", Visibility::Private);
    load(
        &state.store,
        MALLORY_GRAPH,
        "<http://ex.org/m1> a <http://ex.org/Patient> ; <http://ex.org/ssn> \"1\" .",
    );
    state
        .auth_db
        .add_dataset_graph("mallory-ds", MALLORY_GRAPH)
        .unwrap();
    (alice, mallory)
}

fn listed_graphs(body: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(body).unwrap();
    v["graphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["graph"].as_str().unwrap().to_string())
        .collect()
}

fn drill_down(graph: &str) -> String {
    format!("/api/shacl/shapes?graph={}", url_encode(graph))
}

async fn create_shape_graph(state: &AppState, token: &str, vis: &str, ttl: &str) -> String {
    let (st, body) = send(
        state,
        Method::POST,
        "/api/shacl/shape-graphs",
        token,
        json!({ "name": "shapes", "visibility": vis, "turtle": ttl }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create shape graph: {body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    v["id"].as_str().unwrap().to_string()
}

async fn create_pipeline(state: &AppState, token: &str, body: Value) -> (StatusCode, String) {
    send(state, Method::POST, "/api/shacl/pipelines", token, body).await
}

fn id_of(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap();
    v["id"].as_str().unwrap().to_string()
}

// ─── The shapes catalogue ────────────────────────────────────────────────────

#[tokio::test]
async fn the_shapes_catalogue_lists_only_graphs_the_caller_may_read() {
    let (state, admin) = admin_state();
    let (alice, mallory) = tenants(&state);
    make_dataset(&state, "pub-ds", "alice", Visibility::Public);
    for (g, class) in [
        (PUBLIC_GRAPH, "Pub"),
        (PUBLIC_DS_PRIVATE_GRAPH, "PubPrivate"),
    ] {
        load(&state.store, g, &shapes_ttl(class));
        state.auth_db.add_dataset_graph("pub-ds", g).unwrap();
    }
    state
        .auth_db
        .set_dataset_graph_private("pub-ds", PUBLIC_DS_PRIVATE_GRAPH, true)
        .unwrap();
    load(&state.store, ADMIN_GRAPH, &shapes_ttl("Admin"));
    load(&state.store, GRANTED_GRAPH, &shapes_ttl("Granted"));
    state
        .auth_db
        .grant_graph_permission("rule-1", GRANTED_GRAPH, "user", "mallory", "read", "adm")
        .unwrap();

    // Mallory: the public graph and her grant, nothing else.
    let (st, body) = get(&state, "/api/shacl/shapes", &mallory).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let graphs = listed_graphs(&body);
    assert!(graphs.contains(&PUBLIC_GRAPH.to_string()), "{body}");
    assert!(graphs.contains(&GRANTED_GRAPH.to_string()), "{body}");
    for hidden in [PRIVATE_GRAPH, ADMIN_GRAPH, PUBLIC_DS_PRIVATE_GRAPH] {
        assert!(
            !graphs.contains(&hidden.to_string()),
            "<{hidden}> is not readable by mallory but is listed: {body}"
        );
    }

    // Nor can she drill into one she may not read.
    for hidden in [PRIVATE_GRAPH, ADMIN_GRAPH, PUBLIC_DS_PRIVATE_GRAPH] {
        let (st, body) = get(&state, &drill_down(hidden), &mallory).await;
        assert_eq!(
            st,
            StatusCode::FORBIDDEN,
            "drill-down into <{hidden}>: {body}"
        );
        assert!(!body.contains("SecretShape"), "{body}");
        assert!(!body.contains("http://ex.org/"), "{body}");
    }
    for readable in [PUBLIC_GRAPH, GRANTED_GRAPH] {
        let (st, body) = get(&state, &drill_down(readable), &mallory).await;
        assert_eq!(st, StatusCode::OK, "drill-down into <{readable}>: {body}");
        assert!(body.contains("Shape"), "{body}");
    }

    // Alice reads her own private graph, shapes and all.
    let (_, body) = get(&state, "/api/shacl/shapes", &alice).await;
    assert!(
        listed_graphs(&body).contains(&PRIVATE_GRAPH.to_string()),
        "{body}"
    );
    let (st, body) = get(&state, &drill_down(PRIVATE_GRAPH), &alice).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.contains("http://ex.org/SecretShape"), "{body}");
    assert!(body.contains("Alice's secret shape"), "{body}");

    // An admin reads every graph.
    let (_, body) = get(&state, "/api/shacl/shapes", &admin).await;
    let graphs = listed_graphs(&body);
    for g in [
        PRIVATE_GRAPH,
        ADMIN_GRAPH,
        PUBLIC_DS_PRIVATE_GRAPH,
        PUBLIC_GRAPH,
    ] {
        assert!(
            graphs.contains(&g.to_string()),
            "admin misses <{g}>: {body}"
        );
    }
    let (st, _) = get(&state, &drill_down(ADMIN_GRAPH), &admin).await;
    assert_eq!(st, StatusCode::OK);
}

// ─── Validation pipelines ────────────────────────────────────────────────────

/// Mallory may not define a pipeline over anything she may not read, by any
/// of the ways a pipeline names its scope, at create or at update.
#[tokio::test]
async fn a_pipeline_is_defined_only_over_what_its_author_may_read() {
    let (state, _admin) = admin_state();
    let (alice, mallory) = tenants(&state);
    let mallory_sg = create_shape_graph(&state, &mallory, "private", SSN_SHAPES).await;
    let alice_sg = create_shape_graph(&state, &alice, "private", SSN_SHAPES).await;

    let refused = [
        json!({ "name": "p", "graph_iris": [PRIVATE_GRAPH], "shape_graph_ids": [mallory_sg] }),
        json!({ "name": "p", "targets": [{ "kind": "graph", "id": PRIVATE_GRAPH }],
                "shape_graph_ids": [mallory_sg] }),
        json!({ "name": "p", "targets": [{ "kind": "dataset", "id": "alice-ds" }],
                "shape_graph_ids": [mallory_sg] }),
        json!({ "name": "p", "dataset_ids": ["alice-ds"], "shape_graph_ids": [mallory_sg] }),
        json!({ "name": "p", "targets": [{ "kind": "graph", "id": ADMIN_GRAPH }],
                "shape_graph_ids": [mallory_sg] }),
        // Someone else's private shape graph, as the shapes or as the data.
        json!({ "name": "p", "targets": [{ "kind": "graph", "id": MALLORY_GRAPH }],
                "shape_graph_ids": [alice_sg] }),
        json!({ "name": "p", "targets": [{ "kind": "shapegraph", "id": alice_sg }] }),
    ];
    for body in refused {
        let (st, text) = create_pipeline(&state, &mallory, body.clone()).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "create {body}: {text}");
    }
    let (_, text) = get(&state, "/api/shacl/pipelines", &mallory).await;
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!([]),
        "a refused create stores nothing: {text}"
    );

    // Over her own data and shapes, she may.
    let (st, text) = create_pipeline(
        &state,
        &mallory,
        json!({ "name": "mine", "targets": [{ "kind": "graph", "id": MALLORY_GRAPH }],
                "shape_graph_ids": [mallory_sg] }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{text}");
    let pid = id_of(&text);

    // An update may not widen it to alice's graph.
    let (st, text) = send(
        &state,
        Method::PUT,
        &format!("/api/shacl/pipelines/{pid}"),
        &mallory,
        json!({ "name": "mine", "graph_iris": [PRIVATE_GRAPH], "shape_graph_ids": [mallory_sg] }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "update: {text}");
    let (_, text) = get(&state, &format!("/api/shacl/pipelines/{pid}"), &mallory).await;
    assert!(
        !text.contains(PRIVATE_GRAPH),
        "the update was refused: {text}"
    );
}

/// A pipeline's report carries the data it validated, so running one, and
/// opening a stored run, needs read access to its scope at that time.
#[tokio::test]
async fn running_a_pipeline_or_opening_its_report_needs_read_access_to_its_scope() {
    let (state, _admin) = admin_state();
    let (alice, mallory) = tenants(&state);
    let alice_sg = create_shape_graph(&state, &alice, "public", SSN_SHAPES).await;

    // Alice shares a pipeline over her private graph.
    let (st, text) = create_pipeline(
        &state,
        &alice,
        json!({ "name": "alice-check", "visibility": "public",
                "targets": [{ "kind": "graph", "id": PRIVATE_GRAPH }],
                "shape_graph_ids": [alice_sg] }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{text}");
    let pid = id_of(&text);
    let run_uri = format!("/api/shacl/pipelines/{pid}/run");

    // Her run reports the value: the report is data from the graph.
    let (st, text) = send(&state, Method::POST, &run_uri, &alice, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(
        text.contains(SECRET_VALUE),
        "the report echoes values: {text}"
    );
    let run_id = id_of(&text);

    // Mallory sees the shared pipeline but may not run it, test-run it, or
    // open alice's stored run.
    let (st, _) = get(&state, &format!("/api/shacl/pipelines/{pid}"), &mallory).await;
    assert_eq!(st, StatusCode::OK, "the pipeline itself is shared");
    for uri in [run_uri.clone(), format!("{run_uri}?test=true")] {
        let (st, text) = send(&state, Method::POST, &uri, &mallory, Value::Null).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{uri}: {text}");
        assert!(!text.contains(SECRET_VALUE), "{text}");
    }
    let run_detail = format!("/api/shacl/pipelines/{pid}/runs/{run_id}");
    let (st, text) = get(&state, &run_detail, &mallory).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "stored report: {text}");
    assert!(!text.contains(SECRET_VALUE), "{text}");
    let (st, text) = get(&state, &run_detail, &alice).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(text.contains(SECRET_VALUE), "{text}");

    // A grant revoked after the pipeline was made gives nothing.
    load(
        &state.store,
        GRANTED_GRAPH,
        &format!("<http://ex.org/p2> a <http://ex.org/Patient> ; <http://ex.org/ssn> \"{SECRET_VALUE}\" ."),
    );
    state
        .auth_db
        .grant_graph_permission("rule-2", GRANTED_GRAPH, "user", "mallory", "read", "adm")
        .unwrap();
    let mallory_sg = create_shape_graph(&state, &mallory, "private", SSN_SHAPES).await;
    let (st, text) = create_pipeline(
        &state,
        &mallory,
        json!({ "name": "granted", "targets": [{ "kind": "graph", "id": GRANTED_GRAPH }],
                "shape_graph_ids": [mallory_sg] }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "with the grant: {text}");
    let granted_pid = id_of(&text);
    let granted_run = format!("/api/shacl/pipelines/{granted_pid}/run");
    let (st, text) = send(&state, Method::POST, &granted_run, &mallory, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "with the grant: {text}");
    state.auth_db.revoke_graph_permission("rule-2").unwrap();
    let (st, text) = send(&state, Method::POST, &granted_run, &mallory, Value::Null).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "after the revoke: {text}");
    assert!(!text.contains(SECRET_VALUE), "{text}");
}

// ─── A run's persisted report ────────────────────────────────────────────────

/// Every `sh:value` a caller can read over `/sparql`.
async fn sparql_values(state: &AppState, token: &str) -> String {
    let q = "SELECT ?v WHERE { GRAPH ?g { ?r <http://www.w3.org/ns/shacl#value> ?v } }";
    let (st, body) = get(state, &format!("/sparql?query={}", url_encode(q)), token).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    body
}

/// A report persisted as RDF (`results_target`) and attached to a dataset is
/// read by that dataset's readers, so it may be no more readable than the
/// data it reports on: not when the scope takes in a private graph of the
/// dataset, nor a graph outside it.
#[tokio::test]
async fn a_persisted_report_is_no_more_readable_than_what_it_reports_on() {
    const PUB_DATA: &str = "http://pub.example/data";
    const PRIV_DATA: &str = "http://pub.example/private-data";
    let (state, admin) = admin_state();
    let alice = make_user(&state, "alice");
    let bob = make_user(&state, "bob");
    make_dataset(&state, "pub-ds", "alice", Visibility::Public);
    load(
        &state.store,
        PUB_DATA,
        "<http://ex.org/p0> a <http://ex.org/Patient> ; <http://ex.org/ssn> \"public-ok\" .",
    );
    load(&state.store, PRIV_DATA, &secret_ttl());
    for g in [PUB_DATA, PRIV_DATA] {
        state.auth_db.add_dataset_graph("pub-ds", g).unwrap();
    }
    state
        .auth_db
        .set_dataset_graph_private("pub-ds", PRIV_DATA, true)
        .unwrap();

    // Alice may read her private graph; her pipeline over the dataset
    // persists its report in place.
    let sg = create_shape_graph(&state, &alice, "public", SSN_SHAPES).await;
    let (st, text) = create_pipeline(
        &state,
        &alice,
        json!({ "name": "whole-dataset", "targets": [{ "kind": "dataset", "id": "pub-ds" }],
                "shape_graph_ids": [sg], "results_target": "in_place" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{text}");
    let (st, text) = send(
        &state,
        Method::POST,
        &format!("/api/shacl/pipelines/{}/run", id_of(&text)),
        &alice,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(
        sparql_values(&state, &alice).await.contains(SECRET_VALUE),
        "the report was persisted, and alice reads it"
    );
    let seen = sparql_values(&state, &bob).await;
    assert!(
        !seen.contains(SECRET_VALUE),
        "bob may not read the private graph, nor a report on it: {seen}"
    );

    // An admin's pipeline over the dataset's public graph: its report is the
    // dataset's to share, and bob reads it.
    load(
        &state.store,
        ADMIN_GRAPH,
        "<http://ex.org/p9> a <http://ex.org/Patient> ; <http://ex.org/ssn> \"admin-only-9999\" .",
    );
    let sg = create_shape_graph(&state, &admin, "public", SSN_SHAPES).await;
    let body = |graphs: Value| {
        json!({ "name": "mixed", "graph_iris": graphs, "shape_graph_ids": [sg],
                "results_target": "in_place" })
    };
    let (st, text) = create_pipeline(&state, &admin, body(json!([PUB_DATA]))).await;
    assert_eq!(st, StatusCode::CREATED, "{text}");
    let pid = id_of(&text);
    let run = format!("/api/shacl/pipelines/{pid}/run");
    let (st, text) = send(&state, Method::POST, &run, &admin, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(
        sparql_values(&state, &bob).await.contains("public-ok"),
        "a report on the dataset's public graph is attached to it"
    );

    // Widened to a graph of no dataset, the same accumulating report graph
    // is no longer the dataset's to share: nothing of it reaches bob.
    let (st, text) = send(
        &state,
        Method::PUT,
        &format!("/api/shacl/pipelines/{pid}"),
        &admin,
        body(json!([PUB_DATA, ADMIN_GRAPH])),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let (st, text) = send(&state, Method::POST, &run, &admin, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(text.contains("admin-only-9999"), "the run saw it: {text}");
    let seen = sparql_values(&state, &bob).await;
    assert!(
        !seen.contains("admin-only-9999"),
        "bob may not read the admin's graph, nor a report on it: {seen}"
    );
}
