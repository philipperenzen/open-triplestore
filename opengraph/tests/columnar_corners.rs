//! The corners: every query shape an adversarial review of the columnar
//! evaluator named as a place it could answer differently from the engine.
//!
//! The rule under test is the module's contract — **decline rather than
//! differ**. For each query there are exactly two acceptable outcomes:
//!
//! 1. the evaluator declines it (`accepts_text` is false, or `query` returns
//!    `Ok(None)`), and the engine answers it; or
//! 2. the evaluator answers it, and its solutions equal the engine's.
//!
//! Answering differently is the failure. The test collects every mismatch
//! before it fails, so one run reports the whole surface rather than the
//! first corner it trips over.

use std::collections::BTreeMap;

use opengraph::columnar::{accepts_text, Columnar};
use opengraph::parallel::ParAnswer;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;
use oxrdf::Term;

/// A fixture built for the corners: mixed and incomparable datatypes, several
/// language tags, blank nodes, the date/time family, values that only differ
/// by datatype, and two named graphs that share a quad.
const DATA: &str = r#"
@prefix ex: <http://example.org/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:a ex:p ex:b ; ex:q ex:b ; ex:name "A" ; ex:r ex:d .
ex:b ex:p ex:c ; ex:r ex:d ; ex:name "B" .
ex:c ex:r ex:e ; ex:name "C" .
ex:d ex:name "D" .
ex:z ex:p ex:y .
ex:y ex:r ex:w .

# One column, many datatypes: integer, decimal, double, plain string, typed
# string, boolean, and two language tags on the same lexical form.
ex:v1 ex:val 31 .
ex:v2 ex:val 7.5 .
ex:v3 ex:val 8.0e0 .
ex:v4 ex:val "31" .
ex:v5 ex:val "31"^^xsd:string .
ex:v6 ex:val true .
ex:v7 ex:val "text"@en .
ex:v8 ex:val "text"@fr .
ex:v9 ex:val "text" .
ex:v10 ex:val ex:anIri .
ex:v11 ex:val "not-a-number"^^xsd:integer .
ex:v12 ex:val ""^^xsd:boolean .

# The date/time family beyond xsd:dateTime.
ex:d1 ex:when "2020-04-02"^^xsd:date .
ex:d2 ex:when "10:30:00"^^xsd:time .
ex:d3 ex:when "2020-04-02T10:30:00Z"^^xsd:dateTime .
ex:d4 ex:when "2020-04-02T10:30:00Z"^^xsd:dateTimeStamp .
ex:d5 ex:when "2020"^^xsd:gYear .
ex:d6 ex:when "P1Y2M"^^xsd:yearMonthDuration .

# Blank nodes, for STR(), CONSTRUCT and DISTINCT.
_:n1 ex:name "Anon" ; ex:tag "t" .
_:n2 ex:name "Anon" ; ex:tag "t" .

ex:s1 ex:text "Hello World" ; ex:num 3 .
ex:s2 ex:text "a.b" ; ex:num 1 .
"#;

const NAMED: &str = r#"
<http://example.org/p1> <http://example.org/city> "Paris" <http://example.org/g1> .
<http://example.org/p2> <http://example.org/city> "Rome" <http://example.org/g1> .
<http://example.org/p3> <http://example.org/city> "Oslo" <http://example.org/g2> .
<http://example.org/p1> <http://example.org/city> "Paris" <http://example.org/g2> .
<http://example.org/g1> <http://example.org/self> "g1 named as a subject" <http://example.org/g1> .
"#;

const P: &str = "PREFIX ex: <http://example.org/> PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> ";

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

/// Blank nodes render as a placeholder: their labels differ between stores.
fn render(t: &Term) -> String {
    match t {
        Term::BlankNode(_) => "_:".to_string(),
        other => other.to_string(),
    }
}

type Answer = (Vec<String>, Vec<Vec<Option<String>>>, Option<bool>);

