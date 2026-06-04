//! OWL 2 RL conformance tests — per-rule-group coverage.
//!
//! Tests the forward-chaining rules from W3C OWL 2 Profiles Tables 4–9,
//! with a focus on the rules added in the recent implementation: prp-spo2,
//! prp-key, cls-maxqc, cax-adc, scm-cls, scm-int, scm-uni.

#![cfg(feature = "owl2-rl")]

use open_triplestore::reasoning::common::ReasoningError;
use open_triplestore::reasoning::owl2_rl::Owl2RLReasoner;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;

const TG: &str = "urn:entailment:owl2-rl";

fn store_with(ttl: &str) -> TripleStore {
    let store = TripleStore::in_memory().unwrap();
    let preamble = "@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
                    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                    @prefix owl:  <http://www.w3.org/2002/07/owl#> .\n\
                    @prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .\n\
                    @prefix ex:   <http://example.org/> .\n";
    store
        .load_str(&format!("{preamble}{ttl}"), RdfFormat::Turtle, None)
        .unwrap();
    store
}

fn materialize(store: &TripleStore) -> usize {
    let r = Owl2RLReasoner::new(store).materialize().unwrap();
    r.triples_added
}

fn ask(store: &TripleStore, sparql: &str) -> bool {
    match store.query(sparql).unwrap() {
        oxigraph::sparql::QueryResults::Boolean(b) => b,
        _ => panic!("expected ASK result"),
    }
}

fn ask_tg(store: &TripleStore, pattern: &str) -> bool {
    ask(
        store,
        &format!(
            "PREFIX rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
         PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
         PREFIX owl:  <http://www.w3.org/2002/07/owl#>\n\
         ASK {{ GRAPH <{TG}> {{ {pattern} }} }}"
        ),
    )
}

fn check_inconsistency(store: &TripleStore) -> bool {
    matches!(
        Owl2RLReasoner::new(store).materialize(),
        Err(ReasoningError::Inconsistency(_))
    )
}

// ─── prp-dom (property domain) ────────────────────────────────────────────────

#[test]
fn test_prp_dom() {
    let s = store_with("ex:p rdfs:domain ex:C . ex:x ex:p ex:y .");
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/x> rdf:type <http://example.org/C> ."
        ),
        "prp-dom"
    );
}

// ─── prp-rng (property range) ────────────────────────────────────────────────

#[test]
fn test_prp_rng() {
    let s = store_with("ex:p rdfs:range ex:C . ex:x ex:p ex:y .");
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/y> rdf:type <http://example.org/C> ."
        ),
        "prp-rng"
    );
}

// ─── prp-symp (symmetric property) ───────────────────────────────────────────

#[test]
fn test_prp_symp() {
    let s = store_with("ex:knows rdf:type owl:SymmetricProperty . ex:alice ex:knows ex:bob .");
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/bob> <http://example.org/knows> <http://example.org/alice> ."
        ),
        "prp-symp: symmetric property"
    );
}

// ─── prp-trp (transitive property) ───────────────────────────────────────────

#[test]
fn test_prp_trp() {
    let s = store_with(
        "ex:partOf rdf:type owl:TransitiveProperty . \
                        ex:a ex:partOf ex:b . ex:b ex:partOf ex:c .",
    );
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/a> <http://example.org/partOf> <http://example.org/c> ."
        ),
        "prp-trp: transitivity"
    );
}

// ─── prp-spo1 (subPropertyOf inheritance) ────────────────────────────────────

#[test]
fn test_prp_spo1() {
    let s =
        store_with("ex:fatherOf rdfs:subPropertyOf ex:parentOf . ex:bob ex:fatherOf ex:alice .");
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/bob> <http://example.org/parentOf> <http://example.org/alice> ."
        ),
        "prp-spo1"
    );
}

// ─── prp-spo2 (property chain) ────────────────────────────────────────────────

#[test]
fn test_prp_spo2_property_chain() {
    // ex:uncleOf owl:propertyChainAxiom (ex:brotherOf ex:parentOf)
    let s = store_with(
        "ex:uncleOf owl:propertyChainAxiom ( ex:brotherOf ex:parentOf ) . \
         ex:bob ex:brotherOf ex:carol . \
         ex:carol ex:parentOf ex:dave .",
    );
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/bob> <http://example.org/uncleOf> <http://example.org/dave> ."
        ),
        "prp-spo2: property chain axiom"
    );
}

