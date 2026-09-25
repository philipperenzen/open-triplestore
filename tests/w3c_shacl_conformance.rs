//! Runner for the `core` and `sparql` sections of the W3C SHACL test suite,
//! vendored under `tests/fixtures/w3c-shacl/{core,sparql}` (see PROVENANCE.md
//! and LICENSE.md there). Its results are development and regression results
//! at the comparison level below, not a W3C conformance claim.
//!
//! Each suite file is self-contained: data + shapes + an `mf:Manifest` entry
//! (`sht:Validate`) + the expected `sh:ValidationReport` — or `sht:Failure`,
//! for the `sparql/pre-binding/unsupported-*` cases whose shapes graph the
//! validator must reject. The runner loads the file as both shapes graph and
//! data graph (the suite is designed for this — `sht:dataGraph <>` /
//! `sht:shapesGraph <>` reference the file itself), runs our validator and
//! compares:
//!
//!   1. `sh:conforms`, and
//!   2. when non-conforming, the **multiset of violation focus nodes**
//!      (IRIs/literals by lexical form; blank nodes matched by count);
//!   3. for `sht:Failure`, that validation returned an error.
//!
//! This is deliberately one notch below full result-set equality (component
//! IRIs / paths / values), because our `ValidationResult` reports the source
//! constraint as a display string rather than a component IRI — tracked as a
//! possible future refinement in docs/conformance/shacl.md.
//!
//! Gap policy (two-way ratchet): every test NOT in `KNOWN_FAILURES` must pass,
//! and every listed test must still fail — so silent regressions *and* silent
//! fixes both turn the suite red, keeping the list honest.

use open_triplestore::shacl::report::ValidationReport;
use open_triplestore::shacl::validate;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The two vendored sections: SHACL Core, and SHACL-SPARQL (sh:sparql
/// constraints, constraint components, pre-binding).
const SUITES: &[&str] = &["core", "sparql"];
const FIXTURES: &str = "tests/fixtures/w3c-shacl";

/// Tests that currently fail, with the engine gap they sit behind. Keep sorted.
/// Removing an entry requires the test to actually pass (the ratchet asserts
/// both directions). Keys are `<suite>/<path within the suite>`.
///
/// Empirical baseline: 119 pass / 2 known-fail / 15 aux skips
const KNOWN_FAILURES: &[(&str, &str)] = &[
    // core: 97 pass / 1 known-fail / 15 aux skips (was 46/52/15 before the
    // typed-term engine refactor — focus and value nodes are now oxigraph
    // Terms end-to-end). See docs/conformance/shacl.md.
    ("core/property/uniqueLang-002.ttl", "oxigraph's storage canonicalises \"1\"^^xsd:boolean to \"true\" (native value encoding), so the spec's literal-\"true\"-only activation of sh:uniqueLang is indistinguishable post-load"),
    // sparql: 22 pass / 1 known-fail / 0 skips.
    ("sparql/pre-binding/shapesGraph-001.ttl", "$shapesGraph / $currentShape are not supported (SHACL §5.3.1 leaves them to processors that expose the shapes graph to constraint queries); a constraint using them fails the shapes graph instead of producing the expected report"),
];

#[derive(Debug, PartialEq)]
enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

fn suite_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            suite_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "ttl")
            && p.file_name().is_some_and(|n| n != "manifest.ttl")
        {
            out.push(p);
        }
    }
}

/// `term` → comparable lexical key. Blank nodes all map to `_:` so they are
/// compared by count, not label (labels are not stable across parsers).
fn focus_key(term: &oxigraph::model::Term) -> String {
    match term {
        oxigraph::model::Term::NamedNode(nn) => nn.as_str().to_string(),
        oxigraph::model::Term::Literal(l) => l.value().to_string(),
        oxigraph::model::Term::BlankNode(_) => "_:".to_string(),
        other => other.to_string(),
    }
}

enum Expected {
    /// `sh:conforms` and the multiset of expected focus nodes.
    Report(bool, BTreeMap<String, usize>),
    /// `mf:result sht:Failure`: the validator must reject the shapes graph.
    Failure,
}

