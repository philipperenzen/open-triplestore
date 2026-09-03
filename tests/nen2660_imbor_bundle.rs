//! The LD-BIM Stage-1 benchmark on real national-standard data: NEN 2660-2
//! and the IMBOR 2025 object-type library, shipped as a seed bundle
//! (examples/seed-bundles/nen2660-imbor), a sample asset dataset classified
//! against the model layer and validated through the bound shapes — through
//! the real API, exactly like the domain-neutral layered-reference bundle.
//!
//! The RDF is not vendored (run the bundle's fetch.sh once). Without it this
//! test reports that it skipped and passes, so CI stays green; with it, the
//! benchmark runs on the real data.

mod common;

use std::path::Path;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::seed_bundles::load_seed_dir;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const KERN: &str = "https://data.crow.nl/imbor/def/";
const INSTANCES: &str = "https://example.org/imbor-sample/instances";
const BOOM: &str = "https://data.crow.nl/imbor/def/83a942f7-5291-42f0-afb1-9a57d0fb2f15";
const VEGETATIEOBJECT: &str = "https://data.crow.nl/imbor/def/761406d1-87bc-4dc1-b1b7-bd3bb7ab54a7";

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
async fn nen2660_imbor_bundle_classifies_and_validates_real_data() {
    let bundles = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/seed-bundles");
    let bundle = bundles.join("nen2660-imbor");
    for needed in ["nen2660-rdfs.ttl", "imbor-release/imbor2025-kern.ttl"] {
        if !bundle.join(needed).exists() {
            eprintln!(
                "SKIP: {} is not present — run examples/seed-bundles/nen2660-imbor/fetch.sh to run the real-data benchmark",
                needed
            );
            return;
        }
    }

    let (state, token) = admin_state();
    load_seed_dir(&state, &bundles);
    let app = test_app(state.clone());

    // 1. The model layer landed: thousands of IMBOR object types, the NEN 2660-2 model.
    let kern = state.store.graph_count_cached(Some(KERN)).unwrap_or(0);
    assert!(kern > 50_000, "IMBOR Kern loaded ({kern} triples)");
    let nen = state
        .store
        .graph_count_cached(Some("https://w3id.org/nen2660/rdfs/def"))
        .unwrap_or(0);
    assert!(nen > 200, "NEN 2660-2 RDFS loaded ({nen} triples)");

    // 2. The sample dataset conforms to the IMBOR model version and binds Kern as its shapes.
    let (st, layer, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/imbor-sample-assets/conformance",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(layer["conforms_to_model"]["id"], "imbor-otl", "{txt}");
    assert_eq!(layer["conforms_to_model"]["version"], "2025", "{txt}");
    let shapes: Vec<&str> = layer["validation_shapes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(shapes, vec![KERN], "{txt}");
    let sources: Vec<&str> = layer["reasoning_sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        sources.contains(&INSTANCES) && sources.contains(&KERN),
        "{txt}"
    );

    // 3. Classification: every Boom is a Vegetatieobject through IMBOR's subClassOf.
    let (st, report, txt) = req(
        &app,
        Method::POST,
        "/api/reasoning/materialize",
        &token,
        json!({ "regime": "rdfs", "dataset": "imbor-sample-assets" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let tg = report["target_graph"].as_str().unwrap().to_string();
    let ask = |q: &str| matches!(state.store.query(q), Ok(QueryResults::Boolean(true)));
    for t in ["boom-1", "boom-2", "boom-3"] {
        assert!(
            ask(&format!("ASK {{ GRAPH <{tg}> {{ <https://example.org/imbor-sample/{t}> a <{VEGETATIEOBJECT}> }} }}")),
            "{t} is a Vegetatieobject via the model layer: {txt}"
        );
    }
    assert!(ask(&format!(
        "ASK {{ GRAPH <{INSTANCES}> {{ ?t a <{BOOM}> }} }}"
    )));

    // 4. Validation through the bound IMBOR shapes finds the planted violation only.
    let (st, vreport, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/imbor-sample-assets/validate",
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
    assert_eq!(
        report["conforms"], false,
        "boom-3 violates a Kern property shape: {txt}"
    );
    assert!(
        txt.contains("imbor-sample/boom-3"),
        "the report names the offending tree: {txt}"
    );
    assert!(
        !txt.contains("imbor-sample/boom-1"),
        "boom-1 conforms: {txt}"
    );

    // 5. The catalogue advertises the conformance to the IMBOR model version.
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
    assert!(void.contains("data-model/imbor-otl/version/2025"), "{void}");
}
