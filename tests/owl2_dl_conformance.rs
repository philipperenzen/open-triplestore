//! OWL 2 DL conformance tests.
//!
//! The native backend (RL + DL-syntax rules to one joint fixed point:
//! hasSelf both ways, ReflexiveProperty, disjointUnionOf), the cardinality
//! diagnostics graph, the OWL 2 DL profile check, and the backend protocol
//! (`dl_backend`) against an in-test reasoner sidecar and a fake Konclude
//! binary. Live Konclude tests run only when `OTS_TEST_KONCLUDE_BIN` is set.

#![cfg(feature = "owl2-dl")]

use open_triplestore::reasoning::common::ReasoningError;
use open_triplestore::reasoning::dl_backend::{self, CheckTask, Tri};
use open_triplestore::reasoning::dl_config::{DlBackendKind, DlConfig};
use open_triplestore::reasoning::identity::IdentityPolicy;
use open_triplestore::reasoning::owl2_dl::{
    Owl2DLReasoner, DL_EXACT_CARDINALITY, DL_EXACT_QUAL_CARDINALITY, DL_MIN_CARDINALITY,
    DL_MIN_QUAL_CARDINALITY,
};
use open_triplestore::store::TripleStore;
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
    ask(
        store,
        &format!("ASK {{ GRAPH <{TG}> {{ <{s}> <{p}> <{o}> }} }}"),
    )
}

fn count_in_tg(store: &TripleStore) -> usize {
    match store
        .query(&format!(
            "SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{TG}> {{ ?s ?p ?o }} }}"
        ))
        .unwrap()
    {
        oxigraph::sparql::QueryResults::Solutions(mut sols) => sols
            .next()
            .and_then(|r| r.ok())
            .and_then(|s| {
                s.get("c").and_then(|v| match v {
                    oxigraph::model::Term::Literal(lit) => lit.value().parse::<usize>().ok(),
                    _ => None,
                })
            })
            .unwrap_or(0),
        _ => 0,
    }
}

// ═══════════════════════════════════════════════════════════
// Basic metadata
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_empty_store_ok() {
    let store = TripleStore::in_memory().unwrap();
    let report = Owl2DLReasoner::new(&store)
        .materialize()
        .expect("an empty store is trivially consistent");
    assert_eq!(report.regime, "owl2-dl");
    // OWL 2 RL's dt-type1 holds for every ontology, the empty one included: the
    // 32 datatypes of the RL datatype map are rdfs:Datatypes. Nothing else can
    // be derived from nothing.
    assert_eq!(
        report.triples_added, 32,
        "only the dt-type1 datatype-map axioms are derived from nothing"
    );
}

#[test]
fn dl_report_regime_name() {
    let store = TripleStore::in_memory().unwrap();
    let report = Owl2DLReasoner::new(&store).materialize().unwrap();
    assert_eq!(report.regime, "owl2-dl");
}

#[test]
fn dl_entailment_graph_target() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        ex:A rdfs:subClassOf ex:B .
        ex:x a ex:A .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    // Inferred ex:x a ex:B should land in TG, not default graph
    let in_tg = ask(
        &store,
        &format!("ASK {{ GRAPH <{TG}> {{ <http://example.org/x> a <http://example.org/B> }} }}"),
    );
    assert!(in_tg, "inferred triples should be in the entailment graph");
}

#[test]
fn dl_idempotent_second_run() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        ex:A rdfs:subClassOf ex:B .
        ex:x a ex:A .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    let count1 = count_in_tg(&store);
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    let count2 = count_in_tg(&store);
    assert_eq!(count1, count2, "second run should be idempotent");
}

// ═══════════════════════════════════════════════════════════
// Backend selection (D5): no backend, no silent fallback
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_backend_unconfigured_is_unavailable() {
    let store = store_with(
        r#"@prefix ex: <http://example.org/> .
           @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
           ex:A rdfs:subClassOf ex:B . ex:x a ex:A ."#,
    );
    let r = dl_backend::materialize(&store, &DlConfig::default(), None, TG, IdentityPolicy::Full);
    assert!(matches!(r, Err(ReasoningError::Unavailable(_))), "{r:?}");
    assert_eq!(
        count_in_tg(&store),
        0,
        "nothing is derived without a backend"
    );
}

#[test]
fn dl_backend_native_reports_incomplete() {
    let store = store_with(
        r#"@prefix ex: <http://example.org/> .
           @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
           ex:A rdfs:subClassOf ex:B . ex:x a ex:A ."#,
    );
    let cfg = DlConfig::default().with_backend(DlBackendKind::Native);
    let run = dl_backend::materialize(&store, &cfg, None, TG, IdentityPolicy::Full).unwrap();
    assert_eq!(run.backend, "native");
    assert!(
        !run.complete,
        "the native rules are not a complete DL reasoner"
    );
    assert!(
        run.warnings.iter().any(|w| w.contains("not complete")),
        "{:?}",
        run.warnings
    );
    assert!(ask_in_tg(
        &store,
        "http://example.org/x",
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
        "http://example.org/B"
    ));
}

#[test]
fn dl_backend_external_failure_never_falls_back_to_native() {
    let store = store_with(
        r#"@prefix ex: <http://example.org/> .
           @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
           ex:A rdfs:subClassOf ex:B . ex:x a ex:A ."#,
    );
    // A port nothing listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut cfg = DlConfig::default().with_backend(DlBackendKind::Sidecar);
    cfg.sidecar_url = Some(format!("http://127.0.0.1:{port}"));
    let r = dl_backend::materialize(&store, &cfg, None, TG, IdentityPolicy::Full);
    assert!(matches!(r, Err(ReasoningError::Unavailable(_))), "{r:?}");
    assert_eq!(
        count_in_tg(&store),
        0,
        "no native results stand in for the sidecar's"
    );
}

// ═══════════════════════════════════════════════════════════
// owl:hasSelf
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_has_self_inserts_reflexive_triple() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        ex:SelfClass owl:onProperty ex:knows ;
                     owl:hasSelf "true"^^xsd:boolean .
        ex:alice a ex:SelfClass .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(ask_in_tg(
        &store,
        "http://example.org/alice",
        "http://example.org/knows",
        "http://example.org/alice"
    ));
}

#[test]
fn dl_has_self_no_false_positive() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        ex:SelfClass owl:onProperty ex:knows ;
                     owl:hasSelf "true"^^xsd:boolean .
        ex:bob a ex:OtherClass .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(!ask_in_tg(
        &store,
        "http://example.org/bob",
        "http://example.org/knows",
        "http://example.org/bob"
    ));
}

