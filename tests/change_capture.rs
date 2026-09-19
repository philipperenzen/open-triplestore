//! Per-quad change capture with a durable cursor (P2): every mutating
//! primitive of the store writes one row per graph it touched, carrying the
//! net delta as N-Quads where it can bound it, exact counts where it cannot
//! afford the payload, and an honest `unknown` where it cannot know — and a
//! sequence number handed out in commit order that a consumer can resume
//! from. docs/notes/delta-versioning-design.md §2 and §4.3.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::store::changes::{
    Extent, State, WriteContext, WriteContextGuard, DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN,
};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::model::{GraphName, NamedNode, Quad};
use serde_json::{json, Value};
use tower::ServiceExt as _;

const G: &str = "https://example.org/cc/g";
const G2: &str = "https://example.org/cc/g2";

/// Capture is off by default; every test here turns it on.
fn store() -> TripleStore {
    TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD)
}

fn quad(n: u32, g: &str) -> Quad {
    Quad::new(
        NamedNode::new_unchecked(format!("https://example.org/cc/s{n}")),
        NamedNode::new_unchecked("https://example.org/cc/p"),
        NamedNode::new_unchecked(format!("https://example.org/cc/o{n}")),
        GraphName::NamedNode(NamedNode::new_unchecked(g)),
    )
}

fn ttl(ns: &[u32]) -> String {
    ns.iter()
        .map(|n| format!("<https://example.org/cc/s{n}> <https://example.org/cc/p> <https://example.org/cc/o{n}> ."))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn ground_updates_are_full_rows_with_exact_counts() {
    let s = store();
    s.update(&format!(
        "INSERT DATA {{ GRAPH <{G}> {{ <https://example.org/cc/s1> <https://example.org/cc/p> <https://example.org/cc/o1> . <https://example.org/cc/s2> <https://example.org/cc/p> <https://example.org/cc/o2> }} }}"
    ))
    .unwrap();
    let rows = s.changes().rows_after(0, 10);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let r = &rows[0];
    assert_eq!(r.seq, Some(1));
    assert_eq!(r.graph_iri.as_deref(), Some(G));
    assert_eq!(r.scope, "graph");
    assert_eq!(r.extent, Extent::Full);
    assert_eq!(r.state, State::Committed);
    assert_eq!((r.added_n, r.removed_n), (2, 0));
    assert_eq!(r.post_count, Some(2));
    assert!(
        r.added
            .as_deref()
            .unwrap()
            .contains("<https://example.org/cc/s1>"),
        "{r:?}"
    );
    assert_eq!(r.origin, "update");

    // A delete of one present and one absent quad nets to one removal.
    s.update(&format!(
        "DELETE DATA {{ GRAPH <{G}> {{ <https://example.org/cc/s1> <https://example.org/cc/p> <https://example.org/cc/o1> . <https://example.org/cc/s9> <https://example.org/cc/p> <https://example.org/cc/o9> }} }}"
    ))
    .unwrap();
    let r = &s.changes().rows_after(1, 10)[0];
    assert_eq!(r.seq, Some(2));
    assert_eq!((r.added_n, r.removed_n), (0, 1));
    assert_eq!(r.post_count, Some(1));
    assert_eq!(
        s.graph_count_cached(Some(G)),
        Some(1),
        "the count index agrees"
    );
}

#[test]
fn a_where_update_is_scanned_into_a_full_net_row() {
    let s = store();
    s.load_str(&ttl(&[1, 2, 3]), RdfFormat::Turtle, Some(G))
        .unwrap();
    let before = s.changes().last_seq();
    s.update(&format!(
        "DELETE {{ GRAPH <{G}> {{ ?s <https://example.org/cc/p> ?o }} }} \
         INSERT {{ GRAPH <{G}> {{ ?s <https://example.org/cc/q> ?o }} }} \
         WHERE {{ GRAPH <{G}> {{ ?s <https://example.org/cc/p> ?o }} }}"
    ))
    .unwrap();
    let rows = s.changes().rows_after(before, 10);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let r = &rows[0];
    assert_eq!(r.extent, Extent::Full);
    assert_eq!((r.added_n, r.removed_n), (3, 3));
    assert_eq!(r.post_count, Some(3));
    assert!(r.removed.as_deref().unwrap().contains("/cc/p>"));
    assert!(r.added.as_deref().unwrap().contains("/cc/q>"));
}

#[test]
fn an_update_whose_targets_cannot_be_bounded_is_an_unknown_store_row() {
    let s = store();
    s.load_str(&ttl(&[1]), RdfFormat::Turtle, Some(G)).unwrap();
    let before = s.changes().last_seq();
    s.update("DELETE WHERE { GRAPH ?g { <https://example.org/cc/s1> ?p ?o } }")
        .unwrap();
    let rows = s.changes().rows_after(before, 10);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].scope, "store");
    assert_eq!(rows[0].extent, Extent::Unknown);
    assert_eq!(rows[0].state, State::Unknown);
    assert!(rows[0].seq.is_some(), "an unknown row is still sequenced");
}

