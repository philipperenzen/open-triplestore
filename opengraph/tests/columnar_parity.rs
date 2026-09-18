//! The columnar evaluator against the engine: the same data in an in-memory
//! oxigraph store and in a `Columnar` copy, the same queries through both,
//! the same solutions — as multisets, and in order where the query orders.
//! Every operator and function the evaluator implements is exercised; a
//! query the evaluator declines must decline (never answer differently).

use std::collections::BTreeMap;

use opengraph::columnar::{accepts_text, Columnar};
use opengraph::parallel::ParAnswer;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;
use oxrdf::Term;

const DATA: &str = r#"
@prefix ex: <http://example.org/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:p1 ex:name "Alice" ; ex:age 31 ; ex:score 7.5 ; ex:type ex:T1 ; ex:email "alice@example.org" ; ex:born "1993-04-02T10:00:00Z"^^xsd:dateTime ; ex:knows ex:p2, ex:p3 ; ex:label "Alice"@en, "Alicia"@es .
ex:p2 ex:name "Bob" ; ex:age 45 ; ex:score 3.25 ; ex:type ex:T2 ; ex:email "bob@example.org" ; ex:born "1979-11-30T08:30:00Z"^^xsd:dateTime ; ex:knows ex:p3 ; ex:label "Bob"@en .
ex:p3 ex:name "Carol" ; ex:age 31 ; ex:score 9.0 ; ex:type ex:T1 ; ex:email "carol@example.org" ; ex:born "1993-01-15T00:00:00Z"^^xsd:dateTime ; ex:nick "cc" .
ex:p4 ex:name "Dan" ; ex:age 27 ; ex:score "n/a" ; ex:type ex:T3 ; ex:knows ex:p1 ; ex:weight 80.5e0 .
ex:p5 ex:name "Eve" ; ex:age "27"^^xsd:decimal ; ex:type ex:T2 ; ex:knows ex:p4 ; ex:flag true .
ex:T1 ex:label "Type one" . ex:T2 ex:label "Type two" .
_:b1 ex:name "Anon" ; ex:type ex:T3 .
"#;

const NAMED: &str = r#"
<http://example.org/p1> <http://example.org/city> "Paris" <http://example.org/g1> .
<http://example.org/p2> <http://example.org/city> "Rome" <http://example.org/g1> .
<http://example.org/p3> <http://example.org/city> "Oslo" <http://example.org/g2> .
<http://example.org/p1> <http://example.org/city> "Paris" <http://example.org/g2> .
"#;

fn stores() -> (Store, Columnar) {
    let store = Store::new().unwrap();
    store
        .load_from_reader(RdfFormat::Turtle, DATA.as_bytes())
        .unwrap();
    store
        .load_from_reader(RdfFormat::NQuads, NAMED.as_bytes())
        .unwrap();
    let columnar = Columnar::from_quads(store.iter().map(|q| q.unwrap()));
    (store, columnar)
}

/// A header, the solutions as rendered rows, and the boolean of an `ASK`.
type Answer = (Vec<String>, Vec<Vec<Option<String>>>, Option<bool>);

fn engine(store: &Store, q: &str) -> Answer {
    let r = SparqlEvaluator::new()
        .parse_query(q)
        .unwrap()
        .on_store(store)
        .execute()
        .unwrap();
    match r {
        QueryResults::Solutions(s) => {
            let vars: Vec<String> = s
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            let rows = s
                .map(|sol| {
                    let sol = sol.unwrap();
                    vars.iter()
                        .map(|v| sol.get(v.as_str()).map(render))
                        .collect::<Vec<_>>()
                })
                .collect();
            (vars, rows, None)
        }
        QueryResults::Boolean(b) => (vec![], vec![], Some(b)),
        QueryResults::Graph(g) => {
            let mut rows: Vec<Vec<Option<String>>> = g
                .map(|t| {
                    let t = t.unwrap();
                    vec![
                        Some(t.subject.to_string()),
                        Some(t.predicate.to_string()),
                        Some(render(&t.object)),
                    ]
                })
                .collect();
            rows.sort();
            rows.dedup();
            (vec!["s".into(), "p".into(), "o".into()], rows, None)
        }
    }
}

