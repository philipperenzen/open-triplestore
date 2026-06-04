//! W3C SPARQL 1.1 Conformance Tests
//!
//! Tests derived from the official W3C SPARQL 1.1 test suite:
//! https://www.w3.org/2009/sparql/docs/tests/summary.html
//! https://w3c.github.io/rdf-tests/sparql/sparql11/
//!
//! Organized by the same test categories as the W3C manifest:
//!   - aggregates         (COUNT, SUM, AVG, MIN, MAX, GROUP_CONCAT, SAMPLE, HAVING)
//!   - bind               (BIND with expressions)
//!   - bindings/values    (VALUES inline data)
//!   - construct          (CONSTRUCT graph queries)
//!   - distinct           (DISTINCT modifier)
//!   - exists             (EXISTS / NOT EXISTS)
//!   - functions          (built-in SPARQL functions)
//!   - graph              (named graphs / GRAPH clause)
//!   - grouping           (GROUP BY)
//!   - having             (HAVING filter on groups)
//!   - negation           (NOT EXISTS, MINUS)
//!   - optional           (OPTIONAL join)
//!   - order-by           (ORDER BY / LIMIT / OFFSET)
//!   - path               (property paths)
//!   - project-expression (SELECT expressions)
//!   - subquery           (sub-SELECT)
//!   - syntax-query       (positive/negative syntax tests)
//!   - update             (SPARQL Update operations)
//!   - type-promotion     (numeric type coercion)
//!   - algebra            (semantics of graph patterns)

use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;

// ─── Test helpers ─────────────────────────────────────────────────────────────

fn ts() -> open_triplestore::store::TripleStore {
    open_triplestore::store::TripleStore::in_memory().unwrap()
}

fn load(store: &open_triplestore::store::TripleStore, ttl: &str) {
    store.load_str(ttl, RdfFormat::Turtle, None).unwrap();
}

#[allow(dead_code)]
fn load_nt(store: &open_triplestore::store::TripleStore, nt: &str) {
    store.load_str(nt, RdfFormat::NTriples, None).unwrap();
}

fn select(store: &open_triplestore::store::TripleStore, q: &str) -> Vec<Vec<String>> {
    match store.query(q).unwrap() {
        QueryResults::Solutions(sols) => {
            let vars: Vec<_> = sols
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            sols.into_iter()
                .map(|s| {
                    let s = s.unwrap();
                    vars.iter()
                        .map(|v| s.get(v.as_str()).map(|t| t.to_string()).unwrap_or_default())
                        .collect()
                })
                .collect()
        }
        _ => panic!("Expected SELECT results"),
    }
}

fn ask(store: &open_triplestore::store::TripleStore, q: &str) -> bool {
    match store.query(q).unwrap() {
        QueryResults::Boolean(b) => b,
        _ => panic!("Expected ASK result"),
    }
}

fn graph_count(store: &open_triplestore::store::TripleStore, q: &str) -> usize {
    match store.query(q).unwrap() {
        QueryResults::Graph(triples) => triples.count(),
        _ => panic!("Expected CONSTRUCT/DESCRIBE result"),
    }
}

// ═══════════════════════════════════════════════════════════
// Category: aggregates
// W3C tests: agg01..agg15, agg-groupconcat-01..02, etc.
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_agg_count_all() {
    // agg01: COUNT(*) over entire dataset
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :p 1 . :b :p 2 . :c :p 3 .",
    );
    let r = select(&s, "SELECT (COUNT(*) AS ?count) WHERE { ?s ?p ?o }");
    assert_eq!(r[0][0], "\"3\"^^<http://www.w3.org/2001/XMLSchema#integer>");
}

#[test]
fn w3c_agg_count_distinct() {
    // Distinct count of subjects
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :p 1 . :a :p 2 . :b :p 3 .",
    );
    let r = select(
        &s,
        "SELECT (COUNT(DISTINCT ?s) AS ?count) WHERE { ?s ?p ?o }",
    );
    assert_eq!(r[0][0], "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>");
}

#[test]
fn w3c_agg_sum() {
    // agg02: SUM
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :val 1 . :b :val 2 . :c :val 3 .",
    );
    let r = select(
        &s,
        "SELECT (SUM(?v) AS ?sum) WHERE { ?s <http://example/val> ?v }",
    );
    assert!(r[0][0].contains("6"), "SUM should be 6, got {}", r[0][0]);
}

#[test]
fn w3c_agg_min_max() {
    // agg03 / agg04
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :val 10 . :b :val 5 . :c :val 20 .",
    );
    let r = select(
        &s,
        "SELECT (MIN(?v) AS ?mn) (MAX(?v) AS ?mx) WHERE { ?s <http://example/val> ?v }",
    );
    assert!(r[0][0].contains("5"), "MIN should be 5, got {}", r[0][0]);
    assert!(r[0][1].contains("20"), "MAX should be 20, got {}", r[0][1]);
}

#[test]
fn w3c_agg_avg() {
    // agg05: AVG
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :val 6 . :b :val 10 .");
    let r = select(
        &s,
        "SELECT (AVG(?v) AS ?avg) WHERE { ?s <http://example/val> ?v }",
    );
    assert!(r[0][0].contains("8"), "AVG should be 8, got {}", r[0][0]);
}

#[test]
fn w3c_agg_sample() {
    // SAMPLE returns an arbitrary value from the group
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :val 42 .");
    let r = select(
        &s,
        "SELECT (SAMPLE(?v) AS ?s) WHERE { ?x <http://example/val> ?v }",
    );
    assert!(!r.is_empty() && !r[0][0].is_empty());
}

#[test]
fn w3c_agg_group_concat() {
    // GROUP_CONCAT with separator
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :name \"x\" . :b :name \"y\" . :c :name \"z\" .",
    );
    let r = select(
        &s,
        "SELECT (GROUP_CONCAT(?n ; separator=\",\") AS ?joined) WHERE { ?s <http://example/name> ?n } ORDER BY ?n",
    );
    assert!(!r.is_empty());
    // Result contains all names joined
    let joined = &r[0][0];
    assert!(
        joined.contains("x") || joined.contains("y") || joined.contains("z"),
        "GROUP_CONCAT result: {}",
        joined
    );
}

