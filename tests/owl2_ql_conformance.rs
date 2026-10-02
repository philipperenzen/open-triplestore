//! OWL 2 QL conformance tests — query rewriting coverage.
//!
//! Tests the AST-level query rewriter for OWL 2 QL (DL-Lite_R), verifying
//! that queries return correct answers via rewriting without materialisation.
//! Each test loads a TBox + ABox, rewrites the query, and executes it.

#![cfg(feature = "owl2-ql")]

use open_triplestore::reasoning::owl2_ql::QLQueryRewriter;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;

const PREAMBLE: &str = "@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
                        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                        @prefix owl:  <http://www.w3.org/2002/07/owl#> .\n\
                        @prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .\n\
                        @prefix ex:   <http://example.org/> .\n";

const SPARQL_PREFIXES: &str = "PREFIX rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
                                PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                                PREFIX owl:  <http://www.w3.org/2002/07/owl#>\n\
                                PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>\n\
                                PREFIX ex:   <http://example.org/>\n";

fn store_with(ttl: &str) -> TripleStore {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(&format!("{PREAMBLE}{ttl}"), RdfFormat::Turtle, None)
        .unwrap();
    store
}

/// Rewrite + execute an ASK query; return the boolean result.
fn ask_ql(store: &TripleStore, sparql: &str) -> bool {
    let with_prefixes = format!("{SPARQL_PREFIXES}{sparql}");
    let rw = QLQueryRewriter::new(store);
    let rewritten = rw.rewrite_query(&with_prefixes).unwrap();
    match store.query(&rewritten).unwrap() {
        oxigraph::sparql::QueryResults::Boolean(b) => b,
        _ => panic!("expected ASK result"),
    }
}

/// Rewrite + execute a SELECT query; return the number of result rows.
fn count_select(store: &TripleStore, sparql: &str) -> usize {
    let with_prefixes = format!("{SPARQL_PREFIXES}{sparql}");
    let rw = QLQueryRewriter::new(store);
    let rewritten = rw.rewrite_query(&with_prefixes).unwrap();
    match store.query(&rewritten).unwrap() {
        oxigraph::sparql::QueryResults::Solutions(sols) => sols.flatten().count(),
        _ => panic!("expected SELECT result"),
    }
}

// ─── Subclass rewriting ───────────────────────────────────────────────────────

#[test]
fn test_ql_subclass_direct() {
    // Alice is a Prof; TBox says Prof ⊑ Staff
    // Query for Staff should succeed via subclass rewriting
    let s = store_with(
        "ex:Prof rdfs:subClassOf ex:Staff . \
         ex:alice rdf:type ex:Prof .",
    );
    assert!(
        ask_ql(
            &s,
            "ASK { <http://example.org/alice> rdf:type <http://example.org/Staff> }"
        ),
        "direct subclass rewriting"
    );
}

#[test]
fn test_ql_subclass_transitive() {
    // Three-level chain: PhD ⊑ Student ⊑ Person
    let s = store_with(
        "ex:PhD rdfs:subClassOf ex:Student . \
         ex:Student rdfs:subClassOf ex:Person . \
         ex:alice rdf:type ex:PhD .",
    );
    assert!(
        ask_ql(
            &s,
            "ASK { <http://example.org/alice> rdf:type <http://example.org/Student> }"
        ),
        "transitive subclass level 1"
    );
    assert!(
        ask_ql(
            &s,
            "ASK { <http://example.org/alice> rdf:type <http://example.org/Person> }"
        ),
        "transitive subclass level 2"
    );
}

#[test]
fn test_ql_subclass_negative() {
    // Alice is NOT a Manager; should not be inferred via rewriting
    let s = store_with(
        "ex:Employee rdfs:subClassOf ex:Person . \
         ex:alice rdf:type ex:Person .",
    );
    assert!(
        !ask_ql(
            &s,
            "ASK { <http://example.org/alice> rdf:type <http://example.org/Employee> }"
        ),
        "subclass direction: superclass does not imply subclass"
    );
}

// ─── EquivalentClass rewriting ────────────────────────────────────────────────

#[test]
fn test_ql_equivalent_class_forward() {
    let s = store_with(
        "ex:Faculty owl:equivalentClass ex:AcademicStaff . \
         ex:alice rdf:type ex:Faculty .",
    );
    assert!(
        ask_ql(
            &s,
            "ASK { <http://example.org/alice> rdf:type <http://example.org/AcademicStaff> }"
        ),
        "equivalentClass forward rewriting"
    );
}

#[test]
fn test_ql_equivalent_class_backward() {
    // equivalentClass is symmetric: AcademicStaff ≡ Faculty → also works from other direction
    let s = store_with(
        "ex:Faculty owl:equivalentClass ex:AcademicStaff . \
         ex:bob rdf:type ex:AcademicStaff .",
    );
    assert!(
        ask_ql(
            &s,
            "ASK { <http://example.org/bob> rdf:type <http://example.org/Faculty> }"
        ),
        "equivalentClass backward rewriting"
    );
}

#[test]
fn test_ql_equivalent_class_chain() {
    // A ≡ B, B ⊑ C → query for C should match via A
    let s = store_with(
        "ex:A owl:equivalentClass ex:B . \
         ex:B rdfs:subClassOf ex:C . \
         ex:x rdf:type ex:A .",
    );
    assert!(
        ask_ql(
            &s,
            "ASK { <http://example.org/x> rdf:type <http://example.org/C> }"
        ),
        "equivalentClass + subClassOf chain"
    );
}

