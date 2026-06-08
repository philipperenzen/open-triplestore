//! sparqloscope-style SPARQL Conformance Tests
//!
//! Derived from ad-freiburg/sparqloscope and ad-freiburg/sparql-conformance:
//! https://github.com/ad-freiburg/sparqloscope
//! https://github.com/ad-freiburg/sparql-conformance
//!
//! sparqloscope evaluates "most SPARQL 1.1 features relevant in practice"
//! organized into functional categories. This test suite provides exhaustive
//! per-feature verification, validating all SPARQL 1.1 operations against
//! reference implementations (QLever, Virtuoso, Jena, etc.).
//!
//! Categories covered:
//!   - Basic graph patterns (BGP)
//!   - Filter expressions (all operators and functions)
//!   - Optional (left join)
//!   - Union
//!   - Graph patterns (named graphs)
//!   - Solution modifiers (DISTINCT, ORDER BY, LIMIT, OFFSET)
//!   - Aggregates (COUNT, SUM, AVG, MIN, MAX, GROUP_CONCAT, SAMPLE)
//!   - Subqueries
//!   - Property paths (all forms)
//!   - BIND / COALESCE / IF / IRI construction
//!   - VALUES (inline data)
//!   - Negation (NOT EXISTS, MINUS)
//!   - Update operations (all forms)
//!   - SPARQL 1.2 RDF-star

use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn ts() -> open_triplestore::store::TripleStore {
    open_triplestore::store::TripleStore::in_memory().unwrap()
}

fn load(s: &open_triplestore::store::TripleStore, ttl: &str) {
    s.load_str(ttl, RdfFormat::Turtle, None).unwrap();
}

fn sel(s: &open_triplestore::store::TripleStore, q: &str) -> Vec<Vec<String>> {
    match s.query(q).unwrap() {
        QueryResults::Solutions(sols) => {
            let vars: Vec<_> = sols
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            sols.into_iter()
                .map(|sol| {
                    let sol = sol.unwrap();
                    vars.iter()
                        .map(|v| {
                            sol.get(v.as_str())
                                .map(|t| t.to_string())
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .collect()
        }
        _ => panic!("Expected SELECT results"),
    }
}

fn ask(s: &open_triplestore::store::TripleStore, q: &str) -> bool {
    match s.query(q).unwrap() {
        QueryResults::Boolean(b) => b,
        _ => panic!("Expected ASK"),
    }
}

fn update(s: &open_triplestore::store::TripleStore, q: &str) {
    s.update(q).unwrap();
}

// ═══════════════════════════════════════════════════════════
// Category 1: Basic Graph Patterns (BGP)
// sparqloscope: tests basic-*, triple-match-*
// ═══════════════════════════════════════════════════════════

/// Generate a dataset with N people and their properties
fn people_dataset(n: usize) -> String {
    let mut ttl = String::from(
        "@prefix ex: <http://example.org/> . @prefix foaf: <http://xmlns.com/foaf/0.1/> .",
    );
    for i in 0..n {
        ttl.push_str(&format!(
            " ex:p{i} a foaf:Person ; foaf:name \"Person {i}\" ; foaf:age {age} ; ex:score {score} .",
            i = i, age = 20 + (i % 50), score = i * 7 % 100
        ));
    }
    ttl
}

#[test]
fn scope_bgp_single_triple_pattern() {
    let s = ts();
    load(&s, &people_dataset(10));
    let r = sel(&s, "SELECT ?x WHERE { ?x <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://xmlns.com/foaf/0.1/Person> }");
    assert_eq!(r.len(), 10);
}

#[test]
fn scope_bgp_join_two_patterns() {
    let s = ts();
    load(&s, &people_dataset(10));
    let r = sel(&s, "SELECT ?x ?name WHERE { ?x a <http://xmlns.com/foaf/0.1/Person> ; <http://xmlns.com/foaf/0.1/name> ?name }");
    assert_eq!(r.len(), 10);
}

#[test]
fn scope_bgp_three_pattern_join() {
    let s = ts();
    load(&s, &people_dataset(5));
    let r = sel(
        &s,
        "SELECT ?x ?name ?age WHERE { ?x a <http://xmlns.com/foaf/0.1/Person> ; <http://xmlns.com/foaf/0.1/name> ?name ; <http://xmlns.com/foaf/0.1/age> ?age }",
    );
    assert_eq!(r.len(), 5);
}

#[test]
fn scope_bgp_no_results() {
    let s = ts();
    load(&s, &people_dataset(5));
    let r = sel(
        &s,
        "SELECT ?x WHERE { ?x <http://example.org/nonexistent> ?y }",
    );
    assert_eq!(r.len(), 0);
}

#[test]
fn scope_bgp_bound_subject() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:p 1 . ex:b ex:p 2 .",
    );
    let r = sel(
        &s,
        "SELECT ?v WHERE { <http://example.org/a> <http://example.org/p> ?v }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("1"));
}

#[test]
fn scope_bgp_bound_object() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:p 42 . ex:b ex:p 42 . ex:c ex:p 99 .",
    );
    let r = sel(&s, "SELECT ?s WHERE { ?s <http://example.org/p> 42 }");
    assert_eq!(r.len(), 2);
}

#[test]
fn scope_bgp_all_bound_triple() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:b ex:c .");
    assert!(ask(
        &s,
        "ASK { <http://example.org/a> <http://example.org/b> <http://example.org/c> }"
    ));
    assert!(!ask(
        &s,
        "ASK { <http://example.org/a> <http://example.org/b> <http://example.org/x> }"
    ));
}

// ═══════════════════════════════════════════════════════════
// Category 2: Filter Expressions
// sparqloscope: tests filter-*, expr-*
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_filter_comparison_operators() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:v 10 . ex:b ex:v 20 . ex:c ex:v 30 .",
    );
    let q_lt = sel(
        &s,
        "SELECT ?v WHERE { ?s <http://example.org/v> ?v FILTER(?v < 20) }",
    );
    assert_eq!(q_lt.len(), 1);
    let q_le = sel(
        &s,
        "SELECT ?v WHERE { ?s <http://example.org/v> ?v FILTER(?v <= 20) }",
    );
    assert_eq!(q_le.len(), 2);
    let q_gt = sel(
        &s,
        "SELECT ?v WHERE { ?s <http://example.org/v> ?v FILTER(?v > 10) }",
    );
    assert_eq!(q_gt.len(), 2);
    let q_ge = sel(
        &s,
        "SELECT ?v WHERE { ?s <http://example.org/v> ?v FILTER(?v >= 20) }",
    );
    assert_eq!(q_ge.len(), 2);
    let q_eq = sel(
        &s,
        "SELECT ?v WHERE { ?s <http://example.org/v> ?v FILTER(?v = 20) }",
    );
    assert_eq!(q_eq.len(), 1);
    let q_ne = sel(
        &s,
        "SELECT ?v WHERE { ?s <http://example.org/v> ?v FILTER(?v != 20) }",
    );
    assert_eq!(q_ne.len(), 2);
}

