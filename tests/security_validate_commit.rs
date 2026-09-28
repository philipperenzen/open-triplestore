//! `POST /api/datasets/validate-and-commit` must not let a caller claim — and
//! thereby overwrite — a graph that belongs to another dataset.
//!
//! Both commit branches (`target: "new"` and `target: "dataset"`) register the
//! caller-supplied graph IRI and then `graph_store_put` it, which REPLACES the
//! graph's contents. `can_write_dataset` only proves the caller owns *their*
//! dataset, so without the boundary gate this was the register-then-overwrite
//! bypass that the import and mapping-execution paths already close.
//!
//! The handler calls out to a validation service, so these tests stand up a
//! stub validator that always reports `conforms: true` — the interesting part
//! is what happens after validation passes.

mod common;

use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use common::*;
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use oxigraph::sparql::QueryResults;
use serde_json::json;
use tower::ServiceExt as _;

/// A validator that always conforms, started once for the whole binary.
fn stub_validator() {
    static ADDR: OnceLock<()> = OnceLock::new();
    ADDR.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                let app = Router::new().route(
                    "/validate",
                    post(|| async { Json(json!({ "conforms": true, "report": "" })) }),
                );
                axum::serve(listener, app).await.unwrap();
            });
        });
        let addr = rx.recv().unwrap();
        std::env::set_var("VALIDATION_API_URL", format!("http://{addr}"));
    });
}

const VICTIM_GRAPH: &str = "http://victim.example/protected";

/// Seed a graph owned by a *different* dataset, holding data we can check for.
fn seed_victim(state: &AppState) {
    state
        .auth_db
        .create_dataset(
            "victim-ds",
            "victim",
            None,
            OwnerType::User,
            "someone-else",
            Visibility::Private,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("victim-ds", VICTIM_GRAPH)
        .unwrap();
    state
        .store
        .graph_store_put(
            Some(VICTIM_GRAPH),
            "<http://victim.example/s> <http://victim.example/p> \"original\" .",
            oxigraph::io::RdfFormat::Turtle,
        )
        .unwrap();
}

fn victim_intact(state: &AppState) -> bool {
    matches!(
        state.store.query(&format!(
            "ASK {{ GRAPH <{VICTIM_GRAPH}> {{ ?s ?p \"original\" }} }}"
        )),
        Ok(QueryResults::Boolean(true))
    )
}

async fn commit_as(state: &AppState, token: &str, body: serde_json::Value) -> (StatusCode, String) {
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/datasets/validate-and-commit")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    (st, body_text(resp.into_body()).await)
}

fn payload(target: &str, dataset_id: Option<&str>) -> serde_json::Value {
    let mut commit = json!({ "target": target, "graph": VICTIM_GRAPH });
    if let Some(id) = dataset_id {
        commit["dataset_id"] = json!(id);
    }
    json!({
        "data": { "ttl": "<http://ex/mine> <http://ex/p> \"attacker\" ." },
        "shapes": { "ttl": "" },
        "commit_on_valid": commit
    })
}

/// `target: "new"` — creating a fresh dataset must not let it claim a graph
/// another dataset already owns.
#[tokio::test]
async fn commit_to_new_dataset_cannot_claim_a_foreign_graph() {
    stub_validator();
    let (state, _admin) = admin_state();
    seed_victim(&state);
    state
        .auth_db
        .create_user(
            "mallory",
            "mallory",
            "mallory@test.com",
            "hash",
            SystemRole::User,
        )
        .unwrap();
    let mallory = mint_token("mallory", "mallory", "user");

    let (st, body) = commit_as(&state, &mallory, payload("new", None)).await;
    assert!(
        st == StatusCode::FORBIDDEN || st == StatusCode::UNAUTHORIZED,
        "claiming another dataset's graph must be refused, got {st}: {body}"
    );
    assert!(
        victim_intact(&state),
        "the victim graph must not have been replaced"
    );
}

/// `target: "dataset"` — the same, for a caller who legitimately owns *their*
/// dataset but names someone else's graph.
#[tokio::test]
async fn commit_to_own_dataset_cannot_claim_a_foreign_graph() {
    stub_validator();
    let (state, _admin) = admin_state();
    seed_victim(&state);
    state
        .auth_db
        .create_user(
            "mallory",
            "mallory",
            "mallory@test.com",
            "hash",
            SystemRole::User,
        )
        .unwrap();
    state
        .auth_db
        .create_dataset(
            "mallory-ds",
            "mallory's",
            None,
            OwnerType::User,
            "mallory",
            Visibility::Private,
            None,
        )
        .unwrap();
    let mallory = mint_token("mallory", "mallory", "user");

    let (st, body) = commit_as(&state, &mallory, payload("dataset", Some("mallory-ds"))).await;
    assert!(
        st == StatusCode::FORBIDDEN || st == StatusCode::UNAUTHORIZED,
        "naming another dataset's graph must be refused, got {st}: {body}"
    );
    assert!(
        victim_intact(&state),
        "the victim graph must not have been replaced"
    );
}
