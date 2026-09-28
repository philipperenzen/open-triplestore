//! Domain neutrality, proven (7.4): the clinical-reference bundle — a
//! FHIR-shaped record model in a domain the platform knows nothing about —
//! loads through the same manifest engine, resolves its conformance layer,
//! classifies through its model and validates through its shapes exactly as
//! the asset bundles do.

mod common;

use std::path::Path;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::seed_bundles::load_seed_dir;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const EX: &str = "https://example.org/clinical/def#";
const INSTANCES: &str = "https://example.org/clinical/instances";
const SHAPES: &str = "https://example.org/clinical/shapes";

#[tokio::test]
async fn clinical_bundle_classifies_and_validates_like_any_other() {
    let (state, token) = admin_state();
    load_seed_dir(
        &state,
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/seed-bundles"),
    );
    let app = test_app(state.clone());
    let get = |uri: String| {
        let app = app.clone();
        let token = token.clone();
        async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(uri)
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let st = resp.status();
            let txt = body_text(resp.into_body()).await;
            (
                st,
                serde_json::from_str::<Value>(&txt).unwrap_or(Value::Null),
                txt,
            )
        }
    };

    let (st, layer, txt) = get("/api/datasets/clinical-records/conformance".to_string()).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(
        layer["conforms_to_model"]["id"], "clinical-record-model",
        "{txt}"
    );
    assert_eq!(layer["validation_shapes"], json!([SHAPES]), "{txt}");
    let sources: Vec<&str> = layer["reasoning_sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        sources.contains(&INSTANCES) && sources.contains(&"https://example.org/clinical/model"),
        "{txt}"
    );

    // Classification: every Observation is a Resource through the model.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/reasoning/materialize")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "regime": "rdfs", "dataset": "clinical-records" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let report: Value = serde_json::from_str(&body_text(resp.into_body()).await).unwrap();
    let tg = report["target_graph"].as_str().unwrap().to_string();
    let ask = |q: &str| matches!(state.store.query(q), Ok(QueryResults::Boolean(true)));
    assert!(ask(&format!(
        "ASK {{ GRAPH <{tg}> {{ <https://example.org/clinical/record/obs-1> a <{EX}Resource> }} }}"
    )));

    // Validation: exactly the observation without a value.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/datasets/clinical-records/validate")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let txt = body_text(resp.into_body()).await;
    let v: Value = serde_json::from_str(&txt).unwrap();
    let r = if v["report"].is_object() {
        &v["report"]
    } else {
        &v
    };
    assert_eq!(r["conforms"], false, "{txt}");
    assert_eq!(r["results_count"], 1, "{txt}");
    assert!(
        txt.contains("record/obs-3") && !txt.contains("record/obs-1"),
        "{txt}"
    );
}