/// Blank nodes render as a placeholder: their labels differ between stores.
fn render(t: &Term) -> String {
    match t {
        Term::BlankNode(_) => "_:".to_string(),
        other => other.to_string(),
    }
}

fn columnar(c: &Columnar, q: &str) -> Option<Answer> {
    match c.query(q).unwrap()? {
        ParAnswer::Solutions { variables, rows } => Some((
            variables.iter().map(|v| v.as_str().to_string()).collect(),
            rows.iter()
                .map(|r| r.iter().map(|t| t.as_ref().map(render)).collect())
                .collect(),
            None,
        )),
        ParAnswer::Boolean(b) => Some((vec![], vec![], Some(b))),
        ParAnswer::Graph(triples) => {
            let mut rows: Vec<Vec<Option<String>>> = triples
                .iter()
                .map(|t| {
                    vec![
                        Some(t.subject.to_string()),
                        Some(t.predicate.to_string()),
                        Some(render(&t.object)),
                    ]
                })
                .collect();
            rows.sort();
            rows.dedup();
            Some((vec!["s".into(), "p".into(), "o".into()], rows, None))
        }
    }
}

/// The same variables, the same multiset of rows (or the same boolean).
fn assert_same(q: &str, ordered: bool) {
    let (store, c) = stores();
    assert!(accepts_text(q), "the evaluator should accept: {q}");
    let (ev, mut er, eb) = engine(&store, q);
    let (cv, mut cr, cb) = columnar(&c, q).unwrap_or_else(|| panic!("declined: {q}"));
    assert_eq!(ev, cv, "variables differ for {q}");
    assert_eq!(eb, cb, "boolean differs for {q}");
    if !ordered {
        er.sort();
        cr.sort();
    }
    assert_eq!(er, cr, "rows differ for {q}");
}

fn assert_declined(q: &str) {
    let (_, c) = stores();
    assert!(!accepts_text(q), "should decline: {q}");
    assert!(
        c.query(q).unwrap().is_none(),
        "should decline at evaluation: {q}"
    );
}

const P: &str = "PREFIX ex: <http://example.org/> PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> ";

#[test]
fn basic_graph_patterns_and_joins() {
    for q in [
        "SELECT ?s ?name WHERE { ?s ex:name ?name }",
        "SELECT ?s ?p ?o WHERE { ?s ?p ?o }",
        "SELECT ?name ?age WHERE { ?s ex:name ?name ; ex:age ?age }",
        "SELECT ?name ?age ?t WHERE { ?s ex:name ?name ; ex:age ?age ; ex:type ?t }",
        "SELECT ?a ?b WHERE { ?a ex:knows ?b . ?b ex:knows ?c }",
        "SELECT ?a WHERE { ?a ex:knows ?a }",
        "SELECT ?s WHERE { ?s ex:type ex:T1 }",
        "SELECT ?s WHERE { ?s ex:age 31 }",
        "SELECT ?s WHERE { ?s ex:age \"31\"^^xsd:integer }",
        "SELECT ?s WHERE { ?s ex:missing ?o }",
        "SELECT ?s WHERE { ?s ex:label \"Alice\"@en }",
        "SELECT ?x WHERE { ?x ex:name ?n . ?x ex:knows ?y . ?y ex:name ?m }",
        "SELECT * WHERE { ?s ex:name ?name . ?s ex:type ?t }",
        "SELECT ?n WHERE { [] ex:name ?n }",
    ] {
        assert_same(&format!("{P}{q}"), false);
    }
}

