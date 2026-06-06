//! The multi-core query path (subject-sharded in-memory mirror) must return
//! *exactly* the single-store answer for every decomposable aggregate / `ASK` it
//! accelerates — over the default graph and `FROM`-scoped named graphs — and must
//! invalidate after writes and fall back correctly over the triple-count cap.
//!
//! Each parity case runs the identical query on two stores loaded with identical
//! data: one with the mirror disabled (single-core) and one with an 8-shard mirror.
//! Equality across the two is the guarantee that parallelism never changes results.

use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::model::{GraphName, Literal, NamedNode, Quad, Term};
use oxigraph::sparql::QueryResults;

const G: &str = "http://example.org/g";
const EX: &str = "http://example.org/";
const XSD_INT: &str = "http://www.w3.org/2001/XMLSchema#integer";

/// `[from, to)` persons (name/age/type) as N-Quads in graph `graph`.
fn persons_range(from: usize, to: usize, graph: &str) -> String {
    let mut s = String::new();
    for i in from..to {
        s.push_str(&format!(
            "<{EX}p{i}> <{EX}name> \"Person {i}\" <{graph}> .\n"
        ));
        s.push_str(&format!(
            "<{EX}p{i}> <{EX}age> \"{}\"^^<{XSD_INT}> <{graph}> .\n",
            18 + i % 65
        ));
        s.push_str(&format!(
            "<{EX}p{i}> <{EX}type> <{EX}Type{}> <{graph}> .\n",
            i % 10
        ));
    }
    s
}

fn persons(n: usize, graph: &str) -> String {
    persons_range(0, n, graph)
}

fn store(enabled: bool, shards: usize, cap: usize, data: &str) -> TripleStore {
    let store = TripleStore::in_memory()
        .unwrap()
        .with_parallel_query(enabled, shards, cap);
    store.load_str(data, RdfFormat::NQuads, None).unwrap();
    store
}

/// Normalised, order-independent rendering of a query result.
fn normalize(r: QueryResults) -> Vec<String> {
    match r {
        QueryResults::Boolean(b) => vec![format!("ASK:{b}")],
        QueryResults::Solutions(sols) => {
            let vars: Vec<_> = sols.variables().to_vec();
            let mut rows: Vec<String> = sols
                .map(|s| {
                    let s = s.unwrap();
                    vars.iter()
                        .map(|v| s.get(v).map(|t| t.to_string()).unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join("\u{1}")
                })
                .collect();
            rows.sort();
            rows
        }
        QueryResults::Graph(_) => vec!["<graph>".into()],
    }
}

/// Assert the 8-shard parallel mirror matches the single-store answer.
fn assert_parity(data: &str, query: &str) {
    let single = store(false, 1, usize::MAX, data);
    let parallel = store(true, 8, 100_000_000, data);
    assert_eq!(
        normalize(parallel.query(query).unwrap()),
        normalize(single.query(query).unwrap()),
        "parallel mirror diverged from single store for: {query}"
    );
}

fn count_c(store: &TripleStore, q: &str) -> i64 {
    let QueryResults::Solutions(sols) = store.query(q).unwrap() else {
        panic!("expected solutions");
    };
    let sol = sols.into_iter().next().unwrap().unwrap();
    match sol.get("c") {
        Some(Term::Literal(lit)) => lit.value().parse().unwrap(),
        other => panic!("expected ?c literal, got {other:?}"),
    }
}

#[test]
fn parity_join_count_from_named_graph() {
    let data = persons(600, G);
    assert_parity(
        &data,
        &format!(
            "SELECT (COUNT(*) AS ?c) FROM <{G}> WHERE {{ ?s <{EX}name> ?n . ?s <{EX}age> ?a }}"
        ),
    );
}

#[test]
fn parity_filter_count_from_named_graph() {
    let data = persons(600, G);
    assert_parity(
        &data,
        &format!(
            "SELECT (COUNT(*) AS ?c) FROM <{G}> WHERE {{ ?s <{EX}age> ?a FILTER(?a >= 40 && ?a < 60) }}"
        ),
    );
}

#[test]
fn parity_single_pattern_count() {
    let data = persons(600, G);
    assert_parity(
        &data,
        &format!("SELECT (COUNT(*) AS ?c) FROM <{G}> WHERE {{ ?s <{EX}name> ?n }}"),
    );
}

#[test]
fn parity_ask_true_and_false() {
    let data = persons(600, G);
    assert_parity(
        &data,
        &format!("ASK FROM <{G}> {{ ?s <{EX}name> \"Person 7\" }}"),
    );
    assert_parity(
        &data,
        &format!("ASK FROM <{G}> {{ ?s <{EX}name> \"Nobody\" }}"),
    );
}

/// Persons as triples in the default graph (N-Triples).
fn persons_default(n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        s.push_str(&format!("<{EX}p{i}> <{EX}name> \"Person {i}\" .\n"));
        s.push_str(&format!(
            "<{EX}p{i}> <{EX}age> \"{}\"^^<{XSD_INT}> .\n",
            18 + i % 65
        ));
    }
    s
}

