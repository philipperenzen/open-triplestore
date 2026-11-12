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
//! * The dataset's report history (`…/validation-reports`: one record per
//!   run of its write gate and per validate-and-commit) follows the same rule,
//!   a record at a time: a record names the graph it validated and its shapes
//!   graph, so one the caller may not read is left out of the listing and
//!   answers 404.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{
    GraphKind, OwnerType, ResourceRole, SystemRole, ValidationReportRecord, Visibility,
};
use open_triplestore::dataset_versions::cut::{cut, CutRequest};
use open_triplestore::dataset_versions::models::VersionStatus;
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

    // alice, the owner, records the official run (recording needs write
    // access; a viewer's own run would be refused).
    let (st, j) = validate(&state, &alice, false, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{j}");
    let run = j["run_id"].as_str().expect("official").to_string();
    assert!(j["test"].is_null() || j["test"] == json!(false), "{j}");

    // bob, a viewer who sees every graph, reads the report graph and the
    // stored run in full.
    let seen = sparql_values(&state, &bob).await;
    assert!(seen.contains(PUBLIC_VALUE), "{seen}");
    for uri in [
        format!("/api/datasets/pub-ds/validation/runs/{run}"),
        "/api/datasets/pub-ds/validation/latest".to_string(),
    ] {
        let (st, text) = get(&state, &uri, &bob).await;
        assert_eq!(st, StatusCode::OK, "{uri}: {text}");
        assert!(text.contains(PUBLIC_VALUE), "{uri}: full report: {text}");
    }
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

/// Alice's FULLY-public dataset `open-ds`: a shapes graph and a public data
/// graph, no private graph, so a viewer (bob) may read every graph of it and a
/// run of theirs is complete — not partial. This isolates the write-authority
/// rule from the read-scope rule the rest of this file exercises. Returns
/// `(admin, alice, bob)`.
fn public_only_fixture() -> (AppState, String, String, String) {
    let (state, admin) = admin_state();
    let alice = make_user(&state, "alice");
    let bob = make_user(&state, "bob");
    state
        .auth_db
        .create_dataset(
            "open-ds",
            "open-ds",
            None,
            OwnerType::User,
            "alice",
            Visibility::Public,
            None,
        )
        .unwrap();
    load(&state.store, SHAPES, SSN_SHAPES);
    load(&state.store, PUB_DATA, &patient("p0", PUBLIC_VALUE));
    for g in [SHAPES, PUB_DATA] {
        state.auth_db.add_dataset_graph("open-ds", g).unwrap();
    }
    state
        .auth_db
        .set_dataset_graph_role("open-ds", SHAPES, Some(GraphKind::Shapes))
        .unwrap();
    (state, admin, alice, bob)
}

async fn open_history_ids(state: &AppState, token: &str) -> Vec<String> {
    let (st, text) = get(state, "/api/datasets/open-ds/validation/history", token).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    serde_json::from_str::<Value>(&text)
        .unwrap()
        .as_array()
        .expect("an array of run summaries")
        .iter()
        .filter_map(|r| r["id"].as_str().map(String::from))
        .collect()
}

/// Recording an official run writes the dataset's stored status and history. A
/// reader who may see the WHOLE dataset (so their run is complete, not partial)
/// still may not record one — they could otherwise forge a verdict or evict the
/// owner's run. They may run a *test* validation, which records nothing. The
/// owner and an admin record officially.
#[tokio::test]
async fn a_reader_cannot_record_forge_or_evict_an_official_run() {
    let (state, admin, alice, bob) = public_only_fixture();
    let validate_uri = "/api/datasets/open-ds/validate";

    // The owner records the official run.
    let (st, text) = send(&state, Method::POST, validate_uri, &alice, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "owner records: {text}");
    let alice_run = serde_json::from_str::<Value>(&text).unwrap()["run_id"]
        .as_str()
        .expect("an official run id")
        .to_string();

    // bob may read every graph of open-ds, but recording is a write he lacks:
    // 403, so he can neither forge a verdict nor evict alice's run.
    let (st, body) = send(&state, Method::POST, validate_uri, &bob, Value::Null).await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "a reader must not record: {body}"
    );

    // A test run is a read — bob may run it; it records nothing.
    let test_uri = format!("{validate_uri}?test=true");
    let (st, text) = send(&state, Method::POST, &test_uri, &bob, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let j: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(j["test"], json!(true), "{j}");
    assert!(j["run_id"].is_null(), "a test run records nothing: {j}");

    // An admin records too (a writer everywhere, though not the dataset owner).
    let (st, text) = send(&state, Method::POST, validate_uri, &admin, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "admin records: {text}");

    // Alice's official run survived every one of bob's attempts.
    let ids = open_history_ids(&state, &alice).await;
    assert!(
        ids.contains(&alice_run),
        "the owner's official run must still be in history: {ids:?}"
    );
}

// ─── The report history ─────────────────────────────────────────────────────
//
// `GET …/validation-reports` lists what the dataset's write gate and
// validate-and-commit recorded. A record names the graph it validated
// (`data_ref`) and its shapes graph (`shapes_ref`), and a commit's record
// carries the validator's report on the committed data. So a record is served
// as a stored run's report is: to admins, to the dataset's writers, and to
// callers who may read every graph it validated; and to nobody but an admin
// who may not read a private shapes graph it used. A record the caller is not
// served is left out of the listing and answers 404.

/// A second shapes graph of `pub-ds`, private there.
const PRIV_SHAPES: &str = "http://pub.example/private-shapes";
const REPORTS: &str = "/api/datasets/pub-ds/validation-reports";

/// GET `uri` as `token`, or anonymously.
async fn get_as(state: &AppState, uri: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::builder().method(Method::GET).uri(uri);
    if let Some(token) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let resp = test_app(state.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

/// PUT `ttl` into `graph` over the Graph Store, as `token`.
async fn put_graph(state: &AppState, token: &str, graph: &str, ttl: &str) -> StatusCode {
    test_app(state.clone())
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
        .unwrap()
        .status()
}

/// Turn `pub-ds`'s own write gate on, with `shapes` as its shapes graph.
fn gate_with(state: &AppState, shapes: &str) {
    state
        .auth_db
        .update_dataset_shacl("pub-ds", true, Some(shapes))
        .unwrap();
}

/// What `…/validation-reports` lists for `token` (`None`: anonymous): the
/// body as sent, and its records.
async fn report_history(state: &AppState, token: Option<&str>) -> (String, Vec<Value>) {
    let (st, text) = get_as(state, REPORTS, token).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let records = serde_json::from_str::<Value>(&text)
        .unwrap()
        .as_array()
        .expect("an array of reports")
        .clone();
    (text, records)
}

fn listed_ids(records: &[Value]) -> Vec<String> {
    let mut ids: Vec<String> = records
        .iter()
        .filter_map(|r| r["id"].as_str().map(String::from))
        .collect();
    ids.sort();
    ids
}

fn sorted<const N: usize>(ids: [&String; N]) -> Vec<String> {
    let mut ids: Vec<String> = ids.into_iter().cloned().collect();
    ids.sort();
    ids
}

/// One stored report as `token` gets it.
async fn report(state: &AppState, token: Option<&str>, id: &str) -> (StatusCode, String) {
    get_as(state, &format!("{REPORTS}/{id}"), token).await
}

/// A withheld report answers as one that does not exist: 404, the same body,
/// and no private graph's name.
async fn assert_reads_as_missing(state: &AppState, who: &str, token: Option<&str>, id: &str) {
    let (st, missing) = report(state, token, "no-such-report").await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{who}: {missing}");
    let (st, text) = report(state, token, id).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{who}: {text}");
    for private in [PRIV_DATA, PRIV_SHAPES, SECRET_VALUE] {
        assert!(!text.contains(private), "{who}: {private} in {text}");
    }
    assert_eq!(
        text.replace(id, "<id>"),
        missing.replace("no-such-report", "<id>"),
        "{who}: a withheld report answers like a missing one"
    );
}

/// The gate's record of a write to `data_ref` checked against `shapes_ref`.
fn gate_report(records: &[Value], data_ref: &str, shapes_ref: &str, conforms: bool) -> String {
    records
        .iter()
        .find(|r| {
            r["source"] == json!("on-write")
                && r["data_ref"] == json!(data_ref)
                && r["shapes_ref"] == json!(shapes_ref)
                && r["conforms"] == json!(conforms)
        })
        .and_then(|r| r["id"].as_str())
        .unwrap_or_else(|| panic!("no on-write report on <{data_ref}> in {records:?}"))
        .to_string()
}

/// `fixture()` plus a private shapes graph, the dataset's write gate, and four
/// runs of that gate in the report history.
struct GateHistory {
    state: AppState,
    admin: String,
    alice: String,
    bob: String,
    /// A write to the public graph, checked against the public shapes.
    control: String,
    /// A write to the private graph, checked against the public shapes.
    on_private_data: String,
    /// A refused write to the private graph.
    refused: String,
    /// A write to the public graph, checked against the private shapes.
    by_private_shapes: String,
}

async fn gate_history() -> GateHistory {
    let (state, admin, alice, bob) = fixture();
    load(&state.store, PRIV_SHAPES, SSN_SHAPES);
    state
        .auth_db
        .add_dataset_graph("pub-ds", PRIV_SHAPES)
        .unwrap();
    state
        .auth_db
        .set_dataset_graph_role("pub-ds", PRIV_SHAPES, Some(GraphKind::Shapes))
        .unwrap();
    state
        .auth_db
        .set_dataset_graph_private("pub-ds", PRIV_SHAPES, true)
        .unwrap();

    gate_with(&state, SHAPES);
    let ok = patient("p0", "ok");
    assert!(put_graph(&state, &admin, PUB_DATA, &ok).await.is_success());
    assert!(put_graph(&state, &admin, PRIV_DATA, &patient("p1", "ok"))
        .await
        .is_success());
    assert_eq!(
        put_graph(&state, &admin, PRIV_DATA, &patient("p1", SECRET_VALUE)).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the gate refuses a value longer than 3"
    );
    gate_with(&state, PRIV_SHAPES);
    assert!(put_graph(&state, &admin, PUB_DATA, &ok).await.is_success());

    let (text, records) = report_history(&state, Some(&admin)).await;
    assert_eq!(records.len(), 4, "one report per run of the gate: {text}");
    GateHistory {
        control: gate_report(&records, PUB_DATA, SHAPES, true),
        on_private_data: gate_report(&records, PRIV_DATA, SHAPES, true),
        refused: gate_report(&records, PRIV_DATA, SHAPES, false),
        by_private_shapes: gate_report(&records, PUB_DATA, PRIV_SHAPES, true),
        state,
        admin,
        alice,
        bob,
    }
}

/// The gate's reports name the graph written and the shapes graph. A caller
/// who may not write the dataset (anonymous on a public dataset, a signed-in
/// viewer) gets neither a report on a private graph nor one shaped by a
/// private shapes graph: not in the listing, and 404 by id. The report on
/// public graphs stays theirs. Writers and admins get them all.
#[tokio::test]
async fn the_report_history_names_private_graphs_only_to_who_may_read_them() {
    let h = gate_history().await;
    let state = &h.state;
    let erin = make_user(state, "erin");
    state
        .auth_db
        .set_resource_grant(
            "dataset",
            "pub-ds",
            "user",
            "erin",
            ResourceRole::Editor,
            "adm",
        )
        .unwrap();
    state.auth_db.invalidate_accessible_graphs_cache();

    for (who, token) in [("anonymous", None), ("a viewer", Some(h.bob.as_str()))] {
        let (text, records) = report_history(state, token).await;
        assert_eq!(
            listed_ids(&records),
            vec![h.control.clone()],
            "{who} is listed the report on public graphs only: {text}"
        );
        for private in [PRIV_DATA, PRIV_SHAPES] {
            assert!(
                !text.contains(private),
                "{who}: the listing names <{private}>: {text}"
            );
        }
        let (st, text) = report(state, token, &h.control).await;
        assert_eq!(st, StatusCode::OK, "{who}: {text}");
        let j: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(j["data_ref"], json!(PUB_DATA), "{who}: {j}");
        assert_eq!(j["shapes_ref"], json!(SHAPES), "{who}: {j}");
        assert!(
            j["report_ttl"]
                .as_str()
                .is_some_and(|t| t.contains("sh:conforms")),
            "{who}: the body comes with it: {j}"
        );
        for id in [&h.on_private_data, &h.refused, &h.by_private_shapes] {
            assert_reads_as_missing(state, who, token, id).await;
        }
    }

    let all = sorted([
        &h.control,
        &h.on_private_data,
        &h.refused,
        &h.by_private_shapes,
    ]);
    for (who, token) in [
        ("the owner", &h.alice),
        ("an editor", &erin),
        ("an admin", &h.admin),
    ] {
        let (text, records) = report_history(state, Some(token)).await;
        assert_eq!(listed_ids(&records), all, "{who} is listed all: {text}");
        for (id, data_ref, shapes_ref) in [
            (&h.control, PUB_DATA, SHAPES),
            (&h.on_private_data, PRIV_DATA, SHAPES),
            (&h.refused, PRIV_DATA, SHAPES),
            (&h.by_private_shapes, PUB_DATA, PRIV_SHAPES),
        ] {
            let (st, text) = report(state, Some(token), id).await;
            assert_eq!(st, StatusCode::OK, "{who}: {text}");
            let j: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(j["data_ref"], json!(data_ref), "{who}: {j}");
            assert_eq!(j["shapes_ref"], json!(shapes_ref), "{who}: {j}");
        }
    }
}

/// Who may read a graph is asked when the report is: a report on a graph made
/// private since is withheld, one on a graph made public again is back, and a
/// graph-ACL read grant on a private graph gives its reports as it gives the
/// graph over `/sparql`.
#[tokio::test]
async fn a_report_follows_its_graphs_as_they_are_when_it_is_read() {
    let h = gate_history().await;
    let state = &h.state;
    let bob = Some(h.bob.as_str());
    let private = |graph: &str, private: bool| {
        state
            .auth_db
            .set_dataset_graph_private("pub-ds", graph, private)
            .unwrap()
    };

    private(PUB_DATA, true);
    for who in [None, bob] {
        let (text, records) = report_history(state, who).await;
        assert!(
            records.is_empty(),
            "every report is on a private graph: {text}"
        );
        assert!(!text.contains(PUB_DATA), "{text}");
        assert_reads_as_missing(state, "a non-writer", who, &h.control).await;
    }
    let (_, records) = report_history(state, Some(&h.alice)).await;
    assert_eq!(records.len(), 4, "the owner keeps them: {records:?}");

    private(PUB_DATA, false);
    private(PRIV_SHAPES, false);
    let (text, records) = report_history(state, None).await;
    assert_eq!(
        listed_ids(&records),
        sorted([&h.control, &h.by_private_shapes]),
        "public again, shapes graph included: {text}"
    );
    private(PRIV_SHAPES, true);

    // bob is granted read on the private data graph: he reads it over
    // /sparql, and so the reports on it. Anonymous callers still do not.
    state
        .auth_db
        .grant_graph_permission("r-bob", PRIV_DATA, "user", "bob", "read", "adm")
        .unwrap();
    let (text, records) = report_history(state, bob).await;
    assert_eq!(
        listed_ids(&records),
        sorted([&h.control, &h.on_private_data, &h.refused]),
        "bob reads the private data graph by grant, not the private shapes: {text}"
    );
    let (st, text) = report(state, bob, &h.on_private_data).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let (_, records) = report_history(state, None).await;
    assert_eq!(listed_ids(&records), vec![h.control.clone()]);
}

/// Record a validate-and-commit report on `version` as the server does
/// (`data_ref: "inline"`), with `body` as the validator's report.
fn commit_report(state: &AppState, version: &str, body: &str) -> String {
    let id = format!("commit-{version}");
    state
        .auth_db
        .insert_validation_report(&ValidationReportRecord {
            id: id.clone(),
            dataset_id: "pub-ds".to_string(),
            version: Some(version.to_string()),
            conforms: true,
            report_ttl: body.to_string(),
            data_ref: Some("inline".to_string()),
            shapes_ref: None,
            source: "platform".to_string(),
            created_by: Some("alice".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        })
        .unwrap();
    id
}

/// Cut `version` of `pub-ds` from `graph` alone, as a commit into it does.
fn cut_version(state: &AppState, version: &str, graph: &str) {
    let req = CutRequest {
        dataset_id: "pub-ds".to_string(),
        version: version.to_string(),
        source_graphs: vec![graph.to_string()],
        status: VersionStatus::Draft,
        created_by: None,
        notes: None,
        branch: None,
        derived_from: None,
        force_checkpoint: true,
        shared: Default::default(),
        conforms_to_model: None,
        conforms_to_version: None,
    };
    cut(&state.store, &state.base_url, &req).unwrap();
}

/// A validate-and-commit report records its data as `inline` and carries the
/// validator's report on it: the data is the graph its version was cut from.
/// A non-writer gets the report of a commit into a graph they may read; one
/// into a private graph, and one whose version no longer says which graph it
/// was, go to writers and admins only.
#[tokio::test]
async fn a_commit_report_is_about_the_graph_its_version_was_cut_from() {
    let (state, admin, alice, bob) = fixture();
    cut_version(&state, "1.0.0", PUB_DATA);
    cut_version(&state, "1.1.0", PRIV_DATA);
    let warning = |value: &str| {
        format!(
            "@prefix sh: <http://www.w3.org/ns/shacl#> .\n[] a sh:ValidationReport ; \
             sh:conforms true ; sh:result [ sh:resultSeverity sh:Warning ; sh:value \"{value}\" ] .\n"
        )
    };
    let public = commit_report(&state, "1.0.0", &warning(PUBLIC_VALUE));
    let private = commit_report(&state, "1.1.0", &warning(SECRET_VALUE));
    let gone = commit_report(&state, "0.9.0", &warning(SECRET_VALUE));

    for (who, token) in [("anonymous", None), ("a viewer", Some(bob.as_str()))] {
        let (text, records) = report_history(&state, token).await;
        assert_eq!(listed_ids(&records), vec![public.clone()], "{who}: {text}");
        let (st, text) = report(&state, token, &public).await;
        assert_eq!(st, StatusCode::OK, "{who}: {text}");
        assert!(text.contains(PUBLIC_VALUE), "{who}: {text}");
        for id in [&private, &gone] {
            assert_reads_as_missing(&state, who, token, id).await;
        }
    }
    for (who, token) in [("the owner", &alice), ("an admin", &admin)] {
        let (text, records) = report_history(&state, Some(token)).await;
        assert_eq!(
            listed_ids(&records),
            sorted([&public, &private, &gone]),
            "{who}: {text}"
        );
        let (st, text) = report(&state, Some(token), &private).await;
        assert_eq!(st, StatusCode::OK, "{who}: {text}");
        assert!(
            text.contains(SECRET_VALUE),
            "{who} reads it in full: {text}"
        );
    }
}

/// A shapes graph another dataset holds as private is read by that dataset's
/// writers. Linked as this dataset's gate, it shapes reports this dataset's
/// own writers may not read either, as with a stored run: only an admin, and
/// a writer who may read the shapes graph, get them.
#[tokio::test]
async fn a_report_shaped_by_another_datasets_private_graph_is_not_this_datasets_to_share() {
    let (state, admin, alice, bob) = fixture();
    let carol = make_user(&state, "carol");
    let foreign = "http://other.example/shapes";
    state
        .auth_db
        .create_dataset(
            "other-ds",
            "other-ds",
            None,
            OwnerType::User,
            "carol",
            Visibility::Public,
            None,
        )
        .unwrap();
    load(&state.store, foreign, SSN_SHAPES);
    state
        .auth_db
        .add_dataset_graph("other-ds", foreign)
        .unwrap();
    state
        .auth_db
        .set_dataset_graph_private("other-ds", foreign, true)
        .unwrap();
    // carol also writes pub-ds: the one writer of it who may read the shapes.
    state
        .auth_db
        .set_resource_grant(
            "dataset",
            "pub-ds",
            "user",
            "carol",
            ResourceRole::Editor,
            "adm",
        )
        .unwrap();
    state.auth_db.invalidate_accessible_graphs_cache();

    gate_with(&state, SHAPES);
    let ok = patient("p0", "ok");
    assert!(put_graph(&state, &admin, PUB_DATA, &ok).await.is_success());
    gate_with(&state, foreign);
    assert!(put_graph(&state, &admin, PUB_DATA, &ok).await.is_success());
    let (_, records) = report_history(&state, Some(&admin)).await;
    let control = gate_report(&records, PUB_DATA, SHAPES, true);
    let shaped = gate_report(&records, PUB_DATA, foreign, true);

    for (who, token) in [
        ("anonymous", None),
        ("a viewer", Some(bob.as_str())),
        (
            "the owner, who may not read the shapes",
            Some(alice.as_str()),
        ),
    ] {
        let (text, records) = report_history(&state, token).await;
        assert_eq!(listed_ids(&records), vec![control.clone()], "{who}: {text}");
        assert!(!text.contains(foreign), "{who}: {text}");
        let (st, text) = report(&state, token, &shaped).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{who}: {text}");
        assert!(!text.contains(foreign), "{who}: {text}");
    }
    for (who, token) in [("a writer of both", &carol), ("an admin", &admin)] {
        let (text, records) = report_history(&state, Some(token)).await;
        assert_eq!(
            listed_ids(&records),
            sorted([&control, &shaped]),
            "{who}: {text}"
        );
        let (st, text) = report(&state, Some(token), &shaped).await;
        assert_eq!(st, StatusCode::OK, "{who}: {text}");
        assert!(text.contains(foreign), "{who}: {text}");
    }
}

/// When the private flags cannot be read, nobody whose answer depends on them
/// gets a report: the routes fail (500) rather than serve the history as if
/// nothing were private. An admin, who reads every graph, is still answered.
#[tokio::test]
async fn the_report_history_fails_closed_when_private_flags_cannot_be_read() {
    let h = gate_history().await;
    let state = &h.state;
    state
        .auth_db
        .pool()
        .get()
        .unwrap()
        .execute_batch("DROP TABLE dataset_graphs")
        .unwrap();
    state.auth_db.invalidate_accessible_graphs_cache();

    for (who, token) in [
        ("anonymous", None),
        ("a viewer", Some(h.bob.as_str())),
        ("the owner", Some(h.alice.as_str())),
    ] {
        for uri in [
            REPORTS.to_string(),
            format!("{REPORTS}/{}", h.control),
            format!("{REPORTS}/{}", h.on_private_data),
        ] {
            let (st, text) = get_as(state, &uri, token).await;
            assert_eq!(
                st,
                StatusCode::INTERNAL_SERVER_ERROR,
                "{who}, {uri}: {text}"
            );
            for private in [PRIV_DATA, PRIV_SHAPES] {
                assert!(!text.contains(private), "{who}, {uri}: {text}");
            }
        }
    }
    let (text, records) = report_history(state, Some(&h.admin)).await;
    assert_eq!(records.len(), 4, "an admin reads every graph: {text}");
}

// ─── ShEx ───────────────────────────────────────────────────────────────────
//
// ShEx reports name their focus nodes, and conformance itself answers
// questions about the data (`PATTERN "^123"` conforms only when the hidden
// value starts with 123). Both ShEx routes therefore read what `/sparql` lets
// the caller read, and the dataset route reads only that dataset's graphs.

/// Another tenant's PRIVATE dataset holding one more patient.
#[cfg(feature = "shex")]
const OTHER_DATA: &str = "http://other.example/data";

#[cfg(feature = "shex")]
fn shex_fixture() -> (AppState, String, String, String) {
    let (state, admin, alice, bob) = fixture();
    state
        .auth_db
        .create_user("carol", "carol", "carol@t.com", "hash", SystemRole::User)
        .unwrap();
    state
        .auth_db
        .create_dataset(
            "other-ds",
            "other-ds",
            None,
            OwnerType::User,
            "carol",
            Visibility::Private,
            None,
        )
        .unwrap();
    load(&state.store, OTHER_DATA, &patient("p9", "987-65-4321"));
    state
        .auth_db
        .add_dataset_graph("other-ds", OTHER_DATA)
        .unwrap();
    (state, admin, alice, bob)
}

/// POST a ShEx validation; `focus` empty means "no shape map" (the validator
/// then finds its own focus nodes).
#[cfg(feature = "shex")]
async fn shex(state: &AppState, token: &str, uri: &str, pattern: &str, focus: &[&str]) -> Value {
    let shape = "http://ex.org/SsnShape";
    let schema = format!(
        "PREFIX ex: <http://ex.org/>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\
         ex:SsnShape {{ ex:ssn xsd:string PATTERN \"{pattern}\" }}"
    );
    let shape_map = if focus.is_empty() {
        json!({})
    } else {
        let nodes: Vec<String> = focus.iter().map(|f| format!("http://ex.org/{f}")).collect();
        json!({ shape: nodes })
    };
    let (st, text) = send(
        state,
        Method::POST,
        uri,
        token,
        json!({ "schema": schema, "shape_map": shape_map }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{uri}: {text}");
    serde_json::from_str(&text).unwrap()
}

#[cfg(feature = "shex")]
fn status_of(report: &Value, focus: &str) -> Option<bool> {
    report["results"].as_array()?.iter().find_map(|r| {
        let node = r["focus_node"].as_str()?;
        node.contains(&format!("ex.org/{focus}"))
            .then(|| r["status"].as_str() == Some("Conformant"))
    })
}

/// Without a shape map the validator discovers focus nodes itself. A viewer
/// of the dataset discovers only those in graphs they may read; nobody, an
/// admin included, discovers another dataset's nodes through this dataset.
#[cfg(feature = "shex")]
#[tokio::test]
async fn shex_on_a_dataset_discovers_only_its_readable_nodes() {
    let (state, admin, _alice, bob) = shex_fixture();
    let uri = "/api/datasets/pub-ds/shex/validate";

    let report = shex(&state, &bob, uri, ".*", &[]).await;
    let text = report.to_string();
    assert!(
        text.contains("ex.org/p0"),
        "the public node is validated: {text}"
    );
    assert!(!text.contains("ex.org/p1"), "private graph leaked: {text}");
    assert!(
        !text.contains("ex.org/p9"),
        "another dataset leaked: {text}"
    );

    let report = shex(&state, &admin, uri, ".*", &[]).await;
    let text = report.to_string();
    assert!(
        text.contains("ex.org/p1"),
        "admin reads the private graph: {text}"
    );
    assert!(
        !text.contains("ex.org/p9"),
        "the dataset route reads only its own dataset: {text}"
    );
}

/// With a shape map the verdict is the leak: `PATTERN "^123"` conforms only
/// when the secret starts with 123. A viewer gets the verdict on data they
/// may read and never one computed from a graph they may not.
#[cfg(feature = "shex")]
#[tokio::test]
async fn shex_verdicts_never_come_from_unreadable_graphs() {
    let (state, admin, _alice, bob) = shex_fixture();
    for uri in ["/api/datasets/pub-ds/shex/validate", "/api/shex/validate"] {
        // The oracle exists: an admin's verdict reads the secret.
        let report = shex(&state, &admin, uri, "^123", &["p1"]).await;
        assert_eq!(status_of(&report, "p1"), Some(true), "{uri}: {report}");

        let report = shex(&state, &bob, uri, "^public", &["p0"]).await;
        assert_eq!(
            status_of(&report, "p0"),
            Some(true),
            "{uri}: a viewer validates what they may read: {report}"
        );
        let report = shex(&state, &bob, uri, "^123", &["p1"]).await;
        assert_eq!(
            status_of(&report, "p1"),
            Some(false),
            "{uri}: a private graph answered the viewer's question: {report}"
        );
        let report = shex(&state, &bob, uri, "^987", &["p9"]).await;
        assert_eq!(
            status_of(&report, "p9"),
            Some(false),
            "{uri}: another tenant's dataset answered the viewer's question: {report}"
        );
    }
}

/// The inline route has no dataset: it reads what `/sparql` would let the
/// caller read, across datasets.
#[cfg(feature = "shex")]
#[tokio::test]
async fn shex_inline_discovers_only_readable_nodes() {
    let (state, admin, alice, bob) = shex_fixture();
    let uri = "/api/shex/validate";

    let text = shex(&state, &bob, uri, ".*", &[]).await.to_string();
    assert!(text.contains("ex.org/p0"), "{text}");
    assert!(!text.contains("ex.org/p1"), "private graph leaked: {text}");
    assert!(!text.contains("ex.org/p9"), "another tenant leaked: {text}");

    // alice writes pub-ds, so she reads its private graph; carol's she does not.
    let text = shex(&state, &alice, uri, ".*", &[]).await.to_string();
    assert!(text.contains("ex.org/p1"), "{text}");
    assert!(!text.contains("ex.org/p9"), "another tenant leaked: {text}");

    let text = shex(&state, &admin, uri, ".*", &[]).await.to_string();
    for p in ["p0", "p1", "p9"] {
        assert!(
            text.contains(&format!("ex.org/{p}")),
            "admin reads all: {text}"
        );
    }
}

/// `IMPORT <g>` reads a ShExR schema from the store, through the caller's
/// read scope: a schema in another tenant's private graph imports for an
/// admin and is "no readable graph" for everyone else — the same answer as
/// for a graph that does not exist, so an import probes nothing.
#[cfg(feature = "shex")]
#[tokio::test]
async fn shex_imports_read_only_graphs_the_caller_may_read() {
    let (state, admin, _alice, bob) = shex_fixture();
    // carol's private dataset also holds a schema.
    let lib = "http://other.example/schema";
    load(
        &state.store,
        lib,
        "PREFIX sx: <http://www.w3.org/ns/shex#>\n\
         [] a sx:Schema ; sx:shapes ( <http://ex.org/Lib> ) .\n\
         <http://ex.org/Lib> a sx:ShapeDecl ; sx:shapeExpr [ a sx:Shape ;\n\
           sx:expression [ a sx:TripleConstraint ; sx:predicate <http://ex.org/ssn> ] ] .",
    );
    state.auth_db.add_dataset_graph("other-ds", lib).unwrap();
    let body = |imported: &str| {
        json!({
            "schema": format!(
                "PREFIX ex: <http://ex.org/>\nIMPORT <{imported}>\nex:S @ex:Lib"
            ),
            "shape_map": { "http://ex.org/S": ["http://ex.org/p0"] }
        })
    };
    for uri in ["/api/datasets/pub-ds/shex/validate", "/api/shex/validate"] {
        let (st, text) = send(&state, Method::POST, uri, &admin, body(lib)).await;
        assert_eq!(
            st,
            StatusCode::OK,
            "{uri}: the admin reads the schema: {text}"
        );
        assert!(text.contains("\"Conformant\""), "{uri}: {text}");

        let (st, hidden) = send(&state, Method::POST, uri, &bob, body(lib)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{uri}: {hidden}");
        let (st, missing) = send(
            &state,
            Method::POST,
            uri,
            &bob,
            body("http://other.example/none"),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{uri}: {missing}");
        assert_eq!(
            hidden.replace(lib, "<g>"),
            missing.replace("http://other.example/none", "<g>"),
            "{uri}: a private graph answers like a missing one"
        );
    }
}
