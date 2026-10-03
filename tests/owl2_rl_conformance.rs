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
        Err(ReasoningError::Inconsistency { .. })
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
        "cax-adc: two members of an AllDisjointClasses share an instance"
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
    assert_eq!(IMPLEMENTED_RULES.len(), 78);
    assert!(UNIMPLEMENTED_RULES.is_empty(), "every rule runs");
}

// ─── Joins over two derived premises (unscoped runs) ─────────────────────────
//
// Without `with_sources` the rules used to read the unnamed default graph only,
// while every consequence goes to the target graph. A rule whose premises are
// both consequences therefore never fired. The rules now read the default
// graph together with the target graph.

/// The rule an inconsistent run names, or `None` if the run was consistent.
fn inconsistent_rule(store: &TripleStore) -> Option<String> {
    match Owl2RLReasoner::new(store).materialize() {
        Err(ReasoningError::Inconsistency { rule, .. }) => Some(rule),
        Err(e) => panic!("expected an inconsistency or success, got {e}"),
        Ok(_) => None,
    }
}

#[test]
fn prp_trp_closes_a_three_hop_chain() {
    let s = store_with(
        "ex:partOf rdf:type owl:TransitiveProperty . \
         ex:a ex:partOf ex:b . ex:b ex:partOf ex:c . ex:c ex:partOf ex:d .",
    );
    materialize(&s);
    assert!(
        ask_tg(&s, "ex:a ex:partOf ex:d ."),
        "prp-trp: a→d joins two derived links (a→c, b→d) or a derived and an asserted one"
    );
}

#[test]
fn eq_trans_closes_a_three_link_same_as_chain() {
    let s = store_with("ex:a owl:sameAs ex:b . ex:b owl:sameAs ex:c . ex:c owl:sameAs ex:d .");
    materialize(&s);
    assert!(ask_tg(&s, "ex:a owl:sameAs ex:d ."), "eq-trans: a = d");
    assert!(
        ask_tg(&s, "ex:d owl:sameAs ex:a ."),
        "eq-sym over a derived a = d"
    );
}

/// prp-eqp1/2 are listed as subsumed by scm-eqp1/2 + prp-spo1: that only holds
/// when prp-spo1 sees the sub-property axioms scm-eqp derived.
#[test]
fn equivalent_property_propagates_both_ways() {
    let s = store_with(
        "ex:p owl:equivalentProperty ex:q . \
         ex:x ex:p ex:y . ex:u ex:q ex:v .",
    );
    materialize(&s);
    assert!(ask_tg(&s, "ex:x ex:q ex:y ."), "prp-eqp1");
    assert!(ask_tg(&s, "ex:u ex:p ex:v ."), "prp-eqp2");
}

#[test]
fn range_applies_through_a_sub_property() {
    let s = store_with(
        "ex:hasMother rdfs:subPropertyOf ex:hasParent . \
         ex:hasParent rdfs:range ex:Person . \
         ex:sam ex:hasMother ex:ann .",
    );
    materialize(&s);
    assert!(
        ask_tg(&s, "ex:ann rdf:type ex:Person ."),
        "prp-rng over the derived ex:sam ex:hasParent ex:ann (or scm-rng2's derived range)"
    );
}

#[test]
fn derived_membership_of_owl_nothing_is_inconsistent() {
    let s = store_with(
        "ex:Unicorn rdfs:subClassOf ex:Impossible . \
         ex:Impossible rdfs:subClassOf owl:Nothing . \
         ex:u rdf:type ex:Unicorn .",
    );
    assert_eq!(
        inconsistent_rule(&s).as_deref(),
        Some("cls-nothing2"),
        "ex:u rdf:type owl:Nothing is derived by cax-sco, and cls-nothing2 must see it"
    );
}

#[test]
fn derived_same_as_against_different_from_is_inconsistent() {
    let s = store_with(
        "ex:hasBirthMother rdf:type owl:FunctionalProperty . \
         ex:kim ex:hasBirthMother ex:m1 . ex:kim ex:hasBirthMother ex:m2 . \
         ex:m1 owl:differentFrom ex:m2 .",
    );
    assert_eq!(
        inconsistent_rule(&s).as_deref(),
        Some("eq-diff1"),
        "prp-fp derives ex:m1 owl:sameAs ex:m2, and eq-diff1 must see it"
    );
}

/// A run that reaches its iteration limit before the fixed point is an error,
/// not a silently partial graph.
#[test]
fn hitting_the_iteration_limit_is_an_error() {
    let s = store_with(
        "ex:partOf rdf:type owl:TransitiveProperty . \
         ex:a ex:partOf ex:b . ex:b ex:partOf ex:c . ex:c ex:partOf ex:d . ex:d ex:partOf ex:e .",
    );
    match Owl2RLReasoner::new(&s).with_max_iterations(1).materialize() {
        Err(ReasoningError::NotConverged { iterations, .. }) => assert_eq!(iterations, 1),
        other => panic!("expected NotConverged, got {other:?}"),
    }
    // With room to finish, the same input converges.
    let r = Owl2RLReasoner::new(&s).materialize().unwrap();
    assert!(r.iterations > 1, "{r:?}");
    assert!(ask_tg(&s, "ex:a ex:partOf ex:e ."));
}