#[test]
fn optional_union_minus_values_bind_filter() {
    for q in [
        "SELECT ?name ?nick WHERE { ?s ex:name ?name OPTIONAL { ?s ex:nick ?nick } }",
        "SELECT ?name ?k WHERE { ?s ex:name ?name OPTIONAL { ?s ex:knows ?k . ?k ex:age ?ka FILTER(?ka > 40) } }",
        "SELECT ?name WHERE { { ?s ex:name ?name ; ex:type ex:T1 } UNION { ?s ex:name ?name ; ex:type ex:T3 } }",
        "SELECT ?s WHERE { ?s ex:name ?n MINUS { ?s ex:knows ?k } }",
        "SELECT ?s ?n WHERE { VALUES ?s { ex:p1 ex:p3 ex:none } ?s ex:name ?n }",
        "SELECT ?s ?n ?x WHERE { ?s ex:name ?n BIND(CONCAT(?n, \"!\") AS ?x) }",
        "SELECT ?s ?a2 WHERE { ?s ex:age ?a BIND(?a * 2 AS ?a2) }",
        "SELECT ?s WHERE { ?s ex:age ?a FILTER(?a > 30) }",
        "SELECT ?s WHERE { ?s ex:age ?a FILTER(?a >= 31 && ?a < 45) }",
        "SELECT ?s WHERE { ?s ex:score ?sc FILTER(?sc > 5) }",
        "SELECT ?s WHERE { ?s ex:name ?n FILTER(STRSTARTS(?n, \"C\") || CONTAINS(?n, \"o\")) }",
        "SELECT ?s WHERE { ?s ex:type ?t FILTER(?t IN (ex:T1, ex:T3)) }",
        "SELECT ?s WHERE { ?s ex:type ?t FILTER(?t NOT IN (ex:T1)) }",
        "SELECT ?s WHERE { ?s ex:name ?n OPTIONAL { ?s ex:nick ?k } FILTER(!BOUND(?k)) }",
        "SELECT ?s WHERE { ?s ex:label ?l FILTER(LANG(?l) = \"es\") }",
        "SELECT ?s WHERE { ?s ex:label ?l FILTER(LANGMATCHES(LANG(?l), \"en\")) }",
        "SELECT ?s WHERE { ?s ex:score ?sc FILTER(isNumeric(?sc)) }",
        "SELECT ?s WHERE { ?s ex:born ?b FILTER(?b < \"1990-01-01T00:00:00Z\"^^xsd:dateTime) }",
        "SELECT ?s WHERE { ?s ex:age ?a FILTER(?a = 27) }",
        "SELECT ?s WHERE { ?s ex:flag ?f FILTER(?f) }",
        "SELECT ?s (IF(?a > 30, \"old\", \"young\") AS ?c) WHERE { ?s ex:age ?a }",
        "SELECT ?s (COALESCE(?k, \"none\") AS ?c) WHERE { ?s ex:name ?n OPTIONAL { ?s ex:nick ?k } }",
        "SELECT ?s (STRLEN(?n) AS ?l) (UCASE(?n) AS ?u) WHERE { ?s ex:name ?n }",
        "SELECT ?s (STRBEFORE(?e, \"@\") AS ?u) (STRAFTER(?e, \"@\") AS ?d) WHERE { ?s ex:email ?e }",
        "SELECT ?s (DATATYPE(?a) AS ?dt) WHERE { ?s ex:age ?a }",
        "SELECT ?s WHERE { ?s ex:age ?a FILTER(sameTerm(?a, 31)) }",
        "SELECT ?s WHERE { ?s ex:name ?n FILTER(isIRI(?s) && isLiteral(?n)) }",
        "SELECT ?s (ABS(-?a) AS ?x) (ROUND(?sc) AS ?r) WHERE { ?s ex:age ?a ; ex:score ?sc FILTER(isNumeric(?sc)) }",
        "SELECT ?s (?a + 1 AS ?b) (?a - 1 AS ?c) (?a / 2 AS ?d) WHERE { ?s ex:age ?a }",
        "SELECT ?s WHERE { ?s ex:name ?n FILTER(STR(?s) = \"http://example.org/p1\") }",
        "SELECT ?s (ENCODE_FOR_URI(?e) AS ?enc) WHERE { ?s ex:email ?e }",
    ] {
        assert_same(&format!("{P}{q}"), false);
    }
}