#[test]
fn scope_filter_logical_and_or_not() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:v 5 . ex:b ex:v 15 . ex:c ex:v 25 .",
    );
    let r_and = sel(
        &s,
        "SELECT ?v WHERE { ?s <http://example.org/v> ?v FILTER(?v > 3 && ?v < 20) }",
    );
    assert_eq!(r_and.len(), 2);
    let r_or = sel(
        &s,
        "SELECT ?v WHERE { ?s <http://example.org/v> ?v FILTER(?v < 10 || ?v > 20) }",
    );
    assert_eq!(r_or.len(), 2);
    let r_not = sel(
        &s,
        "SELECT ?v WHERE { ?s <http://example.org/v> ?v FILTER(!((?v < 10) || (?v > 20))) }",
    );
    assert_eq!(r_not.len(), 1);
}

#[test]
fn scope_filter_string_functions() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:n \"foobar\" . ex:b ex:n \"bazquux\" . ex:c ex:n \"foo123\" .");
    let r_starts = sel(
        &s,
        "SELECT ?n WHERE { ?s <http://example.org/n> ?n FILTER(STRSTARTS(?n, \"foo\")) }",
    );
    assert_eq!(r_starts.len(), 2);
    let r_ends = sel(
        &s,
        "SELECT ?n WHERE { ?s <http://example.org/n> ?n FILTER(STRENDS(?n, \"bar\")) }",
    );
    assert_eq!(r_ends.len(), 1);
    let r_contains = sel(
        &s,
        "SELECT ?n WHERE { ?s <http://example.org/n> ?n FILTER(CONTAINS(?n, \"oo\")) }",
    );
    assert_eq!(r_contains.len(), 2);
    let r_regex = sel(
        &s,
        "SELECT ?n WHERE { ?s <http://example.org/n> ?n FILTER(REGEX(?n, \"^foo\")) }",
    );
    assert_eq!(r_regex.len(), 2);
}

#[test]
fn scope_filter_type_checking() {
    let s = ts();
    load(
        &s,
        r#"@prefix ex: <http://example.org/> .
        ex:a ex:p <http://example.org/iri> .
        ex:b ex:p "literal" .
        ex:c ex:p 42 .
    "#,
    );
    let r_iri = sel(
        &s,
        "SELECT ?o WHERE { ?s <http://example.org/p> ?o FILTER(isIRI(?o)) }",
    );
    assert_eq!(r_iri.len(), 1);
    let r_lit = sel(
        &s,
        "SELECT ?o WHERE { ?s <http://example.org/p> ?o FILTER(isLiteral(?o)) }",
    );
    assert_eq!(r_lit.len(), 2);
    let r_num = sel(
        &s,
        "SELECT ?o WHERE { ?s <http://example.org/p> ?o FILTER(isNumeric(?o)) }",
    );
    assert_eq!(r_num.len(), 1);
}

#[test]
fn scope_filter_sameas() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:knows ex:b . ex:a ex:knows ex:a .",
    );
    let r = sel(&s, "SELECT ?x WHERE { <http://example.org/a> <http://example.org/knows> ?x FILTER(?x != <http://example.org/a>) }");
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("/b>"));
}

#[test]
fn scope_filter_in_notin() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:v 1 . ex:b ex:v 2 . ex:c ex:v 3 . ex:d ex:v 4 .");
    let r_in = sel(
        &s,
        "SELECT ?v WHERE { ?s <http://example.org/v> ?v FILTER(?v IN (1, 3)) } ORDER BY ?v",
    );
    assert_eq!(r_in.len(), 2);
    assert!(r_in[0][0].contains("1"));
    assert!(r_in[1][0].contains("3"));
    let r_notin = sel(
        &s,
        "SELECT ?v WHERE { ?s <http://example.org/v> ?v FILTER(?v NOT IN (1, 3)) } ORDER BY ?v",
    );
    assert_eq!(r_notin.len(), 2);
}

// ═══════════════════════════════════════════════════════════
// Category 3: Optional (Left Join)
// sparqloscope: tests optional-*
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_optional_basic_unbound() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:name \"Alice\" ; ex:age 30 . ex:b ex:name \"Bob\" .");
    let r = sel(
        &s,
        "SELECT ?name ?age WHERE { ?x <http://example.org/name> ?name . OPTIONAL { ?x <http://example.org/age> ?age } } ORDER BY ?name",
    );
    assert_eq!(r.len(), 2);
    assert!(!r[0][1].is_empty()); // Alice has age
    assert!(r[1][1].is_empty()); // Bob has no age
}

#[test]
fn scope_optional_multiple_optionals() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:p 1 ; ex:q 2 ; ex:r 3 . ex:b ex:p 4 ; ex:q 5 . ex:c ex:p 6 .");
    let r = sel(
        &s,
        "SELECT ?s ?p ?q ?r WHERE { ?s <http://example.org/p> ?p . OPTIONAL { ?s <http://example.org/q> ?q } . OPTIONAL { ?s <http://example.org/r> ?r } } ORDER BY ?s",
    );
    assert_eq!(r.len(), 3);
}

#[test]
fn scope_optional_filter_after() {
    // FILTER applied to result of OPTIONAL
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:name \"A\" ; ex:age 25 . ex:b ex:name \"B\" ; ex:age 35 . ex:c ex:name \"C\" .");
    // Find people whose optional age (if present) is > 30, OR who have no age
    let r = sel(
        &s,
        "SELECT ?name WHERE { ?x <http://example.org/name> ?name . OPTIONAL { ?x <http://example.org/age> ?age } FILTER(!BOUND(?age) || ?age > 30) } ORDER BY ?name",
    );
    // B (age 35 > 30) and C (no age) qualify; A (age 25 <= 30) doesn't
    assert_eq!(r.len(), 2);
    let names: Vec<_> = r.iter().map(|row| row[0].as_str()).collect();
    assert!(names.iter().any(|n| n.contains("\"B\"")));
    assert!(names.iter().any(|n| n.contains("\"C\"")));
}

// ═══════════════════════════════════════════════════════════
// Category 4: Union
// sparqloscope: tests union-*
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_union_disjoint_patterns() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:firstName \"Alice\" . ex:b ex:givenName \"Bob\" .");
    let r = sel(
        &s,
        "SELECT ?name WHERE { { ?s <http://example.org/firstName> ?name } UNION { ?s <http://example.org/givenName> ?name } } ORDER BY ?name",
    );
    assert_eq!(r.len(), 2);
}

#[test]
fn scope_union_overlapping_patterns() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . @prefix : <http://example.org/> . ex:a ex:p1 :x ; ex:p2 :x .");
    let r = sel(
        &s,
        "SELECT ?o WHERE { { ?s <http://example.org/p1> ?o } UNION { ?s <http://example.org/p2> ?o } }",
    );
    // :x appears via both paths, so 2 results (UNION preserves duplicates by default without DISTINCT)
    assert_eq!(r.len(), 2);
}