// ─── Remaining non-datatype rules (Tables 4–7, 9) ────────────────────────────

#[test]
fn eq_diff2_all_different_members_with_a_same_as_pair_is_inconsistent() {
    let s = store_with(
        "[] rdf:type owl:AllDifferent ; owl:members ( ex:a ex:b ex:c ) . \
         ex:a owl:sameAs ex:c .",
    );
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("eq-diff2"));
}

#[test]
fn eq_diff3_distinct_members_with_a_derived_same_as_is_inconsistent() {
    let s = store_with(
        "[] rdf:type owl:AllDifferent ; owl:distinctMembers ( ex:a ex:b ex:c ) . \
         ex:mother rdf:type owl:FunctionalProperty . \
         ex:k ex:mother ex:b . ex:k ex:mother ex:c .",
    );
    assert_eq!(
        inconsistent_rule(&s).as_deref(),
        Some("eq-diff3"),
        "prp-fp derives ex:b owl:sameAs ex:c"
    );
}

/// An individual listed twice in owl:AllDifferent is different from itself:
/// eq-ref makes it owl:sameAs itself whether or not those triples are written.
#[test]
fn eq_diff2_a_member_listed_twice_is_inconsistent() {
    let s = store_with("[] rdf:type owl:AllDifferent ; owl:members ( ex:a ex:b ex:a ) .");
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("eq-diff2"));
}

#[test]
fn all_different_members_without_same_as_are_consistent() {
    let s = store_with(
        "[] rdf:type owl:AllDifferent ; owl:members ( ex:a ex:b ex:c ) . \
         ex:a ex:knows ex:b .",
    );
    assert_eq!(inconsistent_rule(&s), None);
}

#[test]
fn different_from_itself_is_inconsistent() {
    let s = store_with("ex:a owl:differentFrom ex:a .");
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("eq-diff1"));
}

#[test]
fn prp_pdw_shared_pair_is_inconsistent() {
    let s = store_with(
        "ex:likes owl:propertyDisjointWith ex:hates . \
         ex:sam ex:likes ex:kale . ex:sam ex:hates ex:kale .",
    );
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("prp-pdw"));
    let ok = store_with(
        "ex:likes owl:propertyDisjointWith ex:hates . \
         ex:sam ex:likes ex:kale . ex:sam ex:hates ex:beet .",
    );
    assert_eq!(inconsistent_rule(&ok), None);
}

#[test]
fn prp_adp_shared_pair_is_inconsistent() {
    let s = store_with(
        "[] rdf:type owl:AllDisjointProperties ; owl:members ( ex:p ex:q ex:r ) . \
         ex:sub rdfs:subPropertyOf ex:r . \
         ex:x ex:q ex:y . ex:x ex:sub ex:y .",
    );
    assert_eq!(
        inconsistent_rule(&s).as_deref(),
        Some("prp-adp"),
        "ex:x ex:r ex:y is derived by prp-spo1 and clashes with ex:q"
    );
    let ok = store_with(
        "[] rdf:type owl:AllDisjointProperties ; owl:members ( ex:p ex:q ex:r ) . \
         ex:x ex:p ex:y . ex:x ex:q ex:z .",
    );
    assert_eq!(inconsistent_rule(&ok), None);
}

#[test]
fn prp_ap_declares_the_built_in_annotation_properties() {
    let s = store_with("");
    materialize(&s);
    for ap in [
        "rdfs:label",
        "rdfs:comment",
        "rdfs:seeAlso",
        "rdfs:isDefinedBy",
        "owl:deprecated",
        "owl:versionInfo",
        "owl:priorVersion",
        "owl:backwardCompatibleWith",
        "owl:incompatibleWith",
    ] {
        assert!(
            ask_tg(&s, &format!("{ap} rdf:type owl:AnnotationProperty .")),
            "prp-ap: {ap}"
        );
    }
}

#[test]
fn cls_thing_and_cls_nothing1_declare_the_two_built_in_classes() {
    let s = store_with("");
    materialize(&s);
    assert!(ask_tg(&s, "owl:Thing rdf:type owl:Class ."), "cls-thing");
    assert!(
        ask_tg(&s, "owl:Nothing rdf:type owl:Class ."),
        "cls-nothing1"
    );
    // scm-cls then applies to both.
    assert!(ask_tg(&s, "owl:Nothing rdfs:subClassOf owl:Thing ."));
}

#[test]
fn scm_cls_derives_all_four_consequents() {
    let s = store_with("ex:Person rdf:type owl:Class .");
    materialize(&s);
    assert!(ask_tg(&s, "ex:Person rdfs:subClassOf ex:Person ."));
    assert!(ask_tg(&s, "ex:Person owl:equivalentClass ex:Person ."));
    assert!(ask_tg(&s, "ex:Person rdfs:subClassOf owl:Thing ."));
    assert!(ask_tg(&s, "owl:Nothing rdfs:subClassOf ex:Person ."));
}

#[test]
fn scm_op_and_scm_dp_make_properties_their_own_sub_and_equivalent() {
    let s =
        store_with("ex:knows rdf:type owl:ObjectProperty . ex:age rdf:type owl:DatatypeProperty .");
    materialize(&s);
    for p in ["ex:knows", "ex:age"] {
        assert!(
            ask_tg(&s, &format!("{p} rdfs:subPropertyOf {p} .")),
            "{p} ⊑ {p}"
        );
        assert!(
            ask_tg(&s, &format!("{p} owl:equivalentProperty {p} .")),
            "{p} ≡ {p}"
        );
    }
}