#[test]
fn aggregates_order_slice_distinct_subqueries() {
    for (q, ordered) in [
        ("SELECT (COUNT(*) AS ?c) WHERE { ?s ex:name ?n }", false),
        ("SELECT (COUNT(?s) AS ?c) WHERE { ?s ex:missing ?n }", false),
        ("SELECT ?t (COUNT(?s) AS ?c) WHERE { ?s ex:type ?t } GROUP BY ?t", false),
        ("SELECT ?t (COUNT(?s) AS ?c) (AVG(?a) AS ?avg) WHERE { ?s ex:type ?t ; ex:age ?a } GROUP BY ?t", false),
        ("SELECT ?t (SUM(?a) AS ?sum) (MIN(?a) AS ?min) (MAX(?a) AS ?max) WHERE { ?s ex:type ?t ; ex:age ?a } GROUP BY ?t", false),
        ("SELECT (COUNT(DISTINCT ?t) AS ?c) WHERE { ?s ex:type ?t }", false),
        ("SELECT ?t (GROUP_CONCAT(?n; separator=\",\") AS ?names) WHERE { ?s ex:type ?t ; ex:name ?n } GROUP BY ?t ORDER BY ?t", true),
        ("SELECT ?t (SAMPLE(?n) AS ?one) WHERE { ?s ex:type ?t ; ex:name ?n } GROUP BY ?t", false),
        ("SELECT ?t (COUNT(?s) AS ?c) WHERE { ?s ex:type ?t } GROUP BY ?t HAVING (COUNT(?s) > 1)", false),
        ("SELECT (AVG(?sc) AS ?avg) WHERE { ?s ex:score ?sc FILTER(isNumeric(?sc)) }", false),
        ("SELECT ?name WHERE { ?s ex:name ?name } ORDER BY ?name", true),
        ("SELECT ?name ?age WHERE { ?s ex:name ?name ; ex:age ?age } ORDER BY DESC(?age) ?name", true),
        ("SELECT ?name WHERE { ?s ex:name ?name } ORDER BY ?name LIMIT 2 OFFSET 1", true),
        ("SELECT DISTINCT ?t WHERE { ?s ex:type ?t }", false),
        ("SELECT REDUCED ?t WHERE { ?s ex:type ?t }", false),
        ("SELECT ?s ?n WHERE { { SELECT ?s WHERE { ?s ex:age ?a FILTER(?a > 30) } } ?s ex:name ?n }", false),
        ("SELECT ?t ?c WHERE { { SELECT ?t (COUNT(?s) AS ?c) WHERE { ?s ex:type ?t } GROUP BY ?t } FILTER(?c > 1) }", false),
        ("SELECT ?name WHERE { ?s ex:name ?name } LIMIT 3", false),
    ] {
        let (store, c) = stores();
        let q = format!("{P}{q}");
        let (ev, mut er, _) = engine(&store, &q);
        let (cv, mut cr, _) = columnar(&c, &q).unwrap_or_else(|| panic!("declined: {q}"));
        assert_eq!(ev, cv, "{q}");
        if q.contains("LIMIT 3") {
            // Unordered LIMIT: any three of the rows.
            assert_eq!(cr.len(), 3, "{q}");
            continue;
        }
        if q.contains("SAMPLE") {
            // SAMPLE is any value of the group: compare the group keys only.
            for r in er.iter_mut().chain(cr.iter_mut()) {
                r.truncate(1);
            }
        }
        if q.contains("GROUP_CONCAT") {
            // The concatenation order follows the (unspecified) solution order:
            // compare the parts as multisets per group.
            let split = |rows: &mut Vec<Vec<Option<String>>>| {
                for r in rows.iter_mut() {
                    if let Some(Some(v)) = r.get_mut(1) {
                        let mut parts: Vec<&str> = v.trim_matches('"').split(',').collect();
                        parts.sort();
                        *v = parts.join(",");
                    }
                }
            };
            split(&mut er);
            split(&mut cr);
        }
        if !ordered {
            er.sort();
            cr.sort();
        }
        assert_eq!(er, cr, "{q}");
    }
}