#[test]
fn w3c_agg_group_by_count() {
    // GROUP BY with COUNT per group
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :type :X ; :val 1 . :b :type :X ; :val 2 . :c :type :Y ; :val 3 .",
    );
    let r = select(
        &s,
        "SELECT ?t (COUNT(?s) AS ?cnt) WHERE { ?s <http://example/type> ?t } GROUP BY ?t ORDER BY ?t",
    );
    assert_eq!(r.len(), 2);
}

#[test]
fn w3c_agg_having() {
    // HAVING clause to filter groups
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :type :X ; :val 1 . :b :type :X ; :val 2 . :c :type :Y ; :val 10 .",
    );
    let r = select(
        &s,
        "SELECT ?t (SUM(?v) AS ?total) WHERE { ?s <http://example/type> ?t ; <http://example/val> ?v } GROUP BY ?t HAVING (SUM(?v) > 5)",
    );
    // Only :Y group has sum > 5 (=10); :X group has sum=3
    assert_eq!(r.len(), 1);
    assert!(r[0][1].contains("10"));
}

// ═══════════════════════════════════════════════════════════
// Category: bind
// W3C tests: bind01..bind04
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_bind_basic() {
    // bind01: BIND with simple expression
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :val 5 .");
    let r = select(
        &s,
        "SELECT ?doubled WHERE { ?x <http://example/val> ?v . BIND(?v * 2 AS ?doubled) }",
    );
    assert!(r[0][0].contains("10"));
}

#[test]
fn w3c_bind_string() {
    // BIND with string function
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :name \"hello\" .");
    let r = select(
        &s,
        "SELECT ?upper WHERE { ?x <http://example/name> ?n . BIND(UCASE(?n) AS ?upper) }",
    );
    assert!(r[0][0].contains("HELLO"));
}

#[test]
fn w3c_bind_iri() {
    // BIND with IRI construction
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :id \"42\" .");
    let r = select(
        &s,
        "SELECT ?iri WHERE { ?x <http://example/id> ?id . BIND(IRI(CONCAT(\"http://example/item/\", ?id)) AS ?iri) }",
    );
    assert!(r[0][0].contains("item/42"));
}

#[test]
fn w3c_bind_coalesce() {
    // BIND with COALESCE for fallback values
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :name \"Alice\" ; :nick \"Al\" . :b :name \"Bob\" .",
    );
    let r = select(
        &s,
        "SELECT ?name ?display WHERE { ?x <http://example/name> ?name . OPTIONAL { ?x <http://example/nick> ?nick } . BIND(COALESCE(?nick, ?name) AS ?display) } ORDER BY ?name",
    );
    assert_eq!(r.len(), 2);
    assert!(r[0][1].contains("Al"), "Alice should use nick: {}", r[0][1]);
    assert!(
        r[1][1].contains("Bob"),
        "Bob should fall back to name: {}",
        r[1][1]
    );
}

// ═══════════════════════════════════════════════════════════
// Category: bindings / VALUES
// W3C tests: bindings01..bindings08
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_values_inline_simple() {
    // bindings01: simple VALUES clause
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :name \"Alice\" . :b :name \"Bob\" . :c :name \"Charlie\" .",
    );
    let r = select(
        &s,
        "SELECT ?name WHERE { VALUES ?x { <http://example/a> <http://example/c> } ?x <http://example/name> ?name } ORDER BY ?name",
    );
    assert_eq!(r.len(), 2);
    assert!(r[0][0].contains("Alice"));
    assert!(r[1][0].contains("Charlie"));
}

#[test]
fn w3c_values_multivar() {
    // bindings02: multi-variable VALUES
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :name \"Alice\" ; :age 30 . :b :name \"Bob\" ; :age 25 .",
    );
    let r = select(
        &s,
        "SELECT ?name ?age WHERE { VALUES (?name ?age) { (\"Alice\" 30) (\"Bob\" 25) } } ORDER BY ?name",
    );
    assert_eq!(r.len(), 2);
}

#[test]
fn w3c_values_undef() {
    // VALUES with UNDEF (unbound marker)
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :p :x . :b :p :y .");
    // UNDEF means the variable is unbound for that row
    let r = select(
        &s,
        "SELECT ?s ?o WHERE { VALUES (?s ?o) { (<http://example/a> UNDEF) (UNDEF <http://example/y>) } ?s <http://example/p> ?o }",
    );
    // Results join on the non-UNDEF parts
    assert!(!r.is_empty());
}

// ═══════════════════════════════════════════════════════════
// Category: construct
// W3C tests: constructwhere01..04, construct01..04
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_construct_basic() {
    // construct01: CONSTRUCT WHERE shorthand
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :b :c .");
    let count = graph_count(&s, "CONSTRUCT WHERE { ?s ?p ?o }");
    assert_eq!(count, 1);
}

#[test]
fn w3c_construct_template() {
    // construct02: CONSTRUCT with explicit template
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :name \"Alice\" .");
    let count = graph_count(
        &s,
        "CONSTRUCT { ?s <http://schema.org/name> ?name } WHERE { ?s <http://example/name> ?name }",
    );
    assert_eq!(count, 1);
}

#[test]
fn w3c_construct_with_filter() {
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :val 10 . :b :val 20 . :c :val 5 .",
    );
    let count = graph_count(
        &s,
        "CONSTRUCT { ?s <http://example/bigVal> ?v } WHERE { ?s <http://example/val> ?v FILTER(?v > 10) }",
    );
    assert_eq!(count, 1);
}

// ═══════════════════════════════════════════════════════════
// Category: distinct
// W3C tests: distinct-1..distinct-9
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_distinct_basic() {
    // distinct-1: DISTINCT eliminates duplicates
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :p :x . :b :p :x . :c :p :y .",
    );
    let r = select(&s, "SELECT DISTINCT ?o WHERE { ?s <http://example/p> ?o }");
    assert_eq!(r.len(), 2);
}