#[test]
fn parity_default_graph_join_count() {
    // Data loaded into the default graph (no FROM) — the common single-dataset case.
    let data = persons_default(500);
    assert_parity(
        &data,
        &format!("SELECT (COUNT(*) AS ?c) WHERE {{ ?s <{EX}name> ?n . ?s <{EX}age> ?a }}"),
    );
}

#[test]
fn parity_group_by_count() {
    let data = persons(600, G);
    // GROUP BY ?type with COUNT — the object key spans shards but its count sums.
    assert_parity(
        &data,
        &format!("SELECT ?t (COUNT(*) AS ?c) FROM <{G}> WHERE {{ ?s <{EX}type> ?t }} GROUP BY ?t"),
    );
    // GROUP BY over a subject-star join + filter.
    assert_parity(
        &data,
        &format!(
            "SELECT ?t (COUNT(?n) AS ?c) FROM <{G}> WHERE {{ ?s <{EX}type> ?t . ?s <{EX}name> ?n FILTER(?n != \"x\") }} GROUP BY ?t"
        ),
    );
}

#[test]
fn full_mirror_serves_nondecomposable_reads() {
    // Joins, GROUP BY with non-COUNT aggregates, ordered/limited row results — none
    // shard-decomposable — are served by the unsharded in-memory full mirror and
    // must equal the single store exactly (same engine + data, just in RAM).
    let data = persons(400, G);
    assert_parity(
        &data,
        &format!(
            "SELECT ?t (AVG(?a) AS ?avg) FROM <{G}> WHERE {{ ?s <{EX}type> ?t . ?s <{EX}age> ?a }} GROUP BY ?t"
        ),
    );
    assert_parity(
        &data,
        &format!(
            "SELECT ?t (SUM(?a) AS ?s) (MIN(?a) AS ?mn) (MAX(?a) AS ?mx) FROM <{G}> WHERE {{ ?s <{EX}type> ?t . ?s <{EX}age> ?a }} GROUP BY ?t"
        ),
    );
    assert_parity(
        &data,
        &format!("SELECT (COUNT(DISTINCT ?t) AS ?c) FROM <{G}> WHERE {{ ?s <{EX}type> ?t }}"),
    );
    assert_parity(
        &data,
        &format!(
            "SELECT ?n ?a FROM <{G}> WHERE {{ ?s <{EX}name> ?n . ?s <{EX}age> ?a }} ORDER BY ?a ?n LIMIT 40"
        ),
    );
}

#[test]
fn mirror_invalidates_after_write() {
    let store = store(true, 8, 100_000_000, &persons(300, G));
    let q = format!("SELECT (COUNT(*) AS ?c) FROM <{G}> WHERE {{ ?s <{EX}name> ?n }}");
    assert_eq!(count_c(&store, &q), 300, "initial count (warms the mirror)");

    // A write must invalidate the mirror, not serve a stale shard set.
    store
        .load_str(&persons_range(300, 305, G), RdfFormat::NQuads, None)
        .unwrap();
    assert_eq!(
        count_c(&store, &q),
        305,
        "mirror must reflect the post-write data"
    );

    // A DELETE/INSERT that preserves the triple count must still invalidate.
    store
        .update(&format!(
            "DELETE {{ GRAPH <{G}> {{ <{EX}p0> <{EX}name> ?n }} }} \
             INSERT {{ GRAPH <{G}> {{ <{EX}p0> <{EX}name> \"Renamed\" }} }} \
             WHERE  {{ GRAPH <{G}> {{ <{EX}p0> <{EX}name> ?n }} }}"
        ))
        .unwrap();
    assert_eq!(count_c(&store, &q), 305, "count unchanged after rename");
    let ask = format!("ASK FROM <{G}> {{ <{EX}p0> <{EX}name> \"Renamed\" }}");
    assert!(
        matches!(store.query(&ask).unwrap(), QueryResults::Boolean(true)),
        "rename must be visible through the (rebuilt) mirror"
    );
}

