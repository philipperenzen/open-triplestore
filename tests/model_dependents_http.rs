//! Model versions ↔ dependent datasets: a cut dataset version records the model
//! version it was pinned to; the conformance layer flags a newer published model
//! version; `/api/models/:id/dependents` lists the datasets behind it; publishing a
//! model version lands on the model's commit log.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::registry as dmr;
use open_triplestore::server::AppState;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const EX: &str = "http://ex.org/";
const G1: &str = "urn:dep:d1:instances";

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

fn model_version(state: &AppState, ver: &str, status: VersionStatus) {
    let base = state.base_url.to_string();
    dmr::insert_version(
        &state.store,
        &base,
        &DataModelVersion {
            data_model_id: "m1".to_string(),
            version: ver.to_string(),
            status,
            graph_iri: format!("{base}/data-model/m1/version/{ver}"),
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
}

#[tokio::test]
async fn dataset_versions_record_the_model_pin_and_dependents_see_updates() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "m1",
        "Bridges",
        &format!("{EX}def#"),
        None,
        true,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    model_version(&state, "1.0.0", VersionStatus::Published);
    dmr::update_latest_published(&state.store, &base, "m1", "1.0.0").unwrap();

    // A dataset pinned to m1@1.0.0 with one instance graph.
    let (st, v, txt) = req(&app, Method::POST, "/api/datasets", &token,
        json!({ "name": "d1", "owner_type": "user", "owner_id": "adm", "visibility": "private", "graph_role": "instances" })).await;
    assert!(st.is_success(), "{st} {txt}");
    let d1 = v["id"].as_str().unwrap().to_string();
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{d1}/graphs"),
        &token,
        json!({ "graph_iri": G1 }),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    let (st, _, txt) = req(&app, Method::PUT, &format!("/api/datasets/{d1}"), &token,
        json!({ "name": "d1", "visibility": "private", "conforms_to_model": "m1", "conforms_to_version": "1.0.0" })).await;
    assert!(st.is_success(), "{st} {txt}");

    // Cutting a version stamps the pin onto it.
    let (st, ver, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{d1}/versions"),
        &token,
        json!({ "version": "1.0.0", "notes": "first" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(ver["conforms_to_model"], "m1");
    assert_eq!(ver["conforms_to_version"], "1.0.0");
    let (st, list, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{d1}/versions"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(list[0]["conforms_to_version"], "1.0.0");
    let (st, _, _) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{d1}/versions/1.0.0/publish"),
        &token,
        Value::Null,
    )
    .await;
    assert!(st.is_success());

    // Current: the layer says so, dependents are up to date.
    let (st, layer, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{d1}/conformance"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(layer["conforms_to_model"]["latest_published"], "1.0.0");
    assert_eq!(layer["conforms_to_model"]["pinned"], true);
    assert_eq!(layer["conforms_to_model"]["update_available"], false);
    let (st, deps, txt) = req(
        &app,
        Method::GET,
        "/api/models/m1/dependents",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(deps["datasets"][0]["dataset_id"], d1.as_str());
    assert_eq!(deps["datasets"][0]["update_available"], false);
    assert_eq!(deps["datasets"][0]["published_version"], "1.0.0");
    assert_eq!(
        deps["datasets"][0]["published_conforms_to_version"],
        "1.0.0"
    );

    // Model 2.0.0 is published through the API: the dataset is now behind.
    model_version(&state, "2.0.0", VersionStatus::Staged);
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/models/m1/versions/2.0.0/publish",
        &token,
        Value::Null,
    )
    .await;
    assert!(st.is_success(), "publish model: {st} {txt}");
    let (_, layer, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{d1}/conformance"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(
        layer["conforms_to_model"]["version"], "1.0.0",
        "pinned datasets do not move"
    );
    assert_eq!(layer["conforms_to_model"]["latest_published"], "2.0.0");
    assert_eq!(layer["conforms_to_model"]["update_available"], true);
    let (_, deps, _) = req(
        &app,
        Method::GET,
        "/api/models/m1/dependents",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(deps["latest_published"], "2.0.0");
    assert_eq!(deps["datasets"][0]["update_available"], true);
    assert_eq!(deps["datasets"][0]["pinned_version"], "1.0.0");

    // Publishing landed on the model's commit log.
    let (st, commits, txt) = req(
        &app,
        Method::GET,
        "/api/models/m1/commits",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let msgs: Vec<String> = commits
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|c| c["message"].as_str().map(str::to_string))
        .collect();
    assert!(
        msgs.iter().any(|m| m.contains("Published version 2.0.0")),
        "{txt}"
    );

    // The update procedure ends with a re-pin: the layer is current again and the
    // next dataset version records the new model version.
    let (st, _, txt) = req(&app, Method::PUT, &format!("/api/datasets/{d1}"), &token,
        json!({ "name": "d1", "visibility": "private", "conforms_to_model": "m1", "conforms_to_version": "2.0.0" })).await;
    assert!(st.is_success(), "{st} {txt}");
    let (_, ver2, _) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{d1}/versions"),
        &token,
        json!({ "version": "1.1.0", "notes": "conforms to m1@2.0.0" }),
    )
    .await;
    assert_eq!(ver2["conforms_to_version"], "2.0.0");
    let (_, layer, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{d1}/conformance"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(layer["conforms_to_model"]["update_available"], false);

    // A floating dataset (no pinned version) is never "behind".
    let (st, _, txt) = req(&app, Method::PUT, &format!("/api/datasets/{d1}"), &token,
        json!({ "name": "d1", "visibility": "private", "conforms_to_model": "m1", "conforms_to_version": null })).await;
    assert!(st.is_success(), "{st} {txt}");
    let (_, layer, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{d1}/conformance"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(layer["conforms_to_model"]["pinned"], false);
    assert_eq!(layer["conforms_to_model"]["update_available"], false);
}

#[tokio::test]
async fn dependents_are_visibility_scoped() {
    let (state, admin) = admin_state();
    let app = test_app(state.clone());
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "m2",
        "Public model",
        &format!("{EX}def2#"),
        None,
        true,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    model_version(&state, "1.0.0", VersionStatus::Published);
    let (st, v, txt) = req(&app, Method::POST, "/api/datasets", &admin,
        json!({ "name": "secret", "owner_type": "user", "owner_id": "adm", "visibility": "private" })).await;
    assert!(st.is_success(), "{st} {txt}");
    let id = v["id"].as_str().unwrap().to_string();
    let (st, _, txt) = req(&app, Method::PUT, &format!("/api/datasets/{id}"), &admin,
        json!({ "name": "secret", "visibility": "private", "conforms_to_model": "m2", "conforms_to_version": "1.0.0" })).await;
    assert!(st.is_success(), "{st} {txt}");
    // Another (plain) user sees the public model but not the private dataset behind it.
    let other = mint_token("u2", "someone", "user");
    let (st, deps, txt) = req(
        &app,
        Method::GET,
        "/api/models/m2/dependents",
        &other,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(deps["datasets"].as_array().map(|a| a.len()), Some(0));
    let (_, deps, _) = req(
        &app,
        Method::GET,
        "/api/models/m2/dependents",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(deps["datasets"].as_array().map(|a| a.len()), Some(1));
}
