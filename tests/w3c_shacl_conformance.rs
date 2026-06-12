//! W3C SHACL test-suite runner over the vendored official suite
//! (`tests/fixtures/w3c-shacl/core`, see PROVENANCE.md there).
//!
//! Each suite file is self-contained: data + shapes + an `mf:Manifest` entry
//! (`sht:Validate`) + the expected `sh:ValidationReport`. The runner loads the
//! file as both shapes graph and data graph (the suite is designed for this —
//! `sht:dataGraph <>` / `sht:shapesGraph <>` reference the file itself), runs
//! our validator and compares:
//!
//!   1. `sh:conforms`, and
//!   2. when non-conforming, the **multiset of violation focus nodes**
//!      (IRIs/literals by lexical form; blank nodes matched by count).
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

const SUITE_ROOT: &str = "tests/fixtures/w3c-shacl/core";

/// Tests that currently fail, with the engine gap they sit behind. Keep sorted.
/// Removing an entry requires the test to actually pass (the ratchet asserts
/// both directions).
const KNOWN_FAILURES: &[(&str, &str)] = &[
    // Empirical baseline: 97 pass / 1 known-fail / 15 aux skips (was 46/52/15
    // before the typed-term engine refactor — focus and value nodes are now
    // oxigraph Terms end-to-end, so node-level value constraints, typed-literal
    // comparison, ill-formed-literal detection, sh:closed property enumeration,
    // own-path property shapes, nested sh:property, and qualified-shape sibling
    // disjointness all evaluate per spec). See docs/conformance/shacl.md.
    ("property/uniqueLang-002.ttl", "oxigraph's storage canonicalises \"1\"^^xsd:boolean to \"true\" (native value encoding), so the spec's literal-\"true\"-only activation of sh:uniqueLang is indistinguishable post-load"),
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

/// Multiset of expected focus nodes + expected conforms, read from the file's
/// embedded `mf:result` report. Returns `None` when the file declares no
/// `sht:Validate` entry (e.g. include-only manifests).
fn expected(store: &TripleStore) -> Option<(bool, BTreeMap<String, usize>)> {
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
    Some((conforms, focus))
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

fn run_one(path: &Path) -> Outcome {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Outcome::Skip("unreadable".into());
    };
    // The suite's canonical base: relative IRIs (`<>`, `<minLength-001>`)
    // resolve against the test file's location under datashapes.org.
    let rel = path
        .strip_prefix(SUITE_ROOT)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let base = format!("http://datashapes.org/sh/tests/core/{rel}");

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

    let Some((want_conforms, want_focus)) = expected(&store) else {
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
                    "http://datashapes.org/sh/tests/core/{}",
                    sibling
                        .strip_prefix(SUITE_ROOT)
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
        Err(e) => return Outcome::Fail(format!("validate error: {e}")),
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
    let mut files = Vec::new();
    suite_files(Path::new(SUITE_ROOT), &mut files);
    files.sort();
    assert!(
        files.len() > 100,
        "vendored suite present ({} files found)",
        files.len()
    );

    let mut pass = 0usize;
    let mut skip = Vec::new();
    let mut unexpected_failures = Vec::new();
    let mut unexpected_passes = Vec::new();

    for path in &files {
        let rel = path
            .strip_prefix(SUITE_ROOT)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let known = KNOWN_FAILURES.iter().find(|(k, _)| *k == rel);
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
        "W3C SHACL core: {pass} passed, {} known-fail, {} skipped, {} total",
        KNOWN_FAILURES.len(),
        skip.len(),
        files.len()
    );
    for s in &skip {
        println!("  SKIP {s}");
    }
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
}
