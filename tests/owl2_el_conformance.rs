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

/// SNOMED-style definitions: a disorder is classified under another through
/// its definition, and under a body-system disorder through a TBox property
/// chain (`findingSite ∘ partOf ⊑ findingSite`). This used to accept an
/// asserted triple, so it passed whatever the reasoner did.
#[test]
fn test_biomedical_classification() {
    let s = store_with(
        "ex:findingSite owl:propertyChainAxiom ( ex:findingSite ex:partOf ) . \
         ex:Lung rdfs:subClassOf [ owl:onProperty ex:partOf ; owl:someValuesFrom ex:RespiratorySystem ] . \
         ex:Pneumonia owl:equivalentClass [ owl:intersectionOf ( ex:Disorder \
             [ owl:onProperty ex:findingSite ; owl:someValuesFrom ex:Lung ] ) ] . \
         ex:BacterialPneumonia owl:equivalentClass [ owl:intersectionOf ( ex:Disorder \
             [ owl:onProperty ex:findingSite ; owl:someValuesFrom ex:Lung ] \
             [ owl:onProperty ex:causativeAgent ; owl:someValuesFrom ex:Bacterium ] ) ] . \
         ex:RespiratoryDisorder owl:equivalentClass [ owl:intersectionOf ( ex:Disorder \
             [ owl:onProperty ex:findingSite ; owl:someValuesFrom ex:RespiratorySystem ] ) ] .",
    );
    classify(&s);
    let sub = |a: &str, b: &str| ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex(a), ex(b)));
    assert!(sub("BacterialPneumonia", "Pneumonia"), "by definition");
    assert!(sub("Pneumonia", "RespiratoryDisorder"), "through the chain");
    assert!(sub("BacterialPneumonia", "RespiratoryDisorder"));
    assert!(!sub("Pneumonia", "BacterialPneumonia"));
    assert!(!sub("RespiratoryDisorder", "Pneumonia"));
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
        Err(ReasoningError::Inconsistency(_))
    ));

    let s = scoped_store(
        "ex:Cat owl:disjointWith ex:Dog . \
         ex:Kitten rdfs:subClassOf ex:Cat . \
         ex:rex rdf:type ex:Kitten , ex:Dog .",
    );
    assert!(matches!(
        classify_scoped(&s),
        Err(ReasoningError::Inconsistency(_))
    ));

    // With detection off the run completes, and the check reports it.
    let mut c = El2Classifier::new(&s).with_sources(vec![DATA.to_string()]);
    c.detect_inconsistency = false;
    c.classify().unwrap();
    assert!(!c.check_consistency().unwrap());
}

// ─── Native engine: completeness, output and reporting ─────────────────────────

fn report_scoped(ttl: &str) -> (TripleStore, ReasoningReport) {
    let s = scoped_store(ttl);
    let r = classify_scoped(&s).unwrap();
    (s, r)
}

/// An unscoped run sees its own consequences: a sub-property edge feeds a
/// domain, a transitive closure of four links, all from the default graph.
/// The SPARQL loop read the default graph only, so none of these fired.
#[test]
fn test_unscoped_run_sees_its_own_derivations() {
    let s = store_with(
        "ex:partOf rdf:type owl:TransitiveProperty . \
         ex:directPartOf rdfs:subPropertyOf ex:partOf . \
         ex:partOf rdfs:domain ex:Part . \
         ex:a ex:directPartOf ex:b . ex:b ex:directPartOf ex:c . \
         ex:c ex:directPartOf ex:d . ex:d ex:directPartOf ex:e .",
    );
    classify(&s);
    assert!(ask_tg(&s, &format!("{} {} {} .", ex("a"), ex("partOf"), ex("e"))));
    assert!(ask_tg(&s, &format!("{} rdf:type {} .", ex("a"), ex("Part"))));
    assert!(!ask_tg(&s, &format!("{} rdf:type {} .", ex("e"), ex("Part"))));
}