fn engine(store: &Store, q: &str) -> Result<Answer, String> {
    let r = SparqlEvaluator::new()
        .parse_query(q)
        .map_err(|e| e.to_string())?
        .on_store(store)
        .execute()
        .map_err(|e| e.to_string())?;
    match r {
        QueryResults::Solutions(s) => {
            let vars: Vec<String> = s
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            let mut rows = Vec::new();
            for sol in s {
                let sol = sol.map_err(|e| e.to_string())?;
                rows.push(
                    vars.iter()
                        .map(|v| sol.get(v.as_str()).map(render))
                        .collect::<Vec<_>>(),
                );
            }
            Ok((vars, rows, None))
        }
        QueryResults::Boolean(b) => Ok((vec![], vec![], Some(b))),
        QueryResults::Graph(g) => {
            let mut rows: Vec<Vec<Option<String>>> = Vec::new();
            for t in g {
                let t = t.map_err(|e| e.to_string())?;
                rows.push(vec![
                    Some(t.subject.to_string()),
                    Some(t.predicate.to_string()),
                    Some(render(&t.object)),
                ]);
            }
            rows.sort();
            Ok((vec!["s".into(), "p".into(), "o".into()], rows, None))
        }
    }
}

fn columnar(c: &Columnar, q: &str) -> Option<Answer> {
    // `query_semantics`, not `query`: a shape the copy declines for *speed*
    // must still be correct when it is asked, and this suite is about
    // correctness. The routing policy is pinned in columnar_parity.rs.
    match c.query_semantics(q).ok()?? {
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
            Some((vec!["s".into(), "p".into(), "o".into()], rows, None))
        }
    }
}