/// The file's embedded `mf:result`. Returns `None` when the file declares no
/// `sht:Validate` entry (e.g. include-only manifests).
fn expected(store: &TripleStore) -> Option<Expected> {
    if let Ok(QueryResults::Boolean(true)) = store.query(
        "PREFIX sht: <http://www.w3.org/ns/shacl-test#> \
         PREFIX mf: <http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#> \
         ASK { GRAPH <urn:t:shapes> { ?t a sht:Validate ; mf:result sht:Failure } }",
    ) {
        return Some(Expected::Failure);
    }
    let conforms = match store.query(
        "PREFIX sht: <http://www.w3.org/ns/shacl-test#> \
         PREFIX mf: <http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#> \
         PREFIX sh: <http://www.w3.org/ns/shacl#> \
         SELECT ?c WHERE { GRAPH <urn:t:shapes> { ?t a sht:Validate ; mf:result ?r . ?r sh:conforms ?c } }",
    ) {
        Ok(QueryResults::Solutions(mut sols)) => match sols.next() {
            Some(Ok(sol)) => matches!(
                sol.get("c"),
                Some(oxigraph::model::Term::Literal(l)) if l.value() == "true"
            ),
            _ => return None,
        },
        _ => return None,
    };

    let mut focus: BTreeMap<String, usize> = BTreeMap::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(
        "PREFIX sht: <http://www.w3.org/ns/shacl-test#> \
         PREFIX mf: <http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#> \
         PREFIX sh: <http://www.w3.org/ns/shacl#> \
         SELECT ?f WHERE { GRAPH <urn:t:shapes> { \
            ?t a sht:Validate ; mf:result ?r . ?r sh:result ?res . ?res sh:focusNode ?f } }",
    ) {
        for sol in sols.flatten() {
            if let Some(f) = sol.get("f") {
                *focus.entry(focus_key(f)).or_insert(0) += 1;
            }
        }
    }
    Some(Expected::Report(conforms, focus))
}

/// Actual focus-node multiset from our report, normalised like `focus_key`.
fn actual_focus(report: &ValidationReport) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
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

fn run_one(suite: &str, root: &Path, path: &Path) -> Outcome {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Outcome::Skip("unreadable".into());
    };
    // The suite's canonical base: relative IRIs (`<>`, `<minLength-001>`)
    // resolve against the test file's location under datashapes.org.
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let base = format!("http://datashapes.org/sh/tests/{suite}/{rel}");

    let store = match TripleStore::in_memory() {
        Ok(s) => s,
        Err(e) => return Outcome::Skip(format!("store: {e}")),
    };
    // The manifest/expected-report triples always come from the main file.
    for graph in ["urn:t:shapes", "urn:t:data"] {
        if let Err(e) = store.load_str_with_base(&content, RdfFormat::Turtle, &base, Some(graph)) {
            return Outcome::Skip(format!("parse: {e}"));
        }
    }

    let Some(expected) = expected(&store) else {
        return Outcome::Skip("no sht:Validate entry".into());
    };

    // Some tests keep data/shapes in sibling files (`sht:dataGraph <…-data>`).
    // Resolve any non-self graph reference to the sibling `.ttl` and load it
    // into the corresponding graph on top of the main file's triples.
    for (pred, graph) in [("dataGraph", "urn:t:data"), ("shapesGraph", "urn:t:shapes")] {
        let q = format!(
            "PREFIX sht: <http://www.w3.org/ns/shacl-test#> \
             PREFIX mf: <http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#> \
             SELECT ?g WHERE {{ GRAPH <urn:t:shapes> {{ ?t a sht:Validate ; mf:action ?a . ?a sht:{pred} ?g }} }}"
        );
        if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
            for sol in sols.flatten() {
                let Some(oxigraph::model::Term::NamedNode(g)) = sol.get("g") else {
                    continue;
                };
                if g.as_str() == base {
                    continue; // self-reference, already loaded
                }
                let Some(stem) = g.as_str().rsplit('/').next() else {
                    continue;
                };
                // The referenced IRI may or may not carry the .ttl extension.
                let file = if stem.ends_with(".ttl") {
                    stem.to_string()
                } else {
                    format!("{stem}.ttl")
                };
                let sibling = path.with_file_name(&file);
                let Ok(aux) = std::fs::read_to_string(&sibling) else {
                    return Outcome::Skip(format!("external graph not found: {stem}"));
                };
                let aux_base = format!(
                    "http://datashapes.org/sh/tests/{suite}/{}",
                    sibling
                        .strip_prefix(root)
                        .unwrap_or(&sibling)
                        .to_string_lossy()
                        .replace('\\', "/")
                );
                if let Err(e) =
                    store.load_str_with_base(&aux, RdfFormat::Turtle, &aux_base, Some(graph))
                {
                    return Outcome::Skip(format!("parse {stem}: {e}"));
                }
            }
        }
    }

    let report = match validate(&store, "urn:t:shapes", &["urn:t:data".to_string()]) {
        Ok(r) => r,
        Err(e) => {
            return match expected {
                Expected::Failure => Outcome::Pass,
                Expected::Report(..) => Outcome::Fail(format!("validate error: {e}")),
            }
        }
    };
    let (want_conforms, want_focus) = match expected {
        Expected::Failure => {
            return Outcome::Fail(format!(
                "expected the validator to reject the shapes graph (sht:Failure), got a report with conforms={}",
                report.conforms
            ))
        }
        Expected::Report(c, f) => (c, f),
    };

    if report.conforms != want_conforms {
        return Outcome::Fail(format!(
            "conforms: want {want_conforms}, got {} ({} results)",
            report.conforms, report.results_count
        ));
    }
    if !want_conforms && !want_focus.is_empty() {
        let got = actual_focus(&report);
        if got != want_focus {
            return Outcome::Fail(format!("focus nodes: want {want_focus:?}, got {got:?}"));
        }
    }
    Outcome::Pass
}

