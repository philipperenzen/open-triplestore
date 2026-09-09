//! The LD-BIM Stage-1 benchmark, as a mechanism: load a layered seed bundle
//! (reference model + instance dataset that conforms to it + bound shapes +
//! the other role graphs), then classify the instances against the model layer
//! and validate them with SHACL — all through the real HTTP API.
//!
//! The bundle is a domain-neutral toy (an asset registry); the NEN 2660-2 /
//! IMBOR bundle in examples/seed-bundles/nen2660-imbor rides the same
//! mechanism with the national standards as payload.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::seed_bundles::load_seed_dir;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use std::path::Path;
use tower::ServiceExt as _;

const MODEL_GRAPH: &str = "https://example.org/layered/model";
const INSTANCES: &str = "https://example.org/layered/instances";
const SHAPES: &str = "https://example.org/layered/shapes";
const EX: &str = "https://example.org/layered/def#";

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

#[tokio::test]
async fn layered_bundle_classifies_and_validates_end_to_end() {
    let (state, token) = admin_state();
    load_seed_dir(
        &state,
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/seed-bundles"),
    );
    let app = test_app(state.clone());

    // 1. Every layer landed with its declared role.
    let (st, graphs, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/assets/graphs",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let entries = graphs
        .as_array()
        .cloned()
        .or_else(|| graphs["graphs"].as_array().cloned())
        .unwrap_or_default();
    let role_of = |iri: &str| {
        entries
            .iter()
            .find(|e| e["graph_iri"] == iri)
            .map(|e| e["graph_role"].clone())
            .unwrap_or(Value::Null)
    };
    assert_eq!(role_of(INSTANCES), "instances", "{txt}");
    assert_eq!(role_of(SHAPES), "shapes");
    assert_eq!(role_of("https://example.org/layered/linkset"), "linkset");
    assert_eq!(
        role_of("https://example.org/layered/provenance"),
        "provenance"
    );
    assert_eq!(role_of("https://example.org/layered/catalog"), "catalog");

    // 2. The conformance layer resolves the shipped model version and shapes.
    let (st, layer, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/assets/conformance",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(layer["conforms_to_model"]["id"], "asset-model", "{txt}");
    assert_eq!(layer["conforms_to_model"]["version"], "1.0.0");
    assert_eq!(layer["conforms_to_model"]["graph_iri"], MODEL_GRAPH);
    let sources: Vec<&str> = layer["reasoning_sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        sources.contains(&INSTANCES) && sources.contains(&MODEL_GRAPH),
        "{txt}"
    );
    assert!(
        sources.contains(&"https://example.org/layered/vocabulary")
            && sources.contains(&"https://example.org/layered/domain-values"),
        "the model version's sub-graphs are premises too: {txt}"
    );
    assert!(
        !sources.contains(&SHAPES) && !sources.contains(&"https://example.org/layered/provenance"),
        "shapes and provenance are not premises: {txt}"
    );
    let shapes: Vec<&str> = layer["validation_shapes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(shapes, vec![SHAPES], "the bundle bound its shapes: {txt}");

    // 3. Classification against the model layer: every Bridge is an Asset.
    let (st, report, txt) = req(
        &app,
        Method::POST,
        "/api/reasoning/materialize",
        &token,
        json!({ "regime": "rdfs", "dataset": "assets" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let tg = report["target_graph"].as_str().unwrap().to_string();
    let ask = |q: &str| matches!(state.store.query(q), Ok(QueryResults::Boolean(true)));
    for b in ["b1", "b2", "b3"] {
        assert!(
            ask(&format!("ASK {{ GRAPH <{tg}> {{ <https://example.org/layered/asset/{b}> a <{EX}Asset> }} }}")),
            "{b} is classified as an Asset through the model's subClassOf: {txt}"
        );
    }

    // 4. SHACL validation through the bound shapes finds the planted violation.
    let (st, vreport, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/assets/validate",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let report = if vreport["report"].is_object() {
        &vreport["report"]
    } else {
        &vreport
    };
    assert_eq!(report["conforms"], false, "b3 has no status: {txt}");
    assert_eq!(
        report["results_count"], 1,
        "exactly the planted violation: {txt}"
    );
    assert!(
        txt.contains("asset/b3"),
        "the report names the offending bridge: {txt}"
    );
    assert!(
        !txt.contains("asset/b1\"")
            || txt.matches("asset/b1").count() <= txt.matches("asset/b3").count(),
        "b1 conforms"
    );

    // 5. The catalogue advertises the conformance.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/.well-known/void")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let void = body_text(resp.into_body()).await;
    assert!(
        void.contains("data-model/asset-model/version/1.0.0"),
        "dct:conformsTo points at the model version:\n{void}"
    );
}
