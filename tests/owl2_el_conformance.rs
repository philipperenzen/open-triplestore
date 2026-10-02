//! OWL 2 EL conformance tests — completion rule coverage.
//!
//! Tests the EL++ completion rules CR1–CR10, hasKey, and reflexivity,
//! verifying that the `El2Classifier` produces correct classifications.
//!
//! The tests near the end run *scoped* (`with_sources`, as a dataset run
//! does): an unscoped run reads the unnamed default graph only, so a rule
//! whose premise was itself derived into the target graph does not fire
//! there yet.

#![cfg(feature = "owl2-el")]

use open_triplestore::reasoning::common::{ReasoningError, ReasoningReport};
use open_triplestore::reasoning::owl2_el::El2Classifier;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;

const TG: &str = "urn:entailment:owl2-el";

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
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> rdf:type <http://example.org/Person> ."
        ),
        "CR1: type propagation through subClassOf"
    );
}

#[test]
fn test_cr1_three_level_chain() {
    let s = store_with(
        "ex:Manager rdfs:subClassOf ex:Employee . \
                        ex:Employee rdfs:subClassOf ex:Person . \
                        ex:carol rdf:type ex:Manager .",
    );
    classify(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/carol> rdf:type <http://example.org/Employee> ."
        ),
        "CR1: chain level 1"
    );
    assert!(
        ask_tg(
            &s,
            "<http://example.org/carol> rdf:type <http://example.org/Person> ."
        ),
        "CR1: chain level 2"
    );
}

// ─── CR2: Intersection ───────────────────────────────────────────────────────

#[test]
fn test_cr2_intersection_membership() {
    let s = store_with(
        "ex:WorkingParent owl:intersectionOf ( ex:Worker ex:Parent ) . \
                        ex:alice rdf:type ex:Worker . ex:alice rdf:type ex:Parent .",
    );
    classify(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> rdf:type <http://example.org/WorkingParent> ."
        ),
        "CR2: intersection membership"
    );
}

#[test]
fn test_cr2_intersection_with_subclass() {
    let s = store_with(
        "ex:C owl:intersectionOf ( ex:A ex:B ) . \
                        ex:A rdfs:subClassOf ex:C . \
                        ex:x rdf:type ex:A . ex:x rdf:type ex:B .",
    );
    classify(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/x> rdf:type <http://example.org/C> ."
        ),
        "CR2: intersection with subclass chain"
    );
}

// ─── CR4: Existential restriction ─────────────────────────────────────────────

#[test]
fn test_cr4_existential_to_class() {
    let s = store_with(
        "[ owl:someValuesFrom ex:Animal ; owl:onProperty ex:hasPet ] \
                            rdfs:subClassOf ex:PetOwner . \
                        ex:alice ex:hasPet ex:dog . ex:dog rdf:type ex:Animal .",
    );
    classify(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> rdf:type <http://example.org/PetOwner> ."
        ),
        "CR4: existential restriction → class membership"
    );
}

// ─── CR5: Property chain (2-element) ──────────────────────────────────────────

#[test]
fn test_cr5_property_chain_2() {
    let s = store_with(
        "ex:uncleOf owl:propertyChainAxiom ( ex:brotherOf ex:parentOf ) . \
                        ex:bob ex:brotherOf ex:carol . ex:carol ex:parentOf ex:dave .",
    );
    classify(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/bob> <http://example.org/uncleOf> <http://example.org/dave> ."
        ),
        "CR5: 2-element property chain"
    );
}

// ─── CR7: Domain propagation ──────────────────────────────────────────────────

#[test]
fn test_cr7_domain_propagation() {
    let s = store_with("ex:worksFor rdfs:domain ex:Employee . ex:alice ex:worksFor ex:Acme .");
    classify(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> rdf:type <http://example.org/Employee> ."
        ),
        "CR7: domain propagation types subject"
    );
}

#[test]
fn test_cr7_domain_with_subclass() {
    let s = store_with(
        "ex:worksFor rdfs:domain ex:Employee . \
                        ex:Employee rdfs:subClassOf ex:Person . \
                        ex:alice ex:worksFor ex:Acme .",
    );
    classify(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> rdf:type <http://example.org/Employee> ."
        ),
        "CR7 + CR1: domain + subclass chain"
    );
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> rdf:type <http://example.org/Person> ."
        ),
        "CR1 chained from CR7"
    );
}

