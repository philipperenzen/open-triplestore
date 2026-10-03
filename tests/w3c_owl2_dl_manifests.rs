//! Runner for the approved W3C OWL 2 test cases (OWL 2 Conformance §2.2–2.3),
//! vendored unmodified as `tests/fixtures/w3c-owl2/approved/all.rdf` (see
//! LICENSE.md and PROVENANCE.md there).
//!
//! It picks the approved cases whose species is OWL 2 DL and whose semantics
//! is the Direct Semantics, and runs every one through
//! `POST /api/reasoning/check` against the reasoner sidecar
//! (`OTS_DL_BACKEND=sidecar`), the path a user's check takes: the server's own
//! RDF → OWL 2 mapping and profile check first, then the sidecar.
//!
//! - `test:ConsistencyTest` / `test:InconsistencyTest`: task `consistency`
//!   must answer `true` / `false`;
//! - `test:PositiveEntailmentTest` / `test:NegativeEntailmentTest`: task
//!   `entailment` with the conclusion / non-conclusion must answer `true` /
//!   `false`.
//!
//! A case with several types passes when every one of them does. The premise
//! and conclusion are the cases' RDF/XML documents, converted to N-Triples in
//! memory (the endpoint reads Turtle). The server never follows `owl:imports`,
//! so the runner adds the case's `test:importedOntology` documents to the
//! premise itself: the imports closure. Cases with no RDF/XML premise (their
//! normative syntax is functional or OWL/XML only) cannot be posted to an RDF
//! endpoint and are skipped by the runner.
//!
//! The suite's licence allows no performance claims for a partial or altered
//! run, so no score is published (`scripts/conformance_table.py`,
//! docs/conformance/owl2-dl.md). Gap policy, as in
//! `w3c_sparql11_manifests.rs`: every case NOT in `KNOWN_FAILURES` must pass,
//! every listed case must still fail, and a pass floor guards against a
//! loader regression turning passes into skips.
//!
//! The run needs a sidecar: it is skipped unless `OTS_TEST_REASONER_URL` is set
//! (`OTS_TEST_REASONER_TOKEN`: its token). `w3c_owl2_dl_manifest_selection`
//! needs none and runs everywhere. `OTS_TEST_W3C_OWL2_DUMP=<dir>` writes each
//! premise, as N-Triples, to `<dir>/<identifier>.nt`.

#![cfg(feature = "owl2-dl")]

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::reasoning::dl_config::{DlBackendKind, DlConfig};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{Graph, NamedNode, NamedOrBlankNode, Term, Triple};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt as _;

const MANIFEST: &str = "tests/fixtures/w3c-owl2/approved/all.rdf";
const T: &str = "http://www.w3.org/2007/OWL/testOntology#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// Approved ∩ species DL ∩ Direct Semantics in the vendored file.
const SELECTED: usize = 266;

/// `(test:identifier, why)`: cases that fail today. See
/// docs/conformance/owl2-dl.md.
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("New-Feature-Rational-002", "the premise's RDF list ends in `rdf:` (the namespace IRI) instead of `rdf:nil`, so it is not a well-formed OWL 2 document and the server refuses it as outside OWL 2 DL (422)"),
    ("New-Feature-Rational-003", "as New-Feature-Rational-002: a list ending in `rdf:`"),
    ("WebOnt-I5.26-001", "the premise holds a class expression that no axiom uses; the OWL 2 RDF mapping leaves its triples unparsed (Mapping to RDF Graphs §3.2.5: the graph must be empty at the end), so the server refuses the input as outside OWL 2 DL (422)"),
    ("WebOnt-description-logic-208", "HermiT does not decide consistency within the runner's 60 s limit (504, result unknown)"),
    ("WebOnt-description-logic-209", "as WebOnt-description-logic-208: no answer within 60 s"),
];