#[test]
fn dl_has_self_multiple_classes() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        ex:C1 owl:onProperty ex:p1 ; owl:hasSelf "true"^^xsd:boolean .
        ex:C2 owl:onProperty ex:p2 ; owl:hasSelf "true"^^xsd:boolean .
        ex:a a ex:C1 .
        ex:b a ex:C2 .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(ask_in_tg(
        &store,
        "http://example.org/a",
        "http://example.org/p1",
        "http://example.org/a"
    ));
    assert!(ask_in_tg(
        &store,
        "http://example.org/b",
        "http://example.org/p2",
        "http://example.org/b"
    ));
    assert!(!ask_in_tg(
        &store,
        "http://example.org/a",
        "http://example.org/p2",
        "http://example.org/a"
    ));
}

// ═══════════════════════════════════════════════════════════
// owl:disjointUnionOf
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_disjoint_union_subclass() {
    let store = store_with(
        r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex:   <http://example.org/> .
        ex:C owl:disjointUnionOf ( ex:C1 ex:C2 ) .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(ask_in_tg(
        &store,
        "http://example.org/C1",
        "http://www.w3.org/2000/01/rdf-schema#subClassOf",
        "http://example.org/C"
    ));
    assert!(ask_in_tg(
        &store,
        "http://example.org/C2",
        "http://www.w3.org/2000/01/rdf-schema#subClassOf",
        "http://example.org/C"
    ));
}

#[test]
fn dl_disjoint_union_pairwise_disjoint() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:C owl:disjointUnionOf ( ex:C1 ex:C2 ) .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(ask_in_tg(
        &store,
        "http://example.org/C1",
        "http://www.w3.org/2002/07/owl#disjointWith",
        "http://example.org/C2"
    ));
    assert!(ask_in_tg(
        &store,
        "http://example.org/C2",
        "http://www.w3.org/2002/07/owl#disjointWith",
        "http://example.org/C1"
    ));
}

#[test]
fn dl_disjoint_union_three_members() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:C owl:disjointUnionOf ( ex:C1 ex:C2 ex:C3 ) .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    // All three pairwise combinations should be disjoint
    let dw = "http://www.w3.org/2002/07/owl#disjointWith";
    assert!(ask_in_tg(
        &store,
        "http://example.org/C1",
        dw,
        "http://example.org/C2"
    ));
    assert!(ask_in_tg(
        &store,
        "http://example.org/C1",
        dw,
        "http://example.org/C3"
    ));
    assert!(ask_in_tg(
        &store,
        "http://example.org/C2",
        dw,
        "http://example.org/C3"
    ));
}

#[test]
fn dl_disjoint_union_subclass_propagation() {
    // x type C1, C1 in disjointUnion of C → via subClassOf, x should get type C
    let store = store_with(
        r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex:   <http://example.org/> .
        ex:C owl:disjointUnionOf ( ex:C1 ex:C2 ) .
        ex:x a ex:C1 .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    // C1 subClassOf C is inserted; RL prp-sco rule should infer x type C
    assert!(ask(
        &store,
        &format!(
            "ASK {{ GRAPH <{TG}> {{ <http://example.org/x> \
         <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/C> }} }}"
        )
    ));
}

// ═══════════════════════════════════════════════════════════
// owl:NegativePropertyAssertion
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_negative_object_assertion_ok() {
    // NPA defined but the triple does NOT exist — no violation
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        _:npa a owl:NegativePropertyAssertion ;
              owl:sourceIndividual ex:alice ;
              owl:assertionProperty ex:hates ;
              owl:targetIndividual ex:bob .
    "#,
    );
    // The violated counterpart asserts `Err(Inconsistency)`; this one must run
    // to completion AND produce a report, not merely fail to error.
    let report = Owl2DLReasoner::new(&store)
        .materialize()
        .expect("an unviolated negative assertion is consistent");
    assert_eq!(report.regime, "owl2-dl");
}

#[test]
fn dl_negative_object_assertion_violated() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        _:npa a owl:NegativePropertyAssertion ;
              owl:sourceIndividual ex:alice ;
              owl:assertionProperty ex:hates ;
              owl:targetIndividual ex:bob .
        ex:alice ex:hates ex:bob .
    "#,
    );
    let result = Owl2DLReasoner::new(&store).materialize();
    assert!(matches!(result, Err(ReasoningError::Inconsistency { .. })));
}

#[test]
fn dl_negative_data_assertion_violated() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        _:npa a owl:NegativePropertyAssertion ;
              owl:sourceIndividual ex:alice ;
              owl:assertionProperty ex:age ;
              owl:targetValue 30 .
        ex:alice ex:age 30 .
    "#,
    );
    let result = Owl2DLReasoner::new(&store).materialize();
    assert!(matches!(result, Err(ReasoningError::Inconsistency { .. })));
}

#[test]
fn dl_negative_assertion_different_target_ok() {
    // Same property, different target value — no violation
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        _:npa a owl:NegativePropertyAssertion ;
              owl:sourceIndividual ex:alice ;
              owl:assertionProperty ex:hates ;
              owl:targetIndividual ex:bob .
        ex:alice ex:hates ex:carol .
    "#,
    );
    // The violated counterpart asserts `Err(Inconsistency)`; this one must run
    // to completion AND produce a report, not merely fail to error.
    let report = Owl2DLReasoner::new(&store)
        .materialize()
        .expect("an unviolated negative assertion is consistent");
    assert_eq!(report.regime, "owl2-dl");
}

// ═══════════════════════════════════════════════════════════
// owl:hasKey
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_has_key_single_matches() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Person owl:hasKey ( ex:ssn ) .
        ex:alice a ex:Person ; ex:ssn "123" .
        ex:bob   a ex:Person ; ex:ssn "123" .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(ask_in_tg(
        &store,
        "http://example.org/alice",
        "http://www.w3.org/2002/07/owl#sameAs",
        "http://example.org/bob"
    ));
}

#[test]
fn dl_has_key_single_no_match() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Person owl:hasKey ( ex:ssn ) .
        ex:alice a ex:Person ; ex:ssn "123" .
        ex:bob   a ex:Person ; ex:ssn "456" .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(!ask_in_tg(
        &store,
        "http://example.org/alice",
        "http://www.w3.org/2002/07/owl#sameAs",
        "http://example.org/bob"
    ));
}