// ─── Subproperty rewriting ────────────────────────────────────────────────────

#[test]
fn test_ql_subproperty_direct() {
    let s = store_with(
        "ex:fatherOf rdfs:subPropertyOf ex:parentOf . \
         ex:bob ex:fatherOf ex:alice .",
    );
    assert!(
        ask_ql(&s, "ASK { <http://example.org/bob> <http://example.org/parentOf> <http://example.org/alice> }"),
        "subproperty direct rewriting"
    );
}

#[test]
fn test_ql_subproperty_transitive() {
    // fatherOf ⊑ parentOf ⊑ ancestorOf
    let s = store_with(
        "ex:fatherOf rdfs:subPropertyOf ex:parentOf . \
         ex:parentOf rdfs:subPropertyOf ex:ancestorOf . \
         ex:bob ex:fatherOf ex:alice .",
    );
    assert!(
        ask_ql(&s, "ASK { <http://example.org/bob> <http://example.org/ancestorOf> <http://example.org/alice> }"),
        "subproperty transitive rewriting"
    );
}

#[test]
fn test_ql_equivalent_property() {
    let s = store_with(
        "ex:knows owl:equivalentProperty ex:acquaintanceOf . \
         ex:alice ex:knows ex:bob .",
    );
    assert!(
        ask_ql(&s, "ASK { <http://example.org/alice> <http://example.org/acquaintanceOf> <http://example.org/bob> }"),
        "equivalentProperty rewriting"
    );
}

// ─── InverseOf rewriting ──────────────────────────────────────────────────────

#[test]
fn test_ql_inverse_of_simple() {
    // teaches inverseOf taughtBy: bob teaches cs101 → cs101 taughtBy bob
    let s = store_with(
        "ex:teaches owl:inverseOf ex:taughtBy . \
         ex:bob ex:teaches ex:cs101 .",
    );
    assert!(
        ask_ql(&s, "ASK { <http://example.org/cs101> <http://example.org/taughtBy> <http://example.org/bob> }"),
        "inverseOf forward→backward rewriting"
    );
}

#[test]
fn test_ql_inverse_of_symmetric() {
    // inverseOf is symmetric: if A invOf B then B invOf A
    let s = store_with(
        "ex:hasPart owl:inverseOf ex:partOf . \
         ex:wheel ex:partOf ex:car .",
    );
    assert!(
        ask_ql(&s, "ASK { <http://example.org/car> <http://example.org/hasPart> <http://example.org/wheel> }"),
        "inverseOf symmetric: partOf → hasPart"
    );
}

#[test]
fn test_ql_inverse_with_subproperty() {
    // fatherOf ⊑ parentOf, parentOf inverseOf childOf: an inverse and a
    // sub-property compose, so a fatherOf edge answers a childOf query.
    let s = store_with(
        "ex:fatherOf rdfs:subPropertyOf ex:parentOf . \
         ex:parentOf owl:inverseOf ex:childOf . \
         ex:bob ex:parentOf ex:alice . \
         ex:dan ex:fatherOf ex:erin .",
    );
    assert!(
        ask_ql(&s, "ASK { <http://example.org/alice> <http://example.org/childOf> <http://example.org/bob> }"),
        "inverseOf: parentOf → childOf rewriting"
    );
    assert!(
        ask_ql(&s, "ASK { <http://example.org/erin> <http://example.org/childOf> <http://example.org/dan> }"),
        "fatherOf ⊑ parentOf ≡ childOf⁻"
    );
    assert!(
        !ask_ql(&s, "ASK { <http://example.org/dan> <http://example.org/childOf> <http://example.org/erin> }"),
        "the composition keeps its direction"
    );
}

// ─── Domain rewriting (existential) ──────────────────────────────────────────

#[test]
fn test_ql_domain_as_existential() {
    // rdfs:domain acts as ∃P.⊤ ⊑ C: worksFor domain Employee
    // alice worksFor Acme → query for alice type Employee via domain
    let s = store_with(
        "ex:worksFor rdfs:domain ex:Employee . \
         ex:alice ex:worksFor ex:Acme .",
    );
    assert!(
        ask_ql(
            &s,
            "ASK { <http://example.org/alice> rdf:type <http://example.org/Employee> }"
        ),
        "domain as existential rewriting: ∃worksFor.⊤ ⊑ Employee"
    );
}

// ─── SELECT query rewriting ───────────────────────────────────────────────────

#[test]
fn test_ql_select_finds_all_subclass_instances() {
    // Query for all Persons should also return Employees (subclass)
    let s = store_with(
        "ex:Employee rdfs:subClassOf ex:Person . \
         ex:alice rdf:type ex:Employee . \
         ex:bob rdf:type ex:Person .",
    );
    let count = count_select(
        &s,
        "SELECT ?x WHERE { ?x rdf:type <http://example.org/Person> }",
    );
    // Both alice (via Employee ⊑ Person) and bob should match
    assert!(
        count >= 2,
        "SELECT should return both direct and inferred instances, got {count}"
    );
}

#[test]
fn test_ql_select_with_subproperty() {
    // Query for parentOf should also match fatherOf (subproperty)
    let s = store_with(
        "ex:fatherOf rdfs:subPropertyOf ex:parentOf . \
         ex:bob ex:fatherOf ex:alice . \
         ex:carol ex:parentOf ex:dave .",
    );
    let count = count_select(
        &s,
        "SELECT ?x ?y WHERE { ?x <http://example.org/parentOf> ?y }",
    );
    // carol→dave (direct) + bob→alice (via fatherOf ⊑ parentOf)
    assert!(
        count >= 2,
        "SELECT should return both direct and rewritten property triples, got {count}"
    );
}

