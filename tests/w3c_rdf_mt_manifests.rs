//! Runner for the W3C RDF 1.1 Semantics test cases (`rdf/rdf11/rdf-mt` of
//! w3c/rdf-tests), vendored unmodified in `tests/fixtures/w3c-rdf-mt/` (see
//! LICENSE.md and PROVENANCE.md there).
//!
//! Each entry's action is loaded into a fresh store and, for the `RDF` and
//! `RDFS` regimes, materialized with the RDFS engine (`simple` runs no
//! rules). A positive / negative entailment test then passes when the
//! result graph, its blank nodes read as variables, does / does not match
//! the asserted and derived triples. A result of `false` stands for an
//! inconsistent action: the RDFS engine must report a datatype clash.
//!
//! The `RDF` regime runs the RDFS closure too (the engine has no RDF-only
//! mode), so a negative `RDF` case whose result only RDFS entails fails;
//! such cases are listed in `KNOWN_FAILURES` with that reason.
//!
//! No score is published (W3C test-suite policy; `scripts/conformance_table.py`).
//! Gap policy, as in `w3c_sparql11_manifests.rs`: every case NOT in
//! `KNOWN_FAILURES` must pass, every listed case must still fail, and a pass
//! floor guards against a loader regression turning passes into skips.

#![cfg(feature = "rdfs-entailment")]

use open_triplestore::reasoning::common::ReasoningError;
use open_triplestore::reasoning::rdfs::RdfsMaterializer;
use open_triplestore::store::TripleStore;
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{Graph, NamedNode, NamedOrBlankNode, Term, Triple};

const ROOT: &str = "tests/fixtures/w3c-rdf-mt/";
const BASE: &str = "https://w3c.github.io/rdf-tests/rdf/rdf11/rdf-mt/";
const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const TG: &str = "urn:entailment:rdfs";

/// `(entry local name, why)`: cases that fail today. See
/// docs/conformance/entailment.md.
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("datatypes-non-well-formed-literal-1", "the case recognizes no datatypes; this store always recognizes its datatype map, so the ill-typed xsd:integer literal is an inconsistency here"),
    ("datatypes-semantic-equivalence-between-datatypes", "D-entailment between equal values of different datatypes (an integer and a decimal) is not materialized by the RDFS engine"),
    ("datatypes-semantic-equivalence-within-type-1", "D-entailment between value-equal literals of one datatype in different lexical forms: the store keeps literals as written (since 2026-10-03), and the RDFS engine does not materialize the other forms"),
    ("datatypes-semantic-equivalence-within-type-2", "as datatypes-semantic-equivalence-within-type-1"),
    ("double-infinity", "as datatypes-semantic-equivalence-within-type-1: value-equal xsd:double forms of infinity"),
    ("double-round-same", "as datatypes-semantic-equivalence-within-type-1: xsd:double forms that round to the same value"),
    ("float-infinity", "as datatypes-semantic-equivalence-within-type-1: value-equal xsd:float forms of infinity"),
    ("float-round-same", "as datatypes-semantic-equivalence-within-type-1: xsd:float forms that round to the same value"),
    ("literal-type", "rdfD1, exempt by decision D11: the result has a blank node standing for a typed literal's value, which is not materialized"),
    ("pfps-10-non-well-formed-literal-1", "rdfD1, exempt by decision D11: the result has a blank node standing for a typed literal's value, which is not materialized"),
    ("rdfs-entailment-test001", "the lexical forms of rdf:XMLLiteral are not checked, so an ill-formed one is not reported"),
    ("xmlsch-02-whitespace-facet-2", "an xsd:int lexical form with whitespace around it is ill-typed in RDF 1.1 but is not reported (whitespace facet not applied)"),
    ("xmlsch-02-whitespace-facet-3", "rdfD1, exempt by decision D11: the result has a blank node standing for a typed literal's value, which is not materialized"),
    ("xmlsch-02-whitespace-facet-4", "an xsd:int lexical form with whitespace around it is ill-typed in RDF 1.1 but is not reported (whitespace facet not applied)"),
];

/// Pass floor, a little below the current count.
const PASS_FLOOR: usize = 35;

#[derive(Debug)]
struct Entry {
    name: String,
    positive: bool,
    regime: String,
    action: String,
    /// `None` for `mf:result false`.
    result: Option<String>,
}

fn nn(ns: &str, local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{ns}{local}"))
}

fn parse(path_or_iri: &str) -> Result<Vec<Triple>, String> {
    let local = path_or_iri.strip_prefix(BASE).unwrap_or(path_or_iri);
    let path = format!("{ROOT}{local}");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let format = if local.ends_with(".nt") {
        RdfFormat::NTriples
    } else {
        RdfFormat::Turtle
    };
    RdfParser::from_format(format)
        .with_base_iri(format!("{BASE}{local}"))
        .map_err(|e| e.to_string())?
        .for_slice(text.as_bytes())
        .map(|q| q.map(Triple::from).map_err(|e| format!("{local}: {e}")))
        .collect()
}

fn object(g: &Graph, s: &NamedOrBlankNode, p: &NamedNode) -> Option<Term> {
    g.object_for_subject_predicate(s, p).map(|o| o.into_owned())
}

