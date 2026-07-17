//! Query-result cache correctness: write-invalidation (never stale), the
//! non-deterministic-function guard (`UUID`/`NOW`/… never cached), over-cap
//! results streamed correctly, and per-query-string keying (the cross-tenant
//! safety property — different ACL-scoped strings never collide).

use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;

const EX: &str = "http://example.org/";

/// Cache on, mirror off (isolate the cache).
fn store(max_rows: usize) -> TripleStore {
    TripleStore::in_memory()
        .unwrap()
        .with_parallel_query(false, 1, usize::MAX)
        .with_query_cache(true, 256, max_rows)
}

fn count(s: &TripleStore, q: &str) -> i64 {
    let QueryResults::Solutions(sols) = s.query(q).unwrap() else {
        panic!("expected solutions");
    };
    let sol = sols.into_iter().next().unwrap().unwrap();
    match sol.get("c") {
        Some(Term::Literal(l)) => l.value().parse().unwrap(),
        other => panic!("expected ?c literal, got {other:?}"),
    }
}

fn rows(s: &TripleStore, q: &str) -> usize {
    match s.query(q).unwrap() {
        QueryResults::Solutions(sols) => sols.count(),
        _ => panic!("expected solutions"),
    }
}

#[test]
fn cache_never_serves_stale_after_write() {
    let s = store(10_000);
    s.load_str(
        &format!("<{EX}a> <{EX}p> \"x\" . <{EX}b> <{EX}p> \"y\" ."),
        RdfFormat::Turtle,
        None,
    )
    .unwrap();
    let q = "SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }";
    assert_eq!(count(&s, q), 2, "first count (populates cache)");
    assert_eq!(count(&s, q), 2, "cache hit must equal first result");

    // A write must invalidate — the next read recomputes.
    s.load_str(&format!("<{EX}c> <{EX}p> \"z\" ."), RdfFormat::Turtle, None)
        .unwrap();
    assert_eq!(
        count(&s, q),
        3,
        "cache must reflect the post-write data, not stale 2"
    );

    // A DELETE/INSERT that preserves the count must still invalidate row content.
    s.update(&format!(
        "DELETE {{ <{EX}a> <{EX}p> ?o }} INSERT {{ <{EX}a> <{EX}p> \"renamed\" }} WHERE {{ <{EX}a> <{EX}p> ?o }}"
    ))
    .unwrap();
    assert_eq!(count(&s, q), 3, "count unchanged");
    assert!(
        matches!(
            s.query(&format!("ASK {{ <{EX}a> <{EX}p> \"renamed\" }}"))
                .unwrap(),
            QueryResults::Boolean(true)
        ),
        "the rename must be visible (ASK not served stale)"
    );
}

#[test]
fn nondeterministic_query_is_not_cached() {
    let s = store(10_000);
    s.load_str(&format!("<{EX}a> <{EX}p> \"x\" ."), RdfFormat::Turtle, None)
        .unwrap();
    // UUID() yields a fresh value each evaluation; if it were cached, both calls
    // would return the SAME uuid. They must differ.
    let q = "SELECT (UUID() AS ?u) WHERE { ?s ?p ?o }";
    let get = |q: &str| -> String {
        let QueryResults::Solutions(sols) = s.query(q).unwrap() else {
            panic!()
        };
        let sol = sols.into_iter().next().unwrap().unwrap();
        sol.get("u").unwrap().to_string()
    };
    let a = get(q);
    let b = get(q);
    assert_ne!(a, b, "UUID() must not be cached (it is non-deterministic)");
}

#[test]
fn over_cap_results_are_correct_and_not_truncated() {
    // Cap of 2 rows, but the query returns 5 — must stream the full result.
    let s = store(2);
    let mut ttl = String::new();
    for i in 0..5 {
        ttl.push_str(&format!("<{EX}s{i}> <{EX}p> \"{i}\" .\n"));
    }
    s.load_str(&ttl, RdfFormat::Turtle, None).unwrap();
    let q = "SELECT ?s WHERE { ?s ?p ?o }";
    assert_eq!(
        rows(&s, q),
        5,
        "first call returns all rows despite the cap"
    );
    assert_eq!(
        rows(&s, q),
        5,
        "second call (uncached, recomputed) also all rows"
    );
}

#[test]
fn distinct_query_strings_keyed_independently() {
    // Different ACL-scoped queries (different FROM) must never collide — the
    // cross-tenant safety property, exercised at the keying level.
    let s = store(10_000);
    s.load_str(
        &format!("<{EX}a> <{EX}p> \"x\" . <{EX}b> <{EX}p> \"y\" ."),
        RdfFormat::Turtle,
        Some("urn:g1"),
    )
    .unwrap();
    s.load_str(
        &format!("<{EX}c> <{EX}p> \"x\" . <{EX}d> <{EX}p> \"y\" . <{EX}e> <{EX}p> \"z\" ."),
        RdfFormat::Turtle,
        Some("urn:g2"),
    )
    .unwrap();
    let q1 = "SELECT (COUNT(*) AS ?c) FROM <urn:g1> WHERE { ?s ?p ?o }";
    let q2 = "SELECT (COUNT(*) AS ?c) FROM <urn:g2> WHERE { ?s ?p ?o }";
    assert_eq!(count(&s, q1), 2);
    assert_eq!(count(&s, q2), 3);
    // Re-query (now cached) — must still be each graph's own count, no collision.
    assert_eq!(count(&s, q1), 2);
    assert_eq!(count(&s, q2), 3);
}

#[test]
fn cache_disabled_still_correct() {
    let s = TripleStore::in_memory()
        .unwrap()
        .with_query_cache(false, 256, 10_000);
    s.load_str(&format!("<{EX}a> <{EX}p> \"x\" ."), RdfFormat::Turtle, None)
        .unwrap();
    let q = "SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }";
    assert_eq!(count(&s, q), 1);
    s.load_str(&format!("<{EX}b> <{EX}p> \"y\" ."), RdfFormat::Turtle, None)
        .unwrap();
    assert_eq!(count(&s, q), 2);
}