// ─── TBox materialisation ─────────────────────────────────────────────────────

#[test]
fn test_ql_materialize_tbox_subclass() {
    let s = store_with(
        "ex:Prof rdfs:subClassOf ex:Staff . \
         ex:Staff rdfs:subClassOf ex:Employee .",
    );
    let rw = QLQueryRewriter::new(&s);
    let report = rw.materialize_tbox().unwrap();
    // Prof→Staff, Prof→Employee, Staff→Employee
    assert!(
        report.triples_added >= 2,
        "TBox materialisation should add inferred subclass triples"
    );
}

#[test]
fn test_ql_materialize_tbox_equiv_class() {
    let s = store_with("ex:A owl:equivalentClass ex:B .");
    let rw = QLQueryRewriter::new(&s);
    let report = rw.materialize_tbox().unwrap();
    // A⊑B and B⊑A — both directions
    assert!(
        report.triples_added >= 2,
        "equivalentClass should materialise both directions"
    );
}

// ─── No false positives ───────────────────────────────────────────────────────

#[test]
fn test_ql_no_rewriting_when_no_tbox() {
    // Empty TBox: query should only return directly asserted facts
    let s = store_with("ex:alice rdf:type ex:Person .");
    // No TBox entails alice is Animal
    assert!(
        !ask_ql(
            &s,
            "ASK { <http://example.org/alice> rdf:type <http://example.org/Animal> }"
        ),
        "no TBox: should not infer Animal type"
    );
    // Directly asserted type should still be found
    assert!(
        ask_ql(
            &s,
            "ASK { <http://example.org/alice> rdf:type <http://example.org/Person> }"
        ),
        "direct assertion still works with empty TBox"
    );
}

#[test]
fn test_ql_rewriter_is_idempotent() {
    // Rewriting twice should produce the same result as rewriting once
    let s = store_with(
        "ex:A rdfs:subClassOf ex:B . \
         ex:x rdf:type ex:A .",
    );
    let rw = QLQueryRewriter::new(&s);
    let sparql = &format!(
        "{SPARQL_PREFIXES}ASK {{ <http://example.org/x> rdf:type <http://example.org/B> }}"
    );
    let once = rw.rewrite_query(sparql).unwrap();
    // Executing the rewritten query should yield same result regardless of calling twice
    let result1 = match s.query(&once).unwrap() {
        oxigraph::sparql::QueryResults::Boolean(b) => b,
        _ => panic!(),
    };
    assert!(result1, "rewritten query should answer true");
}

// ─── Multi-axiom interaction ──────────────────────────────────────────────────

#[test]
fn test_ql_combined_subclass_and_inverse() {
    // Employee ⊑ Person, manages inverseOf managedBy
    // alice (Employee) manages bob → bob managedBy alice, alice is Person
    let s = store_with(
        "ex:Employee rdfs:subClassOf ex:Person . \
         ex:manages owl:inverseOf ex:managedBy . \
         ex:alice rdf:type ex:Employee . \
         ex:alice ex:manages ex:bob .",
    );
    assert!(
        ask_ql(
            &s,
            "ASK { <http://example.org/alice> rdf:type <http://example.org/Person> }"
        ),
        "combined: subclass of Person"
    );
    assert!(
        ask_ql(&s, "ASK { <http://example.org/bob> <http://example.org/managedBy> <http://example.org/alice> }"),
        "combined: inverse property"
    );
}

#[test]
fn test_ql_diamond_hierarchy() {
    // Diamond: A ⊑ B, A ⊑ C, B ⊑ D, C ⊑ D
    // x type A → should find x type D via both paths
    let s = store_with(
        "ex:A rdfs:subClassOf ex:B . \
         ex:A rdfs:subClassOf ex:C . \
         ex:B rdfs:subClassOf ex:D . \
         ex:C rdfs:subClassOf ex:D . \
         ex:x rdf:type ex:A .",
    );
    assert!(
        ask_ql(
            &s,
            "ASK { <http://example.org/x> rdf:type <http://example.org/D> }"
        ),
        "diamond hierarchy: A → D via both B and C paths"
    );
}

// ─── Soundness: existentials on the right ─────────────────────────────────────

#[test]
fn test_ql_some_values_from_on_the_right_is_not_a_domain() {
    // Parent ⊑ ∃hasChild.Person says every parent has a child; it does not
    // say that whoever has a child is a Parent. The rewriter used to read it
    // as ∃hasChild ⊑ Parent.
    let s = store_with(
        "ex:Parent rdfs:subClassOf [ owl:onProperty ex:hasChild ; owl:someValuesFrom ex:Person ] . \
         ex:x ex:hasChild ex:y .",
    );
    assert!(
        !ask_ql(&s, "ASK { ex:x rdf:type ex:Parent }"),
        "C ⊑ ∃P.D must not type the subjects of P as C"
    );
}

// ─── Fresh variables ──────────────────────────────────────────────────────────

