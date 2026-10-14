//! Full report equality for the SHACL corpus runners
//! (`tests/w3c_shacl_conformance.rs`, `tests/shacl_af_corpus.rs`).
//!
//! An expected report and ours are compared as multisets of results, each
//! keyed on everything but `sh:resultMessage`, whose wording the spec leaves
//! to the processor: `sh:focusNode`, `sh:resultPath` (as a path structure),
//! `sh:value`, `sh:sourceShape`, `sh:sourceConstraintComponent`,
//! `sh:resultSeverity`, `sh:sourceConstraint`, and any other property of the
//! result node (a SHACL-AF result annotation). Our side is the RDF report
//! `report_rdf` writes, loaded back, so the serialisation is under test too.
//! Blank nodes of the data graph (focus nodes, values, annotation values) are
//! wildcards; shape and constraint blank nodes must be the very node of the
//! shapes graph.

use open_triplestore::shacl::report::ValidationReport;
use open_triplestore::shacl_studio::report_rdf::report_to_turtle;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use std::collections::{BTreeMap, HashMap};

const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// The result properties with a key field of their own, plus the two that
/// are not compared (`rdf:type`, `sh:resultMessage`).
const KEYED: &[&str] = &[
    "focusNode",
    "resultPath",
    "value",
    "sourceShape",
    "sourceConstraintComponent",
    "resultSeverity",
    "sourceConstraint",
    "resultMessage",
];

/// The triples of one graph, by subject.
type Triples = HashMap<Term, Vec<(String, Term)>>;

fn graph_triples(store: &TripleStore, graph: &str) -> Triples {
    let mut out: Triples = HashMap::new();
    let q = format!("SELECT ?s ?p ?o WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}");
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        for sol in sols.flatten() {
            if let (Some(s), Some(Term::NamedNode(p)), Some(o)) =
                (sol.get("s"), sol.get("p"), sol.get("o"))
            {
                out.entry(s.clone())
                    .or_default()
                    .push((p.as_str().to_string(), o.clone()));
            }
        }
    }
    out
}

fn objects<'a>(triples: &'a Triples, s: &Term, p: &str) -> Vec<&'a Term> {
    triples
        .get(s)
        .map(|po| po.iter().filter(|(q, _)| q == p).map(|(_, o)| o).collect())
        .unwrap_or_default()
}

/// The exact term: shapes-graph nodes must be the same node.
fn exact_key(t: &Term) -> String {
    t.to_string()
}

/// Data-graph nodes: a blank node matches any blank node.
fn wildcard_key(t: &Term) -> String {
    match t {
        Term::BlankNode(_) => "_:".to_string(),
        other => other.to_string(),
    }
}

/// The members of an RDF list.
fn list_items<'a>(triples: &'a Triples, mut head: &'a Term) -> Vec<&'a Term> {
    let mut items = Vec::new();
    while let Some(first) = objects(triples, head, &format!("{RDF}first")).first() {
        items.push(*first);
        match objects(triples, head, &format!("{RDF}rest")).first() {
            Some(rest) => head = rest,
            None => break,
        }
        if items.len() > 64 {
            break;
        }
    }
    items
}

/// A SHACL path structure as a canonical, fully parenthesised string.
fn path_key(triples: &Triples, t: &Term, depth: usize) -> String {
    if depth > 16 {
        return "…".to_string();
    }
    let Term::BlankNode(_) = t else {
        return exact_key(t);
    };
    let sub = |p: &str| -> Option<String> {
        objects(triples, t, &format!("{SH}{p}"))
            .first()
            .map(|o| path_key(triples, o, depth + 1))
    };
    let list = |head: &Term, sep: &str| -> String {
        list_items(triples, head)
            .into_iter()
            .map(|i| path_key(triples, i, depth + 1))
            .collect::<Vec<_>>()
            .join(sep)
    };
    if !objects(triples, t, &format!("{RDF}first")).is_empty() {
        return format!("({})", list(t, "/"));
    }
    if let Some(alt) = objects(triples, t, &format!("{SH}alternativePath")).first() {
        return format!("({})", list(alt, "|"));
    }
    for (p, fmt) in [
        ("inversePath", "^"),
        ("zeroOrMorePath", "*"),
        ("oneOrMorePath", "+"),
        ("zeroOrOnePath", "?"),
    ] {
        if let Some(inner) = sub(p) {
            return if fmt == "^" {
                format!("^({inner})")
            } else {
                format!("({inner}){fmt}")
            };
        }
    }
    "_:?".to_string()
}