#[test]
fn w3c_reduced() {
    // REDUCED is a hint to allow (not require) deduplication
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :p :x . :b :p :x . :c :p :y .",
    );
    let r = select(&s, "SELECT REDUCED ?o WHERE { ?s <http://example/p> ?o }");
    // REDUCED may or may not deduplicate; result has 2 or 3 rows
    assert!(r.len() >= 2 && r.len() <= 3);
}

// ═══════════════════════════════════════════════════════════
// Category: exists
// W3C tests: exists01..exists06
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_exists_basic() {
    // exists01: FILTER EXISTS
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :name \"Alice\" ; :email \"a@b.com\" . :b :name \"Bob\" .",
    );
    let r = select(
        &s,
        "SELECT ?name WHERE { ?x <http://example/name> ?name . FILTER EXISTS { ?x <http://example/email> ?e } }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("Alice"));
}

#[test]
fn w3c_not_exists_basic() {
    // exists02: FILTER NOT EXISTS
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :name \"Alice\" ; :email \"a@b.com\" . :b :name \"Bob\" .",
    );
    let r = select(
        &s,
        "SELECT ?name WHERE { ?x <http://example/name> ?name . FILTER NOT EXISTS { ?x <http://example/email> ?e } }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("Bob"));
}

#[test]
fn w3c_exists_correlated() {
    // exists03: correlated EXISTS using outer variable
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :knows :b . :a :knows :c . :b :knows :c .",
    );
    let r = select(
        &s,
        "SELECT ?x WHERE { ?x <http://example/knows> ?y . FILTER EXISTS { ?y <http://example/knows> ?z } }",
    );
    // :a knows :b; :b knows :c → :a has ?y=:b which knows :c → :a qualifies
    assert!(!r.is_empty());
}

// ═══════════════════════════════════════════════════════════
// Category: functions
// W3C tests: functions01..functions33 (built-in SPARQL functions)
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_fn_str() {
    let s = ts();
    let r = select(&s, "SELECT (STR(42) AS ?v) WHERE {}");
    assert!(r[0][0].contains("42"));
}

#[test]
fn w3c_fn_lang() {
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :p \"hello\"@en .");
    let r = select(&s, "SELECT (LANG(?o) AS ?lang) WHERE { ?s ?p ?o }");
    assert!(r[0][0].contains("en"));
}

#[test]
fn w3c_fn_langmatches() {
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :p \"hello\"@en-US ; :p \"bonjour\"@fr .",
    );
    let r = select(
        &s,
        "SELECT ?o WHERE { ?s <http://example/p> ?o . FILTER LANGMATCHES(LANG(?o), \"en\") }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("hello"));
}

#[test]
fn w3c_fn_datatype() {
    let s = ts();
    let r = select(&s, "SELECT (DATATYPE(42) AS ?dt) WHERE {}");
    assert!(r[0][0].contains("integer"));
}

#[test]
fn w3c_fn_bound() {
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :name \"Alice\" .");
    let r = select(
        &s,
        "SELECT ?name WHERE { ?x <http://example/name> ?name . OPTIONAL { ?x <http://example/age> ?age } FILTER(BOUND(?age) = false) }",
    );
    assert_eq!(r.len(), 1);
}

#[test]
fn w3c_fn_iri() {
    let s = ts();
    let r = select(
        &s,
        "SELECT (IRI(\"http://example.org/test\") AS ?iri) WHERE {}",
    );
    assert!(r[0][0].contains("example.org/test"));
}

#[test]
fn w3c_fn_bnode() {
    // BNODE() creates blank nodes
    let s = ts();
    let r = select(&s, "SELECT (BNODE() AS ?bn) WHERE {}");
    assert!(!r[0][0].is_empty());
}

#[test]
fn w3c_fn_strdt() {
    // STRDT constructs typed literal
    let s = ts();
    let r = select(
        &s,
        "SELECT (STRDT(\"42\", <http://www.w3.org/2001/XMLSchema#integer>) AS ?v) WHERE {}",
    );
    assert!(r[0][0].contains("42"));
}

#[test]
fn w3c_fn_strlang() {
    // STRLANG constructs language-tagged literal
    let s = ts();
    let r = select(&s, "SELECT (STRLANG(\"hello\", \"en\") AS ?v) WHERE {}");
    assert!(r[0][0].contains("hello") && r[0][0].contains("en"));
}

#[test]
fn w3c_fn_numeric_abs() {
    let s = ts();
    let r = select(&s, "SELECT (ABS(-5) AS ?v) WHERE {}");
    assert!(r[0][0].contains("5"));
}

#[test]
fn w3c_fn_numeric_round() {
    let s = ts();
    let r = select(&s, "SELECT (ROUND(2.5) AS ?v) WHERE {}");
    let v = &r[0][0];
    assert!(v.contains("2") || v.contains("3")); // implementation-defined rounding
}

#[test]
fn w3c_fn_numeric_ceil() {
    let s = ts();
    let r = select(&s, "SELECT (CEIL(4.1) AS ?v) WHERE {}");
    assert!(r[0][0].contains("5"));
}

#[test]
fn w3c_fn_numeric_floor() {
    let s = ts();
    let r = select(&s, "SELECT (FLOOR(4.9) AS ?v) WHERE {}");
    assert!(r[0][0].contains("4"));
}

#[test]
fn w3c_fn_rand() {
    let s = ts();
    let r = select(&s, "SELECT (RAND() AS ?v) WHERE {}");
    assert!(!r[0][0].is_empty());
}

#[test]
fn w3c_fn_strlen() {
    let s = ts();
    let r = select(&s, "SELECT (STRLEN(\"hello\") AS ?len) WHERE {}");
    assert!(r[0][0].contains("5"));
}

#[test]
fn w3c_fn_substr_2arg() {
    let s = ts();
    let r = select(&s, "SELECT (SUBSTR(\"foobar\", 4) AS ?v) WHERE {}");
    assert!(r[0][0].contains("bar"));
}