#[test]
fn ask_construct_graphs_and_paths() {
    for q in [
        "ASK { ?s ex:name \"Alice\" }",
        "ASK { ?s ex:name \"Nobody\" }",
        "CONSTRUCT { ?s ex:label ?n } WHERE { ?s ex:name ?n }",
        "SELECT ?s ?c WHERE { GRAPH ex:g1 { ?s ex:city ?c } }",
        "SELECT ?g ?s ?c WHERE { GRAPH ?g { ?s ex:city ?c } }",
        "SELECT ?s ?c WHERE { GRAPH ?g { ?s ex:city ?c } ?s ex:name ?n }",
        "SELECT ?s ?c FROM ex:g1 FROM ex:g2 WHERE { ?s ex:city ?c }",
        "SELECT ?s ?c FROM ex:g1 WHERE { ?s ex:city ?c }",
        "SELECT ?s ?c FROM NAMED ex:g1 WHERE { GRAPH ?g { ?s ex:city ?c } }",
        "SELECT ?a ?c WHERE { ?a ex:knows/ex:knows ?c }",
        "SELECT ?a ?b WHERE { ?a ^ex:knows ?b }",
        "SELECT ?a ?n WHERE { ?a ex:knows/ex:name ?n }",
    ] {
        assert_same(&format!("{P}{q}"), false);
    }
}

#[test]
fn what_is_not_implemented_is_declined() {
    for q in [
        "SELECT ?a ?b WHERE { ?a ex:knows+ ?b }",
        "SELECT ?a ?b WHERE { ?a ex:knows* ?b }",
        "SELECT ?s WHERE { ?s ex:name ?n FILTER EXISTS { ?s ex:nick ?k } }",
        "SELECT ?s WHERE { ?s ex:name ?n FILTER NOT EXISTS { ?s ex:nick ?k } }",
        "SELECT ?s (NOW() AS ?t) WHERE { ?s ex:name ?n }",
        "SELECT ?s (MD5(?n) AS ?h) WHERE { ?s ex:name ?n }",
        "SELECT ?s (xsd:integer(?a) AS ?i) WHERE { ?s ex:age ?a }",
        "DESCRIBE ex:p1",
        "SELECT ?s WHERE { ?s !ex:name ?o }",
        // An alternative, and a sequence with an alternative on one side: the
        // parser emits these as their own algebra node joined to its sibling
        // through a fresh blank node, which this evaluator does not carry
        // across the join (see columnar_corners.rs).
        "SELECT ?s ?l WHERE { ?s ex:name|ex:label ?l }",
        "SELECT ?s ?o WHERE { ?s ex:knows/(ex:name|ex:nick) ?o }",
        // The date/time accessors, whose type errors differ.
        "SELECT ?s (YEAR(?b) AS ?y) WHERE { ?s ex:born ?b }",
        "SELECT ?s (TZ(?b) AS ?z) WHERE { ?s ex:born ?b }",
        // SUBSTR, STRLANG and STRDT: argument validation differs.
        "SELECT (SUBSTR(?n, 2) AS ?x) WHERE { ?s ex:name ?n }",
        "SELECT (STRLANG(?n, \"de\") AS ?x) WHERE { ?s ex:name ?n }",
        "SELECT (STRDT(?n, xsd:integer) AS ?x) WHERE { ?s ex:name ?n }",
        // Regular expressions: string matching the engine does better.
        "SELECT ?s WHERE { ?s ex:name ?n FILTER(REGEX(?n, \"^a\", \"i\")) }",
        "SELECT (REPLACE(?n, \"a\", \"b\") AS ?x) WHERE { ?s ex:name ?n }",
        // GRAPH ?g over a body that need not bind a triple, and ?g reused.
        "SELECT ?g WHERE { GRAPH ?g { } }",
        "SELECT ?g WHERE { GRAPH ?g { FILTER(true) } }",
        "SELECT ?g ?c WHERE { GRAPH ?g { ?g ex:city ?c } }",
        // This module's own reserved variable namespace.
        "SELECT ?__og_bnode_a WHERE { ?__og_bnode_a ex:name ?n }",
    ] {
        assert_declined(&format!("{P}{q}"));
    }
}