/// D2: eq-ref (every term owl:sameAs itself) is opt-in — it adds about one
/// triple per term — and `sameas-off` skips it like the other equality rules.
#[test]
fn eq_ref_is_opt_in() {
    let s = store_with("ex:x ex:p ex:y . ex:x ex:age 7 .");
    materialize(&s);
    assert!(
        !ask_tg(&s, "ex:x owl:sameAs ex:x ."),
        "eq-ref is off by default"
    );

    let s = store_with("ex:x ex:p ex:y . ex:x ex:age 7 .");
    Owl2RLReasoner::new(&s)
        .with_eq_ref(true)
        .materialize()
        .unwrap();
    for t in ["ex:x", "ex:p", "ex:y", "ex:age"] {
        assert!(ask_tg(&s, &format!("{t} owl:sameAs {t} .")), "eq-ref: {t}");
    }

    let s = store_with("ex:x ex:p ex:y .");
    Owl2RLReasoner::new(&s)
        .with_eq_ref(true)
        .with_identity_policy(open_triplestore::reasoning::identity::IdentityPolicy::Off)
        .materialize()
        .unwrap();
    assert!(
        !ask_tg(&s, "ex:x owl:sameAs ex:x ."),
        "sameas-off skips eq-ref"
    );
}

#[test]
fn cls_int1_matches_an_intersection_of_three() {
    let s = store_with(
        "ex:C owl:intersectionOf ( ex:A ex:B ex:D ) . \
         ex:x rdf:type ex:A , ex:B , ex:D . \
         ex:y rdf:type ex:A , ex:B .",
    );
    materialize(&s);
    assert!(
        ask_tg(&s, "ex:x rdf:type ex:C ."),
        "cls-int1 over three members"
    );
    assert!(!ask_tg(&s, "ex:y rdf:type ex:C ."), "ex:y lacks ex:D");
}

#[test]
fn prp_spo2_follows_a_chain_of_three() {
    let s = store_with(
        "ex:greatGrandParent owl:propertyChainAxiom ( ex:parent ex:parent ex:parent ) . \
         ex:a ex:parent ex:b . ex:b ex:parent ex:c . ex:c ex:parent ex:d .",
    );
    materialize(&s);
    assert!(ask_tg(&s, "ex:a ex:greatGrandParent ex:d ."));
    assert!(!ask_tg(&s, "ex:a ex:greatGrandParent ex:c ."));
}

/// Blank-node class expressions are members like any other: scm-int, scm-uni
/// and cax-adc used to keep IRI members only.
#[test]
fn blank_node_members_of_intersections_unions_and_disjoint_lists_count() {
    let s = store_with(
        "ex:C owl:intersectionOf ( ex:A [ owl:onProperty ex:p ; owl:someValuesFrom ex:B ] ) . \
         ex:U owl:unionOf ( ex:A [ owl:onProperty ex:q ; owl:hasValue ex:v ] ) .",
    );
    materialize(&s);
    // The consequence is in the target graph; the restriction it names is
    // the asserted blank node in the default graph.
    let ex = "http://example.org/";
    let owl = "http://www.w3.org/2002/07/owl#";
    let sco = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
    assert!(
        ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <{ex}C> <{sco}> ?r }} \
                 ?r <{owl}someValuesFrom> <{ex}B> . FILTER(isBlank(?r)) }}"
            )
        ),
        "scm-int"
    );
    assert!(
        ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ ?r <{sco}> <{ex}U> }} \
                 ?r <{owl}hasValue> <{ex}v> . FILTER(isBlank(?r)) }}"
            )
        ),
        "scm-uni"
    );

    let s = store_with(
        "[] rdf:type owl:AllDisjointClasses ; \
            owl:members ( ex:A [ owl:onProperty ex:p ; owl:hasValue ex:v ] ) . \
         ex:x rdf:type ex:A ; ex:p ex:v .",
    );
    assert_eq!(
        inconsistent_rule(&s).as_deref(),
        Some("cax-adc"),
        "cax-adc pairs ex:A with the blank restriction, cls-hv2 types ex:x with it"
    );
}

/// The spec's prp-npa rules have no `rdf:type owl:NegativePropertyAssertion`
/// premise: the three NPA properties alone are enough.
#[test]
fn prp_npa_needs_no_type_triple() {
    let s = store_with(
        "[] owl:sourceIndividual ex:alice ; owl:assertionProperty ex:knows ; \
            owl:targetIndividual ex:bob . \
         ex:alice ex:knows ex:bob .",
    );
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("prp-npa1"));
    let s = store_with(
        "[] owl:sourceIndividual ex:alice ; owl:assertionProperty ex:age ; \
            owl:targetValue 30 . \
         ex:alice ex:age 30 .",
    );
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("prp-npa2"));
}

