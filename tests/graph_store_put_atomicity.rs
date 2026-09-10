//! Graph Store Protocol `PUT` replaces a graph atomically.
//!
//! The replace used to be `clear_graph` (one transaction) followed by a bulk
//! load (another): a reader between the two saw an empty graph, and a crash
//! between the two left the graph empty for good (readiness audit, "Graph
//! Store PUT is not atomic"; the parse-before-clear half was closed earlier
//! and is pinned by `api_protocol_conformance::gsp_put_with_malformed_body_
//! leaves_graph_intact`). These tests race a reader against replaces on both
//! backends: the reader must always see the full old or the full new graph.

use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const G: &str = "http://example.org/replace";
const N: usize = 12_000;

/// `n` distinct quads; `salt` makes each payload different from the last so a
/// replace really replaces.
fn payload(n: usize, salt: &str) -> String {
    let mut s = String::with_capacity(n * 48);
    for i in 0..n {
        s.push_str(&format!(
            "<urn:r:s{}> <urn:r:p{}> \"{salt}{i}\" .\n",
            i / 8,
            i % 8
        ));
    }
    s
}

/// `COUNT(?s)`, not `COUNT(*)`: the latter is answered from the maintained
/// count index, which is bookkeeping *around* the store. This test observes
/// the store itself.
fn scan_count(store: &TripleStore) -> usize {
    let q = format!("SELECT (COUNT(?s) AS ?c) WHERE {{ GRAPH <{G}> {{ ?s ?p ?o }} }}");
    match store.query(&q).unwrap() {
        QueryResults::Solutions(sols) => {
            let sol = sols.into_iter().next().unwrap().unwrap();
            match sol.get("c") {
                Some(Term::Literal(l)) => l.value().parse().unwrap(),
                other => panic!("expected ?c, got {other:?}"),
            }
        }
        _ => panic!("SELECT expected"),
    }
}

/// A PUT replaces a graph in one transaction: a concurrent reader sees the old
/// contents or the new contents, never an empty or half-filled graph.
fn replace_never_exposes_an_empty_graph(store: TripleStore, label: &str) {
    // Result cache and parallel mirror off: every read is one plain
    // evaluation against the backend under test.
    let store = store
        .with_parallel_query(false, 1, usize::MAX)
        .with_query_cache(false, 1, 1);
    store
        .graph_store_put(Some(G), &payload(N, "a"), RdfFormat::NTriples)
        .unwrap();
    assert_eq!(scan_count(&store), N, "{label}: seed");

    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let s = store.clone();
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            for salt in ["b", "c", "d", "e", "f", "g"] {
                s.graph_store_put(Some(G), &payload(N, salt), RdfFormat::NTriples)
                    .unwrap();
            }
            stop.store(true, Ordering::Release);
        })
    };
    let mut reads = 0usize;
    while !stop.load(Ordering::Acquire) {
        reads += 1;
        let c = scan_count(&store);
        assert_eq!(
            c, N,
            "{label}: read #{reads} saw {c} quads in a graph being replaced — \
             the replace is not atomic (readers can observe an empty or partial graph)"
        );
    }
    writer.join().unwrap();
    assert!(
        reads >= 2,
        "{label}: the reader must have run during the replaces ({reads})"
    );
    assert_eq!(scan_count(&store), N, "{label}: final");
    assert_eq!(
        store.count_graph(Some(G)).unwrap(),
        N,
        "{label}: count index after replace"
    );
    assert!(
        matches!(
            store
                .query(&format!("ASK {{ GRAPH <{G}> {{ ?s ?p \"g0\" }} }}"))
                .unwrap(),
            QueryResults::Boolean(true)
        ),
        "{label}: the last payload is the live one"
    );
    println!("{label}: {reads} reads during 6 replaces of {N} quads, none empty or partial");
}

#[test]
fn gsp_put_replace_is_atomic_in_memory() {
    replace_never_exposes_an_empty_graph(TripleStore::in_memory().unwrap(), "in-memory");
}

#[test]
fn gsp_put_replace_is_atomic_on_rocksdb() {
    let tmp = tempfile::tempdir().unwrap();
    replace_never_exposes_an_empty_graph(TripleStore::open(tmp.path()).unwrap(), "rocksdb");
}

/// The replaced graph's count index reflects the DISTINCT quads of the new
/// payload, not old + new and not the raw line count.
#[test]
fn gsp_put_replace_counts_distinct_new_quads() {
    let store = TripleStore::in_memory().unwrap();
    store
        .graph_store_put(Some(G), &payload(500, "a"), RdfFormat::NTriples)
        .unwrap();
    assert_eq!(store.count_graph(Some(G)).unwrap(), 500);
    // 300 distinct quads, each listed twice.
    let mut dup = payload(300, "b");
    dup.push_str(&payload(300, "b"));
    store
        .graph_store_put(Some(G), &dup, RdfFormat::NTriples)
        .unwrap();
    assert_eq!(store.count_graph(Some(G)).unwrap(), 300);
    assert_eq!(scan_count(&store), 300);
}