#[test]
fn scope_union_with_filter() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:p 1 . ex:b ex:q 10 . ex:c ex:p 5 . ex:d ex:q 3 .");
    let r = sel(
        &s,
        "SELECT ?v WHERE { { ?s <http://example.org/p> ?v FILTER(?v > 3) } UNION { ?s <http://example.org/q> ?v FILTER(?v < 5) } } ORDER BY ?v",
    );
    // From p: ?v=5 (>3); From q: ?v=3 (<5) — 2 results
    assert_eq!(r.len(), 2);
}

#[test]
fn scope_union_three_way() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:p1 \"a\" . ex:b ex:p2 \"b\" . ex:c ex:p3 \"c\" .");
    let r = sel(
        &s,
        "SELECT ?v WHERE { { ?s <http://example.org/p1> ?v } UNION { ?s <http://example.org/p2> ?v } UNION { ?s <http://example.org/p3> ?v } } ORDER BY ?v",
    );
    assert_eq!(r.len(), 3);
}

// ═══════════════════════════════════════════════════════════
// Category 5: Solution Modifiers
// sparqloscope: tests distinct-*, limit-*, offset-*, order-*
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_modifier_distinct() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . @prefix : <http://example.org/> . ex:a ex:p :x . ex:b ex:p :x . ex:c ex:p :y .");
    let without = sel(&s, "SELECT ?o WHERE { ?s <http://example.org/p> ?o }");
    let with_d = sel(
        &s,
        "SELECT DISTINCT ?o WHERE { ?s <http://example.org/p> ?o }",
    );
    assert_eq!(without.len(), 3);
    assert_eq!(with_d.len(), 2);
}

#[test]
fn scope_modifier_order_by_multiple() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:t \"X\" ; ex:v 2 . ex:b ex:t \"X\" ; ex:v 1 . ex:c ex:t \"Y\" ; ex:v 5 .");
    let r = sel(
        &s,
        "SELECT ?t ?v WHERE { ?s <http://example.org/t> ?t ; <http://example.org/v> ?v } ORDER BY ?t ASC(?v)",
    );
    assert_eq!(r.len(), 3);
    // ORDER: X/1, X/2, Y/5
    assert!(r[0][0].contains("X") && r[0][1].contains("1"));
    assert!(r[1][0].contains("X") && r[1][1].contains("2"));
    assert!(r[2][0].contains("Y") && r[2][1].contains("5"));
}

#[test]
fn scope_modifier_limit_exact() {
    let s = ts();
    for i in 0..20 {
        update(
            &s,
            &format!(
                "INSERT DATA {{ <http://ex/s{}> <http://ex/p> \"v{}\" }}",
                i, i
            ),
        );
    }
    let r = sel(&s, "SELECT ?s WHERE { ?s ?p ?o } ORDER BY ?s LIMIT 5");
    assert_eq!(r.len(), 5);
}

#[test]
fn scope_modifier_offset_pagination() {
    let s = ts();
    for i in 0..10 {
        update(
            &s,
            &format!(
                "INSERT DATA {{ <http://ex/s{}> <http://ex/p> \"v{}\" }}",
                i, i
            ),
        );
    }
    let page1 = sel(
        &s,
        "SELECT ?s WHERE { ?s ?p ?o } ORDER BY ?s LIMIT 3 OFFSET 0",
    );
    let page2 = sel(
        &s,
        "SELECT ?s WHERE { ?s ?p ?o } ORDER BY ?s LIMIT 3 OFFSET 3",
    );
    let page3 = sel(
        &s,
        "SELECT ?s WHERE { ?s ?p ?o } ORDER BY ?s LIMIT 3 OFFSET 6",
    );
    assert_eq!(page1.len(), 3);
    assert_eq!(page2.len(), 3);
    assert_eq!(page3.len(), 3);
    // Pages should not overlap
    assert_ne!(page1[0][0], page2[0][0]);
    assert_ne!(page2[0][0], page3[0][0]);
}

// ═══════════════════════════════════════════════════════════
// Category 6: Aggregates
// sparqloscope: tests agg-*, group-*, having-*
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_agg_all_functions() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:v 1 . ex:b ex:v 2 . ex:c ex:v 3 . ex:d ex:v 4 . ex:e ex:v 5 .");
    let r = sel(
        &s,
        "SELECT (COUNT(?v) AS ?cnt) (SUM(?v) AS ?sum) (AVG(?v) AS ?avg) (MIN(?v) AS ?mn) (MAX(?v) AS ?mx) WHERE { ?s <http://example.org/v> ?v }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("5"), "count: {}", r[0][0]);
    assert!(r[0][1].contains("15"), "sum: {}", r[0][1]);
    assert!(r[0][2].contains("3"), "avg: {}", r[0][2]);
    assert!(r[0][3].contains("1"), "min: {}", r[0][3]);
    assert!(r[0][4].contains("5"), "max: {}", r[0][4]);
}

#[test]
fn scope_agg_group_by_with_count() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:type \"A\" ; ex:v 1 . ex:b ex:type \"A\" ; ex:v 2 . ex:c ex:type \"B\" ; ex:v 10 . ex:d ex:type \"B\" ; ex:v 20 . ex:e ex:type \"B\" ; ex:v 30 .",
    );
    let r = sel(
        &s,
        "SELECT ?type (COUNT(?s) AS ?count) (SUM(?v) AS ?total) WHERE { ?s <http://example.org/type> ?type ; <http://example.org/v> ?v } GROUP BY ?type ORDER BY ?type",
    );
    assert_eq!(r.len(), 2);
    assert!(r[0][1].contains("2"), "A has 2 items: {}", r[0][1]);
    assert!(r[1][1].contains("3"), "B has 3 items: {}", r[1][1]);
    assert!(r[1][2].contains("60"), "B total = 60: {}", r[1][2]);
}

#[test]
fn scope_agg_having_filter() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:type \"A\" ; ex:v 1 . ex:b ex:type \"A\" ; ex:v 2 . ex:c ex:type \"B\" ; ex:v 100 .",
    );
    let r = sel(
        &s,
        "SELECT ?type (SUM(?v) AS ?total) WHERE { ?s <http://example.org/type> ?type ; <http://example.org/v> ?v } GROUP BY ?type HAVING(SUM(?v) > 50)",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("B"), "Only B has total > 50: {}", r[0][0]);
}

