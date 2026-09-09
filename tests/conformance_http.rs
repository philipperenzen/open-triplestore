//! TBox/ABox separation over HTTP (5.2): a dataset declares the model version
//! it conforms to, `GET …/conformance` resolves the layer, and
//! `POST /api/reasoning/materialize` reasons over exactly that layer.
//!
//! Two defects this pins: `source_graphs` was parsed and ignored, and the
//! rules read only the unnamed default graph — so a dataset's named graphs
//! were invisible to materialisation however the endpoint was called.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::registry as dmr;
use open_triplestore::server::AppState;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const EX: &str = "http://ex.org/";
const G1: &str = "urn:conf:d1:instances";
const G2: &str = "urn:conf:d2:instances";

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = if body.is_null() {
        Body::empty()
    } else {
        b = b.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

fn ask(state: &AppState, q: &str) -> bool {
    matches!(state.store.query(q), Ok(QueryResults::Boolean(true)))
}

/// A published model version `m1@1.0` whose graph says Bridge ⊑ Structure.
fn model_layer(state: &AppState) -> String {
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "m1",
        "Structures model",
        &format!("{EX}def#"),
        None,
        true,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    let graph_iri = format!("{base}/data-model/m1/version/1.0");
    dmr::insert_version(
        &state.store,
        &base,
        &DataModelVersion {
            data_model_id: "m1".to_string(),
            version: "1.0".to_string(),
            status: VersionStatus::Published,
            graph_iri: graph_iri.clone(),
            sub_graphs: vec![],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: vec![],
        },
    )
    .unwrap();
    dmr::update_latest_published(&state.store, &base, "m1", "1.0").unwrap();
    state
        .store
        .load_str(
            &format!(
                "<{EX}Bridge> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <{EX}Structure> ."
            ),
            RdfFormat::Turtle,
            Some(&graph_iri),
        )
        .unwrap();
    graph_iri
}

async fn dataset(
    app: &Router,
    token: &str,
    name: &str,
    graph: &str,
    ttl: &str,
    state: &AppState,
) -> String {
    let (st, v, txt) = req(
        app,
        Method::POST,
        "/api/datasets",
        token,
        json!({ "name": name, "owner_type": "user", "owner_id": "adm", "visibility": "private", "graph_role": "instances" }),
    )
    .await;
    assert!(st.is_success(), "create {name}: {st} {txt}");
    let id = v["id"].as_str().unwrap().to_string();
    let (st, _, txt) = req(
        app,
        Method::POST,
        &format!("/api/datasets/{id}/graphs"),
        token,
        json!({ "graph_iri": graph }),
    )
    .await;
    assert!(st.is_success(), "register graph: {st} {txt}");
    state
        .store
        .load_str(ttl, RdfFormat::Turtle, Some(graph))
        .unwrap();
    id
}

#[tokio::test]
async fn materialisation_reasons_over_the_declared_layer_only() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let model_graph = model_layer(&state);
    let d1 = dataset(
        &app,
        &token,
        "d1",
        G1,
        &format!("<{EX}b1> a <{EX}Bridge> ."),
        &state,
    )
    .await;
    let _d2 = dataset(
        &app,
        &token,
        "d2",
        G2,
        &format!("<{EX}x> a <{EX}Bridge> ."),
        &state,
    )
    .await;

    // d1 conforms to m1@1.0 (declared through the ordinary dataset update).
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{d1}"),
        &token,
        json!({ "name": "d1", "visibility": "private", "conforms_to_model": "m1", "conforms_to_version": "1.0" }),
    )
    .await;
    assert!(st.is_success(), "declare conformance: {st} {txt}");

    // The layer resolves to d1's instance graph plus the model version's graph.
    let (st, layer, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{d1}/conformance"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(layer["conforms_to_model"]["id"], "m1");
    assert_eq!(layer["conforms_to_model"]["version"], "1.0");
    assert_eq!(layer["conforms_to_model"]["graph_iri"], model_graph);
    let sources: Vec<&str> = layer["reasoning_sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        sources.contains(&G1) && sources.contains(&model_graph.as_str()),
        "{txt}"
    );
    assert!(
        !sources.contains(&G2),
        "another dataset's graph is not in this layer: {txt}"
    );
    assert_eq!(layer["dataset_role"], "instances");

    // Materialise RDFS over that layer: b1 becomes a Structure, x (d2) does not.
    let (st, report, txt) = req(
        &app,
        Method::POST,
        "/api/reasoning/materialize",
        &token,
        json!({ "regime": "rdfs", "dataset": d1 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(report["regime"], "rdfs");
    let tg = report["target_graph"]
        .as_str()
        .expect("target graph in report")
        .to_string();
    assert!(
        ask(
            &state,
            &format!("ASK {{ GRAPH <{tg}> {{ <{EX}b1> a <{EX}Structure> }} }}")
        ),
        "the conformed model's subclass axiom applies to d1's instance: {txt}"
    );
    assert!(
        !ask(
            &state,
            &format!("ASK {{ GRAPH <{tg}> {{ <{EX}x> a <{EX}Structure> }} }}")
        ),
        "d2 is outside the layer and must not be reasoned over"
    );
}

/// Explicit `source_graphs` are honoured (they used to be ignored), and each
/// must be readable by the caller.
#[tokio::test]
async fn explicit_source_graphs_are_honoured_and_access_checked() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let model_graph = model_layer(&state);
    let _d2 = dataset(
        &app,
        &token,
        "d2",
        G2,
        &format!("<{EX}x> a <{EX}Bridge> ."),
        &state,
    )
    .await;

    let (st, report, txt) = req(
        &app,
        Method::POST,
        "/api/reasoning/materialize",
        &token,
        json!({ "regime": "rdfs", "source_graphs": [G2, model_graph] }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let tg = report["target_graph"].as_str().unwrap().to_string();
    assert!(
        ask(
            &state,
            &format!("ASK {{ GRAPH <{tg}> {{ <{EX}x> a <{EX}Structure> }} }}")
        ),
        "{txt}"
    );

    // A plain user with no grant on d2's graph cannot use it as a premise.
    state
        .auth_db
        .create_user("mallory", "mallory", "m@t.com", "hash", SystemRole::User)
        .unwrap();
    let mallory = mint_token("mallory", "mallory", "user");
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/reasoning/materialize",
        &mallory,
        json!({ "regime": "rdfs", "source_graphs": [G2] }),
    )
    .await;
    assert!(
        st == StatusCode::FORBIDDEN
            || st == StatusCode::NOT_FOUND
            || st == StatusCode::UNAUTHORIZED,
        "an unreadable premise graph is refused: {st} {txt}"
    );
}

#[tokio::test]
async fn conformance_reports_an_unresolvable_model_instead_of_hiding_it() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let d1 = dataset(
        &app,
        &token,
        "d1",
        G1,
        &format!("<{EX}b1> a <{EX}Bridge> ."),
        &state,
    )
    .await;
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{d1}"),
        &token,
        json!({ "name": "d1", "visibility": "private", "conforms_to_model": "nope", "conforms_to_version": "9" }),
    )
    .await;
    assert!(st.is_success(), "{txt}");
    let (st, layer, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{d1}/conformance"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(layer["conforms_to_model"].is_null());
    assert_eq!(layer["unresolved_model"], "nope@9", "{txt}");
    let sources: Vec<&str> = layer["reasoning_sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(sources, vec![G1], "only the dataset's own graphs remain");
}
