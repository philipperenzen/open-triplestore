//! Runner for the SHACL Advanced Features tests of TopQuadrant's SHACL API,
//! vendored under `tests/fixtures/shacl-af-topquadrant/` (Apache-2.0; see
//! LICENSE.md and PROVENANCE.md there). Its results are development and
//! regression results on those files, not a conformance claim.
//!
//! The tests use TopQuadrant's `dash:` test vocabulary. Each `*.test.ttl` file
//! is self-contained: data, shapes and the expected outcome live in one graph,
//! which is both the shapes graph and the data graph of the run, merged with
//! any sibling file it `owl:imports` (`rules/triple/person.ttl`). Imports of
//! `dash:` and `schema:` themselves are vocabulary only and are not loaded.
//! The three test kinds and what is compared:
//!
//!   * `dash:GraphValidationTestCase` — `sh:conforms` and the multiset of
//!     focus nodes, at the same level as `tests/w3c_shacl_conformance.rs`;
//!   * `dash:InferencingTestCase` — the set of inferred triples, i.e. what
//!     `infer_into` writes to a separate graph minus what the file asserts,
//!     against the `dash:expectedResult` statements;
//!   * `dash:FunctionTestCase` — the value of the SPARQL expression in
//!     `dash:expression`, evaluated in a run of the file's shapes graph (a
//!     function belongs to the runs of the graph that declares it), against
//!     `dash:expectedResult`. The expression's prefixed names use the file's
//!     Turtle prefixes, as in TopQuadrant's own runner.
//!
//! Gap policy (two-way ratchet): every case NOT in `KNOWN_FAILURES` must pass,
//! and every listed case must still fail.

use open_triplestore::shacl::report::ValidationReport;
use open_triplestore::shacl::{infer_into, validate};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const FIXTURES: &str = "tests/fixtures/shacl-af-topquadrant";
const GRAPH: &str = "urn:af:test";
const INFERRED: &str = "urn:af:inferred";
const DASH: &str = "http://datashapes.org/dash#";

/// Cases that currently fail, with the gap they sit behind. Keep sorted.
/// Removing an entry requires the case to pass (the ratchet asserts both
/// directions). Keys are paths within the fixture directory.
///
/// Empirical baseline: 9 pass / 1 known-fail / 0 aux skips
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("target/sparqlTarget-001.test.ttl", "the target's query uses the owl: prefix, which its sh:prefixes ontology does not declare; TopBraid falls back to the file's Turtle prefixes, which the SHACL prefix mechanism (SHACL §5.2.1) does not include, so the shapes graph fails to load here"),
];

#[derive(Debug, PartialEq)]
enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

fn case_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            case_files(&p, out);
        } else if p.to_string_lossy().ends_with(".test.ttl") {
            out.push(p);
        }
    }
}

fn select(store: &TripleStore, q: &str) -> Vec<oxigraph::sparql::QuerySolution> {
    match store.query(q) {
        Ok(QueryResults::Solutions(s)) => s.flatten().collect(),
        _ => Vec::new(),
    }
}

/// Load `path` and the sibling files it imports into [`GRAPH`].
fn load_case(store: &TripleStore, path: &Path) -> Result<(), String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("unreadable: {e}"))?;
    store
        .load_str(&content, RdfFormat::Turtle, Some(GRAPH))
        .map_err(|e| format!("parse: {e}"))?;
    let imports = select(
        store,
        &format!(
            "SELECT ?i WHERE {{ GRAPH <{GRAPH}> {{ ?o a <http://www.w3.org/2002/07/owl#Ontology> ; \
             <http://www.w3.org/2002/07/owl#imports> ?i }} }}"
        ),
    );
    for sol in imports {
        let Some(Term::NamedNode(i)) = sol.get("i") else {
            continue;
        };
        if matches!(
            i.as_str(),
            "http://datashapes.org/dash" | "http://datashapes.org/schema"
        ) {
            continue;
        }
        let stem = i.as_str().rsplit('/').next().unwrap_or_default();
        let sibling = path.with_file_name(format!("{stem}.ttl"));
        let aux = std::fs::read_to_string(&sibling)
            .map_err(|_| format!("import <{}> not vendored", i.as_str()))?;
        store
            .load_str(&aux, RdfFormat::Turtle, Some(GRAPH))
            .map_err(|e| format!("parse {stem}: {e}"))?;
    }
    Ok(())
}

fn test_case_of(store: &TripleStore, kind: &str) -> bool {
    matches!(
        store.query(&format!(
            "ASK {{ GRAPH <{GRAPH}> {{ ?t a <{DASH}{kind}> }} }}"
        )),
        Ok(QueryResults::Boolean(true))
    )
}