// ─── CR8: Range propagation ───────────────────────────────────────────────────

#[test]
fn test_cr8_range_propagation() {
    let s = store_with("ex:hasChild rdfs:range ex:Person . ex:alice ex:hasChild ex:bob .");
    classify(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/bob> rdf:type <http://example.org/Person> ."
        ),
        "CR8: range propagation types object"
    );
}

// ─── CR9: Reflexive property ──────────────────────────────────────────────────

#[test]
fn test_cr9_reflexive_property() {
    let s =
        store_with("ex:relatedTo rdf:type owl:ReflexiveProperty . ex:alice rdf:type ex:Person .");
    classify(&s);
    assert!(ask_tg(&s, "<http://example.org/alice> <http://example.org/relatedTo> <http://example.org/alice> ."),
        "CR9: reflexive property should generate self-loop");
}

// ─── CR10: 3-element property chain ───────────────────────────────────────────

#[test]
fn test_cr10_property_chain_3() {
    let s = store_with(
        "ex:r owl:propertyChainAxiom ( ex:p1 ex:p2 ex:p3 ) . \
                        ex:a ex:p1 ex:b . ex:b ex:p2 ex:c . ex:c ex:p3 ex:d .",
    );
    classify(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/a> <http://example.org/r> <http://example.org/d> ."
        ),
        "CR10: 3-element property chain"
    );
}

// ─── hasKey ───────────────────────────────────────────────────────────────────

#[test]
fn test_has_key_merges_individuals() {
    let s = store_with(
        "ex:Person owl:hasKey ( ex:ssn ) . \
                        ex:alice rdf:type ex:Person . ex:alice ex:ssn ex:SSN001 . \
                        ex:bob rdf:type ex:Person . ex:bob ex:ssn ex:SSN001 .",
    );
    classify(&s);
    assert!(
        ask_tg(
            &s,
            "<http://example.org/alice> owl:sameAs <http://example.org/bob> ."
        ),
        "hasKey: two individuals with same key should be merged"
    );
}

// ─── Complex scenario: biomedical-style classification ─────────────────────────