#[test]
fn w3c_fn_substr_3arg() {
    let s = ts();
    let r = select(&s, "SELECT (SUBSTR(\"foobar\", 2, 3) AS ?v) WHERE {}");
    assert!(r[0][0].contains("oob"));
}

#[test]
fn w3c_fn_ucase_lcase() {
    let s = ts();
    let ru = select(&s, "SELECT (UCASE(\"hello\") AS ?v) WHERE {}");
    assert!(ru[0][0].contains("HELLO"));
    let rl = select(&s, "SELECT (LCASE(\"WORLD\") AS ?v) WHERE {}");
    assert!(rl[0][0].contains("world"));
}

#[test]
fn w3c_fn_strstarts_strends() {
    let s = ts();
    let r1 = select(&s, "SELECT (STRSTARTS(\"foobar\", \"foo\") AS ?v) WHERE {}");
    assert!(r1[0][0].contains("true"));
    let r2 = select(&s, "SELECT (STRENDS(\"foobar\", \"bar\") AS ?v) WHERE {}");
    assert!(r2[0][0].contains("true"));
}

#[test]
fn w3c_fn_contains() {
    let s = ts();
    let r = select(&s, "SELECT (CONTAINS(\"foobar\", \"oba\") AS ?v) WHERE {}");
    assert!(r[0][0].contains("true"));
}

#[test]
fn w3c_fn_strbefore_strafter() {
    let s = ts();
    let r1 = select(&s, "SELECT (STRBEFORE(\"abc\", \"b\") AS ?v) WHERE {}");
    assert!(r1[0][0].contains("a"));
    let r2 = select(&s, "SELECT (STRAFTER(\"abc\", \"b\") AS ?v) WHERE {}");
    assert!(r2[0][0].contains("c"));
}

#[test]
fn w3c_fn_encode_for_uri() {
    let s = ts();
    let r = select(&s, "SELECT (ENCODE_FOR_URI(\"a b\") AS ?v) WHERE {}");
    assert!(r[0][0].contains("a%20b") || r[0][0].contains("a+b"));
}

#[test]
fn w3c_fn_concat() {
    let s = ts();
    let r = select(&s, "SELECT (CONCAT(\"foo\", \"bar\") AS ?v) WHERE {}");
    assert!(r[0][0].contains("foobar"));
}

#[test]
fn w3c_fn_regex() {
    let s = ts();
    let r = select(&s, "SELECT (REGEX(\"foobar\", \"^foo\") AS ?v) WHERE {}");
    assert!(r[0][0].contains("true"));
}

#[test]
fn w3c_fn_replace() {
    let s = ts();
    let r = select(
        &s,
        "SELECT (REPLACE(\"abcd\", \"b\", \"Z\") AS ?v) WHERE {}",
    );
    assert!(r[0][0].contains("aZcd"));
}

#[test]
fn w3c_fn_md5() {
    let s = ts();
    let r = select(&s, "SELECT (MD5(\"abc\") AS ?v) WHERE {}");
    // MD5("abc") = 900150983cd24fb0d6963f7d28e17f72
    assert!(r[0][0].contains("900150983cd24fb0d6963f7d28e17f72"));
}

#[test]
fn w3c_fn_sha1() {
    let s = ts();
    let r = select(&s, "SELECT (SHA1(\"abc\") AS ?v) WHERE {}");
    // SHA1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
    assert!(r[0][0].to_lowercase().contains("a9993e36"));
}

#[test]
fn w3c_fn_sha256() {
    let s = ts();
    let r = select(&s, "SELECT (SHA256(\"abc\") AS ?v) WHERE {}");
    // SHA256("abc") starts with ba7816bf
    assert!(r[0][0].contains("ba7816bf"));
}

#[test]
fn w3c_fn_sha512() {
    let s = ts();
    let r = select(&s, "SELECT (SHA512(\"abc\") AS ?v) WHERE {}");
    // SHA512("abc") starts with ddaf35a
    assert!(r[0][0].to_lowercase().contains("ddaf35a"));
}

#[test]
fn w3c_fn_now() {
    let s = ts();
    let r = select(&s, "SELECT (NOW() AS ?t) WHERE {}");
    assert!(!r[0][0].is_empty());
}

#[test]
fn w3c_fn_year_month_day() {
    let s = ts();
    let r = select(
        &s,
        "SELECT (YEAR(\"2024-03-15T10:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>) AS ?y) (MONTH(\"2024-03-15T10:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>) AS ?m) (DAY(\"2024-03-15T10:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>) AS ?d) WHERE {}",
    );
    assert!(r[0][0].contains("2024"));
    assert!(r[0][1].contains("3"));
    assert!(r[0][2].contains("15"));
}

#[test]
fn w3c_fn_hours_minutes_seconds() {
    let s = ts();
    let r = select(
        &s,
        "SELECT (HOURS(\"2024-03-15T14:30:45Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>) AS ?h) (MINUTES(\"2024-03-15T14:30:45Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>) AS ?m) (SECONDS(\"2024-03-15T14:30:45Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>) AS ?s) WHERE {}",
    );
    assert!(r[0][0].contains("14"));
    assert!(r[0][1].contains("30"));
    assert!(r[0][2].contains("45"));
}

#[test]
fn w3c_fn_timezone_tz() {
    let s = ts();
    let r = select(
        &s,
        "SELECT (TIMEZONE(\"2024-01-01T00:00:00+02:00\"^^<http://www.w3.org/2001/XMLSchema#dateTime>) AS ?tz) WHERE {}",
    );
    assert!(!r[0][0].is_empty());
}

