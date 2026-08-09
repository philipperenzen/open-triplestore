//! End-to-end coverage for the full-text index behind the SPARQL endpoint.
//!
//! The unit tests in `src/text_search` pin the rewriting rules; these pin the
//! *wiring* — that a query arriving over HTTP is actually preprocessed, that a
//! write is reflected without a manual reindex, and that the read boundary the
//! index applies matches the one the endpoint applies.

#![cfg(feature = "text-search")]

mod common;

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{body_text, mint_token, test_app, test_state};
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use open_triplestore::text_search::index::GraphScopeOwned;
use open_triplestore::text_search::TextIndex;
use tower::ServiceExt as _;

const PUBLIC_GRAPH: &str = "http://example.org/graphs/public";
const PRIVATE_GRAPH: &str = "http://example.org/graphs/private";
const LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";

/// A state whose store holds one public and one private graph, each with a
/// resource whose label mentions "waalbrug", plus a live Tantivy index.
fn state_with_index() -> (AppState, tempfile::TempDir) {
    let mut state = test_state();

    let dir = tempfile::tempdir().unwrap();
    state.text_index = Some(Arc::new(TextIndex::open(dir.path()).unwrap()));
    // The store is written below; the index rebuilds itself from it lazily.
    state
        .text_dirty
        .store(true, std::sync::atomic::Ordering::Relaxed);

    state
        .auth_db
        .create_user("u1", "alice", "alice@test.com", "hash", SystemRole::User)
        .unwrap();
    state
        .auth_db
        .create_user("u2", "bob", "bob@test.com", "hash", SystemRole::User)
        .unwrap();

    // Public dataset — readable by everyone, including guests.
    state
        .auth_db
        .create_dataset(
            "pub-ds",
            "Public",
            None,
            OwnerType::User,
            "u1",
            Visibility::Public,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("pub-ds", PUBLIC_GRAPH)
        .unwrap();

    // Private dataset owned by u1 — u2 and guests must not see it.
    state
        .auth_db
        .create_dataset(
            "priv-ds",
            "Private",
            None,
            OwnerType::User,
            "u1",
            Visibility::Private,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("priv-ds", PRIVATE_GRAPH)
        .unwrap();
    state.auth_db.invalidate_accessible_graphs_cache();

    state
        .store
        .update(&format!(
            "INSERT DATA {{ \
               GRAPH <{PUBLIC_GRAPH}> {{ \
                 <http://example.org/bridge> <{LABEL}> \"Waalbrug Nijmegen\" \
               }} \
               GRAPH <{PRIVATE_GRAPH}> {{ \
                 <http://example.org/secret> <{LABEL}> \"Waalbrug classified\" \
               }} \
             }}"
        ))
        .unwrap();

    (state, dir)
}

async fn sparql(state: &AppState, query: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/sparql")
        .header("content-type", "application/sparql-query")
        .header("accept", "application/sparql-results+json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = test_app(state.clone())
        .oneshot(req.body(Body::from(query.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

#[tokio::test]
async fn text_search_returns_matches_without_a_manual_reindex() {
    // The index starts dirty against an already-populated store: the query
    // path must rebuild it and then *see* the rebuild. A reader left on the
    // pre-rebuild segments is why this used to come back empty every time.
    let (state, _dir) = state_with_index();
    let token = mint_token("u1", "alice", "user");

    let (status, body) = sparql(
        &state,
        "PREFIX ft: <tag:open-triplestore,2024:ft:>\n\
         SELECT ?s ?score WHERE {\n  (?s ?score) ft:search(\"waalbrug\" 10) .\n}",
        Some(&token),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains("http://example.org/bridge"),
        "a matching literal must be found, got: {body}"
    );
}

#[tokio::test]
async fn text_search_hits_are_limited_to_readable_graphs() {
    // The expansion is a bare VALUES clause — no triple pattern for the
    // endpoint's FROM scoping to constrain — so the index has to enforce the
    // read boundary itself or private subjects leak.
    let (state, _dir) = state_with_index();

    let query = "SELECT ?s ?score WHERE {\n  (?s ?score) text:search (\"waalbrug\" 10) .\n}";

    let owner = mint_token("u1", "alice", "user");
    let (status, body) = sparql(&state, query, Some(&owner)).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains("http://example.org/secret"), "got: {body}");

    // A different user, and a guest, may only see the public graph.
    let other = mint_token("u2", "bob", "user");
    for token in [Some(other.as_str()), None] {
        let (status, body) = sparql(&state, query, token).await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert!(
            !body.contains("http://example.org/secret"),
            "a private graph's subjects must not leak, got: {body}"
        );
        assert!(body.contains("http://example.org/bridge"), "got: {body}");
    }
}

#[tokio::test]
async fn a_write_is_visible_to_the_next_search() {
    let (state, _dir) = state_with_index();
    let token = mint_token("u1", "alice", "user");
    let query = "SELECT ?s WHERE {\n  (?s ?score) text:search (\"drawbridge\" 10) .\n}";

    let (_, before) = sparql(&state, query, Some(&token)).await;
    assert!(!before.contains("http://example.org/new"), "got: {before}");

    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PUBLIC_GRAPH}> {{ \
               <http://example.org/new> <{LABEL}> \"Drawbridge\" }} }}"
        ))
        .unwrap();
    state.mark_text_dirty();

    let (status, after) = sparql(&state, query, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "body: {after}");
    assert!(
        after.contains("http://example.org/new"),
        "a write must be searchable on the next query, got: {after}"
    );
}

#[tokio::test]
async fn contains_still_matches_inside_longer_words() {
    // The push-down prunes candidates with the index; if it prunes a row the
    // FILTER would have kept, the query silently returns the wrong answer.
    // "bridge" is a substring of "Waalbrug"'s sibling label below, but not a
    // token of it — exactly the case the old tokenized push-down dropped.
    let (state, _dir) = state_with_index();
    let token = mint_token("u1", "alice", "user");

    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PUBLIC_GRAPH}> {{ \
               <http://example.org/draw> <{LABEL}> \"Drawbridge\" }} }}"
        ))
        .unwrap();
    state.mark_text_dirty();

    let (status, body) = sparql(
        &state,
        &format!("SELECT ?s WHERE {{\n  ?s <{LABEL}> ?l .\n  FILTER(CONTAINS(?l, \"bridge\"))\n}}"),
        Some(&token),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains("http://example.org/draw"),
        "CONTAINS must still match a substring inside a word, got: {body}"
    );
}

#[tokio::test]
async fn contains_inside_optional_keeps_unmatched_rows() {
    // Hoisting an OPTIONAL's filter to the top level turns it into an inner
    // join and drops every row without a match.
    let (state, _dir) = state_with_index();
    let token = mint_token("u1", "alice", "user");

    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PUBLIC_GRAPH}> {{ \
               <http://example.org/plain> <http://example.org/kind> \"thing\" }} }}"
        ))
        .unwrap();
    state.mark_text_dirty();

    let (status, body) = sparql(
        &state,
        &format!(
            "SELECT ?s WHERE {{\n  ?s <http://example.org/kind> ?k .\n  \
             OPTIONAL {{ ?s <{LABEL}> ?l . FILTER(CONTAINS(?l, \"bridge\")) }}\n}}"
        ),
        Some(&token),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains("http://example.org/plain"),
        "a row with no OPTIONAL match must survive, got: {body}"
    );
}

/// Seed `n` extra literal triples so a rebuild takes long enough to observe.
fn seed_literals(state: &AppState, n: usize) {
    let mut insert = format!("INSERT DATA {{ GRAPH <{PUBLIC_GRAPH}> {{ ");
    for i in 0..n {
        insert.push_str(&format!(
            "<http://example.org/n{i}> <{LABEL}> \"filler label number {i} waalbrug\" . "
        ));
    }
    insert.push_str("} }");
    state.store.update(&insert).unwrap();
}

/// A whole-store reindex must not stall the async runtime.
///
/// The test runs on the default single-worker `#[tokio::test]` runtime, so a
/// rebuild executed inline would own the one worker for its whole duration and
/// the heartbeat below could not tick at all. Handing the work to
/// `spawn_blocking` leaves the worker free to keep driving other tasks — which
/// on a real deployment is every other in-flight request.
#[tokio::test]
async fn a_reindex_does_not_stall_the_runtime() {
    let (state, _dir) = state_with_index();
    seed_literals(&state, 20_000);
    state.mark_text_dirty();

    let ticks = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&ticks);
    let heartbeat = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(2)).await;
            counter.fetch_add(1, Ordering::Relaxed);
        }
    });

    // Let the heartbeat get going before the blocking work starts.
    tokio::time::sleep(Duration::from_millis(20)).await;
    let before = ticks.load(Ordering::Relaxed);

    let scope = GraphScopeOwned::Only(Arc::new(HashSet::from([PUBLIC_GRAPH.to_string()])));
    let started = Instant::now();
    let out = state
        .apply_text_search(
            "SELECT ?s ?score WHERE { (?s ?score) text:search (\"waalbrug\" 5) . }",
            scope,
        )
        .await
        .expect("preprocessing must not fail");
    let elapsed = started.elapsed();

    heartbeat.abort();
    let during = ticks.load(Ordering::Relaxed) - before;

    // Guard against a vacuous pass: if the rebuild finished too quickly there
    // was never a stall to detect, and the assertion below proves nothing.
    assert!(
        elapsed > Duration::from_millis(100),
        "rebuild finished in {elapsed:?} — too fast for this test to mean anything; \
         raise the seed count"
    );
    assert!(
        during >= 5,
        "only {during} heartbeat ticks during a {elapsed:?} rebuild — the runtime was blocked"
    );
    // …and it still did the work.
    assert!(!out.contains("text:search"), "got:\n{out}");
    assert!(out.contains("http://example.org/n"), "got:\n{out}");
}