#[test]
fn w3c_shacl_core_suite() {
    let mut total_files = 0usize;
    let mut pass = 0usize;
    let mut skip = Vec::new();
    let mut unexpected_failures = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut seen_known = 0usize;

    for suite in SUITES {
        let root = Path::new(FIXTURES).join(suite);
        let mut files = Vec::new();
        suite_files(&root, &mut files);
        files.sort();
        assert!(
            !files.is_empty(),
            "vendored suite `{suite}` present ({} files found)",
            files.len()
        );
        total_files += files.len();
        let mut suite_pass = 0usize;
        let mut suite_known = 0usize;
        let mut suite_skip = 0usize;

        for path in &files {
            let rel = format!(
                "{suite}/{}",
                path.strip_prefix(&root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/")
            );
            let known = KNOWN_FAILURES.iter().find(|(k, _)| *k == rel);
            if known.is_some() {
                seen_known += 1;
            }
            match run_one(suite, &root, path) {
                Outcome::Pass => {
                    pass += 1;
                    suite_pass += 1;
                    if let Some((k, why)) = known {
                        unexpected_passes.push(format!("{k} (listed as: {why})"));
                    }
                }
                Outcome::Fail(reason) => {
                    if known.is_none() {
                        unexpected_failures.push(format!("{rel}: {reason}"));
                    } else {
                        suite_known += 1;
                    }
                }
                Outcome::Skip(reason) => {
                    suite_skip += 1;
                    skip.push(format!("{rel}: {reason}"));
                }
            }
        }
        println!(
            "W3C SHACL {suite}: {suite_pass} passed, {suite_known} known-fail, {suite_skip} skipped, {} files",
            files.len()
        );
    }

    println!(
        "W3C SHACL total: {pass} passed, {} known-fail, {} skipped, {total_files} files",
        KNOWN_FAILURES.len(),
        skip.len()
    );
    for s in &skip {
        println!("  SKIP {s}");
    }
    assert_eq!(
        seen_known,
        KNOWN_FAILURES.len(),
        "every KNOWN_FAILURES key must name a vendored file (stale entries?)"
    );
    assert!(
        unexpected_failures.is_empty(),
        "tests failing that are not in KNOWN_FAILURES:\n  {}",
        unexpected_failures.join("\n  ")
    );
    assert!(
        unexpected_passes.is_empty(),
        "KNOWN_FAILURES entries now pass — remove them to ratchet forward:\n  {}",
        unexpected_passes.join("\n  ")
    );
    // A floor as well as a ratchet. The runner turns an unreadable or
    // unparseable file into a silent `Skip`, so a Turtle-parser regression
    // would have turned every file into a skip and still passed the two
    // asserts above. 119 pass today (97 core + 22 sparql); 110 leaves headroom
    // for suite churn.
    assert!(
        pass >= 110,
        "only {pass} W3C SHACL cases passed (floor 110); skips: {}",
        skip.len()
    );
    assert!(
        skip.len() <= 20,
        "{} cases were skipped (ceiling 20) — a parse/load regression turns passes into skips:\n  {}",
        skip.len(),
        skip.join("\n  ")
    );
}