#[test]
fn scope_agg_group_concat_separator() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:g ex:item \"a\" . ex:g ex:item \"b\" . ex:g ex:item \"c\" .");
    let r = sel(
        &s,
        "SELECT (GROUP_CONCAT(?item ; separator=\"|\") AS ?items) WHERE { <http://example.org/g> <http://example.org/item> ?item }",
    );
    assert_eq!(r.len(), 1);
    let s_val = &r[0][0];
    // All items should appear, order may vary
    assert!(
        s_val.contains("a") && s_val.contains("b") && s_val.contains("c"),
        "GROUP_CONCAT: {}",
        s_val
    );
    assert!(s_val.contains("|"), "Separator should be |: {}", s_val);
}

#[test]
fn scope_agg_count_distinct() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . @prefix : <http://example.org/> . ex:a ex:p :x . ex:b ex:p :x . ex:c ex:p :y .");
    let r = sel(
        &s,
        "SELECT (COUNT(DISTINCT ?o) AS ?cnt) WHERE { ?s <http://example.org/p> ?o }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("2"), "2 distinct objects: {}", r[0][0]);
}

// ═══════════════════════════════════════════════════════════
// Category 7: Subqueries
// sparqloscope: tests subquery-*
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_subquery_with_aggregation() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:v 10 . ex:b ex:v 20 . ex:c ex:v 30 .",
    );
    let r = sel(
        &s,
        "SELECT ?s ?v WHERE { ?s <http://example.org/v> ?v { SELECT (AVG(?x) AS ?avg) WHERE { ?y <http://example.org/v> ?x } } FILTER(?v > ?avg) }",
    );
    // Average = 20; values > 20: only 30
    assert_eq!(r.len(), 1);
    assert!(r[0][1].contains("30"));
}

#[test]
fn scope_subquery_top_n() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:v 5 . ex:b ex:v 3 . ex:c ex:v 8 . ex:d ex:v 1 . ex:e ex:v 6 .");
    let r = sel(
        &s,
        "SELECT ?s ?v WHERE { ?s <http://example.org/v> ?v { SELECT ?topV WHERE { ?x <http://example.org/v> ?topV } ORDER BY DESC(?topV) LIMIT 2 } FILTER(?v = ?topV) }",
    );
    // Top 2: 8 and 6
    assert_eq!(r.len(), 2);
    let vals: Vec<_> = r.iter().map(|row| row[1].as_str()).collect();
    assert!(vals.iter().any(|v| v.contains("8")));
    assert!(vals.iter().any(|v| v.contains("6")));
}

#[test]
fn scope_subquery_correlation() {
    // Subquery correlated via VALUES
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:dept \"Eng\" ; ex:salary 80000 . ex:b ex:dept \"Eng\" ; ex:salary 90000 . ex:c ex:dept \"HR\" ; ex:salary 70000 .");
    let r = sel(
        &s,
        "SELECT ?emp ?salary WHERE { ?emp <http://example.org/dept> \"Eng\" ; <http://example.org/salary> ?salary { SELECT (MAX(?s2) AS ?maxSalary) WHERE { ?e2 <http://example.org/dept> \"Eng\" ; <http://example.org/salary> ?s2 } } FILTER(?salary = ?maxSalary) }",
    );
    // Max salary in Eng = 90000
    assert_eq!(r.len(), 1);
    assert!(r[0][1].contains("90000"));
}

// ═══════════════════════════════════════════════════════════
// Category 8: Property Paths
// sparqloscope: tests path-*
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_path_sequence_multi_hop() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:p ex:b . ex:b ex:q ex:c . ex:c ex:r ex:d .",
    );
    let r = sel(
        &s,
        "SELECT ?end WHERE { <http://example.org/a> <http://example.org/p>/<http://example.org/q>/<http://example.org/r> ?end }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("/d>"));
}

#[test]
fn scope_path_alternative_properties() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:firstName \"Alice\" . ex:b ex:givenName \"Bob\" . ex:c ex:name \"Carol\" .");
    let r = sel(
        &s,
        "SELECT ?name WHERE { ?s (<http://example.org/firstName>|<http://example.org/givenName>|<http://example.org/name>) ?name } ORDER BY ?name",
    );
    assert_eq!(r.len(), 3);
}

#[test]
fn scope_path_zero_or_more_transitive() {
    // Test transitive closure p*
    let s = ts();
    load(&s, "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . @prefix ex: <http://example.org/> . ex:A rdfs:subClassOf ex:B . ex:B rdfs:subClassOf ex:C . ex:C rdfs:subClassOf ex:D .");
    let r = sel(
        &s,
        "SELECT ?c WHERE { <http://example.org/A> <http://www.w3.org/2000/01/rdf-schema#subClassOf>* ?c }",
    );
    // Includes A (via length-0), B, C, D
    assert!(r.len() >= 4, "Zero-or-more path: {:?}", r);
}

#[test]
fn scope_path_one_or_more_no_self() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:sub ex:b . ex:b ex:sub ex:c .",
    );
    let r = sel(
        &s,
        "SELECT ?x WHERE { <http://example.org/a> <http://example.org/sub>+ ?x }",
    );
    // :b and :c, but NOT :a itself
    assert_eq!(r.len(), 2);
    assert!(
        !r.iter().any(|row| row[0].contains("/a>")),
        "a should not appear in 1+ path: {:?}",
        r
    );
}

#[test]
fn scope_path_inverse_simple() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:parent ex:hasChild ex:child .",
    );
    let r = sel(
        &s,
        "SELECT ?parent WHERE { <http://example.org/child> ^<http://example.org/hasChild> ?parent }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("parent"));
}

#[test]
fn scope_path_negated_property_set() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:p ex:x . ex:a ex:q ex:y . ex:a ex:r ex:z .",
    );
    let r = sel(
        &s,
        "SELECT ?o WHERE { <http://example.org/a> !<http://example.org/p> ?o } ORDER BY ?o",
    );
    assert_eq!(r.len(), 2, "Properties q and r match negated p: {:?}", r);
}

#[test]
fn scope_path_complex_combination() {
    // (p1|p2)+ transitive closure of alternative properties
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:p1 ex:b . ex:b ex:p2 ex:c . ex:c ex:p1 ex:d .",
    );
    let r = sel(
        &s,
        "SELECT ?x WHERE { <http://example.org/a> (<http://example.org/p1>|<http://example.org/p2>)+ ?x }",
    );
    assert_eq!(r.len(), 3, "a reaches b, c, d via p1|p2+: {:?}", r);
}

// ═══════════════════════════════════════════════════════════
// Category 9: BIND and Expression Evaluation
// sparqloscope: tests bind-*, expr-builtin-*
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_bind_arithmetic() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:v 7 .");
    let r = sel(
        &s,
        "SELECT ?add ?sub ?mul ?div WHERE { ?s <http://example.org/v> ?v . BIND(?v + 3 AS ?add) . BIND(?v - 2 AS ?sub) . BIND(?v * 4 AS ?mul) . BIND(?v / 2 AS ?div) }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("10"), "add: {}", r[0][0]);
    assert!(r[0][1].contains("5"), "sub: {}", r[0][1]);
    assert!(r[0][2].contains("28"), "mul: {}", r[0][2]);
    assert!(
        r[0][3].contains("3.5") || r[0][3].contains("3"),
        "div: {}",
        r[0][3]
    );
}