#[test]
fn a_scan_over_the_cap_is_an_unknown_graph_row_not_a_guess() {
    let s = store().with_change_capture(2, 250_000);
    s.load_str(&ttl(&[1, 2, 3, 4, 5]), RdfFormat::Turtle, Some(G))
        .unwrap();
    let before = s.changes().last_seq();
    s.update(&format!(
        "DELETE {{ GRAPH <{G}> {{ ?s ?p ?o }} }} WHERE {{ GRAPH <{G}> {{ ?s ?p ?o }} }}"
    ))
    .unwrap();
    let rows = s.changes().rows_after(before, 10);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].graph_iri.as_deref(), Some(G));
    assert_eq!(rows[0].extent, Extent::Unknown);
    assert_eq!(s.graph_count_cached(Some(G)), Some(0));
}

#[test]
fn a_batch_is_one_transaction_with_one_row_per_graph_and_aborts_whole() {
    let s = store();
    let results = s
        .batch_update(&[
            format!("INSERT DATA {{ GRAPH <{G}> {{ <https://example.org/cc/s1> <https://example.org/cc/p> <https://example.org/cc/o1> }} }}"),
            format!("INSERT DATA {{ GRAPH <{G2}> {{ <https://example.org/cc/s2> <https://example.org/cc/p> <https://example.org/cc/o2> }} }}"),
        ])
        .unwrap();
    assert_eq!(results.len(), 2);
    let rows = s.changes().rows_after(0, 10);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].txn, rows[1].txn);
    assert_eq!(rows[0].seq, Some(1));
    assert_eq!(rows[1].seq, Some(2));
    assert_eq!(rows[0].origin, "batch_update");
    // A batch that fails at execution applies nothing and records nothing.
    let results = s
        .batch_update(&[
            format!("INSERT DATA {{ GRAPH <{G}> {{ <https://example.org/cc/s3> <https://example.org/cc/p> <https://example.org/cc/o3> }} }}"),
            "DROP GRAPH <https://example.org/cc/missing>".to_string(),
        ])
        .unwrap();
    assert!(matches!(
        results[1],
        open_triplestore::store::engine::BatchStatement::Failed(_)
    ));
    assert_eq!(
        s.changes().last_seq(),
        2,
        "no row for the rolled-back batch"
    );
    assert_eq!(
        s.changes().status().pending,
        0,
        "the intent rows were aborted"
    );
}

#[test]
fn graph_store_put_records_a_replace_as_the_net_diff() {
    let s = store();
    s.graph_store_put(Some(G), &ttl(&[1, 2]), RdfFormat::Turtle)
        .unwrap();
    let rows = s.changes().rows_after(0, 10);
    assert_eq!(
        rows.len(),
        1,
        "a first PUT (the empty-target path) is one row: {rows:?}"
    );
    assert_eq!((rows[0].added_n, rows[0].removed_n), (2, 0));
    assert_eq!(rows[0].post_count, Some(2));
    // Replace: b stays, a goes, c comes.
    s.graph_store_put(Some(G), &ttl(&[2, 3]), RdfFormat::Turtle)
        .unwrap();
    let rows = s.changes().rows_after(1, 10);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let r = &rows[0];
    assert_eq!(r.extent, Extent::Full);
    assert_eq!((r.added_n, r.removed_n), (1, 1));
    assert!(r.added.as_deref().unwrap().contains("/cc/s3>"));
    assert!(r.removed.as_deref().unwrap().contains("/cc/s1>"));
    assert_eq!(r.post_count, Some(2));
    assert_eq!(r.origin, "graph_store_put");
}

