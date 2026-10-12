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
