//! Runner for the approved W3C OWL 2 test cases of the RL profile (OWL 2
//! Conformance §2.2–2.3), from the vendored, unmodified
//! `tests/fixtures/w3c-owl2/approved/all.rdf` (see LICENSE.md and
//! PROVENANCE.md there).
//!
//! It picks the approved cases marked `test:profile test:RL` that have an
//! RDF/XML premise, loads the premise (with its `test:importedOntology`
//! documents: the imports closure) into a store, and materializes it with the
//! OWL 2 RL engine:
//!
//! - `test:ConsistencyTest` / `test:InconsistencyTest`: the run must succeed /
//!   end in `ReasoningError::Inconsistency`;
//! - `test:PositiveEntailmentTest` / `test:NegativeEntailmentTest`: the
//!   conclusion graph, its blank nodes read as variables, must / must not
//!   match the asserted and derived triples. The conclusion's ontology header
//!   (`rdf:type owl:Ontology`) is not part of what is entailed.
//!
//! A case with several types passes when every one of them does. RL is a
//! partial axiomatization of the OWL 2 RDF-Based Semantics (OWL 2 Profiles
//! §4.3, Theorem PR1 bounds what it must entail), so some entailment cases
//! are out of its reach; those are listed in `KNOWN_FAILURES` with the reason.
//!
//! The suite's licence allows no performance claims for a partial or altered
//! run, so no score is published (`scripts/conformance_table.py`,
//! docs/conformance/owl2-rl.md). Gap policy, as in
//! `w3c_sparql11_manifests.rs`: every case NOT in `KNOWN_FAILURES` must pass,
//! every listed case must still fail, and a pass floor guards against a
//! loader regression turning passes into skips.

#![cfg(feature = "owl2-rl")]

use open_triplestore::reasoning::common::ReasoningError;
use open_triplestore::reasoning::owl2_rl::Owl2RLReasoner;
use open_triplestore::store::TripleStore;
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{Graph, NamedNode, NamedOrBlankNode, Term, Triple};

const MANIFEST: &str = "tests/fixtures/w3c-owl2/approved/all.rdf";
const T: &str = "http://www.w3.org/2007/OWL/testOntology#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL_ONTOLOGY: &str = "http://www.w3.org/2002/07/owl#Ontology";
const TG: &str = "urn:entailment:owl2-rl";

/// `(test:identifier, why)`: cases that fail today. See
/// docs/conformance/owl2-rl.md.
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("DisjointClasses-001", "the conclusion is a class axiom over a class expression the premise does not contain; the RL/RDF rules conclude no new class expressions (OWL 2 Profiles §4.3, Theorem PR1 covers assertions)"),
    ("DisjointClasses-003", "the conclusion is a class axiom over a class expression the premise does not contain; the RL/RDF rules conclude no new class expressions (OWL 2 Profiles §4.3, Theorem PR1 covers assertions)"),
    ("New-Feature-DisjointDataProperties-002", "the conclusion is an owl:AllDifferent axiom; no OWL 2 RL/RDF rule concludes owl:differentFrom or owl:AllDifferent"),
    ("New-Feature-DisjointObjectProperties-001", "no OWL 2 RL/RDF rule concludes owl:differentFrom"),
    ("New-Feature-DisjointObjectProperties-002", "the conclusion is an owl:AllDifferent axiom; no OWL 2 RL/RDF rule concludes owl:differentFrom or owl:AllDifferent"),
    ("New-Feature-ObjectQCR-002", "the conclusion is a class axiom over a class expression the premise does not contain; the RL/RDF rules conclude no new class expressions (OWL 2 Profiles §4.3, Theorem PR1 covers assertions)"),
    ("New-Feature-ReflexiveProperty-001", "owl:ReflexiveProperty is outside OWL 2 RL; no RL/RDF rule reads it"),
    ("WebOnt-I4.6-005-Direct", "the conclusion carries an annotation the premise does not; annotations have no Direct Semantics meaning, but the runner matches triples"),
    ("WebOnt-I5.26-010", "the conclusion is an owl:minCardinality restriction, an existential the RL/RDF rules never conclude"),
    ("WebOnt-I5.5-005", "the conclusion is a class axiom over a class expression the premise does not contain; the RL/RDF rules conclude no new class expressions (OWL 2 Profiles §4.3, Theorem PR1 covers assertions)"),
    ("WebOnt-I5.8-006", "the conclusion is a datatype range derived from datatype intersections; the RL/RDF rules conclude no datatype ranges"),
    ("WebOnt-I5.8-008", "as WebOnt-I5.8-006"),
    ("WebOnt-I5.8-009", "as WebOnt-I5.8-006"),
    ("WebOnt-differentFrom-001", "the symmetry of owl:differentFrom: no OWL 2 RL/RDF rule concludes owl:differentFrom"),
    ("WebOnt-equivalentClass-008-Direct", "as WebOnt-I4.6-005-Direct: an annotation in the conclusion"),
    ("chain2trans1", "the conclusion is owl:TransitiveProperty, a property axiom the RL/RDF rules never conclude"),
    ("owl2-rl-rules-fp-differentFrom", "no OWL 2 RL/RDF rule concludes owl:differentFrom (here from a functional property and different values)"),
    ("owl2-rl-rules-ifp-differentFrom", "no OWL 2 RL/RDF rule concludes owl:differentFrom (here from an inverse-functional property)"),
];