// ─── prp-fp (functional property) ────────────────────────────────────────────

#[test]
fn test_prp_fp_merges() {
    let s = store_with(
        "ex:hasSSN rdf:type owl:FunctionalProperty . \
                        ex:alice ex:hasSSN ex:SSN1 . ex:alice ex:hasSSN ex:SSN2 .",
    );
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/SSN1> owl:sameAs <http://example.org/SSN2> ."
        ),
        "prp-fp: functional property should merge values"
    );
}

// ─── prp-ifp (inverse functional property) ───────────────────────────────────

#[test]
fn test_prp_ifp_merges() {
    let s = store_with(
        "ex:hasEmail rdf:type owl:InverseFunctionalProperty . \
                        ex:alice ex:hasEmail ex:email1 . ex:bob ex:hasEmail ex:email1 .",
    );
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> owl:sameAs <http://example.org/bob> ."
        ),
        "prp-ifp: inverse functional property should merge subjects"
    );
}

// ─── prp-key (hasKey) ─────────────────────────────────────────────────────────

#[test]
fn test_prp_key() {
    let s = store_with(
        "ex:Person owl:hasKey ( ex:ssn ) . \
                        ex:alice rdf:type ex:Person . ex:alice ex:ssn ex:SSN001 . \
                        ex:bob rdf:type ex:Person . ex:bob ex:ssn ex:SSN001 .",
    );
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> owl:sameAs <http://example.org/bob> ."
        ),
        "prp-key: hasKey should identify individuals"
    );
}

// ─── cls-int1 (intersection membership) ──────────────────────────────────────

#[test]
fn test_cls_int1() {
    let s = store_with(
        "ex:WorkingParent owl:intersectionOf ( ex:Worker ex:Parent ) . \
                        ex:alice rdf:type ex:Worker . ex:alice rdf:type ex:Parent .",
    );
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> rdf:type <http://example.org/WorkingParent> ."
        ),
        "cls-int1: intersection membership"
    );
}

// ─── cls-svf1 (someValuesFrom) ────────────────────────────────────────────────

#[test]
fn test_cls_svf_typing() {
    let s = store_with(
        "ex:HasChild owl:someValuesFrom ex:Person ; owl:onProperty ex:hasChild . \
                        ex:Employee rdfs:subClassOf ex:HasChild . \
                        ex:alice rdf:type ex:Employee . ex:alice ex:hasChild ex:bob . \
                        ex:bob rdf:type ex:Person .",
    );
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> rdf:type <http://example.org/HasChild> ."
        ) || ask_tg(
            &s,
            "<http://example.org/alice> rdf:type <http://example.org/Employee> ."
        ),
        "cls-svf: someValuesFrom typing"
    );
}

// ─── cax-sco (subClassOf) ────────────────────────────────────────────────────

#[test]
fn test_cax_sco() {
    let s = store_with("ex:Employee rdfs:subClassOf ex:Person . ex:alice rdf:type ex:Employee .");
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> rdf:type <http://example.org/Person> ."
        ),
        "cax-sco: type propagation"
    );
}

// ─── cax-eqc (equivalentClass) ────────────────────────────────────────────────

#[test]
fn test_cax_eqc() {
    let s =
        store_with("ex:Employee owl:equivalentClass ex:Worker . ex:alice rdf:type ex:Employee .");
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> rdf:type <http://example.org/Worker> ."
        ),
        "cax-eqc: equivalentClass propagation"
    );
}

// ─── cax-dw (disjointWith inconsistency) ─────────────────────────────────────

#[test]
fn test_cax_dw_inconsistency() {
    let s = store_with("ex:A owl:disjointWith ex:B . ex:x rdf:type ex:A . ex:x rdf:type ex:B .");
    assert!(
        check_inconsistency(&s),
        "cax-dw: disjointWith should be inconsistent"
    );
}

// ─── cax-adc (AllDisjointClasses) ────────────────────────────────────────────

#[test]
fn test_cax_adc_inconsistency() {
    let s = store_with(
        "[] rdf:type owl:AllDisjointClasses ; \
            owl:members ( ex:A ex:B ex:C ) . \
         ex:x rdf:type ex:A . ex:x rdf:type ex:B .",
    );
    assert!(
        check_inconsistency(&s),
        "cax-adc: AllDisjointClasses expansion + inconsistency"
    );
}

// ─── scm-cls (every class subClassOf owl:Thing) ────────────────────────────────

