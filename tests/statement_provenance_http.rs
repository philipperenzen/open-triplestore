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
        std::env::set_var("VALIDATION_API_URL", format!("http://{addr}"));
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