/// Concurrent searches must share one rebuild, not race each other.
///
/// `reindex_from_store` empties the index before refilling it, so two rebuilds
/// running at once would delete each other's documents. That was already true
/// on a multi-worker runtime; moving the work to `spawn_blocking` widens the
/// window, because the blocking pool will happily run every waiting request in
/// parallel. `text_sync_lock` is what keeps that safe.
#[tokio::test]
async fn concurrent_searches_share_one_rebuild() {
    const QUERY: &str = "SELECT ?s ?score WHERE { (?s ?score) text:search (\"waalbrug\" 5) . }";

    let (state, _dir) = state_with_index();
    seed_literals(&state, 20_000);
    let scope = GraphScopeOwned::Only(Arc::new(HashSet::from([PUBLIC_GRAPH.to_string()])));

    // Baseline: one rebuild, uncontended.
    state.mark_text_dirty();
    let solo = Instant::now();
    state
        .apply_text_search(QUERY, scope.clone())
        .await
        .expect("preprocessing must not fail");
    let one_rebuild = solo.elapsed();
    assert!(
        one_rebuild > Duration::from_millis(100),
        "rebuild finished in {one_rebuild:?} — too fast to tell one from several"
    );

    // Now make it stale again and pile on.
    state.mark_text_dirty();
    let started = Instant::now();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let state = state.clone();
            let scope = scope.clone();
            tokio::spawn(async move { state.apply_text_search(QUERY, scope).await })
        })
        .collect();
    let mut outputs = Vec::new();
    for h in handles {
        outputs.push(
            h.await
                .expect("task panicked")
                .expect("preprocessing failed"),
        );
    }
    let elapsed = started.elapsed();

    for out in &outputs {
        assert!(!out.contains("text:search"), "not expanded:\n{out}");
        assert!(
            out.contains("http://example.org/n"),
            "a concurrent rebuild lost documents:\n{out}"
        );
    }
    assert!(
        elapsed < one_rebuild * 3,
        "{elapsed:?} for four concurrent searches against a {one_rebuild:?} rebuild — \
         they are not sharing one"
    );
    assert!(
        !state.text_dirty.load(std::sync::atomic::Ordering::Relaxed),
        "the index should be clean once the rebuild finished"
    );
}