#[test]
fn dl_has_key_two_keys_both_match() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Person owl:hasKey ( ex:first ex:last ) .
        ex:alice a ex:Person ; ex:first "Alice" ; ex:last "Smith" .
        ex:alice2 a ex:Person ; ex:first "Alice" ; ex:last "Smith" .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(ask_in_tg(
        &store,
        "http://example.org/alice",
        "http://www.w3.org/2002/07/owl#sameAs",
        "http://example.org/alice2"
    ));
}

#[test]
fn dl_has_key_two_keys_partial_match() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Person owl:hasKey ( ex:first ex:last ) .
        ex:alice a ex:Person ; ex:first "Alice" ; ex:last "Smith" .
        ex:other a ex:Person ; ex:first "Alice" ; ex:last "Jones" .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(!ask_in_tg(
        &store,
        "http://example.org/alice",
        "http://www.w3.org/2002/07/owl#sameAs",
        "http://example.org/other"
    ));
}

#[test]
fn dl_has_key_blank_nodes_excluded() {
    // Blank node subjects should not produce sameAs triples
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Person owl:hasKey ( ex:ssn ) .
        _:a a ex:Person ; ex:ssn "999" .
        _:b a ex:Person ; ex:ssn "999" .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    // No IRI-to-IRI sameAs should be produced for blank nodes
    assert!(!ask(
        &store,
        &format!(
            "ASK {{ GRAPH <{TG}> {{ ?x <http://www.w3.org/2002/07/owl#sameAs> ?y \
         FILTER(isIRI(?x)) FILTER(isIRI(?y)) }} }}"
        )
    ));
}

// ═══════════════════════════════════════════════════════════
// Cardinality obligations go to the diagnostics graph (D8)
// ═══════════════════════════════════════════════════════════

const DIAG: &str = "urn:entailment:owl2-dl:diagnostics";

/// The obligation `mark` is recorded for ex:item in the diagnostics graph
/// and never in the entailment graph that queries fold in.
fn obligation_recorded(ttl: &str, mark: &str) {
    let store = store_with(ttl);
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(
        ask(
            &store,
            &format!("ASK {{ GRAPH <{DIAG}> {{ <http://example.org/item> <{mark}> ?n }} }}")
        ),
        "{mark} is recorded in the diagnostics graph"
    );
    assert!(
        !ask(
            &store,
            &format!("ASK {{ GRAPH <{TG}> {{ ?x <{mark}> ?n }} }}")
        ),
        "{mark} is not an inference and stays out of <{TG}>"
    );
}

#[test]
fn dl_min_cardinality_annotation_in_diagnostics() {
    obligation_recorded(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Restriction owl:onProperty ex:hasPart ;
                        owl:minCardinality 1 .
        ex:item a ex:Restriction .
    "#,
        DL_MIN_CARDINALITY,
    );
}

#[test]
fn dl_exact_cardinality_annotation() {
    obligation_recorded(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Restriction owl:onProperty ex:hasPart ;
                        owl:cardinality 2 .
        ex:item a ex:Restriction .
    "#,
        DL_EXACT_CARDINALITY,
    );
}

#[test]
fn dl_min_qualified_cardinality_annotation() {
    obligation_recorded(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Restriction owl:onProperty ex:hasPart ;
                        owl:minQualifiedCardinality 1 ;
                        owl:onClass ex:Part .
        ex:item a ex:Restriction .
    "#,
        DL_MIN_QUAL_CARDINALITY,
    );
}

#[test]
fn dl_qualified_cardinality_annotation() {
    obligation_recorded(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:Restriction owl:onProperty ex:hasPart ;
                        owl:qualifiedCardinality 3 ;
                        owl:onClass ex:Part .
        ex:item a ex:Restriction .
    "#,
        DL_EXACT_QUAL_CARDINALITY,
    );
}

// ═══════════════════════════════════════════════════════════
// RL pipeline integration
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_rl_pipeline_subclass_fires() {
    // The RL cls-svf1 / prp-sco rules should fire within the DL run
    let store = store_with(
        r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex:   <http://example.org/> .
        ex:A rdfs:subClassOf ex:B .
        ex:B rdfs:subClassOf ex:C .
        ex:x a ex:A .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(ask(
        &store,
        &format!(
            "ASK {{ GRAPH <{TG}> {{ <http://example.org/x> \
         <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/C> }} }}"
        )
    ));
}

#[test]
fn dl_rl_disjoint_inconsistency_fires() {
    // RL cls-dw detects disjointWith violations; should propagate through DL run
    let store = store_with(
        r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix ex:   <http://example.org/> .
        ex:A owl:disjointWith ex:B .
        ex:x a ex:A .
        ex:x a ex:B .
    "#,
    );
    let result = Owl2DLReasoner::new(&store).materialize();
    assert!(matches!(result, Err(ReasoningError::Inconsistency { .. })));
}

#[test]
fn dl_combined_has_self_and_subclass() {
    let store = store_with(
        r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex:   <http://example.org/> .
        @prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
        ex:Base owl:onProperty ex:ref ; owl:hasSelf "true"^^xsd:boolean .
        ex:Sub rdfs:subClassOf ex:Base .
        ex:y a ex:Sub .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    // y type Sub → y type Base (RL subClassOf) → y ref y (hasSelf)
    assert!(ask_in_tg(
        &store,
        "http://example.org/y",
        "http://example.org/ref",
        "http://example.org/y"
    ));
}

#[test]
fn dl_rl_equivalent_class() {
    let store = store_with(
        r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix ex:   <http://example.org/> .
        ex:A owl:equivalentClass ex:B .
        ex:x a ex:A .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    // RL cls-com: equivalentClass → both subClassOf directions → x type B
    assert!(ask(
        &store,
        &format!(
            "ASK {{ GRAPH <{TG}> {{ <http://example.org/x> \
         <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/B> }} }}"
        )
    ));
}

// ═══════════════════════════════════════════════════════════
// High-complexity OWL 2 DL tests (research-derived, spec-verified)
//
// The native reasoner is RL forward-chaining + DL-syntax extension rules. RL-
// expressible entailments (property chains, inverse-functional, symmetric) are
// materialized into the entailment graph. Full DL tableau reasoning (profile
// validation, nominal/cardinality/datatype inconsistency, reflexive+irreflexive
// contradiction) requires the external reasoner bridge and is documented as a
// tracked gap. Verifier correction applied: the property chain (research
// complex-05) is rewritten so the chain actually fires.
// ═══════════════════════════════════════════════════════════

const OWL_SAME_AS: &str = "http://www.w3.org/2002/07/owl#sameAs";

// complex-05 (CORRECTED): hasParent ∘ hasBrother ⊑ hasUncle — a chain that fires.
#[test]
fn dl_cx_property_chain_entailment() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:hasUncle owl:propertyChainAxiom ( ex:hasParent ex:hasBrother ) .
        ex:John ex:hasParent ex:Mary .
        ex:Mary ex:hasBrother ex:Bob .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(
        ask_in_tg(
            &store,
            "http://example.org/John",
            "http://example.org/hasUncle",
            "http://example.org/Bob"
        ),
        "property chain hasParent∘hasBrother ⊑ hasUncle must entail John hasUncle Bob"
    );
}

// complex-12: an inverse-functional property entails SameIndividual for a shared object.
#[test]
fn dl_cx_inverse_functional_same_individual() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:isMotherOf a owl:InverseFunctionalProperty .
        ex:MarySmith ex:isMotherOf ex:John .
        ex:MaryJones ex:isMotherOf ex:John .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    let fwd = ask_in_tg(
        &store,
        "http://example.org/MarySmith",
        OWL_SAME_AS,
        "http://example.org/MaryJones",
    );
    let rev = ask_in_tg(
        &store,
        "http://example.org/MaryJones",
        OWL_SAME_AS,
        "http://example.org/MarySmith",
    );
    assert!(
        fwd || rev,
        "inverse-functional property must entail SameIndividual(MarySmith, MaryJones)"
    );
}

// complex-07 (positive half): a symmetric property entails the reverse assertion.
#[test]
fn dl_cx_symmetric_property_entailment() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:marriedTo a owl:SymmetricProperty .
        ex:Alice ex:marriedTo ex:Bob .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(
        ask_in_tg(
            &store,
            "http://example.org/Bob",
            "http://example.org/marriedTo",
            "http://example.org/Alice"
        ),
        "symmetric property must entail the reverse assertion"
    );
}