/// prp-trp on a cycle derives the reflexive triple; it used to be filtered out.
#[test]
fn prp_trp_closes_a_cycle() {
    let s = store_with(
        "ex:near rdf:type owl:TransitiveProperty . ex:a ex:near ex:b . ex:b ex:near ex:a .",
    );
    materialize(&s);
    assert!(ask_tg(&s, "ex:a ex:near ex:a ."));
    assert!(ask_tg(&s, "ex:b ex:near ex:b ."));
}

/// cls-hv1/2 run once (the duplicate prp-hv1/2 copies are gone): both
/// directions still hold.
#[test]
fn has_value_types_and_fills_in_both_directions() {
    let s = store_with(
        "ex:Dutch owl:equivalentClass [ owl:onProperty ex:nationality ; owl:hasValue ex:NL ] . \
         ex:a rdf:type ex:Dutch . ex:b ex:nationality ex:NL .",
    );
    materialize(&s);
    assert!(ask_tg(&s, "ex:a ex:nationality ex:NL ."), "cls-hv1");
    assert!(ask_tg(&s, "ex:b rdf:type ex:Dutch ."), "cls-hv2 + cax-eqc");
}

// ─── Inverse property expressions ([ owl:inverseOf P ]) ──────────────────────
//
// An inverse property expression is a blank node, and no RDF triple can have
// a blank-node predicate. A premise `?u PE ?v` therefore reads `?v P ?u`, and
// a conclusion `(x, [inverseOf P], y)` is written as `y P x`.

#[test]
fn inverse_in_a_some_values_from_restriction() {
    let s = store_with(
        "[ owl:onProperty [ owl:inverseOf ex:hasParent ] ; owl:someValuesFrom ex:Person ] \
             rdfs:subClassOf ex:Parent . \
         ex:kid ex:hasParent ex:mum . ex:kid rdf:type ex:Person .",
    );
    materialize(&s);
    assert!(
        ask_tg(&s, "ex:mum rdf:type ex:Parent ."),
        "cls-svf1 over an inverse"
    );
}

#[test]
fn inverse_in_all_values_from_and_has_value_restrictions() {
    let s = store_with(
        "ex:Pet rdfs:subClassOf [ owl:onProperty [ owl:inverseOf ex:owns ] ; owl:allValuesFrom ex:Owner ] . \
         ex:rex rdf:type ex:Pet . ex:ann ex:owns ex:rex . \
         ex:AcmeAsset owl:equivalentClass \
             [ owl:onProperty [ owl:inverseOf ex:owns ] ; owl:hasValue ex:acme ] . \
         ex:truck rdf:type ex:AcmeAsset . ex:acme ex:owns ex:van .",
    );
    materialize(&s);
    assert!(
        ask_tg(&s, "ex:ann rdf:type ex:Owner ."),
        "cls-avf over an inverse"
    );
    assert!(
        ask_tg(&s, "ex:acme ex:owns ex:truck ."),
        "cls-hv1 writes the inverse head"
    );
    assert!(
        ask_tg(&s, "ex:van rdf:type ex:AcmeAsset ."),
        "cls-hv2 over an inverse"
    );
}

#[test]
fn inverse_in_a_property_chain() {
    let s = store_with(
        "ex:sibling owl:propertyChainAxiom ( ex:hasParent [ owl:inverseOf ex:hasParent ] ) . \
         ex:a ex:hasParent ex:m . ex:b ex:hasParent ex:m .",
    );
    materialize(&s);
    assert!(
        ask_tg(&s, "ex:a ex:sibling ex:b ."),
        "prp-spo2 over an inverse link"
    );
    assert!(ask_tg(&s, "ex:b ex:sibling ex:a ."));
}

#[test]
fn inverse_as_sub_and_super_property() {
    let s = store_with(
        "[ owl:inverseOf ex:hasParent ] rdfs:subPropertyOf ex:hasChild . \
         ex:hasGuardian rdfs:subPropertyOf [ owl:inverseOf ex:guards ] . \
         ex:kid ex:hasParent ex:mum . ex:kid ex:hasGuardian ex:aunt .",
    );
    materialize(&s);
    assert!(
        ask_tg(&s, "ex:mum ex:hasChild ex:kid ."),
        "inverse sub-property"
    );
    assert!(
        ask_tg(&s, "ex:aunt ex:guards ex:kid ."),
        "inverse super-property"
    );
}

/// A key member that is an inverse expression is a real condition. It used to
/// be dropped, which made the key weaker and merged individuals it must not.
#[test]
fn inverse_in_a_key() {
    let s = store_with(
        "ex:Account owl:hasKey ( ex:bank [ owl:inverseOf ex:holds ] ) . \
         ex:a1 rdf:type ex:Account ; ex:bank ex:b . ex:alice ex:holds ex:a1 . \
         ex:a2 rdf:type ex:Account ; ex:bank ex:b . ex:alice ex:holds ex:a2 . \
         ex:a3 rdf:type ex:Account ; ex:bank ex:b . ex:bob ex:holds ex:a3 .",
    );
    materialize(&s);
    assert!(
        ask_tg(&s, "ex:a1 owl:sameAs ex:a2 ."),
        "same bank and holder"
    );
    assert!(
        !ask_tg(&s, "ex:a1 owl:sameAs ex:a3 ."),
        "same bank, different holder: the inverse key member must count"
    );
}