#[test]
fn graph_store_post_records_only_the_quads_that_were_new() {
    let s = store();
    s.graph_store_put(Some(G), &ttl(&[1, 2]), RdfFormat::Turtle)
        .unwrap();
    s.graph_store_post(Some(G), &ttl(&[2, 3]), RdfFormat::Turtle)
        .unwrap();
    let rows = s.changes().rows_after(1, 10);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].added_n, rows[0].removed_n), (1, 0));
    assert_eq!(rows[0].post_count, Some(3));
    assert!(rows[0].added.as_deref().unwrap().contains("/cc/s3>"));
}

#[test]
fn deleting_a_graph_records_every_removed_quad() {
    let s = store();
    s.graph_store_put(Some(G), &ttl(&[1, 2]), RdfFormat::Turtle)
        .unwrap();
    s.graph_store_delete(Some(G)).unwrap();
    let rows = s.changes().rows_after(1, 10);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].added_n, rows[0].removed_n), (0, 2));
    assert_eq!(rows[0].post_count, Some(0));
    assert_eq!(rows[0].extent, Extent::Full);
}

#[test]
fn bulk_primitives_record_one_row_per_graph() {
    let s = store();
    s.bulk_insert_quads(
        vec![quad(1, G), quad(2, G), quad(3, G2), quad(3, G2)],
        &[G.to_string(), G2.to_string()],
    )
    .unwrap();
    let rows = s.changes().rows_after(0, 10);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let by_graph = |g: &str| {
        rows.iter()
            .find(|r| r.graph_iri.as_deref() == Some(g))
            .unwrap()
    };
    assert_eq!(by_graph(G).added_n, 2);
    assert_eq!(by_graph(G2).added_n, 1, "the duplicate is one quad");
    assert_eq!(by_graph(G2).post_count, Some(1));
    // Re-inserting a present quad is a row that changed nothing.
    s.bulk_insert_quads(vec![quad(1, G)], &[G.to_string()])
        .unwrap();
    assert_eq!(s.changes().rows_after(2, 10)[0].added_n, 0);
    s.bulk_delete_graphs(&[G, G2]).unwrap();
    let rows = s.changes().rows_after(3, 10);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(by_graph(G).graph_iri.as_deref(), Some(G));
    assert_eq!(rows.iter().map(|r| r.removed_n).sum::<i64>(), 3);
    assert!(rows.iter().all(|r| r.post_count == Some(0)));
}

#[test]
fn store_quad_probes_before_it_counts() {
    let s = store();
    s.store_quad(quad(1, G)).unwrap();
    s.store_quad(quad(1, G)).unwrap();
    let rows = s.changes().rows_after(0, 10);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].added_n, 1);
    assert_eq!(rows[1].added_n, 0);
    assert_eq!(rows[1].post_count, Some(1));
}

#[test]
fn a_streamed_default_graph_load_is_an_honest_unknown() {
    let s = store();
    s.load_str(&ttl(&[1, 2]), RdfFormat::Turtle, None).unwrap();
    let rows = s.changes().rows_after(0, 10);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].extent, Extent::Unknown);
    assert_eq!(rows[0].scope, "store");
    assert_eq!(rows[0].origin, "load_reader");
}

#[test]
fn sequence_numbers_are_dense_and_follow_commit_order() {
    let s = store();
    for i in 0..5u32 {
        s.store_quad(quad(i, G)).unwrap();
    }
    s.graph_store_put(Some(G2), &ttl(&[9]), RdfFormat::Turtle)
        .unwrap();
    let rows = s.changes().rows_after(0, 100);
    let seqs: Vec<i64> = rows.iter().map(|r| r.seq.unwrap()).collect();
    assert_eq!(seqs, (1..=6).collect::<Vec<_>>());
    assert_eq!(s.changes().last_seq(), 6);
    assert_eq!(s.changes().status().next_seq, 7);
    assert_eq!(s.changes().status().pending, 0);
    // Resuming from a cursor yields exactly what came after it.
    assert_eq!(s.changes().rows_after(4, 100).len(), 2);
}

