//! Statement-level provenance and the Skolem opt-in
//! (docs/notes/delta-versioning-design.md §5.2, §6).
//!
//! A dataset that opts in gets every change to one of its statements
//! annotated in RDF 1.2 in its statement-provenance graph, by a change-log
//! consumer: a named reifier per statement, an event per change carrying the
//! operation, the change-log sequence number and the commit. These tests pin
//! the RDF 1.2 behaviour that design rests on (§8.2 item 8), the annotations,
//! the graph's place outside the data (§6.3) and Skolem imports.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::server::AppState;
use open_triplestore::store::changes::{DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const G: &str = "urn:sp:data";

fn recording() -> TripleStore {
    TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD)
}

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    ct: Option<&str>,
    body: &str,
) -> (StatusCode, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(c) = ct {
        b = b.header(header::CONTENT_TYPE, c);
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    (st, body_text(resp.into_body()).await)
}

async fn json_req(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let (st, text) = req(
        app,
        method,
        uri,
        token,
        Some("application/json"),
        &body.to_string(),
    )
    .await;
    (st, serde_json::from_str(&text).unwrap_or(Value::Null))
}

async fn dataset(state: &AppState, app: &Router, token: &str) -> String {
    let (st, v) = json_req(
        app,
        Method::POST,
        "/api/datasets",
        Some(token),
        json!({ "name": "sp", "owner_type": "user", "owner_id": "adm", "visibility": "public" }),
    )
    .await;
    assert!(st.is_success(), "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let (st, v) = json_req(
        app,
        Method::POST,
        &format!("/api/datasets/{id}/graphs"),
        Some(token),
        json!({ "graph_iri": G }),
    )
    .await;
    assert!(st.is_success(), "{v}");
    let _ = state;
    id
}

fn ask(state: &AppState, q: &str) -> bool {
    matches!(state.store.query(q), Ok(QueryResults::Boolean(true)))
}

fn count(state: &AppState, graph: &str) -> usize {
    state.store.count_graph(Some(graph)).unwrap_or(0)
}

/// §8.2 item 8: a *named* reifier is idempotent and deletable — the
/// anonymous `<< >>` shorthand mints a fresh blank node every time.
#[tokio::test]
async fn a_named_reifier_is_idempotent_and_deletable() {
    let (state, _token) = admin_state();
    let insert = "INSERT DATA { GRAPH <urn:sp:r> { <urn:sp:r1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies> <<( <urn:sp:s> <urn:sp:p> \"v\" )>> } }";
    state.store.update(insert).unwrap();
    state.store.update(insert).unwrap();
    assert_eq!(count(&state, "urn:sp:r"), 1, "run twice, one quad");
    state
        .store
        .update(&insert.replace("INSERT DATA", "DELETE DATA"))
        .unwrap();
    assert_eq!(count(&state, "urn:sp:r"), 0);
    // Turtle 1.2 annotation syntax: the triple asserted, its reifier named,
    // the annotation on the reifier.
    state
        .store
        .load_str(
            "<urn:sp:s> <urn:sp:p> \"v\" ~ <urn:sp:r2> {| <urn:sp:by> <urn:sp:alice> |} .",
            RdfFormat::Turtle,
            Some("urn:sp:t"),
        )
        .unwrap();
    assert!(ask(&state, "ASK { GRAPH <urn:sp:t> { <urn:sp:s> <urn:sp:p> \"v\" . <urn:sp:r2> <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies> <<( <urn:sp:s> <urn:sp:p> \"v\" )>> ; <urn:sp:by> <urn:sp:alice> } }"));
}

#[tokio::test]
async fn settings_are_readable_by_readers_and_set_by_writers() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let id = dataset(&state, &app, &token).await;
    let uri = format!("/api/datasets/{id}/settings");
    let (st, s) = json_req(&app, Method::GET, &uri, None, Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(s["skolem"], false);
    assert_eq!(s["provenance"], "off");
    state
        .auth_db
        .create_user("eve", "eve", "eve@t.com", "h", SystemRole::User)
        .unwrap();
    let eve = mint_token("eve", "eve", "user");
    let (st, _) = json_req(
        &app,
        Method::PUT,
        &uri,
        Some(&eve),
        json!({ "skolem": true }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, s) = json_req(
        &app,
        Method::PUT,
        &uri,
        Some(&token),
        json!({ "provenance": "full" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{s}");
    assert_eq!(s["provenance"], "full");
    assert_eq!(s["skolem"], false, "a field left out keeps its value");
    // Opting in registers the graph, with the Provenance role.
    let entries = state.auth_db.list_dataset_graph_entries(&id).unwrap();
    let pg = format!("urn:ots:statement-provenance:{id}");
    let e = entries
        .iter()
        .find(|e| e.graph_iri == pg)
        .expect("registered");
    assert_eq!(
        e.graph_role,
        Some(open_triplestore::auth::models::GraphKind::Provenance)
    );
}

#[tokio::test]
async fn every_change_to_a_statement_is_an_annotated_event() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let id = dataset(&state, &app, &token).await;
    let (st, _) = json_req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{id}/settings"),
        Some(&token),
        json!({ "provenance": "full" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let tick = || open_triplestore::statement_provenance::tick(&state);
    assert_eq!(tick(), 0, "the first tick starts from the head");
    let store_uri = format!("/store?graph={}", url_encode(G));
    let post = |body: &'static str| {
        req(
            &app,
            Method::POST,
            &store_uri,
            Some(&token),
            Some("text/turtle"),
            body,
        )
    };
    let (st, _) = post("<urn:sp:s> <urn:sp:name> \"Ada\" .").await;
    assert!(st.is_success());
    assert!(tick() > 0);
    let pg = format!("urn:ots:statement-provenance:{id}");
    // The query the design buys (§6.5).
    let q = format!(
        "PREFIX ots: <https://opentriplestore.org/ns#>
         PREFIX prov: <http://www.w3.org/ns/prov#>
         SELECT ?op ?commit ?actor ?time WHERE {{ GRAPH <{pg}> {{
           ?r <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies> <<( <urn:sp:s> <urn:sp:name> \"Ada\" )>> ; ots:change ?e .
           ?e ots:op ?op ; prov:wasGeneratedBy ?commit .
           OPTIONAL {{ ?e prov:wasAssociatedWith ?actor ; prov:atTime ?time }}
         }} }}"
    );
    let rows: Vec<(String, String, Option<String>)> = match state.store.query(&q).unwrap() {
        QueryResults::Solutions(s) => s
            .flatten()
            .map(|r| {
                (
                    r.get("op").unwrap().to_string(),
                    r.get("commit").unwrap().to_string(),
                    r.get("actor").map(|t| t.to_string()),
                )
            })
            .collect(),
        _ => panic!(),
    };
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "\"add\"");
    assert!(rows[0].1.contains("/commit/"), "{rows:?}");
    assert!(
        rows[0]
            .2
            .as_deref()
            .is_some_and(|a| a.contains("/users/adm")),
        "{rows:?}"
    );
    // Removal and re-addition keep both histories on the same reifier.
    state
        .store
        .update(&format!(
            "DELETE DATA {{ GRAPH <{G}> {{ <urn:sp:s> <urn:sp:name> \"Ada\" }} }}"
        ))
        .unwrap();
    tick();
    let n = match state
        .store
        .query(&format!(
            "SELECT (COUNT(DISTINCT ?e) AS ?n) WHERE {{ GRAPH <{pg}> {{ ?r <https://opentriplestore.org/ns#change> ?e }} }}"
        ))
        .unwrap()
    {
        QueryResults::Solutions(mut s) => s.next().unwrap().unwrap().get("n").unwrap().to_string(),
        _ => panic!(),
    };
    assert!(n.starts_with("\"2\""), "two events: {n}");
    // A blank-node statement is not annotated, and the skip is recorded.
    let (st, _) = post("<urn:sp:s> <urn:sp:geom> [ <urn:sp:wkt> \"POINT(1 2)\" ] .").await;
    assert!(st.is_success());
    tick();
    assert!(ask(
        &state,
        &format!("ASK {{ GRAPH <{pg}> {{ ?gap a <https://opentriplestore.org/ns#ProvenanceGap> ; <https://opentriplestore.org/ns#skippedBlankNodeStatements> ?n }} }}")
    ));
}

#[tokio::test]
async fn the_provenance_graph_stays_out_of_the_data() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let id = dataset(&state, &app, &token).await;
    json_req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{id}/settings"),
        Some(&token),
        json!({ "provenance": "minimal" }),
    )
    .await;
    let tick = || open_triplestore::statement_provenance::tick(&state);
    tick();
    state
        .store
        .load_str("<urn:sp:a> <urn:sp:p> 1 .", RdfFormat::Turtle, Some(G))
        .unwrap();
    assert!(tick() > 0);
    let pg = format!("urn:ots:statement-provenance:{id}");
    // A version cut leaves it out by default.
    let (st, v) = json_req(
        &app,
        Method::POST,
        &format!("/api/datasets/{id}/versions"),
        Some(&token),
        json!({ "version": "1.0.0" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let covered: Vec<&str> = v["source_map"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["source_graph"].as_str().unwrap())
        .collect();
    assert_eq!(covered, vec![G]);
    // Browsing the dataset does not list its annotations.
    let (st, body) = req(
        &app,
        Method::GET,
        &format!("/api/browse/triples?dataset_id={id}&limit=100"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(!body.contains("statement/"), "{body}");
    // JSON-LD cannot carry the triple terms: refused before any byte, not
    // truncated after a 200.
    let (st, _) = req(
        &app,
        Method::GET,
        &format!("/store?graph={}&format=jsonld", url_encode(&pg)),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::NOT_ACCEPTABLE);
    let (st, _) = req(
        &app,
        Method::GET,
        &format!("/store?graph={}&format=nquads", url_encode(&pg)),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn a_skolem_dataset_imports_the_same_blank_nodes_as_the_same_iris() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let id = dataset(&state, &app, &token).await;
    json_req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{id}/settings"),
        Some(&token),
        json!({ "skolem": true }),
    )
    .await;
    let body = "<urn:sp:f> <urn:sp:geom> [ <urn:sp:wkt> \"POINT(1 2)\" ] .";
    for _ in 0..2 {
        let (st, _) = req(
            &app,
            Method::POST,
            &format!("/store?graph={}", url_encode(G)),
            Some(&token),
            Some("text/turtle"),
            body,
        )
        .await;
        assert!(st.is_success());
    }
    assert_eq!(count(&state, G), 2, "the re-import added nothing");
    assert!(!ask(
        &state,
        &format!("ASK {{ GRAPH <{G}> {{ ?s ?p ?o FILTER(isBlank(?s) || isBlank(?o)) }} }}")
    ));
    assert!(ask(
        &state,
        &format!("ASK {{ GRAPH <{G}> {{ <urn:sp:f> <urn:sp:geom> ?g FILTER(STRSTARTS(STR(?g), \"http://localhost:7878/.well-known/genid/\")) }} }}")
    ));
}

/// The provenance graph is a graph of the dataset but not its data. A
/// dataset-wide SHACL rule run over one data graph still infers in place —
/// counting the annotations as a second data graph would move the inferences
/// into a separate inference graph — and a pipeline over the dataset is not
/// re-run by the annotations its own run's write produced.
#[tokio::test]
async fn dataset_wide_runs_leave_the_annotations_alone() {
    use open_triplestore::auth::models::{OwnerType, Visibility};
    use open_triplestore::shacl_studio::models::{
        ResultsTarget, SeverityThreshold, ShapeSource, ValidationPipeline, WriteTarget,
    };
    use open_triplestore::shacl_studio::store::ShaclStudioStore;
    use open_triplestore::shacl_studio::trigger::{self, Tick};

    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let id = dataset(&state, &app, &token).await;
    let (st, _) = json_req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{id}/settings"),
        Some(&token),
        json!({ "provenance": "minimal" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // A write-triggered pipeline over the whole dataset.
    let studio = ShaclStudioStore::new(state.auth_db.pool());
    state
        .store
        .load_str(
            "@prefix sh: <http://www.w3.org/ns/shacl#> . \
             <urn:sp:PersonShape> a sh:NodeShape ; sh:targetClass <urn:sp:Person> ; \
             sh:property [ sh:path <urn:sp:name> ; sh:minCount 1 ] .",
            RdfFormat::Turtle,
            Some("urn:sp:shapes"),
        )
        .unwrap();
    let shapes = studio
        .create_shape_graph(
            "people",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            "urn:sp:shapes",
            &[],
            ShapeSource::Manual,
            Some("adm"),
        )
        .unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    let p = ValidationPipeline {
        id: uuid::Uuid::new_v4().to_string(),
        name: "dataset on write".into(),
        description: None,
        owner_type: OwnerType::User,
        owner_id: "adm".into(),
        visibility: Visibility::Private,
        targets: vec![],
        dataset_ids: vec![id.clone()],
        graph_iris: vec![],
        target_classes: vec![],
        shape_graph_ids: vec![shapes.id],
        severity_threshold: SeverityThreshold::Violation,
        run_inference: false,
        max_results: None,
        inferred_target: WriteTarget::InPlace,
        inferred_target_graph: None,
        results_target: ResultsTarget::None,
        results_target_graph: None,
        trigger_on_write: true,
        schedule_cron: None,
        gate_writes: false,
        retention: 10,
        last_run_at: None,
        last_conforms: None,
        created_by: Some("adm".into()),
        created_at: now.clone(),
        updated_at: now,
    };
    studio.insert_pipeline(&p).unwrap();
    assert_eq!(
        open_triplestore::shacl_studio::exec::resolve_data_graphs(&state.auth_db, &studio, &p),
        vec![G.to_string()],
        "the dataset's data, without its annotations"
    );
    let annotate = || open_triplestore::statement_provenance::tick(&state);
    let validate = || trigger::tick(&state.store, &state.auth_db, &state.base_url);
    annotate();
    validate();

    state
        .store
        .load_str("<urn:sp:a> a <urn:sp:Person> .", RdfFormat::Turtle, Some(G))
        .unwrap();
    assert_eq!(
        validate(),
        Tick {
            incremental: 0,
            full: 1
        }
    );
    assert!(annotate() > 0);
    assert_eq!(
        validate(),
        Tick::default(),
        "annotating the change is not a change to validate"
    );

    // A SHACL rule run over the dataset: one data graph, so in place.
    let (st, body) = req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{id}/shapes"),
        Some(&token),
        Some("text/turtle"),
        "@prefix sh: <http://www.w3.org/ns/shacl#> . \
         <urn:sp:Tag> a sh:NodeShape ; sh:targetClass <urn:sp:Person> ; \
         sh:rule [ a sh:SPARQLRule ; sh:construct \"CONSTRUCT { $this <urn:sp:tagged> true } WHERE { }\" ] .",
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{body}");
    let (st, body) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{id}/infer"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(
        ask(
            &state,
            &format!("ASK {{ GRAPH <{G}> {{ <urn:sp:a> <urn:sp:tagged> true }} }}")
        ),
        "inferred in place: {body}"
    );
    assert!(!state
        .auth_db
        .list_dataset_graphs(&id)
        .unwrap()
        .contains(&open_triplestore::auth::dataset_graph::dataset_inference_graph_iri(&id)));
}

/// A validator that always conforms, for validate-and-commit.
fn stub_validator() {
    use axum::routing::post;
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                let app = Router::new().route(
                    "/validate",
                    post(|| async { axum::Json(json!({ "conforms": true, "report": "" })) }),
                );
                axum::serve(listener, app).await.unwrap();
            });
        });
        let addr = rx.recv().unwrap();
        std::env::set_var("OTS_VALIDATION_API_URL", format!("http://{addr}"));
    });
}

/// A write that names no graph lands in a default graph of the dataset —
/// never its provenance graph, which may well be the first one registered:
/// a property state's current value is refused for want of a data graph, and
/// validate-and-commit, which replaces the graph it lands in, makes one.
#[tokio::test]
async fn defaults_never_pick_the_provenance_graph() {
    stub_validator();
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let (st, v) = json_req(
        &app,
        Method::POST,
        "/api/datasets",
        Some(&token),
        json!({ "name": "fresh", "owner_type": "user", "owner_id": "adm", "visibility": "public" }),
    )
    .await;
    assert!(st.is_success(), "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let (st, _) = json_req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{id}/settings"),
        Some(&token),
        json!({ "provenance": "minimal" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let pg = format!("urn:ots:statement-provenance:{id}");
    assert_eq!(
        state.auth_db.list_dataset_graphs(&id).unwrap(),
        vec![pg.clone()]
    );
    state
        .store
        .load_str("<urn:sp:keep> <urn:sp:p> 1 .", RdfFormat::Turtle, Some(&pg))
        .unwrap();

    let (st, body) = json_req(
        &app,
        Method::POST,
        &format!("/api/datasets/{id}/properties/state"),
        Some(&token),
        json!({ "entity": "urn:sp:bridge", "property": "urn:sp:status", "value": "open" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(!ask(
        &state,
        &format!("ASK {{ GRAPH <{pg}> {{ <urn:sp:bridge> ?p ?o }} }}")
    ));

    let (st, body) = json_req(
        &app,
        Method::POST,
        "/api/datasets/validate-and-commit",
        Some(&token),
        json!({
            "data": { "ttl": "<urn:sp:new> <urn:sp:p> 2 ." },
            "shapes": { "ttl": "" },
            "commit_on_valid": { "target": "dataset", "dataset_id": id }
        }),
    )
    .await;
    assert!(st.is_success(), "{body}");
    assert!(ask(
        &state,
        &format!("ASK {{ GRAPH <{pg}> {{ <urn:sp:keep> <urn:sp:p> 1 }} }}")
    ));
    assert!(!ask(
        &state,
        &format!("ASK {{ GRAPH <{pg}> {{ <urn:sp:new> ?p ?o }} }}")
    ));
    assert!(ask(
        &state,
        &format!(
            "ASK {{ GRAPH <http://localhost:7878/dataset/{id}/graph/imported> {{ <urn:sp:new> <urn:sp:p> 2 }} }}"
        )
    ));
}

fn multipart(boundary: &str, parts: &[(&str, &str, Option<&str>, &str)]) -> String {
    let mut out = String::new();
    for (name, ct, filename, body) in parts {
        out.push_str(&format!("--{boundary}\r\n"));
        match filename {
            Some(f) => out.push_str(&format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\n"
            )),
            None => out.push_str(&format!(
                "Content-Disposition: form-data; name=\"{name}\"\r\n"
            )),
        }
        out.push_str(&format!("Content-Type: {ct}\r\n\r\n{body}\r\n"));
    }
    out.push_str(&format!("--{boundary}--\r\n"));
    out
}

/// #533: the statement-provenance graph is system-managed. Only the change-log
/// consumer writes it; every write path a request can take refuses it — for
/// the dataset's owner and for an admin alike — while reading it still works.
/// An admin's store-wide rewrite through a variable graph passes over it.
#[tokio::test]
async fn only_the_consumer_writes_the_provenance_graph() {
    use open_triplestore::auth::models::{OwnerType, Visibility};
    let (state, admin) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    state
        .auth_db
        .create_user("ed", "ed", "ed@t.com", "h", SystemRole::User)
        .unwrap();
    let ed = mint_token("ed", "ed", "user");
    state
        .auth_db
        .create_dataset(
            "spw",
            "spw",
            None,
            OwnerType::User,
            "ed",
            Visibility::Private,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("spw", G).unwrap();
    let (st, v) = json_req(
        &app,
        Method::PUT,
        "/api/datasets/spw/settings",
        Some(&ed),
        json!({ "provenance": "minimal" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let tick = || open_triplestore::statement_provenance::tick(&state);
    tick();
    // The owner writes the data (a patch: the dataset grants that, the graph
    // ACL is not involved); the consumer annotates it.
    let (st, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/spw/patch?graph={}", url_encode(G)),
        Some(&ed),
        Some("application/rdf-patch"),
        "TX .\nA <urn:sp:a> <urn:sp:p> \"1\" .\nTC .\n",
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    assert!(tick() > 0, "the consumer writes the graph");
    let pg = "urn:ots:statement-provenance:spw";
    let annotated = count(&state, pg);
    assert!(annotated > 0);

    let forged = "<urn:sp:x> <urn:sp:p> \"forged\" .";
    for (who, token) in [("owner", ed.as_str()), ("admin", admin.as_str())] {
        let refused = |st: StatusCode, txt: &str, path: &str| {
            assert_eq!(st, StatusCode::FORBIDDEN, "{who}, {path}: {txt}");
        };
        for update in [
            format!("INSERT DATA {{ GRAPH <{pg}> {{ {forged} }} }}"),
            format!("DELETE WHERE {{ GRAPH <{pg}> {{ ?s ?p ?o }} }}"),
            format!("CLEAR GRAPH <{pg}>"),
            format!("DROP GRAPH <{pg}>"),
            format!(
                "INSERT {{ GRAPH <{pg}> {{ ?s ?p ?o }} }} WHERE {{ GRAPH <{G}> {{ ?s ?p ?o }} }}"
            ),
        ] {
            let (st, txt) = req(
                &app,
                Method::POST,
                "/sparql",
                Some(token),
                Some("application/sparql-update"),
                &update,
            )
            .await;
            refused(st, &txt, &update);
        }
        let gsp = format!("/store?graph={}", url_encode(pg));
        for method in [Method::PUT, Method::POST] {
            let (st, txt) = req(
                &app,
                method.clone(),
                &gsp,
                Some(token),
                Some("text/turtle"),
                forged,
            )
            .await;
            refused(st, &txt, &format!("GSP {method}"));
        }
        let (st, txt) = req(&app, Method::DELETE, &gsp, Some(token), None, "").await;
        refused(st, &txt, "GSP DELETE");
        let patch = format!("TX .\nA {} <{pg}> .\nTC .\n", forged.trim_end_matches(" ."));
        let (st, txt) = req(
            &app,
            Method::POST,
            "/api/datasets/spw/patch",
            Some(token),
            Some("application/rdf-patch"),
            &patch,
        )
        .await;
        refused(st, &txt, "RDF Patch");
        let (st, txt) = req(
            &app,
            Method::POST,
            "/api/datasets/spw/log",
            Some(token),
            Some("application/rdf-patch"),
            &format!("H id <urn:uuid:{}> .\n{patch}", uuid::Uuid::new_v4()),
        )
        .await;
        refused(st, &txt, "patch log append");
        let (st, txt) = req(
            &app,
            Method::POST,
            "/api/import/bulk",
            Some(token),
            Some("multipart/form-data; boundary=BND"),
            &multipart(
                "BND",
                &[
                    ("dataset_id", "text/plain", None, "spw"),
                    ("default_target_graph", "text/plain", None, pg),
                    ("file", "text/turtle", Some("a.ttl"), forged),
                ],
            ),
        )
        .await;
        refused(st, &txt, "bulk import");
    }
    // Nor can another dataset take it up.
    let (st, v) = json_req(
        &app,
        Method::POST,
        "/api/datasets",
        Some(&admin),
        json!({ "name": "spw2", "owner_type": "user", "owner_id": "adm", "visibility": "private" }),
    )
    .await;
    assert!(st.is_success(), "{v}");
    let other = v["id"].as_str().unwrap().to_string();
    let (st, v) = json_req(
        &app,
        Method::POST,
        &format!("/api/datasets/{other}/graphs"),
        Some(&admin),
        json!({ "graph_iri": pg }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(count(&state, pg), annotated, "nothing got through");

    // An admin's variable-graph rewrite passes over the provenance graph.
    let (st, txt) = req(
        &app,
        Method::POST,
        "/sparql",
        Some(&admin),
        Some("application/sparql-update"),
        &format!(
            "DELETE {{ GRAPH ?g {{ ?s ?p ?o }} }} WHERE {{ GRAPH ?g {{ ?s ?p ?o }} FILTER(?g IN (<{G}>, <{pg}>)) }}"
        ),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    assert_eq!(count(&state, G), 0, "the data graph was rewritten");
    assert_eq!(count(&state, pg), annotated, "the annotations were not");

    // Reading it is unaffected, and the consumer keeps writing it.
    let (st, body) = req(
        &app,
        Method::GET,
        &format!("/store?graph={}&format=nquads", url_encode(pg)),
        Some(&admin),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    tick();
    let before = count(&state, pg);
    let (st, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/spw/patch?graph={}", url_encode(G)),
        Some(&ed),
        Some("application/rdf-patch"),
        "TX .\nA <urn:sp:b> <urn:sp:p> \"2\" .\nTC .\n",
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    assert!(tick() > 0, "the consumer still writes the graph");
    assert!(count(&state, pg) > before);
}

const HIDDEN: &str = "urn:sp:hidden";
const HIDDEN_VALUE: &str = "kept-from-readers";
const OPEN_VALUE: &str = "open-to-readers";
const ROUTES: [&str; 3] = ["/sparql", "/store", "the change feed"];
const OTS_GRAPH: &str = "https://opentriplestore.org/ns#graph";

/// The callers of a dataset: its admin, a signed-in user with a Viewer grant
/// and one with an Editor grant.
struct Readers {
    admin: String,
    viewer: String,
    writer: String,
}

impl Readers {
    fn each(&self) -> [(&'static str, Option<&str>); 4] {
        [
            ("anonymous", None),
            ("viewer", Some(self.viewer.as_str())),
            ("writer", Some(self.writer.as_str())),
            ("admin", Some(self.admin.as_str())),
        ]
    }
}

/// A public dataset that records `full` provenance, holding `G` and `HIDDEN`
/// (not private yet), with a viewer and a writer besides the admin. The
/// consumer's cursor is at the head.
async fn two_graph_dataset() -> (AppState, Router, String, Readers) {
    use open_triplestore::auth::models::ResourceRole;
    // Every caller over every route, more than once: more requests than the
    // per-IP limiter allows one test.
    std::env::set_var("OTS_RATE_LIMIT_ENABLED", "false");
    let (state, admin) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let id = dataset(&state, &app, &admin).await;
    let (st, v) = json_req(
        &app,
        Method::POST,
        &format!("/api/datasets/{id}/graphs"),
        Some(&admin),
        json!({ "graph_iri": HIDDEN }),
    )
    .await;
    assert!(st.is_success(), "{v}");
    for (user, role) in [
        ("vera", ResourceRole::Viewer),
        ("wes", ResourceRole::Editor),
    ] {
        state
            .auth_db
            .create_user(user, user, &format!("{user}@t.com"), "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .set_resource_grant("dataset", &id, "user", user, role, "adm")
            .unwrap();
    }
    state.auth_db.invalidate_accessible_graphs_cache();
    let (st, v) = json_req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{id}/settings"),
        Some(&admin),
        json!({ "provenance": "full" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    open_triplestore::statement_provenance::tick(&state);
    let readers = Readers {
        admin,
        viewer: mint_token("vera", "vera", "user"),
        writer: mint_token("wes", "wes", "user"),
    };
    (state, app, id, readers)
}

/// `PATCH /api/datasets/{id}/graphs`: the privacy toggle.
async fn set_private(
    app: &Router,
    token: &str,
    id: &str,
    graph: &str,
    private: bool,
) -> (StatusCode, Value) {
    json_req(
        app,
        Method::PATCH,
        &format!("/api/datasets/{id}/graphs"),
        Some(token),
        json!({ "graph_iri": graph, "private": private }),
    )
    .await
}

/// One statement `<urn:sp:s> <urn:sp:note> "value"` into `graph`.
async fn note(app: &Router, token: &str, graph: &str, value: &str) {
    let (st, txt) = req(
        app,
        Method::POST,
        &format!("/store?graph={}", url_encode(graph)),
        Some(token),
        Some("text/turtle"),
        &format!("<urn:sp:s> <urn:sp:note> \"{value}\" ."),
    )
    .await;
    assert!(st.is_success(), "{graph}: {txt}");
}

/// What `token` is served of the dataset's annotations, as three texts: over
/// `POST /sparql` (every annotation in any graph the caller may read), over
/// `GET /store?graph=` on its two statement-provenance graphs, and over its
/// change feed (the patches of the rows the consumer's own writes to those
/// graphs left in the change log).
async fn provenance_texts(app: &Router, token: Option<&str>, id: &str) -> [String; 3] {
    let query = "SELECT ?pg ?g ?statement ?op WHERE { GRAPH ?pg {
           ?r <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies> ?statement ;
              <https://opentriplestore.org/ns#graph> ?g ;
              <https://opentriplestore.org/ns#change> ?e .
           ?e <https://opentriplestore.org/ns#op> ?op } }";
    let (st, sparql) = req(
        app,
        Method::POST,
        "/sparql",
        token,
        Some("application/sparql-query"),
        query,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{sparql}");
    let mut store = String::new();
    for pg in [
        format!("urn:ots:statement-provenance:{id}"),
        format!("urn:ots:statement-provenance:{id}:private"),
    ] {
        let (st, body) = req(
            app,
            Method::GET,
            &format!("/store?graph={}&format=nquads", url_encode(&pg)),
            token,
            None,
            "",
        )
        .await;
        if st == StatusCode::OK {
            store.push_str(&body);
        }
    }
    let (st, feed) = json_req(
        app,
        Method::GET,
        &format!("/api/datasets/{id}/changes"),
        token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{feed}");
    let mut patches = String::new();
    for row in feed["rows"].as_array().unwrap() {
        let annotations = row["graph_iri"]
            .as_str()
            .is_some_and(|g| g.starts_with("urn:ots:statement-provenance:"));
        if annotations {
            let (_, patch) = req(
                app,
                Method::GET,
                &format!("/api/datasets/{id}/changes/{}", row["seq"]),
                token,
                None,
                "",
            )
            .await;
            patches.push_str(&patch);
        }
    }
    [sparql, store, patches]
}

/// The rule, for every caller over every route: what the annotations say of
/// the private graph `HIDDEN` (its statement as a triple term, its IRI as
/// `ots:graph`) is served to the dataset's writers and to nobody else; what
/// they say of the public graph is served to every reader, except over the
/// change feed, whose statement-provenance rows are the writers'.
async fn assert_private_annotations_are_the_writers(app: &Router, id: &str, readers: &Readers) {
    for (who, token) in readers.each() {
        let writes = matches!(who, "writer" | "admin");
        let texts = provenance_texts(app, token, id).await;
        for (route, text) in ROUTES.iter().zip(texts) {
            assert_eq!(
                text.contains(HIDDEN_VALUE) || text.contains(HIDDEN),
                writes,
                "{who} over {route}: the private graph's annotations\n{text}"
            );
            assert_eq!(
                text.contains(OPEN_VALUE),
                writes || *route != "the change feed",
                "{who} over {route}: the public graph's annotations\n{text}"
            );
        }
    }
}

/// Whether statement-provenance graph `pg` says anything about `graph`.
fn annotates(state: &AppState, pg: &str, graph: &str) -> bool {
    ask(
        state,
        &format!("ASK {{ GRAPH <{pg}> {{ ?r <{OTS_GRAPH}> <{graph}> }} }}"),
    )
}

/// A private graph is read by its dataset's writers and graph-ACL readers
/// only; the annotations of its statements are that graph's statements again
/// (`rdf:reifies <<( s p o )>>`, `ots:graph`), so they are read by the same
/// callers and no others, on a public dataset too: they go to the dataset's
/// private statement-provenance graph.
#[tokio::test]
async fn a_private_graphs_annotations_are_read_only_by_who_may_read_the_graph() {
    let (state, app, id, readers) = two_graph_dataset().await;
    let (st, v) = set_private(&app, &readers.admin, &id, HIDDEN, true).await;
    assert!(st.is_success(), "{v}");
    note(&app, &readers.admin, G, OPEN_VALUE).await;
    note(&app, &readers.admin, HIDDEN, HIDDEN_VALUE).await;
    assert!(open_triplestore::statement_provenance::tick(&state) > 0);

    assert_private_annotations_are_the_writers(&app, &id, &readers).await;

    let pg = format!("urn:ots:statement-provenance:{id}");
    let private_pg = format!("{pg}:private");
    assert!(annotates(&state, &pg, G) && !annotates(&state, &pg, HIDDEN));
    assert!(annotates(&state, &private_pg, HIDDEN) && !annotates(&state, &private_pg, G));
    // Registered as the public one is, and private.
    let entry = state
        .auth_db
        .list_dataset_graph_entries(&id)
        .unwrap()
        .into_iter()
        .find(|e| e.graph_iri == private_pg)
        .expect("the private statement-provenance graph is registered");
    assert!(entry.private);
    assert_eq!(
        entry.graph_role,
        Some(open_triplestore::auth::models::GraphKind::Provenance)
    );
    let (_, settings) = json_req(
        &app,
        Method::GET,
        &format!("/api/datasets/{id}/settings"),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(settings["provenance_graph"], pg.as_str());
    assert_eq!(settings["provenance_private_graph"], private_pg.as_str());

    // The feed lists a statement-provenance row to a writer only, and serves
    // its patch to a writer only.
    let feed_of = |token: Option<String>| {
        let app = app.clone();
        let uri = format!("/api/datasets/{id}/changes");
        async move {
            let (_, feed) = json_req(&app, Method::GET, &uri, token.as_deref(), Value::Null).await;
            feed["rows"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| {
                    r["graph_iri"]
                        .as_str()
                        .is_some_and(|g| g.starts_with("urn:ots:statement-provenance:"))
                })
                .map(|r| r["seq"].as_i64().unwrap())
                .collect::<Vec<i64>>()
        }
    };
    let rows = feed_of(Some(readers.writer.clone())).await;
    assert_eq!(rows.len(), 2, "one row per provenance graph");
    assert!(feed_of(Some(readers.viewer.clone())).await.is_empty());
    assert!(feed_of(None).await.is_empty());
    for seq in rows {
        for (who, token) in readers.each() {
            let (st, _) = req(
                &app,
                Method::GET,
                &format!("/api/datasets/{id}/changes/{seq}"),
                token,
                None,
                "",
            )
            .await;
            assert_eq!(
                st == StatusCode::OK,
                matches!(who, "writer" | "admin"),
                "{who}, change {seq}: {st}"
            );
        }
    }
}

/// A graph made private after it was annotated takes its annotations along,
/// in the request that makes it private: reifiers, their change events and
/// gap nodes. Made public again, it is annotated publicly from then on, and
/// what was annotated while it was private stays private.
#[tokio::test]
async fn a_graph_made_private_takes_its_annotations_along() {
    let (state, app, id, readers) = two_graph_dataset().await;
    let tick = || open_triplestore::statement_provenance::tick(&state);
    let pg = format!("urn:ots:statement-provenance:{id}");
    let private_pg = format!("{pg}:private");
    note(&app, &readers.admin, G, OPEN_VALUE).await;
    note(&app, &readers.admin, HIDDEN, HIDDEN_VALUE).await;
    // A statement with a blank node is not annotated: a gap node records it.
    let (st, txt) = req(
        &app,
        Method::POST,
        &format!("/store?graph={}", url_encode(HIDDEN)),
        Some(&readers.admin),
        Some("text/turtle"),
        "[] <urn:sp:note> \"anonymous\" .",
    )
    .await;
    assert!(st.is_success(), "{txt}");
    assert!(tick() > 0);
    // Public so far: every reader has the graph and its annotations.
    let [sparql, _, _] = provenance_texts(&app, None, &id).await;
    assert!(sparql.contains(HIDDEN_VALUE), "{sparql}");
    let gap = |graph: &str| {
        ask(
            &state,
            &format!(
                "ASK {{ GRAPH <{graph}> {{ ?gap a <https://opentriplestore.org/ns#ProvenanceGap> ; <{OTS_GRAPH}> <{HIDDEN}> }} }}"
            ),
        )
    };
    assert!(gap(&pg) && !gap(&private_pg));
    let before = count(&state, &pg);

    let (st, v) = set_private(&app, &readers.admin, &id, HIDDEN, true).await;
    assert!(st.is_success(), "{v}");
    // No tick in between: the request moved them.
    assert!(!annotates(&state, &pg, HIDDEN) && annotates(&state, &private_pg, HIDDEN));
    assert!(!gap(&pg) && gap(&private_pg));
    assert_eq!(
        count(&state, &pg) + count(&state, &private_pg),
        before,
        "moved, none lost and none copied"
    );
    // The events went with their reifiers.
    assert!(ask(
        &state,
        &format!(
            "ASK {{ GRAPH <{private_pg}> {{ ?r <{OTS_GRAPH}> <{HIDDEN}> ; <https://opentriplestore.org/ns#change> ?e . ?e <https://opentriplestore.org/ns#op> \"add\" ; <http://www.w3.org/ns/prov#atTime> ?t }} }}"
        )
    ));
    assert!(!ask(
        &state,
        &format!(
            "ASK {{ GRAPH <{pg}> {{ ?e <https://opentriplestore.org/ns#op> ?op . FILTER NOT EXISTS {{ ?r <https://opentriplestore.org/ns#change> ?e }} }} }}"
        )
    ));
    // The change log still holds the rows that wrote them publicly, and the
    // move's own row: none of it is served to a reader.
    assert_private_annotations_are_the_writers(&app, &id, &readers).await;

    // While private: a removal is annotated privately.
    state
        .store
        .update(&format!(
            "DELETE DATA {{ GRAPH <{HIDDEN}> {{ <urn:sp:s> <urn:sp:note> \"{HIDDEN_VALUE}\" }} }}"
        ))
        .unwrap();
    assert!(tick() > 0);
    assert!(!annotates(&state, &pg, HIDDEN));
    assert_private_annotations_are_the_writers(&app, &id, &readers).await;

    // Public again: annotated publicly from here on; the private period,
    // with the statement removed in it, stays with the writers.
    let (st, v) = set_private(&app, &readers.admin, &id, HIDDEN, false).await;
    assert!(st.is_success(), "{v}");
    note(&app, &readers.admin, HIDDEN, "open-again").await;
    assert!(tick() > 0);
    for (who, token) in readers.each() {
        let writes = matches!(who, "writer" | "admin");
        let [sparql, store, patches] = provenance_texts(&app, token, &id).await;
        assert!(sparql.contains("open-again"), "{who}: {sparql}");
        assert!(store.contains("open-again"), "{who}: {store}");
        for (route, text) in ROUTES.iter().zip([sparql, store, patches]) {
            assert_eq!(
                text.contains(HIDDEN_VALUE),
                writes,
                "{who} over {route}: the private period\n{text}"
            );
        }
    }
}

/// The flag is not only set through the privacy toggle (a report graph
/// follows the graph it reports on, a pipeline flags its target): whatever
/// set it, the consumer's next tick moves the annotations, also for a
/// dataset that has stopped recording provenance since.
#[tokio::test]
async fn a_flag_set_outside_the_toggle_is_followed_on_the_next_tick() {
    let (state, app, id, readers) = two_graph_dataset().await;
    let tick = || open_triplestore::statement_provenance::tick(&state);
    let pg = format!("urn:ots:statement-provenance:{id}");
    note(&app, &readers.admin, G, OPEN_VALUE).await;
    note(&app, &readers.admin, HIDDEN, HIDDEN_VALUE).await;
    assert!(tick() > 0);
    let (st, v) = json_req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{id}/settings"),
        Some(&readers.admin),
        json!({ "provenance": "off" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    state
        .auth_db
        .set_dataset_graph_private(&id, HIDDEN, true)
        .unwrap();
    assert!(annotates(&state, &pg, HIDDEN), "nothing has moved them yet");
    // Nothing to annotate; the tick still moves what is there.
    assert_eq!(tick(), 0);
    assert!(!annotates(&state, &pg, HIDDEN));
    assert_private_annotations_are_the_writers(&app, &id, &readers).await;
}

/// An install that annotated private graphs before datasets had a private
/// statement-provenance graph holds their annotations where every reader of
/// the dataset reads. The one-time pass moves them: for a graph that is
/// private, and for a graph the dataset no longer holds (its flag can no
/// longer be told), also where the dataset has since stopped recording
/// provenance.
#[tokio::test]
async fn annotations_of_private_graphs_written_earlier_are_moved_once() {
    use open_triplestore::statement_provenance::{
        existing_annotations_split, split_existing_annotations, tick,
    };
    const GONE: &str = "urn:sp:gone";
    const GONE_VALUE: &str = "of-a-detached-graph";
    let (state, app, id, readers) = two_graph_dataset().await;
    let pg = format!("urn:ots:statement-provenance:{id}");
    let private_pg = format!("{pg}:private");
    let (st, v) = json_req(
        &app,
        Method::POST,
        &format!("/api/datasets/{id}/graphs"),
        Some(&readers.admin),
        json!({ "graph_iri": GONE }),
    )
    .await;
    assert!(st.is_success(), "{v}");
    note(&app, &readers.admin, G, OPEN_VALUE).await;
    note(&app, &readers.admin, HIDDEN, HIDDEN_VALUE).await;
    note(&app, &readers.admin, GONE, GONE_VALUE).await;
    assert!(tick(&state) > 0);
    // The earlier layout: everything in the one graph, then the flag and the
    // detach as the identity database records them, and provenance off.
    state
        .auth_db
        .set_dataset_graph_private(&id, HIDDEN, true)
        .unwrap();
    state.auth_db.remove_dataset_graph(&id, GONE).unwrap();
    let (st, v) = json_req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{id}/settings"),
        Some(&readers.admin),
        json!({ "provenance": "off" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(annotates(&state, &pg, HIDDEN) && annotates(&state, &pg, GONE));
    let [sparql, _, _] = provenance_texts(&app, None, &id).await;
    assert!(
        sparql.contains(HIDDEN_VALUE),
        "what the pass is for: {sparql}"
    );

    assert!(!existing_annotations_split(&state.auth_db));
    assert!(split_existing_annotations(&state).unwrap() > 0);
    assert!(existing_annotations_split(&state.auth_db));
    assert!(annotates(&state, &pg, G));
    for graph in [HIDDEN, GONE] {
        assert!(!annotates(&state, &pg, graph), "{graph}");
        assert!(annotates(&state, &private_pg, graph), "{graph}");
    }
    assert_private_annotations_are_the_writers(&app, &id, &readers).await;
    for (who, token) in readers.each() {
        let writes = matches!(who, "writer" | "admin");
        for (route, text) in ROUTES.iter().zip(provenance_texts(&app, token, &id).await) {
            assert_eq!(
                text.contains(GONE_VALUE),
                writes,
                "{who} over {route}: the detached graph's annotations\n{text}"
            );
        }
    }
    assert_eq!(
        split_existing_annotations(&state).unwrap(),
        0,
        "nothing left to move"
    );
}

/// The private statement-provenance graph stays private and system-managed:
/// its flag cannot be cleared, no request writes it, and detaching it drops
/// the stored graph with the registration (a graph no dataset holds is read
/// by everyone). The consumer registers it again, private, when it next
/// needs it.
#[tokio::test]
async fn the_private_provenance_graph_stays_private() {
    let (state, app, id, readers) = two_graph_dataset().await;
    let tick = || open_triplestore::statement_provenance::tick(&state);
    let private_pg = format!("urn:ots:statement-provenance:{id}:private");
    let (st, v) = set_private(&app, &readers.admin, &id, HIDDEN, true).await;
    assert!(st.is_success(), "{v}");
    note(&app, &readers.admin, HIDDEN, HIDDEN_VALUE).await;
    assert!(tick() > 0);
    let is_private = || {
        state
            .auth_db
            .list_dataset_graph_entries(&id)
            .unwrap()
            .into_iter()
            .find(|e| e.graph_iri == private_pg)
            .map(|e| e.private)
    };
    assert_eq!(is_private(), Some(true));

    for token in [&readers.writer, &readers.admin] {
        let (st, _) = set_private(&app, token, &id, &private_pg, false).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, txt) = req(
            &app,
            Method::POST,
            &format!("/store?graph={}", url_encode(&private_pg)),
            Some(token),
            Some("text/turtle"),
            "<urn:sp:x> <urn:sp:p> 1 .",
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{txt}");
    }
    assert_eq!(is_private(), Some(true));
    // A flag cleared behind the API is set again before the next write.
    state
        .auth_db
        .set_dataset_graph_private(&id, &private_pg, false)
        .unwrap();
    note(&app, &readers.admin, HIDDEN, "second").await;
    assert!(tick() > 0);
    assert_eq!(is_private(), Some(true));

    let (st, v) = json_req(
        &app,
        Method::DELETE,
        &format!("/api/datasets/{id}/graphs"),
        Some(&readers.writer),
        json!({ "graph_iri": private_pg }),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{v}");
    assert_eq!(is_private(), None);
    assert_eq!(
        count(&state, &private_pg),
        0,
        "the stored graph went with it"
    );
    note(&app, &readers.admin, HIDDEN, "third").await;
    assert!(tick() > 0);
    assert_eq!(is_private(), Some(true));
    assert!(annotates(&state, &private_pg, HIDDEN));
    assert_private_annotations_are_the_writers_of_hidden(&app, &id, &readers).await;
}

/// As [`assert_private_annotations_are_the_writers`], for a dataset whose
/// public graph holds nothing: only the private graph's side of the rule.
async fn assert_private_annotations_are_the_writers_of_hidden(
    app: &Router,
    id: &str,
    readers: &Readers,
) {
    for (who, token) in readers.each() {
        let writes = matches!(who, "writer" | "admin");
        for (route, text) in ROUTES.iter().zip(provenance_texts(app, token, id).await) {
            assert_eq!(
                text.contains(HIDDEN_VALUE) || text.contains(HIDDEN),
                writes,
                "{who} over {route}: the private graph's annotations\n{text}"
            );
        }
    }
}