#[test]
fn the_copy_counts_its_quads_and_graphs() {
    let (store, c) = stores();
    let n = store.iter().count();
    assert_eq!(c.len(), n);
    let graphs: BTreeMap<u32, usize> = c
        .graphs()
        .iter()
        .map(|g| (*g, c.count(opengraph::columnar::index::Perm::Gspo, &[*g])))
        .collect();
    assert_eq!(graphs.values().sum::<usize>(), n);
    assert_eq!(c.named_graphs().count(), 2);
}

/// The row budget is an *early exit*, not a different answer.
///
/// The evaluator stops producing rows once a `LIMIT` can no longer use them.
/// That is only sound if the rows it stopped at are the very rows an
/// unbounded evaluation would have put first — so for every query shape and
/// every `n`, `LIMIT n` must equal the first `n` rows of the same query with
/// no limit, and `OFFSET k LIMIT n` the `n` rows after the first `k`. This is
/// what lets the budget reach down through `Slice`, `Project`, `Extend`,
/// `Graph` and `Union` into the pattern scan itself.
#[test]
fn a_limit_is_the_prefix_of_the_unlimited_answer() {
    let (_, c) = stores();
    // One shape per way the budget travels: a single pattern (the scan stops),
    // several patterns (only the last may stop), and every operator that
    // forwards, splits, widens or refuses to pass the budget on.
    for body in [
        "SELECT ?s ?name WHERE { ?s ex:name ?name }",
        "SELECT ?name ?age WHERE { ?s ex:name ?name ; ex:age ?age }",
        "SELECT ?s ?p ?o WHERE { ?s ?p ?o }",
        "SELECT ?name WHERE { ?s ex:name ?name ; ex:age ?age FILTER(?age > 28) }",
        "SELECT ?x WHERE { { ?x ex:name ?n } UNION { ?x ex:type ?t } }",
        "SELECT ?s ?nick WHERE { ?s ex:name ?name OPTIONAL { ?s ex:nick ?nick } }",
        "SELECT ?s WHERE { ?s ex:name ?name MINUS { ?s ex:nick ?x } }",
        "SELECT DISTINCT ?age WHERE { ?s ex:age ?age }",
        "SELECT ?g ?city WHERE { GRAPH ?g { ?s ex:city ?city } }",
        "SELECT ?name ?u WHERE { ?s ex:name ?name BIND(UCASE(?name) AS ?u) }",
        "SELECT ?a ?c WHERE { ?a ex:knows/ex:knows ?c }",
        "SELECT ?name WHERE { ?s ex:name ?name } ORDER BY ?name",
        "SELECT ?t (COUNT(?s) AS ?n) WHERE { ?s ex:type ?t } GROUP BY ?t",
        "SELECT ?v WHERE { VALUES ?v { 1 2 3 4 } }",
    ] {
        let q = format!("{P}{body}");
        let (vars, full, _) = columnar(&c, &q).unwrap_or_else(|| panic!("declined: {q}"));
        assert!(!full.is_empty(), "fixture produces no rows for {body}");

        // A `LIMIT` over more than one triple pattern is declined for speed —
        // the engine stops early through the whole chain and this evaluator
        // only through the last pattern (see `limited_over_several_patterns`).
        // Where that applies the property is unobservable from here, and the
        // decline itself is pinned by `a_limited_join_is_left_to_the_engine`.
        if columnar(&c, &format!("{q} LIMIT 1")).is_none() {
            continue;
        }

        for n in 0..=full.len() + 1 {
            let limited = format!("{q} LIMIT {n}");
            let (lv, lr, _) =
                columnar(&c, &limited).unwrap_or_else(|| panic!("declined: {limited}"));
            assert_eq!(lv, vars, "LIMIT {n} changed the header: {body}");
            let want = &full[..n.min(full.len())];
            assert_eq!(lr, want, "LIMIT {n} is not the first {n} rows of: {body}");
        }

        // The same with an offset: the budget has to be widened by it.
        for k in 0..=2usize.min(full.len()) {
            for n in 0..=2 {
                let sliced = format!("{q} OFFSET {k} LIMIT {n}");
                let (_, sr, _) =
                    columnar(&c, &sliced).unwrap_or_else(|| panic!("declined: {sliced}"));
                let start = k.min(full.len());
                let want = &full[start..(start + n).min(full.len())];
                assert_eq!(sr, want, "OFFSET {k} LIMIT {n} is wrong for: {body}");
            }
        }
    }
}