#[test]
fn the_write_context_lands_on_the_rows() {
    let s = store();
    {
        let _ctx = WriteContextGuard::set(WriteContext {
            actor_iri: Some("https://example.org/users/alice".into()),
            commit_iri: Some("https://example.org/commits/1".into()),
            kind: Some("SparqlUpdate".into()),
        });
        s.store_quad(quad(1, G)).unwrap();
    }
    s.store_quad(quad(2, G)).unwrap();
    let rows = s.changes().rows_after(0, 10);
    assert_eq!(
        rows[0].actor_iri.as_deref(),
        Some("https://example.org/users/alice")
    );
    assert_eq!(rows[0].kind.as_deref(), Some("SparqlUpdate"));
    assert!(
        rows[1].actor_iri.is_none(),
        "the guard restored the context"
    );
}

#[test]
fn the_off_switch_records_nothing_and_changes_nothing_else() {
    let s = store().with_change_capture_disabled();
    s.store_quad(quad(1, G)).unwrap();
    s.update(&format!(
        "INSERT DATA {{ GRAPH <{G}> {{ <https://example.org/cc/s2> <https://example.org/cc/p> <https://example.org/cc/o2> }} }}"
    ))
    .unwrap();
    assert!(!s.changes().enabled());
    assert!(s.changes().rows_after(0, 10).is_empty());
    assert_eq!(s.graph_count_cached(Some(G)), Some(2));
}

/// A persistent store reopens with its log: rows survive, a crash between
/// the data commit and the row is resolved by probing, and a write that
/// bypassed the log (the raw oxigraph store) is caught by the count check.
#[test]
fn reopening_resolves_pending_rows_and_reconciles_counts() {
    std::env::set_var("OTS_CHANGE_CAPTURE", "on");
    let dir = tempfile::tempdir().unwrap();
    {
        let s = TripleStore::open(dir.path()).unwrap();
        s.graph_store_put(Some(G), &ttl(&[1, 2]), RdfFormat::Turtle)
            .unwrap();
        assert_eq!(s.changes().last_seq(), 1);
        // A write nobody recorded: the raw store, behind the log's back.
        s.store().insert(quad(7, G).as_ref()).unwrap();
        // A crash after the commit, before the row: an intent left pending.
        let intent = s
            .changes()
            .begin("test", Some(&[Some(G.to_string())]), &|_| Some(2))
            .expect("recorded");
        drop(intent);
        assert_eq!(s.changes().status().pending, 1);
    }
    let s = TripleStore::open(dir.path()).unwrap();
    let st = s.changes().status();
    assert_eq!(st.pending, 0, "{st:?}");
    let rows = s.changes().rows_after(1, 10);
    // The pending row without a payload resolved to unknown; the count
    // disagreement (3 in the store, 2 recorded) added a reconcile row.
    assert!(
        rows.iter()
            .any(|r| r.state == State::Unknown && r.origin == "test"),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.origin == "reconcile" && r.post_count == Some(3)),
        "{rows:?}"
    );
    assert!(!st.epoch.is_empty());
    // And the next write continues the sequence.
    s.store_quad(quad(8, G)).unwrap();
    assert!(s.changes().last_seq() > rows.last().unwrap().seq.unwrap());
}

