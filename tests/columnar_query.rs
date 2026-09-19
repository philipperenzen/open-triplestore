//! The columnar copy on the live query path (P5): the store answers the
//! queries the evaluator accepts from the dictionary-encoded copy — the
//! telemetry says so — and the answers equal the engine's, taken from a
//! store whose accelerator is off. Declined shapes still reach the engine.

use std::collections::BTreeSet;

use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;

fn persons(n: usize) -> String {
    let mut s = String::from(
        "@prefix ex: <http://example.org/> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n",
    );
    for i in 0..n {
        let kind = i % 10;
        let age = 18 + (i % 65);
        let score = (i as f64 * 7.13) % 100.0;
        s.push_str(&format!(
            "ex:p{i} ex:name \"Person {i}\" ; ex:age {age} ; ex:type ex:Type{kind} ; ex:score {score:.2} ; ex:email \"person{i}@example.org\" .\n"
        ));
        if i > 0 {
            s.push_str(&format!("ex:p{i} ex:knows ex:p{} .\n", i - 1));
        }
    }
    s
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
        QueryResults::Graph(g) => g
            .map(|t| {
                let t = t.unwrap();
                vec![Some(t.to_string())]
            })
            .collect(),
    }
}

const QUERIES: &[&str] = &[
    "SELECT ?s ?name WHERE { ?s <http://example.org/name> ?name }",
    "SELECT ?name ?age WHERE { ?s <http://example.org/name> ?name ; <http://example.org/age> ?age }",
    "SELECT ?name ?age ?type WHERE { ?s <http://example.org/name> ?name ; <http://example.org/age> ?age ; <http://example.org/type> ?type }",
    "SELECT ?s WHERE { ?s <http://example.org/age> ?age FILTER(?age > 60) }",
    "SELECT ?s WHERE { ?s <http://example.org/email> ?e FILTER(REGEX(?e, \"^person1\")) }",
    "SELECT ?name ?k WHERE { ?s <http://example.org/name> ?name OPTIONAL { ?s <http://example.org/knows> ?k } }",
    "SELECT ?type (COUNT(?s) AS ?cnt) (MIN(?age) AS ?youngest) WHERE { ?s <http://example.org/type> ?type ; <http://example.org/age> ?age } GROUP BY ?type",
    "SELECT ?type (GROUP_CONCAT(?name; separator=\",\") AS ?names) WHERE { ?s <http://example.org/type> ?type ; <http://example.org/name> ?name } GROUP BY ?type",
    "SELECT (COUNT(DISTINCT ?type) AS ?c) WHERE { ?s <http://example.org/type> ?type }",
    "SELECT ?a ?c WHERE { ?a <http://example.org/knows>/<http://example.org/knows> ?c }",
    "SELECT ?s ?age WHERE { ?s <http://example.org/age> ?age } ORDER BY DESC(?age) ?s LIMIT 5",
    "ASK { ?s <http://example.org/name> \"Person 7\" }",
    "CONSTRUCT { ?s <http://example.org/label> ?name } WHERE { ?s <http://example.org/name> ?name FILTER(?s = <http://example.org/p3>) }",
];

/// Every accepted query is answered by the columnar copy, and answers the
/// same as the engine alone.
#[test]
fn the_columnar_copy_answers_accepted_queries_like_the_engine() {
    let data = persons(200);
    let fast = TripleStore::in_memory()
        .unwrap()
        .with_query_cache(false, 0, 0)
        .with_parallel_query(true, 4, 10_000_000)
        .with_parallel_rebuild_quiet_ms(0);
    fast.load_str(&data, RdfFormat::Turtle, None).unwrap();
    let plain = TripleStore::in_memory()
        .unwrap()
        .with_query_cache(false, 0, 0)
        .with_parallel_query(false, 1, 0);
    plain.load_str(&data, RdfFormat::Turtle, None).unwrap();
    // The first query after the load builds the copies (no quiet window).
    let _ = fast.query("SELECT ?s WHERE { ?s ?p ?o } LIMIT 1").unwrap();
    for q in QUERIES {
        let a = rows(fast.query(q).unwrap());
        let b = rows(plain.query(q).unwrap());
        if q.contains("GROUP_CONCAT") {
            // The concatenation order follows the solution order, which SPARQL
            // leaves open: compare the parts per group.
            let split = |set: BTreeSet<Vec<Option<String>>>| -> BTreeSet<Vec<Option<String>>> {
                set.into_iter()
                    .map(|mut r| {
                        if let Some(Some(v)) = r.get_mut(1) {
                            let mut parts: Vec<&str> = v.trim_matches('"').split(',').collect();
                            parts.sort();
                            *v = parts.join(",");
                        }
                        r
                    })
                    .collect()
            };
            assert_eq!(split(a), split(b), "{q}");
        } else {
            assert_eq!(a, b, "{q}");
        }
    }
    // Which exit answered, and why. The grouped aggregates go to the shards,
    // which decompose them across cores and are consulted first; the REGEX
    // filter is declined and the full copy takes it. Both are by design — see
    // docs/performance.md, "The columnar copy". Everything else is the
    // columnar copy's, and nothing falls all the way through to the engine.
    let by = &fast.telemetry().summary().queries.by_served;
    let served = |exit: &str| by.get(exit).copied().unwrap_or(0);
    assert_eq!(served("engine"), 0, "a query reached the engine: {by:?}");
    assert!(
        served("columnar") >= 9,
        "the columnar copy answered only {} of {} ({by:?})",
        served("columnar"),
        QUERIES.len(),
    );
    assert!(
        served("shards") >= 3,
        "the aggregates should go to the shards: {by:?}"
    );
    assert!(
        served("full_copy") >= 1,
        "the declined REGEX filter should reach the full copy: {by:?}"
    );
}

/// A shape the evaluator declines still reaches the engine, and a write
/// dirties the copy so no stale answer leaks.
#[test]
fn declined_shapes_and_writes_fall_through() {
    let store = TripleStore::in_memory()
        .unwrap()
        .with_query_cache(false, 0, 0)
        .with_parallel_query(true, 2, 10_000_000)
        .with_parallel_rebuild_quiet_ms(0);
    store
        .load_str(&persons(20), RdfFormat::Turtle, None)
        .unwrap();
    let path = "SELECT ?a ?c WHERE { ?a <http://example.org/knows>+ ?c }";
    let n = rows(store.query(path).unwrap()).len();
    assert!(n > 20, "{n}");
    let before = store.telemetry().summary().queries.by_served;
    assert_eq!(
        before.get("columnar").copied().unwrap_or(0),
        0,
        "{before:?}"
    );
    // A write, then the same simple query: the answer includes the new quad.
    store
        .update("INSERT DATA { <http://example.org/p99> <http://example.org/name> \"Person 99\" }")
        .unwrap();
    let r = rows(
        store
            .query("SELECT ?s WHERE { ?s <http://example.org/name> \"Person 99\" }")
            .unwrap(),
    );
    assert_eq!(r.len(), 1);
}