/// `LIMIT 0` produces the header and no rows, and an `OFFSET` past the end
/// produces nothing — the budget reaching zero must not be read as "no bound".
#[test]
fn a_zero_budget_is_a_bound_not_an_absence() {
    let (_, c) = stores();
    for body in [
        "SELECT ?s ?name WHERE { ?s ex:name ?name }",
        "SELECT ?t (COUNT(?s) AS ?n) WHERE { ?s ex:type ?t } GROUP BY ?t",
    ] {
        let (vars, full, _) = columnar(&c, &format!("{P}{body}")).unwrap();
        let (zv, zr, _) = columnar(&c, &format!("{P}{body} LIMIT 0")).unwrap();
        assert_eq!(zv, vars, "LIMIT 0 changed the header: {body}");
        assert!(zr.is_empty(), "LIMIT 0 returned rows: {body}");
        let (_, past, _) =
            columnar(&c, &format!("{P}{body} OFFSET {} LIMIT 5", full.len() + 3)).unwrap();
        assert!(past.is_empty(), "OFFSET past the end returned rows: {body}");
    }
}

/// A `LIMIT` over several triple patterns goes to the engine; over one it stays
/// here. This is a *speed* decision, measured on the perf gate's
/// `concurrent/reads` (a two-pattern join with `LIMIT 100`: 377 us for the
/// engine, 605 us here), and the only one of the copy's declines that depends
/// on a limit being present at all — so it is worth stating both halves.
#[test]
fn a_limited_join_is_left_to_the_engine() {
    let (_, c) = stores();
    let declined = |q: &str| columnar(&c, &format!("{P}{q}")).is_none();

    // One pattern: the budget stops the scan, so the copy keeps it.
    assert!(!declined("SELECT ?n WHERE { ?s ex:name ?n } LIMIT 5"));
    assert!(!declined(
        "SELECT ?n WHERE { ?s ex:name ?n } OFFSET 2 LIMIT 5"
    ));
    // …and without a limit a join is the copy's best shape, so it keeps that too.
    assert!(!declined(
        "SELECT ?n ?a WHERE { ?s ex:name ?n ; ex:age ?a }"
    ));

    // Several patterns under a limit: the engine stops early throughout.
    assert!(declined(
        "SELECT ?n ?a WHERE { ?s ex:name ?n ; ex:age ?a } LIMIT 5"
    ));
    assert!(declined(
        "SELECT ?n ?k WHERE { ?s ex:name ?n OPTIONAL { ?s ex:knows ?k } } LIMIT 5"
    ));
    assert!(declined(
        "SELECT ?x WHERE { { ?x ex:name ?n } UNION { ?x ex:type ?t } } LIMIT 5"
    ));

    // An ORDER BY under the limit has to see every row anyway, so nothing can
    // stop early on either side and the copy keeps the shape.
    assert!(!declined(
        "SELECT ?n ?a WHERE { ?s ex:name ?n ; ex:age ?a } ORDER BY ?n LIMIT 5"
    ));
}
