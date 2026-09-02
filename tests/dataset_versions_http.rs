//! Dataset versioning over HTTP: create → list → data → publish → restore →
//! branch, plus the ownership gate.
//!
//! `src/dataset_versions/{registry,handlers}` shipped with no test that drove a
//! request through them. A snapshot that captured nothing, a restore that
//! restored nothing, or a publish that anyone could perform would all have gone
//! unnoticed. These tests pin the observable contract: a version's data is what
//! the graph held when it was cut, and restore brings the graph back to it.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::server::AppState;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const G: &str = "urn:vds:g1";

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

/// A private dataset owned by the admin, with `G` registered and holding two triples.
async fn dataset_with_graph(state: &AppState, app: &Router, token: &str) -> String {
    let (st, v, txt) = req(
        app,
        Method::POST,
        "/api/datasets",
        token,
        json!({ "name": "vds", "owner_type": "user", "owner_id": "adm", "visibility": "private" }),
    )
    .await;
    assert!(st.is_success(), "create dataset: {st} {txt}");
    let id = v["id"].as_str().expect("dataset id").to_string();
    let (st, _, txt) = req(
        app,
        Method::POST,
        &format!("/api/datasets/{id}/graphs"),
        token,
        json!({ "graph_iri": G }),
    )
    .await;
    assert!(st.is_success(), "register graph: {st} {txt}");
    state
        .store
        .load_str(
            "<urn:vds:a> <urn:vds:p> \"one\" . <urn:vds:b> <urn:vds:p> \"two\" .",
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
    id
}

fn count(state: &AppState, graph: &str) -> u64 {
    match state.store.query(&format!(
        "SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}"
    )) {
        Ok(QueryResults::Solutions(mut s)) => s
            .next()
            .and_then(|r| r.ok())
            .and_then(|r| r.get("c").map(|t| t.to_string()))
            .and_then(|t| t.trim_start_matches('"').split('"').next()?.parse().ok())
            .unwrap_or(0),
        _ => 0,
    }
}

#[tokio::test]
async fn version_lifecycle_create_list_publish_restore_branch() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let ds = dataset_with_graph(&state, &app, &token).await;

    // Cut a version: the record names what it snapshotted.
    let (st, v, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions"),
        &token,
        json!({ "version": "1.0.0", "notes": "first cut" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create version: {txt}");
    assert_eq!(v["version"], "1.0.0");
    assert!(
        !v["snapshot_graphs"]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(true),
        "a version snapshots the dataset's graphs: {txt}"
    );
    let snapshot = v["snapshot_graphs"][0].as_str().unwrap().to_string();
    assert_eq!(
        count(&state, &snapshot),
        2,
        "the snapshot holds the graph's triples"
    );

    let (st, v, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/versions"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(
        v.as_array().map(Vec::len),
        Some(1),
        "one version listed: {txt}"
    );

    // The version's data is served, and it is the data that was cut.
    let (st, _, data) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/versions/1.0.0/data"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{data}");
    assert!(
        data.contains("urn:vds:a") && data.contains("urn:vds:b"),
        "{data}"
    );

    // Publish.
    let (st, v, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions/1.0.0/publish"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["status"], "published");
    let (_, v, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/versions/1.0.0"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(v["status"], "published", "the status is persisted: {v}");

    // The live graph drifts; restore brings it back to the snapshot.
    state
        .store
        .load_str(
            "<urn:vds:c> <urn:vds:p> \"three\" .",
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
    assert_eq!(count(&state, G), 3);
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions/1.0.0/restore"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "restore: {txt}");
    assert_eq!(
        count(&state, G),
        2,
        "restore replaces the live graph with the snapshot"
    );
    assert!(
        !matches!(
            state
                .store
                .query(&format!("ASK {{ GRAPH <{G}> {{ <urn:vds:c> ?p ?o }} }}")),
            Ok(QueryResults::Boolean(true))
        ),
        "the drifted triple is gone after restore"
    );

    // Branch off the published version.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/branches"),
        &token,
        json!({ "branch": "b1", "from_version": "1.0.0" }),
    )
    .await;
    assert!(st.is_success(), "create branch: {st} {txt}");
    let (st, _, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/branches"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(txt.contains("b1"), "the branch is listed: {txt}");
}

/// Cutting or publishing a version is a write on the dataset: a user with no
/// grant on it must be refused, not silently snapshot another tenant's data.
#[tokio::test]
async fn versions_require_write_on_the_dataset() {
    let (state, admin) = admin_state();
    state
        .auth_db
        .create_user("mallory", "mallory", "m@t.com", "hash", SystemRole::User)
        .unwrap();
    let mallory = mint_token("mallory", "mallory", "user");
    let app = test_app(state.clone());
    let ds = dataset_with_graph(&state, &app, &admin).await;

    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions"),
        &mallory,
        json!({ "version": "9.9.9" }),
    )
    .await;
    // Exactly 403: this used to be a 401, which means "not authenticated" and
    // which clients treat as a session expiry rather than a missing grant.
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "a stranger cannot cut a version: {txt}"
    );
    let (st, v, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/versions"),
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        v.as_array().map(Vec::len),
        Some(0),
        "nothing was created: {v}"
    );
}
