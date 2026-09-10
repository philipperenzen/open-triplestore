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
         PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>\n\
         PREFIX ex:   <http://example.org/>\n\
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

// ─── P1 item 2: composite owl:hasKey, Table 8 datatypes, rule inventory ───────

/// prp-key over a composite key: two individuals merge only when EVERY key
/// property agrees. The old rule matched single-property lists only, so a
/// composite key silently produced no owl:sameAs.
#[test]
fn prp_key_composite_merges_only_when_every_key_property_matches() {
    let store = store_with(
        "ex:Person owl:hasKey ( ex:first ex:last ) .
         ex:a a ex:Person ; ex:first \"Ada\" ; ex:last \"Lovelace\" .
         ex:b a ex:Person ; ex:first \"Ada\" ; ex:last \"Lovelace\" .
         ex:c a ex:Person ; ex:first \"Ada\" ; ex:last \"Byron\" .",
    );
    materialize(&store);
    assert!(
        ask_tg(&store, "ex:a owl:sameAs ex:b"),
        "both key properties agree: a sameAs b"
    );
    assert!(
        !ask_tg(&store, "ex:a owl:sameAs ex:c"),
        "one key property differs: no merge"
    );
    assert!(!ask_tg(&store, "ex:b owl:sameAs ex:c"));
}

/// Two keys on one class are independent: either suffices.
#[test]
fn prp_key_several_keys_each_fire() {
    let store = store_with(
        "ex:Person owl:hasKey ( ex:ssn ) , ( ex:first ex:last ) .
         ex:a a ex:Person ; ex:ssn \"1\" ; ex:first \"A\" ; ex:last \"B\" .
         ex:b a ex:Person ; ex:ssn \"1\" ; ex:first \"X\" ; ex:last \"Y\" .
         ex:c a ex:Person ; ex:ssn \"2\" ; ex:first \"A\" ; ex:last \"B\" .",
    );
    materialize(&store);
    assert!(ask_tg(&store, "ex:a owl:sameAs ex:b"), "same ssn");
    assert!(ask_tg(&store, "ex:a owl:sameAs ex:c"), "same first+last");
}

/// dt-type1: the datatypes of the OWL 2 RL datatype map are rdfs:Datatypes.
#[test]
fn dt_type1_declares_the_datatype_map() {
    let store = store_with("ex:x ex:p 1 .");
    materialize(&store);
    for dt in ["xsd:integer", "xsd:string", "xsd:dateTime", "xsd:boolean"] {
        assert!(
            ask_tg(&store, &format!("{dt} a rdfs:Datatype")),
            "{dt} is declared an rdfs:Datatype"
        );
    }
}

/// dt-not-type: a literal outside its datatype's lexical space is an
/// inconsistency; well-formed literals are not.
#[test]
fn dt_not_type_flags_an_ill_typed_literal() {
    let bad = store_with("ex:x ex:age \"abc\"^^xsd:integer .");
    assert!(
        check_inconsistency(&bad),
        "\"abc\"^^xsd:integer is not in the lexical space of xsd:integer"
    );
    let good = store_with(
        "ex:x ex:age \"42\"^^xsd:integer ; ex:when \"2026-01-01T00:00:00Z\"^^xsd:dateTime ; \
         ex:ok \"true\"^^xsd:boolean ; ex:name \"free text\" ; ex:tag \"hi\"@en .",
    );
    let r = Owl2RLReasoner::new(&good).materialize();
    assert!(r.is_ok(), "well-formed literals are consistent: {r:?}");
}

/// Identity policy: `sameas-off` skips the Table 4 equality rules; the
/// default engine keeps them.
#[test]
fn identity_policy_off_skips_the_equality_rules() {
    use open_triplestore::reasoning::identity::IdentityPolicy;
    let data = "ex:a owl:sameAs ex:b . ex:a ex:p 1 .";
    let full = store_with(data);
    materialize(&full);
    assert!(ask_tg(&full, "ex:b ex:p 1"), "eq-rep-s by default");
    assert!(ask_tg(&full, "ex:b owl:sameAs ex:a"), "eq-sym by default");

    let off = store_with(data);
    Owl2RLReasoner::new(&off)
        .with_identity_policy(IdentityPolicy::Off)
        .materialize()
        .unwrap();
    assert!(!ask_tg(&off, "ex:b ex:p 1"), "sameas-off: no eq-rep-s");
    assert!(
        !ask_tg(&off, "ex:b owl:sameAs ex:a"),
        "sameas-off: no eq-sym"
    );
}

