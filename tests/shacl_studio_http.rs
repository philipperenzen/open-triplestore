//! SHACL Studio over HTTP: the shape-graph lifecycle, pipelines and bindings.
//!
//! `src/shacl_studio/{handlers,store,exec}` (~120 KB) shipped with no test that
//! drove a request through them — only the cross-tenant *denials* in
//! `security_shacl_studio.rs` were covered, so a pipeline run that validated
//! nothing, or a revision history that recorded nothing, would have gone
//! unnoticed. These tests assert the positive path end to end: what a run
//! reports must follow from the data it was pointed at.
mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::server::AppState;
use oxigraph::io::RdfFormat;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const DATA_GRAPH: &str = "urn:studio:data";

const SHAPES: &str = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .
ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:name ; sh:minCount 1 ] .
"#;

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    content_type: &str,
    body: &str,
) -> (StatusCode, Value, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

async fn json_req(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value, String) {
    send(
        app,
        method,
        uri,
        token,
        "application/json",
        &body.to_string(),
    )
    .await
}

fn load(state: &AppState, turtle: &str) {
    state
        .store
        .load_str(turtle, RdfFormat::Turtle, Some(DATA_GRAPH))
        .unwrap();
}

async fn create_shape_graph(app: &Router, token: &str, turtle: &str) -> String {
    let (st, v, txt) = json_req(
        app,
        Method::POST,
        "/api/shacl/shape-graphs",
        token,
        json!({ "name": "people", "visibility": "private", "turtle": turtle }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create shape graph: {txt}");
    v["id"].as_str().expect("shape graph id").to_string()
}

/// Either a bare array or an object wrapping one under a conventional key.
fn items(v: &Value) -> Vec<Value> {
    if let Some(a) = v.as_array() {
        return a.clone();
    }
    for key in [
        "items",
        "runs",
        "revisions",
        "bindings",
        "pipelines",
        "shape_graphs",
        "targets",
    ] {
        if let Some(a) = v[key].as_array() {
            return a.clone();
        }
    }
    vec![]
}

// ─── Shape graph lifecycle ───────────────────────────────────────────────────

#[tokio::test]
async fn shape_graph_create_read_update_delete_round_trip() {
    let (state, token) = admin_state();
    let app = test_app(state);

    let id = create_shape_graph(&app, &token, SHAPES).await;

    let (st, v, txt) = json_req(
        &app,
        Method::GET,
        &format!("/api/shacl/shape-graphs/{id}"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["name"], "people");

    // The Turtle we posted is what comes back.
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/shacl/shape-graphs/{id}/turtle"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::ACCEPT, "text/turtle")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ttl = body_text(resp.into_body()).await;
    assert!(
        ttl.contains("PersonShape") && ttl.contains("minCount"),
        "{ttl}"
    );

    // Replace the content; a revision is recorded.
    let v2 = SHAPES.replace("sh:minCount 1", "sh:minCount 1 ; sh:maxCount 1");
    let (st, _, txt) = send(
        &app,
        Method::PUT,
        &format!("/api/shacl/shape-graphs/{id}/turtle"),
        &token,
        "text/turtle",
        &v2,
    )
    .await;
    assert!(st.is_success(), "PUT turtle: {st} {txt}");
    let (st, v, txt) = json_req(
        &app,
        Method::GET,
        &format!("/api/shacl/shape-graphs/{id}/revisions"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(
        !items(&v).is_empty(),
        "an update must leave a revision behind: {txt}"
    );

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/shacl/shape-graphs/{id}/turtle"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let ttl = body_text(app.clone().oneshot(req).await.unwrap().into_body()).await;
    assert!(ttl.contains("maxCount"), "the update is live: {ttl}");

    // Delete, then it is gone.
    let (st, _, txt) = json_req(
        &app,
        Method::DELETE,
        &format!("/api/shacl/shape-graphs/{id}"),
        &token,
        Value::Null,
    )
    .await;
    assert!(st.is_success(), "delete: {st} {txt}");
    let (st, _, _) = json_req(
        &app,
        Method::GET,
        &format!("/api/shacl/shape-graphs/{id}"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

// ─── Pipelines ───────────────────────────────────────────────────────────────

/// A pipeline run's verdict must follow from the data: violating data yields a
/// non-conforming run with a counted violation; fixing the data flips it.
#[tokio::test]
async fn pipeline_run_reports_what_the_data_warrants() {
    let (state, token) = admin_state();
    load(
        &state,
        "<http://example.org/bob> a <http://example.org/Person> .",
    );
    let app = test_app(state.clone());
    let sg = create_shape_graph(&app, &token, SHAPES).await;

    let (st, v, txt) = json_req(
        &app,
        Method::POST,
        "/api/shacl/pipelines",
        &token,
        json!({
            "name": "people-check",
            "targets": [{ "kind": "graph", "id": DATA_GRAPH }],
            "shape_graph_ids": [sg],
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create pipeline: {txt}");
    let pid = v["id"].as_str().expect("pipeline id").to_string();

    let (st, run, txt) = json_req(
        &app,
        Method::POST,
        &format!("/api/shacl/pipelines/{pid}/run"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "run: {txt}");
    assert_eq!(run["conforms"], false, "bob has no name: {txt}");
    assert!(
        run["violation_count"].as_i64().unwrap_or(0) >= 1,
        "the missing name is a counted violation: {txt}"
    );

    load(
        &state,
        "<http://example.org/bob> <http://example.org/name> \"Bob\" .",
    );
    let (st, run2, txt) = json_req(
        &app,
        Method::POST,
        &format!("/api/shacl/pipelines/{pid}/run"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(run2["conforms"], true, "with a name bob conforms: {txt}");

    // Both runs are in the history.
    let (st, v, txt) = json_req(
        &app,
        Method::GET,
        &format!("/api/shacl/pipelines/{pid}/runs"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(items(&v).len() >= 2, "two runs recorded: {txt}");
}

// ─── Bindings ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn binding_a_shape_graph_to_a_graph_is_listed_for_that_target() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let sg = create_shape_graph(&app, &token, SHAPES).await;

    let (st, _, txt) = json_req(
        &app,
        Method::POST,
        "/api/shacl/bindings",
        &token,
        json!({ "target": { "kind": "graph", "id": DATA_GRAPH }, "shape_graph_id": sg }),
    )
    .await;
    assert!(st.is_success(), "bind: {st} {txt}");

    let (st, v, txt) = json_req(
        &app,
        Method::GET,
        &format!(
            "/api/shacl/bindings?target_kind=graph&target_id={}",
            url_encode(DATA_GRAPH)
        ),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let bound: Vec<String> = items(&v)
        .iter()
        .filter_map(|b| {
            b["shape_graph_id"]
                .as_str()
                .or_else(|| b["shape_graph"]["id"].as_str())
                .or_else(|| b["id"].as_str())
                .map(str::to_string)
        })
        .collect();
    assert!(
        bound.contains(&sg),
        "the binding lists the shape graph: {txt}"
    );

    // Reverse lookup works too.
    let (st, v, txt) = json_req(
        &app,
        Method::GET,
        &format!("/api/shacl/bindings?shape_graph_id={sg}"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(
        txt.contains(DATA_GRAPH),
        "the shape graph's bindings name the target: {}",
        items(&v).len()
    );
}
