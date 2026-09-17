//! QLever as a read backend (P5): the feeder keeps an endpoint current
//! from the change log (deltas for `full` rows, graph replaces for the rest,
//! everything on an epoch change), the router sends only what the policy
//! names and only while the feeder is caught up, and the answers equal the
//! engine's. In-process: a stand-in endpoint takes the same updates.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use open_triplestore::store::changes::{DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN};
use open_triplestore::store::qlever::{FakeQlever, QleverConfig, Route};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::Value;
use tower::ServiceExt as _;

mod common;
use common::*;

const G: &str = "https://example.org/ql/g";

fn ttl(ns: &[u32]) -> String {
    ns.iter()
        .map(|n| format!("<https://example.org/ql/s{n}> <https://example.org/ql/p> {n} ."))
        .collect::<Vec<_>>()
        .join("\n")
}

fn rows(r: QueryResults<'static>) -> BTreeSet<Vec<Option<String>>> {
    match r {
        QueryResults::Solutions(s) => {
            let vars: Vec<String> = s
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            s.map(|sol| {
                let sol = sol.unwrap();
                vars.iter()
                    .map(|v| sol.get(v.as_str()).map(|t| t.to_string()))
                    .collect()
            })
            .collect()
        }
        QueryResults::Boolean(b) => BTreeSet::from([vec![Some(b.to_string())]]),
        QueryResults::Graph(_) => BTreeSet::new(),
    }
}

fn store_with(route: Route, fake: &Arc<FakeQlever>) -> TripleStore {
    let mut config =
        QleverConfig::parse(Some("inprocess"), None, None, None, Some("2"), None, None);
    config.route = route;
    let endpoint: Arc<dyn open_triplestore::store::qlever::QleverEndpoint> = fake.clone();
    TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD)
        .with_query_cache(false, 0, 0)
        .with_parallel_query(false, 1, 0)
        .with_qlever(config, Some(endpoint))
}

#[test]
fn the_feeder_keeps_the_endpoint_current_and_the_router_serves_from_it() {
    let fake = Arc::new(FakeQlever::new());
    let s = store_with(Route::First, &fake);
    s.graph_store_put(Some(G), &ttl(&[1, 2, 3]), RdfFormat::Turtle)
        .unwrap();
    s.update("INSERT DATA { <https://example.org/ql/d1> <https://example.org/ql/p> 10 }")
        .unwrap();
    let q = "SELECT ?s ?o WHERE { GRAPH ?g { ?s <https://example.org/ql/p> ?o } }";
    let dq = "SELECT ?s ?o WHERE { ?s <https://example.org/ql/p> ?o }";
    let count = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }";

    // Not fed yet: not caught up, nothing routed.
    assert!(!s.qlever().caught_up(&s));
    assert_eq!(s.qlever().status(&s).queries_served, 0);
    let _ = s.query(q).unwrap();
    assert_eq!(s.qlever().status(&s).queries_served, 0);

    // The first feed replaces everything (the epoch was unknown), including
    // the default graph; then the router serves.
    let p = s.qlever().feed_once(&s).unwrap();
    assert_eq!(p.resyncs, 1, "{p:?}");
    assert!(p.replaced_graphs >= 2, "{p:?}");
    assert!(s.qlever().caught_up(&s));
    assert_eq!(
        rows(s.query(q).unwrap()),
        rows(fake.store.query(q).unwrap())
    );
    assert_eq!(rows(s.query(dq).unwrap()).len(), 1);
    assert_eq!(rows(s.query(q).unwrap()).len(), 3);
    let st = s.qlever().status(&s);
    assert!(st.queries_served >= 2, "{st:?}");
    assert_eq!(st.applied_seq, s.changes().last_seq());
    assert_eq!(
        s.changes().cursor("qlever").map(|c| c.seq),
        Some(st.applied_seq)
    );

    // A ground update is a delta: applied as such, no graph replace.
    s.update(&format!(
        "INSERT DATA {{ GRAPH <{G}> {{ <https://example.org/ql/s9> <https://example.org/ql/p> 9 }} }}"
    ))
    .unwrap();
    assert!(
        !s.qlever().caught_up(&s),
        "a write leaves the feeder behind"
    );
    let served_before = s.qlever().status(&s).queries_served;
    let _ = s.query(count).unwrap();
    assert_eq!(
        s.qlever().status(&s).queries_served,
        served_before,
        "not routed while behind"
    );
    let p = s.qlever().feed_once(&s).unwrap();
    assert_eq!(
        (p.applied_rows, p.replaced_graphs, p.resyncs),
        (1, 0, 0),
        "{p:?}"
    );
    assert_eq!(rows(s.query(q).unwrap()).len(), 4);
    assert_eq!(rows(fake.store.query(q).unwrap()).len(), 4);

    // A scanned update over the cap (caps of two) is an unknown row: a replace.
    let s2 = TripleStore::in_memory()
        .unwrap()
        .with_change_capture(1, 1)
        .with_query_cache(false, 0, 0)
        .with_parallel_query(false, 1, 0)
        .with_qlever(
            QleverConfig::parse(
                Some("inprocess"),
                None,
                None,
                Some("first"),
                None,
                None,
                None,
            ),
            Some(Arc::new(FakeQlever::new())
                as Arc<dyn open_triplestore::store::qlever::QleverEndpoint>),
        );
    s2.graph_store_put(Some(G), &ttl(&[1, 2]), RdfFormat::Turtle)
        .unwrap();
    s2.qlever().feed_once(&s2).unwrap();
    s2.update(&format!(
        "INSERT {{ GRAPH <{G}> {{ ?s <https://example.org/ql/q> ?o }} }} WHERE {{ GRAPH <{G}> {{ ?s <https://example.org/ql/p> ?o }} }}"
    ))
    .unwrap();
    let p = s2.qlever().feed_once(&s2).unwrap();
    assert!(p.replaced_graphs >= 1, "{p:?}");
    assert_eq!(
        rows(
            s2.query("SELECT ?s ?o WHERE { GRAPH ?g { ?s <https://example.org/ql/q> ?o } }")
                .unwrap()
        )
        .len(),
        2
    );
}

