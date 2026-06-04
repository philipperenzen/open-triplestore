//! OWL 2 DL conformance tests.
//!
//! Tests the native DL extension rules (hasSelf, disjointUnionOf,
//! NegativePropertyAssertion, hasKey, cardinality annotations) as well as
//! the RL pipeline integration and ExternalReasonerBridge behaviour.

#![cfg(feature = "owl2-dl")]

use open_triplestore::store::TripleStore;
use open_triplestore::reasoning::owl2_dl::{
    Owl2DLReasoner, ExternalReasonerBridge, ExternalReasoner, NativeTableauStub,
    DL_MIN_CARDINALITY, DL_EXACT_CARDINALITY,
    DL_MIN_QUAL_CARDINALITY, DL_EXACT_QUAL_CARDINALITY,
};
use open_triplestore::reasoning::common::ReasoningError;
use oxigraph::io::RdfFormat;

const TG: &str = "urn:entailment:owl2-dl";

fn store_with(ttl: &str) -> TripleStore {
    let store = TripleStore::in_memory().unwrap();
    store.load_str(ttl, RdfFormat::Turtle, None).unwrap();
    store
}

fn ask(store: &TripleStore, sparql: &str) -> bool {
    match store.query(sparql).unwrap() {
        oxigraph::sparql::QueryResults::Boolean(b) => b,
        _ => panic!("expected ASK result"),
    }
}

fn ask_in_tg(store: &TripleStore, s: &str, p: &str, o: &str) -> bool {
    ask(store, &format!("ASK {{ GRAPH <{TG}> {{ <{s}> <{p}> <{o}> }} }}"))
}

fn count_in_tg(store: &TripleStore) -> usize {
    match store.query(&format!(
        "SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{TG}> {{ ?s ?p ?o }} }}"
    )).unwrap() {
        oxigraph::sparql::QueryResults::Solutions(mut sols) => {
            sols.next().and_then(|r| r.ok()).and_then(|s| {
                s.get("c").and_then(|v| match v {
                    oxigraph::model::Term::Literal(lit) => lit.value().parse::<usize>().ok(),
                    _ => None,
                })
            }).unwrap_or(0)
        }
        _ => 0,
    }
}

// ═══════════════════════════════════════════════════════════
// Basic metadata
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_empty_store_ok() {
    let store = TripleStore::in_memory().unwrap();
    assert!(Owl2DLReasoner::new(&store).materialize().is_ok());
}

#[test]
fn dl_report_regime_name() {
    let store = TripleStore::in_memory().unwrap();
    let report = Owl2DLReasoner::new(&store).materialize().unwrap();
    assert_eq!(report.regime, "owl2-dl");
}

#[test]
fn dl_entailment_graph_target() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        ex:A rdfs:subClassOf ex:B .
        ex:x a ex:A .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    // Inferred ex:x a ex:B should land in TG, not default graph
    let in_tg = ask(&store, &format!(
        "ASK {{ GRAPH <{TG}> {{ <http://example.org/x> a <http://example.org/B> }} }}"
    ));
    assert!(in_tg, "inferred triples should be in the entailment graph");
}

#[test]
fn dl_idempotent_second_run() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        ex:A rdfs:subClassOf ex:B .
        ex:x a ex:A .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    let count1 = count_in_tg(&store);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    let count2 = count_in_tg(&store);
    assert_eq!(count1, count2, "second run should be idempotent");
}

// ═══════════════════════════════════════════════════════════
// ExternalReasonerBridge
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_bridge_stub_now_succeeds() {
    let store = TripleStore::in_memory().unwrap();
    let bridge = ExternalReasonerBridge::new(Box::new(NativeTableauStub));
    let result = bridge.materialize(&store, &[], TG);
    assert!(result.is_ok(), "bridge with stub should succeed after native DL rules run");
}