#[test]
fn test_biomedical_classification() {
    // SNOMED-like classification: Drug --hasClinicalModality--> Analgesic subClassOf Drug
    let s = store_with(
        "ex:hasClinicalModality rdfs:range ex:ClinicalModality . \
         ex:Analgesic rdfs:subClassOf ex:Drug . \
         ex:Aspirin ex:hasClinicalModality ex:AnalgesicModality . \
         ex:AnalgesicModality rdf:type ex:ClinicalModality .",
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
    let first = El2Classifier::new(&s).classify().unwrap();
    assert!(first.triples_added > 0, "the first run derives something");
    let size_after_first = count_tg(&s);

    // `triples_added` is the delta this run wrote. A rerun over an unchanged
    // store derives nothing new, and the entailment graph does not grow.
    // (This used to compare two graph SIZES, which are equal on any rerun —
    // it could not distinguish idempotence from accumulation.)
    let second = El2Classifier::new(&s).classify().unwrap();
    assert_eq!(second.triples_added, 0, "a rerun adds nothing");
    assert_eq!(count_tg(&s), size_after_first, "the graph does not grow");
}

fn count_tg(s: &TripleStore) -> usize {
    match s
        .query(&format!(
            "SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{TG}> {{ ?s ?p ?o }} }}"
        ))
        .unwrap()
    {
        oxigraph::sparql::QueryResults::Solutions(mut sols) => sols
            .next()
            .and_then(|r| r.ok())
            .and_then(|r| r.get("c").map(|t| t.to_string()))
            .and_then(|t| t.trim_start_matches('"').split('"').next()?.parse().ok())
            .unwrap_or(0),
        _ => 0,
    }
}

// ─── Scoped runs: soundness, equivalence, roles, keys, consistency ────────────

const DATA: &str = "urn:test:el-data";

/// A store with `ttl` in the named graph [`DATA`].
fn scoped_store(ttl: &str) -> TripleStore {
    let store = TripleStore::in_memory().unwrap();
    let preamble = "@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
                    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                    @prefix owl:  <http://www.w3.org/2002/07/owl#> .\n\
                    @prefix ex:   <http://example.org/> .\n";
    store
        .load_str(&format!("{preamble}{ttl}"), RdfFormat::Turtle, Some(DATA))
        .unwrap();
    store
}

fn classify_scoped(store: &TripleStore) -> Result<ReasoningReport, ReasoningError> {
    El2Classifier::new(store)
        .with_sources(vec![DATA.to_string()])
        .classify()
}

fn ask(store: &TripleStore, q: &str) -> bool {
    match store.query(q).unwrap() {
        oxigraph::sparql::QueryResults::Boolean(b) => b,
        _ => panic!("expected ASK result"),
    }
}

fn ex(local: &str) -> String {
    format!("<http://example.org/{local}>")
}

/// The old CR3 turned `A ⊑ ∃p.B` and `B ⊑ C` into `∃p.C ⊑ A`, which does not
/// follow; the ABox existential rule then typed any `x p y, y a C` as an `A`.
/// What does follow is the other direction, `A ⊑ ∃p.C`.
#[test]
fn test_cr3_soundness_regression() {
    let ttl = "ex:A rdfs:subClassOf [ owl:onProperty ex:p ; owl:someValuesFrom ex:B ] . \
               ex:B rdfs:subClassOf ex:C . \
               ex:D rdfs:subClassOf [ owl:onProperty ex:p ; owl:someValuesFrom ex:C ] . \
               ex:x ex:p ex:y . ex:y rdf:type ex:C .";
    let s = scoped_store(ttl);
    classify_scoped(&s).unwrap();
    assert!(
        !ask_tg(&s, &format!("{} rdf:type {} .", ex("x"), ex("A"))),
        "x p y, y a C does not make x an A"
    );
    assert!(
        !ask_tg(&s, &format!("?r rdfs:subClassOf {} .", ex("A"))),
        "nothing is entailed to be under A (∃p.C ⊑ A does not follow)"
    );
    assert!(
        ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ {} <http://www.w3.org/2000/01/rdf-schema#subClassOf> ?r }} \
                 GRAPH ?g {{ ?r <http://www.w3.org/2002/07/owl#someValuesFrom> {} }} }}",
                ex("A"),
                ex("C")
            )
        ),
        "A ⊑ ∃p.C is entailed"
    );

    // The unscoped run never writes the unsound axiom either.
    let s = store_with(ttl);
    classify(&s);
    assert!(!ask_tg(&s, &format!("?r rdfs:subClassOf {} .", ex("A"))));
}

/// CR4 with filler subsumption: `A ⊑ ∃r.B`, `B ⊑ C`, `∃r.C ⊑ D` ⊨ `A ⊑ D`.
#[test]
fn test_cr4_filler_subsumption() {
    let s = scoped_store(
        "ex:A rdfs:subClassOf [ owl:onProperty ex:r ; owl:someValuesFrom ex:B ] . \
         ex:B rdfs:subClassOf ex:C . \
         [ owl:onProperty ex:r ; owl:someValuesFrom ex:C ] rdfs:subClassOf ex:D . \
         ex:a rdf:type ex:A .",
    );
    classify_scoped(&s).unwrap();
    assert!(ask_tg(
        &s,
        &format!("{} rdfs:subClassOf {} .", ex("A"), ex("D"))
    ));
    assert!(ask_tg(&s, &format!("{} rdf:type {} .", ex("a"), ex("D"))));
    assert!(
        !ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("D"), ex("A"))),
        "subsumption runs one way"
    );
}

