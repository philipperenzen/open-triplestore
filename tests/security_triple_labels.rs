//! Triple security labels must actually withhold the triples they label.
//!
//! The feature was a no-op. The admin API stored terms exactly as the caller
//! sent them — a bare `http://ex/s` — while the filter builds its lookup keys by
//! splitting an N-Triples serialisation, giving `<http://ex/s>`. Exact SQL
//! equality between the two never matched, so no label ever denied anything.
//! The repo's own API test posted bare IRIs and asserted only that creation
//! returned 201.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::SystemRole;
use tower::ServiceExt as _;

const DATA_GRAPH: &str = "http://example.org/hr";
const LABEL_GRAPH: &str = "http://example.org/labels/confidential";

/// A labelled triple is withheld from a reader without access to the label
/// graph, while the rest of the graph is still served.
#[tokio::test]
async fn a_labelled_triple_is_withheld_from_an_unprivileged_reader() {
    let (state, _admin) = admin_state();
    state
        .store
        .graph_store_put(
            Some(DATA_GRAPH),
            "<http://example.org/alice> <http://example.org/salary> \"120000\" .\n\
             <http://example.org/alice> <http://example.org/role> \"Engineer\" .",
            oxigraph::io::RdfFormat::Turtle,
        )
        .unwrap();

    // A reader who may read the data graph but NOT the label graph.
    state
        .auth_db
        .create_user("reader", "reader", "reader@t.com", "hash", SystemRole::User)
        .unwrap();
    state
        .auth_db
        .grant_graph_permission("g1", DATA_GRAPH, "user", "reader", "read", "adm")
        .unwrap();
    let reader = mint_token("reader", "reader", "user");

    // Label the salary triple. Terms are given in the natural bare form — the
    // form the admin UI sends — and must be canonicalised on the way in.
    state
        .auth_db
        .create_triple_security_label(
            "lbl-1",
            "http://example.org/alice",
            "http://example.org/salary",
            "\"120000\"",
            DATA_GRAPH,
            LABEL_GRAPH,
        )
        .unwrap();

    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/store?graph={}", url_encode(DATA_GRAPH)))
                .header(header::AUTHORIZATION, format!("Bearer {reader}"))
                .header(header::ACCEPT, "application/n-triples")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "the graph is readable");
    let body = body_text(resp.into_body()).await;

    assert!(
        !body.contains("120000"),
        "the labelled triple must be withheld, but the response contained it:\n{body}"
    );
    assert!(
        body.contains("Engineer"),
        "unlabelled triples in the same graph must still be served:\n{body}"
    );
}

/// An admin sees everything — the label filter is a read-down control, not a
/// blanket redaction.
#[tokio::test]
async fn an_admin_still_sees_labelled_triples() {
    let (state, admin) = admin_state();
    state
        .store
        .graph_store_put(
            Some(DATA_GRAPH),
            "<http://example.org/alice> <http://example.org/salary> \"120000\" .",
            oxigraph::io::RdfFormat::Turtle,
        )
        .unwrap();
    state
        .auth_db
        .create_triple_security_label(
            "lbl-1",
            "http://example.org/alice",
            "http://example.org/salary",
            "\"120000\"",
            DATA_GRAPH,
            LABEL_GRAPH,
        )
        .unwrap();

    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/store?graph={}", url_encode(DATA_GRAPH)))
                .header(header::AUTHORIZATION, format!("Bearer {admin}"))
                .header(header::ACCEPT, "application/n-triples")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains("120000"),
        "an admin bypasses the label filter:\n{body}"
    );
}