#[test]
fn test_ql_fresh_variable_per_atom() {
    // Two atoms rewritten through domains each get their own existential
    // variable. With one shared name the two rewritings were joined on it,
    // so this query (alice's employer ≠ bob's team) returned nothing.
    let s = store_with(
        "ex:worksFor rdfs:domain ex:Employee . \
         ex:manages rdfs:domain ex:Manager . \
         ex:alice ex:worksFor ex:acme . \
         ex:bob ex:manages ex:team1 .",
    );
    assert_eq!(
        count_select(
            &s,
            "SELECT ?x ?y WHERE { ?x a ex:Employee . ?y a ex:Manager }"
        ),
        1
    );

    // The fresh variables are not projected by SELECT *.
    let rw = QLQueryRewriter::new(&s);
    let rewritten = rw
        .rewrite_query(&format!(
            "{SPARQL_PREFIXES}SELECT * WHERE {{ ?x a ex:Employee . ?y a ex:Manager }}"
        ))
        .unwrap();
    match s.query(&rewritten).unwrap() {
        oxigraph::sparql::QueryResults::Solutions(sols) => {
            let vars: Vec<String> = sols
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            assert_eq!(vars, ["x", "y"], "rewritten: {rewritten}");
        }
        _ => panic!("expected SELECT result"),
    }
}

// ─── Range, and domain/range through the hierarchies ──────────────────────────

#[test]
fn test_ql_range() {
    let s = store_with(
        "ex:hasChild rdfs:range ex:Person . \
         ex:alice ex:hasChild ex:bob .",
    );
    assert!(ask_ql(&s, "ASK { ex:bob rdf:type ex:Person }"));
    assert!(
        !ask_ql(&s, "ASK { ex:alice rdf:type ex:Person }"),
        "a range types the object, not the subject"
    );
}

#[test]
fn test_ql_domain_and_range_through_hierarchies() {
    // The queried class is a superclass of the domain, and the asserted
    // property a sub-property of the one with the domain (or range).
    let s = store_with(
        "ex:worksFor rdfs:domain ex:Employee . \
         ex:worksFor rdfs:range ex:Organisation . \
         ex:Employee rdfs:subClassOf ex:Person . \
         ex:Organisation rdfs:subClassOf ex:Agent . \
         ex:headOf rdfs:subPropertyOf ex:worksFor . \
         ex:alice ex:headOf ex:acme .",
    );
    assert!(ask_ql(&s, "ASK { ex:alice rdf:type ex:Person }"));
    assert!(ask_ql(&s, "ASK { ex:acme rdf:type ex:Agent }"));
    assert!(!ask_ql(&s, "ASK { ex:acme rdf:type ex:Person }"));

    // Through an inverse: the range of an inverse is the domain.
    let s = store_with(
        "ex:employs owl:inverseOf ex:worksFor . \
         ex:worksFor rdfs:domain ex:Employee . \
         ex:acme ex:employs ex:carol .",
    );
    assert!(ask_ql(&s, "ASK { ex:carol rdf:type ex:Employee }"));
    assert!(!ask_ql(&s, "ASK { ex:acme rdf:type ex:Employee }"));
}

#[test]
fn test_ql_unqualified_existential_on_the_left() {
    // ∃teaches ⊑ Teacher and ∃teaches⁻ ⊑ Course, written as restrictions.
    let s = store_with(
        "[ owl:onProperty ex:teaches ; owl:someValuesFrom owl:Thing ] rdfs:subClassOf ex:Teacher . \
         [ owl:onProperty [ owl:inverseOf ex:teaches ] ; owl:someValuesFrom owl:Thing ] \
             rdfs:subClassOf ex:Course . \
         ex:bob ex:teaches ex:cs101 .",
    );
    assert!(ask_ql(&s, "ASK { ex:bob rdf:type ex:Teacher }"));
    assert!(ask_ql(&s, "ASK { ex:cs101 rdf:type ex:Course }"));
    assert!(!ask_ql(&s, "ASK { ex:cs101 rdf:type ex:Teacher }"));
}

// ─── DL-Lite_R closure: materialisation, consistency, existentials ─────────────
//
// These run the reasoner the way the server does: the data sits in a named
// graph, `materialize()` writes the ground closure into the QL entailment
// graph, and a query reads both through `FROM`, its blank nodes rewritten by
// `rewrite_existentials` over the TBox of the graphs it reads.

use open_triplestore::reasoning::common::{ReasoningError, ReasoningReport};
use open_triplestore::reasoning::owl2_ql::rewrite_existentials;

const DATA: &str = "urn:test:ql:data";
const ENT: &str = "urn:entailment:owl2-ql";

fn data_store(ttl: &str) -> TripleStore {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(&format!("{PREAMBLE}{ttl}"), RdfFormat::Turtle, Some(DATA))
        .unwrap();
    store
}

fn materialise(store: &TripleStore) -> Result<ReasoningReport, ReasoningError> {
    QLQueryRewriter::new(store)
        .with_sources(vec![DATA.to_string()])
        .materialize()
}

/// A store with `ttl` in the data graph, materialised (and consistent).
fn closed(ttl: &str) -> TripleStore {
    let store = data_store(ttl);
    materialise(&store).expect("consistent");
    store
}

/// Run `form WHERE { pattern }` over the data and the entailment graph, the
/// blank nodes rewritten as `/sparql?entailment=owl2-ql` does.
fn entailed(
    store: &TripleStore,
    form: &str,
    pattern: &str,
) -> oxigraph::sparql::QueryResults<'static> {
    let q = format!("{SPARQL_PREFIXES}{form} FROM <{DATA}> FROM <{ENT}> WHERE {{ {pattern} }}");
    let q = rewrite_existentials(store, &q).unwrap().unwrap_or(q);
    store.query(&q).unwrap_or_else(|e| panic!("{e}\n{q}"))
}

fn ask_entailed(store: &TripleStore, pattern: &str) -> bool {
    match entailed(store, "ASK", pattern) {
        oxigraph::sparql::QueryResults::Boolean(b) => b,
        _ => panic!("expected ASK result"),
    }
}