/// CR11 in the TBox: `A ⊑ ∃r.B`, `B ⊑ ∃s.C`, `r ∘ s ⊑ t`, `∃t.C ⊑ D` ⊨ `A ⊑ D`.
#[test]
fn test_tbox_property_chain() {
    let (s, _) = report_scoped(
        "ex:A rdfs:subClassOf [ owl:onProperty ex:r ; owl:someValuesFrom ex:B ] . \
         ex:B rdfs:subClassOf [ owl:onProperty ex:s ; owl:someValuesFrom ex:C ] . \
         ex:t owl:propertyChainAxiom ( ex:r ex:s ) . \
         [ owl:onProperty ex:t ; owl:someValuesFrom ex:C ] rdfs:subClassOf ex:D .",
    );
    assert!(ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("A"), ex("D"))));
    assert!(!ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("B"), ex("D"))));
}

/// Transitivity in the TBox: parts of parts are parts.
#[test]
fn test_tbox_transitivity() {
    let (s, _) = report_scoped(
        "ex:partOf rdf:type owl:TransitiveProperty . \
         ex:Finger rdfs:subClassOf [ owl:onProperty ex:partOf ; owl:someValuesFrom ex:Hand ] . \
         ex:Hand rdfs:subClassOf [ owl:onProperty ex:partOf ; owl:someValuesFrom ex:Arm ] . \
         ex:ArmPart owl:equivalentClass [ owl:onProperty ex:partOf ; owl:someValuesFrom ex:Arm ] .",
    );
    assert!(ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("Finger"), ex("ArmPart"))));
    assert!(ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("Hand"), ex("ArmPart"))));
}

/// A property chain of four properties over individuals (the SPARQL loop
/// stopped at three).
#[test]
fn test_property_chain_of_four() {
    let (s, _) = report_scoped(
        "ex:r owl:propertyChainAxiom ( ex:p1 ex:p2 ex:p3 ex:p4 ) . \
         ex:a ex:p1 ex:b . ex:b ex:p2 ex:c . ex:c ex:p3 ex:d . ex:d ex:p4 ex:e .",
    );
    assert!(ask_tg(&s, &format!("{} {} {} .", ex("a"), ex("r"), ex("e"))));
    assert!(!ask_tg(&s, &format!("{} {} {} .", ex("a"), ex("r"), ex("d"))));
}

/// A range applies through the property hierarchy, to individuals and to
/// the successor of an existential — without making every filler an
/// instance of the range.
#[test]
fn test_range_through_hierarchy_and_existentials() {
    let (s, _) = report_scoped(
        "ex:hasMother rdfs:subPropertyOf ex:hasParent . \
         ex:hasParent rdfs:range ex:Person . \
         ex:x ex:hasMother ex:y . \
         ex:A rdfs:subClassOf [ owl:onProperty ex:hasMother ; owl:someValuesFrom ex:Female ] . \
         [ owl:onProperty ex:hasParent ; owl:someValuesFrom \
             [ owl:intersectionOf ( ex:Female ex:Person ) ] ] rdfs:subClassOf ex:HasFemaleParent .",
    );
    assert!(ask_tg(&s, &format!("{} rdf:type {} .", ex("y"), ex("Person"))));
    assert!(ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("A"), ex("HasFemaleParent"))));
    assert!(
        !ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("Female"), ex("Person"))),
        "the range does not leak into the shared filler"
    );
    assert!(!ask_tg(&s, &format!("{} rdf:type {} .", ex("x"), ex("Person"))));
}