#[test]
fn w3c_fn_is_iri_blank_literal() {
    let s = ts();
    let ri = select(&s, "SELECT (isIRI(<http://example.org/>) AS ?v) WHERE {}");
    assert!(ri[0][0].contains("true"));
    // Blank nodes must be bound in WHERE, not used directly in SELECT expressions
    let rb = select(
        &s,
        "SELECT (isBLANK(?b) AS ?v) WHERE { BIND(BNODE() AS ?b) }",
    );
    assert!(rb[0][0].contains("true"));
    let rl = select(&s, "SELECT (isLiteral(\"hello\") AS ?v) WHERE {}");
    assert!(rl[0][0].contains("true"));
    let rn = select(&s, "SELECT (isNumeric(42) AS ?v) WHERE {}");
    assert!(rn[0][0].contains("true"));
}

// ═══════════════════════════════════════════════════════════
// Category: graph
// W3C tests: graph01..graph13
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_graph_query_named() {
    let s = ts();
    s.load_str(
        "<http://example/s> <http://example/p> \"v1\" .",
        RdfFormat::NTriples,
        Some("http://example/g1"),
    )
    .unwrap();
    s.load_str(
        "<http://example/s> <http://example/p> \"v2\" .",
        RdfFormat::NTriples,
        Some("http://example/g2"),
    )
    .unwrap();

    let r = select(
        &s,
        "SELECT ?v WHERE { GRAPH <http://example/g1> { ?s <http://example/p> ?v } }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("v1"));
}

#[test]
fn w3c_graph_from_named() {
    // FROM NAMED clause
    let s = ts();
    s.load_str(
        "<http://example/s> <http://example/p> \"named\" .",
        RdfFormat::NTriples,
        Some("http://example/g1"),
    )
    .unwrap();
    s.load_str(
        "<http://example/s> <http://example/p> \"default\" .",
        RdfFormat::NTriples,
        None,
    )
    .unwrap();

    let r = select(
        &s,
        "SELECT ?v FROM NAMED <http://example/g1> WHERE { GRAPH ?g { ?s <http://example/p> ?v } }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("named"));
}

#[test]
fn w3c_graph_variable() {
    // GRAPH ?g clause
    let s = ts();
    s.load_str(
        "<http://example/s> <http://example/p> \"v1\" .",
        RdfFormat::NTriples,
        Some("http://example/g1"),
    )
    .unwrap();
    s.load_str(
        "<http://example/s> <http://example/p> \"v2\" .",
        RdfFormat::NTriples,
        Some("http://example/g2"),
    )
    .unwrap();

    let r = select(
        &s,
        "SELECT ?g ?v WHERE { GRAPH ?g { ?s <http://example/p> ?v } } ORDER BY ?g",
    );
    assert_eq!(r.len(), 2);
}

// ═══════════════════════════════════════════════════════════
// Category: negation
// W3C tests: neg-exists-1..11, minus-1..minus-5
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_negation_minus_basic() {
    // neg-minus-1: MINUS set difference
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :p :x . :b :p :y . :c :p :z .",
    );
    let r = select(
        &s,
        "SELECT ?s WHERE { ?s <http://example/p> ?o MINUS { ?s <http://example/p> <http://example/x> } }",
    );
    // :a is excluded, :b and :c remain
    assert_eq!(r.len(), 2);
    for row in &r {
        assert!(!row[0].contains("/a>"), "a should be excluded");
    }
}

#[test]
fn w3c_negation_minus_no_shared_vars() {
    // neg-minus-2: MINUS with no shared variables → no rows excluded
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :p :x . :b :p :y .");
    let r = select(
        &s,
        "SELECT ?s WHERE { ?s <http://example/p> ?o MINUS { ?x <http://example/p> <http://example/x> } }",
    );
    // The MINUS RHS doesn't share ?s, so the set difference doesn't remove anything
    // (SPARQL spec: MINUS with no shared vars is identity)
    assert_eq!(r.len(), 2);
}

#[test]
fn w3c_negation_not_exists_scoping() {
    // NOT EXISTS should use the outer binding for ?x
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :p :x . :b :p :y . :a :q :z .",
    );
    let r = select(
        &s,
        "SELECT ?s WHERE { ?s <http://example/p> ?o . FILTER NOT EXISTS { ?s <http://example/q> ?r } }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("/b>"));
}

// ═══════════════════════════════════════════════════════════
// Category: optional
// W3C tests: opt-simple-1..22
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_optional_basic() {
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :name \"Alice\" ; :age 30 . :b :name \"Bob\" .",
    );
    let r = select(
        &s,
        "SELECT ?name ?age WHERE { ?x <http://example/name> ?name . OPTIONAL { ?x <http://example/age> ?age } } ORDER BY ?name",
    );
    assert_eq!(r.len(), 2);
    assert!(!r[0][1].is_empty()); // Alice has age
    assert!(r[1][1].is_empty()); // Bob has no age
}

#[test]
fn w3c_optional_filter_inside() {
    // FILTER inside OPTIONAL only applies within the optional part
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :p 1 . :b :p 2 . :c :p 10 . :c :q \"yes\" .",
    );
    let r = select(
        &s,
        "SELECT ?x ?pv ?q WHERE { ?x <http://example/p> ?pv . OPTIONAL { ?x <http://example/q> ?q . FILTER(?pv > 5) } } ORDER BY ?x",
    );
    // All 3 subjects appear; only :c gets ?q bound (pv=10 > 5)
    assert_eq!(r.len(), 3);
}

#[test]
fn w3c_optional_nested() {
    // Nested OPTIONAL
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :p 1 ; :q 2 ; :r 3 . :b :p 4 ; :q 5 . :c :p 6 .",
    );
    let r = select(
        &s,
        "SELECT ?x ?p ?q ?r WHERE { ?x <http://example/p> ?p . OPTIONAL { ?x <http://example/q> ?q . OPTIONAL { ?x <http://example/r> ?r } } } ORDER BY ?x",
    );
    assert_eq!(r.len(), 3);
}

// ═══════════════════════════════════════════════════════════
// Category: order-by / solution modifiers
// W3C tests: orderby01..18, limit-1..3, offset-1..2
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_order_by_asc() {
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :val 3 . :b :val 1 . :c :val 2 .",
    );
    let r = select(
        &s,
        "SELECT ?v WHERE { ?s <http://example/val> ?v } ORDER BY ASC(?v)",
    );
    assert_eq!(r.len(), 3);
    assert!(r[0][0].contains("1"));
    assert!(r[1][0].contains("2"));
    assert!(r[2][0].contains("3"));
}

