//! Selectable entailment per dataset (7.3): a dataset picks a regime and a
//! materialisation mode; writes re-materialise into the dataset's own
//! entailment graph; queries opt in with `entailment_dataset`, over GET and
//! both POST flavours; switching off clears the graph.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{GraphKind, OwnerType, SystemRole, Visibility};
use oxigraph::io::RdfFormat;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const MODEL: &str = "https://example.org/ent/model";
const DATA: &str = "https://example.org/ent/instances";
const EX: &str = "https://example.org/ent/";

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    ct: Option<&str>,
    body: &str,
) -> (StatusCode, Value, String) {
    let mut b = Request::builder().method(method).uri(uri).header(
        header::ACCEPT,
        "application/sparql-results+json, application/json",
    );
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
    let text = body_text(resp.into_body()).await;
    (st, serde_json::from_str(&text).unwrap_or(Value::Null), text)
}

fn rows(v: &Value) -> usize {
    v["results"]["bindings"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0)
}

#[tokio::test]
async fn dataset_regime_materialises_on_write_and_joins_queries_on_request() {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "ent",
            "Entailment",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("ent", MODEL).unwrap();
    state
        .auth_db
        .set_dataset_graph_role("ent", MODEL, Some(GraphKind::Model))
        .unwrap();
    state.auth_db.add_dataset_graph("ent", DATA).unwrap();
    state
        .auth_db
        .set_dataset_graph_role("ent", DATA, Some(GraphKind::Instances))
        .unwrap();
    state
        .store
        .load_str(
            &format!(
                "<{EX}Bridge> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <{EX}Asset> ."
            ),
            RdfFormat::Turtle,
            Some(MODEL),
        )
        .unwrap();
    state
        .store
        .load_str(
            &format!("<{EX}b1> a <{EX}Bridge> ."),
            RdfFormat::Turtle,
            Some(DATA),
        )
        .unwrap();
    let app = test_app(state.clone());
    let q = format!("SELECT ?b WHERE {{ ?b a <{EX}Asset> }}");
    let enc = url_encode(&q);

    // Nothing configured: no inferred Asset.
    let (st, v, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/ent/entailment",
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["mode"], "off");
    let (st, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={enc}&entailment_dataset=ent"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(rows(&v), 0, "{txt}");

    // Select RDFS + materialize: runs immediately.
    let (st, v, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/ent/entailment",
        Some(&token),
        Some("application/json"),
        &json!({ "regime": "rdfs", "mode": "materialize" }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["graph"], "urn:entailment:rdfs:ent");
    assert!(v["triples"].as_i64().unwrap() >= 1, "{txt}");
    let (st, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={enc}&entailment_dataset=ent"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(
        rows(&v),
        1,
        "b1 is an Asset through the dataset's entailment graph: {txt}"
    );
    let (_, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={enc}"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(
        rows(&v),
        0,
        "without opting in, the entailment graph stays out: {txt}"
    );

    // POST, both flavours.
    let (st, v, txt) = req(
        &app,
        Method::POST,
        "/sparql?entailment_dataset=ent",
        Some(&token),
        Some("application/sparql-query"),
        &q,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(rows(&v), 1, "{txt}");
    let (st, v, txt) = req(
        &app,
        Method::POST,
        "/sparql",
        Some(&token),
        Some("application/x-www-form-urlencoded"),
        &format!("query={enc}&entailment_dataset=ent"),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(rows(&v), 1, "{txt}");

    // A write re-materialises: a second bridge appears as an Asset.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/store?graph={}", url_encode(DATA)),
        Some(&token),
        Some("text/turtle"),
        &format!("<{EX}b2> a <{EX}Bridge> ."),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    let (_, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={enc}&entailment_dataset=ent"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(rows(&v), 2, "{txt}");
    // …and a delete drops the consequence (the graph is rebuilt, not appended).
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        &format!("/store?graph={}", url_encode(DATA)),
        Some(&token),
        Some("text/turtle"),
        &format!("<{EX}b2> a <{EX}Bridge> ."),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    let (_, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={enc}&entailment_dataset=ent"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(
        rows(&v),
        1,
        "b1 was removed, so its inferred type is gone: {txt}"
    );
    let (_, v, _) = req(
        &app,
        Method::GET,
        "/api/datasets/ent/entailment",
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(v["regime"], "rdfs");
    assert!(v["last_run_at"].is_string());

    // Off clears the graph; unknown regimes are refused; strangers cannot configure.
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/ent/entailment",
        Some(&token),
        Some("application/json"),
        &json!({ "regime": "rdfs", "mode": "off" }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let (_, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={enc}&entailment_dataset=ent"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(rows(&v), 0, "{txt}");
    let (st, _, _) = req(
        &app,
        Method::PUT,
        "/api/datasets/ent/entailment",
        Some(&token),
        Some("application/json"),
        &json!({ "regime": "magic" }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    state
        .auth_db
        .create_user("eve", "eve", "eve@t.com", "h", SystemRole::User)
        .unwrap();
    let eve = mint_token("eve", "eve", "user");
    let (st, _, _) = req(
        &app,
        Method::PUT,
        "/api/datasets/ent/entailment",
        Some(&eve),
        Some("application/json"),
        &json!({ "regime": "rdfs" }).to_string(),
    )
    .await;
    assert!(
        st == StatusCode::FORBIDDEN || st == StatusCode::NOT_FOUND,
        "{st}"
    );
}

/// OWL 2 QL as a dataset regime: a write re-materialises the ground closure
/// into the dataset's entailment graph, and a query that opts in has its
/// blank nodes rewritten over the TBox, so it reaches the elements the
/// model's existentials imply. The materialisation used to write only the
/// TBox closure, and to the shared graph rather than the dataset's.
#[cfg(feature = "owl2-ql")]
#[tokio::test]
async fn dataset_owl2_ql_materialises_and_rewrites_blank_nodes() {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "ql",
            "QL",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    for (g, kind) in [(MODEL, GraphKind::Model), (DATA, GraphKind::Instances)] {
        state.auth_db.add_dataset_graph("ql", g).unwrap();
        state
            .auth_db
            .set_dataset_graph_role("ql", g, Some(kind))
            .unwrap();
    }
    state
        .store
        .load_str(
            &format!(
                "@prefix owl: <http://www.w3.org/2002/07/owl#> . \
                 @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
                 <{EX}Bridge> rdfs:subClassOf <{EX}Asset> , \
                     [ owl:onProperty <{EX}hasSpan> ; owl:someValuesFrom <{EX}Span> ] . \
                 <{EX}hasSpan> rdfs:domain <{EX}Structure> ."
            ),
            RdfFormat::Turtle,
            Some(MODEL),
        )
        .unwrap();
    state
        .store
        .load_str(
            &format!("<{EX}b1> a <{EX}Bridge> ."),
            RdfFormat::Turtle,
            Some(DATA),
        )
        .unwrap();
    let app = test_app(state.clone());
    let (st, v, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/ql/entailment",
        Some(&token),
        Some("application/json"),
        &json!({ "regime": "owl2-ql", "mode": "materialize" }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["graph"], "urn:entailment:owl2-ql:ql");
    let select = |q: &str| {
        let app = app.clone();
        let token = token.clone();
        let uri = format!("/sparql?query={}&entailment_dataset=ql", url_encode(q));
        async move {
            let (st, v, txt) = req(&app, Method::GET, &uri, Some(&token), None, "").await;
            assert_eq!(st, StatusCode::OK, "{txt}");
            rows(&v)
        }
    };
    // Ground atoms, through the hierarchy and the existential's domain.
    assert_eq!(
        select(&format!("SELECT ?b WHERE {{ ?b a <{EX}Asset> }}")).await,
        1
    );
    assert_eq!(
        select(&format!("SELECT ?b WHERE {{ ?b a <{EX}Structure> }}")).await,
        1
    );
    // The span exists but has no name: a blank node finds it, a variable not.
    let anon = format!("SELECT ?b WHERE {{ ?b <{EX}hasSpan> [ a <{EX}Span> ] }}");
    assert_eq!(select(&anon).await, 1);
    assert_eq!(
        select(&format!("SELECT ?b ?s WHERE {{ ?b <{EX}hasSpan> ?s }}")).await,
        0
    );
    // A write re-materialises: a second bridge has a span too.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/store?graph={}", url_encode(DATA)),
        Some(&token),
        Some("text/turtle"),
        &format!("<{EX}b2> a <{EX}Bridge> ."),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    assert_eq!(select(&anon).await, 2);
    assert_eq!(
        select(&format!("SELECT ?b WHERE {{ ?b a <{EX}Asset> }}")).await,
        2
    );
    // Without opting in, blank nodes are plain variables over the data.
    let (_, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={}", url_encode(&anon)),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(rows(&v), 0, "{txt}");
}

/// An `owl2-dl` dataset is not re-materialised inside the write: the write
/// answers at once and a debounced background run follows (D9). `GET
/// …/entailment` says `queued` meanwhile and then what the run found, with
/// the backend that ran and whether it is complete.
#[cfg(feature = "owl2-dl")]
#[tokio::test]
async fn dl_dataset_reruns_in_the_background_after_a_write() {
    use open_triplestore::reasoning::dl_config::{DlBackendKind, DlConfig};
    let (mut state, token) = admin_state();
    let mut cfg = DlConfig::default().with_backend(DlBackendKind::Native);
    cfg.debounce = std::time::Duration::from_millis(1500);
    state.dl = std::sync::Arc::new(cfg);
    state
        .auth_db
        .create_dataset(
            "dlbg",
            "DL",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    for (g, role) in [(MODEL, GraphKind::Model), (DATA, GraphKind::Instances)] {
        state.auth_db.add_dataset_graph("dlbg", g).unwrap();
        state
            .auth_db
            .set_dataset_graph_role("dlbg", g, Some(role))
            .unwrap();
    }
    state
        .store
        .load_str(
            &format!(
                "<{EX}Bridge> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <{EX}Asset> ."
            ),
            RdfFormat::Turtle,
            Some(MODEL),
        )
        .unwrap();
    state
        .store
        .load_str(
            &format!("<{EX}b1> a <{EX}Bridge> ."),
            RdfFormat::Turtle,
            Some(DATA),
        )
        .unwrap();
    let app = test_app(state.clone());
    let enc = url_encode(&format!("SELECT ?b WHERE {{ ?b a <{EX}Asset> }}"));

    // Selecting the regime runs at once, as for every regime.
    let (st, v, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/dlbg/entailment",
        Some(&token),
        Some("application/json"),
        &json!({ "regime": "owl2-dl", "mode": "materialize" }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["backend"], "native", "{txt}");
    assert_eq!(v["complete"], json!(false), "{txt}");

    // The write answers without waiting for the DL run.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/store?graph={}", url_encode(DATA)),
        Some(&token),
        Some("text/turtle"),
        &format!("<{EX}b2> a <{EX}Bridge> ."),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    let (_, v, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/dlbg/entailment",
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(v["status"], "queued", "{txt}");
    assert_eq!(v["dl_backend"], "native", "{txt}");

    // …and the background run catches up.
    let mut caught_up = false;
    for _ in 0..300 {
        let (_, v, _) = req(
            &app,
            Method::GET,
            "/api/datasets/dlbg/entailment",
            Some(&token),
            None,
            "",
        )
        .await;
        let (_, rows_v, _) = req(
            &app,
            Method::GET,
            &format!("/sparql?query={enc}&entailment_dataset=dlbg"),
            Some(&token),
            None,
            "",
        )
        .await;
        if v["status"] == "ok" && rows(&rows_v) == 2 {
            assert_eq!(v["backend"], "native", "{v}");
            assert_eq!(v["complete"], json!(false), "{v}");
            assert_eq!(v["consistent"], json!(true), "{v}");
            caught_up = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(caught_up, "the background owl2-dl run did not finish");
}

/// The `skos` regime: OWL 2 RL over the dataset with the bundled SKOS schema
/// as a premise, so the SKOS data model's inverses, symmetric and transitive
/// properties and label sub-properties are materialised — and the schema's
/// own closure is not.
#[cfg(feature = "owl2-rl")]
#[tokio::test]
async fn skos_regime_materialises_the_skos_data_model() {
    const THES: &str = "https://example.org/thes/concepts";
    const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "thes",
            "Thesaurus",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("thes", THES).unwrap();
    state
        .auth_db
        .set_dataset_graph_role("thes", THES, Some(GraphKind::Instances))
        .unwrap();
    state
        .store
        .load_str(
            &format!(
                "@prefix skos: <{SKOS}> . @prefix ex: <{EX}> .\n\
                 ex:poodle skos:broader ex:dog . ex:dog skos:broader ex:mammal .\n\
                 ex:cat skos:related ex:mouse .\n\
                 ex:dog skos:prefLabel \"dog\"@en ."
            ),
            RdfFormat::Turtle,
            Some(THES),
        )
        .unwrap();
    let app = test_app(state.clone());

    let (st, v, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/thes/entailment",
        Some(&token),
        Some("application/json"),
        &json!({ "regime": "skos", "mode": "materialize" }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let graph = "urn:entailment:skos:thes";
    assert_eq!(v["graph"], graph);

    let ask = |pattern: &str| {
        matches!(
            state.store.query(&format!(
                "PREFIX skos: <{SKOS}> PREFIX ex: <{EX}> \
                 PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> \
                 ASK {{ GRAPH <{graph}> {{ {pattern} }} }}"
            )),
            Ok(oxigraph::sparql::QueryResults::Boolean(true))
        )
    };
    assert!(ask("ex:dog skos:narrower ex:poodle"), "owl:inverseOf");
    assert!(
        ask("ex:poodle skos:broaderTransitive ex:mammal"),
        "sub-property of a transitive property"
    );
    assert!(
        ask("ex:mammal skos:narrowerTransitive ex:poodle"),
        "inverse of the transitive closure"
    );
    assert!(ask("ex:mouse skos:related ex:cat"), "owl:SymmetricProperty");
    assert!(
        ask("ex:dog rdfs:label \"dog\"@en"),
        "skos:prefLabel is a sub-property of rdfs:label"
    );
    assert!(
        !ask(&format!(
            "?s ?p ?o FILTER(isIRI(?s) && STRSTARTS(STR(?s), \"{SKOS}\"))"
        )),
        "the SKOS schema's own closure is pruned from the dataset's graph"
    );

    // Queries opt in as for every other regime.
    let q = format!("SELECT ?n WHERE {{ <{EX}mammal> <{SKOS}narrowerTransitive> ?n }}");
    let (st, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={}&entailment_dataset=thes", url_encode(&q)),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(rows(&v), 2, "dog and poodle: {txt}");

    // A write re-materialises.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/store?graph={}", url_encode(THES)),
        Some(&token),
        Some("text/turtle"),
        &format!("<{EX}puppy> <{SKOS}broader> <{EX}poodle> ."),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    assert!(ask("ex:puppy skos:broaderTransitive ex:mammal"));
}

// ─── SWRL rules stored with a dataset ──────────────────────────────────────

#[cfg(feature = "swrl")]
const RULES: &str = "https://example.org/ent/rules";
#[cfg(feature = "swrl")]
const SWRL_TTL_PREFIXES: &str = "@prefix ex: <https://example.org/ent/> . \
     @prefix swrl: <http://www.w3.org/2003/11/swrl#> . ";

/// A dataset `rul` with an instances graph and an `entailment`-role graph.
#[cfg(feature = "swrl")]
fn rules_dataset(state: &open_triplestore::server::AppState, id: &str) {
    state
        .auth_db
        .create_dataset(
            id,
            "Rules",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    for (g, role) in [
        (DATA, GraphKind::Instances),
        (MODEL, GraphKind::Model),
        (RULES, GraphKind::Entailment),
    ] {
        state.auth_db.add_dataset_graph(id, g).unwrap();
        state
            .auth_db
            .set_dataset_graph_role(id, g, Some(role))
            .unwrap();
    }
}

/// `ex:Asset(?x) -> ex:Inspected(?x)`, in the SWRL RDF syntax.
#[cfg(feature = "swrl")]
fn inspected_rule() -> String {
    format!(
        "{SWRL_TTL_PREFIXES} ex:x a swrl:Variable . \
         ex:inspect a swrl:Imp ; \
           swrl:body ( [ a swrl:ClassAtom ; swrl:classPredicate ex:Asset ; swrl:argument1 ex:x ] ) ; \
           swrl:head ( [ a swrl:ClassAtom ; swrl:classPredicate ex:Inspected ; swrl:argument1 ex:x ] ) ."
    )
}

#[cfg(feature = "swrl")]
async fn assets_inspected(app: &Router, token: &str, ds: &str) -> usize {
    let q = format!("SELECT ?b WHERE {{ ?b a <{EX}Inspected> }}");
    let (st, v, txt) = req(
        app,
        Method::GET,
        &format!("/sparql?query={}&entailment_dataset={ds}", url_encode(&q)),
        Some(token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    rows(&v)
}

/// Stored rules always run (owner decision D13): a dataset with no regime
/// configured still re-runs its `swrl:Imp` rules after every write, into its
/// rules graph, rebuilt so a deleted premise drops its conclusion.
#[cfg(feature = "swrl")]
#[tokio::test]
async fn dataset_rules_rerun_after_write() {
    let (state, token) = admin_state();
    rules_dataset(&state, "rul");
    state
        .store
        .load_str(&inspected_rule(), RdfFormat::Turtle, Some(RULES))
        .unwrap();
    let app = test_app(state.clone());

    // A write to the instances graph runs the stored rule.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/store?graph={}", url_encode(DATA)),
        Some(&token),
        Some("text/turtle"),
        &format!("<{EX}b1> a <{EX}Asset> ."),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    assert_eq!(assets_inspected(&app, &token, "rul").await, 1);

    let (st, v, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/rul/entailment",
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["inference_graph"], "urn:entailment:swrl:rul", "{txt}");
    assert_eq!(v["rules"]["count"], 1, "{txt}");
    assert_eq!(v["rules"]["last_run"]["converged"], true, "{txt}");

    // Replacing the data rebuilds the conclusions.
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        &format!("/store?graph={}", url_encode(DATA)),
        Some(&token),
        Some("text/turtle"),
        &format!("<{EX}b2> a <{EX}Asset> . <{EX}b3> a <{EX}Asset> ."),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    assert_eq!(assets_inspected(&app, &token, "rul").await, 2);

    // A rule the strict reader refuses is reported, not silently skipped.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/store?graph={}", url_encode(RULES)),
        Some(&token),
        Some("text/turtle"),
        &format!(
            "{SWRL_TTL_PREFIXES} ex:broken a swrl:Imp ; \
               swrl:head ( [ a swrl:ClassAtom ; swrl:classPredicate ex:T ] ) ."
        ),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    let (_, v, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/rul/entailment",
        Some(&token),
        None,
        "",
    )
    .await;
    assert!(
        v["rules"]["error"]
            .as_str()
            .is_some_and(|e| e.contains("swrl:argument1")),
        "{txt}"
    );
}

/// Rules and the regime reach one joint fixed point: OWL 2 RL makes `b1` an
/// Asset (Bridge ⊑ Asset), the stored rule makes every Asset Inspected, and
/// RL again makes it Tracked (Inspected ⊑ Tracked). Neither alone gets there.
#[cfg(feature = "swrl")]
#[tokio::test]
async fn swrl_and_rl_reach_joint_fixpoint() {
    let (state, token) = admin_state();
    rules_dataset(&state, "joint");
    let sub = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
    state
        .store
        .load_str(
            &format!("<{EX}Bridge> <{sub}> <{EX}Asset> . <{EX}Inspected> <{sub}> <{EX}Tracked> ."),
            RdfFormat::Turtle,
            Some(MODEL),
        )
        .unwrap();
    state
        .store
        .load_str(
            &format!("<{EX}b1> a <{EX}Bridge> ."),
            RdfFormat::Turtle,
            Some(DATA),
        )
        .unwrap();
    state
        .store
        .load_str(&inspected_rule(), RdfFormat::Turtle, Some(RULES))
        .unwrap();
    let app = test_app(state.clone());

    let (st, v, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/joint/entailment",
        Some(&token),
        Some("application/json"),
        &json!({ "regime": "owl2-rl", "mode": "materialize" }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["rules"]["converged"], true, "{txt}");
    assert!(v["rules"]["rounds"].as_u64().unwrap() >= 2, "{txt}");
    let graph = "urn:entailment:owl2-rl:joint";
    assert_eq!(v["inference_graph"], graph, "{txt}");

    let q = format!("ASK {{ GRAPH <{graph}> {{ <{EX}b1> a <{EX}Tracked> }} }}");
    assert!(
        matches!(
            state.store.query(&q),
            Ok(oxigraph::sparql::QueryResults::Boolean(true))
        ),
        "regime -> rule -> regime: b1 must end up Tracked: {txt}"
    );
    // Queries opting in see the joint closure.
    assert_eq!(assets_inspected(&app, &token, "joint").await, 1);
}

/// Stored rules use built-ins and class expressions like any other rule:
/// `swrlb:add` binds a value, and a class-expression body atom
/// (`ObjectSomeValuesFrom(ex:inspectedBy ex:Engineer)`) becomes an auxiliary
/// class the dataset's OWL 2 RL regime materialises, to one joint fixed point.
/// Without a regime the class-expression rule is reported, not skipped.
#[cfg(feature = "swrl")]
#[tokio::test]
async fn dataset_rules_with_builtins_and_class_expressions() {
    let (state, token) = admin_state();
    rules_dataset(&state, "blt");
    state
        .store
        .load_str(
            &format!(
                "<{EX}b1> <{EX}age> 40 ; <{EX}inspectedBy> <{EX}eve> . \
                 <{EX}eve> a <{EX}Engineer> . <{EX}b2> <{EX}age> 3 ."
            ),
            RdfFormat::Turtle,
            Some(DATA),
        )
        .unwrap();
    let rules = format!(
        "{SWRL_TTL_PREFIXES} @prefix swrlb: <http://www.w3.org/2003/11/swrlb#> . \
         @prefix owl: <http://www.w3.org/2002/07/owl#> . \
         ex:x a swrl:Variable . ex:a a swrl:Variable . ex:n a swrl:Variable . \
         ex:next a swrl:Imp ; \
           swrl:body ( [ a swrl:DatavaluedPropertyAtom ; swrl:propertyPredicate ex:age ; \
                         swrl:argument1 ex:x ; swrl:argument2 ex:a ] \
                       [ a swrl:BuiltinAtom ; swrl:builtin swrlb:add ; \
                         swrl:arguments ( ex:n ex:a 1 ) ] ) ; \
           swrl:head ( [ a swrl:DatavaluedPropertyAtom ; swrl:propertyPredicate ex:nextAge ; \
                         swrl:argument1 ex:x ; swrl:argument2 ex:n ] ) . \
         ex:checked a swrl:Imp ; \
           swrl:body ( [ a swrl:ClassAtom ; swrl:argument1 ex:x ; \
                         swrl:classPredicate [ a owl:Restriction ; owl:onProperty ex:inspectedBy ; \
                                               owl:someValuesFrom ex:Engineer ] ] ) ; \
           swrl:head ( [ a swrl:ClassAtom ; swrl:classPredicate ex:Inspected ; swrl:argument1 ex:x ] ) ."
    );
    state
        .store
        .load_str(&rules, RdfFormat::Turtle, Some(RULES))
        .unwrap();
    let app = test_app(state.clone());

    let (st, v, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/blt/entailment",
        Some(&token),
        Some("application/json"),
        &json!({ "regime": "owl2-rl", "mode": "materialize" }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["rules"]["converged"], true, "{txt}");
    let graph = "urn:entailment:owl2-rl:blt";
    let holds = |q: String| {
        matches!(
            state.store.query(&q),
            Ok(oxigraph::sparql::QueryResults::Boolean(true))
        )
    };
    assert!(
        holds(format!(
            "ASK {{ GRAPH <{graph}> {{ <{EX}b1> <{EX}nextAge> ?n FILTER(?n = 41) }} }}"
        )),
        "swrlb:add binds ?n: {txt}"
    );
    assert!(
        holds(format!(
            "ASK {{ GRAPH <{graph}> {{ <{EX}b1> a <{EX}Inspected> }} }}"
        )),
        "b1 is inspected by an Engineer, which RL derives through the auxiliary class: {txt}"
    );
    assert!(
        !holds(format!(
            "ASK {{ GRAPH <{graph}> {{ <{EX}b2> a <{EX}Inspected> }} }}"
        )),
        "{txt}"
    );

    // With the regime off, the class-expression rule cannot run: reported.
    let (st, v, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/blt/entailment",
        Some(&token),
        Some("application/json"),
        &json!({ "regime": "owl2-rl", "mode": "off" }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(
        v["rules"]["error"]
            .as_str()
            .is_some_and(|e| e.contains("regime")),
        "{txt}"
    );
}