/// Pass floor: a loader or mapping regression that turns passes into skips
/// or errors must not go unnoticed. It sits a little below the current count.
const PASS_FLOOR: usize = 235;

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
fn cases() -> Vec<Case> {
    let raw = std::fs::read_to_string(MANIFEST).expect("the vendored OWL 2 test manifest");
    let xml = double_quoted_entities(&raw);
    let g: Graph = RdfParser::from_format(RdfFormat::RdfXml)
        .for_slice(xml.as_bytes())
        .map(|q| q.map(Triple::from))
        .collect::<Result<_, _>>()
        .expect("the OWL 2 test manifest parses as RDF/XML");
    let ty = iri(RDF_TYPE);
    let mut out = Vec::new();
    let subjects: Vec<NamedOrBlankNode> = g
        .subjects_for_predicate_object(&ty, &t("TestCase"))
        .map(|s| s.into_owned())
        .collect();
    for s in subjects {
        if !has(&g, &s, &t("status"), &t("Approved"))
            || !has(&g, &s, &t("species"), &t("DL"))
            || !has(&g, &s, &t("semantics"), &t("DIRECT"))
        {
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

/// An RDF/XML document as N-Triples (which is Turtle).
fn ntriples(rdfxml: &str, id: &str) -> Result<String, String> {
    let base = format!("http://owl.semanticweb.org/id/{id}");
    let mut s = String::new();
    let rdfxml = double_quoted_entities(rdfxml);
    for q in RdfParser::from_format(RdfFormat::RdfXml)
        .with_base_iri(&base)
        .map_err(|e| e.to_string())?
        .for_slice(rdfxml.as_bytes())
    {
        let t = Triple::from(q.map_err(|e| format!("RDF/XML: {e}"))?);
        s.push_str(&t.to_string());
        s.push_str(" .\n");
    }
    Ok(s)
}

fn sidecar() -> Option<DlConfig> {
    let Ok(url) = std::env::var("OTS_TEST_REASONER_URL") else {
        assert!(
            std::env::var_os("OTS_TEST_LIVE_REQUIRED").is_none(),
            "OTS_TEST_LIVE_REQUIRED is set but OTS_TEST_REASONER_URL is not"
        );
        return None;
    };
    let mut c = DlConfig::default().with_backend(DlBackendKind::Sidecar);
    c.sidecar_url = Some(url);
    c.sidecar_token = std::env::var("OTS_TEST_REASONER_TOKEN").ok();
    // Every case but the known slow ones answers in well under a second.
    c.timeout = Duration::from_secs(60);
    Some(c)
}

enum Outcome {
    Pass,
    Fail(String),
    Skip(&'static str),
}

async fn check(app: &axum::Router, token: &str, body: Value) -> Result<String, String> {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/reasoning/check")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let st = resp.status();
    let v = body_json(resp.into_body()).await;
    if st != StatusCode::OK {
        let mut why = format!("HTTP {}: {}", st.as_u16(), v);
        why.truncate(400);
        return Err(why);
    }
    v["result"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("no result in {v}"))
}

async fn run(app: &axum::Router, token: &str, c: &Case) -> Outcome {
    let Some(premise) = &c.premise else {
        return Outcome::Skip("no RDF/XML premise");
    };
    if c.kinds.is_empty() {
        return Outcome::Skip("no reasoning test type");
    }
    let mut premise_nt = match ntriples(premise, &c.id) {
        Ok(p) => p,
        Err(e) => return Outcome::Fail(format!("premise: {e}")),
    };
    for (i, imported) in c.imports.iter().enumerate() {
        match ntriples(imported, &format!("{}-import{i}", c.id)) {
            Ok(p) => premise_nt.push_str(&p),
            Err(e) => return Outcome::Fail(format!("imported ontology: {e}")),
        }
    }
    // Debugging aid: OTS_TEST_W3C_OWL2_DUMP=<dir> keeps each premise as sent.
    if let Some(dir) = std::env::var_os("OTS_TEST_W3C_OWL2_DUMP") {
        let _ = std::fs::write(
            std::path::Path::new(&dir).join(format!("{}.nt", c.id)),
            &premise_nt,
        );
    }
    for k in &c.kinds {
        let (task, conclusion, expected) = match k {
            Kind::Consistency => ("consistency", None, "true"),
            Kind::Inconsistency => ("consistency", None, "false"),
            Kind::PositiveEntailment => ("entailment", c.conclusion.as_ref(), "true"),
            Kind::NegativeEntailment => ("entailment", c.non_conclusion.as_ref(), "false"),
        };
        let mut body = json!({ "task": task, "premise": premise_nt });
        if task == "entailment" {
            let Some(conclusion) = conclusion else {
                return Outcome::Skip("no RDF/XML conclusion");
            };
            match ntriples(conclusion, &format!("{}-conclusion", c.id)) {
                Ok(nt) => body["conclusion"] = json!(nt),
                Err(e) => return Outcome::Fail(format!("conclusion: {e}")),
            }
        }
        match check(app, token, body).await {
            Ok(r) if r == expected => {}
            Ok(r) => return Outcome::Fail(format!("{k:?}: expected {expected}, got {r}")),
            Err(e) => return Outcome::Fail(format!("{k:?}: {e}")),
        }
    }
    Outcome::Pass
}

/// The selection itself, without a sidecar: the vendored file parses, the
/// filter finds the approved DL / Direct Semantics cases, and every
/// known-failure id names one of them.
#[test]
fn w3c_owl2_dl_manifest_selection() {
    let cs = cases();
    assert_eq!(
        cs.len(),
        SELECTED,
        "approved OWL 2 DL / Direct Semantics cases"
    );
    assert!(
        cs.iter().all(|c| !c.kinds.is_empty()),
        "every selected case is a reasoning test"
    );
    for (id, _) in KNOWN_FAILURES {
        assert!(
            cs.iter().any(|c| c.id == *id),
            "KNOWN_FAILURES names unknown case {id}"
        );
    }
    let readable = cs
        .iter()
        .filter_map(|c| c.premise.as_ref().map(|p| (c, p)))
        .filter(|(c, p)| ntriples(p, &c.id).is_ok())
        .count();
    assert_eq!(
        readable,
        cs.iter().filter(|c| c.premise.is_some()).count(),
        "every RDF/XML premise converts to N-Triples"
    );
}

#[tokio::test]
async fn w3c_owl2_dl_suite_against_the_sidecar() {
    let Some(cfg) = sidecar() else { return };
    let (mut state, token) = admin_state();
    state.dl = Arc::new(cfg);
    let app = test_app(state);

    let (mut pass, mut skips) = (0usize, Vec::new());
    let (mut unexpected, mut fixed, mut listed_seen) = (Vec::new(), Vec::new(), 0usize);
    for c in cases() {
        let known = KNOWN_FAILURES.iter().find(|(k, _)| *k == c.id);
        if known.is_some() {
            listed_seen += 1;
        }
        match (run(&app, &token, &c).await, known) {
            (Outcome::Pass, None) => pass += 1,
            (Outcome::Pass, Some(_)) => fixed.push(c.id.clone()),
            (Outcome::Fail(_), Some(_)) => {}
            (Outcome::Fail(why), None) => unexpected.push(format!("{}: {why}", c.id)),
            (Outcome::Skip(why), _) => skips.push(format!("{} ({why})", c.id)),
        }
    }
    for s in &skips {
        println!("  SKIP {s}");
    }
    println!(
        "W3C OWL 2 DL: {pass} passed, {listed_seen} known-fail, {} skipped",
        skips.len()
    );
    assert_eq!(
        listed_seen,
        KNOWN_FAILURES.len(),
        "KNOWN_FAILURES lists {} cases but {listed_seen} were selected (stale ids?)",
        KNOWN_FAILURES.len()
    );
    assert!(
        unexpected.is_empty(),
        "{} cases failing that are not in KNOWN_FAILURES:\n  {}",
        unexpected.len(),
        unexpected.join("\n  ")
    );
    assert!(
        fixed.is_empty(),
        "KNOWN_FAILURES cases now pass — remove them to ratchet forward:\n  {}",
        fixed.join("\n  ")
    );
    assert!(
        pass >= PASS_FLOOR,
        "only {pass} W3C OWL 2 DL cases passed (floor {PASS_FLOOR}); skips: {}",
        skips.join(", ")
    );
}