#[test]
fn inverse_property_characteristics_and_domain() {
    let s = store_with(
        "[ owl:inverseOf ex:employs ] rdf:type owl:FunctionalProperty . \
         [ owl:inverseOf ex:employs ] rdfs:domain ex:Employee . \
         ex:acme ex:employs ex:kim . ex:acme2 ex:employs ex:kim .",
    );
    materialize(&s);
    assert!(
        ask_tg(&s, "ex:kim rdf:type ex:Employee ."),
        "prp-dom over an inverse"
    );
    assert!(
        ask_tg(&s, "ex:acme owl:sameAs ex:acme2 ."),
        "prp-fp over an inverse (ex:employs is inverse-functional)"
    );
}

// ─── Table 8 over data values (dt-type2, dt-eq, dt-diff) ─────────────────────
//
// The three rules conclude triples with a literal subject. The engine works
// out their RDF-representable consequences from the literals' data values.

/// dt-eq + eq-rep-o: equal values written with different datatypes get each
/// other's triples.
#[test]
fn dt_eq_copies_triples_to_equal_valued_literals() {
    let s = store_with("ex:x ex:p \"1\"^^xsd:integer . ex:y ex:q \"1.0\"^^xsd:decimal .");
    materialize(&s);
    assert!(
        ask_tg(&s, "ex:x ex:p ?o . FILTER(datatype(?o) = xsd:decimal)"),
        "the integer 1 and the decimal 1.0 are one value"
    );
    assert!(
        ask_tg(&s, "ex:y ex:q ?o . FILTER(datatype(?o) = xsd:integer)"),
        "and the copy goes both ways"
    );
    let s = store_with("ex:x ex:p \"1\"^^xsd:integer . ex:y ex:q \"1.5\"^^xsd:decimal .");
    materialize(&s);
    assert!(
        !ask_tg(&s, "ex:x ex:p ?o . FILTER(datatype(?o) = xsd:decimal)"),
        "different values are not copied"
    );
}

/// cls-hv2 by value: a hasValue restriction matches an equal value written
/// differently.
#[test]
fn has_value_matches_by_value() {
    let s = store_with(
        "ex:One owl:onProperty ex:n ; owl:hasValue \"1\"^^xsd:integer . \
         ex:a ex:n \"1.0\"^^xsd:decimal . ex:b ex:n \"2\"^^xsd:integer .",
    );
    materialize(&s);
    assert!(ask_tg(&s, "ex:a rdf:type ex:One ."), "1.0 is the value 1");
    assert!(!ask_tg(&s, "ex:b rdf:type ex:One ."), "2 is not");
}

/// prp-key by value: key values that are one value make the individuals
/// the same.
#[test]
fn has_key_matches_by_value() {
    let s = store_with(
        "ex:C owl:hasKey ( ex:id ) . ex:a a ex:C ; ex:id \"7\"^^xsd:integer . \
         ex:b a ex:C ; ex:id \"7.0\"^^xsd:decimal . ex:c a ex:C ; ex:id \"8\"^^xsd:integer .",
    );
    materialize(&s);
    assert!(
        ask_tg(&s, "ex:a owl:sameAs ex:b ."),
        "7 and 7.0 are one key value"
    );
    assert!(!ask_tg(&s, "ex:a owl:sameAs ex:c ."), "8 is another");
}

/// prp-npa2 by value: a negative data assertion is violated by an equal
/// value written differently.
#[test]
fn negative_data_assertion_matches_by_value() {
    let s = store_with(
        "[] owl:sourceIndividual ex:i ; owl:assertionProperty ex:n ; \
            owl:targetValue \"5\"^^xsd:integer . \
         ex:i ex:n \"5.0\"^^xsd:decimal .",
    );
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("prp-npa2"));
    let s = store_with(
        "[] owl:sourceIndividual ex:i ; owl:assertionProperty ex:n ; \
            owl:targetValue \"5\"^^xsd:integer . \
         ex:i ex:n \"6\"^^xsd:integer .",
    );
    assert_eq!(inconsistent_rule(&s), None);
}

/// dt-diff + eq-diff1: two different values of a functional data property
/// are an inconsistency; one value written two ways is not.
#[test]
fn functional_data_property_with_two_values_is_inconsistent() {
    let s = store_with("ex:age a owl:FunctionalProperty . ex:x ex:age 41, 42 .");
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("dt-diff"));
    let s = store_with(
        "ex:age a owl:FunctionalProperty . ex:x ex:age \"1\"^^xsd:integer, \"1.0\"^^xsd:decimal .",
    );
    assert_eq!(inconsistent_rule(&s), None, "one value, two lexical forms");
}

/// cls-maxc2 over a data property: a maximum cardinality of one with two
/// different values is an inconsistency.
#[test]
fn max_cardinality_one_with_two_data_values_is_inconsistent() {
    let s = store_with(
        "ex:R owl:onProperty ex:code ; owl:maxCardinality \"1\"^^xsd:nonNegativeInteger . \
         ex:x a ex:R ; ex:code \"a\", \"b\" .",
    );
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("dt-diff"));
}