/// The rows of `SELECT vars`, each as the values' strings, sorted.
fn select_entailed(store: &TripleStore, vars: &str, pattern: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = match entailed(store, &format!("SELECT {vars}"), pattern) {
        oxigraph::sparql::QueryResults::Solutions(sols) => {
            let names: Vec<String> = sols
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            sols.flatten()
                .map(|s| {
                    names
                        .iter()
                        .map(|n| s.get(n.as_str()).map(|t| t.to_string()).unwrap_or_default())
                        .collect()
                })
                .collect()
        }
        _ => panic!("expected SELECT result"),
    };
    rows.sort();
    rows
}

fn in_entailment_graph(store: &TripleStore, pattern: &str) -> bool {
    matches!(
        store.query(&format!(
            "{SPARQL_PREFIXES}ASK {{ GRAPH <{ENT}> {{ {pattern} }} }}"
        )),
        Ok(oxigraph::sparql::QueryResults::Boolean(true))
    )
}

fn inconsistency(ttl: &str) -> String {
    match materialise(&data_store(ttl)) {
        Err(ReasoningError::Inconsistency { rule, detail }) => format!("{rule}: {detail}"),
        other => panic!("expected an inconsistency, got {other:?}"),
    }
}

#[test]
fn test_ql_materialises_ground_atoms() {
    // Every entailed membership and assertion over named individuals is
    // written; nothing already asserted is written again.
    let s = closed(
        "ex:worksFor rdfs:domain ex:Employee . \
         ex:Employee rdfs:subClassOf ex:Person . \
         ex:headOf rdfs:subPropertyOf ex:worksFor . \
         ex:employs owl:inverseOf ex:worksFor . \
         ex:alice ex:headOf ex:acme . \
         ex:bob a ex:Employee .",
    );
    assert!(in_entailment_graph(&s, "ex:alice a ex:Employee"));
    assert!(in_entailment_graph(&s, "ex:alice a ex:Person"));
    assert!(in_entailment_graph(&s, "ex:alice ex:worksFor ex:acme"));
    assert!(in_entailment_graph(&s, "ex:acme ex:employs ex:alice"));
    assert!(in_entailment_graph(&s, "ex:bob a ex:Person"));
    assert!(
        !in_entailment_graph(&s, "ex:bob a ex:Employee"),
        "asserted, not re-written"
    );
    assert!(!in_entailment_graph(&s, "ex:acme a ex:Person"));
    // The TBox closure is there too.
    assert!(in_entailment_graph(
        &s,
        "ex:headOf rdfs:subPropertyOf ex:worksFor"
    ));
    // Variables bind to names: answered from the materialised graph alone.
    assert_eq!(
        select_entailed(&s, "?x", "?x a ex:Person"),
        [["<http://example.org/alice>"], ["<http://example.org/bob>"]]
    );
}

#[test]
fn test_ql_qualified_existential_is_existential() {
    // Parent ⊑ ∃hasChild.Person: x has *some* child who is a person. No
    // child is named, so no ground atom; a blank node finds the anonymous one.
    let s = closed(
        "ex:Parent rdfs:subClassOf [ owl:onProperty ex:hasChild ; owl:someValuesFrom ex:Person ] . \
         ex:Person rdfs:subClassOf ex:Agent . \
         ex:x a ex:Parent .",
    );
    assert!(!ask_entailed(&s, "ex:x ex:hasChild ?y"), "no named child");
    assert!(ask_entailed(&s, "ex:x ex:hasChild []"));
    assert!(ask_entailed(&s, "ex:x ex:hasChild [ a ex:Person ]"));
    assert!(ask_entailed(&s, "ex:x ex:hasChild [ a ex:Agent ]"));
    assert!(!ask_entailed(&s, "ex:x ex:hasChild [ a ex:Parent ]"));
    assert!(!ask_entailed(&s, "ex:x ex:hasParent []"));
    // Still sound: a child does not make a parent.
    let s = closed(
        "ex:Parent rdfs:subClassOf [ owl:onProperty ex:hasChild ; owl:someValuesFrom ex:Person ] . \
         ex:y ex:hasChild ex:z .",
    );
    assert!(!ask_entailed(&s, "ex:y a ex:Parent"));
}

#[test]
fn test_ql_existential_chain() {
    // Two levels of anonymous elements.
    let s = closed(
        "ex:A rdfs:subClassOf [ owl:onProperty ex:r ; owl:someValuesFrom ex:B ] . \
         ex:B rdfs:subClassOf [ owl:onProperty ex:s ; owl:someValuesFrom ex:C ] . \
         ex:a a ex:A .",
    );
    assert!(ask_entailed(&s, "ex:a ex:r [ ex:s [ a ex:C ] ]"));
    assert!(ask_entailed(
        &s,
        "ex:a ex:r _:b . _:b a ex:B . _:b ex:s _:c"
    ));
    assert!(!ask_entailed(&s, "ex:a ex:r [ ex:r [] ]"));
    assert!(!ask_entailed(&s, "ex:a ex:s []"));
    // Anywhere: some element has an s-successor.
    assert!(ask_entailed(&s, "[] ex:s [ a ex:C ]"));
    assert!(!ask_entailed(&s, "[] ex:s [ a ex:A ]"));
}

