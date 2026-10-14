//! The materialisation budget (docs/notes/delta-versioning-design.md §3.3,
//! Q6): reading a delta builds it, and a build that would hold more quads
//! than `OTS_MATERIALISATION_MAX_QUADS` is refused with 507 Insufficient
//! Storage instead of filling the disk.
//!
//! A binary of its own because the budget is read from the environment on
//! every build, and the delta tests in `delta_versions_http.rs` build versions
//! concurrently.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::store::changes::{DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const G: &str = "urn:dvb:g";

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
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    )
}

#[tokio::test]
async fn a_build_over_the_budget_is_refused_with_507() {
    let store = TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD);
    let (state, token) = admin_state_with_store(store);
    let app = test_app(state.clone());
    let (st, v, txt) = req(
        &app,
        Method::POST,
        "/api/datasets",
        &token,
        json!({ "name": "dvb", "owner_type": "user", "owner_id": "adm", "visibility": "private" }),
    )
    .await;
    assert!(st.is_success(), "{txt}");
    let ds = v["id"].as_str().unwrap().to_string();
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/graphs"),
        &token,
        json!({ "graph_iri": G }),
    )
    .await;
    assert!(st.is_success(), "{txt}");
    state
        .store
        .load_str(
            "<urn:dvb:a> <urn:dvb:p> 1 . <urn:dvb:b> <urn:dvb:p> 2 .",
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
    let cut = |version: &'static str| {
        let app = app.clone();
        let token = token.clone();
        let ds = ds.clone();
        async move {
            let (st, v, txt) = req(
                &app,
                Method::POST,
                &format!("/api/datasets/{ds}/versions"),
                &token,
                json!({ "version": version }),
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{txt}");
            v
        }
    };
    cut("1.0.0").await;
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{G}> {{ <urn:dvb:c> <urn:dvb:p> 3 . <urn:dvb:d> <urn:dvb:p> 4 }} }}"
        ))
        .unwrap();
    let v2 = cut("1.1.0").await;
    assert_eq!(v2["kind"], "delta", "{v2}");
    assert_eq!(v2["quad_count"], 4, "{v2}");

    let data_uri = format!("/api/datasets/{ds}/versions/1.1.0/data?format=ntriples");
    std::env::set_var("OTS_MATERIALISATION_MAX_QUADS", "3");
    let (st, _, txt) = req(&app, Method::GET, &data_uri, &token, Value::Null).await;
    assert_eq!(st, StatusCode::INSUFFICIENT_STORAGE, "{txt}");
    assert!(
        state.store.changes().materialisations(None).is_empty(),
        "nothing was built"
    );

    // Within the budget the same version builds and reads whole.
    std::env::set_var("OTS_MATERIALISATION_MAX_QUADS", "4");
    let (st, _, txt) = req(&app, Method::GET, &data_uri, &token, Value::Null).await;
    std::env::remove_var("OTS_MATERIALISATION_MAX_QUADS");
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(txt.lines().filter(|l| !l.trim().is_empty()).count(), 4);
}