/// The multiset of results of the report(s) selected by `report_pattern`
/// (a SPARQL pattern binding `?res`; prefixes `sh:`, `sht:`, `mf:` and
/// `dash:` are declared) in `graph`, each as a comparable key.
pub fn result_keys(
    store: &TripleStore,
    graph: &str,
    report_pattern: &str,
) -> BTreeMap<String, usize> {
    let triples = graph_triples(store, graph);
    let q = format!(
        "PREFIX sht: <http://www.w3.org/ns/shacl-test#> \
         PREFIX mf: <http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#> \
         PREFIX sh: <http://www.w3.org/ns/shacl#> \
         PREFIX dash: <http://datashapes.org/dash#> \
         SELECT DISTINCT ?res WHERE {{ GRAPH <{graph}> {{ {report_pattern} }} }}"
    );
    let mut out = BTreeMap::new();
    let Ok(QueryResults::Solutions(sols)) = store.query(&q) else {
        return out;
    };
    for sol in sols.flatten() {
        let Some(res) = sol.get("res") else { continue };
        let field = |p: &str, key: &dyn Fn(&Term) -> String| -> String {
            let mut v: Vec<String> = objects(&triples, res, &format!("{SH}{p}"))
                .into_iter()
                .map(key)
                .collect();
            v.sort();
            v.join(",")
        };
        let mut other: Vec<String> = triples
            .get(res)
            .into_iter()
            .flatten()
            .filter(|(p, _)| {
                p != &format!("{RDF}type")
                    && !p
                        .strip_prefix(SH)
                        .is_some_and(|local| KEYED.contains(&local))
            })
            .map(|(p, o)| format!("<{p}>={}", wildcard_key(o)))
            .collect();
        other.sort();
        let key = format!(
            "focus={} path={} value={} shape={} component={} severity={} constraint={} other=[{}]",
            field("focusNode", &wildcard_key),
            field("resultPath", &|t| path_key(&triples, t, 0)),
            field("value", &wildcard_key),
            field("sourceShape", &exact_key),
            field("sourceConstraintComponent", &exact_key),
            field("resultSeverity", &exact_key),
            field("sourceConstraint", &exact_key),
            other.join(","),
        );
        *out.entry(key).or_insert(0) += 1;
    }
    out
}

/// Compare `report` against the expected results selected by
/// `expected_pattern` in `expected_graph`: our report is written as RDF,
/// loaded into `report_graph` and keyed the same way. `Err` describes the
/// difference.
pub fn compare_results(
    store: &TripleStore,
    report: &ValidationReport,
    expected_graph: &str,
    expected_pattern: &str,
    report_graph: &str,
) -> Result<(), String> {
    let want = result_keys(store, expected_graph, expected_pattern);
    let ttl = report_to_turtle(report, &format!("{report_graph}#run"));
    store
        .load_str_with_base(&ttl, RdfFormat::Turtle, report_graph, Some(report_graph))
        .map_err(|e| format!("our report RDF does not load: {e}"))?;
    let got = result_keys(
        store,
        report_graph,
        "?r a sh:ValidationReport ; sh:result ?res",
    );
    if got == want {
        return Ok(());
    }
    let diff = |a: &BTreeMap<String, usize>, b: &BTreeMap<String, usize>| -> Vec<String> {
        a.iter()
            .filter_map(|(k, n)| {
                let m = b.get(k).copied().unwrap_or(0);
                (*n > m).then(|| format!("{}x {k}", n - m))
            })
            .collect()
    };
    Err(format!(
        "results differ ({} expected, {} got)\n      missing: {:?}\n      unexpected: {:?}",
        want.values().sum::<usize>(),
        got.values().sum::<usize>(),
        diff(&want, &got),
        diff(&got, &want),
    ))
}
