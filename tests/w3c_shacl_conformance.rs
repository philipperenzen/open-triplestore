//! Runner for the `core` and `sparql` sections of the W3C SHACL test suite,
//! vendored under `tests/fixtures/w3c-shacl/{core,sparql}` (see PROVENANCE.md
//! and LICENSE.md there). Its results are development and regression results
//! at the comparison levels below, not a W3C conformance claim.
//!
//! Each suite file is self-contained: data + shapes + an `mf:Manifest` entry
//! (`sht:Validate`) + the expected `sh:ValidationReport` — or `sht:Failure`,
//! for the `sparql/pre-binding/unsupported-*` cases whose shapes graph the
//! validator must reject. The runner loads the file as both shapes graph and
//! data graph (the suite is designed for this — `sht:dataGraph <>` /
//! `sht:shapesGraph <>` reference the file itself), runs our validator and
//! compares `sh:conforms` and the **multiset of results**, each on focus
//! node, `sh:resultPath` (as a path structure), `sh:value`, `sh:sourceShape`,
//! `sh:sourceConstraintComponent`, `sh:resultSeverity` and
//! `sh:sourceConstraint` — everything but `sh:resultMessage`, whose wording the
//! spec leaves to the processor. Our side is the RDF report `report_rdf`
//! writes, loaded back, so the RDF serialisation is under test too. Blank nodes
//! of the data graph (focus nodes, values) are wildcards; shape and constraint
//! blank nodes must be the very node of the shapes graph. (Until 2026-10-02 the
//! runner compared only `sh:conforms` and the focus-node multiset; full report
//! equality replaced that once both agreed on every case.)
//!
//! For `sht:Failure` both check that validation returned an error.
//!
//! Gap policy (two-way ratchet): every test NOT in `KNOWN_FAILURES` must
//! pass, and every listed test must still fail — so silent regressions *and*
//! silent fixes both turn the suite red, keeping the lists honest.
//!
//! `OPTIONAL_UNSUPPORTED` is a third category, for tests of a feature the
//! specification makes optional and requires a processor that lacks it to
//! report as a failure: such a test passes when validation fails with that
//! failure, and fails if it ever produces a report.

use open_triplestore::shacl::report::ValidationReport;
use open_triplestore::shacl::validate;
use open_triplestore::shacl_studio::report_rdf::report_to_turtle;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// The two vendored sections: SHACL Core, and SHACL-SPARQL (sh:sparql
/// constraints, constraint components, pre-binding).
const SUITES: &[&str] = &["core", "sparql"];
const FIXTURES: &str = "tests/fixtures/w3c-shacl";

/// Tests that currently fail, with the engine gap they sit behind. Keep sorted.
/// Removing an entry requires the test to actually pass (the ratchet asserts
/// both directions). Keys are `<suite>/<path within the suite>`.
///
/// Empirical baseline: 119 pass / 1 known-fail / 15 aux skips / 1 optional unsupported
const KNOWN_FAILURES: &[(&str, &str)] = &[
    // core: 97 pass / 1 known-fail / 15 aux skips (was 46/52/15 before the
    // typed-term engine refactor — focus and value nodes are now oxigraph
    // Terms end-to-end). See docs/conformance/shacl.md.
    ("core/property/uniqueLang-002.ttl", "oxigraph's storage canonicalises \"1\"^^xsd:boolean to \"true\" (native value encoding), so the spec's literal-\"true\"-only activation of sh:uniqueLang is indistinguishable post-load"),
    // sparql: 22 pass / 0 known-fail / 0 skips / 1 optional unsupported.
];

/// Tests of an optional feature this processor does not implement, where the
/// specification requires the processor to report a failure — which it does.
/// Each entry names the error text the failure must carry. These pass when
/// validation fails with that error, and fail on a report.
const OPTIONAL_UNSUPPORTED: &[(&str, &str, &str)] = &[(
    "sparql/pre-binding/shapesGraph-001.ttl",
    "$shapesGraph",
    "$shapesGraph and $currentShape are optional (SHACL §5.3.1), and a processor without them \
     must report a failure when a constraint uses them, which this one does; the test's \
     expected report assumes support, which w3c/data-shapes#426 contests, and SHACL 1.2 \
     SPARQL Extensions drops both variables",
)];

#[derive(Debug, PartialEq)]
enum Outcome {
    Pass,
    Fail(String),
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

enum Expected {
    /// The expected `sh:conforms`; the results are read from the store.
    Report(bool),
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