/// Literal and IRI focus nodes by lexical form, blank nodes by count.
fn focus_key(term: &Term) -> String {
    match term {
        Term::NamedNode(nn) => nn.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        _ => "_:".to_string(),
    }
}

fn actual_focus(report: &ValidationReport) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for r in &report.results {
        let key = if r.focus_node.starts_with("_:") {
            "_:".to_string()
        } else {
            r.focus_node.clone()
        };
        *out.entry(key).or_insert(0) += 1;
    }
    out
}

fn run_validation(store: &TripleStore) -> Outcome {
    let expected = select(
        store,
        &format!(
            "SELECT ?c WHERE {{ GRAPH <{GRAPH}> {{ ?t a <{DASH}GraphValidationTestCase> ; \
             <{DASH}expectedResult> ?r . ?r <http://www.w3.org/ns/shacl#conforms> ?c }} }}"
        ),
    );
    let Some(want_conforms) = expected.first().and_then(|s| match s.get("c") {
        Some(Term::Literal(l)) => Some(l.value() == "true"),
        _ => None,
    }) else {
        return Outcome::Skip("no expected sh:conforms".into());
    };
    let mut want_focus: BTreeMap<String, usize> = BTreeMap::new();
    for sol in select(
        store,
        &format!(
            "SELECT ?f WHERE {{ GRAPH <{GRAPH}> {{ ?t a <{DASH}GraphValidationTestCase> ; \
             <{DASH}expectedResult> ?r . ?r <http://www.w3.org/ns/shacl#result> ?res . \
             ?res <http://www.w3.org/ns/shacl#focusNode> ?f }} }}"
        ),
    ) {
        if let Some(f) = sol.get("f") {
            *want_focus.entry(focus_key(f)).or_insert(0) += 1;
        }
    }
    let report = match validate(store, GRAPH, &[GRAPH.to_string()]) {
        Ok(r) => r,
        Err(e) => return Outcome::Fail(format!("validate error: {e}")),
    };
    if report.conforms != want_conforms {
        return Outcome::Fail(format!(
            "conforms: want {want_conforms}, got {} ({:?})",
            report.conforms, report.results
        ));
    }
    let got = actual_focus(&report);
    if got != want_focus {
        return Outcome::Fail(format!("focus nodes: want {want_focus:?}, got {got:?}"));
    }
    Outcome::Pass
}

fn run_inferencing(store: &TripleStore) -> Outcome {
    let rdf = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    let want: BTreeSet<String> = select(
        store,
        &format!(
            "SELECT ?s ?p ?o WHERE {{ GRAPH <{GRAPH}> {{ ?t a <{DASH}InferencingTestCase> ; \
             <{DASH}expectedResult> ?r . ?r <{rdf}subject> ?s ; <{rdf}predicate> ?p ; <{rdf}object> ?o }} }}"
        ),
    )
    .iter()
    .map(|s| format!("{} {} {}", s["s"], s["p"], s["o"]))
    .collect();
    if let Err(e) = infer_into(store, GRAPH, &[GRAPH.to_string()], Some(INFERRED)) {
        return Outcome::Fail(format!("infer error: {e}"));
    }
    let got: BTreeSet<String> = select(
        store,
        &format!(
            "SELECT ?s ?p ?o WHERE {{ GRAPH <{INFERRED}> {{ ?s ?p ?o }} \
             FILTER NOT EXISTS {{ GRAPH <{GRAPH}> {{ ?s ?p ?o }} }} }}"
        ),
    )
    .iter()
    .map(|s| format!("{} {} {}", s["s"], s["p"], s["o"]))
    .collect();
    if got != want {
        return Outcome::Fail(format!("inferred triples: want {want:?}, got {got:?}"));
    }
    Outcome::Pass
}

/// The `@prefix` declarations of a Turtle file, as a SPARQL prologue.
fn turtle_prefixes(content: &str) -> String {
    let mut out = String::new();
    for line in content.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("@prefix") else {
            continue;
        };
        if let Some((name, iri)) = rest.trim().split_once(':') {
            let iri = iri.trim().trim_end_matches('.').trim();
            out.push_str(&format!("PREFIX {}: {iri}\n", name.trim()));
        }
    }
    out
}