/// Typed correspondences are never identity: none of them feeds eq-rep-*.
#[test]
fn correspondence_predicates_never_feed_the_equality_rules() {
    use open_triplestore::reasoning::identity::CORRESPONDENCE_PREDICATES;
    for pred in CORRESPONDENCE_PREDICATES {
        let store = store_with(&format!("ex:a <{pred}> ex:b . ex:a ex:p 1 ."));
        materialize(&store);
        assert!(
            !ask_tg(&store, "ex:b ex:p 1"),
            "<{pred}> must not propagate properties like owl:sameAs"
        );
    }
}

/// The implemented + unimplemented rule lists are exactly the specification's
/// 78 RL/RDF rules (OWL 2 Profiles §4.3, Tables 4–9) — a two-way inventory,
/// so a rule cannot be added or dropped without the record following.
#[test]
fn rule_inventory_is_the_whole_rl_rule_set() {
    use open_triplestore::reasoning::owl2_rl::{IMPLEMENTED_RULES, UNIMPLEMENTED_RULES};
    const SPEC: &[&str] = &[
        "eq-ref",
        "eq-sym",
        "eq-trans",
        "eq-rep-s",
        "eq-rep-p",
        "eq-rep-o",
        "eq-diff1",
        "eq-diff2",
        "eq-diff3",
        "prp-ap",
        "prp-dom",
        "prp-rng",
        "prp-fp",
        "prp-ifp",
        "prp-irp",
        "prp-symp",
        "prp-asyp",
        "prp-trp",
        "prp-spo1",
        "prp-spo2",
        "prp-eqp1",
        "prp-eqp2",
        "prp-pdw",
        "prp-adp",
        "prp-inv1",
        "prp-inv2",
        "prp-key",
        "prp-npa1",
        "prp-npa2",
        "cls-thing",
        "cls-nothing1",
        "cls-nothing2",
        "cls-int1",
        "cls-int2",
        "cls-uni",
        "cls-com",
        "cls-svf1",
        "cls-svf2",
        "cls-avf",
        "cls-hv1",
        "cls-hv2",
        "cls-maxc1",
        "cls-maxc2",
        "cls-maxqc1",
        "cls-maxqc2",
        "cls-maxqc3",
        "cls-maxqc4",
        "cls-oo",
        "cax-sco",
        "cax-eqc1",
        "cax-eqc2",
        "cax-dw",
        "cax-adc",
        "dt-type1",
        "dt-type2",
        "dt-eq",
        "dt-diff",
        "dt-not-type",
        "scm-cls",
        "scm-sco",
        "scm-eqc1",
        "scm-eqc2",
        "scm-op",
        "scm-dp",
        "scm-spo",
        "scm-eqp1",
        "scm-eqp2",
        "scm-dom1",
        "scm-dom2",
        "scm-rng1",
        "scm-rng2",
        "scm-hv",
        "scm-svf1",
        "scm-svf2",
        "scm-avf1",
        "scm-avf2",
        "scm-int",
        "scm-uni",
    ];
    assert_eq!(SPEC.len(), 78, "the specification lists 78 rules");
    let mut all: Vec<&str> = IMPLEMENTED_RULES.to_vec();
    all.extend(UNIMPLEMENTED_RULES.iter().map(|(r, _)| *r));
    let mut sorted = all.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), all.len(), "a rule is listed twice");
    let mut spec: Vec<&str> = SPEC.to_vec();
    spec.sort_unstable();
    assert_eq!(
        sorted, spec,
        "implemented + unimplemented must be exactly the spec's rules"
    );
    for (r, why) in UNIMPLEMENTED_RULES {
        assert!(!why.is_empty(), "{r} needs a reason");
    }
    for must in ["prp-key", "dt-type1", "dt-not-type", "eq-rep-s", "prp-trp"] {
        assert!(IMPLEMENTED_RULES.contains(&must), "{must} is implemented");
    }
    assert_eq!(IMPLEMENTED_RULES.len(), 63);
}