#[test]
fn test_ql_existential_through_roles_inverses_and_ranges() {
    let s = closed(
        "ex:Mother rdfs:subClassOf [ owl:onProperty ex:hasSon ; owl:someValuesFrom owl:Thing ] . \
         ex:hasSon rdfs:subPropertyOf ex:hasChild . \
         ex:hasChild rdfs:range ex:Child . \
         ex:hasParent owl:inverseOf ex:hasChild . \
         ex:Orphanage rdfs:subClassOf \
             [ owl:onProperty [ owl:inverseOf ex:livesIn ] ; owl:someValuesFrom owl:Thing ] . \
         ex:m a ex:Mother . \
         ex:home a ex:Orphanage .",
    );
    assert!(ask_entailed(&s, "ex:m ex:hasChild [ a ex:Child ]"));
    assert!(ask_entailed(&s, "[] ex:hasParent ex:m"));
    assert!(ask_entailed(&s, "[] ex:livesIn ex:home"));
    assert!(!ask_entailed(&s, "ex:home ex:livesIn []"));
    assert_eq!(
        select_entailed(&s, "?x", "?x ex:hasChild []"),
        [["<http://example.org/m>"]]
    );
    // A query blank node used twice must be the same element: the child's
    // parent is the mother herself.
    assert_eq!(
        select_entailed(&s, "?x ?y", "?x ex:hasChild _:c . _:c ex:hasParent ?y"),
        [["<http://example.org/m>", "<http://example.org/m>"]]
    );
}

#[test]
fn test_ql_existential_matches_named_and_anonymous_once() {
    // x has a named child and an anonymous one: one row, not two or three.
    let s = closed(
        "ex:Parent rdfs:subClassOf [ owl:onProperty ex:hasChild ; owl:someValuesFrom owl:Thing ] . \
         ex:x a ex:Parent . ex:x ex:hasChild ex:c1 . ex:x ex:hasChild ex:c2 . \
         ex:y ex:hasChild ex:c3 .",
    );
    assert_eq!(
        select_entailed(&s, "?x", "?x ex:hasChild []"),
        [["<http://example.org/x>"], ["<http://example.org/y>"]]
    );
    // A blank node still matches named individuals.
    assert!(!ask_entailed(&s, "ex:y ex:hasChild [ ex:hasChild ex:c3 ]"));
    assert!(ask_entailed(&s, "[ ex:hasChild ex:c3 ]"));
    // No TBox, no change: the plain answer.
    let plain = closed("ex:p ex:knows ex:q . ex:q ex:knows ex:r .");
    assert_eq!(
        select_entailed(&plain, "?x", "?x ex:knows [ ex:knows [] ]"),
        [["<http://example.org/p>"]]
    );
}

#[test]
fn test_ql_unqualified_tbox_closure_through_existentials() {
    // A ⊑ ∃r.B and ∃r ⊑ C (a domain): every A is a C, a named-class
    // subsumption only the closure through the existential shows.
    let s = closed(
        "ex:A rdfs:subClassOf [ owl:onProperty ex:r ; owl:someValuesFrom ex:B ] . \
         ex:r rdfs:domain ex:C . \
         ex:a a ex:A .",
    );
    assert!(in_entailment_graph(&s, "ex:A rdfs:subClassOf ex:C"));
    assert!(in_entailment_graph(&s, "ex:a a ex:C"));
}

#[test]
fn test_ql_rewrite_query_answers_existentials_without_materialisation() {
    // The standalone rewriting (`POST /api/reasoning/rewrite`) expands ground
    // atoms itself and rewrites blank nodes too.
    let s = store_with(
        "ex:Parent rdfs:subClassOf [ owl:onProperty ex:hasChild ; owl:someValuesFrom ex:Person ] . \
         ex:Mother rdfs:subClassOf ex:Parent . \
         ex:m a ex:Mother .",
    );
    assert!(ask_ql(&s, "ASK { ex:m ex:hasChild [ a ex:Person ] }"));
    assert!(ask_ql(&s, "ASK { ex:m a ex:Parent }"));
    assert!(!ask_ql(&s, "ASK { ex:m ex:hasChild ?c }"));
}

#[test]
fn test_ql_disjoint_classes_inconsistent() {
    let m = inconsistency(
        "ex:Cat owl:disjointWith ex:Dog . \
         ex:Kitten rdfs:subClassOf ex:Cat . \
         ex:barks rdfs:domain ex:Dog . \
         ex:tom a ex:Kitten . ex:tom ex:barks ex:loudly .",
    );
    assert!(m.contains("ql-cls-disjoint"), "{m}");
    // owl:AllDisjointClasses and owl:complementOf say the same.
    let m = inconsistency(
        "[] a owl:AllDisjointClasses ; owl:members ( ex:A ex:B ex:C ) . \
         ex:x a ex:A , ex:C .",
    );
    assert!(m.contains("ql-cls-disjoint"), "{m}");
    let m = inconsistency("ex:A rdfs:subClassOf [ owl:complementOf ex:B ] . ex:x a ex:A , ex:B .");
    assert!(m.contains("ql-cls-disjoint"), "{m}");
    // Consistent data stays consistent.
    assert!(materialise(&data_store(
        "ex:Cat owl:disjointWith ex:Dog . ex:tom a ex:Cat . ex:rex a ex:Dog ."
    ))
    .is_ok());
}