/// dt-not-type through prp-rng: a value outside its property's datatype
/// range is an inconsistency.
#[test]
fn value_outside_a_datatype_range_is_inconsistent() {
    let s = store_with("ex:age rdfs:range xsd:integer . ex:x ex:age \"forty\" .");
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("dt-not-type"));
    let s = store_with("ex:age rdfs:range xsd:decimal . ex:x ex:age 40 .");
    assert_eq!(inconsistent_rule(&s), None, "an integer is a decimal value");
    let s = store_with("ex:age rdfs:range xsd:nonNegativeInteger . ex:x ex:age -1 .");
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("dt-not-type"));
}

/// D4, the storage limit: oxigraph keeps `"300"^^xsd:byte` as the integer
/// 300, so the literal itself is not ill-typed after storage; a range of
/// `xsd:byte` still rejects the value. (Storage that keeps lexical forms
/// and datatypes would turn the first case into `dt-not-type`.)
#[test]
fn out_of_range_byte_is_caught_by_value_not_by_its_stored_datatype() {
    let s = store_with("ex:x ex:n \"300\"^^xsd:byte .");
    assert_eq!(inconsistent_rule(&s), None, "stored as the integer 300");
    let s = store_with("ex:n rdfs:range xsd:byte . ex:x ex:n 300 .");
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("dt-not-type"));
}

/// dt-type2 feeding cls-svf1: a data value types the subject of a
/// someValuesFrom restriction on a datatype that holds the value.
#[test]
fn data_values_type_some_values_from_subjects() {
    let s = store_with(
        "ex:HasCount owl:onProperty ex:n ; owl:someValuesFrom xsd:decimal . \
         ex:Flagged owl:onProperty ex:n ; owl:someValuesFrom xsd:boolean . \
         ex:a ex:n 3 . ex:b ex:n \"three\" .",
    );
    materialize(&s);
    assert!(
        ask_tg(&s, "ex:a rdf:type ex:HasCount ."),
        "3 is a decimal value"
    );
    assert!(
        !ask_tg(&s, "ex:b rdf:type ex:HasCount ."),
        "a string is not"
    );
    assert!(!ask_tg(&s, "ex:a rdf:type ex:Flagged ."), "3 is no boolean");
}

/// A double is not a decimal: the float/double value spaces are disjoint
/// from the decimal one.
#[test]
fn double_values_are_not_decimal_values() {
    let s = store_with(
        "ex:HasDecimal owl:onProperty ex:n ; owl:someValuesFrom xsd:decimal . \
         ex:a ex:n \"1.0E0\"^^xsd:double .",
    );
    materialize(&s);
    assert!(!ask_tg(&s, "ex:a rdf:type ex:HasDecimal ."));
    let s = store_with(
        "ex:age a owl:FunctionalProperty . ex:x ex:age \"1\"^^xsd:integer, \"1.0E0\"^^xsd:double .",
    );
    assert_eq!(
        inconsistent_rule(&s).as_deref(),
        Some("dt-diff"),
        "1 and 1.0E0 differ"
    );
}

/// cls-nothing2 over a literal: a range of owl:Nothing has no values.
#[test]
fn a_literal_in_owl_nothing_is_inconsistent() {
    let s = store_with("ex:p rdfs:range owl:Nothing . ex:x ex:p \"v\" .");
    assert_eq!(inconsistent_rule(&s).as_deref(), Some("cls-nothing2"));
}

// ─── Differential test against a generalized-triple reference evaluator ──────
//
// `support/rl_reference.rs` applies the 78 RL/RDF rules naively over
// generalized triples (literal subjects included). For seeded random graphs
// the engine must agree with it on consistency and, when consistent, its
// output must be exactly the RDF-representable part of the reference
// closure. Reflexive `x owl:sameAs x` triples are left out on both sides:
// they are eq-ref conclusions, which are opt-in (decision D2).
//
// The generator keeps object and data properties apart (as OWL 2's typing
// does): object properties link individuals, data properties link an
// individual to a literal. Graphs that use a data property as an object
// property produce conclusions through literal-subject triples, which the
// engine does not simulate (see `docs/owl2-rl.md`).

#[path = "support/rl_reference.rs"]
mod rl_reference;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
    /// `k` distinct picks.
    fn distinct<'a>(&mut self, xs: &[&'a str], k: usize) -> Vec<&'a str> {
        let mut v: Vec<&'a str> = xs.to_vec();
        for i in 0..v.len() {
            let j = i + self.below(v.len() - i);
            v.swap(i, j);
        }
        v.truncate(k.min(xs.len()));
        v
    }
}

const CLASSES: &[&str] = &["ex:C0", "ex:C1", "ex:C2", "ex:C3"];
const OPROPS: &[&str] = &["ex:P0", "ex:P1", "ex:P2"];
const DPROPS: &[&str] = &["ex:D0", "ex:D1"];
const INDS: &[&str] = &["ex:I0", "ex:I1", "ex:I2", "ex:I3"];
const LITS: &[&str] = &[
    "\"0\"^^xsd:integer",
    "\"1\"^^xsd:integer",
    "\"1.0\"^^xsd:decimal",
    "\"1.5\"^^xsd:decimal",
    "\"x1\"",
    "\"y2\"",
    "\"true\"^^xsd:boolean",
];
const DTYPES: &[&str] = &[
    "xsd:integer",
    "xsd:decimal",
    "xsd:string",
    "xsd:boolean",
    "xsd:nonNegativeInteger",
    "xsd:positiveInteger",
    "xsd:byte",
];