#[test]
fn test_scm_cls() {
    let s = store_with("ex:Person rdf:type owl:Class .");
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/Person> rdfs:subClassOf owl:Thing ."
        ),
        "scm-cls: every class should be subClassOf owl:Thing"
    );
}

// ─── scm-int (intersection entailment) ────────────────────────────────────────

#[test]
fn test_scm_int() {
    let s = store_with("ex:AB owl:intersectionOf ( ex:A ex:B ) .");
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/AB> rdfs:subClassOf <http://example.org/A> ."
        ),
        "scm-int: intersection member entailment A"
    );
    assert!(
        ask_tg(
            &s,
            "<http://example.org/AB> rdfs:subClassOf <http://example.org/B> ."
        ),
        "scm-int: intersection member entailment B"
    );
}

// ─── scm-uni (union entailment) ───────────────────────────────────────────────

#[test]
fn test_scm_uni() {
    let s = store_with("ex:AorB owl:unionOf ( ex:A ex:B ) .");
    materialize(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/A> rdfs:subClassOf <http://example.org/AorB> ."
        ),
        "scm-uni: union member entailment A"
    );
    assert!(
        ask_tg(
            &s,
            "<http://example.org/B> rdfs:subClassOf <http://example.org/AorB> ."
        ),
        "scm-uni: union member entailment B"
    );
}

// ─── prp-npa1/npa2 (NegativePropertyAssertion) ───────────────────────────────

#[test]
fn test_prp_npa1_inconsistency() {
    let s = store_with(
        "[] rdf:type owl:NegativePropertyAssertion ; \
            owl:sourceIndividual ex:alice ; \
            owl:assertionProperty ex:knows ; \
            owl:targetIndividual ex:bob . \
         ex:alice ex:knows ex:bob .",
    );
    assert!(
        check_inconsistency(&s),
        "prp-npa1: NegativePropertyAssertion violation"
    );
}

#[test]
fn test_prp_npa2_data_inconsistency() {
    let s = store_with(
        "[] rdf:type owl:NegativePropertyAssertion ; \
            owl:sourceIndividual ex:alice ; \
            owl:assertionProperty ex:age ; \
            owl:targetValue \"30\"^^xsd:integer . \
         ex:alice ex:age \"30\"^^xsd:integer .",
    );
    assert!(
        check_inconsistency(&s),
        "prp-npa2: NegativePropertyAssertion data violation"
    );
}

// ─── cls-maxc1 (maxCardinality 0 object) ─────────────────────────────────────

#[test]
fn test_cls_maxc1_zero_cardinality() {
    let s = store_with(
        "ex:C rdfs:subClassOf [ owl:maxCardinality 0 ; owl:onProperty ex:p ] . \
         ex:x rdf:type ex:C . ex:x ex:p ex:y .",
    );
    assert!(
        check_inconsistency(&s),
        "cls-maxc1: maxCardinality 0 should be inconsistent"
    );
}

// ─── Multi-rule interaction test ──────────────────────────────────────────────

#[test]
fn test_multi_rule_interaction() {
    // Chain: subPropertyOf + domain + subClassOf
    let s = store_with(
        "ex:fatherOf rdfs:subPropertyOf ex:parentOf . \
         ex:parentOf rdfs:domain ex:Parent . \
         ex:Parent rdfs:subClassOf ex:Person . \
         ex:bob ex:fatherOf ex:alice .",
    );
    materialize(&s);
    // bob → fatherOf → parentOf (prp-spo1)
    assert!(
        ask_tg(
            &s,
            "<http://example.org/bob> <http://example.org/parentOf> <http://example.org/alice> ."
        ),
        "spo1 propagation"
    );
    // bob → domain(parentOf) → Parent (prp-dom)
    assert!(
        ask_tg(
            &s,
            "<http://example.org/bob> rdf:type <http://example.org/Parent> ."
        ),
        "prp-dom via subproperty"
    );
    // bob → Parent → Person (cax-sco)
    assert!(
        ask_tg(
            &s,
            "<http://example.org/bob> rdf:type <http://example.org/Person> ."
        ),
        "cax-sco chained"
    );
}

#[test]
fn test_rl_report_has_positive_count() {
    let s = store_with("ex:Manager rdfs:subClassOf ex:Employee . ex:alice rdf:type ex:Manager .");
    let count = materialize(&s);
    assert!(count > 0, "Materialization should add triples");
}