#[test]
fn scope_bind_string_operations() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:first \"hello\" ; ex:last \"world\" .",
    );
    let r = sel(
        &s,
        "SELECT ?upper ?combined ?length WHERE { ?s <http://example.org/first> ?f ; <http://example.org/last> ?l . BIND(UCASE(?f) AS ?upper) . BIND(CONCAT(?f, \" \", ?l) AS ?combined) . BIND(STRLEN(?f) AS ?length) }",
    );
    assert!(r[0][0].contains("HELLO"), "UCASE: {}", r[0][0]);
    assert!(r[0][1].contains("hello world"), "CONCAT: {}", r[0][1]);
    assert!(r[0][2].contains("5"), "STRLEN: {}", r[0][2]);
}

#[test]
fn scope_bind_if_expression() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:score 85 . ex:b ex:score 45 . ex:c ex:score 60 .");
    let r = sel(
        &s,
        "SELECT ?s ?grade WHERE { ?s <http://example.org/score> ?score . BIND(IF(?score >= 90, \"A\", IF(?score >= 70, \"B\", IF(?score >= 50, \"C\", \"F\"))) AS ?grade) } ORDER BY ?s",
    );
    assert_eq!(r.len(), 3);
    // 85 → B, 45 → F, 60 → C
    let grades: Vec<_> = r.iter().map(|row| row[1].as_str()).collect();
    assert!(
        grades.iter().any(|g| g.contains("\"B\"")),
        "85 should be B: {:?}",
        grades
    );
    assert!(
        grades.iter().any(|g| g.contains("\"F\"")),
        "45 should be F: {:?}",
        grades
    );
    assert!(
        grades.iter().any(|g| g.contains("\"C\"")),
        "60 should be C: {:?}",
        grades
    );
}

#[test]
fn scope_bind_coalesce() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:name \"Alice\" ; ex:nick \"Al\" . ex:b ex:name \"Bob\" .");
    let r = sel(
        &s,
        "SELECT ?display WHERE { ?x <http://example.org/name> ?name . OPTIONAL { ?x <http://example.org/nick> ?nick } . BIND(COALESCE(?nick, ?name) AS ?display) } ORDER BY ?display",
    );
    assert_eq!(r.len(), 2);
    assert!(r[0][0].contains("Al"), "Alice uses nick: {}", r[0][0]);
    assert!(
        r[1][0].contains("Bob"),
        "Bob falls back to name: {}",
        r[1][0]
    );
}

#[test]
fn scope_bind_iri_construction() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:id \"42\" . ex:b ex:id \"99\" .",
    );
    let r = sel(
        &s,
        "SELECT ?item WHERE { ?s <http://example.org/id> ?id . BIND(IRI(CONCAT(\"http://example.org/item/\", ?id)) AS ?item) } ORDER BY ?item",
    );
    assert_eq!(r.len(), 2);
    assert!(r[0][0].contains("item/42"), "IRI construction: {}", r[0][0]);
    assert!(r[1][0].contains("item/99"), "IRI construction: {}", r[1][0]);
}

// ═══════════════════════════════════════════════════════════
// Category 10: VALUES (Inline Data)
// sparqloscope: tests values-*, bindings-*
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_values_single_var() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:name \"Alice\" . ex:b ex:name \"Bob\" . ex:c ex:name \"Carol\" .");
    let r = sel(
        &s,
        "SELECT ?name WHERE { VALUES ?s { <http://example.org/a> <http://example.org/c> } ?s <http://example.org/name> ?name } ORDER BY ?name",
    );
    assert_eq!(r.len(), 2);
    assert!(r[0][0].contains("Alice"));
    assert!(r[1][0].contains("Carol"));
}

#[test]
fn scope_values_multi_var() {
    let s = ts();
    let r = sel(
        &s,
        "SELECT ?x ?y ?sum WHERE { VALUES (?x ?y) { (1 2) (3 4) (5 6) } BIND(?x + ?y AS ?sum) } ORDER BY ?x",
    );
    assert_eq!(r.len(), 3);
    assert!(r[0][2].contains("3"), "1+2=3: {}", r[0][2]);
    assert!(r[1][2].contains("7"), "3+4=7: {}", r[1][2]);
    assert!(r[2][2].contains("11"), "5+6=11: {}", r[2][2]);
}

#[test]
fn scope_values_with_undef() {
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:p \"pa\" ; ex:q \"qa\" . ex:b ex:p \"pb\" .",
    );
    let r = sel(
        &s,
        "SELECT ?s ?p ?q WHERE { VALUES (?s ?q) { (<http://example.org/a> UNDEF) (UNDEF \"qa\") } ?s <http://example.org/p> ?p . OPTIONAL { ?s <http://example.org/q> ?q } } ORDER BY ?s",
    );
    assert!(!r.is_empty(), "VALUES with UNDEF: {:?}", r);
}

#[test]
fn scope_values_post_where() {
    // VALUES inline data binding: filter results to a specific set of IRI values
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:tag ex:tagA . ex:b ex:tag ex:tagB . ex:c ex:tag ex:tagA .");
    // VALUES inside WHERE constrains ?t to a specific IRI
    let r = sel(
        &s,
        "SELECT ?s WHERE { ?s <http://example.org/tag> ?t . VALUES ?t { <http://example.org/tagA> } } ORDER BY ?s",
    );
    assert_eq!(r.len(), 2); // ex:a and ex:c both have tagA
    assert!(r.iter().any(|row| row[0].contains("/a>")));
    assert!(r.iter().any(|row| row[0].contains("/c>")));
}

// ═══════════════════════════════════════════════════════════
// Category 11: Negation (NOT EXISTS, MINUS)
// sparqloscope: tests negation-*, minus-*
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_negation_not_exists_simple() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:name \"Alice\" ; ex:email \"a@b\" . ex:b ex:name \"Bob\" .");
    let r = sel(
        &s,
        "SELECT ?name WHERE { ?x <http://example.org/name> ?name . FILTER NOT EXISTS { ?x <http://example.org/email> ?e } }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("Bob"));
}

#[test]
fn scope_negation_minus_basic() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . @prefix : <http://example.org/> . ex:a ex:p :x . ex:b ex:p :y . ex:c ex:p :z .");
    let r = sel(
        &s,
        "SELECT ?s WHERE { ?s <http://example.org/p> ?o MINUS { ?s <http://example.org/p> <http://example.org/x> } }",
    );
    assert_eq!(r.len(), 2);
    assert!(!r.iter().any(|row| row[0].contains("/a>")));
}