fn card(r: &mut Rng) -> &'static str {
    if r.chance(50) {
        "\"0\"^^xsd:nonNegativeInteger"
    } else {
        "\"1\"^^xsd:nonNegativeInteger"
    }
}

/// A restriction, written as an inline blank node.
fn restriction(r: &mut Rng) -> String {
    let data = r.chance(40);
    let p = if data { r.pick(DPROPS) } else { r.pick(OPROPS) };
    let filler = |r: &mut Rng| -> String {
        if data {
            r.pick(DTYPES).to_string()
        } else if r.chance(20) {
            "owl:Thing".to_string()
        } else {
            r.pick(CLASSES).to_string()
        }
    };
    let body = match r.below(5) {
        0 => format!("owl:someValuesFrom {}", filler(r)),
        1 => format!("owl:allValuesFrom {}", filler(r)),
        2 => {
            let v = if data { r.pick(LITS) } else { r.pick(INDS) };
            format!("owl:hasValue {v}")
        }
        3 => format!("owl:maxCardinality {}", card(r)),
        _ => format!(
            "owl:maxQualifiedCardinality {} ; owl:onClass {}",
            card(r),
            filler(r)
        ),
    };
    format!("[ owl:onProperty {p} ; {body} ]")
}

/// A class expression: mostly named, sometimes a restriction.
fn class(r: &mut Rng) -> String {
    if r.chance(35) {
        restriction(r)
    } else {
        r.pick(CLASSES).to_string()
    }
}

fn list(xs: &[&str]) -> String {
    format!("( {} )", xs.join(" "))
}

/// One random axiom, as Turtle.
fn axiom(r: &mut Rng) -> String {
    let c = |r: &mut Rng| r.pick(CLASSES);
    let p = |r: &mut Rng| r.pick(OPROPS);
    let d = |r: &mut Rng| r.pick(DPROPS);
    match r.below(27) {
        0 => format!("{} rdfs:subClassOf {} .", c(r), class(r)),
        1 => format!("{} rdfs:subClassOf {} .", class(r), c(r)),
        2 => format!("{} owl:equivalentClass {} .", c(r), class(r)),
        3 => format!("{} owl:disjointWith {} .", c(r), c(r)),
        4 => format!("{} owl:complementOf {} .", c(r), c(r)),
        5 => format!("{} rdfs:subPropertyOf {} .", p(r), p(r)),
        6 => format!("{} owl:equivalentProperty {} .", p(r), p(r)),
        7 => format!("{} owl:inverseOf {} .", p(r), p(r)),
        8 => format!("{} owl:propertyDisjointWith {} .", p(r), p(r)),
        9 => {
            let prop = if r.chance(50) { p(r) } else { d(r) };
            format!("{prop} rdfs:domain {} .", class(r))
        }
        10 => format!("{} rdfs:range {} .", p(r), class(r)),
        11 => format!("{} rdfs:range {} .", d(r), r.pick(DTYPES)),
        12 => {
            let kinds = [
                "owl:FunctionalProperty",
                "owl:InverseFunctionalProperty",
                "owl:SymmetricProperty",
                "owl:TransitiveProperty",
                "owl:IrreflexiveProperty",
                "owl:AsymmetricProperty",
            ];
            format!("{} a {} .", p(r), r.pick(&kinds))
        }
        13 => {
            let k = r.pick(&["owl:FunctionalProperty", "owl:InverseFunctionalProperty"]);
            format!("{} a {k} .", d(r))
        }
        14 => {
            let n = 2 + r.below(2);
            let chain: Vec<&str> = (0..n).map(|_| p(r)).collect();
            format!("{} owl:propertyChainAxiom {} .", p(r), list(&chain))
        }
        15 => {
            let rel = r.pick(&["rdfs:subPropertyOf", "owl:equivalentProperty"]);
            format!("{} {rel} {} .", d(r), d(r))
        }
        16 => {
            let n = 2 + r.below(2);
            let ms: Vec<String> = (0..n).map(|_| class(r)).collect();
            format!("{} owl:intersectionOf ( {} ) .", c(r), ms.join(" "))
        }
        17 => format!("{} owl:unionOf {} .", c(r), list(&r.distinct(CLASSES, 2))),
        18 => format!("{} owl:oneOf {} .", c(r), list(&r.distinct(INDS, 2))),
        19 => {
            let keys: Vec<&str> = (0..1 + r.below(2))
                .map(|_| if r.chance(50) { p(r) } else { d(r) })
                .collect();
            format!("{} owl:hasKey {} .", c(r), list(&keys))
        }
        20 => {
            let k = 2 + r.below(2);
            let ms = r.distinct(CLASSES, k);
            format!("[] a owl:AllDisjointClasses ; owl:members {} .", list(&ms))
        }
        21 => format!(
            "[] a owl:AllDisjointProperties ; owl:members {} .",
            list(&r.distinct(OPROPS, 2))
        ),
        22 => {
            let pred = r.pick(&["owl:members", "owl:distinctMembers"]);
            let k = 2 + r.below(2);
            let ms = r.distinct(INDS, k);
            format!("[] a owl:AllDifferent ; {pred} {} .", list(&ms))
        }
        23 => format!(
            "[] owl:sourceIndividual {} ; owl:assertionProperty {} ; owl:targetIndividual {} .",
            r.pick(INDS),
            p(r),
            r.pick(INDS)
        ),
        24 => format!(
            "[] owl:sourceIndividual {} ; owl:assertionProperty {} ; owl:targetValue {} .",
            r.pick(INDS),
            d(r),
            r.pick(LITS)
        ),
        25 => {
            let ab = r.distinct(INDS, 2);
            format!("{} owl:sameAs {} .", ab[0], ab[1])
        }
        _ => {
            let ab = r.distinct(INDS, 2);
            format!("{} owl:differentFrom {} .", ab[0], ab[1])
        }
    }
}