/// `None` when the corner is safe (declined, or answered exactly like the
/// engine); `Some(report)` when the evaluator answered differently.
fn mismatch(store: &Store, c: &Columnar, q: &str, ordered: bool) -> Option<String> {
    let full = format!("{P}{q}");
    let accepted = accepts_text(&full);
    let got = columnar(c, &full);
    if !accepted && got.is_some() {
        return Some(format!(
            "{q}\n    accepts_text said no but query() answered"
        ));
    }
    let Some((cv, mut cr, cb)) = got else {
        return None; // Declined: the engine answers it. Always safe.
    };
    let (ev, mut er, eb) = match engine(store, &full) {
        Ok(a) => a,
        // The engine itself rejects the query: the evaluator must not have
        // answered it either.
        Err(e) => {
            return Some(format!(
                "{q}\n    engine errored ({e}) but the evaluator answered {} rows",
                cr.len()
            ))
        }
    };
    if !ordered {
        er.sort();
        cr.sort();
    }
    if ev != cv || eb != cb || er != cr {
        let show = |rows: &Vec<Vec<Option<String>>>| {
            let mut s: Vec<String> = rows
                .iter()
                .take(6)
                .map(|r| {
                    r.iter()
                        .map(|c| c.clone().unwrap_or_else(|| "UNBOUND".into()))
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .collect();
            if rows.len() > 6 {
                s.push(format!("... {} more", rows.len() - 6));
            }
            s.join("\n        ")
        };
        return Some(format!(
            "{q}\n    vars  engine={ev:?} columnar={cv:?}\n    bool  engine={eb:?} columnar={cb:?}\n\
             \x20   rows  engine={} columnar={}\n      engine:\n        {}\n      columnar:\n        {}",
            er.len(),
            cr.len(),
            show(&er),
            show(&cr),
        ));
    }
    None
}

/// Every corner, in one run. Grouped so a failure names the area.
fn check(cases: &[(&str, &str, bool)]) {
    let (store, c) = stores();
    let mut bad: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (area, q, ordered) in cases {
        if let Some(report) = mismatch(&store, &c, q, *ordered) {
            bad.entry(area).or_default().push(report);
        }
    }
    if !bad.is_empty() {
        let n: usize = bad.values().map(|v| v.len()).sum();
        let mut out = format!(
            "\n{n} of {} corners answered differently from the engine:\n",
            cases.len()
        );
        for (area, reports) in &bad {
            out.push_str(&format!("\n=== {area} ({}) ===\n", reports.len()));
            for r in reports {
                out.push_str(&format!("  - {r}\n"));
            }
        }
        panic!("{out}");
    }
}

#[test]
fn property_paths() {
    check(&[
        // A sequence whose side is an alternative: spargebra splits this into
        // sibling algebra nodes joined through a fresh blank node.
        (
            "path",
            "SELECT * WHERE { ?s ex:p/(ex:r|ex:name) ?o }",
            false,
        ),
        ("path", "SELECT * WHERE { ?s (ex:p|ex:q)/ex:r ?o }", false),
        (
            "path",
            "SELECT * WHERE { ?s (ex:p|ex:q)/(ex:r|ex:name) ?o }",
            false,
        ),
        (
            "path",
            "SELECT (COUNT(*) AS ?n) WHERE { ?s (ex:p|ex:q)/ex:r ?o }",
            false,
        ),
        // Alternatives that both match the same pair.
        ("path", "SELECT * WHERE { ex:a (ex:p|ex:q) ?o }", false),
        (
            "path",
            "SELECT (COUNT(*) AS ?n) WHERE { ex:a (ex:p|ex:q) ?o }",
            false,
        ),
        // Simple sequences, which fold into one BGP.
        ("path", "SELECT * WHERE { ?a ex:p/ex:r ?c }", false),
        ("path", "SELECT * WHERE { ?a ^ex:p ?c }", false),
        ("path", "SELECT * WHERE { ?s ex:name|ex:tag ?l }", false),
    ]);
}

#[test]
fn named_graph_scoping() {
    check(&[
        // The graph variable reused inside the pattern.
        (
            "graph-var",
            "SELECT * WHERE { GRAPH ?g { ?g ex:self ?t } }",
            false,
        ),
        (
            "graph-var",
            "SELECT * WHERE { GRAPH ?g { ?s ex:city ?g } }",
            false,
        ),
        // A group under GRAPH ?g with no triple pattern of its own.
        ("graph-var", "SELECT ?g WHERE { GRAPH ?g { } }", false),
        (
            "graph-var",
            "SELECT ?g WHERE { GRAPH ?g { FILTER(true) } }",
            false,
        ),
        // The ordinary cases, which must keep working.
        (
            "graph-var",
            "SELECT ?g ?city WHERE { GRAPH ?g { ?s ex:city ?city } }",
            false,
        ),
        (
            "graph-const",
            "SELECT ?city WHERE { GRAPH ex:g1 { ?s ex:city ?city } }",
            false,
        ),
        (
            "graph-const",
            "SELECT ?city WHERE { GRAPH ex:nosuch { ?s ex:city ?city } }",
            false,
        ),
        ("graph-const", "SELECT * WHERE { GRAPH ex:g1 { } }", false),
        // The default graph must not see the named graphs.
        (
            "default",
            "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }",
            false,
        ),
        ("default", "SELECT ?city WHERE { ?s ex:city ?city }", false),
    ]);
}

#[test]
fn equality_and_comparison() {
    check(&[
        // = and != across datatypes that cannot be compared by value.
        (
            "eq",
            "SELECT ?s WHERE { ?s ex:val ?v FILTER(?v = 31) }",
            false,
        ),
        (
            "eq",
            "SELECT ?s WHERE { ?s ex:val ?v FILTER(?v != 31) }",
            false,
        ),
        (
            "eq",
            "SELECT ?s WHERE { ?s ex:val ?v FILTER(?v = \"31\") }",
            false,
        ),
        (
            "eq",
            "SELECT ?s WHERE { ?s ex:val ?v FILTER(?v != \"31\") }",
            false,
        ),
        (
            "eq",
            "SELECT ?a ?b WHERE { ?a ex:val ?x . ?b ex:val ?y FILTER(?x = ?y) }",
            false,
        ),
        (
            "eq",
            "SELECT ?a ?b WHERE { ?a ex:val ?x . ?b ex:val ?y FILTER(?x != ?y) }",
            false,
        ),
        (
            "eq",
            "SELECT ?s WHERE { ?s ex:val ?v FILTER(?v = \"text\"@en) }",
            false,
        ),
        (
            "eq",
            "SELECT ?s WHERE { ?s ex:val ?v FILTER(sameTerm(?v, \"31\")) }",
            false,
        ),
        // Ordering comparisons, including same-language and identical terms.
        (
            "cmp",
            "SELECT ?a ?b WHERE { ?a ex:val ?x . ?b ex:val ?y FILTER(?x < ?y) }",
            false,
        ),
        (
            "cmp",
            "SELECT ?s WHERE { ?s ex:val ?v FILTER(?v < ?v) }",
            false,
        ),
        (
            "cmp",
            "SELECT ?s WHERE { ?s ex:val ?v FILTER(?v <= ?v) }",
            false,
        ),
        (
            "cmp",
            "SELECT ?s WHERE { ?s ex:val ?v FILTER(?v > true) }",
            false,
        ),
        (
            "cmp",
            "SELECT ?a ?b WHERE { ?a ex:when ?x . ?b ex:when ?y FILTER(?x < ?y) }",
            false,
        ),
    ]);
}

#[test]
fn effective_boolean_value() {
    check(&[
        ("ebv", "SELECT ?s WHERE { ?s ex:val ?v FILTER(?v) }", false),
        ("ebv", "SELECT ?s WHERE { ?s ex:val ?v FILTER(!?v) }", false),
        (
            "ebv",
            "SELECT ?s WHERE { ?s ex:val ?v FILTER(?v || false) }",
            false,
        ),
        (
            "ebv",
            "SELECT ?s ?b WHERE { ?s ex:val ?v BIND(IF(?v, 1, 0) AS ?b) }",
            false,
        ),
        (
            "ebv",
            "SELECT ?s ?b WHERE { ?s ex:val ?v BIND(BOUND(?v) AS ?b) }",
            false,
        ),
        (
            "ebv",
            "SELECT ?s ?b WHERE { ?s ex:val ?v BIND(COALESCE(?v + 1, -1) AS ?b) }",
            false,
        ),
    ]);
}

#[test]
fn ordering() {
    check(&[
        (
            "order",
            "SELECT ?v WHERE { ?s ex:val ?v } ORDER BY ?v",
            true,
        ),
        (
            "order",
            "SELECT ?v WHERE { ?s ex:val ?v } ORDER BY DESC(?v)",
            true,
        ),
        (
            "order",
            "SELECT ?v WHERE { ?s ex:when ?v } ORDER BY ?v",
            true,
        ),
        (
            "order",
            "SELECT ?s ?v WHERE { ?s ex:val ?v } ORDER BY ?v ?s",
            true,
        ),
        (
            "order",
            "SELECT ?v WHERE { ?s ex:val ?v } ORDER BY STR(?v)",
            true,
        ),
        (
            "order",
            "SELECT ?s WHERE { ?s ex:name ?n OPTIONAL { ?s ex:tag ?t } } ORDER BY ?t ?n",
            true,
        ),
    ]);
}

#[test]
fn aggregates() {
    check(&[
        (
            "agg",
            "SELECT (MIN(?v) AS ?m) WHERE { ?s ex:val ?v }",
            false,
        ),
        (
            "agg",
            "SELECT (MAX(?v) AS ?m) WHERE { ?s ex:val ?v }",
            false,
        ),
        (
            "agg",
            "SELECT (COUNT(?v) AS ?n) WHERE { ?s ex:val ?v }",
            false,
        ),
        (
            "agg",
            "SELECT (COUNT(?v + 1) AS ?n) WHERE { ?s ex:val ?v }",
            false,
        ),
        (
            "agg",
            "SELECT (MIN(?v + 1) AS ?n) WHERE { ?s ex:val ?v }",
            false,
        ),
        (
            "agg",
            "SELECT (GROUP_CONCAT(?v) AS ?g) WHERE { ?s ex:val ?v }",
            false,
        ),
        (
            "agg",
            "SELECT (GROUP_CONCAT(?n; SEPARATOR=\"-\") AS ?g) WHERE { ?s ex:name ?n }",
            false,
        ),
        (
            "agg",
            "SELECT (COUNT(*) AS ?n) WHERE { ?s ex:nothing ?v }",
            false,
        ),
        (
            "agg",
            "SELECT (SUM(?v) AS ?n) WHERE { ?s ex:nothing ?v }",
            false,
        ),
        (
            "agg",
            "SELECT (MIN(?v) AS ?n) WHERE { ?s ex:nothing ?v }",
            false,
        ),
        (
            "agg",
            "SELECT (AVG(?v) AS ?n) WHERE { ?s ex:nothing ?v }",
            false,
        ),
        (
            "agg",
            "SELECT ?t (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s ex:tag ?t } GROUP BY ?t",
            false,
        ),
        (
            "agg",
            "SELECT (COUNT(DISTINCT ?v) AS ?n) WHERE { ?s ex:val ?v }",
            false,
        ),
    ]);
}

#[test]
fn string_and_type_functions() {
    check(&[
        (
            "str",
            "SELECT ?s ?x WHERE { ?s ex:name ?n BIND(STR(?s) AS ?x) }",
            false,
        ),
        (
            "substr",
            "SELECT ?x WHERE { ?s ex:text ?t BIND(SUBSTR(?t, 1, 3) AS ?x) }",
            false,
        ),
        (
            "substr",
            "SELECT ?x WHERE { ?s ex:text ?t BIND(SUBSTR(?t, 0) AS ?x) }",
            false,
        ),
        (
            "substr",
            "SELECT ?x WHERE { ?s ex:text ?t BIND(SUBSTR(?t, -2, 5) AS ?x) }",
            false,
        ),
        (
            "substr",
            "SELECT ?x WHERE { ?s ex:text ?t BIND(SUBSTR(?t, 1.5) AS ?x) }",
            false,
        ),
        (
            "substr",
            "SELECT ?x WHERE { ?s ex:text ?t BIND(SUBSTR(?t, 2, 99999999999999) AS ?x) }",
            false,
        ),
        (
            "regex",
            "SELECT ?s WHERE { ?s ex:text ?t FILTER(REGEX(?t, \"a.b\", \"q\")) }",
            false,
        ),
        (
            "regex",
            "SELECT ?s WHERE { ?s ex:text ?t FILTER(REGEX(?t, \"A.B\", \"qi\")) }",
            false,
        ),
        (
            "regex",
            "SELECT ?s WHERE { ?s ex:text ?t FILTER(REGEX(?t, \"hello\", \"i\")) }",
            false,
        ),
        (
            "strlang",
            "SELECT ?x WHERE { ?s ex:val ?v BIND(STRLANG(?v, \"de\") AS ?x) }",
            false,
        ),
        (
            "strlang",
            "SELECT ?x WHERE { ?s ex:val ?v BIND(STRDT(?v, xsd:integer) AS ?x) }",
            false,
        ),
        (
            "case",
            "SELECT ?x WHERE { ?s ex:val ?v BIND(UCASE(?v) AS ?x) }",
            false,
        ),
        (
            "concat",
            "SELECT ?x WHERE { ?s ex:val ?v BIND(CONCAT(?v, ?v) AS ?x) }",
            false,
        ),
        (
            "datetime",
            "SELECT ?x WHERE { ?s ex:when ?w BIND(YEAR(?w) AS ?x) }",
            false,
        ),
        (
            "datetime",
            "SELECT ?x WHERE { ?s ex:when ?w BIND(TZ(?w) AS ?x) }",
            false,
        ),
        (
            "num",
            "SELECT ?x WHERE { ?s ex:val ?v BIND(ABS(?v) AS ?x) }",
            false,
        ),
        (
            "num",
            "SELECT ?x WHERE { ?s ex:val ?v BIND(?v / 2 AS ?x) }",
            false,
        ),
    ]);
}

#[test]
fn construct_and_blank_nodes() {
    check(&[
        (
            "construct",
            "CONSTRUCT { ?s ex:copy ?n } WHERE { ?s ex:name ?n }",
            false,
        ),
        (
            "construct",
            "CONSTRUCT { ?s ex:copy ?t } WHERE { ?s ex:tag ?t }",
            false,
        ),
        (
            "distinct",
            "SELECT DISTINCT ?n WHERE { ?s ex:name ?n }",
            false,
        ),
        (
            "distinct",
            "SELECT DISTINCT ?s WHERE { ?s ex:tag ?t }",
            false,
        ),
        ("bnode", "SELECT ?s WHERE { ?s ex:name \"Anon\" }", false),
        (
            "bnode",
            "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s ex:tag ?t }",
            false,
        ),
    ]);
}
