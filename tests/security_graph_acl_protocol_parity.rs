//! A `graph_acl` read grant must mean the same thing over every read protocol.
//!
//! `check_graph_read_access` (the Graph Store read path) consulted only
//! dataset-derived visibility, while the SPARQL path merges that set with
//! explicit `graph_acl` read grants. One grant therefore behaved differently
//! depending on which protocol you used — rows over `/sparql`, 401 over
//! `/store` — although `docs/security.md` presents graph ACLs as covering both.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::SystemRole;
use tower::ServiceExt as _;

const GRANTED_GRAPH: &str = "http://example.org/acl-granted";

/// A user holding only an explicit `graph_acl` read grant (no dataset access)
/// can read that graph over BOTH `/sparql` and `/store`.
#[tokio::test]
async fn graph_acl_read_grant_works_over_sparql_and_graph_store() {
    let (state, _admin) = admin_state();
    state
        .store
        .graph_store_put(
            Some(GRANTED_GRAPH),
            "<http://ex/s> <http://ex/p> \"granted-value\" .",
            oxigraph::io::RdfFormat::Turtle,
        )
        .unwrap();
    state
        .auth_db
        .create_user("reader", "reader", "reader@t.com", "hash", SystemRole::User)
        .unwrap();
    state
        .auth_db
        .grant_graph_permission("rule-1", GRANTED_GRAPH, "user", "reader", "read", "adm")
        .unwrap();
    let reader = mint_token("reader", "reader", "user");

    // Over SPARQL — the path that already honoured the grant.
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sparql?query={}",
                    url_encode(&format!(
                        "SELECT ?o WHERE {{ GRAPH <{GRANTED_GRAPH}> {{ ?s ?p ?o }} }}"
                    ))
                ))
                .header(header::AUTHORIZATION, format!("Bearer {reader}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "SPARQL read should succeed");
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains("granted-value"),
        "the grant must yield rows over SPARQL: {body}"
    );

    // Over the Graph Store Protocol — the path that ignored it.
    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/store?graph={}", url_encode(GRANTED_GRAPH)))
                .header(header::AUTHORIZATION, format!("Bearer {reader}"))
                .header(header::ACCEPT, "text/turtle")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "the same grant must also be honoured over the Graph Store Protocol"
    );
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains("granted-value"),
        "the graph's triples must be served: {body}"
    );
}

/// Without a grant, the same user is refused — the merge must not become a
/// blanket allow.
#[tokio::test]
async fn graph_store_read_still_denies_without_a_grant() {
    let (state, _admin) = admin_state();
    state
        .store
        .graph_store_put(
            Some("http://example.org/ungranted"),
            "<http://ex/s> <http://ex/p> \"secret\" .",
            oxigraph::io::RdfFormat::Turtle,
        )
        .unwrap();
    state
        .auth_db
        .create_user("nobody", "nobody", "nobody@t.com", "hash", SystemRole::User)
        .unwrap();
    let nobody = mint_token("nobody", "nobody", "user");

    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/store?graph={}",
                    url_encode("http://example.org/ungranted")
                ))
                .header(header::AUTHORIZATION, format!("Bearer {nobody}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a graph with no grant must stay unreadable"
    );
}

const WRITABLE_GRAPH: &str = "http://example.org/acl-writable";
const SECRET_GRAPH: &str = "http://example.org/acl-secret";

/// An UPDATE reads every graph its WHERE clause names, including the ones
/// named only inside an `EXISTS` / `NOT EXISTS` expression — in a `FILTER`,
/// a `BIND`, an `OPTIONAL`'s condition or a sub-select's `ORDER BY`. A writer
/// of one graph who cannot read another must not be able to probe it that
/// way: each probe below would otherwise write a marker into the writable
/// graph exactly when the secret graph holds the guessed triple.
#[tokio::test]
async fn an_update_cannot_probe_an_unreadable_graph_through_exists() {
    let (state, _admin) = admin_state();
    state
        .store
        .graph_store_put(
            Some(SECRET_GRAPH),
            "<http://ex/s> <http://ex/p> \"top-secret\" .",
            oxigraph::io::RdfFormat::Turtle,
        )
        .unwrap();
    state
        .auth_db
        .create_user("writer", "writer", "writer@t.com", "hash", SystemRole::User)
        .unwrap();
    state
        .auth_db
        .grant_graph_permission("rule-w", WRITABLE_GRAPH, "user", "writer", "write", "adm")
        .unwrap();
    let writer = mint_token("writer", "writer", "user");
    let guess = format!("GRAPH <{SECRET_GRAPH}> {{ ?s ?p \"top-secret\" }}");
    let insert =
        format!("INSERT {{ GRAPH <{WRITABLE_GRAPH}> {{ <http://ex/probe> <http://ex/hit> ?h }} }}");
    let probes = [
        format!("{insert} WHERE {{ BIND(1 AS ?h) FILTER EXISTS {{ {guess} }} }}"),
        format!("{insert} WHERE {{ BIND(1 AS ?h) FILTER NOT EXISTS {{ {guess} }} }}"),
        format!("{insert} WHERE {{ BIND(EXISTS {{ {guess} }} AS ?h) }}"),
        format!(
            "{insert} WHERE {{ BIND(1 AS ?x) OPTIONAL {{ BIND(1 AS ?h) FILTER(EXISTS {{ {guess} }}) }} }}"
        ),
        format!(
            "{insert} WHERE {{ {{ SELECT ?h WHERE {{ VALUES ?h {{ 1 2 }} }} ORDER BY (EXISTS {{ {guess} }}) LIMIT 1 }} }}"
        ),
        format!(
            "{insert} WHERE {{ {{ SELECT (SUM(IF(EXISTS {{ {guess} }}, 1, 0)) AS ?h) WHERE {{ VALUES ?v {{ 1 }} }} }} }}"
        ),
    ];
    for probe in probes {
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sparql")
                    .header(header::AUTHORIZATION, format!("Bearer {writer}"))
                    .header(header::CONTENT_TYPE, "application/sparql-update")
                    .body(Body::from(probe.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        assert!(
            status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
            "an EXISTS over an unreadable graph must be refused, got {status} for {probe}"
        );
    }
    assert_eq!(
        state.store.count_graph(Some(WRITABLE_GRAPH)).unwrap_or(0),
        0,
        "no probe may have written its marker"
    );

    // The same shape over a graph the writer may read still runs.
    state
        .auth_db
        .grant_graph_permission("rule-r", SECRET_GRAPH, "user", "writer", "read", "adm")
        .unwrap();
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sparql")
                .header(header::AUTHORIZATION, format!("Bearer {writer}"))
                .header(header::CONTENT_TYPE, "application/sparql-update")
                .body(Body::from(format!(
                    "{insert} WHERE {{ BIND(1 AS ?h) FILTER EXISTS {{ {guess} }} }}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        status.is_success(),
        "a readable graph in EXISTS is an ordinary read: {status} {}",
        body_text(resp.into_body()).await
    );
    assert_eq!(
        state.store.count_graph(Some(WRITABLE_GRAPH)).unwrap_or(0),
        1
    );
}