/// Reflexivity: every individual gets the loop, including one that only
/// appears as an object (the SPARQL rule missed it), and `∃r.A ⊑ B` with
/// `r` reflexive gives `A ⊑ B`. Classes get no loop.
#[test]
fn test_reflexive_property() {
    let (s, _) = report_scoped(
        "ex:knows rdf:type owl:ReflexiveProperty . \
         ex:alice ex:likes ex:bob . ex:likes rdfs:subPropertyOf ex:likes2 . \
         [ owl:onProperty ex:knows ; owl:someValuesFrom ex:Expert ] rdfs:subClassOf ex:KnowsExpert . \
         ex:Expert rdfs:subClassOf ex:Person .",
    );
    assert!(ask_tg(&s, &format!("{} {} {} .", ex("bob"), ex("knows"), ex("bob"))));
    assert!(ask_tg(&s, &format!("{} {} {} .", ex("alice"), ex("knows"), ex("alice"))));
    assert!(ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("Expert"), ex("KnowsExpert"))));
    assert!(!ask_tg(&s, &format!("{} {} {} .", ex("Expert"), ex("knows"), ex("Expert"))));
}

/// ⊥ travels back along property edges: an individual whose successor is
/// unsatisfiable makes the ontology inconsistent.
#[test]
fn test_bottom_through_successor_is_inconsistent() {
    let s = scoped_store(
        "ex:Empty rdfs:subClassOf owl:Nothing . \
         ex:x ex:p ex:y . ex:A owl:equivalentClass [ owl:onProperty ex:p ; owl:someValuesFrom ex:B ] . \
         ex:B rdfs:subClassOf ex:Empty . ex:y rdf:type ex:B .",
    );
    let err = classify_scoped(&s).unwrap_err();
    assert!(matches!(err, ReasoningError::Inconsistency(_)), "{err}");
}

/// `owl:Thing ⊑ owl:Nothing` is an inconsistency even without individuals.
#[test]
fn test_top_below_bottom_is_inconsistent() {
    let s = scoped_store("owl:Thing rdfs:subClassOf ex:A . ex:A rdfs:subClassOf owl:Nothing .");
    match classify_scoped(&s) {
        Err(ReasoningError::Inconsistency(msg)) => assert!(msg.contains("owl:Thing"), "{msg}"),
        other => panic!("expected an inconsistency, got {other:?}"),
    }
}

/// `owl:AllDisjointClasses` makes its members pairwise disjoint.
#[test]
fn test_all_disjoint_classes() {
    let ttl = "[ rdf:type owl:AllDisjointClasses ; owl:members ( ex:A ex:B ex:C ) ] . \
               ex:D rdfs:subClassOf ex:A , ex:C .";
    let s = scoped_store(ttl);
    classify_scoped(&s).unwrap();
    let unsat = El2Classifier::new(&s)
        .with_sources(vec![DATA.to_string()])
        .unsatisfiable_classes()
        .unwrap();
    assert_eq!(unsat, vec!["http://example.org/D".to_string()]);

    let s = scoped_store(&format!("{ttl} ex:x rdf:type ex:B , ex:C ."));
    match classify_scoped(&s) {
        Err(ReasoningError::Inconsistency(msg)) => assert!(msg.contains("disjoint"), "{msg}"),
        other => panic!("expected an inconsistency, got {other:?}"),
    }
}

/// Classification writes `owl:equivalentClass` between equivalent class IRIs,
/// told or entailed, and `owl:Thing ⊑ C` types every individual as a `C`.
#[test]
fn test_equivalence_and_top_output() {
    let (s, _) = report_scoped(
        "ex:A owl:equivalentClass [ owl:onProperty ex:p ; owl:someValuesFrom ex:C ] . \
         ex:B owl:equivalentClass [ owl:onProperty ex:p ; owl:someValuesFrom ex:C ] . \
         owl:Thing rdfs:subClassOf ex:Entity . \
         ex:x ex:q ex:y .",
    );
    assert!(ask_tg(&s, &format!("{} owl:equivalentClass {} .", ex("A"), ex("B"))));
    assert!(ask_tg(&s, &format!("{} owl:equivalentClass {} .", ex("B"), ex("A"))));
    assert!(ask_tg(&s, &format!("{} rdf:type {} .", ex("y"), ex("Entity"))));
    assert!(ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("A"), ex("Entity"))));
}

