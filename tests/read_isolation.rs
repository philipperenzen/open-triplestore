//! Per-query read isolation.
//!
//! `opengraph/src/mvcc.rs` claimed that oxigraph takes a RocksDB snapshot per
//! *iterator*, so a SELECT joining two triple patterns could observe a write
//! that committed between the evaluation of the two patterns (readiness audit,
//! "documented per-query read isolation gap is unresolved and untracked by any
//! test"). This suite tries to observe exactly that: a writer flips two
//! properties of every subject together in ONE update (one transaction) while
//! a reader joins the two properties and asks for subjects where they
//! disagree. Any row is a torn read.
//!
//! Oxigraph 0.5 binds one storage snapshot per query (`PreparedSparqlQuery::
//! on_store` calls `storage().snapshot()` once), so no row is ever expected —
//! on the in-memory backend and on RocksDB alike. The test pins that.

use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Subjects per flip: enough rows that a query spans many index probes.
const SUBJECTS: usize = 400;
/// How long the writer keeps flipping.
const WRITE_FOR: Duration = Duration::from_secs(3);

fn seed(store: &TripleStore) {
    let mut ttl = String::with_capacity(SUBJECTS * 40);
    for i in 0..SUBJECTS {
        ttl.push_str(&format!("<urn:s{i}> <urn:p> 0 ; <urn:q> 0 .\n"));
    }
    store.load_str(&ttl, RdfFormat::Turtle, None).unwrap();
}

/// One update = one transaction: every subject's `<urn:p>` and `<urn:q>` move
/// from `from` to `to` together, so at any committed state they agree.
fn flip(from: i64, to: i64) -> String {
    format!(
        "DELETE {{ ?s <urn:p> {from} . ?s <urn:q> {from} }} \
         INSERT {{ ?s <urn:p> {to} . ?s <urn:q> {to} }} \
         WHERE {{ ?s <urn:p> {from} ; <urn:q> {from} }}"
    )
}

/// Subjects whose two properties disagree — a read that mixed two states.
fn torn_rows(store: &TripleStore) -> usize {
    match store
        .query("SELECT ?s WHERE { ?s <urn:p> ?v . ?s <urn:q> ?w . FILTER(?v != ?w) }")
        .unwrap()
    {
        QueryResults::Solutions(sols) => sols.count(),
        _ => panic!("SELECT expected"),
    }
}

fn no_torn_reads(store: TripleStore, label: &str) {
    // Result cache and parallel mirror off: every read is one plain evaluation
    // against the backend under test.
    let store = store
        .with_parallel_query(false, 1, usize::MAX)
        .with_query_cache(false, 1, 1);
    seed(&store);
    assert_eq!(torn_rows(&store), 0, "{label}: seed must agree");

    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let s = store.clone();
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let deadline = Instant::now() + WRITE_FOR;
            let mut cur = 0i64;
            let mut flips = 0usize;
            while Instant::now() < deadline {
                let next = 1 - cur;
                s.update(&flip(cur, next)).unwrap();
                cur = next;
                flips += 1;
            }
            stop.store(true, Ordering::Release);
            flips
        })
    };

    let mut reads = 0usize;
    while !stop.load(Ordering::Acquire) {
        reads += 1;
        let torn = torn_rows(&store);
        assert_eq!(
            torn, 0,
            "{label}: read #{reads} saw {torn} subjects whose <urn:p> and <urn:q> disagree — \
             a write landed between two patterns of one SELECT (torn read)"
        );
    }
    let flips = writer.join().unwrap();
    assert!(
        flips >= 2 && reads >= 2,
        "{label}: the race needs both sides to run ({flips} flips, {reads} reads)"
    );
    println!("{label}: {reads} reads interleaved with {flips} atomic flips, 0 torn");
}

#[test]
fn a_select_never_observes_a_partially_applied_write_in_memory() {
    no_torn_reads(TripleStore::in_memory().unwrap(), "in-memory");
}

#[test]
fn a_select_never_observes_a_partially_applied_write_on_rocksdb() {
    let tmp = tempfile::tempdir().unwrap();
    no_torn_reads(TripleStore::open(tmp.path()).unwrap(), "rocksdb");
}
