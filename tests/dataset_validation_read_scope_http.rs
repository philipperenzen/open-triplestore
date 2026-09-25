//! Validating a dataset reads what the caller may read — through the real
//! router. "May read" is the set `/sparql` scopes a query to: graphs of
//! datasets the caller can access (a private graph only for its dataset's
//! writers) plus graph-ACL read grants; admins read every graph.
//!
//! A SHACL report carries its data graphs' focus nodes and values, so:
//!
//! * `POST /api/datasets/{id}/validate` validates only the dataset graphs the
//!   caller may read. A run that could not see every one of them is answered
//!   as a test run: nothing is recorded, and the dataset's official status is
//!   left as it was.
//! * An official run's report graph (`urn:system:reports:dataset:{id}`) is
//!   private in the dataset whenever a graph it validated is, and is never
//!   made public again.
//! * A stored run's full report (`…/validation/latest`, `…/validation/runs/
//!   {run_id}`) goes to the dataset's writers and to callers who may read
//!   every graph that run validated; everyone else gets its summary.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{GraphKind, OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use open_triplestore::store::TripleStore;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const SHAPES: &str = "http://pub.example/shapes";
const PUB_DATA: &str = "http://pub.example/data";
const PRIV_DATA: &str = "http://pub.example/private-data";
const REPORT_GRAPH: &str = "urn:system:reports:dataset:pub-ds";

const SECRET_VALUE: &str = "123-45-6789";
const PUBLIC_VALUE: &str = "public-ok";

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

fn load(store: &TripleStore, graph: &str, ttl: &str) {
    store
        .load_str(ttl, oxigraph::io::RdfFormat::Turtle, Some(graph))
        .unwrap();
}

fn patient(id: &str, ssn: &str) -> String {
    format!("<http://ex.org/{id}> a <http://ex.org/Patient> ; <http://ex.org/ssn> \"{ssn}\" .")
}

/// Alice's PUBLIC dataset `pub-ds`: a shapes-role graph, a public data graph
/// and a PRIVATE data graph holding the secret. Returns `(admin, alice, bob)`;
/// bob is a plain signed-in user, so a viewer of `pub-ds`.
fn fixture() -> (AppState, String, String, String) {
    let (state, admin) = admin_state();
    let alice = make_user(&state, "alice");
    let bob = make_user(&state, "bob");
    state
        .auth_db
        .create_dataset(
            "pub-ds",
            "pub-ds",
            None,
            OwnerType::User,
            "alice",
            Visibility::Public,
            None,
        )
        .unwrap();
    load(&state.store, SHAPES, SSN_SHAPES);
    load(&state.store, PUB_DATA, &patient("p0", PUBLIC_VALUE));
    load(&state.store, PRIV_DATA, &patient("p1", SECRET_VALUE));
    for g in [SHAPES, PUB_DATA, PRIV_DATA] {
        state.auth_db.add_dataset_graph("pub-ds", g).unwrap();
    }
    state
        .auth_db
        .set_dataset_graph_role("pub-ds", SHAPES, Some(GraphKind::Shapes))
        .unwrap();
    state
        .auth_db
        .set_dataset_graph_private("pub-ds", PRIV_DATA, true)
        .unwrap();
    (state, admin, alice, bob)
}

async fn validate(state: &AppState, token: &str, test: bool, body: Value) -> (StatusCode, Value) {
    let uri = if test {
        "/api/datasets/pub-ds/validate?test=true"
    } else {
        "/api/datasets/pub-ds/validate"
    };
    let (st, text) = send(state, Method::POST, uri, token, body).await;
    let json = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
    (st, json)
}

async fn sparql_values(state: &AppState, token: &str) -> String {
    let q = "SELECT ?v WHERE { GRAPH ?g { ?r <http://www.w3.org/ns/shacl#value> ?v } }";
    let (st, body) = get(state, &format!("/sparql?query={}", url_encode(q)), token).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    body
}

async fn history_ids(state: &AppState, token: &str) -> Vec<String> {
    let (st, text) = get(state, "/api/datasets/pub-ds/validation/history", token).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let runs: Value = serde_json::from_str(&text).unwrap();
    runs.as_array()
        .expect("an array of run summaries")
        .iter()
        .filter_map(|r| r["id"].as_str().map(String::from))
        .collect()
}

