//! RDFS conformance tests.
//!
//! Tests each of the 13 RDFS entailment rules (rdfs1–rdfs13) individually
//! and in combination, verifying that the `RdfsMaterializer` produces the
//! correct inferred triples.

#![cfg(feature = "rdfs-entailment")]

use open_triplestore::store::TripleStore;
use open_triplestore::reasoning::rdfs::RdfsMaterializer;
use oxigraph::io::RdfFormat;

const TG: &str = "urn:entailment:rdfs";

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

fn materialize(store: &TripleStore) {
    RdfsMaterializer::with_target(store, TG).materialize().unwrap();
}

fn ask(store: &TripleStore, sparql: &str) -> bool {
    match store.query(sparql).unwrap() {
        oxigraph::sparql::QueryResults::Boolean(b) => b,
        _ => panic!("expected ASK result"),
    }
}

fn ask_in_tg(store: &TripleStore, pattern: &str) -> bool {
    ask(store, &format!(
        "PREFIX rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
         PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
         PREFIX owl:  <http://www.w3.org/2002/07/owl#>\n\
         PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>\n\
         ASK {{ GRAPH <{TG}> {{ {pattern} }} }}"
    ))
}

// ─── rdfs2: property domain ───────────────────────────────────────────────────

#[test]
fn test_rdfs2_domain_basic() {
    let s = store_with("ex:worksFor rdfs:domain ex:Employee . ex:alice ex:worksFor ex:Acme .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/Employee> ."),
        "rdfs2: domain should type subject");
}

#[test]
fn test_rdfs2_domain_chain() {
    // Domain + subClassOf chain: typing should propagate
    let s = store_with("ex:worksFor rdfs:domain ex:Employee . \
                        ex:Employee rdfs:subClassOf ex:Person . \
                        ex:alice ex:worksFor ex:Acme .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/Employee> ."),
        "rdfs2 base type");
    assert!(ask_in_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/Person> ."),
        "rdfs9 propagation via subClassOf");
}

// ─── rdfs3: property range ────────────────────────────────────────────────────

#[test]
fn test_rdfs3_range_basic() {
    let s = store_with("ex:hasChild rdfs:range ex:Person . ex:bob ex:hasChild ex:carol .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/carol> rdf:type <http://example.org/Person> ."),
        "rdfs3: range should type object");
}

#[test]
fn test_rdfs3_range_iri_only() {
    // Range should NOT apply to literal objects
    let s = store_with("ex:name rdfs:range ex:Name . ex:alice ex:name \"Alice\" .");
    materialize(&s);
    // Literal "Alice" should not become type ex:Name (literals can't have rdf:type via rdfs3)
    assert!(!ask_in_tg(&s, "\"Alice\" rdf:type <http://example.org/Name> ."),
        "rdfs3: should not type literals");
}

// ─── rdfs5: subPropertyOf transitivity ───────────────────────────────────────

#[test]
fn test_rdfs5_subproperty_transitivity() {
    let s = store_with("ex:fatherOf rdfs:subPropertyOf ex:parentOf . \
                        ex:parentOf rdfs:subPropertyOf ex:ancestorOf .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/fatherOf> rdfs:subPropertyOf <http://example.org/ancestorOf> ."),
        "rdfs5: subPropertyOf should be transitive");
}

// ─── rdfs7: property inheritance ─────────────────────────────────────────────

#[test]
fn test_rdfs7_property_inheritance() {
    let s = store_with("ex:fatherOf rdfs:subPropertyOf ex:parentOf . \
                        ex:bob ex:fatherOf ex:alice .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/bob> <http://example.org/parentOf> <http://example.org/alice> ."),
        "rdfs7: triple should be inherited through subPropertyOf");
}

// ─── rdfs9: type inheritance ──────────────────────────────────────────────────

#[test]
fn test_rdfs9_type_inheritance() {
    let s = store_with("ex:Employee rdfs:subClassOf ex:Person . \
                        ex:alice rdf:type ex:Employee .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/Person> ."),
        "rdfs9: type should propagate through subClassOf");
}

#[test]
fn test_rdfs9_chain() {
    let s = store_with("ex:Manager rdfs:subClassOf ex:Employee . \
                        ex:Employee rdfs:subClassOf ex:Person . \
                        ex:carol rdf:type ex:Manager .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/carol> rdf:type <http://example.org/Employee> ."),
        "rdfs9 chain level 1");
    assert!(ask_in_tg(&s, "<http://example.org/carol> rdf:type <http://example.org/Person> ."),
        "rdfs9 chain level 2");
}

// ─── rdfs11: subClassOf transitivity ─────────────────────────────────────────

#[test]
fn test_rdfs11_subclass_transitivity() {
    let s = store_with("ex:Manager rdfs:subClassOf ex:Employee . \
                        ex:Employee rdfs:subClassOf ex:Person .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/Manager> rdfs:subClassOf <http://example.org/Person> ."),
        "rdfs11: subClassOf should be transitive");
}

#[test]
fn test_rdfs11_three_level() {
    let s = store_with("ex:A rdfs:subClassOf ex:B . ex:B rdfs:subClassOf ex:C . ex:C rdfs:subClassOf ex:D .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/A> rdfs:subClassOf <http://example.org/D> ."),
        "rdfs11: transitivity across three levels");
}

// ─── rdfs12: ContainerMembershipProperty ──────────────────────────────────────