fn run_functions(store: &TripleStore, path: &Path) -> Outcome {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let prologue = turtle_prefixes(&content);
    let cases = select(
        store,
        &format!(
            "SELECT ?t ?expr ?want WHERE {{ GRAPH <{GRAPH}> {{ ?t a <{DASH}FunctionTestCase> ; \
             <{DASH}expression> ?expr ; <{DASH}expectedResult> ?want }} }} ORDER BY ?t"
        ),
    );
    if cases.is_empty() {
        return Outcome::Skip("no dash:FunctionTestCase".into());
    }
    // One probe shape per expression, added to the file's own graph so the
    // run sees the functions it declares: its single result carries the
    // expression's value.
    let mut probes = String::from("@prefix sh: <http://www.w3.org/ns/shacl#> .\n");
    let mut expected = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let Some(Term::Literal(expr)) = case.get("expr") else {
            return Outcome::Skip("dash:expression is not a literal".into());
        };
        let query = format!(
            "{prologue}SELECT $this ?value WHERE {{ BIND (({}) AS ?value) }}",
            expr.value()
        );
        probes.push_str(&format!(
            "<urn:af:probe:{i}> a sh:NodeShape ; sh:targetNode <urn:af:focus> ;\n  \
             sh:sparql [ sh:select {} ] .\n",
            Term::from(oxigraph::model::Literal::new_simple_literal(query))
        ));
        expected.push((format!("urn:af:probe:{i}"), case["want"].to_string()));
    }
    if let Err(e) = store.load_str(&probes, RdfFormat::Turtle, Some(GRAPH)) {
        return Outcome::Skip(format!("probe: {e}"));
    }
    let report = match validate(store, GRAPH, &[GRAPH.to_string()]) {
        Ok(r) => r,
        Err(e) => return Outcome::Fail(format!("validate error: {e}")),
    };
    for (probe, want) in expected {
        let got: Vec<_> = report
            .results
            .iter()
            .filter(|r| r.source_shape == probe)
            .map(|r| r.value.clone())
            .collect();
        if got != vec![Some(want.clone())] {
            return Outcome::Fail(format!("{probe}: want {want}, got {got:?}"));
        }
    }
    Outcome::Pass
}

fn run_one(path: &Path) -> Outcome {
    let store = match TripleStore::in_memory() {
        Ok(s) => s,
        Err(e) => return Outcome::Skip(format!("store: {e}")),
    };
    if let Err(e) = load_case(&store, path) {
        return Outcome::Skip(e);
    }
    if test_case_of(&store, "GraphValidationTestCase") {
        run_validation(&store)
    } else if test_case_of(&store, "InferencingTestCase") {
        run_inferencing(&store)
    } else if test_case_of(&store, "FunctionTestCase") {
        run_functions(&store, path)
    } else {
        Outcome::Skip("no dash test case".into())
    }
}

#[test]
fn topquadrant_shacl_af_suite() {
    let root = Path::new(FIXTURES);
    let mut files = Vec::new();
    case_files(root, &mut files);
    files.sort();
    assert!(!files.is_empty(), "vendored corpus present");

    let mut pass = 0usize;
    let mut skip = Vec::new();
    let mut unexpected_failures = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut seen_known = 0usize;
    for path in &files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let known = KNOWN_FAILURES.iter().find(|(k, _)| *k == rel);
        if known.is_some() {
            seen_known += 1;
        }
        match run_one(path) {
            Outcome::Pass => {
                pass += 1;
                if let Some((k, why)) = known {
                    unexpected_passes.push(format!("{k} (listed as: {why})"));
                }
            }
            Outcome::Fail(reason) => {
                if known.is_none() {
                    unexpected_failures.push(format!("{rel}: {reason}"));
                }
            }
            Outcome::Skip(reason) => skip.push(format!("{rel}: {reason}")),
        }
    }
    println!(
        "TopQuadrant SHACL-AF: {pass} passed, {} known-fail, {} skipped, {} cases",
        KNOWN_FAILURES.len(),
        skip.len(),
        files.len()
    );
    assert_eq!(
        seen_known,
        KNOWN_FAILURES.len(),
        "every KNOWN_FAILURES key must name a vendored case (stale entries?)"
    );
    assert!(
        unexpected_failures.is_empty(),
        "cases failing that are not in KNOWN_FAILURES:\n  {}",
        unexpected_failures.join("\n  ")
    );
    assert!(
        unexpected_passes.is_empty(),
        "KNOWN_FAILURES entries now pass — remove them to ratchet forward:\n  {}",
        unexpected_passes.join("\n  ")
    );
    // A floor as well as a ratchet: an unreadable or unparseable file is a
    // silent skip, which must not pass for green.
    assert!(skip.is_empty(), "skipped cases:\n  {}", skip.join("\n  "));
    assert!(pass >= 9, "only {pass} cases passed (floor 9)");
}