/// A viewer validates the graphs they may read, and nothing else: a private
/// graph's focus nodes and values never reach them, a run they make is not
/// official, and it leaves the dataset's recorded status alone.
#[tokio::test]
async fn a_viewer_validates_only_what_they_may_read_and_never_officially() {
    let (state, _admin, alice, bob) = fixture();

    // Control: bob cannot read the private graph over /sparql.
    let q = format!("SELECT ?v WHERE {{ GRAPH <{PRIV_DATA}> {{ ?s ?p ?v }} }}");
    let (st, body) = get(&state, &format!("/sparql?query={}", url_encode(&q)), &bob).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(!body.contains(SECRET_VALUE), "{body}");

    // Alice's official run sees everything and is recorded.
    let (st, j) = validate(&state, &alice, false, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{j}");
    let alice_run = j["run_id"]
        .as_str()
        .expect("an official run id")
        .to_string();
    assert!(j.to_string().contains(SECRET_VALUE), "alice reads it: {j}");
    assert_eq!(history_ids(&state, &alice).await, vec![alice_run.clone()]);

    // bob's test run: the public graph is validated, the private one is not.
    let (st, j) = validate(&state, &bob, true, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{j}");
    let text = j.to_string();
    assert!(
        text.contains(PUBLIC_VALUE),
        "the public graph is validated: {j}"
    );
    assert!(!text.contains(SECRET_VALUE), "no private value: {j}");
    assert!(
        !text.contains("http://ex.org/p1"),
        "no private focus node: {j}"
    );
    assert_eq!(j["test"], json!(true), "{j}");

    // bob's "official" run could not see every graph: answered as a test run.
    let (st, j) = validate(&state, &bob, false, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{j}");
    let text = j.to_string();
    assert!(!text.contains(SECRET_VALUE), "no private value: {j}");
    assert!(
        !text.contains("http://ex.org/p1"),
        "no private focus node: {j}"
    );
    assert_eq!(j["test"], json!(true), "not official: {j}");
    assert!(j["run_id"].is_null(), "nothing recorded: {j}");
    assert_eq!(
        history_ids(&state, &alice).await,
        vec![alice_run.clone()],
        "bob's partial run must not be recorded"
    );
    let (st, text) = get(&state, "/api/datasets/pub-ds/validation/latest", &alice).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let latest: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        latest["id"].as_str(),
        Some(alice_run.as_str()),
        "the official status is still alice's run: {latest}"
    );
    assert!(
        text.contains(SECRET_VALUE),
        "alice reads her own run in full"
    );
}

/// An explicit `shapes_graph` follows the same read rule as `/sparql`: a
/// graph of a dataset bob may read is fine (it used to need a graph-ACL
/// grant), a private one is refused, and neither makes bob's run official.
#[tokio::test]
async fn an_explicit_shapes_graph_must_be_readable_by_the_sparql_rule() {
    let (state, _admin, _alice, bob) = fixture();

    let (st, j) = validate(&state, &bob, false, json!({ "shapes_graph": SHAPES })).await;
    assert_eq!(st, StatusCode::OK, "a shapes graph bob may read: {j}");
    assert_eq!(j["test"], json!(true), "still not official: {j}");
    assert!(!j.to_string().contains(SECRET_VALUE), "{j}");

    let (st, j) = validate(&state, &bob, true, json!({ "shapes_graph": PRIV_DATA })).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "a private graph as shapes: {j}");
}

/// The report graph an official run writes is read by the dataset's readers,
/// so it is private whenever a graph the run validated is, and it stays so.
#[tokio::test]
async fn the_report_graph_is_no_more_readable_than_what_it_reports_on() {
    let (state, admin, alice, bob) = fixture();

    let (st, j) = validate(&state, &alice, false, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{j}");
    assert!(
        sparql_values(&state, &alice).await.contains(SECRET_VALUE),
        "the report was persisted, and alice reads it"
    );
    let seen = sparql_values(&state, &bob).await;
    assert!(
        !seen.contains(SECRET_VALUE),
        "bob may not read the private graph, nor a report on it: {seen}"
    );

    // An admin's official run covers the private graph too.
    let (st, j) = validate(&state, &admin, false, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{j}");
    assert!(j["run_id"].is_string(), "{j}");
    let seen = sparql_values(&state, &bob).await;
    assert!(!seen.contains(SECRET_VALUE), "{seen}");

    // The graph turns public and the run covers nothing private any more:
    // the report graph, which held private data before, stays private.
    state
        .auth_db
        .set_dataset_graph_private("pub-ds", PRIV_DATA, false)
        .unwrap();
    let (st, j) = validate(&state, &alice, false, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{j}");
    let entry = state
        .auth_db
        .list_dataset_graph_entries("pub-ds")
        .unwrap()
        .into_iter()
        .find(|e| e.graph_iri == REPORT_GRAPH)
        .expect("the report graph is the dataset's");
    assert!(entry.private, "never made public again");
}

/// A dataset whose graphs are all public: its report graph is the dataset's
/// to share, and a viewer reads its stored runs in full.
#[tokio::test]
async fn a_report_on_public_graphs_is_the_datasets_to_share() {
    let (state, _admin, alice, bob) = fixture();
    state
        .auth_db
        .remove_dataset_graph("pub-ds", PRIV_DATA)
        .unwrap();

    // bob sees every graph: his official run is recorded.
    let (st, j) = validate(&state, &bob, false, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{j}");
    let run = j["run_id"].as_str().expect("official").to_string();
    assert!(j["test"].is_null() || j["test"] == json!(false), "{j}");

    let seen = sparql_values(&state, &bob).await;
    assert!(seen.contains(PUBLIC_VALUE), "{seen}");
    let (st, text) = get(
        &state,
        &format!("/api/datasets/pub-ds/validation/runs/{run}"),
        &bob,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(text.contains(PUBLIC_VALUE), "full report: {text}");
    let (st, text) = get(&state, "/api/datasets/pub-ds/validation/latest", &alice).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(text.contains(PUBLIC_VALUE), "full report: {text}");
}

/// A stored run's full report reaches a viewer only if they may read every
/// graph it validated (checked when they ask); its summary stays public.
#[tokio::test]
async fn a_stored_run_is_served_in_full_only_to_who_may_read_all_it_covered() {
    let (state, _admin, alice, bob) = fixture();

    let (st, j) = validate(&state, &alice, false, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{j}");
    let run = j["run_id"].as_str().expect("official").to_string();

    for uri in [
        "/api/datasets/pub-ds/validation/latest".to_string(),
        format!("/api/datasets/pub-ds/validation/runs/{run}"),
    ] {
        let (st, text) = get(&state, &uri, &bob).await;
        assert_eq!(st, StatusCode::OK, "{uri}: {text}");
        assert!(!text.contains(SECRET_VALUE), "{uri}: {text}");
        assert!(!text.contains("http://ex.org/p1"), "{uri}: {text}");
        let j: Value = serde_json::from_str(&text).unwrap();
        assert!(j["report"].is_null(), "{uri}: report withheld: {j}");
        assert_eq!(j["report_withheld"], json!(true), "{uri}: {j}");
        assert_eq!(j["id"].as_str(), Some(run.as_str()), "{uri}: {j}");
        assert_eq!(j["conforms"], json!(false), "summary kept: {j}");
        assert_eq!(j["violation_count"], json!(2), "summary kept: {j}");

        let (st, text) = get(&state, &uri, &alice).await;
        assert_eq!(st, StatusCode::OK, "{uri}: {text}");
        assert!(text.contains(SECRET_VALUE), "the owner reads it: {text}");
    }
    assert_eq!(history_ids(&state, &bob).await, vec![run.clone()]);

    // A run over graphs bob could read, one of which is private since: the
    // check is made when he asks, not when the run was made.
    state
        .auth_db
        .set_dataset_graph_private("pub-ds", PRIV_DATA, false)
        .unwrap();
    let (st, j) = validate(&state, &alice, false, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{j}");
    let open_run = j["run_id"].as_str().expect("official").to_string();
    let uri = format!("/api/datasets/pub-ds/validation/runs/{open_run}");
    let (st, text) = get(&state, &uri, &bob).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(
        text.contains(SECRET_VALUE),
        "public at the time bob asks: {text}"
    );
    state
        .auth_db
        .set_dataset_graph_private("pub-ds", PRIV_DATA, true)
        .unwrap();
    let (st, text) = get(&state, &uri, &bob).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(!text.contains(SECRET_VALUE), "private again: {text}");

    // A run stored before runs recorded what they validated: writers only.
    let legacy = state
        .auth_db
        .insert_validation_run(
            "pub-ds",
            false,
            1,
            1,
            0,
            0,
            &json!({ "conforms": false, "results_count": 1, "results": [{
                "severity": "violation", "focus_node": "http://ex.org/p1",
                "path": "http://ex.org/ssn", "value": SECRET_VALUE,
                "source_shape": "_:s", "source_constraint": "sh:MaxLengthConstraintComponent",
                "message": "too long" }] })
            .to_string(),
            Some("alice"),
        )
        .unwrap();
    let uri = format!("/api/datasets/pub-ds/validation/runs/{}", legacy.id);
    let (st, text) = get(&state, &uri, &bob).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(!text.contains(SECRET_VALUE), "legacy run withheld: {text}");
    let (st, text) = get(&state, &uri, &alice).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(text.contains(SECRET_VALUE), "{text}");
}