/// Intersections of any arity, on either side of an axiom.
#[test]
fn test_nary_intersection() {
    let (s, _) = report_scoped(
        "ex:C owl:equivalentClass [ owl:intersectionOf ( ex:A1 ex:A2 ex:A3 ex:A4 ) ] . \
         ex:x rdf:type ex:A1 , ex:A2 , ex:A3 , ex:A4 . \
         ex:y rdf:type ex:A1 , ex:A2 , ex:A3 . \
         ex:z rdf:type ex:C .",
    );
    assert!(ask_tg(&s, &format!("{} rdf:type {} .", ex("x"), ex("C"))));
    assert!(!ask_tg(&s, &format!("{} rdf:type {} .", ex("y"), ex("C"))));
    assert!(ask_tg(&s, &format!("{} rdf:type {} .", ex("z"), ex("A4"))));
}

/// Only new triples are written: nothing already stated in a source graph,
/// and nothing the target graph already holds.
#[test]
fn test_writes_only_new_triples() {
    let (s, r) = report_scoped(
        "ex:A rdfs:subClassOf ex:B . ex:B rdfs:subClassOf ex:C . \
         ex:x rdf:type ex:A , ex:B .",
    );
    assert!(!ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("A"), ex("B"))));
    assert!(!ask_tg(&s, &format!("{} rdf:type {} .", ex("x"), ex("B"))));
    assert!(ask_tg(&s, &format!("{} rdf:type {} .", ex("x"), ex("C"))));
    assert_eq!(r.triples_added, 2, "A ⊑ C and x a C: {r:?}");
    assert_eq!(count_tg(&s), 2);
    // The writes went through insert_quads: the graph index knows the count.
    assert_eq!(s.graph_count_cached(Some(TG)), Some(2));
}

/// A scoped run reads only its sources (and the target graph).
#[test]
fn test_scope_excludes_other_graphs() {
    let s = scoped_store("ex:x rdf:type ex:A .");
    s.load_str(
        "<http://example.org/A> <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
         <http://example.org/B> .",
        RdfFormat::Turtle,
        Some("urn:test:other"),
    )
    .unwrap();
    classify_scoped(&s).unwrap();
    assert!(!ask_tg(&s, &format!("{} rdf:type {} .", ex("x"), ex("B"))));
    El2Classifier::new(&s)
        .with_sources(vec![DATA.to_string(), "urn:test:other".to_string()])
        .classify()
        .unwrap();
    assert!(ask_tg(&s, &format!("{} rdf:type {} .", ex("x"), ex("B"))));
}

/// Keys see inferred types and sub-property edges.
#[test]
fn test_has_key_with_inferred_facts() {
    let (s, _) = report_scoped(
        "ex:Person owl:hasKey ( ex:ssn ) . \
         ex:Student rdfs:subClassOf ex:Person . \
         ex:studentNumber rdfs:subPropertyOf ex:ssn . \
         ex:alice rdf:type ex:Student ; ex:studentNumber \"42\" . \
         ex:bob rdf:type ex:Person ; ex:ssn \"42\" . \
         ex:carol ex:ssn \"42\" .",
    );
    assert!(ask_tg(&s, &format!("{} owl:sameAs {} .", ex("alice"), ex("bob"))));
    assert!(ask_tg(&s, &format!("{} owl:sameAs {} .", ex("bob"), ex("alice"))));
    assert!(
        !ask_tg(&s, &format!("{} owl:sameAs {} .", ex("alice"), ex("carol"))),
        "carol is not known to be a Person"
    );
}

/// A data property's domain types the subject; the literal gets no type.
#[test]
fn test_data_property_domain_and_hierarchy() {
    let (s, _) = report_scoped(
        "ex:age rdf:type owl:DatatypeProperty ; rdfs:domain ex:Agent . \
         ex:exactAge rdfs:subPropertyOf ex:age . \
         ex:x ex:exactAge 42 .",
    );
    assert!(ask_tg(&s, &format!("{} rdf:type {} .", ex("x"), ex("Agent"))));
    assert!(ask_tg(&s, &format!("{} {} 42 .", ex("x"), ex("age"))));
    assert!(!ask_tg(&s, "?lit rdf:type ?c . FILTER(isLiteral(?lit))"));
}