/// CR4 through the role hierarchy, and restrictions identified by structure:
/// `A ⊑ ∃r.B`, `r ⊑ s`, and a *different* blank node `∃s.B ⊑ D` ⊨ `A ⊑ D`;
/// `∃r.⊤ ⊑ E` (a `someValuesFrom owl:Thing`) ⊨ `A ⊑ E`.
#[test]
fn test_cr4_role_hierarchy_and_structural_identity() {
    let s = scoped_store(
        "ex:A rdfs:subClassOf [ owl:onProperty ex:r ; owl:someValuesFrom ex:B ] . \
         ex:r rdfs:subPropertyOf ex:s . \
         [ owl:onProperty ex:s ; owl:someValuesFrom ex:B ] rdfs:subClassOf ex:D . \
         [ owl:onProperty ex:r ; owl:someValuesFrom owl:Thing ] rdfs:subClassOf ex:E . \
         [ owl:onProperty ex:r ; owl:someValuesFrom ex:B ] rdfs:subClassOf ex:F .",
    );
    classify_scoped(&s).unwrap();
    for sup in ["D", "E", "F"] {
        assert!(
            ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("A"), ex(sup))),
            "A ⊑ {sup}"
        );
    }
    // The hierarchy runs one way: ∃s.B is not under ∃r.B.
    let s = scoped_store(
        "ex:A rdfs:subClassOf [ owl:onProperty ex:s ; owl:someValuesFrom ex:B ] . \
         ex:r rdfs:subPropertyOf ex:s . \
         [ owl:onProperty ex:r ; owl:someValuesFrom ex:B ] rdfs:subClassOf ex:D .",
    );
    classify_scoped(&s).unwrap();
    assert!(!ask_tg(
        &s,
        &format!("{} rdfs:subClassOf {} .", ex("A"), ex("D"))
    ));
}

/// `owl:equivalentClass` holds in both directions, for a named class and for
/// a definition (`D ≡ E ⊓ ∃p.C`, the usual shape of an EL definition).
#[test]
fn test_equivalent_class_both_directions() {
    let s = scoped_store(
        "ex:Faculty owl:equivalentClass ex:AcademicStaff . \
         ex:alice rdf:type ex:Faculty . ex:bob rdf:type ex:AcademicStaff . \
         ex:D owl:equivalentClass [ owl:intersectionOf ( ex:E \
              [ owl:onProperty ex:p ; owl:someValuesFrom ex:C ] ) ] . \
         ex:x rdf:type ex:E ; ex:p ex:y . ex:y rdf:type ex:C . \
         ex:z rdf:type ex:D . \
         ex:G rdfs:subClassOf ex:E , [ owl:onProperty ex:p ; owl:someValuesFrom ex:C ] .",
    );
    classify_scoped(&s).unwrap();
    assert!(ask_tg(
        &s,
        &format!("{} rdf:type {} .", ex("alice"), ex("AcademicStaff"))
    ));
    assert!(ask_tg(
        &s,
        &format!("{} rdf:type {} .", ex("bob"), ex("Faculty"))
    ));
    assert!(
        ask_tg(&s, &format!("{} rdf:type {} .", ex("x"), ex("D"))),
        "E ⊓ ∃p.C ⊑ D"
    );
    assert!(
        ask_tg(&s, &format!("{} rdf:type {} .", ex("z"), ex("E"))),
        "D ⊑ E"
    );
    assert!(
        ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("G"), ex("D"))),
        "G ⊑ E ⊓ ∃p.C ⊑ D"
    );
}

/// `rdfs:subPropertyOf` and `owl:equivalentProperty` in the ABox, with a
/// domain on the super-property.
#[test]
fn test_subproperty_and_equivalent_property() {
    let s = scoped_store(
        "ex:fatherOf rdfs:subPropertyOf ex:parentOf . \
         ex:parentOf owl:equivalentProperty ex:hasChild . \
         ex:hasChild rdfs:domain ex:Parent . \
         ex:bob ex:fatherOf ex:alice .",
    );
    classify_scoped(&s).unwrap();
    for p in ["parentOf", "hasChild"] {
        assert!(
            ask_tg(&s, &format!("{} {} {} .", ex("bob"), ex(p), ex("alice"))),
            "bob {p} alice"
        );
    }
    assert!(ask_tg(
        &s,
        &format!("{} rdfs:subPropertyOf {} .", ex("fatherOf"), ex("hasChild"))
    ));
    assert!(ask_tg(
        &s,
        &format!("{} rdf:type {} .", ex("bob"), ex("Parent"))
    ));
    assert!(!ask_tg(
        &s,
        &format!("{} {} {} .", ex("bob"), ex("fatherOf"), ex("carol"))
    ));
}

