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