/// Constructs outside the profile are counted in the report, and leaving
/// them out adds no wrong consequence.
#[test]
fn test_ignored_constructs_are_reported() {
    let (s, r) = report_scoped(
        "ex:A rdfs:subClassOf [ owl:unionOf ( ex:B ex:C ) ] . \
         ex:D rdfs:subClassOf [ owl:onProperty ex:p ; owl:allValuesFrom ex:E ] . \
         ex:F rdfs:subClassOf [ owl:onProperty ex:p ; owl:maxCardinality 1 ] . \
         ex:G rdfs:subClassOf [ owl:onProperty ex:p ; owl:maxCardinality 2 ] . \
         ex:p owl:inverseOf ex:q . \
         ex:p rdf:type owl:FunctionalProperty . \
         ex:x rdf:type ex:A .",
    );
    let ignored: Vec<(String, usize)> = r
        .ignored
        .iter()
        .map(|i| (i.construct.clone(), i.count))
        .collect();
    for (construct, count) in [
        ("ObjectUnionOf", 1),
        ("ObjectAllValuesFrom", 1),
        ("cardinality restriction", 2),
        ("InverseObjectProperties", 1),
        ("FunctionalObjectProperty", 1),
    ] {
        assert!(
            ignored.contains(&(construct.to_string(), count)),
            "{construct} × {count} in {ignored:?}"
        );
    }
    assert!(!ask_tg(&s, &format!("{} rdf:type {} .", ex("x"), ex("B"))));
    let json = serde_json::to_value(&r).unwrap();
    assert!(json["ignored"].is_array());

    // A report with nothing ignored serializes without the field.
    let (_, r) = report_scoped("ex:A rdfs:subClassOf ex:B .");
    assert!(r.ignored.is_empty());
    assert!(serde_json::to_value(&r).unwrap().get("ignored").is_none());
}

/// A three-hundred-class generated ontology with existentials, chains and an
/// ABox classifies, and the deepest class lands under the root definition.
#[test]
fn test_generated_ontology_classifies() {
    let mut ttl = String::from("ex:partOf rdf:type owl:TransitiveProperty . ");
    for i in 1..300 {
        ttl.push_str(&format!(
            "ex:C{i} rdfs:subClassOf ex:C{} , [ owl:onProperty ex:partOf ; owl:someValuesFrom ex:C{} ] . ",
            i - 1,
            i / 2
        ));
        ttl.push_str(&format!("ex:i{i} rdf:type ex:C{i} . "));
    }
    ttl.push_str(
        "ex:PartOfRoot owl:equivalentClass [ owl:onProperty ex:partOf ; owl:someValuesFrom ex:C0 ] . ",
    );
    let (s, r) = report_scoped(&ttl);
    assert!(ask_tg(&s, &format!("{} rdfs:subClassOf {} .", ex("C299"), ex("PartOfRoot"))));
    assert!(ask_tg(&s, &format!("{} rdf:type {} .", ex("i299"), ex("C0"))));
    assert!(r.triples_added > 1000, "{r:?}");
}

// ─── Differential test against the RL engine on EL ∩ RL ────────────────────────