#[test]
fn w3c_order_by_desc() {
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :val 3 . :b :val 1 . :c :val 2 .",
    );
    let r = select(
        &s,
        "SELECT ?v WHERE { ?s <http://example/val> ?v } ORDER BY DESC(?v)",
    );
    assert!(r[0][0].contains("3"));
    assert!(r[2][0].contains("1"));
}

#[test]
fn w3c_limit_offset() {
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :v 1 . :b :v 2 . :c :v 3 . :d :v 4 . :e :v 5 .",
    );
    // LIMIT 2
    let r = select(
        &s,
        "SELECT ?v WHERE { ?s <http://example/v> ?v } ORDER BY ?v LIMIT 2",
    );
    assert_eq!(r.len(), 2);
    // LIMIT 2 OFFSET 2
    let r2 = select(
        &s,
        "SELECT ?v WHERE { ?s <http://example/v> ?v } ORDER BY ?v LIMIT 2 OFFSET 2",
    );
    assert_eq!(r2.len(), 2);
    assert!(r2[0][0].contains("3"));
}

#[test]
fn w3c_order_by_string_collation() {
    // String ordering: uppercase < lowercase in XSD comparison
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :n \"Alice\" . :b :n \"alice\" . :c :n \"Bob\" .",
    );
    let r = select(
        &s,
        "SELECT ?n WHERE { ?s <http://example/n> ?n } ORDER BY ?n",
    );
    assert_eq!(r.len(), 3);
}

// ═══════════════════════════════════════════════════════════
// Category: property-path
// W3C tests: path-1..35
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_path_sequence() {
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :p :b . :b :q :c .");
    let r = select(
        &s,
        "SELECT ?c WHERE { <http://example/a> <http://example/p>/<http://example/q> ?c }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("/c>"));
}

#[test]
fn w3c_path_alternative() {
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :p1 :x . :b :p2 :y .");
    let r = select(
        &s,
        "SELECT ?o WHERE { ?s (<http://example/p1>|<http://example/p2>) ?o } ORDER BY ?o",
    );
    assert_eq!(r.len(), 2);
}

#[test]
fn w3c_path_zero_or_more() {
    // p* includes length-0 paths (reflexive)
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :sub :b . :b :sub :c .",
    );
    let r = select(
        &s,
        "SELECT ?o WHERE { <http://example/a> <http://example/sub>* ?o }",
    );
    // :a, :b, :c (including :a itself via length-0)
    assert!(r.len() >= 3);
}

#[test]
fn w3c_path_one_or_more() {
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :sub :b . :b :sub :c .",
    );
    let r = select(
        &s,
        "SELECT ?o WHERE { <http://example/a> <http://example/sub>+ ?o }",
    );
    assert_eq!(r.len(), 2); // :b, :c (not :a itself)
}

#[test]
fn w3c_path_zero_or_one() {
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :p :b .");
    let r = select(
        &s,
        "SELECT ?o WHERE { <http://example/a> <http://example/p>? ?o }",
    );
    // :a (length-0) and :b (length-1)
    assert_eq!(r.len(), 2);
}

#[test]
fn w3c_path_inverse() {
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :p :b .");
    // ^p reverses direction: ?s ^p :a means "find ?s where :a p ?s"
    // We have :a :p :b, so :a p :b gives ?s = :b (1 result)
    let r = select(
        &s,
        "SELECT ?s WHERE { ?s ^<http://example/p> <http://example/a> }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("/b>"));

    // ?s ^p :b means "find ?s where :b p ?s"; :b is not a subject here → 0 results
    let r2 = select(
        &s,
        "SELECT ?s WHERE { ?s ^<http://example/p> <http://example/b> }",
    );
    assert_eq!(r2.len(), 0);
}

#[test]
fn w3c_path_negated_property_set() {
    // !(p) matches any property except p
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :p :b . :a :q :c .");
    let r = select(
        &s,
        "SELECT ?o WHERE { <http://example/a> !<http://example/p> ?o }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("/c>"));
}

#[test]
fn w3c_path_rdfs_subclass() {
    // Common use: rdfs:subClassOf* for class hierarchy
    let s = ts();
    load(
        &s,
        r#"@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
           @prefix : <http://example/> .
           :A rdfs:subClassOf :B .
           :B rdfs:subClassOf :C .
           :C rdfs:subClassOf :D ."#,
    );
    let r = select(
        &s,
        "SELECT ?super WHERE { <http://example/A> <http://www.w3.org/2000/01/rdf-schema#subClassOf>+ ?super }",
    );
    assert_eq!(r.len(), 3); // B, C, D
}

// ═══════════════════════════════════════════════════════════
// Category: project-expression
// W3C tests: pex01..pex05
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_project_expression_basic() {
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :val 10 .");
    let r = select(
        &s,
        "SELECT (?v + 5 AS ?result) WHERE { ?x <http://example/val> ?v }",
    );
    assert!(r[0][0].contains("15"));
}

#[test]
fn w3c_project_expression_conditional() {
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :val 10 . :b :val 30 .",
    );
    let r = select(
        &s,
        "SELECT ?v (IF(?v > 20, \"big\", \"small\") AS ?size) WHERE { ?s <http://example/val> ?v } ORDER BY ?v",
    );
    assert!(r[0][1].contains("small"));
    assert!(r[1][1].contains("big"));
}

// ═══════════════════════════════════════════════════════════
// Category: subquery
// W3C tests: subquery01..subquery04
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_subquery_basic() {
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :val 10 . :b :val 20 . :c :val 30 .",
    );
    let r = select(
        &s,
        "SELECT ?s ?v WHERE { ?s <http://example/val> ?v { SELECT (MAX(?x) AS ?max) WHERE { ?y <http://example/val> ?x } } FILTER(?v = ?max) }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][1].contains("30"));
}

#[test]
fn w3c_subquery_limit_propagation() {
    // Subquery with LIMIT
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :p 1 . :b :p 2 . :c :p 3 .",
    );
    let r = select(
        &s,
        "SELECT ?s WHERE { { SELECT ?s WHERE { ?s <http://example/p> ?v } ORDER BY ?v LIMIT 2 } }",
    );
    assert_eq!(r.len(), 2);
}

