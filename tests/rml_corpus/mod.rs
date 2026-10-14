//! What the vendored RML corpus runners share: running one case through the
//! file executor and comparing its output dataset with the expected one,
//! and holding a whole suite to its known-failure list.
//!
//! A case passes when its output dataset is isomorphic to the expected one
//! (blank nodes matched by canonicalisation; literals compared after the
//! store's canonical encoding on both sides), or — for a case that expects an
//! error — when the mapping is refused or the run fails.

#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use open_triplestore::rml::checks::OnDataError;
use open_triplestore::rml::{execute_with, parse_rml};
use open_triplestore::store::TripleStore;
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::dataset::CanonicalizationAlgorithm;
use oxigraph::model::Dataset;
use oxigraph::sparql::QueryResults;

/// One test case.
#[derive(Debug, Clone)]
pub struct Case {
    pub id: String,
    pub dir: PathBuf,
    pub mapping: String,
    /// The expected output's file name; `None` when there is none.
    pub output: Option<String>,
    /// Whether the case expects the mapping to be refused or the run to fail.
    pub error: bool,
    /// The base IRI the case's relative IRIs resolve against.
    pub base: String,
}

#[derive(Debug)]
pub enum Outcome {
    Pass,
    Fail(String),
}

/// The fixture directory of a vendored corpus.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The cases a W3C KG-Construct manifest (`test:TestCase`, `rmltest:*`)
/// lists, in identifier order.
pub fn manifest_cases(dir: &Path, default_base: &str) -> Vec<Case> {
    let store = TripleStore::in_memory().unwrap();
    let text = std::fs::read_to_string(dir.join("manifest.ttl")).expect("manifest.ttl");
    store
        .load_str(&text, RdfFormat::Turtle, None)
        .expect("the manifest parses");
    let q = "PREFIX test: <http://www.w3.org/2006/03/test-description#>
             PREFIX rmltest: <http://w3id.org/rml/test/>
             PREFIX dcterms: <http://purl.org/dc/terms/>
             SELECT ?id ?error ?mapping ?base ?output WHERE {
               ?c a test:TestCase ;
                  dcterms:identifier ?id ;
                  rmltest:hasError ?error ;
                  rmltest:mappingDocument ?mapping .
               OPTIONAL { ?c rmltest:defaultBaseIRI ?base }
               OPTIONAL { ?c rmltest:output ?o . ?o rmltest:output ?output }
             } ORDER BY ?id";
    let QueryResults::Solutions(sols) = store.query(q).expect("manifest query") else {
        panic!("expected solutions")
    };
    let value = |t: Option<&oxigraph::model::Term>| -> Option<String> {
        match t? {
            oxigraph::model::Term::Literal(l) => Some(l.value().to_string()),
            oxigraph::model::Term::NamedNode(n) => Some(n.as_str().to_string()),
            _ => None,
        }
    };
    let mut out: Vec<Case> = Vec::new();
    for s in sols {
        let s = s.expect("solution");
        let id = value(s.get("id")).unwrap();
        if out.iter().any(|c| c.id == id) {
            continue;
        }
        out.push(Case {
            dir: dir.join(&id),
            mapping: value(s.get("mapping")).unwrap(),
            output: value(s.get("output")),
            error: value(s.get("error")).as_deref() == Some("true"),
            base: value(s.get("base")).unwrap_or_else(|| default_base.to_string()),
            id,
        });
    }
    out
}

/// The store's contents as a dataset with canonical blank-node labels.
fn canonical(store: &TripleStore) -> Dataset {
    let mut nq = Vec::new();
    store.dump_all_nquads(&mut nq).expect("dump");
    let mut ds = Dataset::new();
    for q in RdfParser::from_format(RdfFormat::NQuads).for_slice(&nq) {
        ds.insert(&q.expect("own dump parses"));
    }
    ds.canonicalize(CanonicalizationAlgorithm::Unstable);
    ds
}

/// The case's input files: everything in its directory but the mapping,
/// the expected output and the README.
fn inputs(case: &Case) -> HashMap<String, Vec<u8>> {
    let mut out = HashMap::new();
    for entry in std::fs::read_dir(&case.dir).expect("case directory") {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == case.mapping
            || name.ends_with(".nq")
            || name == "README.md"
            || !entry.file_type().unwrap().is_file()
        {
            continue;
        }
        out.insert(name, std::fs::read(entry.path()).unwrap());
    }
    out
}