#[test]
fn dl_bridge_with_mock_external_reasoner() {
    /// A mock external reasoner that appends a fixed triple as Turtle.
    struct MockReasoner;
    impl ExternalReasoner for MockReasoner {
        fn name(&self) -> &'static str { "mock" }
        fn classify(&self, _: &str) -> Result<String, ReasoningError> { Ok(String::new()) }
        fn check_consistency(&self, _: &str) -> Result<bool, ReasoningError> { Ok(true) }
        fn get_inferences(&self, _: &str) -> Result<String, ReasoningError> {
            Ok("<http://example.org/mock> <http://example.org/prop> <http://example.org/val> .\n"
                .to_string())
        }
    }

    let store = TripleStore::in_memory().unwrap();
    let bridge = ExternalReasonerBridge::new(Box::new(MockReasoner));
    bridge.materialize(&store, &[], TG).unwrap();

    assert!(ask_in_tg(
        &store,
        "http://example.org/mock",
        "http://example.org/prop",
        "http://example.org/val",
    ), "mock reasoner triple should be in the entailment graph");
}

// ═══════════════════════════════════════════════════════════
// owl:hasSelf
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_has_self_inserts_reflexive_triple() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        ex:SelfClass owl:onProperty ex:knows ;
                     owl:hasSelf "true"^^xsd:boolean .
        ex:alice a ex:SelfClass .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(ask_in_tg(&store, "http://example.org/alice", "http://example.org/knows", "http://example.org/alice"));
}

#[test]
fn dl_has_self_no_false_positive() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        ex:SelfClass owl:onProperty ex:knows ;
                     owl:hasSelf "true"^^xsd:boolean .
        ex:bob a ex:OtherClass .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(!ask_in_tg(&store, "http://example.org/bob", "http://example.org/knows", "http://example.org/bob"));
}

#[test]
fn dl_has_self_multiple_classes() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        ex:C1 owl:onProperty ex:p1 ; owl:hasSelf "true"^^xsd:boolean .
        ex:C2 owl:onProperty ex:p2 ; owl:hasSelf "true"^^xsd:boolean .
        ex:a a ex:C1 .
        ex:b a ex:C2 .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(ask_in_tg(&store, "http://example.org/a", "http://example.org/p1", "http://example.org/a"));
    assert!(ask_in_tg(&store, "http://example.org/b", "http://example.org/p2", "http://example.org/b"));
    assert!(!ask_in_tg(&store, "http://example.org/a", "http://example.org/p2", "http://example.org/a"));
}

// ═══════════════════════════════════════════════════════════
// owl:disjointUnionOf
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_disjoint_union_subclass() {
    let store = store_with(r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex:   <http://example.org/> .
        ex:C owl:disjointUnionOf ( ex:C1 ex:C2 ) .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(ask_in_tg(
        &store, "http://example.org/C1",
        "http://www.w3.org/2000/01/rdf-schema#subClassOf",
        "http://example.org/C"
    ));
    assert!(ask_in_tg(
        &store, "http://example.org/C2",
        "http://www.w3.org/2000/01/rdf-schema#subClassOf",
        "http://example.org/C"
    ));
}

#[test]
fn dl_disjoint_union_pairwise_disjoint() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:C owl:disjointUnionOf ( ex:C1 ex:C2 ) .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(ask_in_tg(
        &store, "http://example.org/C1",
        "http://www.w3.org/2002/07/owl#disjointWith",
        "http://example.org/C2"
    ));
    assert!(ask_in_tg(
        &store, "http://example.org/C2",
        "http://www.w3.org/2002/07/owl#disjointWith",
        "http://example.org/C1"
    ));
}

#[test]
fn dl_disjoint_union_three_members() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:C owl:disjointUnionOf ( ex:C1 ex:C2 ex:C3 ) .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    // All three pairwise combinations should be disjoint
    let dw = "http://www.w3.org/2002/07/owl#disjointWith";
    assert!(ask_in_tg(&store, "http://example.org/C1", dw, "http://example.org/C2"));
    assert!(ask_in_tg(&store, "http://example.org/C1", dw, "http://example.org/C3"));
    assert!(ask_in_tg(&store, "http://example.org/C2", dw, "http://example.org/C3"));
}