/// A deterministic generator (xorshift64*), so a failure names its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A random ontology in the intersection of OWL 2 EL and OWL 2 RL: no
/// existential on the right, no reflexivity, no keys or equality, binary
/// intersections and chains (the RL rules are binary), no disjointness.
fn random_el_rl(seed: u64) -> String {
    let mut g = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let (nc, np, ni) = (8, 4, 7);
    let c = |i: usize| format!("ex:C{i}");
    let p = |i: usize| format!("ex:p{i}");
    let ind = |i: usize| format!("ex:i{i}");
    let mut t = String::new();
    for _ in 0..14 {
        let line = match g.below(10) {
            0 | 1 => format!("{} rdfs:subClassOf {} .", c(g.below(nc)), c(g.below(nc))),
            2 => format!("{} owl:equivalentClass {} .", c(g.below(nc)), c(g.below(nc))),
            3 => format!(
                "[ owl:intersectionOf ( {} {} ) ] rdfs:subClassOf {} .",
                c(g.below(nc)),
                c(g.below(nc)),
                c(g.below(nc))
            ),
            4 => format!(
                "{} rdfs:subClassOf [ owl:intersectionOf ( {} {} ) ] .",
                c(g.below(nc)),
                c(g.below(nc)),
                c(g.below(nc))
            ),
            5 => {
                let filler = if g.below(4) == 0 { "owl:Thing".to_string() } else { c(g.below(nc)) };
                format!(
                    "[ owl:onProperty {} ; owl:someValuesFrom {} ] rdfs:subClassOf {} .",
                    p(g.below(np)),
                    filler,
                    c(g.below(nc))
                )
            }
            6 => format!("{} rdfs:subPropertyOf {} .", p(g.below(np)), p(g.below(np))),
            7 => match g.below(3) {
                0 => format!("{} rdf:type owl:TransitiveProperty .", p(g.below(np))),
                _ => format!(
                    "{} owl:propertyChainAxiom ( {} {} ) .",
                    p(g.below(np)),
                    p(g.below(np)),
                    p(g.below(np))
                ),
            },
            8 => format!("{} rdfs:domain {} .", p(g.below(np)), c(g.below(nc))),
            _ => format!("{} rdfs:range {} .", p(g.below(np)), c(g.below(nc))),
        };
        t.push_str(&line);
        t.push(' ');
    }
    for _ in 0..6 {
        t.push_str(&format!("{} rdf:type {} . ", ind(g.below(ni)), c(g.below(nc))));
    }
    for _ in 0..9 {
        t.push_str(&format!(
            "{} {} {} . ",
            ind(g.below(ni)),
            p(g.below(np)),
            ind(g.below(ni))
        ));
    }
    t
}

/// The named types of the individuals and the property edges between them,
/// over every graph.
#[cfg(feature = "owl2-rl")]
fn abox_closure(s: &TripleStore) -> std::collections::BTreeSet<String> {
    let q = "SELECT DISTINCT ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } \
             FILTER(STRSTARTS(STR(?s), \"http://example.org/i\")) \
             FILTER((?p = <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
                     && STRSTARTS(STR(?o), \"http://example.org/C\")) \
                 || (STRSTARTS(STR(?p), \"http://example.org/p\") \
                     && STRSTARTS(STR(?o), \"http://example.org/i\"))) }";
    let mut out = std::collections::BTreeSet::new();
    if let oxigraph::sparql::QueryResults::Solutions(sols) = s.query(q).unwrap() {
        for sol in sols {
            let sol = sol.unwrap();
            out.insert(format!("{} {} {}", sol.get("s").unwrap(), sol.get("p").unwrap(), sol.get("o").unwrap()));
        }
    }
    out
}

/// On ontologies in both profiles, the EL engine's realization and property
/// closure equal the RL engine's (OWL 2 Profiles Theorem PR1 makes RL
/// complete for these atomic consequences).
#[cfg(feature = "owl2-rl")]
#[test]
fn test_differential_against_rl() {
    use open_triplestore::reasoning::owl2_rl::Owl2RLReasoner;
    for seed in 1..=40u64 {
        let ttl = random_el_rl(seed);
        let el = scoped_store(&ttl);
        classify_scoped(&el).unwrap();
        let rl = scoped_store(&ttl);
        Owl2RLReasoner::new(&rl)
            .with_sources(vec![DATA.to_string()])
            .materialize()
            .unwrap();
        let (a, b) = (abox_closure(&el), abox_closure(&rl));
        assert_eq!(
            a,
            b,
            "seed {seed}: EL-only {:?}, RL-only {:?}\n{ttl}",
            a.difference(&b).collect::<Vec<_>>(),
            b.difference(&a).collect::<Vec<_>>()
        );
    }
}