/// Pass floor: a loader regression that turns passes into skips must not go
/// unnoticed. It sits a little below the current count.
const PASS_FLOOR: usize = 45;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Consistency,
    Inconsistency,
    PositiveEntailment,
    NegativeEntailment,
}

#[derive(Debug)]
struct Case {
    id: String,
    kinds: Vec<Kind>,
    premise: Option<String>,
    conclusion: Option<String>,
    non_conclusion: Option<String>,
    imports: Vec<String>,
}

fn iri(s: &str) -> NamedNode {
    NamedNode::new_unchecked(s)
}

fn t(local: &str) -> NamedNode {
    iri(&format!("{T}{local}"))
}

fn objects(g: &Graph, s: &NamedOrBlankNode, p: &NamedNode) -> Vec<Term> {
    g.objects_for_subject_predicate(s, p)
        .map(|o| o.into_owned())
        .collect()
}

fn literal(g: &Graph, s: &NamedOrBlankNode, local: &str) -> Option<String> {
    objects(g, s, &t(local)).into_iter().find_map(|o| match o {
        Term::Literal(l) => Some(l.value().to_string()),
        _ => None,
    })
}

fn has(g: &Graph, s: &NamedOrBlankNode, p: &NamedNode, o: &NamedNode) -> bool {
    objects(g, s, p)
        .iter()
        .any(|x| x == &Term::NamedNode(o.clone()))
}
/// oxigraph's RDF/XML parser reads only double-quoted `<!ENTITY>` values;
/// the manifest and some test documents quote them with apostrophes. Swap
/// those quotes in memory (the vendored file stays unmodified).
fn double_quoted_entities(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(i) = rest.find("<!ENTITY") {
        let (head, tail) = rest.split_at(i);
        out.push_str(head);
        let end = tail.find('>').map_or(tail.len(), |e| e + 1);
        let decl = &tail[..end];
        match (decl.find('\''), decl.rfind('\'')) {
            (Some(a), Some(b)) if a < b && !decl.contains('"') => {
                out.push_str(&decl[..a]);
                out.push('"');
                out.push_str(&decl[a + 1..b]);
                out.push('"');
                out.push_str(&decl[b + 1..]);
            }
            _ => out.push_str(decl),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// The approved OWL 2 DL / Direct Semantics cases, in identifier order.

/// The approved RL-profile cases, in identifier order.
fn cases() -> Vec<Case> {
    let raw = std::fs::read_to_string(MANIFEST).expect("the vendored OWL 2 test manifest");
    let xml = double_quoted_entities(&raw);
    let g: Graph = RdfParser::from_format(RdfFormat::RdfXml)
        .for_slice(xml.as_bytes())
        .map(|q| q.map(Triple::from))
        .collect::<Result<_, _>>()
        .expect("the OWL 2 test manifest parses as RDF/XML");
    let ty = iri(RDF_TYPE);
    let subjects: Vec<NamedOrBlankNode> = g
        .subjects_for_predicate_object(&ty, &t("TestCase"))
        .map(|s| s.into_owned())
        .collect();
    let mut out = Vec::new();
    for s in subjects {
        if !has(&g, &s, &t("status"), &t("Approved")) || !has(&g, &s, &t("profile"), &t("RL")) {
            continue;
        }
        let mut kinds = Vec::new();
        for (local, k) in [
            ("ConsistencyTest", Kind::Consistency),
            ("InconsistencyTest", Kind::Inconsistency),
            ("PositiveEntailmentTest", Kind::PositiveEntailment),
            ("NegativeEntailmentTest", Kind::NegativeEntailment),
        ] {
            if has(&g, &s, &ty, &t(local)) {
                kinds.push(k);
            }
        }
        let imports = objects(&g, &s, &t("importedOntology"))
            .into_iter()
            .filter_map(|o| match o {
                Term::NamedNode(n) => {
                    literal(&g, &NamedOrBlankNode::NamedNode(n), "rdfXmlInputOntology")
                }
                _ => None,
            })
            .collect();
        out.push(Case {
            id: literal(&g, &s, "identifier").expect("every case has a test:identifier"),
            kinds,
            premise: literal(&g, &s, "rdfXmlPremiseOntology"),
            conclusion: literal(&g, &s, "rdfXmlConclusionOntology"),
            non_conclusion: literal(&g, &s, "rdfXmlNonConclusionOntology"),
            imports,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// An RDF/XML document's triples.
fn triples(rdfxml: &str, id: &str) -> Result<Vec<Triple>, String> {
    let base = format!("http://owl.semanticweb.org/id/{id}");
    let rdfxml = double_quoted_entities(rdfxml);
    RdfParser::from_format(RdfFormat::RdfXml)
        .with_base_iri(&base)
        .map_err(|e| e.to_string())?
        .for_slice(rdfxml.as_bytes())
        .map(|q| q.map(Triple::from).map_err(|e| format!("RDF/XML: {e}")))
        .collect()
}

#[derive(Debug)]
enum Outcome {
    Pass,
    Fail(String),
    /// No RDF/XML document to run.
    Skip,
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

/// Whether the store's asserted and derived triples match the graph.
fn entails(store: &TripleStore, graph: &[Triple]) -> Result<bool, String> {
    let mut body = String::new();
    for tr in graph {
        if tr.predicate.as_str() == RDF_TYPE && tr.object == Term::NamedNode(iri(OWL_ONTOLOGY)) {
            continue;
        }
        let s: Term = tr.subject.clone().into();
        body.push_str(&format!(
            "{} {} {} .\n",
            pattern_term(&s),
            tr.predicate,
            pattern_term(&tr.object)
        ));
    }
    let q = format!("ASK {{ {body} }}");
    match store
        .query_over(&q, &[TG.to_string()])
        .map_err(|e| e.to_string())?
    {
        oxigraph::sparql::QueryResults::Boolean(b) => Ok(b),
        _ => Err("ASK gave no boolean".into()),
    }
}

/// With `OTS_TEST_W3C_OWL2_RL_EXPLAIN` set: the conclusion triples that do
/// not match one by one (at most five), to diagnose a failing case.
fn unmatched(store: &TripleStore, graph: &[Triple]) -> String {
    if std::env::var_os("OTS_TEST_W3C_OWL2_RL_EXPLAIN").is_none() {
        return String::new();
    }
    let missing: Vec<String> = graph
        .iter()
        .filter(|t| !entails(store, std::slice::from_ref(*t)).unwrap_or(false))
        .take(5)
        .map(|t| t.to_string())
        .collect();
    format!(": unmatched {missing:?}")
}

fn run(c: &Case) -> Outcome {
    let Some(premise) = &c.premise else {
        return Outcome::Skip;
    };
    let store = TripleStore::in_memory().expect("store");
    let mut docs = vec![premise.clone()];
    docs.extend(c.imports.iter().cloned());
    for doc in &docs {
        let ts = match triples(doc, &c.id) {
            Ok(ts) => ts,
            Err(e) => return Outcome::Fail(format!("premise: {e}")),
        };
        let nt: String = ts.iter().map(|t| format!("{t} .\n")).collect();
        if let Err(e) = store.load_str(&nt, RdfFormat::NTriples, None) {
            return Outcome::Fail(format!("load: {e}"));
        }
    }
    let result = Owl2RLReasoner::new(&store).materialize();
    for kind in &c.kinds {
        let ok = match kind {
            Kind::Consistency => match &result {
                Ok(_) => true,
                Err(e) => return Outcome::Fail(format!("consistent case, engine: {e}")),
            },
            Kind::Inconsistency => matches!(result, Err(ReasoningError::Inconsistency { .. })),
            Kind::PositiveEntailment | Kind::NegativeEntailment => {
                if let Err(e) = &result {
                    return Outcome::Fail(format!("entailment case, engine: {e}"));
                }
                let (doc, want) = match kind {
                    Kind::PositiveEntailment => (&c.conclusion, true),
                    _ => (&c.non_conclusion, false),
                };
                let Some(doc) = doc else {
                    return Outcome::Skip;
                };
                let graph = match triples(doc, &c.id) {
                    Ok(g) => g,
                    Err(e) => return Outcome::Fail(format!("conclusion: {e}")),
                };
                match entails(&store, &graph) {
                    Ok(got) if got == want => true,
                    Ok(_) => {
                        return Outcome::Fail(format!(
                            "{kind:?} does not hold{}",
                            unmatched(&store, &graph)
                        ))
                    }
                    Err(e) => return Outcome::Fail(e),
                }
            }
        };
        if !ok {
            return Outcome::Fail(format!("{kind:?} does not hold"));
        }
    }
    Outcome::Pass
}

/// Approved cases of the RL profile in the vendored file.
const SELECTED: usize = 70;

#[test]
fn w3c_owl2_rl_manifest_selection() {
    let cases = cases();
    assert_eq!(cases.len(), SELECTED, "approved RL-profile cases");
    for (id, why) in KNOWN_FAILURES {
        assert!(!why.is_empty(), "{id} needs a reason");
        assert!(
            cases.iter().any(|c| &c.id == id),
            "{id} is not a selected case"
        );
    }
}

#[test]
fn w3c_owl2_rl_suite() {
    let mut passed = 0usize;
    let mut unexpected = Vec::new();
    let mut fixed = Vec::new();
    for c in cases() {
        let known = KNOWN_FAILURES.iter().any(|(id, _)| *id == c.id);
        match run(&c) {
            Outcome::Pass if known => fixed.push(c.id.clone()),
            Outcome::Pass => passed += 1,
            Outcome::Fail(why) if !known => unexpected.push(format!("{}: {why}", c.id)),
            Outcome::Fail(_) | Outcome::Skip => {}
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