// Tracked gap: the native rules do no tableau reasoning — an entailment that
// needs an existential witness is not derived. Konclude or the sidecar derive
// it (see `dl_konclude_live_existential_witness`).
#[test]
fn dl_cx_native_tableau_reasoning_is_a_gap() {
    let store = store_with(
        r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex:   <http://example.org/> .
        ex:A rdfs:subClassOf [ a owl:Restriction ; owl:onProperty ex:r ; owl:someValuesFrom ex:B ] .
        [ a owl:Restriction ; owl:onProperty ex:r ; owl:someValuesFrom ex:B ] rdfs:subClassOf ex:C .
        ex:a a ex:A .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(
        !ask_in_tg(
            &store,
            "http://example.org/a",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
            "http://example.org/C"
        ),
        "the native rules do not create existential witnesses (DL-tableau gap)"
    );
}

// ═══════════════════════════════════════════════════════════
// Joint RL + DL fixed point (card 4)
// ═══════════════════════════════════════════════════════════

/// `x knows x` from hasSelf is a premise for RL's domain and sub-property
/// rules: the RL rules run again after the DL rules.
#[test]
fn dl_rl_rules_see_dl_derivations() {
    let store = store_with(
        r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex:   <http://example.org/> .
        ex:Narcissist owl:equivalentClass [ a owl:Restriction ; owl:onProperty ex:loves ; owl:hasSelf true ] .
        ex:loves rdfs:domain ex:Lover ; rdfs:subPropertyOf ex:likes .
        ex:n a ex:Narcissist .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    let n = "http://example.org/n";
    assert!(ask_in_tg(&store, n, "http://example.org/loves", n));
    assert!(
        ask_in_tg(&store, n, "http://example.org/likes", n),
        "prp-spo1 sees the hasSelf consequence"
    );
    assert!(
        ask_in_tg(
            &store,
            n,
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
            "http://example.org/Lover"
        ),
        "prp-dom sees the hasSelf consequence"
    );
}

/// `x p x` makes x an instance of `∃p.Self`.
#[test]
fn dl_has_self_converse() {
    let store = store_with(
        r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix ex:   <http://example.org/> .
        ex:SelfAware owl:equivalentClass [ a owl:Restriction ; owl:onProperty ex:knows ; owl:hasSelf true ] .
        ex:s ex:knows ex:s .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(
        ask_in_tg(
            &store,
            "http://example.org/s",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
            "http://example.org/SelfAware"
        ),
        "x p x ⇒ x : ∃p.Self, and so x : SelfAware"
    );
}

/// A key of three properties (RL `prp-key`; the DL layer's own 1- and
/// 2-property key rules were duplicates and are gone).
#[test]
fn dl_has_key_three_properties() {
    let store = store_with(
        r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix ex:   <http://example.org/> .
        ex:Address owl:hasKey ( ex:street ex:number ex:city ) .
        ex:a1 a ex:Address ; ex:street "Main" ; ex:number 1 ; ex:city "X" .
        ex:a2 a ex:Address ; ex:street "Main" ; ex:number 1 ; ex:city "X" .
        ex:a3 a ex:Address ; ex:street "Main" ; ex:number 1 ; ex:city "Y" .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    let same = "http://www.w3.org/2002/07/owl#sameAs";
    assert!(
        ask_in_tg(
            &store,
            "http://example.org/a1",
            same,
            "http://example.org/a2"
        ) || ask_in_tg(
            &store,
            "http://example.org/a2",
            same,
            "http://example.org/a1"
        )
    );
    assert!(!ask_in_tg(
        &store,
        "http://example.org/a1",
        same,
        "http://example.org/a3"
    ));
}

/// Reflexive + irreflexive on one property, with an individual in scope, is
/// inconsistent: `dl-reflexive` gives `x p x`, and `prp-irp` fires.
#[test]
fn dl_reflexive_irreflexive_is_inconsistent() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:knows a owl:ReflexiveProperty , owl:IrreflexiveProperty .
        ex:x a owl:Thing .
    "#,
    );
    let r = Owl2DLReasoner::new(&store).with_target(TG).materialize();
    match r {
        Err(ReasoningError::Inconsistency { rule, .. }) => assert_eq!(rule, "prp-irp"),
        other => panic!("expected prp-irp, got {other:?}"),
    }
}

/// A reflexive property alone is consistent and makes every individual in
/// scope related to itself.
#[test]
fn dl_reflexive_property_relates_individuals_to_themselves() {
    let store = store_with(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:sameAgeAs a owl:ReflexiveProperty .
        ex:p a ex:Person .
    "#,
    );
    Owl2DLReasoner::new(&store)
        .with_target(TG)
        .materialize()
        .unwrap();
    assert!(ask_in_tg(
        &store,
        "http://example.org/p",
        "http://example.org/sameAgeAs",
        "http://example.org/p"
    ));
}

// ═══════════════════════════════════════════════════════════
// OWL 2 DL profile (D6): non-DL input is refused with the violations
// ═══════════════════════════════════════════════════════════

fn native() -> DlConfig {
    DlConfig::default().with_backend(DlBackendKind::Native)
}

fn violations_of(ttl: &str) -> Vec<String> {
    let store = store_with(ttl);
    match dl_backend::materialize(&store, &native(), None, TG, IdentityPolicy::Full) {
        Err(ReasoningError::NotInProfile { violations }) => {
            violations.into_iter().map(|v| v.rule).collect()
        }
        other => panic!("expected NotInProfile, got {other:?}"),
    }
}

#[test]
fn dl_profile_non_simple_role_in_cardinality() {
    let v = violations_of(
        r#"
        @prefix owl:  <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex:   <http://example.org/> .
        ex:part a owl:TransitiveProperty ; rdfs:subPropertyOf ex:hasPart .
        ex:Wheel rdfs:subClassOf [ a owl:Restriction ;
            owl:onProperty ex:hasPart ; owl:maxQualifiedCardinality 4 ;
            owl:onClass ex:Spoke ] .
    "#,
    );
    assert!(v.contains(&"non-simple-property".to_string()), "{v:?}");
}

#[test]
fn dl_profile_irregular_chain() {
    let v = violations_of(
        r#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex:  <http://example.org/> .
        ex:hasUncle owl:propertyChainAxiom ( ex:hasFather ex:hasBrother ) .
        ex:hasBrother owl:propertyChainAxiom ( ex:hasChild ex:hasUncle ) .
    "#,
    );
    assert!(
        v.contains(&"irregular-property-hierarchy".to_string()),
        "{v:?}"
    );
}

#[test]
fn dl_profile_object_data_punning() {
    let v = violations_of(
        r#"
        @prefix ex: <http://example.org/> .
        ex:a ex:code ex:b .
        ex:c ex:code "42" .
    "#,
    );
    assert!(v.contains(&"property-punning".to_string()), "{v:?}");
}

#[test]
fn dl_profile_triples_without_owl_reading_are_refused() {
    let v = violations_of(
        r#"
        @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
        @prefix ex:  <http://example.org/> .
        ex:a rdf:value ex:b .
    "#,
    );
    assert!(v.contains(&"unmapped-triple".to_string()), "{v:?}");
}

#[test]
fn dl_profile_check_answers_without_a_backend() {
    let store = TripleStore::in_memory().unwrap();
    let premise =
        triples(r#"@prefix ex: <http://example.org/> . ex:a ex:code ex:b . ex:c ex:code "42" ."#);
    let (o, backend, _) = dl_backend::check(
        &store,
        &DlConfig::default(),
        None,
        Some(premise),
        &CheckTask::Profile,
        IdentityPolicy::Full,
    )
    .unwrap();
    assert_eq!(o.result, Tri::False);
    assert_eq!(backend, "server");
    assert!(o.violations.iter().any(|v| v.rule == "property-punning"));
}

fn triples(ttl: &str) -> Vec<oxigraph::model::Triple> {
    oxigraph::io::RdfParser::from_format(RdfFormat::Turtle)
        .for_slice(ttl.as_bytes())
        .map(|q| q.unwrap().into())
        .collect()
}

// ═══════════════════════════════════════════════════════════
// Native checks: `false` is sound, nothing found is `unknown`
// ═══════════════════════════════════════════════════════════

#[test]
fn dl_native_check_consistency_false_or_unknown() {
    let store = TripleStore::in_memory().unwrap();
    let bad = triples(
        r#"@prefix owl: <http://www.w3.org/2002/07/owl#> . @prefix ex: <http://example.org/> .
           ex:A owl:disjointWith ex:B . ex:x a ex:A , ex:B ."#,
    );
    let (o, backend, complete) = dl_backend::check(
        &store,
        &native(),
        None,
        Some(bad),
        &CheckTask::Consistency,
        IdentityPolicy::Full,
    )
    .unwrap();
    assert_eq!((o.result, backend, complete), (Tri::False, "native", false));
    assert_eq!(
        o.inconsistency.as_ref().map(|i| i.0.as_str()),
        Some("cax-dw")
    );

    let fine = triples(r#"@prefix ex: <http://example.org/> . ex:x a ex:A ."#);
    let (o, ..) = dl_backend::check(
        &store,
        &native(),
        None,
        Some(fine),
        &CheckTask::Consistency,
        IdentityPolicy::Full,
    )
    .unwrap();
    assert_eq!(
        o.result,
        Tri::Unknown,
        "the native rules cannot prove consistency"
    );
}

#[test]
fn dl_native_check_entailment_true_or_unknown() {
    let store = TripleStore::in_memory().unwrap();
    let premise = triples(
        r#"@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . @prefix ex: <http://example.org/> .
           ex:A rdfs:subClassOf ex:B . ex:x a ex:A ."#,
    );
    let task = CheckTask::Entailment {
        conclusion: triples(r#"@prefix ex: <http://example.org/> . ex:x a ex:B ."#),
    };
    let (o, ..) = dl_backend::check(
        &store,
        &native(),
        None,
        Some(premise.clone()),
        &task,
        IdentityPolicy::Full,
    )
    .unwrap();
    assert_eq!(o.result, Tri::True);
    let task = CheckTask::Entailment {
        conclusion: triples(r#"@prefix ex: <http://example.org/> . ex:x a ex:C ."#),
    };
    let (o, ..) = dl_backend::check(
        &store,
        &native(),
        None,
        Some(premise),
        &task,
        IdentityPolicy::Full,
    )
    .unwrap();
    assert_eq!(
        o.result,
        Tri::Unknown,
        "not derived natively proves nothing"
    );
}

// ═══════════════════════════════════════════════════════════
// The reasoner sidecar protocol, against an in-test mock
// ═══════════════════════════════════════════════════════════

mod sidecar {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use std::time::Duration;

    #[derive(Clone)]
    struct Mock {
        reason: Value,
        check: Value,
        delay: Duration,
        token: Option<&'static str>,
    }

    async fn answer(m: &Mock, headers: &HeaderMap, body: Value) -> Result<Json<Value>, StatusCode> {
        if let Some(t) = m.token {
            let ok = headers
                .get("authorization")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h == format!("Bearer {t}"));
            if !ok {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        assert!(body["data"].is_string(), "the premise travels as N-Triples");
        tokio::time::sleep(m.delay).await;
        Ok(Json(Value::Null))
    }

    fn serve(m: Mock) -> String {
        let app = Router::new()
            .route(
                "/v1/reason",
                post(
                    |State(m): State<Mock>, h: HeaderMap, Json(b): Json<Value>| async move {
                        answer(&m, &h, b).await.map(|_| Json(m.reason.clone()))
                    },
                ),
            )
            .route(
                "/v1/check",
                post(
                    |State(m): State<Mock>, h: HeaderMap, Json(b): Json<Value>| async move {
                        answer(&m, &h, b).await.map(|_| Json(m.check.clone()))
                    },
                ),
            )
            .with_state(m);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
        format!("http://{}", rx.recv().unwrap())
    }

    fn cfg(url: String, token: Option<&str>) -> DlConfig {
        let mut c = DlConfig::default().with_backend(DlBackendKind::Sidecar);
        c.sidecar_url = Some(url);
        c.sidecar_token = token.map(str::to_string);
        c.timeout = Duration::from_secs(1);
        c
    }

    fn mock(reason: Value) -> Mock {
        Mock {
            reason,
            check: json!({ "result": "unknown" }),
            delay: Duration::ZERO,
            token: None,
        }
    }

    const DATA: &str = r#"@prefix ex: <http://example.org/> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        ex:A rdfs:subClassOf ex:B . ex:x a ex:A ."#;

    #[test]
    fn dl_sidecar_only_named_entity_triples_reach_the_target() {
        let store = store_with(DATA);
        let url = serve(mock(json!({
            "consistent": true,
            "in_profile": true,
            "complete": true,
            "backend": { "name": "mock", "version": "1" },
            "inferred": "<http://example.org/x> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/B> .
                         <http://example.org/x> <http://example.org/r> _:w .
                         <http://example.org/x> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/A> .
"
        })));
        let run = dl_backend::materialize(&store, &cfg(url, None), None, TG, IdentityPolicy::Full)
            .unwrap();
        assert_eq!(run.backend, "sidecar");
        assert!(run.complete);
        assert_eq!(run.version.as_deref(), Some("mock 1"));
        assert!(ask_in_tg(
            &store,
            "http://example.org/x",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
            "http://example.org/B"
        ));
        assert_eq!(
            count_in_tg(&store),
            1,
            "the blank-node triple and the asserted one stay out of the target graph"
        );
    }

    #[test]
    fn dl_sidecar_inconsistency_is_structured() {
        let store = store_with(DATA);
        let url = serve(mock(json!({
            "consistent": false,
            "inconsistency": "x is in disjoint classes",
            "in_profile": true,
            "inferred": ""
        })));
        match dl_backend::materialize(&store, &cfg(url, None), None, TG, IdentityPolicy::Full) {
            Err(ReasoningError::Inconsistency { rule, detail }) => {
                assert_eq!(rule, "external-reasoner");
                assert!(detail.contains("disjoint"), "{detail}");
            }
            other => panic!("expected an inconsistency, got {other:?}"),
        }
    }

    #[test]
    fn dl_sidecar_timeout_is_unknown() {
        let store = store_with(DATA);
        let mut m = mock(json!({ "consistent": true, "inferred": "" }));
        m.delay = Duration::from_secs(4);
        let url = serve(m);
        let started = std::time::Instant::now();
        let r = dl_backend::materialize(&store, &cfg(url, None), None, TG, IdentityPolicy::Full);
        assert!(matches!(r, Err(ReasoningError::Timeout { .. })), "{r:?}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the client gave up on time"
        );
    }

    #[test]
    fn dl_sidecar_sends_its_token() {
        let store = store_with(DATA);
        let mut m = mock(json!({ "consistent": true, "inferred": "" }));
        m.token = Some("s3cret");
        let url = serve(m);
        let r = dl_backend::materialize(
            &store,
            &cfg(url.clone(), None),
            None,
            TG,
            IdentityPolicy::Full,
        );
        assert!(
            matches!(r, Err(ReasoningError::Backend { .. })),
            "no token: {r:?}"
        );
        dl_backend::materialize(
            &store,
            &cfg(url, Some("s3cret")),
            None,
            TG,
            IdentityPolicy::Full,
        )
        .expect("the token is sent as a bearer token");
    }

    #[test]
    fn dl_sidecar_profile_rejection_is_not_in_profile() {
        let store = store_with(DATA);
        let url = serve(mock(json!({
            "consistent": null,
            "in_profile": false,
            "violations": [{ "rule": "OWL2DLProfile", "detail": "use of reserved vocabulary" }],
            "inferred": ""
        })));
        let r = dl_backend::materialize(&store, &cfg(url, None), None, TG, IdentityPolicy::Full);
        assert!(
            matches!(r, Err(ReasoningError::NotInProfile { .. })),
            "{r:?}"
        );
    }

    #[test]
    fn dl_sidecar_check_passes_false_through() {
        let store = TripleStore::in_memory().unwrap();
        let mut m = mock(json!({}));
        m.check = json!({ "result": "false", "backend": { "name": "mock" } });
        let url = serve(m);
        let task = CheckTask::Entailment {
            conclusion: triples(r#"@prefix ex: <http://example.org/> . ex:x a ex:C ."#),
        };
        let (o, backend, complete) = dl_backend::check(
            &store,
            &cfg(url, None),
            None,
            Some(triples(DATA)),
            &task,
            IdentityPolicy::Full,
        )
        .unwrap();
        assert_eq!((o.result, backend, complete), (Tri::False, "sidecar", true));
    }

    #[test]
    fn dl_input_over_the_cap_is_refused() {
        let store = store_with(DATA);
        let mut c = cfg("http://127.0.0.1:9".into(), None);
        c.max_triples = 1;
        let r = dl_backend::materialize(&store, &c, None, TG, IdentityPolicy::Full);
        assert!(
            matches!(
                r,
                Err(ReasoningError::TooLarge {
                    triples: 2,
                    limit: 1,
                    ..
                })
            ),
            "{r:?}"
        );
    }

    #[test]
    fn dl_sameas_off_with_an_external_backend_is_refused() {
        let store = store_with(DATA);
        let c = cfg("http://127.0.0.1:9".into(), None);
        let r = dl_backend::materialize(&store, &c, None, TG, IdentityPolicy::Off);
        assert!(matches!(r, Err(ReasoningError::NotSupported(_))), "{r:?}");
    }
}

// ═══════════════════════════════════════════════════════════
// Konclude: the OWLlink exchange against a fake binary, and live
// ═══════════════════════════════════════════════════════════

#[cfg(unix)]
mod konclude {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::Duration;

    /// A fake `Konclude` that checks it was handed a functional-syntax file
    /// and answers with canned OWLlink / SPARQL documents.
    fn fake(dir: &std::path::Path, owllink: &str, sparql: &str, sleep: u32) -> PathBuf {
        std::fs::write(dir.join("owllink.xml"), owllink).unwrap();
        std::fs::write(dir.join("sparql.xml"), sparql).unwrap();
        let bin = dir.join("Konclude");
        std::fs::write(
            &bin,
            format!(
                r#"#!/bin/sh
echo "{{info}} >> Reasoner for the SROIQV(D) Description Logic, 64-bit, Version v0.0.0-test - x"
sleep {sleep}
cmd="$1"; shift
out=""; in=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    -i) in="$2"; shift 2 ;;
    *) shift ;;
  esac
done
if [ "$cmd" = "owllinkfile" ]; then
  ofn=$(sed -n 's/.*IRI="file:\([^"]*\)".*/\1/p' "$in" | head -1)
  grep -q '^Ontology(' "$ofn" || {{ echo "no functional syntax in $ofn" >&2; exit 3; }}
  cp "{dir}/owllink.xml" "$out"
else
  cp "{dir}/sparql.xml" "$out"
fi
"#,
                dir = dir.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    fn cfg(bin: PathBuf) -> DlConfig {
        let mut c = DlConfig::default().with_backend(DlBackendKind::Konclude);
        c.konclude_bin = bin.display().to_string();
        c.timeout = Duration::from_secs(10);
        c
    }

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("ots-fake-konclude-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const DATA: &str = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex: <http://example.org/> .
        ex:A rdfs:subClassOf ex:B . ex:a a ex:A ; ex:r ex:b ."#;

    /// Individuals a, b: KB, OK, IsKBSatisfiable, hierarchy, 2 × types,
    /// 2 × same individuals, ReleaseKB.
    const OWLLINK: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<ResponseMessage xmlns="http://www.owllink.org/owllink#" xmlns:owl="http://www.w3.org/2002/07/owl#">
 <KB kb="urn:ots:kb"/>
 <OK/>
 <BooleanResponse result="true"/>
 <ClassHierarchy>
  <ClassSynset><owl:Class IRI="http://www.w3.org/2002/07/owl#Nothing"/></ClassSynset>
  <ClassSubClassesPair>
   <ClassSynset><owl:Class IRI="http://www.w3.org/2002/07/owl#Thing"/></ClassSynset>
   <SubClassSynsets><ClassSynset><owl:Class IRI="http://example.org/B"/></ClassSynset></SubClassSynsets>
  </ClassSubClassesPair>
  <ClassSubClassesPair>
   <ClassSynset><owl:Class IRI="http://example.org/B"/></ClassSynset>
   <SubClassSynsets><ClassSynset><owl:Class IRI="http://example.org/A"/></ClassSynset></SubClassSynsets>
  </ClassSubClassesPair>
 </ClassHierarchy>
 <Classes><owl:Class IRI="http://example.org/A"/><owl:Class IRI="http://example.org/B"/><owl:Class IRI="http://www.w3.org/2002/07/owl#Thing"/></Classes>
 <Classes><owl:Class IRI="http://www.w3.org/2002/07/owl#Thing"/></Classes>
 <IndividualSynonyms><owl:NamedIndividual IRI="http://example.org/a"/></IndividualSynonyms>
 <IndividualSynonyms><owl:NamedIndividual IRI="http://example.org/b"/><owl:NamedIndividual IRI="http://example.org/c"/></IndividualSynonyms>
 <OK/>
</ResponseMessage>"#;

    const SPARQL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<sparql xmlns="http://www.w3.org/2005/sparql-results#">
<head><variable name="x"/><variable name="y"/></head>
<results>
 <result><binding name="x"><uri>http://example.org/b</uri></binding><binding name="y"><uri>http://example.org/a</uri></binding></result>
</results>
</sparql>"#;

    #[test]
    fn dl_konclude_bridge_reads_owllink_answers() {
        let dir = tmp();
        let store = store_with(DATA);
        let run = dl_backend::materialize(
            &store,
            &cfg(fake(&dir, OWLLINK, SPARQL, 0)),
            None,
            TG,
            IdentityPolicy::Full,
        )
        .unwrap();
        assert_eq!(run.backend, "konclude");
        assert_eq!(run.version.as_deref(), Some("Konclude v0.0.0-test"));
        let ty = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        assert!(ask_in_tg(
            &store,
            "http://example.org/a",
            ty,
            "http://example.org/B"
        ));
        assert!(!ask_in_tg(
            &store,
            "http://example.org/a",
            ty,
            "http://www.w3.org/2002/07/owl#Thing"
        ));
        assert!(ask_in_tg(
            &store,
            "http://example.org/b",
            "http://www.w3.org/2002/07/owl#sameAs",
            "http://example.org/c"
        ));
        assert!(
            ask_in_tg(
                &store,
                "http://example.org/b",
                "http://example.org/r",
                "http://example.org/a"
            ),
            "object property assertions come from the SPARQL answers"
        );
        assert!(
            !ask_in_tg(
                &store,
                "http://example.org/A",
                "http://www.w3.org/2000/01/rdf-schema#subClassOf",
                "http://example.org/B"
            ),
            "asserted triples are not copied into the target graph"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dl_konclude_inconsistent_answer_is_an_inconsistency() {
        let dir = tmp();
        let store = store_with(DATA);
        let owllink = r#"<ResponseMessage xmlns="http://www.owllink.org/owllink#">
 <KB kb="urn:ots:kb"/><OK/><BooleanResponse result="false"/>
 <UnsatisfiableKBError error="Ontology 'urn:ots:kb' is inconsistent."/>
 <UnsatisfiableKBError error="x"/><UnsatisfiableKBError error="x"/>
 <UnsatisfiableKBError error="x"/><UnsatisfiableKBError error="x"/><OK/>
</ResponseMessage>"#;
        let r = dl_backend::materialize(
            &store,
            &cfg(fake(&dir, owllink, SPARQL, 0)),
            None,
            TG,
            IdentityPolicy::Full,
        );
        assert!(
            matches!(&r, Err(ReasoningError::Inconsistency { rule, .. }) if rule == "external-reasoner"),
            "{r:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dl_konclude_missing_binary_is_unavailable() {
        let store = store_with(DATA);
        let r = dl_backend::materialize(
            &store,
            &cfg(PathBuf::from("/nonexistent/Konclude")),
            None,
            TG,
            IdentityPolicy::Full,
        );
        assert!(matches!(r, Err(ReasoningError::Unavailable(_))), "{r:?}");
    }

    #[test]
    fn dl_konclude_run_past_the_deadline_is_killed() {
        let dir = tmp();
        let store = store_with(DATA);
        let mut c = cfg(fake(&dir, OWLLINK, SPARQL, 30));
        c.timeout = Duration::from_secs(1);
        let started = std::time::Instant::now();
        let r = dl_backend::materialize(&store, &c, None, TG, IdentityPolicy::Full);
        assert!(matches!(r, Err(ReasoningError::Timeout { .. })), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(dir);
    }

    // ── live: OTS_TEST_KONCLUDE_BIN=/path/to/Konclude ──────────────────────

    fn live() -> Option<DlConfig> {
        let Ok(bin) = std::env::var("OTS_TEST_KONCLUDE_BIN") else {
            assert!(
                std::env::var_os("OTS_TEST_LIVE_REQUIRED").is_none(),
                "OTS_TEST_LIVE_REQUIRED is set but OTS_TEST_KONCLUDE_BIN is not"
            );
            return None;
        };
        let mut c = cfg(PathBuf::from(bin));
        c.timeout = Duration::from_secs(120);
        Some(c)
    }

    fn live_run(ttl: &str) -> Option<(TripleStore, Result<dl_backend::DlRun, ReasoningError>)> {
        let c = live()?;
        let store = store_with(ttl);
        let r = dl_backend::materialize(&store, &c, None, TG, IdentityPolicy::Full);
        Some((store, r))
    }

    const TY: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

    #[test]
    fn dl_konclude_live_existential_witness() {
        let Some((store, r)) = live_run(
            r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
               @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
               @prefix ex: <http://example.org/> .
               ex:A rdfs:subClassOf [ a owl:Restriction ; owl:onProperty ex:r ; owl:someValuesFrom ex:B ] .
               [ a owl:Restriction ; owl:onProperty ex:r ; owl:someValuesFrom ex:B ] rdfs:subClassOf ex:C .
               ex:a a ex:A ."#,
        ) else {
            return;
        };
        r.unwrap();
        assert!(ask_in_tg(
            &store,
            "http://example.org/a",
            TY,
            "http://example.org/C"
        ));
    }

    #[test]
    fn dl_konclude_live_case_split() {
        let Some((store, r)) = live_run(
            r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
               @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
               @prefix ex: <http://example.org/> .
               ex:A rdfs:subClassOf [ a owl:Class ; owl:unionOf ( ex:B ex:C ) ] .
               ex:B rdfs:subClassOf ex:D . ex:C rdfs:subClassOf ex:D .
               ex:a a ex:A ."#,
        ) else {
            return;
        };
        r.unwrap();
        assert!(ask_in_tg(
            &store,
            "http://example.org/a",
            TY,
            "http://example.org/D"
        ));
        assert!(ask_in_tg(
            &store,
            "http://example.org/A",
            "http://www.w3.org/2000/01/rdf-schema#subClassOf",
            "http://example.org/D"
        ));
    }

    #[test]
    fn dl_konclude_live_nominals_give_same_as() {
        let Some((store, r)) = live_run(
            r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
               @prefix ex: <http://example.org/> .
               ex:TheOne owl:equivalentClass [ a owl:Class ; owl:oneOf ( ex:x ) ] .
               ex:a a ex:TheOne ."#,
        ) else {
            return;
        };
        r.unwrap();
        assert!(ask_in_tg(
            &store,
            "http://example.org/a",
            "http://www.w3.org/2002/07/owl#sameAs",
            "http://example.org/x"
        ));
    }

    #[test]
    fn dl_konclude_live_property_chain_assertions() {
        let Some((store, r)) = live_run(
            r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
               @prefix ex: <http://example.org/> .
               ex:hasUncle owl:propertyChainAxiom ( ex:hasParent ex:hasBrother ) .
               ex:John ex:hasParent ex:Mary . ex:Mary ex:hasBrother ex:Bob ."#,
        ) else {
            return;
        };
        r.unwrap();
        assert!(ask_in_tg(
            &store,
            "http://example.org/John",
            "http://example.org/hasUncle",
            "http://example.org/Bob"
        ));
    }

    #[test]
    fn dl_konclude_live_datatype_facet_inconsistency() {
        let Some((_, r)) = live_run(
            r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
               @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
               @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
               @prefix ex: <http://example.org/> .
               ex:age a owl:DatatypeProperty .
               ex:Adult rdfs:subClassOf [ a owl:Restriction ; owl:onProperty ex:age ;
                 owl:allValuesFrom [ a rdfs:Datatype ; owl:onDatatype xsd:integer ;
                   owl:withRestrictions ( [ xsd:minInclusive 18 ] ) ] ] .
               ex:kid a ex:Adult ; ex:age 5 ."#,
        ) else {
            return;
        };
        assert!(
            matches!(r, Err(ReasoningError::Inconsistency { .. })),
            "{r:?}"
        );
    }

    #[test]
    fn dl_konclude_live_checks() {
        let Some(c) = live() else { return };
        let store = TripleStore::in_memory().unwrap();
        let premise = triples(
            r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
               @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
               @prefix ex: <http://example.org/> .
               ex:A rdfs:subClassOf [ a owl:Class ; owl:unionOf ( ex:B ex:C ) ] .
               ex:B rdfs:subClassOf ex:D . ex:C rdfs:subClassOf ex:D .
               ex:E rdfs:subClassOf ex:B , ex:C . ex:B owl:disjointWith ex:C .
               ex:a a ex:A ."#,
        );
        let check = |task: CheckTask| {
            dl_backend::check(
                &store,
                &c,
                None,
                Some(premise.clone()),
                &task,
                IdentityPolicy::Full,
            )
            .unwrap()
            .0
            .result
        };
        assert_eq!(check(CheckTask::Consistency), Tri::True);
        let entails = |ttl: &str| {
            check(CheckTask::Entailment {
                conclusion: triples(&format!(
                    "@prefix ex: <http://example.org/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . {ttl}"
                )),
            })
        };
        assert_eq!(entails("ex:a a ex:D ."), Tri::True);
        assert_eq!(entails("ex:A rdfs:subClassOf ex:D ."), Tri::True);
        assert_eq!(entails("ex:a a ex:B ."), Tri::False);
        assert_eq!(
            check(CheckTask::Satisfiability {
                class: "http://example.org/E".into()
            }),
            Tri::False
        );
        assert_eq!(
            check(CheckTask::Satisfiability {
                class: "http://example.org/A".into()
            }),
            Tri::True
        );
    }
}