/// The route policy: `analytical` sends aggregates only, after the copies;
/// `off` sends nothing.
#[test]
fn the_route_policy_decides_what_goes_to_qlever() {
    let fake = Arc::new(FakeQlever::new());
    let s = store_with(Route::Analytical, &fake);
    s.graph_store_put(Some(G), &ttl(&[1, 2]), RdfFormat::Turtle)
        .unwrap();
    s.qlever().feed_once(&s).unwrap();
    let _ = s
        .query("SELECT ?s WHERE { GRAPH ?g { ?s ?p ?o } }")
        .unwrap();
    assert_eq!(
        s.qlever().status(&s).queries_served,
        0,
        "a row query is not analytical"
    );
    let n = rows(
        s.query("SELECT (COUNT(?s) AS ?n) WHERE { GRAPH ?g { ?s <https://example.org/ql/p> ?o } }")
            .unwrap(),
    );
    assert_eq!(n.len(), 1);
    assert_eq!(s.qlever().status(&s).queries_served, 1, "an aggregate is");
    let summary = s.telemetry().summary();
    assert_eq!(
        summary
            .queries
            .by_served
            .get("qlever")
            .copied()
            .unwrap_or(0),
        1,
        "{:?}",
        summary.queries.by_served
    );

    let off = store_with(Route::Off, &fake);
    off.graph_store_put(Some(G), &ttl(&[1]), RdfFormat::Turtle)
        .unwrap();
    assert!(!off.qlever().active());
    let _ = off
        .query("SELECT (COUNT(?s) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }")
        .unwrap();
    assert_eq!(off.qlever().status(&off).queries_served, 0);
}

#[tokio::test]
async fn the_status_route_is_for_admins() {
    let fake = Arc::new(FakeQlever::new());
    let (state, token) = admin_state_with_store(store_with(Route::All, &fake));
    let app = test_app(state.clone());
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/admin/qlever/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/admin/qlever/status")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v: Value = serde_json::from_str(&body_text(resp.into_body()).await).unwrap();
    assert_eq!(v["configured"], true, "{v}");
    assert_eq!(v["route"], "all", "{v}");
    assert_eq!(v["caught_up"], false, "{v}");
}