#[test]
fn count_alias_variable_is_preserved() {
    // The merged count must be readable by the query's OWN projection alias, not a
    // fixed `?c` — regression for the live LDP `count_members` (`COUNT(?m) AS ?n`),
    // which read `?n` and got nothing when the merge hardcoded `?c`.
    let store = store(true, 8, 100_000_000, &persons(200, G));
    let r = store
        .query(&format!(
            "SELECT (COUNT(?nm) AS ?members) FROM <{G}> WHERE {{ ?s <{EX}name> ?nm }}"
        ))
        .unwrap();
    let QueryResults::Solutions(sols) = r else {
        panic!("expected solutions");
    };
    let sol = sols.into_iter().next().unwrap().unwrap();
    match sol.get("members") {
        Some(Term::Literal(lit)) => assert_eq!(lit.value().parse::<i64>().unwrap(), 200),
        other => panic!("count must bind the query's alias ?members, got {other:?}"),
    }
}

#[test]
fn mirror_is_consulted_not_silently_bypassed() {
    // Proves the parallel path is actually used (a silent single-store fallback
    // would make the feature a no-op yet leave every parity test green). Warm the
    // mirror, then insert straight into the underlying Oxigraph store — bypassing
    // TripleStore's write tracking, so the mirror is deliberately NOT invalidated.
    // If the mirror is consulted it keeps serving the warm shards (count stays 100);
    // a bypass would already reflect the new triple (101).
    let store = store(true, 4, 100_000_000, &persons(100, G));
    let q = format!("SELECT (COUNT(*) AS ?c) FROM <{G}> WHERE {{ ?s <{EX}name> ?nm }}");
    assert_eq!(count_c(&store, &q), 100, "warms the mirror");

    let quad = Quad::new(
        NamedNode::new(format!("{EX}pNEW")).unwrap(),
        NamedNode::new(format!("{EX}name")).unwrap(),
        Literal::new_simple_literal("New"),
        GraphName::NamedNode(NamedNode::new(G).unwrap()),
    );
    store.store().insert(&quad).unwrap();
    assert_eq!(
        count_c(&store, &q),
        100,
        "warm mirror shards must be consulted, not silently bypassed"
    );

    // A tracked reindex invalidates the mirror, which then sees the new triple.
    store.rebuild_graph_index();
    assert_eq!(
        count_c(&store, &q),
        101,
        "mirror rebuilds after invalidation"
    );
}

#[test]
fn mirror_over_cap_falls_back_to_single_store() {
    // 300 persons ≈ 900 triples; a cap of 100 keeps the mirror disabled, and the
    // single store still answers correctly.
    let store = store(true, 8, 100, &persons(300, G));
    assert_eq!(
        count_c(
            &store,
            &format!("SELECT (COUNT(*) AS ?c) FROM <{G}> WHERE {{ ?s <{EX}name> ?n }}")
        ),
        300,
    );
}

#[test]
fn parity_holds_across_shard_counts() {
    let data = persons(400, G);
    let query = format!(
        "SELECT (COUNT(*) AS ?c) FROM <{G}> WHERE {{ ?s <{EX}name> ?n . ?s <{EX}age> ?a }}"
    );
    let expected = normalize(store(false, 1, usize::MAX, &data).query(&query).unwrap());
    for shards in [1usize, 2, 3, 7, 16] {
        let got = normalize(
            store(true, shards, 100_000_000, &data)
                .query(&query)
                .unwrap(),
        );
        assert_eq!(got, expected, "diverged at {shards} shards");
    }
}