/// The route's primitive (`update_targeted_delta`) and the dataset-scoped one
/// record exactly as the plain `update` does: an exact row for a ground
/// update, a scanned net row for a WHERE update.
#[test]
fn the_targeted_and_scoped_primitives_record_too() {
    let s = store();
    let ins = format!(
        "INSERT DATA {{ GRAPH <{G}> {{ <https://example.org/cc/s1> <https://example.org/cc/p> <https://example.org/cc/o1> }} }}"
    );
    s.update_targeted_delta(&ins, &[G.to_string()], false)
        .unwrap();
    let rows = s.changes().rows_after(0, 10);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].origin, "update_targeted");
    assert_eq!(rows[0].extent, Extent::Full);
    assert_eq!((rows[0].added_n, rows[0].post_count), (1, Some(1)));
    // Re-inserting the same quad is a no-op: no row, no sequence number.
    s.update_targeted_delta(&ins, &[G.to_string()], false)
        .unwrap();
    assert_eq!(s.changes().last_seq(), 1);
    // A WHERE update with a static target is scanned into a net row.
    s.update_targeted_delta(
        &format!(
            "INSERT {{ GRAPH <{G}> {{ ?s <https://example.org/cc/q> ?o }} }} WHERE {{ GRAPH <{G}> {{ ?s <https://example.org/cc/p> ?o }} }}"
        ),
        &[G.to_string()],
        false,
    )
    .unwrap();
    let rows = s.changes().rows_after(1, 10);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].added_n, rows[0].post_count), (1, Some(2)));
    // The dataset-scoped primitive: the scope is the read side (`USING`);
    // the write side is the text's, here the store's default graph.
    s.update_scoped(
        "INSERT DATA { <https://example.org/cc/s3> <https://example.org/cc/p> <https://example.org/cc/o3> }",
        &[G.to_string()],
    )
    .unwrap();
    let rows = s.changes().rows_after(2, 10);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].origin, "update_scoped");
    assert_eq!(rows[0].graph_iri, None, "the default graph: {rows:?}");
    assert_eq!((rows[0].added_n, rows[0].post_count), (1, Some(1)));
}

// ─── The admin surface ────────────────────────────────────────────────────

async fn call(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let st = resp.status();
    let txt = body_text(resp.into_body()).await;
    (st, serde_json::from_str(&txt).unwrap_or(Value::String(txt)))
}

#[tokio::test]
async fn the_change_log_is_read_and_bookmarked_through_the_admin_api() {
    std::env::set_var("OTS_CHANGE_CAPTURE", "on");
    let (state, token) = admin_state();
    state
        .store
        .graph_store_put(Some(G), &ttl(&[1, 2]), RdfFormat::Turtle)
        .unwrap();
    state.store.store_quad(quad(3, G2)).unwrap();
    let app = test_app(state.clone());

    let (st, v) = call(
        &app,
        Method::GET,
        "/api/admin/changes?after=0&limit=10",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{v}");
    assert_eq!(rows[0]["seq"], 1);
    assert_eq!(rows[0]["graph_iri"], G);
    assert_eq!(rows[0]["extent"], "full");
    assert!(rows[0]["added"].as_str().unwrap().contains("/cc/s1>"));
    assert_eq!(v["next_after"], 2);
    assert!(v["epoch"].as_str().map(|e| !e.is_empty()).unwrap_or(false));
    // Paging from a cursor value.
    let (_, v) = call(
        &app,
        Method::GET,
        "/api/admin/changes?after=1",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(v["rows"].as_array().unwrap().len(), 1);
    // Filtering by graph.
    let (_, v) = call(
        &app,
        Method::GET,
        &format!("/api/admin/changes?after=0&graph={}", url_encode(G2)),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(v["rows"].as_array().unwrap().len(), 1, "{v}");

    let (st, v) = call(
        &app,
        Method::GET,
        "/api/admin/changes/status",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["enabled"], true);
    assert_eq!(v["newest_seq"], 2);
    assert_eq!(v["pending"], 0);

    // A consumer bookmarks its position; the bookmark pins retention.
    let (st, v) = call(
        &app,
        Method::PUT,
        "/api/admin/changes/cursors/replica-a",
        Some(&token),
        Some(json!({ "seq": 2 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["seq"], 2);
    let (_, v) = call(
        &app,
        Method::GET,
        "/api/admin/changes/status",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(v["cursors"][0]["name"], "replica-a", "{v}");
    let (st, _) = call(
        &app,
        Method::DELETE,
        "/api/admin/changes/cursors/replica-a",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(
        &app,
        Method::DELETE,
        "/api/admin/changes/cursors/replica-a",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Admins only.
    let (st, _) = call(&app, Method::GET, "/api/admin/changes", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}