#[test]
fn dl_disjoint_union_subclass_propagation() {
    // x type C1, C1 in disjointUnion of C → via subClassOf, x should get type C
    let store = store_with(r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex:   <http://example.org/> .
        ex:C owl:disjointUnionOf ( ex:C1 ex:C2 ) .
        ex:x a ex:C1 .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    // C1 subClassOf C is inserted; RL prp-sco rule should infer x type C
    assert!(ask(&store, &format!(
        "ASK {{ GRAPH <{TG}> {{ <http://example.org/x> \
         <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/C> }} }}"
    )));
}

// ═══════════════════════════════════════════════════════════
// owl:NegativePropertyAssertion
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_negative_object_assertion_ok() {
    // NPA defined but the triple does NOT exist — no violation
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        _:npa a owl:NegativePropertyAssertion ;
              owl:sourceIndividual ex:alice ;
              owl:assertionProperty ex:hates ;
              owl:targetIndividual ex:bob .
    "#);
    assert!(Owl2DLReasoner::new(&store).materialize().is_ok());
}

#[test]
fn dl_negative_object_assertion_violated() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        _:npa a owl:NegativePropertyAssertion ;
              owl:sourceIndividual ex:alice ;
              owl:assertionProperty ex:hates ;
              owl:targetIndividual ex:bob .
        ex:alice ex:hates ex:bob .
    "#);
    let result = Owl2DLReasoner::new(&store).materialize();
    assert!(matches!(result, Err(ReasoningError::Inconsistency(_))));
}

#[test]
fn dl_negative_data_assertion_violated() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        _:npa a owl:NegativePropertyAssertion ;
              owl:sourceIndividual ex:alice ;
              owl:assertionProperty ex:age ;
              owl:targetValue 30 .
        ex:alice ex:age 30 .
    "#);
    let result = Owl2DLReasoner::new(&store).materialize();
    assert!(matches!(result, Err(ReasoningError::Inconsistency(_))));
}

#[test]
fn dl_negative_assertion_different_target_ok() {
    // Same property, different target value — no violation
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        _:npa a owl:NegativePropertyAssertion ;
              owl:sourceIndividual ex:alice ;
              owl:assertionProperty ex:hates ;
              owl:targetIndividual ex:bob .
        ex:alice ex:hates ex:carol .
    "#);
    assert!(Owl2DLReasoner::new(&store).materialize().is_ok());
}

// ═══════════════════════════════════════════════════════════
// owl:hasKey
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_has_key_single_matches() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Person owl:hasKey ( ex:ssn ) .
        ex:alice a ex:Person ; ex:ssn "123" .
        ex:bob   a ex:Person ; ex:ssn "123" .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(ask_in_tg(
        &store, "http://example.org/alice",
        "http://www.w3.org/2002/07/owl#sameAs",
        "http://example.org/bob"
    ));
}

#[test]
fn dl_has_key_single_no_match() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Person owl:hasKey ( ex:ssn ) .
        ex:alice a ex:Person ; ex:ssn "123" .
        ex:bob   a ex:Person ; ex:ssn "456" .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(!ask_in_tg(
        &store, "http://example.org/alice",
        "http://www.w3.org/2002/07/owl#sameAs",
        "http://example.org/bob"
    ));
}

#[test]
fn dl_has_key_two_keys_both_match() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Person owl:hasKey ( ex:first ex:last ) .
        ex:alice a ex:Person ; ex:first "Alice" ; ex:last "Smith" .
        ex:alice2 a ex:Person ; ex:first "Alice" ; ex:last "Smith" .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(ask_in_tg(
        &store, "http://example.org/alice",
        "http://www.w3.org/2002/07/owl#sameAs",
        "http://example.org/alice2"
    ));
}

#[test]
fn dl_has_key_two_keys_partial_match() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Person owl:hasKey ( ex:first ex:last ) .
        ex:alice a ex:Person ; ex:first "Alice" ; ex:last "Smith" .
        ex:other a ex:Person ; ex:first "Alice" ; ex:last "Jones" .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(!ask_in_tg(
        &store, "http://example.org/alice",
        "http://www.w3.org/2002/07/owl#sameAs",
        "http://example.org/other"
    ));
}