fn entries() -> Vec<Entry> {
    let g: Graph = parse(&format!("{BASE}manifest.ttl"))
        .expect("the vendored rdf-mt manifest")
        .into_iter()
        .collect();
    let mut subjects: Vec<NamedOrBlankNode> = Vec::new();
    for kind in ["PositiveEntailmentTest", "NegativeEntailmentTest"] {
        subjects.extend(
            g.subjects_for_predicate_object(&nn(RDF, "type"), &nn(MF, kind))
                .map(|s| s.into_owned()),
        );
    }
    let mut out = Vec::new();
    for s in subjects {
        let ty = object(&g, &s, &nn(RDF, "type"))
            .map(|t| t.to_string())
            .unwrap_or_default();
        let positive = ty.contains("PositiveEntailmentTest");
        let lexical = |p: &str| match object(&g, &s, &nn(MF, p)) {
            Some(Term::Literal(l)) => l.value().to_string(),
            Some(Term::NamedNode(n)) => n.as_str().to_string(),
            _ => String::new(),
        };
        let result = match object(&g, &s, &nn(MF, "result")) {
            Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
            _ => None,
        };
        let name = match &s {
            NamedOrBlankNode::NamedNode(n) => {
                n.as_str().rsplit('#').next().unwrap_or("").to_string()
            }
            NamedOrBlankNode::BlankNode(_) => lexical("name"),
        };
        out.push(Entry {
            name,
            positive,
            regime: lexical("entailmentRegime"),
            action: lexical("action"),
            result,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The SPARQL form of a term, blank nodes as variables.
fn pattern_term(t: &Term) -> String {
    match t {
        Term::BlankNode(b) => format!(
            "?b{}",
            b.as_str()
                .replace(|c: char| !c.is_ascii_alphanumeric(), "_")
        ),
        other => other.to_string(),
    }
}

/// Whether the asserted and derived triples match `graph`.
fn entails(store: &TripleStore, graph: &[Triple]) -> Result<bool, String> {
    let body: String = graph
        .iter()
        .map(|t| {
            let s: Term = t.subject.clone().into();
            format!(
                "{} {} {} .\n",
                pattern_term(&s),
                t.predicate,
                pattern_term(&t.object)
            )
        })
        .collect();
    match store
        .query_over(&format!("ASK {{ {body} }}"), &[TG.to_string()])
        .map_err(|e| e.to_string())?
    {
        oxigraph::sparql::QueryResults::Boolean(b) => Ok(b),
        _ => Err("ASK gave no boolean".into()),
    }
}

/// `Ok(())` when the case behaves as the manifest says.
fn run(e: &Entry) -> Result<(), String> {
    let store = TripleStore::in_memory().map_err(|e| e.to_string())?;
    let action = parse(&e.action)?;
    let nt: String = action.iter().map(|t| format!("{t} .\n")).collect();
    store
        .load_str(&nt, RdfFormat::NTriples, None)
        .map_err(|e| e.to_string())?;
    let mut inconsistent = false;
    if e.regime != "simple" {
        match RdfsMaterializer::with_target(&store, TG).materialize() {
            Ok(_) => {}
            Err(ReasoningError::Inconsistency { .. }) => inconsistent = true,
            Err(err) => return Err(err.to_string()),
        }
    }
    // An inconsistent graph entails everything, `false` included.
    let got = match &e.result {
        None => inconsistent,
        Some(_) if inconsistent => true,
        Some(result) => entails(&store, &parse(result)?)?,
    };
    if got == e.positive {
        Ok(())
    } else if e.positive {
        let mut detail = String::new();
        // With OTS_TEST_W3C_RDF_MT_EXPLAIN set: the result triples that do not
        // match one by one (at most five).
        if let (Some(result), Some(_)) =
            (&e.result, std::env::var_os("OTS_TEST_W3C_RDF_MT_EXPLAIN"))
        {
            let graph = parse(result)?;
            let missing: Vec<String> = graph
                .iter()
                .filter(|t| !entails(&store, std::slice::from_ref(*t)).unwrap_or(false))
                .take(5)
                .map(|t| t.to_string())
                .collect();
            detail = format!(": unmatched {missing:?}");
        }
        Err(format!(
            "{} regime: the result is not entailed{detail}",
            e.regime
        ))
    } else {
        Err(format!(
            "{} regime: the result is entailed but must not be",
            e.regime
        ))
    }
}

/// Entries in the vendored manifest.
const ENTRIES: usize = 51;

#[test]
fn w3c_rdf_mt_manifest_entries() {
    let entries = entries();
    assert_eq!(entries.len(), ENTRIES);
    for (id, why) in KNOWN_FAILURES {
        assert!(!why.is_empty(), "{id} needs a reason");
        assert!(
            entries.iter().any(|e| &e.name == id),
            "{id} is not an entry"
        );
    }
}

#[test]
fn w3c_rdf_mt_suite() {
    let mut passed = 0usize;
    let (mut unexpected, mut fixed) = (Vec::new(), Vec::new());
    for e in entries() {
        let known = KNOWN_FAILURES.iter().any(|(id, _)| *id == e.name);
        match run(&e) {
            Ok(()) if known => fixed.push(e.name.clone()),
            Ok(()) => passed += 1,
            Err(why) if !known => unexpected.push(format!("{}: {why}", e.name)),
            Err(_) => {}
        }
    }
    assert!(
        unexpected.is_empty(),
        "cases outside KNOWN_FAILURES fail:\n{}",
        unexpected.join("\n")
    );
    assert!(
        fixed.is_empty(),
        "KNOWN_FAILURES entries now pass, remove them: {fixed:?}"
    );
    assert!(passed >= PASS_FLOOR, "pass floor: {passed} < {PASS_FLOOR}");
}
