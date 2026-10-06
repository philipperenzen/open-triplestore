//! Runner for the SPARQL 1.1 Entailment Regimes section of the W3C SPARQL 1.1
//! test suite (`sparql/sparql11/entailment` of w3c/rdf-tests), vendored
//! unmodified in `tests/fixtures/w3c-sparql11/entailment/` (see LICENSE.md
//! and PROVENANCE.md in `tests/fixtures/w3c-sparql11/`).
//!
//! Each case lists the regimes its expected result holds under. The runner
//! materializes the case's data with the RDFS engine when the list names
//! `RDFS`, `RDF` or `D`, else with the OWL 2 RL engine when it names
//! `OWL-RDF-Based`; cases for the OWL Direct Semantics or RIF only are
//! skipped. The query then runs over the data and the derived triples, and
//! its solutions are compared with the expected ones as multisets, blank
//! nodes compared as "some blank node" (a weaker check than the isomorphism
//! the SPARQL runner uses). Materialization is not the regimes' answer
//! semantics (they restrict answers to the queried graph's vocabulary, for
//! example), so some cases differ; they are listed in `KNOWN_FAILURES`.
//!
//! No score is published (W3C test-suite policy; `scripts/conformance_table.py`).
//! Gap policy, as in `w3c_sparql11_manifests.rs`: every case NOT in
//! `KNOWN_FAILURES` must pass, every listed case must still fail, and a pass
//! floor guards against a loader regression turning passes into skips.

#![cfg(all(feature = "rdfs-entailment", feature = "owl2-rl"))]

use open_triplestore::reasoning::owl2_rl::Owl2RLReasoner;
use open_triplestore::reasoning::rdfs::RdfsMaterializer;
use open_triplestore::store::TripleStore;
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{Graph, NamedNode, NamedOrBlankNode, Term, Triple};
use oxigraph::sparql::results::{
    QueryResultsFormat, QueryResultsParser, ReaderQueryResultsParserOutput,
};
use oxigraph::sparql::QueryResults;

const ROOT: &str = "tests/fixtures/w3c-sparql11/entailment/";
const BASE: &str = "https://w3c.github.io/rdf-tests/sparql/sparql11/entailment/";
const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";
const QT: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-query#";
const SD: &str = "http://www.w3.org/ns/sparql-service-description#";
const ENT: &str = "http://www.w3.org/ns/entailment/";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// `(entry IRI fragment, why)`: cases that fail today. See docs/conformance/entailment.md.
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("sparqldl-10", "the expected answers need OWL reasoning beyond the RL/RDF rules (RL is a partial axiomatization of the RDF-Based Semantics)"),
    ("sparqldl-11", "as sparqldl-10"),
    ("sparqldl-12", "an answer binds a blank-node class (a restriction); the regime answers only with terms that name things in the queried graph"),
];

/// Pass floor, a little below the current count.
const PASS_FLOOR: usize = 37;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Engine {
    Rdfs,
    OwlRl,
}

#[derive(Debug)]
struct Case {
    name: String,
    query: String,
    data: Vec<String>,
    result: String,
    engine: Option<Engine>,
}

fn nn(ns: &str, local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{ns}{local}"))
}

fn local(iri: &str) -> &str {
    iri.strip_prefix(BASE).unwrap_or(iri)
}

fn read(iri: &str) -> Result<String, String> {
    let path = format!("{ROOT}{}", local(iri));
    std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))
}

fn parse(iri: &str) -> Result<Vec<Triple>, String> {
    RdfParser::from_format(RdfFormat::Turtle)
        .with_base_iri(format!("{BASE}{}", local(iri)))
        .map_err(|e| e.to_string())?
        .for_slice(read(iri)?.as_bytes())
        .map(|q| {
            q.map(Triple::from)
                .map_err(|e| format!("{}: {e}", local(iri)))
        })
        .collect()
}