#[test]
fn dl_has_key_blank_nodes_excluded() {
    // Blank node subjects should not produce sameAs triples
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Person owl:hasKey ( ex:ssn ) .
        _:a a ex:Person ; ex:ssn "999" .
        _:b a ex:Person ; ex:ssn "999" .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    // No IRI-to-IRI sameAs should be produced for blank nodes
    assert!(!ask(&store, &format!(
        "ASK {{ GRAPH <{TG}> {{ ?x <http://www.w3.org/2002/07/owl#sameAs> ?y \
         FILTER(isIRI(?x)) FILTER(isIRI(?y)) }} }}"
    )));
}

// ═══════════════════════════════════════════════════════════
// Cardinality annotations
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_min_cardinality_annotation_in_tg() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Restriction owl:onProperty ex:hasPart ;
                        owl:minCardinality 1 .
        ex:item a ex:Restriction .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(ask(&store, &format!(
        "ASK {{ GRAPH <{TG}> {{ <http://example.org/item> <{DL_MIN_CARDINALITY}> ?n }} }}"
    )));
}

#[test]
fn dl_exact_cardinality_annotation() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Restriction owl:onProperty ex:hasPart ;
                        owl:cardinality 2 .
        ex:item a ex:Restriction .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(ask(&store, &format!(
        "ASK {{ GRAPH <{TG}> {{ <http://example.org/item> <{DL_EXACT_CARDINALITY}> ?n }} }}"
    )));
}

#[test]
fn dl_min_qualified_cardinality_annotation() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Restriction owl:onProperty ex:hasPart ;
                        owl:minQualifiedCardinality 1 ;
                        owl:onClass ex:Part .
        ex:item a ex:Restriction .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(ask(&store, &format!(
        "ASK {{ GRAPH <{TG}> {{ <http://example.org/item> <{DL_MIN_QUAL_CARDINALITY}> ?n }} }}"
    )));
}

#[test]
fn dl_qualified_cardinality_annotation() {
    let store = store_with(r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Restriction owl:onProperty ex:hasPart ;
                        owl:qualifiedCardinality 3 ;
                        owl:onClass ex:Part .
        ex:item a ex:Restriction .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(ask(&store, &format!(
        "ASK {{ GRAPH <{TG}> {{ <http://example.org/item> <{DL_EXACT_QUAL_CARDINALITY}> ?n }} }}"
    )));
}

// ═══════════════════════════════════════════════════════════
// RL pipeline integration
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_rl_pipeline_subclass_fires() {
    // The RL cls-svf1 / prp-sco rules should fire within the DL run
    let store = store_with(r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex:   <http://example.org/> .
        ex:A rdfs:subClassOf ex:B .
        ex:B rdfs:subClassOf ex:C .
        ex:x a ex:A .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    assert!(ask(&store, &format!(
        "ASK {{ GRAPH <{TG}> {{ <http://example.org/x> \
         <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/C> }} }}"
    )));
}

#[test]
fn dl_rl_disjoint_inconsistency_fires() {
    // RL cls-dw detects disjointWith violations; should propagate through DL run
    let store = store_with(r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix ex:   <http://example.org/> .
        ex:A owl:disjointWith ex:B .
        ex:x a ex:A .
        ex:x a ex:B .
    "#);
    let result = Owl2DLReasoner::new(&store).materialize();
    assert!(matches!(result, Err(ReasoningError::Inconsistency(_))));
}

#[test]
fn dl_combined_has_self_and_subclass() {
    let store = store_with(r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex:   <http://example.org/> .
        @prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
        ex:Base owl:onProperty ex:ref ; owl:hasSelf "true"^^xsd:boolean .
        ex:Sub rdfs:subClassOf ex:Base .
        ex:y a ex:Sub .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    // y type Sub → y type Base (RL subClassOf) → y ref y (hasSelf)
    assert!(ask_in_tg(&store, "http://example.org/y", "http://example.org/ref", "http://example.org/y"));
}

#[test]
fn dl_rl_equivalent_class() {
    let store = store_with(r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix ex:   <http://example.org/> .
        ex:A owl:equivalentClass ex:B .
        ex:x a ex:A .
    "#);
    Owl2DLReasoner::new(&store).with_target(TG).materialize().unwrap();
    // RL cls-com: equivalentClass → both subClassOf directions → x type B
    assert!(ask(&store, &format!(
        "ASK {{ GRAPH <{TG}> {{ <http://example.org/x> \
         <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/B> }} }}"
    )));
}