/// Run one case.
pub fn run_case(case: &Case, on_data_error: OnDataError) -> Outcome {
    let text = match std::fs::read_to_string(case.dir.join(&case.mapping)) {
        Ok(t) => t,
        Err(e) => return Outcome::Fail(format!("reading the mapping: {e}")),
    };
    let store = TripleStore::in_memory().unwrap();
    let result = parse_rml(&text).and_then(|mut mapping| {
        mapping.base_iri = Some(case.base.clone());
        execute_with(&mapping, &inputs(case), &store, None, on_data_error, |_| {
            Ok(())
        })
    });
    match (case.error, result) {
        (true, Err(_)) => Outcome::Pass,
        (true, Ok(_)) => Outcome::Fail(format!(
            "expected an error, but the run produced {} quads",
            store.len().unwrap_or(0)
        )),
        (false, Err(e)) => Outcome::Fail(format!("error: {e}")),
        (false, Ok(_)) => {
            let expected = TripleStore::in_memory().unwrap();
            if let Some(output) = &case.output {
                let text = match std::fs::read_to_string(case.dir.join(output)) {
                    Ok(t) => t,
                    Err(e) => return Outcome::Fail(format!("reading {output}: {e}")),
                };
                if let Err(e) = expected.load_str(&text, RdfFormat::NQuads, None) {
                    return Outcome::Fail(format!("the expected output does not parse: {e}"));
                }
            }
            let (want, got) = (canonical(&expected), canonical(&store));
            if want == got {
                Outcome::Pass
            } else {
                let w: BTreeSet<String> = want.iter().map(|q| q.to_string()).collect();
                let g: BTreeSet<String> = got.iter().map(|q| q.to_string()).collect();
                let missing: Vec<&String> = w.difference(&g).take(4).collect();
                let extra: Vec<&String> = g.difference(&w).take(4).collect();
                Outcome::Fail(format!(
                    "output differs; missing {missing:?}, unexpected {extra:?}"
                ))
            }
        }
    }
}

/// What a suite run counted.
#[derive(Debug, Default)]
pub struct Tally {
    pub pass: usize,
    pub known: usize,
    pub skipped: usize,
}

/// Run every case and hold the suite to `known` (failures, with reasons)
/// and `skips` (cases the runner does not run, with reasons): a case that
/// fails without being listed fails the suite, and so does a listed case
/// that passes, or a list entry the suite does not have.
pub fn run_suite(
    name: &str,
    cases: &[Case],
    known: &[(&str, &str)],
    skips: &[(&str, &str)],
    on_data_error: OnDataError,
) -> Tally {
    let mut tally = Tally::default();
    let mut unexpected = Vec::new();
    let mut fixed = Vec::new();
    for case in cases {
        if skips.iter().any(|(id, _)| *id == case.id) {
            tally.skipped += 1;
            continue;
        }
        let listed = known.iter().any(|(id, _)| *id == case.id);
        match (run_case(case, on_data_error), listed) {
            (Outcome::Pass, false) => tally.pass += 1,
            (Outcome::Pass, true) => fixed.push(case.id.clone()),
            (Outcome::Fail(_), true) => tally.known += 1,
            (Outcome::Fail(why), false) => unexpected.push(format!("{}: {why}", case.id)),
        }
    }
    let ids: BTreeSet<&str> = cases.iter().map(|c| c.id.as_str()).collect();
    let stale: Vec<&str> = known
        .iter()
        .chain(skips.iter())
        .map(|(id, _)| *id)
        .filter(|id| !ids.contains(id))
        .collect();
    eprintln!(
        "{name}: {} passed, {} known failures, {} skipped, {} cases",
        tally.pass,
        tally.known,
        tally.skipped,
        cases.len()
    );
    assert!(
        unexpected.is_empty() && fixed.is_empty() && stale.is_empty(),
        "{name}: {} unexpected failure(s):\n  {}\nknown failures that now pass (remove them): \
         {fixed:?}\nlist entries the suite does not have: {stale:?}",
        unexpected.len(),
        unexpected.join("\n  ")
    );
    tally
}

/// The `Empirical baseline: N pass / N known-fail / N aux skips` comment of
/// a runner, which `scripts/conformance_table.py` publishes — held to what
/// the run counted, so a stale comment fails the test.
pub fn assert_baseline(source: &str, tally: &Tally) {
    let line = source
        .lines()
        .find(|l| l.contains("Empirical baseline:"))
        .expect("the runner records its baseline");
    let expected = format!(
        "Empirical baseline: {} pass / {} known-fail / {} aux skips",
        tally.pass, tally.known, tally.skipped
    );
    assert!(
        line.contains(&expected),
        "the runner's baseline comment is stale: it says\n  {}\nbut the run counted\n  {expected}",
        line.trim()
    );
}