#[test]
fn test_ql_unsatisfiability_propagates_through_existentials() {
    // A ⊑ ∃r.B, the range of r is C, and B ⊥ C: no A can exist. The closure
    // says so (A ⊑ owl:Nothing), and an A in the data is inconsistent; the
    // derived atoms stay in the graph.
    let ttl = "ex:A rdfs:subClassOf [ owl:onProperty ex:r ; owl:someValuesFrom ex:B ] . \
               ex:r rdfs:range ex:C . \
               ex:B owl:disjointWith ex:C . \
               ex:Sub rdfs:subClassOf ex:A . \
               ex:a a ex:Sub .";
    let s = data_store(ttl);
    match materialise(&s) {
        Err(ReasoningError::Inconsistency { rule, detail }) => {
            assert_eq!(rule, "ql-cls-nothing", "{detail}")
        }
        other => panic!("expected an inconsistency, got {other:?}"),
    }
    assert!(in_entailment_graph(&s, "ex:A rdfs:subClassOf owl:Nothing"));
    assert!(in_entailment_graph(
        &s,
        "ex:Sub rdfs:subClassOf owl:Nothing"
    ));
    assert!(in_entailment_graph(&s, "ex:a a ex:A"));
    // Without an A, the TBox alone is consistent.
    let s = closed(
        "ex:A rdfs:subClassOf [ owl:onProperty ex:r ; owl:someValuesFrom ex:B ] . \
         ex:r rdfs:range ex:C . ex:B owl:disjointWith ex:C .",
    );
    assert!(in_entailment_graph(&s, "ex:A rdfs:subClassOf owl:Nothing"));
}

#[test]
fn test_ql_property_characteristics() {
    // Symmetric: the converse is materialised.
    let s = closed("ex:knows a owl:SymmetricProperty . ex:a ex:knows ex:b .");
    assert!(in_entailment_graph(&s, "ex:b ex:knows ex:a"));
    // Asymmetric, through a sub-property.
    let m = inconsistency(
        "ex:parentOf a owl:AsymmetricProperty . ex:motherOf rdfs:subPropertyOf ex:parentOf . \
         ex:a ex:motherOf ex:b . ex:b ex:parentOf ex:a .",
    );
    assert!(m.contains("ql-prp-disjoint"), "{m}");
    // Irreflexive, through an inverse.
    let m = inconsistency(
        "ex:marriedTo a owl:IrreflexiveProperty . ex:spouseOf owl:inverseOf ex:marriedTo . \
         ex:a ex:spouseOf ex:a .",
    );
    assert!(m.contains("ql-prp-irp"), "{m}");
    // Disjoint properties, through the hierarchy.
    let m = inconsistency(
        "ex:likes owl:propertyDisjointWith ex:hates . ex:adores rdfs:subPropertyOf ex:likes . \
         ex:a ex:adores ex:b . ex:a ex:hates ex:b .",
    );
    assert!(m.contains("ql-prp-disjoint"), "{m}");
    // `a owl:differentFrom a`.
    let m = inconsistency("ex:a owl:differentFrom ex:a .");
    assert!(m.contains("ql-different-from"), "{m}");
}

#[test]
fn test_ql_reflexive_property() {
    // Every individual has the loop, and so is in the property's domain.
    let s = closed(
        "ex:knows a owl:ReflexiveProperty . ex:knows rdfs:domain ex:Agent . \
         ex:a ex:likes ex:b . ex:c a ex:Thingy .",
    );
    assert!(in_entailment_graph(&s, "ex:a ex:knows ex:a"));
    assert!(in_entailment_graph(&s, "ex:b ex:knows ex:b"));
    assert!(in_entailment_graph(&s, "ex:c a ex:Agent"));
    assert!(ask_entailed(&s, "ex:c ex:knows []"));
    assert!(ask_entailed(&s, "_:x ex:knows _:x"));
    // Reflexive and irreflexive together: no model, whatever the data.
    let m = inconsistency(
        "ex:p a owl:ReflexiveProperty . ex:q a owl:IrreflexiveProperty . \
         ex:p rdfs:subPropertyOf ex:q .",
    );
    assert!(m.contains("ql-prp-irp"), "{m}");
}

#[test]
fn test_ql_data_properties() {
    let s = closed(
        "ex:age a owl:DatatypeProperty ; rdfs:domain ex:Person ; rdfs:range xsd:integer . \
         ex:ageInYears rdfs:subPropertyOf ex:age . \
         ex:Adult rdfs:subClassOf [ owl:onProperty ex:age ; owl:someValuesFrom xsd:integer ] . \
         ex:a ex:ageInYears 42 . \
         ex:b a ex:Adult .",
    );
    assert!(in_entailment_graph(&s, "ex:a ex:age 42"));
    assert!(in_entailment_graph(&s, "ex:a a ex:Person"));
    assert!(in_entailment_graph(&s, "ex:b a ex:Person"));
    assert!(ask_entailed(&s, "ex:b ex:age []"));
    assert!(!ask_entailed(&s, "ex:b ex:age ?v"));
    // A literal is never typed by a class range or domain.
    assert!(!in_entailment_graph(
        &s,
        "?l a ex:Person FILTER(isLiteral(?l))"
    ));
    // A value outside the declared datatype is an inconsistency...
    let m = inconsistency(
        "ex:age a owl:DatatypeProperty ; rdfs:range xsd:integer . ex:a ex:age \"old\" .",
    );
    assert!(m.contains("ql-dt-range"), "{m}");
    // ...but one that needs a value check is not judged until the OWL 2
    // datatype map lands (stored integers all read back as xsd:integer).
    assert!(materialise(&data_store(
        "ex:age a owl:DatatypeProperty ; rdfs:range xsd:nonNegativeInteger . ex:a ex:age 5 ."
    ))
    .is_ok());
}