/// One random fact.
fn fact(r: &mut Rng) -> String {
    match r.below(3) {
        0 => format!("{} a {} .", r.pick(INDS), class(r)),
        1 => format!("{} {} {} .", r.pick(INDS), r.pick(OPROPS), r.pick(INDS)),
        _ => format!("{} {} {} .", r.pick(INDS), r.pick(DPROPS), r.pick(LITS)),
    }
}

/// A random graph for `seed`, as Turtle (without the prefixes).
fn random_graph(seed: u64) -> String {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut out: Vec<String> = Vec::new();
    for c in CLASSES {
        out.push(format!("{c} a owl:Class ."));
    }
    for p in OPROPS {
        out.push(format!("{p} a owl:ObjectProperty ."));
    }
    for d in DPROPS {
        out.push(format!("{d} a owl:DatatypeProperty ."));
    }
    for _ in 0..3 + r.below(6) {
        out.push(axiom(&mut r));
    }
    for _ in 0..3 + r.below(7) {
        out.push(fact(&mut r));
    }
    out.join("\n")
}

/// The triples of the unnamed default graph, or of `graph`.
fn triples_of(
    store: &TripleStore,
    graph: Option<&str>,
) -> std::collections::HashSet<rl_reference::T> {
    let q = match graph {
        Some(g) => format!("SELECT ?s ?p ?o WHERE {{ GRAPH <{g}> {{ ?s ?p ?o }} }}"),
        None => "SELECT ?s ?p ?o WHERE { ?s ?p ?o }".to_string(),
    };
    let mut out = std::collections::HashSet::new();
    if let oxigraph::sparql::QueryResults::Solutions(rows) = store.query(&q).unwrap() {
        for row in rows {
            let row = row.unwrap();
            out.insert((
                row.get("s").unwrap().clone(),
                row.get("p").unwrap().clone(),
                row.get("o").unwrap().clone(),
            ));
        }
    }
    out
}

/// Drop what neither side should be judged on: generalized triples, and
/// reflexive owl:sameAs (eq-ref, opt-in).
fn comparable(
    set: impl IntoIterator<Item = rl_reference::T>,
) -> std::collections::BTreeSet<String> {
    let same = rl_reference::owl("sameAs");
    set.into_iter()
        .filter(|(s, p, o)| {
            !matches!(s, oxigraph::model::Term::Literal(_)) && !(p == &same && s == o)
        })
        .map(|(s, p, o)| format!("{s} {p} {o}"))
        .collect()
}

/// Compare the engine with the reference on one graph; `Err` explains the
/// disagreement. `Ok(true)` when both found the graph consistent.
fn differential(seed: u64) -> Result<bool, String> {
    let ttl = random_graph(seed);
    let store = store_with(&ttl);
    let reference = rl_reference::closure(triples_of(&store, None));
    let engine = Owl2RLReasoner::new(&store).materialize();
    let ctx = || format!("seed {seed}:\n{ttl}\n");
    match (reference, engine) {
        (Err(_), Err(ReasoningError::Inconsistency { .. })) => Ok(false),
        (Err(rule), Ok(_)) => Err(format!(
            "{}the reference derives false by {rule}; the engine found it consistent",
            ctx()
        )),
        (Ok(_), Err(e)) => Err(format!(
            "{}the reference closure is consistent; the engine: {e}",
            ctx()
        )),
        (Err(_), Err(e)) => Err(format!("{}engine error: {e}", ctx())),
        (Ok(g), Ok(_)) => {
            let want = comparable(g.set);
            let mut got_set = triples_of(&store, None);
            got_set.extend(triples_of(&store, Some(TG)));
            let got = comparable(got_set);
            let missing: Vec<&String> = want.difference(&got).take(15).collect();
            let extra: Vec<&String> = got.difference(&want).take(15).collect();
            if missing.is_empty() && extra.is_empty() {
                Ok(true)
            } else {
                Err(format!("{}missing (in the reference, not the engine): {missing:#?}\nextra (engine only): {extra:#?}", ctx()))
            }
        }
    }
}

#[test]
fn engine_agrees_with_the_reference_evaluator_on_random_graphs() {
    let seeds: u64 = std::env::var("OTS_RL_DIFF_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    let mut consistent = 0;
    let mut failures = Vec::new();
    for seed in 0..seeds {
        match differential(seed) {
            Ok(true) => consistent += 1,
            Ok(false) => {}
            Err(e) => failures.push(e),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {seeds} graphs disagree; the first:\n{}",
        failures.len(),
        failures
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n---\n")
    );
    assert!(
        consistent * 4 >= seeds,
        "too few consistent graphs ({consistent} of {seeds}) to compare closures"
    );
}
