//! `POST /api/reasoning/materialize` over HTTP: entailment graphs are rebuilt,
//! not accumulated, and the report counts what this run added.
//!
//! Materialisation only ever INSERTed into `urn:entailment:*`, so a source
//! triple that was later deleted kept its stale consequences forever — and they
//! were still folded into every `?entailment=` query. Separately, every
//! materializer reported `triples_added` as the target graph's SIZE, so a run
//! that inferred nothing still claimed thousands of additions.

#![cfg(feature = "rdfs-entailment")]

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::server::AppState;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const RDFS_TG: &str = "urn:entailment:rdfs";

async fn materialize(state: &AppState, token: &str, regime: &str) -> (StatusCode, Value) {
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/reasoning/materialize")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(json!({ "regime": regime }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    (st, body_json(resp.into_body()).await)
}

fn entailed(state: &AppState, pattern: &str) -> bool {
    matches!(
        state
            .store
            .query(&format!("ASK {{ GRAPH <{RDFS_TG}> {{ {pattern} }} }}")),
        Ok(QueryResults::Boolean(true))
    )
}

fn count_entailed(state: &AppState) -> usize {
    match state.store.query(&format!(
        "SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{RDFS_TG}> {{ ?s ?p ?o }} }}"
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

/// Deleting a source triple and re-materialising removes its consequences.
#[tokio::test]
async fn rematerialising_drops_stale_inferences() {
    let (state, token) = admin_state();
    state
        .store
        .load_str(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
             @prefix ex: <http://example.org/> . \
             ex:Dog rdfs:subClassOf ex:Animal . \
             ex:rex a ex:Dog .",
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();

    let (st, body) = materialize(&state, &token, "rdfs").await;
    assert_eq!(st, StatusCode::OK, "first run: {body}");
    assert!(
        entailed(
            &state,
            "<http://example.org/rex> a <http://example.org/Animal>"
        ),
        "rdfs9 must derive rex a Animal"
    );

    // The source of that inference goes away.
    state
        .store
        .update("DELETE DATA { <http://example.org/rex> a <http://example.org/Dog> }")
        .unwrap();

    let (st, body) = materialize(&state, &token, "rdfs").await;
    assert_eq!(st, StatusCode::OK, "second run: {body}");
    assert!(
        !entailed(
            &state,
            "<http://example.org/rex> a <http://example.org/Animal>"
        ),
        "a consequence whose premise was deleted must not survive re-materialisation"
    );
}

/// Over HTTP the entailment graph is rebuilt from scratch each run, so the
/// report's `triples_added` is the size of the freshly rebuilt graph — and that
/// size must SHRINK when a premise is removed. Without the rebuild, stale
/// consequences stayed put and the count could only ever grow. (The
/// delta-versus-size distinction itself is pinned at store level, where no
/// clearing happens: see `test_el_idempotent` and `test_rdfs_rerun_adds_nothing`.)
#[tokio::test]
async fn rebuilt_graph_shrinks_when_a_premise_is_removed() {
    let (state, token) = admin_state();
    state
        .store
        .load_str(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
             @prefix ex: <http://example.org/> . \
             ex:Dog rdfs:subClassOf ex:Animal . \
             ex:rex a ex:Dog . ex:fido a ex:Dog .",
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();

    let (st, first) = materialize(&state, &token, "rdfs").await;
    assert_eq!(st, StatusCode::OK, "{first}");
    let added_first = first["triples_added"].as_u64().unwrap();
    let size_first = count_entailed(&state) as u64;
    assert!(added_first > 0, "a fresh run must add something: {first}");
    assert_eq!(
        added_first, size_first,
        "into an empty entailment graph, added == size: {first}"
    );

    // Remove one premise. The rebuilt graph loses its consequences, so both
    // the reported count and the graph itself get smaller.
    state
        .store
        .update("DELETE DATA { <http://example.org/fido> a <http://example.org/Dog> }")
        .unwrap();
    let (st, second) = materialize(&state, &token, "rdfs").await;
    assert_eq!(st, StatusCode::OK, "{second}");
    let added_second = second["triples_added"].as_u64().unwrap();
    assert!(
        added_second < added_first,
        "after removing a premise the rebuilt graph must be smaller: \
         first={added_first} second={added_second}"
    );
    assert_eq!(
        count_entailed(&state) as u64,
        added_second,
        "the reported count must be exactly what the rebuilt graph holds"
    );
}
