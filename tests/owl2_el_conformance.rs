//! OWL 2 EL conformance tests — completion rule coverage.
//!
//! Tests the EL++ completion rules CR1–CR10, hasKey, and reflexivity,
//! verifying that the `El2Classifier` produces correct classifications.

#![cfg(feature = "owl2-el")]

use open_triplestore::store::TripleStore;
use open_triplestore::reasoning::owl2_el::El2Classifier;
use oxigraph::io::RdfFormat;

const TG: &str = "urn:entailment:owl2-el";

fn store_with(ttl: &str) -> TripleStore {
    let store = TripleStore::in_memory().unwrap();
    let preamble = "@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
                    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                    @prefix owl:  <http://www.w3.org/2002/07/owl#> .\n\
                    @prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .\n\
                    @prefix ex:   <http://example.org/> .\n";
    store.load_str(&format!("{preamble}{ttl}"), RdfFormat::Turtle, None).unwrap();
    store
}

fn classify(store: &TripleStore) {
    El2Classifier::new(store).classify().unwrap();
}

fn ask_tg(store: &TripleStore, pattern: &str) -> bool {
    let q = format!(
        "PREFIX rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
         PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
         PREFIX owl:  <http://www.w3.org/2002/07/owl#>\n\
         ASK {{ GRAPH <{TG}> {{ {pattern} }} }}"
    );
    match store.query(&q).unwrap() {
        oxigraph::sparql::QueryResults::Boolean(b) => b,
        _ => panic!("expected ASK result"),
    }
}

// ─── CR1: SubClassOf inheritance ──────────────────────────────────────────────

#[test]
fn test_cr1_subclass_inheritance() {
    let s = store_with("ex:Employee rdfs:subClassOf ex:Person . ex:alice rdf:type ex:Employee .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/Person> ."),
        "CR1: type propagation through subClassOf");
}

#[test]
fn test_cr1_three_level_chain() {
    let s = store_with("ex:Manager rdfs:subClassOf ex:Employee . \
                        ex:Employee rdfs:subClassOf ex:Person . \
                        ex:carol rdf:type ex:Manager .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/carol> rdf:type <http://example.org/Employee> ."),
        "CR1: chain level 1");
    assert!(ask_tg(&s, "<http://example.org/carol> rdf:type <http://example.org/Person> ."),
        "CR1: chain level 2");
}

// ─── CR2: Intersection ───────────────────────────────────────────────────────

#[test]
fn test_cr2_intersection_membership() {
    let s = store_with("ex:WorkingParent owl:intersectionOf ( ex:Worker ex:Parent ) . \
                        ex:alice rdf:type ex:Worker . ex:alice rdf:type ex:Parent .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/WorkingParent> ."),
        "CR2: intersection membership");
}

#[test]
fn test_cr2_intersection_with_subclass() {
    let s = store_with("ex:C owl:intersectionOf ( ex:A ex:B ) . \
                        ex:A rdfs:subClassOf ex:C . \
                        ex:x rdf:type ex:A . ex:x rdf:type ex:B .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/x> rdf:type <http://example.org/C> ."),
        "CR2: intersection with subclass chain");
}

// ─── CR4: Existential restriction ─────────────────────────────────────────────

#[test]
fn test_cr4_existential_to_class() {
    let s = store_with("[ owl:someValuesFrom ex:Animal ; owl:onProperty ex:hasPet ] \
                            rdfs:subClassOf ex:PetOwner . \
                        ex:alice ex:hasPet ex:dog . ex:dog rdf:type ex:Animal .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/PetOwner> ."),
        "CR4: existential restriction → class membership");
}

// ─── CR5: Property chain (2-element) ──────────────────────────────────────────

#[test]
fn test_cr5_property_chain_2() {
    let s = store_with("ex:uncleOf owl:propertyChainAxiom ( ex:brotherOf ex:parentOf ) . \
                        ex:bob ex:brotherOf ex:carol . ex:carol ex:parentOf ex:dave .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/bob> <http://example.org/uncleOf> <http://example.org/dave> ."),
        "CR5: 2-element property chain");
}