    Some(Expected::Report(conforms))
}

/// A loaded test case: the store with the file in `urn:t:shapes` and
/// `urn:t:data` (plus any sibling graphs), and what it expects.
struct Case {
    store: TripleStore,
    expected: Expected,
}

/// Load one suite file; `Err` is the reason to skip it.
fn load_case(suite: &str, root: &Path, path: &Path) -> Result<Case, String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Err("unreadable".into());
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
        Err(e) => return Err(format!("store: {e}")),
    };
    // The manifest/expected-report triples always come from the main file.
    for graph in ["urn:t:shapes", "urn:t:data"] {
        if let Err(e) = store.load_str_with_base(&content, RdfFormat::Turtle, &base, Some(graph)) {
            return Err(format!("parse: {e}"));
        }
    }

    let Some(expected) = expected(&store) else {
        return Err("no sht:Validate entry".into());
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
                    return Err(format!("external graph not found: {stem}"));
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
                    return Err(format!("parse {stem}: {e}"));
                }
            }
        }
    }
    Ok(Case { store, expected })
}

fn run_validate(case: &Case) -> Result<ValidationReport, String> {
    validate(&case.store, "urn:t:shapes", &["urn:t:data".to_string()])
}

/// An `OPTIONAL_UNSUPPORTED` case: validation must fail, naming `needle`.
fn check_optional(case: &Case, needle: &str) -> Outcome {
    match run_validate(case) {
        Err(e) if e.contains(needle) => Outcome::Pass,
        Err(e) => Outcome::Fail(format!(
            "expected the optional-feature failure naming {needle}, got another error: {e}"
        )),
        Ok(r) => Outcome::Fail(format!(
            "expected a failure naming {needle} (optional feature, unsupported), got a report with conforms={}",
            r.conforms
        )),
    }
}

/// Validate, settling `sht:Failure` cases and a wrong `sh:conforms`; `Ok`
/// carries the report of a report case.
fn validated(case: &Case) -> Result<ValidationReport, Outcome> {
    let report = match run_validate(case) {
        Ok(r) => r,
        Err(e) => {
            return Err(match case.expected {
                Expected::Failure => Outcome::Pass,
                Expected::Report(..) => Outcome::Fail(format!("validate error: {e}")),
            })
        }
    };
    match &case.expected {
        Expected::Failure => Err(Outcome::Fail(format!(
            "expected the validator to reject the shapes graph (sht:Failure), got a report with conforms={}",
            report.conforms
        ))),
        Expected::Report(want_conforms) => {
            if report.conforms != *want_conforms {
                return Err(Outcome::Fail(format!(
                    "conforms: want {want_conforms}, got {} ({} results)",
                    report.conforms, report.results_count
                )));
            }
            Ok(report)
        }
    }
}

const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

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
/// (a SPARQL pattern binding `?res`) in `graph`, each as a comparable key over
/// everything but `sh:resultMessage`.
fn result_keys(store: &TripleStore, graph: &str, report_pattern: &str) -> BTreeMap<String, usize> {
    let triples = graph_triples(store, graph);
    let q = format!(
        "PREFIX sht: <http://www.w3.org/ns/shacl-test#> \
         PREFIX mf: <http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#> \
         PREFIX sh: <http://www.w3.org/ns/shacl#> \
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
        let key = format!(
            "focus={} path={} value={} shape={} component={} severity={} constraint={}",
            field("focusNode", &wildcard_key),
            field("resultPath", &|t| path_key(&triples, t, 0)),
            field("value", &wildcard_key),
            field("sourceShape", &exact_key),
            field("sourceConstraintComponent", &exact_key),
            field("resultSeverity", &exact_key),
            field("sourceConstraint", &exact_key),
        );
        *out.entry(key).or_insert(0) += 1;
    }
    out
}

/// `sh:conforms` and the full result multiset, compared through the RDF report
/// our writer produces.
fn run_case(case: &Case) -> Outcome {
    let report = match validated(case) {
        Ok(v) => v,
        Err(outcome) => return outcome,
    };
    let want = result_keys(
        &case.store,
        "urn:t:shapes",
        "?t a sht:Validate ; mf:result ?r . ?r sh:result ?res",
    );
    let ttl = report_to_turtle(&report, "urn:t:report#run");
    if let Err(e) = case.store.load_str_with_base(
        &ttl,
        RdfFormat::Turtle,
        "urn:t:report",
        Some("urn:t:report"),
    ) {
        return Outcome::Fail(format!("our report RDF does not load: {e}"));
    }
    let got = result_keys(
        &case.store,
        "urn:t:report",
        "?r a sh:ValidationReport ; sh:result ?res",
    );
    if got == want {
        return Outcome::Pass;
    }
    let diff = |a: &BTreeMap<String, usize>, b: &BTreeMap<String, usize>| -> Vec<String> {
        a.iter()
            .filter_map(|(k, n)| {
                let m = b.get(k).copied().unwrap_or(0);
                (*n > m).then(|| format!("{}x {k}", n - m))
            })
            .collect()
    };
    Outcome::Fail(format!(
        "results differ ({} expected, {} got)\n      missing: {:?}\n      unexpected: {:?}",
        want.values().sum::<usize>(),
        got.values().sum::<usize>(),
        diff(&want, &got),
        diff(&got, &want),
    ))
}

