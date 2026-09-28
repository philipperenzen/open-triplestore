//! The re-check after an admin's SPARQL Update whose written graphs cannot be
//! named before it runs (a variable graph in a DELETE/INSERT template,
//! `CLEAR ALL`), through the real router.
//!
//! No guard can look at such a write's graphs beforehand, so every model copy
//! a check vouched for is compared with its recorded digest afterwards, and a
//! changed one is marked possibly modified: an altered no-derivatives copy
//! (IMBOR) is then withheld from everyone who may not write its entry. The
//! re-check runs in the write's own blocking task, so it runs whenever the
//! write committed: also when the request stopped waiting at the timeout.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::data_models::{registry, seed_vocab};
use open_triplestore::server::AppState;
use tower::ServiceExt as _;

const IMBOR_TERM: &str = "https://data.crow.nl/imbor/term/74e825e1-9b93-4dd5-8fab-52783fdb758b";

async fn send(state: &AppState, req: Request<Body>) -> (StatusCode, String) {
    let resp = test_app(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

fn imbor_unchanged(state: &AppState) -> bool {
    registry::get_attribution(
        &state.store,
        &registry::version_record_iri(&state.base_url, "imbor", "2025"),
    )
    .unwrap()
    .unchanged
}

fn imbor_download() -> Request<Body> {
    Request::builder()
        .uri("/api/models/imbor/versions/2025/data?format=nt")
        .body(Body::empty())
        .unwrap()
}

/// An update that writes IMBOR through `GRAPH ?g`, made slow enough by a
/// count over the whole store that a zero-second timeout gives up on it.
fn wildcard_edit() -> String {
    format!(
        "INSERT {{ GRAPH ?g {{ <{IMBOR_TERM}> <http://www.w3.org/2000/01/rdf-schema#comment> \"edited\" }} }} \
         WHERE {{ GRAPH ?g {{ <{IMBOR_TERM}> a ?t }} \
                  {{ SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH ?h {{ ?a ?b ?c }} }} }} }}"
    )
}

async fn wait_until_marked(state: &AppState) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while imbor_unchanged(state) {
        assert!(
            std::time::Instant::now() < deadline,
            "the re-check never marked the altered IMBOR copy"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// The request stops waiting at the timeout; the write commits anyway, and
/// the re-check still runs after it: IMBOR's record stops calling the copy
/// unchanged and the altered copy is withheld from anonymous callers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unscoped_update_that_times_out_is_still_rechecked() {
    let (mut state, token) = admin_state();
    seed_vocab::seed_standard_vocabularies(&state);
    assert!(imbor_unchanged(&state));
    state.query_timeout_secs = 0;

    let (status, body) = send(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/sparql")
            .header(header::CONTENT_TYPE, "application/sparql-update")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(wildcard_edit()))
            .unwrap(),
    )
    .await;
    assert!(
        status == StatusCode::NO_CONTENT
            || (status == StatusCode::BAD_REQUEST && body.contains("timed out")),
        "{status}: {body}"
    );

    wait_until_marked(&state).await;
    let edited = format!(
        "ASK {{ GRAPH <{}> {{ <{IMBOR_TERM}> ?p \"edited\" }} }}",
        registry::version_record_iri(&state.base_url, "imbor", "2025")
    );
    assert!(matches!(
        state.store.query(&edited),
        Ok(oxigraph::sparql::QueryResults::Boolean(true))
    ));
    assert_eq!(
        send(&state, imbor_download()).await.0,
        StatusCode::FORBIDDEN
    );
}

/// `/sparql/batch` re-checks after a statement that writes unnamed graphs.
#[tokio::test]
async fn a_batch_that_writes_unnamed_graphs_is_rechecked() {
    let (state, token) = admin_state();
    seed_vocab::seed_standard_vocabularies(&state);
    assert!(imbor_unchanged(&state));
    let batch = serde_json::json!({ "updates": [
        "INSERT DATA { GRAPH <http://example.org/mine> { <http://ex.org/a> <http://ex.org/b> \"c\" } }",
        wildcard_edit(),
    ]});
    let (status, body) = send(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/sparql/batch")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(batch.to_string()))
            .unwrap(),
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
    // The response follows the re-check.
    assert!(!imbor_unchanged(&state));
    assert_eq!(
        send(&state, imbor_download()).await.0,
        StatusCode::FORBIDDEN
    );
    // Other copies are unaffected.
    assert!(
        registry::get_attribution(
            &state.store,
            &registry::version_record_iri(&state.base_url, "rdf", "1.1"),
        )
        .unwrap()
        .unchanged
    );
}
