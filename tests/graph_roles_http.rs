//! The layered-graph role convention over HTTP: every role of `GraphKind` can
//! be declared per dataset and per graph, the convention's own spelling
//! (`ontology`) folds onto the canonical `model`, and an unknown role is a 400
//! — it used to fold to "no role" and silently CLEAR the graph's role.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const G: &str = "urn:roles:alignments";

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

async fn role_of(app: &Router, token: &str, ds: &str, graph: &str) -> Value {
    let (st, v, txt) = req(
        app,
        Method::GET,
        &format!("/api/datasets/{ds}/graphs"),
        token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let entries = v
        .as_array()
        .cloned()
        .or_else(|| v["graphs"].as_array().cloned())
        .unwrap_or_default();
    entries
        .iter()
        .find(|e| e["graph_iri"] == graph)
        .map(|e| e["graph_role"].clone())
        .unwrap_or_else(|| panic!("graph {graph} not registered: {txt}"))
}

#[tokio::test]
async fn roles_are_declared_per_dataset_and_per_graph_with_aliases_folded() {
    let (state, token) = admin_state();
    let app = test_app(state);

    // Dataset-level role in the convention's own word.
    let (st, v, txt) = req(
        &app,
        Method::POST,
        "/api/datasets",
        &token,
        json!({ "name": "roles", "owner_type": "user", "owner_id": "adm", "visibility": "private", "graph_role": "ontology" }),
    )
    .await;
    assert!(st.is_success(), "create dataset: {st} {txt}");
    let ds = v["id"].as_str().unwrap().to_string();
    assert_eq!(
        v["graph_role"], "model",
        "`ontology` folds onto the canonical `model`: {txt}"
    );

    // Per-graph roles, including the four layered-convention ones.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/graphs"),
        &token,
        json!({ "graph_iri": G }),
    )
    .await;
    assert!(st.is_success(), "register graph: {st} {txt}");
    for role in [
        "linkset",
        "provenance",
        "catalog",
        "domain-values",
        "vocabulary",
    ] {
        let (st, _, txt) = req(
            &app,
            Method::PATCH,
            &format!("/api/datasets/{ds}/graphs"),
            &token,
            json!({ "graph_iri": G, "graph_role": role }),
        )
        .await;
        assert!(st.is_success(), "set role {role}: {st} {txt}");
        assert_eq!(
            role_of(&app, &token, &ds, G).await,
            role,
            "role {role} is stored and listed"
        );
    }

    // An explicit null clears the role.
    let (st, _, txt) = req(
        &app,
        Method::PATCH,
        &format!("/api/datasets/{ds}/graphs"),
        &token,
        json!({ "graph_iri": G, "graph_role": null }),
    )
    .await;
    assert!(st.is_success(), "{txt}");
    assert!(role_of(&app, &token, &ds, G).await.is_null());
}

#[tokio::test]
async fn an_unknown_role_is_a_400_and_does_not_clear_the_role() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let (_, v, _) = req(
        &app,
        Method::POST,
        "/api/datasets",
        &token,
        json!({ "name": "roles2", "owner_type": "user", "owner_id": "adm", "visibility": "private" }),
    )
    .await;
    let ds = v["id"].as_str().unwrap().to_string();
    req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/graphs"),
        &token,
        json!({ "graph_iri": G }),
    )
    .await;
    req(
        &app,
        Method::PATCH,
        &format!("/api/datasets/{ds}/graphs"),
        &token,
        json!({ "graph_iri": G, "graph_role": "linkset" }),
    )
    .await;

    let (st, _, txt) = req(
        &app,
        Method::PATCH,
        &format!("/api/datasets/{ds}/graphs"),
        &token,
        json!({ "graph_iri": G, "graph_role": "bogus" }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "a typo must not be a 200: {txt}"
    );
    assert!(
        txt.contains("bogus") && txt.contains("linkset"),
        "the error names the value and the valid roles: {txt}"
    );
    assert_eq!(
        role_of(&app, &token, &ds, G).await,
        "linkset",
        "the role survives the rejected request"
    );

    // The dataset-level role endpoint behaves the same.
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{ds}/role"),
        &token,
        json!({ "graph_role": "bogus" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        &format!("/api/datasets/{ds}/role"),
        &token,
        json!({ "graph_role": "catalog" }),
    )
    .await;
    assert!(st.is_success(), "{txt}");
}