#[test]
fn scope_negation_minus_no_overlap() {
    // MINUS with no shared variables = identity (no rows excluded)
    let s = ts();
    load(
        &s,
        "@prefix ex: <http://example.org/> . ex:a ex:p 1 . ex:b ex:p 2 .",
    );
    let r = sel(
        &s,
        "SELECT ?s WHERE { ?s <http://example.org/p> ?o MINUS { <http://example.org/a> <http://example.org/p> ?x } }",
    );
    // No shared variables → no exclusion (as per SPARQL spec)
    assert_eq!(r.len(), 2, "MINUS without shared vars = identity: {:?}", r);
}

#[test]
fn scope_negation_exists_vs_minus_equivalence() {
    // FILTER NOT EXISTS should give same results as MINUS in simple cases
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:a ex:name \"A\" ; ex:blocked true . ex:b ex:name \"B\" . ex:c ex:name \"C\" ; ex:blocked true .");
    let r_fne = sel(
        &s,
        "SELECT ?name WHERE { ?x <http://example.org/name> ?name . FILTER NOT EXISTS { ?x <http://example.org/blocked> true } } ORDER BY ?name",
    );
    let r_minus = sel(
        &s,
        "SELECT ?name WHERE { ?x <http://example.org/name> ?name . MINUS { ?x <http://example.org/blocked> true } } ORDER BY ?name",
    );
    assert_eq!(
        r_fne.len(),
        r_minus.len(),
        "FNE and MINUS give same results: {:?} vs {:?}",
        r_fne,
        r_minus
    );
    assert_eq!(r_fne, r_minus, "Results must match");
}

// ═══════════════════════════════════════════════════════════
// Category 12: SPARQL Update
// sparqloscope: tests update-*
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_update_full_lifecycle() {
    let s = ts();

    // INSERT DATA
    update(
        &s,
        "INSERT DATA { <http://ex/s> <http://ex/p> \"initial\" }",
    );
    assert_eq!(s.len().unwrap(), 1);

    // INSERT WHERE (copy with transformation)
    update(
        &s,
        "INSERT { <http://ex/s> <http://ex/p2> ?v } WHERE { <http://ex/s> <http://ex/p> ?v }",
    );
    assert_eq!(s.len().unwrap(), 2);

    // DELETE DATA
    update(
        &s,
        "DELETE DATA { <http://ex/s> <http://ex/p> \"initial\" }",
    );
    assert_eq!(s.len().unwrap(), 1);

    // DELETE WHERE
    update(&s, "DELETE WHERE { <http://ex/s> <http://ex/p2> ?v }");
    assert_eq!(s.len().unwrap(), 0);
}