fn objects(g: &Graph, s: &NamedOrBlankNode, p: &NamedNode) -> Vec<Term> {
    g.objects_for_subject_predicate(s, p)
        .map(|o| o.into_owned())
        .collect()
}

fn node(t: &Term) -> Option<NamedOrBlankNode> {
    match t {
        Term::NamedNode(n) => Some(n.clone().into()),
        Term::BlankNode(b) => Some(b.clone().into()),
        _ => None,
    }
}

/// A term, or the members of the list it heads.
fn members(g: &Graph, t: Term) -> Vec<Term> {
    let mut out = Vec::new();
    let mut cur = t;
    loop {
        let Some(n) = node(&cur) else {
            return vec![cur];
        };
        let first = objects(g, &n, &nn(RDF, "first"));
        let Some(f) = first.into_iter().next() else {
            return if out.is_empty() && cur.to_string() != format!("<{RDF}nil>") {
                vec![cur]
            } else {
                out
            };
        };
        out.push(f);
        match objects(g, &n, &nn(RDF, "rest")).into_iter().next() {
            Some(r) => cur = r,
            None => return out,
        }
    }
}

fn cases() -> Vec<Case> {
    let g: Graph = parse(&format!("{BASE}manifest.ttl"))
        .expect("the vendored entailment manifest")
        .into_iter()
        .collect();
    let ty = nn(RDF, "type");
    let tests: Vec<NamedOrBlankNode> = g
        .subjects_for_predicate_object(&ty, &nn(MF, "QueryEvaluationTest"))
        .map(|s| s.into_owned())
        .collect();
    let iri_of = |t: &Term| match t {
        Term::NamedNode(n) => Some(n.as_str().to_string()),
        _ => None,
    };
    let mut out = Vec::new();
    for s in tests {
        // The entry's IRI fragment (`bind01`): short and unique.
        let name = match &s {
            NamedOrBlankNode::NamedNode(n) => {
                n.as_str().rsplit('#').next().unwrap_or("").to_string()
            }
            NamedOrBlankNode::BlankNode(_) => continue,
        };
        let Some(action) = objects(&g, &s, &nn(MF, "action")).first().and_then(node) else {
            continue;
        };
        let one = |p: NamedNode| objects(&g, &action, &p).first().and_then(iri_of);
        let regimes: Vec<String> = objects(&g, &action, &nn(SD, "entailmentRegime"))
            .into_iter()
            .flat_map(|t| members(&g, t))
            .filter_map(|t| iri_of(&t))
            .collect();
        let has = |r: &str| regimes.iter().any(|x| x == &format!("{ENT}{r}"));
        let engine = if has("RDFS") || has("RDF") || has("D") {
            Some(Engine::Rdfs)
        } else if has("OWL-RDF-Based") {
            Some(Engine::OwlRl)
        } else {
            None
        };
        out.push(Case {
            name,
            query: one(nn(QT, "query")).unwrap_or_default(),
            data: objects(&g, &action, &nn(QT, "data"))
                .iter()
                .filter_map(iri_of)
                .collect(),
            result: objects(&g, &s, &nn(MF, "result"))
                .first()
                .and_then(iri_of)
                .unwrap_or_default(),
            engine,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A solution as sorted `var=term` strings, blank nodes as `_:`.
fn row(pairs: impl Iterator<Item = (String, Term)>) -> Vec<String> {
    let mut v: Vec<String> = pairs
        .map(|(var, t)| match t {
            Term::BlankNode(_) => format!("{var}=_:"),
            t => format!("{var}={t}"),
        })
        .collect();
    v.sort();
    v
}

enum Answer {
    Bool(bool),
    Rows(Vec<Vec<String>>),
}

fn expected(iri: &str) -> Result<Answer, String> {
    let bytes = read(iri)?.into_bytes();
    match QueryResultsParser::from_format(QueryResultsFormat::Xml)
        .for_reader(bytes.as_slice())
        .map_err(|e| format!("{}: {e}", local(iri)))?
    {
        ReaderQueryResultsParserOutput::Boolean(b) => Ok(Answer::Bool(b)),
        ReaderQueryResultsParserOutput::Solutions(it) => {
            let mut rows = Vec::new();
            for sol in it {
                let sol = sol.map_err(|e| e.to_string())?;
                rows.push(row(sol
                    .iter()
                    .map(|(v, t)| (v.as_str().to_string(), t.clone()))));
            }
            rows.sort();
            Ok(Answer::Rows(rows))
        }
    }
}

const TG: &str = "urn:entailment:test";

fn run(c: &Case) -> Result<(), String> {
    let Some(engine) = c.engine else {
        return Err("skip".into());
    };
    let store = TripleStore::in_memory().map_err(|e| e.to_string())?;
    for d in &c.data {
        let nt: String = parse(d)?.iter().map(|t| format!("{t} .\n")).collect();
        store
            .load_str(&nt, RdfFormat::NTriples, None)
            .map_err(|e| e.to_string())?;
    }
    match engine {
        Engine::Rdfs => RdfsMaterializer::with_target(&store, TG).materialize(),
        Engine::OwlRl => Owl2RLReasoner::new(&store).with_target(TG).materialize(),
    }
    .map_err(|e| format!("materialize: {e}"))?;
    let query = read(&c.query)?;
    let got = match store
        .query_over(&query, &[TG.to_string()])
        .map_err(|e| format!("query: {e}"))?
    {
        QueryResults::Boolean(b) => Answer::Bool(b),
        QueryResults::Solutions(sols) => {
            let mut rows = Vec::new();
            for sol in sols {
                let sol = sol.map_err(|e| e.to_string())?;
                rows.push(row(sol
                    .iter()
                    .map(|(v, t)| (v.as_str().to_string(), t.clone()))));
            }
            rows.sort();
            Answer::Rows(rows)
        }
        QueryResults::Graph(_) => return Err("graph results are not compared".into()),
    };
    match (expected(&c.result)?, got) {
        (Answer::Bool(a), Answer::Bool(b)) if a == b => Ok(()),
        (Answer::Rows(a), Answer::Rows(b)) if a == b => Ok(()),
        (Answer::Rows(a), Answer::Rows(b)) => {
            let mut detail = String::new();
            // With OTS_TEST_W3C_ENTAILMENT_EXPLAIN set: up to five rows each way.
            if std::env::var_os("OTS_TEST_W3C_ENTAILMENT_EXPLAIN").is_some() {
                let only = |x: &[Vec<String>], y: &[Vec<String>]| -> Vec<String> {
                    x.iter()
                        .filter(|r| !y.contains(r))
                        .take(5)
                        .map(|r| r.join(" "))
                        .collect()
                };
                detail = format!("; missing {:?}; extra {:?}", only(&a, &b), only(&b, &a));
            }
            Err(format!(
                "{} expected rows, {} got ({engine:?}){detail}",
                a.len(),
                b.len()
            ))
        }
        _ => Err(format!("the answer differs ({engine:?})")),
    }
}

/// Entries in the vendored manifest.
const ENTRIES: usize = 70;

#[test]
fn w3c_sparql11_entailment_manifest_entries() {
    let cases = cases();
    assert_eq!(cases.len(), ENTRIES);
    for (id, why) in KNOWN_FAILURES {
        assert!(!why.is_empty(), "{id} needs a reason");
        assert!(cases.iter().any(|c| &c.name == id), "{id} is not an entry");
    }
}

#[test]
fn w3c_sparql11_entailment_suite() {
    let mut passed = 0usize;
    let (mut unexpected, mut fixed) = (Vec::new(), Vec::new());
    for c in cases() {
        if c.engine.is_none() {
            continue;
        }
        let known = KNOWN_FAILURES.iter().any(|(id, _)| *id == c.name);
        match run(&c) {
            Ok(()) if known => fixed.push(c.name.clone()),
            Ok(()) => passed += 1,
            Err(why) if !known => unexpected.push(format!("{}: {why}", c.name)),
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