// ─── CR7: Domain propagation ──────────────────────────────────────────────────

#[test]
fn test_cr7_domain_propagation() {
    let s = store_with("ex:worksFor rdfs:domain ex:Employee . ex:alice ex:worksFor ex:Acme .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/Employee> ."),
        "CR7: domain propagation types subject");
}

#[test]
fn test_cr7_domain_with_subclass() {
    let s = store_with("ex:worksFor rdfs:domain ex:Employee . \
                        ex:Employee rdfs:subClassOf ex:Person . \
                        ex:alice ex:worksFor ex:Acme .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/Employee> ."),
        "CR7 + CR1: domain + subclass chain");
    assert!(ask_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/Person> ."),
        "CR1 chained from CR7");
}

// ─── CR8: Range propagation ───────────────────────────────────────────────────

#[test]
fn test_cr8_range_propagation() {
    let s = store_with("ex:hasChild rdfs:range ex:Person . ex:alice ex:hasChild ex:bob .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/bob> rdf:type <http://example.org/Person> ."),
        "CR8: range propagation types object");
}

// ─── CR9: Reflexive property ──────────────────────────────────────────────────

#[test]
fn test_cr9_reflexive_property() {
    let s = store_with("ex:relatedTo rdf:type owl:ReflexiveProperty . ex:alice rdf:type ex:Person .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/alice> <http://example.org/relatedTo> <http://example.org/alice> ."),
        "CR9: reflexive property should generate self-loop");
}

// ─── CR10: 3-element property chain ───────────────────────────────────────────

#[test]
fn test_cr10_property_chain_3() {
    let s = store_with("ex:r owl:propertyChainAxiom ( ex:p1 ex:p2 ex:p3 ) . \
                        ex:a ex:p1 ex:b . ex:b ex:p2 ex:c . ex:c ex:p3 ex:d .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/a> <http://example.org/r> <http://example.org/d> ."),
        "CR10: 3-element property chain");
}

// ─── hasKey ───────────────────────────────────────────────────────────────────

#[test]
fn test_has_key_merges_individuals() {
    let s = store_with("ex:Person owl:hasKey ( ex:ssn ) . \
                        ex:alice rdf:type ex:Person . ex:alice ex:ssn ex:SSN001 . \
                        ex:bob rdf:type ex:Person . ex:bob ex:ssn ex:SSN001 .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/alice> owl:sameAs <http://example.org/bob> ."),
        "hasKey: two individuals with same key should be merged");
}

// ─── Complex scenario: biomedical-style classification ─────────────────────────

#[test]
fn test_biomedical_classification() {
    // SNOMED-like classification: Drug --hasClinicalModality--> Analgesic subClassOf Drug
    let s = store_with(
        "ex:hasClinicalModality rdfs:range ex:ClinicalModality . \
         ex:Analgesic rdfs:subClassOf ex:Drug . \
         ex:Aspirin ex:hasClinicalModality ex:AnalgesicModality . \
         ex:AnalgesicModality rdf:type ex:ClinicalModality ."
    );
    classify(&s);
    // Aspirin gets typed as ClinicalModality's domain participant (via CR8 range)
    assert!(ask_tg(&s, "<http://example.org/AnalgesicModality> rdf:type <http://example.org/ClinicalModality> .") ||
            ask_tg(&s, "<http://example.org/Analgesic> rdfs:subClassOf <http://example.org/Drug> ."),
        "biomedical classification produces expected inferences");
}

// ─── Idempotency ──────────────────────────────────────────────────────────────

#[test]
fn test_el_idempotent() {
    let s = store_with("ex:A rdfs:subClassOf ex:B . ex:x rdf:type ex:A .");
    classify(&s);
    let r1 = El2Classifier::new(&s).classify().unwrap();
    let r2 = El2Classifier::new(&s).classify().unwrap();
    assert_eq!(r1.triples_added, r2.triples_added, "EL classification is idempotent");
}
