//! The commit log claims to record "every data mutation", and
//! `GET /api/datasets/:id/commits` presents it as the dataset's history — yet
//! Graph Store writes, bulk imports and every dataset-version operation left
//! no trace. These tests pin that each of them does now.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const G: &str = "urn:trail:g1";

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

async fn raw(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    content_type: &str,
    body: &str,
) -> StatusCode {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

async fn dataset(app: &Router, token: &str, name: &str) -> String {
    let (st, v, txt) = req(
        app,
        Method::POST,
        "/api/datasets",
        token,
        json!({ "name": name, "owner_type": "user", "owner_id": "adm", "visibility": "private" }),
    )
    .await;
    assert!(st.is_success(), "create dataset: {st} {txt}");
    let id = v["id"].as_str().unwrap().to_string();
    let (st, _, txt) = req(
        app,
        Method::POST,
        &format!("/api/datasets/{id}/graphs"),
        token,
        json!({ "graph_iri": G }),
    )
    .await;
    assert!(st.is_success(), "register graph: {st} {txt}");
    id
}

async fn history(app: &Router, token: &str, ds: &str) -> Vec<Value> {
    let (st, v, txt) = req(
        app,
        Method::GET,
        &format!("/api/datasets/{ds}/commits"),
        token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    v.as_array()
        .cloned()
        .or_else(|| v["commits"].as_array().cloned())
        .unwrap_or_default()
}

fn of_kind<'a>(trail: &'a [Value], kind: &str) -> Vec<&'a Value> {
    trail.iter().filter(|c| c["kind"] == kind).collect()
}

#[tokio::test]
async fn graph_store_writes_are_in_the_dataset_history() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let ds = dataset(&app, &token, "trail").await;
    let uri = format!("/store?graph={}", url_encode(G));

    let st = raw(
        &app,
        Method::PUT,
        &uri,
        &token,
        "text/turtle",
        "<urn:a> <urn:p> \"1\" . <urn:b> <urn:p> \"2\" .",
    )
    .await;
    assert!(st.is_success(), "PUT: {st}");
    let after_put = history(&app, &token, &ds).await;
    let put = of_kind(&after_put, "graph-store");
    assert_eq!(put.len(), 1, "one Graph Store commit after PUT");
    assert!(
        put[0]["message"].as_str().unwrap().contains("PUT"),
        "{}",
        put[0]
    );
    assert!(put[0]["affected_graphs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|g| g == G));
    assert_eq!(put[0]["added"], 2, "{}", put[0]);

    let st = raw(
        &app,
        Method::POST,
        &uri,
        &token,
        "text/turtle",
        "<urn:c> <urn:p> \"3\" .",
    )
    .await;
    assert!(st.is_success(), "POST: {st}");
    let st = raw(&app, Method::DELETE, &uri, &token, "text/turtle", "").await;
    assert!(st.is_success(), "DELETE: {st}");
    let trail = history(&app, &token, &ds).await;
    let gsp = of_kind(&trail, "graph-store");
    assert_eq!(
        gsp.len(),
        3,
        "PUT, POST and DELETE each leave a commit: {trail:?}"
    );
    let delete = gsp
        .iter()
        .find(|c| c["message"].as_str().unwrap().contains("DELETE"))
        .unwrap();
    assert_eq!(
        delete["removed"], 3,
        "the delete records what it removed: {delete}"
    );
}

#[tokio::test]
async fn version_operations_are_in_the_dataset_history() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let ds = dataset(&app, &token, "trail-versions").await;
    raw(
        &app,
        Method::PUT,
        &format!("/store?graph={}", url_encode(G)),
        &token,
        "text/turtle",
        "<urn:a> <urn:p> \"1\" .",
    )
    .await;

    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions"),
        &token,
        json!({ "version": "1.0.0" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions/1.0.0/publish"),
        &token,
        Value::Null,
    )
    .await;
    req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions/1.0.0/restore"),
        &token,
        Value::Null,
    )
    .await;

    let trail = history(&app, &token, &ds).await;
    let versions = of_kind(&trail, "dataset");
    let messages: Vec<&str> = versions
        .iter()
        .map(|c| c["message"].as_str().unwrap())
        .collect();
    for expected in [
        "Cut version 1.0.0",
        "Published version 1.0.0",
        "Restored version 1.0.0",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(expected)),
            "missing '{expected}' in {messages:?}"
        );
    }
    assert!(
        versions.iter().all(|c| c["version"] == "1.0.0"),
        "each carries the version: {versions:?}"
    );
}

#[tokio::test]
async fn bulk_imports_are_in_the_dataset_history() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let ds = dataset(&app, &token, "trail-import").await;
    let boundary = "ots-trail-boundary";
    let ttl = "<urn:imp:a> <urn:imp:p> \"1\" .\n<urn:imp:b> <urn:imp:p> \"2\" .\n";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.ttl\"\r\nContent-Type: text/turtle\r\n\r\n{ttl}\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"dataset_id\"\r\n\r\n{ds}\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"meta\"\r\n\r\n{{\"dataset_id\": \"{ds}\", \"default_target_graph\": \"{g}\"}}\r\n--{b}--\r\n",
        b = boundary,
        g = G
    );
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/import/bulk")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    let txt = body_text(resp.into_body()).await;
    assert!(st.is_success(), "import: {st} {txt}");
    let outcome: Value = serde_json::from_str(&txt).unwrap_or(Value::Null);
    assert_eq!(
        outcome["success"], true,
        "the file must actually import: {txt}"
    );

    // Written at all? (distinguishes "not recorded" from "not listed")
    let written = matches!(
        state.store.query(
            "ASK { GRAPH <urn:system:commit-log> { ?c <urn:system:vocab/kind> \"import\" } }"
        ),
        Ok(oxigraph::sparql::QueryResults::Boolean(true))
    );
    assert!(
        written,
        "no import commit was written to the commit graph at all: {txt}"
    );
    let trail = history(&app, &token, &ds).await;
    let imports = of_kind(&trail, "import");
    assert_eq!(
        imports.len(),
        1,
        "the import is one commit: trail={trail:?} import response={txt}"
    );
    assert!(
        imports[0]["added"].as_u64().unwrap_or(0) >= 2,
        "it counts what it loaded: {}",
        imports[0]
    );
    assert!(
        !imports[0]["affected_graphs"].as_array().unwrap().is_empty(),
        "it names the graphs it filled"
    );
}