#[test]
fn test_rdfs12_container_membership_property() {
    let s = store_with("ex:_1 rdf:type rdfs:ContainerMembershipProperty .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/_1> rdfs:subPropertyOf rdfs:member ."),
        "rdfs12: ContainerMembershipProperty → subPropertyOf rdfs:member");
}

// ─── rdfs13: Datatype ─────────────────────────────────────────────────────────

#[test]
fn test_rdfs13_datatype_subclass_literal() {
    let s = store_with("ex:MyDT rdf:type rdfs:Datatype .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/MyDT> rdfs:subClassOf rdfs:Literal ."),
        "rdfs13: Datatype → subClassOf rdfs:Literal");
}

// ─── rdfs1: Datatype axiom ────────────────────────────────────────────────────

#[test]
fn test_rdfs1_xsd_datatypes() {
    // After materialization xsd:integer should be subClassOf rdfs:Literal (axiomatic rule rdfs1)
    // Activated by the presence of any typed literal
    let s = store_with("ex:alice ex:age \"30\"^^xsd:integer .");
    materialize(&s);
    assert!(ask_in_tg(&s, "xsd:integer rdfs:subClassOf rdfs:Literal ."),
        "rdfs1: xsd:integer should be subClassOf rdfs:Literal");
}

// ─── rdfs6: property reflexivity (axiomatic) ─────────────────────────────────

#[test]
fn test_rdfs6_property_reflexive() {
    let s = store_with("ex:knows rdf:type rdf:Property .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/knows> rdfs:subPropertyOf <http://example.org/knows> ."),
        "rdfs6: every property is subPropertyOf itself");
}

// ─── rdfs8: class subClassOf Resource (axiomatic) ────────────────────────────

#[test]
fn test_rdfs8_class_subclass_resource() {
    let s = store_with("ex:Person rdf:type rdfs:Class .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/Person> rdfs:subClassOf rdfs:Resource ."),
        "rdfs8: every class should be subClassOf rdfs:Resource");
}

// ─── rdfs10: class reflexivity (axiomatic) ────────────────────────────────────

#[test]
fn test_rdfs10_class_reflexive() {
    let s = store_with("ex:Person rdf:type rdfs:Class .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/Person> rdfs:subClassOf <http://example.org/Person> ."),
        "rdfs10: every class is subClassOf itself");
}

// ─── rdfs4a/4b: Resource typing (axiomatic) ──────────────────────────────────

#[test]
fn test_rdfs4a_subject_is_resource() {
    let s = store_with("ex:alice ex:knows ex:bob .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/alice> rdf:type rdfs:Resource ."),
        "rdfs4a: every subject should be typed as rdfs:Resource");
}

#[test]
fn test_rdfs4b_iri_object_is_resource() {
    let s = store_with("ex:alice ex:knows ex:bob .");
    materialize(&s);
    assert!(ask_in_tg(&s, "<http://example.org/bob> rdf:type rdfs:Resource ."),
        "rdfs4b: every IRI object should be typed as rdfs:Resource");
}

// ─── Combined interaction tests ───────────────────────────────────────────────

#[test]
fn test_combined_domain_and_subclass() {
    let s = store_with("ex:teaches rdfs:domain ex:Instructor . \
                        ex:Instructor rdfs:subClassOf ex:Person . \
                        ex:alice ex:teaches ex:cs101 .");
    materialize(&s);
    // alice → Instructor (rdfs2) → Person (rdfs9)
    assert!(ask_in_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/Instructor> ."),
        "combined: domain types alice as Instructor");
    assert!(ask_in_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/Person> ."),
        "combined: subClassOf propagates to Person");
}

#[test]
fn test_combined_range_and_subproperty() {
    let s = store_with("ex:fatherOf rdfs:subPropertyOf ex:parentOf . \
                        ex:parentOf rdfs:range ex:Person . \
                        ex:bob ex:fatherOf ex:alice .");
    materialize(&s);
    // fatherOf → parentOf (rdfs7), parentOf range Person → alice:Person (rdfs3)
    assert!(ask_in_tg(&s, "<http://example.org/alice> rdf:type <http://example.org/Person> ."),
        "combined: subPropertyOf + range");
}

#[test]
fn test_idempotent_double_materialization() {
    let s = store_with("ex:Employee rdfs:subClassOf ex:Person . ex:alice rdf:type ex:Employee .");
    materialize(&s);
    let count1 = match s.query(&format!("SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{TG}> {{ ?s ?p ?o }} }}")).unwrap() {
        oxigraph::sparql::QueryResults::Solutions(mut sols) => {
            sols.next().unwrap().unwrap().get("c").and_then(|v| {
                if let oxigraph::model::Term::Literal(l) = v { l.value().parse::<usize>().ok() } else { None }
            }).unwrap_or(0)
        }
        _ => 0,
    };
    // Second materialization should not add more triples
    materialize(&s);
    let count2 = match s.query(&format!("SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{TG}> {{ ?s ?p ?o }} }}")).unwrap() {
        oxigraph::sparql::QueryResults::Solutions(mut sols) => {
            sols.next().unwrap().unwrap().get("c").and_then(|v| {
                if let oxigraph::model::Term::Literal(l) = v { l.value().parse::<usize>().ok() } else { None }
            }).unwrap_or(0)
        }
        _ => 0,
    };
    assert_eq!(count1, count2, "Materialization should be idempotent");
}