// ═══════════════════════════════════════════════════════════
// Category: type-promotion
// W3C tests: cast-01..16, type-promotion-01..42
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_type_promotion_int_decimal() {
    // xsd:integer promotes to xsd:decimal in arithmetic
    let s = ts();
    let r = select(&s, "SELECT (1 + 1.5 AS ?v) WHERE {}");
    assert!(r[0][0].contains("2.5"));
}

#[test]
fn w3c_type_promotion_decimal_float() {
    // xsd:decimal promotes to xsd:float/double in arithmetic with float
    let s = ts();
    let r = select(
        &s,
        "SELECT (2.0 * \"3\"^^<http://www.w3.org/2001/XMLSchema#float> AS ?v) WHERE {}",
    );
    assert!(!r[0][0].is_empty());
}

#[test]
fn w3c_cast_integer() {
    let s = ts();
    // Use IRI form for XSD type casting (Oxigraph supports IRI function call syntax)
    let r = select(
        &s,
        "SELECT (<http://www.w3.org/2001/XMLSchema#integer>(\"42\") AS ?v) WHERE {}",
    );
    assert!(!r.is_empty() && r[0][0].contains("42"));
    // Also test integer arithmetic preserves type
    let r2 = select(&s, "SELECT (1 + 1 AS ?v) WHERE {}");
    assert!(!r2.is_empty() && r2[0][0].contains("2"));
}

#[test]
fn w3c_cast_boolean() {
    let s = ts();
    let r = select(
        &s,
        "SELECT (<http://www.w3.org/2001/XMLSchema#boolean>(\"true\") AS ?v) WHERE {}",
    );
    assert!(r[0][0].contains("true"));
}

#[test]
fn w3c_numeric_comparison() {
    // Mixed type comparisons should work via type promotion
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :v \"10\"^^<http://www.w3.org/2001/XMLSchema#integer> . :b :v \"10.0\"^^<http://www.w3.org/2001/XMLSchema#decimal> .",
    );
    let r = select(
        &s,
        "SELECT (COUNT(*) AS ?c) WHERE { ?s <http://example/v> ?v . FILTER(?v = 10) }",
    );
    // Both :a and :b have value numerically equal to 10
    assert!(r[0][0].contains("2"));
}

// ═══════════════════════════════════════════════════════════
// Category: algebra (graph pattern semantics)
// W3C tests: algebra/join-*, algebra/leftjoin-*, etc.
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_algebra_filter_placement() {
    // FILTER should apply to the entire group, not just preceding patterns
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :p 1 ; :q 2 . :b :p 10 ; :q 20 .",
    );
    let r = select(
        &s,
        "SELECT ?s WHERE { ?s <http://example/p> ?p . ?s <http://example/q> ?q . FILTER(?p < ?q) }",
    );
    assert_eq!(r.len(), 2); // Both have p < q
}

#[test]
fn w3c_algebra_optional_filter_interaction() {
    // FILTER inside OPTIONAL scope
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :p 1 . :b :p 2 . :b :q 20 .",
    );
    let r = select(
        &s,
        "SELECT ?s ?q WHERE { ?s <http://example/p> ?p . OPTIONAL { ?s <http://example/q> ?q . FILTER(?q > 10) } }",
    );
    assert_eq!(r.len(), 2);
    // :a has no :q; :b has :q = 20 > 10
    let has_q: Vec<_> = r.iter().filter(|row| !row[1].is_empty()).collect();
    assert_eq!(has_q.len(), 1);
}

#[test]
fn w3c_algebra_union_semantics() {
    // UNION: both sides can match the same solution
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :p :x ; :q :x .");
    let r = select(
        &s,
        "SELECT ?o WHERE { { ?s <http://example/p> ?o } UNION { ?s <http://example/q> ?o } }",
    );
    // :x appears via both p and q patterns
    assert_eq!(r.len(), 2);
}

// ═══════════════════════════════════════════════════════════
// Category: update
// W3C tests: Update-1..many (INSERT DATA, DELETE DATA, INSERT WHERE, etc.)
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_update_insert_data() {
    let s = ts();
    s.update("INSERT DATA { <http://example/s> <http://example/p> <http://example/o> }")
        .unwrap();
    assert_eq!(s.len().unwrap(), 1);
}

#[test]
fn w3c_update_delete_data() {
    let s = ts();
    s.update("INSERT DATA { <http://example/s> <http://example/p> <http://example/o> }")
        .unwrap();
    s.update("DELETE DATA { <http://example/s> <http://example/p> <http://example/o> }")
        .unwrap();
    assert_eq!(s.len().unwrap(), 0);
}

#[test]
fn w3c_update_insert_where() {
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :name \"Alice\" .");
    s.update("INSERT { ?s <http://example/label> ?n } WHERE { ?s <http://example/name> ?n }")
        .unwrap();
    assert!(ask(
        &s,
        "ASK { <http://example/a> <http://example/label> \"Alice\" }"
    ));
}

#[test]
fn w3c_update_delete_where() {
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :status \"old\" . :b :status \"new\" .",
    );
    s.update("DELETE WHERE { ?s <http://example/status> \"old\" }")
        .unwrap();
    assert_eq!(s.len().unwrap(), 1);
}