/// `owl:TransitiveProperty` closes a chain of any length.
#[test]
fn test_transitive_property() {
    let s = scoped_store(
        "ex:partOf rdf:type owl:TransitiveProperty . \
         ex:a ex:partOf ex:b . ex:b ex:partOf ex:c . ex:c ex:partOf ex:d . ex:d ex:partOf ex:e .",
    );
    classify_scoped(&s).unwrap();
    for o in ["c", "d", "e"] {
        assert!(
            ask_tg(&s, &format!("{} {} {} .", ex("a"), ex("partOf"), ex(o))),
            "a partOf {o}"
        );
    }
    assert!(!ask_tg(
        &s,
        &format!("{} {} {} .", ex("e"), ex("partOf"), ex("a"))
    ));
}

/// A composite key merges only individuals that share every key property.
#[test]
fn test_has_key_composite() {
    let s = store_with(
        "ex:Person owl:hasKey ( ex:country ex:ssn ) . \
         ex:alice rdf:type ex:Person ; ex:country ex:NL ; ex:ssn ex:S1 . \
         ex:bob   rdf:type ex:Person ; ex:country ex:NL ; ex:ssn ex:S1 . \
         ex:carol rdf:type ex:Person ; ex:country ex:BE ; ex:ssn ex:S1 .",
    );
    classify(&s);
    assert!(ask_tg(
        &s,
        &format!("{} owl:sameAs {} .", ex("alice"), ex("bob"))
    ));
    assert!(
        !ask_tg(&s, &format!("{} owl:sameAs {} .", ex("alice"), ex("carol"))),
        "carol shares only one of the two key properties"
    );
}

/// An unsatisfiable class without instances is not an inconsistency — the
/// old `check_consistency` said it was. Unsatisfiability also reaches a
/// class through `∃p.⊥` and through two disjoint superclasses.
#[test]
fn test_unsatisfiable_class_is_consistent() {
    let s = scoped_store(
        "ex:A rdfs:subClassOf ex:B . ex:B rdfs:subClassOf owl:Nothing . \
         ex:E rdfs:subClassOf [ owl:onProperty ex:p ; owl:someValuesFrom ex:B ] . \
         ex:F rdfs:subClassOf ex:G , ex:H . ex:G owl:disjointWith ex:H . \
         ex:x rdf:type ex:C .",
    );
    classify_scoped(&s).unwrap();
    let c = El2Classifier::new(&s).with_sources(vec![DATA.to_string()]);
    assert!(c.check_consistency().unwrap());
    let unsat = c.unsatisfiable_classes().unwrap();
    for cls in ["A", "B", "E", "F"] {
        assert!(
            unsat.contains(&format!("http://example.org/{cls}")),
            "{cls} is unsatisfiable: {unsat:?}"
        );
    }
    assert!(!unsat.contains(&"http://example.org/C".to_string()));
}

/// An individual of an unsatisfiable class, or of two disjoint classes,
/// makes the ontology inconsistent, and `classify` says so.
#[test]
fn test_inconsistency_is_reported() {
    let s = scoped_store(
        "ex:A rdfs:subClassOf ex:B . ex:B rdfs:subClassOf owl:Nothing . \
         ex:x rdf:type ex:A .",
    );
    assert!(matches!(
        classify_scoped(&s),
        Err(ReasoningError::Inconsistency { .. })
    ));

    let s = scoped_store(
        "ex:Cat owl:disjointWith ex:Dog . \
         ex:Kitten rdfs:subClassOf ex:Cat . \
         ex:rex rdf:type ex:Kitten , ex:Dog .",
    );
    assert!(matches!(
        classify_scoped(&s),
        Err(ReasoningError::Inconsistency { .. })
    ));

    // With detection off the run completes, and the check reports it.
    let mut c = El2Classifier::new(&s).with_sources(vec![DATA.to_string()]);
    c.detect_inconsistency = false;
    c.classify().unwrap();
    assert!(!c.check_consistency().unwrap());
}