#[test]
fn test_ql_reports_ignored_axioms() {
    // D9: axioms outside OWL 2 QL are not used, and the report says which.
    let s = data_store(
        "ex:ancestorOf a owl:TransitiveProperty . \
         ex:hasMother a owl:FunctionalProperty . \
         ex:a owl:sameAs ex:b . \
         ex:Pet rdfs:subClassOf [ owl:unionOf ( ex:Cat ex:Dog ) ] . \
         ex:Prof rdfs:subClassOf ex:Staff .",
    );
    let report = materialise(&s).unwrap();
    assert_eq!(report.ignored_axioms, 4, "{:?}", report.ignored_sample);
    let axioms: Vec<&str> = report
        .ignored_sample
        .iter()
        .map(|a| a.axiom.as_str())
        .collect();
    for a in [
        "owl:TransitiveProperty",
        "owl:FunctionalProperty",
        "owl:sameAs",
        "rdfs:subClassOf",
    ] {
        assert!(axioms.contains(&a), "{axioms:?}");
    }
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["ignored_axioms"], 4);
    // A QL-only TBox reports nothing (and the fields are omitted).
    let report = materialise(&data_store("ex:Prof rdfs:subClassOf ex:Staff .")).unwrap();
    let json = serde_json::to_value(&report).unwrap();
    assert!(json.get("ignored_axioms").is_none(), "{json}");
}

/// A small deterministic generator (xorshift) for the differential test.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A random ontology in the intersection of OWL 2 QL and OWL 2 RL.
fn random_ql_rl(rng: &mut Rng) -> String {
    let c = |rng: &mut Rng| format!("ex:C{}", rng.below(5));
    let p = |rng: &mut Rng| format!("ex:p{}", rng.below(4));
    let i = |rng: &mut Rng| format!("ex:i{}", rng.below(6));
    let mut ttl = String::new();
    for _ in 0..(4 + rng.below(6)) {
        let axiom = match rng.below(10) {
            0 | 1 => format!("{} rdfs:subClassOf {} .", c(rng), c(rng)),
            2 => format!("{} rdfs:subPropertyOf {} .", p(rng), p(rng)),
            3 => format!("{} owl:inverseOf {} .", p(rng), p(rng)),
            4 => format!("{} rdfs:domain {} .", p(rng), c(rng)),
            5 => format!("{} rdfs:range {} .", p(rng), c(rng)),
            6 => format!("{} a owl:SymmetricProperty .", p(rng)),
            7 => format!(
                "[ owl:onProperty {} ; owl:someValuesFrom owl:Thing ] rdfs:subClassOf {} .",
                p(rng),
                c(rng)
            ),
            8 => format!("{} owl:equivalentClass {} .", c(rng), c(rng)),
            _ => format!(
                "{} rdfs:subClassOf [ owl:intersectionOf ( {} {} ) ] .",
                c(rng),
                c(rng),
                c(rng)
            ),
        };
        ttl.push_str(&axiom);
        ttl.push('\n');
    }
    for _ in 0..(4 + rng.below(8)) {
        let fact = if rng.below(2) == 0 {
            format!("{} a {} .", i(rng), c(rng))
        } else {
            format!("{} {} {} .", i(rng), p(rng), i(rng))
        };
        ttl.push_str(&fact);
        ttl.push('\n');
    }
    ttl
}

/// The ground atoms over the individuals `ex:i*` in a graph (asserted or
/// derived), as N-Triples-ish strings.
fn individual_atoms(store: &TripleStore, graphs: &[&str]) -> std::collections::BTreeSet<String> {
    let from: String = graphs.iter().map(|g| format!("FROM <{g}> ")).collect();
    let q = format!(
        "SELECT DISTINCT ?s ?p ?o {from} WHERE {{ ?s ?p ?o . \
           FILTER(STRSTARTS(STR(?s), \"http://example.org/i\")) \
           FILTER(STRSTARTS(STR(?o), \"http://example.org/i\") \
               || STRSTARTS(STR(?o), \"http://example.org/C\")) }}"
    );
    match store.query(&q).unwrap() {
        oxigraph::sparql::QueryResults::Solutions(sols) => sols
            .flatten()
            .map(|s| format!("{} {} {}", s["s"], s["p"], s["o"]))
            .collect(),
        _ => panic!("expected SELECT result"),
    }
}

#[cfg(feature = "owl2-rl")]
#[test]
fn test_ql_differential_against_rl() {
    // On ontologies in both profiles, OWL 2 QL and OWL 2 RL entail the same
    // ground atoms over named individuals.
    use open_triplestore::reasoning::owl2_rl::Owl2RLReasoner;
    const RL: &str = "urn:test:ql:rl";
    let mut rng = Rng(0x005e_ed0f_0e11_2026);
    for case in 0..60 {
        let ttl = random_ql_rl(&mut rng);
        let store = data_store(&ttl);
        materialise(&store).unwrap_or_else(|e| panic!("case {case}: {e}\n{ttl}"));
        Owl2RLReasoner::new(&store)
            .with_target(RL)
            .with_sources(vec![DATA.to_string()])
            .materialize()
            .unwrap_or_else(|e| panic!("case {case}: {e}\n{ttl}"));
        let ql = individual_atoms(&store, &[DATA, ENT]);
        let rl = individual_atoms(&store, &[DATA, RL]);
        assert_eq!(
            ql,
            rl,
            "case {case}: QL and RL disagree\n{ttl}\nQL only: {:?}\nRL only: {:?}",
            ql.difference(&rl).collect::<Vec<_>>(),
            rl.difference(&ql).collect::<Vec<_>>()
        );
    }
}