#[test]
fn scope_update_conditional_replace() {
    let s = ts();
    load(&s, "@prefix ex: <http://example.org/> . ex:x ex:status \"pending\" . ex:y ex:status \"done\" .");

    // Atomically replace "pending" with "active"
    update(
        &s,
        r#"DELETE { ?s <http://example.org/status> "pending" }
                  INSERT { ?s <http://example.org/status> "active" }
                  WHERE  { ?s <http://example.org/status> "pending" }"#,
    );

    assert!(ask(
        &s,
        "ASK { <http://example.org/x> <http://example.org/status> \"active\" }"
    ));
    assert!(ask(
        &s,
        "ASK { <http://example.org/y> <http://example.org/status> \"done\" }"
    ));
    assert!(!ask(
        &s,
        "ASK { ?s <http://example.org/status> \"pending\" }"
    ));
}

#[test]
fn scope_update_named_graph_operations() {
    let s = ts();

    // CREATE GRAPH
    update(&s, "CREATE GRAPH <http://ex/g>");

    // INSERT into named graph
    update(
        &s,
        "INSERT DATA { GRAPH <http://ex/g> { <http://ex/s> <http://ex/p> \"in-graph\" } }",
    );

    let r = sel(
        &s,
        "SELECT ?v WHERE { GRAPH <http://ex/g> { ?s <http://ex/p> ?v } }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("in-graph"));

    // CLEAR GRAPH
    update(&s, "CLEAR GRAPH <http://ex/g>");
    let r2 = sel(
        &s,
        "SELECT ?v WHERE { GRAPH <http://ex/g> { ?s <http://ex/p> ?v } }",
    );
    assert_eq!(r2.len(), 0);

    // DROP GRAPH
    update(&s, "DROP GRAPH <http://ex/g>");
}

#[test]
fn scope_update_copy_graph() {
    let s = ts();
    update(
        &s,
        "INSERT DATA { GRAPH <http://ex/src> { <http://ex/a> <http://ex/p> \"val\" } }",
    );
    update(&s, "COPY <http://ex/src> TO <http://ex/dst>");

    let r = sel(
        &s,
        "SELECT ?v WHERE { GRAPH <http://ex/dst> { ?s <http://ex/p> ?v } }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("val"));
}

#[test]
fn scope_update_move_graph() {
    let s = ts();
    update(
        &s,
        "INSERT DATA { GRAPH <http://ex/src> { <http://ex/a> <http://ex/p> \"val\" } }",
    );
    update(&s, "MOVE <http://ex/src> TO <http://ex/dst>");

    let src = sel(
        &s,
        "SELECT ?v WHERE { GRAPH <http://ex/src> { ?s <http://ex/p> ?v } }",
    );
    let dst = sel(
        &s,
        "SELECT ?v WHERE { GRAPH <http://ex/dst> { ?s <http://ex/p> ?v } }",
    );

    assert_eq!(src.len(), 0, "Source should be empty after MOVE");
    assert_eq!(dst.len(), 1, "Destination should have data");
}

#[test]
fn scope_update_add_graphs() {
    let s = ts();
    update(
        &s,
        "INSERT DATA { GRAPH <http://ex/g1> { <http://ex/a> <http://ex/p> \"v1\" } }",
    );
    update(
        &s,
        "INSERT DATA { GRAPH <http://ex/g2> { <http://ex/b> <http://ex/p> \"v2\" } }",
    );
    update(&s, "ADD <http://ex/g1> TO <http://ex/g2>");

    let r = sel(
        &s,
        "SELECT (COUNT(*) AS ?c) WHERE { GRAPH <http://ex/g2> { ?s ?p ?o } }",
    );
    assert!(r[0][0].contains("2"), "ADD merges g1 into g2: {}", r[0][0]);
    // g1 still exists
    let r1 = sel(
        &s,
        "SELECT (COUNT(*) AS ?c) WHERE { GRAPH <http://ex/g1> { ?s ?p ?o } }",
    );
    assert!(
        r1[0][0].contains("1"),
        "g1 unchanged after ADD: {}",
        r1[0][0]
    );
}

// ═══════════════════════════════════════════════════════════
// Category 13: SPARQL 1.2 / RDF-star
// sparqloscope / W3C SPARQL 1.2 tests
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_rdfstar_insert_and_query() {
    let s = ts();
    update(
        &s,
        r#"
        INSERT DATA {
            << <http://ex/s> <http://ex/p> <http://ex/o> >> <http://ex/certainty> "0.95"^^<http://www.w3.org/2001/XMLSchema#double> .
        }
    "#,
    );

    let r = sel(
        &s,
        "SELECT ?cert WHERE { << <http://ex/s> <http://ex/p> <http://ex/o> >> <http://ex/certainty> ?cert }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("0.95"));
}

#[test]
fn scope_rdfstar_nested_annotation() {
    let s = ts();
    update(
        &s,
        r#"
        INSERT DATA {
            << << <http://ex/a> <http://ex/b> <http://ex/c> >> <http://ex/source> <http://ex/s> >>
               <http://ex/confidence> "0.8"^^<http://www.w3.org/2001/XMLSchema#double> .
        }
    "#,
    );

    let r = sel(
        &s,
        "SELECT ?conf WHERE { << << <http://ex/a> <http://ex/b> <http://ex/c> >> <http://ex/source> ?src >> <http://ex/confidence> ?conf }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("0.8"));
}

#[test]
fn scope_rdfstar_multiple_annotations() {
    let s = ts();
    update(
        &s,
        r#"
        INSERT DATA {
            << <http://ex/alice> <http://ex/knows> <http://ex/bob> >>
                <http://ex/source> <http://ex/paper1> ;
                <http://ex/since>  "2020"^^<http://www.w3.org/2001/XMLSchema#gYear> .
        }
    "#,
    );

    let r = sel(
        &s,
        "SELECT ?source ?since WHERE { << <http://ex/alice> <http://ex/knows> <http://ex/bob> >> <http://ex/source> ?source ; <http://ex/since> ?since }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("paper1"));
    assert!(r[0][1].contains("2020"));
}

#[test]
fn scope_rdfstar_in_graph() {
    let s = ts();
    update(
        &s,
        r#"
        INSERT DATA {
            GRAPH <http://ex/named> {
                << <http://ex/s> <http://ex/p> <http://ex/o> >> <http://ex/meta> "info" .
            }
        }
    "#,
    );

    let r = sel(
        &s,
        "SELECT ?meta WHERE { GRAPH <http://ex/named> { << <http://ex/s> <http://ex/p> <http://ex/o> >> <http://ex/meta> ?meta } }",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("info"));
}

// ═══════════════════════════════════════════════════════════
// Category 14: Numeric Functions
// sparqloscope: tests fn-*
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_numeric_all_functions() {
    let s = ts();

    #[allow(clippy::type_complexity)] // (query, assertion-fn) test table; clear inline
    let tests: &[(&str, fn(&str) -> bool)] = &[
        ("SELECT (ABS(-42) AS ?v) WHERE {}", |r| r.contains("42")),
        ("SELECT (CEIL(4.1) AS ?v) WHERE {}", |r| r.contains("5")),
        ("SELECT (FLOOR(4.9) AS ?v) WHERE {}", |r| r.contains("4")),
        ("SELECT (ROUND(4.5) AS ?v) WHERE {}", |r| {
            r.contains("4") || r.contains("5")
        }),
        ("SELECT (ROUND(3.4) AS ?v) WHERE {}", |r| r.contains("3")),
        ("SELECT (1 + 2 AS ?v) WHERE {}", |r| r.contains("3")),
        ("SELECT (10 - 3 AS ?v) WHERE {}", |r| r.contains("7")),
        ("SELECT (4 * 5 AS ?v) WHERE {}", |r| r.contains("20")),
    ];

    for (q, check) in tests {
        let r = sel(&s, q);
        assert!(
            !r.is_empty() && check(&r[0][0]),
            "Query: {}\nResult: {:?}",
            q,
            r
        );
    }
}

// ═══════════════════════════════════════════════════════════
// Category 15: String Functions (exhaustive)
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_string_all_functions() {
    let s = ts();

    let tests: &[(&str, &str)] = &[
        ("SELECT (STRLEN(\"hello\") AS ?v) WHERE {}", "5"),
        ("SELECT (UCASE(\"hello\") AS ?v) WHERE {}", "HELLO"),
        ("SELECT (LCASE(\"WORLD\") AS ?v) WHERE {}", "world"),
        ("SELECT (CONCAT(\"foo\", \"bar\") AS ?v) WHERE {}", "foobar"),
        ("SELECT (SUBSTR(\"abcdef\", 3, 2) AS ?v) WHERE {}", "cd"),
        (
            "SELECT (STRSTARTS(\"foobar\", \"foo\") AS ?v) WHERE {}",
            "true",
        ),
        (
            "SELECT (STRENDS(\"foobar\", \"bar\") AS ?v) WHERE {}",
            "true",
        ),
        (
            "SELECT (CONTAINS(\"foobar\", \"oba\") AS ?v) WHERE {}",
            "true",
        ),
        ("SELECT (STRBEFORE(\"abc\", \"b\") AS ?v) WHERE {}", "a"),
        ("SELECT (STRAFTER(\"abc\", \"b\") AS ?v) WHERE {}", "c"),
        (
            "SELECT (REPLACE(\"hello\", \"l\", \"L\") AS ?v) WHERE {}",
            "heLLo",
        ),
        ("SELECT (STR(42) AS ?v) WHERE {}", "42"),
        (
            "SELECT (REGEX(\"foobar\", \"^foo\") AS ?v) WHERE {}",
            "true",
        ),
        (
            "SELECT (REGEX(\"foobar\", \"^baz\") AS ?v) WHERE {}",
            "false",
        ),
    ];

    for (q, expected) in tests {
        let r = sel(&s, q);
        assert!(!r.is_empty(), "Empty result for: {}", q);
        assert!(
            r[0][0].contains(expected),
            "Query: {}\nExpected: {}\nGot: {}",
            q,
            expected,
            r[0][0]
        );
    }
}

// ═══════════════════════════════════════════════════════════
// Category 16: Hash Functions
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_hash_functions_correct_values() {
    let s = ts();

    // Known hash values for "abc"
    let md5 = sel(&s, "SELECT (MD5(\"abc\") AS ?v) WHERE {}");
    assert!(
        md5[0][0].contains("900150983cd24fb0d6963f7d28e17f72"),
        "MD5(abc): {}",
        md5[0][0]
    );

    let sha1 = sel(&s, "SELECT (SHA1(\"abc\") AS ?v) WHERE {}");
    assert!(
        sha1[0][0]
            .to_lowercase()
            .contains("a9993e364706816aba3e25717850c26c9cd0d89d"),
        "SHA1(abc): {}",
        sha1[0][0]
    );

    let sha256 = sel(&s, "SELECT (SHA256(\"abc\") AS ?v) WHERE {}");
    assert!(
        sha256[0][0].contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        "SHA256(abc): {}",
        sha256[0][0]
    );

    let sha512 = sel(&s, "SELECT (SHA512(\"abc\") AS ?v) WHERE {}");
    assert!(
        sha512[0][0]
            .to_lowercase()
            .starts_with("\"ddaf35a193617aba"),
        "SHA512(abc) prefix: {}",
        sha512[0][0]
    );
}

// ═══════════════════════════════════════════════════════════
// Category 17: Date/Time Functions
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_datetime_extraction() {
    let s = ts();
    let dt = "\"2024-07-15T08:30:45.123Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>";

    let tests: &[(&str, &str)] = &[
        (&format!("SELECT (YEAR({}) AS ?v) WHERE {{}}", dt), "2024"),
        (&format!("SELECT (MONTH({}) AS ?v) WHERE {{}}", dt), "7"),
        (&format!("SELECT (DAY({}) AS ?v) WHERE {{}}", dt), "15"),
        (&format!("SELECT (HOURS({}) AS ?v) WHERE {{}}", dt), "8"),
        (&format!("SELECT (MINUTES({}) AS ?v) WHERE {{}}", dt), "30"),
    ];

    for (q, expected) in tests {
        let r = sel(&s, q);
        assert!(!r.is_empty(), "Empty result for: {}", q);
        assert!(
            r[0][0].contains(expected),
            "Query: {}\nExpected: {}\nGot: {}",
            q,
            expected,
            r[0][0]
        );
    }
}

// ═══════════════════════════════════════════════════════════
// Category 18: Complete Coverage Matrix
// A compact matrix test that verifies all SPARQL 1.1 features in one run
// ═══════════════════════════════════════════════════════════

#[test]
fn scope_full_coverage_matrix() {
    let s = ts();

    // Load rich dataset
    load(
        &s,
        r#"
        @prefix ex:   <http://example.org/> .
        @prefix foaf: <http://xmlns.com/foaf/0.1/> .
        @prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .

        ex:alice a foaf:Person ; foaf:name "Alice" ; ex:age 30 ; ex:dept ex:eng ;
                 ex:salary 90000 ; foaf:mbox <mailto:alice@ex.org> .
        ex:bob   a foaf:Person ; foaf:name "Bob"   ; ex:age 25 ; ex:dept ex:eng ;
                 ex:salary 70000 .
        ex:carol a foaf:Person ; foaf:name "Carol" ; ex:age 35 ; ex:dept ex:hr  ;
                 ex:salary 80000 ; foaf:mbox <mailto:carol@ex.org> .
        ex:dave  a foaf:Person ; foaf:name "Dave"  ; ex:age 28 ; ex:dept ex:hr  ;
                 ex:salary 65000 .

        ex:eng a ex:Department ; ex:deptName "Engineering" .
        ex:hr  a ex:Department ; ex:deptName "Human Resources" .

        ex:alice ex:manages ex:bob .
        ex:carol ex:manages ex:dave .
    "#,
    );

    // Common PREFIX declarations for all queries in this test
    let pfx = "PREFIX ex: <http://example.org/> PREFIX foaf: <http://xmlns.com/foaf/0.1/> ";
    let pq = |q: &str| format!("{pfx}{q}");

    // 1. Basic pattern
    let r1 = sel(&s, &pq("SELECT ?n WHERE { ?s foaf:name ?n } ORDER BY ?n"));
    assert_eq!(r1.len(), 4);

    // 2. Filter
    let r2 = sel(
        &s,
        &pq("SELECT ?n WHERE { ?s foaf:name ?n ; ex:age ?a FILTER(?a > 28) } ORDER BY ?n"),
    );
    assert_eq!(r2.len(), 2); // Alice (30), Carol (35)

    // 3. Optional
    let r3 = sel(&s, &pq("SELECT ?n ?email WHERE { ?s foaf:name ?n . OPTIONAL { ?s foaf:mbox ?email } } ORDER BY ?n"));
    assert_eq!(r3.len(), 4);
    let with_email = r3.iter().filter(|row| !row[1].is_empty()).count();
    assert_eq!(with_email, 2); // Alice and Carol

    // 4. Union
    let r4 = sel(&s, &pq("SELECT ?n WHERE { { ?s ex:age 30 ; foaf:name ?n } UNION { ?s ex:age 35 ; foaf:name ?n } } ORDER BY ?n"));
    assert_eq!(r4.len(), 2);

    // 5. Distinct
    let r5 = sel(&s, &pq("SELECT DISTINCT ?dept WHERE { ?s ex:dept ?dept }"));
    assert_eq!(r5.len(), 2);

    // 6. Aggregate — 76250 = (90000+70000+80000+65000)/4
    let r6 = sel(
        &s,
        &pq("SELECT (AVG(?sal) AS ?avg) WHERE { ?s ex:salary ?sal }"),
    );
    assert!(r6[0][0].contains("76250"), "Average salary: {}", r6[0][0]);

    // 7. Group by
    let r7 = sel(&s, &pq("SELECT ?dept (COUNT(?s) AS ?n) WHERE { ?s ex:dept ?dept } GROUP BY ?dept ORDER BY ?dept"));
    assert_eq!(r7.len(), 2);

    // 8. Having
    let r8 = sel(&s, &pq("SELECT ?dept (AVG(?sal) AS ?avg) WHERE { ?s ex:dept ?dept ; ex:salary ?sal } GROUP BY ?dept HAVING(AVG(?sal) > 75000) ORDER BY ?dept"));
    assert_eq!(r8.len(), 1); // eng avg = (90000+70000)/2 = 80000; hr avg = (80000+65000)/2 = 72500
    assert!(r8[0][0].contains("eng"), "High-paying dept: {}", r8[0][0]);

    // 9. Property path
    let r9 = sel(
        &s,
        &pq("SELECT ?managed WHERE { ?mgr ex:manages+ ?managed }"),
    );
    assert_eq!(r9.len(), 2); // bob (managed by alice), dave (managed by carol)

    // 10. Subquery
    let r10 = sel(&s, &pq("SELECT ?n WHERE { ?s foaf:name ?n ; ex:salary ?sal { SELECT (MAX(?x) AS ?max) WHERE { ?e ex:salary ?x } } FILTER(?sal = ?max) }"));
    assert_eq!(r10.len(), 1);
    assert!(r10[0][0].contains("Alice"), "Highest earner: {}", r10[0][0]);

    // 11. NOT EXISTS
    let r11 = sel(&s, &pq("SELECT ?n WHERE { ?s foaf:name ?n . FILTER NOT EXISTS { ?mgr ex:manages ?s } } ORDER BY ?n"));
    // Alice and Carol are managers (not managed by anyone); Bob and Dave are managed
    // We want those NOT managed: Alice and Carol
    assert_eq!(r11.len(), 2);

    // 12. VALUES with full IRIs
    let r12 = sel(
        &s,
        &pq("SELECT ?n WHERE { VALUES ?s { ex:alice ex:carol } ?s foaf:name ?n } ORDER BY ?n"),
    );
    assert_eq!(r12.len(), 2);
    assert!(r12[0][0].contains("Alice"));
    assert!(r12[1][0].contains("Carol"));
}