/// What a pass over every suite file found.
#[derive(Default)]
struct Tally {
    total_files: usize,
    pass: usize,
    optional: usize,
    known_fail: usize,
    skip: Vec<String>,
    unexpected_failures: Vec<String>,
    unexpected_passes: Vec<String>,
    seen_known: usize,
    seen_optional: usize,
}

/// Run every suite file under the two-way ratchet of `KNOWN_FAILURES`.
fn run_suite() -> Tally {
    let mut t = Tally::default();
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
        t.total_files += files.len();
        let (mut suite_pass, mut suite_known, mut suite_skip, mut suite_optional) = (0, 0, 0, 0);

        for path in &files {
            let rel = format!(
                "{suite}/{}",
                path.strip_prefix(&root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/")
            );
            let case = match load_case(suite, &root, path) {
                Ok(c) => c,
                Err(reason) => {
                    suite_skip += 1;
                    t.skip.push(format!("{rel}: {reason}"));
                    continue;
                }
            };
            if let Some((_, needle, _)) = OPTIONAL_UNSUPPORTED.iter().find(|(k, ..)| *k == rel) {
                t.seen_optional += 1;
                match check_optional(&case, needle) {
                    Outcome::Pass => {
                        t.optional += 1;
                        suite_optional += 1;
                    }
                    Outcome::Fail(reason) => t.unexpected_failures.push(format!("{rel}: {reason}")),
                }
                continue;
            }
            let known_entry = KNOWN_FAILURES.iter().find(|(k, _)| *k == rel);
            if known_entry.is_some() {
                t.seen_known += 1;
            }
            match run_case(&case) {
                Outcome::Pass => {
                    t.pass += 1;
                    suite_pass += 1;
                    if let Some((k, why)) = known_entry {
                        t.unexpected_passes.push(format!("{k} (listed as: {why})"));
                    }
                }
                Outcome::Fail(reason) => {
                    if known_entry.is_none() {
                        t.unexpected_failures.push(format!("{rel}: {reason}"));
                    } else {
                        t.known_fail += 1;
                        suite_known += 1;
                    }
                }
            }
        }
        println!(
            "W3C SHACL {suite}: {suite_pass} passed, {suite_known} known-fail, \
             {suite_optional} optional unsupported, {suite_skip} skipped, {} files",
            files.len()
        );
    }
    println!(
        "W3C SHACL total: {} passed, {} known-fail, {} optional unsupported, {} skipped, {} files",
        t.pass,
        t.known_fail,
        t.optional,
        t.skip.len(),
        t.total_files
    );
    for s in &t.skip {
        println!("  SKIP {s}");
    }
    t
}

#[test]
fn w3c_shacl_full_report_equality() {
    let t = run_suite();
    assert_eq!(
        t.seen_known,
        KNOWN_FAILURES.len(),
        "every KNOWN_FAILURES key must name a vendored, loadable file (stale entries?)"
    );
    assert_eq!(
        t.seen_optional,
        OPTIONAL_UNSUPPORTED.len(),
        "every OPTIONAL_UNSUPPORTED key must name a vendored, loadable file"
    );
    assert!(
        t.unexpected_failures.is_empty(),
        "tests failing that are not in KNOWN_FAILURES:\n  {}",
        t.unexpected_failures.join("\n  ")
    );
    assert!(
        t.unexpected_passes.is_empty(),
        "KNOWN_FAILURES entries now pass — remove them to ratchet forward:\n  {}",
        t.unexpected_passes.join("\n  ")
    );
    assert!(
        t.skip.len() <= 20,
        "{} cases were skipped (ceiling 20) — a parse/load regression turns passes into skips:\n  {}",
        t.skip.len(),
        t.skip.join("\n  ")
    );
    // A floor as well as a ratchet. The runner turns an unreadable or
    // unparseable file into a silent skip, so a Turtle-parser regression
    // would have turned every file into a skip and still passed the
    // asserts above. 119 pass today (97 core + 22 sparql); 110 leaves
    // headroom for suite churn.
    assert!(
        t.pass >= 110,
        "only {} W3C SHACL cases passed (floor 110); skips: {}",
        t.pass,
        t.skip.len()
    );
}