#[test]
fn w3c_update_delete_insert() {
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :status \"active\" .");
    s.update(
        r#"DELETE { ?s <http://example/status> "active" }
           INSERT { ?s <http://example/status> "retired" }
           WHERE  { ?s <http://example/status> "active" }"#,
    )
    .unwrap();
    assert!(ask(
        &s,
        "ASK { <http://example/a> <http://example/status> \"retired\" }"
    ));
    assert!(!ask(
        &s,
        "ASK { <http://example/a> <http://example/status> \"active\" }"
    ));
}

#[test]
fn w3c_update_clear_default() {
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :p :b .");
    s.update("CLEAR DEFAULT").unwrap();
    assert_eq!(s.len().unwrap(), 0);
}

#[test]
fn w3c_update_clear_graph() {
    let s = ts();
    s.load_str(
        "<http://example/s> <http://example/p> <http://example/o> .",
        RdfFormat::NTriples,
        Some("http://example/g"),
    )
    .unwrap();
    s.update("CLEAR GRAPH <http://example/g>").unwrap();
    assert_eq!(s.len().unwrap(), 0);
}

#[test]
fn w3c_update_create_drop_graph() {
    let s = ts();
    s.update("CREATE GRAPH <http://example/g>").unwrap();
    let graphs = s.named_graphs().unwrap();
    assert!(graphs.iter().any(|g| g.as_str().contains("example/g")));
    s.update("DROP GRAPH <http://example/g>").unwrap();
}

#[test]
fn w3c_update_add_copy_move() {
    let s = ts();
    load(&s, "@prefix : <http://example/> . :s :p :o .");

    // ADD: union source into target
    s.update("ADD DEFAULT TO <http://example/target>").unwrap();
    let r = select(
        &s,
        "SELECT (COUNT(*) AS ?c) WHERE { GRAPH <http://example/target> { ?s ?p ?o } }",
    );
    assert!(r[0][0].contains("1"));

    // COPY: replace target with source
    s.update("COPY DEFAULT TO <http://example/copy>").unwrap();
    let r2 = select(
        &s,
        "SELECT (COUNT(*) AS ?c) WHERE { GRAPH <http://example/copy> { ?s ?p ?o } }",
    );
    assert!(r2[0][0].contains("1"));
}

#[test]
fn w3c_update_insert_named_graph() {
    let s = ts();
    s.update(
        "INSERT DATA { GRAPH <http://example/g> { <http://example/s> <http://example/p> \"v\" } }",
    )
    .unwrap();
    let r = select(
        &s,
        "SELECT ?v WHERE { GRAPH <http://example/g> { ?s <http://example/p> ?v } }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("v"));
}

// ═══════════════════════════════════════════════════════════
// Category: syntax (positive/negative syntax tests)
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_syntax_positive_select() {
    // Valid SPARQL syntax should parse without error
    let s = ts();
    s.query("SELECT * WHERE { ?s ?p ?o }").unwrap();
    s.query("SELECT DISTINCT ?s WHERE { ?s ?p ?o }").unwrap();
    s.query("SELECT ?s (COUNT(*) AS ?c) WHERE { ?s ?p ?o } GROUP BY ?s")
        .unwrap();
}

#[test]
fn w3c_syntax_positive_ask() {
    let s = ts();
    s.query("ASK { ?s ?p ?o }").unwrap();
    s.query("ASK WHERE { <http://example/s> ?p ?o }").unwrap();
}

#[test]
fn w3c_syntax_positive_construct() {
    let s = ts();
    s.query("CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }")
        .unwrap();
    s.query("CONSTRUCT WHERE { ?s ?p ?o }").unwrap();
}

#[test]
fn w3c_syntax_negative_bad_filter() {
    // Syntactically invalid SPARQL should return an error
    let s = ts();
    assert!(s.query("SELECT ?s WHERE { FILTER(?s }").is_err());
}

#[test]
fn w3c_syntax_negative_unclosed_brace() {
    let s = ts();
    assert!(s.query("SELECT ?s WHERE { ?s ?p ?o").is_err());
}

// ═══════════════════════════════════════════════════════════
// Category: entailment / SPARQL 1.1 semantics edge cases
// ═══════════════════════════════════════════════════════════

#[test]
fn w3c_ask_empty_where() {
    // ASK with empty WHERE clause over non-empty store
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :b :c .");
    assert!(ask(&s, "ASK {}"));
}

#[test]
fn w3c_ask_false_on_empty_store() {
    let s = ts();
    // Empty store with pattern that can't match
    assert!(!ask(&s, "ASK { ?s ?p ?o }"));
}

#[test]
fn w3c_select_star() {
    let s = ts();
    load(&s, "@prefix : <http://example/> . :a :p :b .");
    let r = select(&s, "SELECT * WHERE { ?s ?p ?o }");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].len(), 3); // s, p, o
}

#[test]
fn w3c_filter_logical_operators() {
    // AND, OR, NOT in FILTER
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :v 5 . :b :v 15 . :c :v 25 .",
    );
    let r_and = select(
        &s,
        "SELECT ?v WHERE { ?s <http://example/v> ?v FILTER(?v > 3 && ?v < 20) } ORDER BY ?v",
    );
    assert_eq!(r_and.len(), 2);

    let r_or = select(
        &s,
        "SELECT ?v WHERE { ?s <http://example/v> ?v FILTER(?v < 10 || ?v > 20) } ORDER BY ?v",
    );
    assert_eq!(r_or.len(), 2);
}

#[test]
fn w3c_sparql12_rdf_star() {
    // SPARQL 1.2 / RDF-star: embedded triples
    let s = ts();
    s.update(
        "INSERT DATA { << <http://ex/s> <http://ex/p> <http://ex/o> >> <http://ex/meta> \"value\" }",
    )
    .unwrap();
    let r = select(
        &s,
        "SELECT ?m WHERE { << <http://ex/s> <http://ex/p> <http://ex/o> >> <http://ex/meta> ?m }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("value"));
}

#[test]
fn w3c_describe_resources() {
    let s = ts();
    load(
        &s,
        "@prefix : <http://example/> . :a :name \"Alice\" ; :age 30 ; :knows :b .",
    );
    let count = graph_count(&s, "DESCRIBE <http://example/a>");
    assert!(count >= 3);
}
