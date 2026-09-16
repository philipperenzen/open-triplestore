//! `/sparql/batch` — batch atomicity at the HTTP layer.
//!
//! The endpoint is documented as applying its statements "atomically in one
//! transaction". The engine used to run each statement as its own oxigraph
//! transaction, so a batch that failed half-way left the earlier statements
//! applied (readiness audit: "/sparql/batch is documented as atomic but is
//! not"). These tests pin the documented contract: a batch is all-or-nothing,
//! whether a statement fails to parse or fails at execution.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::server::AppState;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

async fn batch(app: &Router, token: &str, updates: &[&str]) -> (StatusCode, Value, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/sparql/batch")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "updates": updates }).to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

fn ask(state: &AppState, q: &str) -> bool {
    match state.store.query(q).unwrap() {
        QueryResults::Boolean(b) => b,
        _ => panic!("ASK expected"),
    }
}

fn app() -> (Router, AppState, String) {
    let (state, token) = admin_state();
    (test_app(state.clone()), state, token)
}

/// The audit's probe: a statement that does not parse aborts the batch, and
/// the statements before it are not applied.
#[tokio::test]
async fn a_statement_that_fails_to_parse_applies_nothing() {
    let (app, state, token) = app();
    let (st, _, txt) = batch(
        &app,
        &token,
        &["INSERT DATA { <urn:a> <urn:p> 1 }", "THIS IS NOT SPARQL"],
    )
    .await;
    assert!(
        st.is_client_error(),
        "an unparseable statement is a client error, got {st}: {txt}"
    );
    assert!(
        !ask(&state, "ASK { <urn:a> <urn:p> 1 }"),
        "statement 1 must not be applied when statement 2 fails to parse: {txt}"
    );
}

/// A statement that parses but fails at execution — `DROP` of a graph that
/// does not exist, without `SILENT` — rolls the whole batch back: the
/// statements before it are undone, the ones after it never run.
#[tokio::test]
async fn a_statement_that_fails_at_execution_rolls_the_batch_back() {
    let (app, state, token) = app();
    let (st, body, txt) = batch(
        &app,
        &token,
        &[
            "INSERT DATA { <urn:a> <urn:p> 1 }",
            "DROP GRAPH <urn:batch:does-not-exist>",
            "INSERT DATA { <urn:b> <urn:p> 2 }",
        ],
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a rolled-back batch is 422, nothing applied: {txt}"
    );
    assert_eq!(body["status"], "rolled_back", "{txt}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.starts_with("statement 1 failed:")),
        "the body says which statement failed and why: {txt}"
    );
    assert_eq!(body["count"], 3, "{txt}");
    let results = body["results"].as_array().expect("per-statement results");
    assert_eq!(results.len(), 3, "{txt}");
    assert_eq!(results[1]["index"], 1, "{txt}");
    assert_eq!(results[1]["status"], "error", "{txt}");
    assert!(
        results[1]["error"].as_str().is_some_and(|e| !e.is_empty()),
        "the failing statement carries its error: {txt}"
    );
    for i in [0, 2] {
        assert_eq!(
            results[i]["status"], "rolled_back",
            "statement {i} must be reported as rolled back, not ok: {txt}"
        );
    }
    assert!(
        !ask(&state, "ASK { <urn:a> <urn:p> 1 }"),
        "statement 1 was applied although statement 2 failed: {txt}"
    );
    assert!(
        !ask(&state, "ASK { <urn:b> <urn:p> 2 }"),
        "statement 3 was applied although statement 2 failed: {txt}"
    );
}

/// A batch whose statements all succeed is applied as a whole.
#[tokio::test]
async fn a_batch_that_succeeds_applies_every_statement() {
    let (app, state, token) = app();
    let (st, body, txt) = batch(
        &app,
        &token,
        &[
            "INSERT DATA { <urn:x> <urn:p> 1 }",
            "INSERT DATA { GRAPH <urn:g> { <urn:y> <urn:p> 2 } }",
            "INSERT DATA { <urn:z> <urn:p> 3 }",
        ],
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(body["status"], "ok", "{txt}");
    assert_eq!(body["count"], 3, "{txt}");
    assert!(ask(&state, "ASK { <urn:x> <urn:p> 1 }"), "{txt}");
    assert!(
        ask(&state, "ASK { GRAPH <urn:g> { <urn:y> <urn:p> 2 } }"),
        "{txt}"
    );
    assert!(ask(&state, "ASK { <urn:z> <urn:p> 3 }"), "{txt}");
    // The graph count index is maintained across the batch.
    assert_eq!(state.store.count_graph(Some("urn:g")).unwrap(), 1);
}

/// Statements run in order inside one transaction: a later statement sees
/// the effect of an earlier one.
#[tokio::test]
async fn statements_in_a_batch_see_each_other() {
    let (app, state, token) = app();
    let (st, body, txt) = batch(
        &app,
        &token,
        &[
            "INSERT DATA { <urn:c> <urn:p> 1 }",
            "DELETE { ?s <urn:p> ?o } INSERT { ?s <urn:p> 2 } WHERE { ?s <urn:p> ?o }",
        ],
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(body["status"], "ok", "{txt}");
    assert!(ask(&state, "ASK { <urn:c> <urn:p> 2 }"), "{txt}");
    assert!(!ask(&state, "ASK { <urn:c> <urn:p> 1 }"), "{txt}");
}
