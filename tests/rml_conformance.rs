//! RML (RDF Mapping Language) + R2RML conformance tests.
//!
//! Grounded in the RML spec (https://rml.io/specs/rml/) and R2RML
//! (https://www.w3.org/TR/r2rml/), adversarially fact-checked. The engine
//! implements R2RML vocabulary (`rr:`) + RML source extensions (`rml:`) for
//! CSV / JSONPath / XPath logical sources and registered relational sources,
//! with template / reference / constant term maps, R2RML's term types and
//! defaults (§7.4), IRI-safe template values (§7.3), blank nodes per value and
//! graph (§9.1, §11.2), datatypes, languages, `rr:class`, graph maps with
//! R2RML's union semantics (§9, §11.1), a base IRI, and SQL identifiers.
//!
//! Most tests name their term maps (`ex:Subj rr:template …`); inline blank
//! nodes parse the same way (`rml_inline_blank_node_mapping`).
//!
//! A predicate-object map generates every predicate map × every object map
//! (§6.3, §11.1). A non-conforming mapping is refused at parse with the
//! construct named, a column the source lacks is an error, and a data error
//! (§4.3) aborts the run unless it opts into skipping and reporting. An empty
//! value is a value; RML-IO `rml:null` names the values that count as NULL.
//!
//! Referencing object maps (`rr:parentTriplesMap`) join on every kind of
//! source: CSV, JSON and XML files as well as relational sources, by an index
//! of the parent's rows (R2RML §8, §11.1). One without a join condition joins
//! each row to itself, not to every parent row.
//!
//! A mapping version frozen before the engine followed R2RML's term rules
//! keeps the old ones (`Semantics::Legacy`); `rml_legacy_semantics_*` pin them.

use open_triplestore::rml::checks::OnDataError;
use open_triplestore::rml::model::{RmlMapping, Semantics};
use open_triplestore::rml::sql::{execute_relational_filtered, SqlOutcome};
use open_triplestore::rml::{execute_with, parse_from_store_as, parse_rml};
use open_triplestore::store::TripleStore;
use oxigraph::sparql::QueryResults;
use std::collections::HashMap;

const PFX: &str = "@prefix rr: <http://www.w3.org/ns/r2rml#> .\n\
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .\n\
@prefix ql: <http://semweb.mmlab.be/ns/ql#> .\n\
@prefix ex: <http://example.org/> .\n\
@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n\
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n";

const SPARQL_PFX: &str = "PREFIX rr: <http://www.w3.org/ns/r2rml#> \
PREFIX foaf: <http://xmlns.com/foaf/0.1/> \
PREFIX ex: <http://example.org/> \
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> \
PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> ";

/// Run a file mapping the default way: a data error aborts.
fn execute(
    mapping: &RmlMapping,
    source_data: &HashMap<String, String>,
    store: &TripleStore,
    target_graph: Option<&str>,
) -> Result<usize, String> {
    execute_with(
        mapping,
        source_data,
        store,
        target_graph,
        OnDataError::Abort,
        |_| Ok(()),
    )
    .map(|o| o.triples)
}

fn run_rml(mapping: &str, sources: &[(&str, &str)]) -> (TripleStore, usize) {
    let m = parse_rml(&format!("{PFX}{mapping}")).expect("parse_rml");
    let mut src = HashMap::new();
    for (k, v) in sources {
        src.insert(k.to_string(), v.to_string());
    }
    let store = TripleStore::in_memory().unwrap();
    let n = execute(&m, &src, &store, None).expect("execute");
    (store, n)
}

fn count(store: &TripleStore, q: &str) -> usize {
    match store.query(&format!("{SPARQL_PFX}{q}")).unwrap() {
        QueryResults::Solutions(s) => s.count(),
        _ => panic!("expected solutions"),
    }
}

fn first(store: &TripleStore, q: &str) -> Option<String> {
    match store.query(&format!("{SPARQL_PFX}{q}")).unwrap() {
        QueryResults::Solutions(mut s) => s
            .next()
            .and_then(|r| r.ok())
            .and_then(|r| r.iter().next().map(|(_, t)| t.to_string())),
        _ => None,
    }
}

// CSV: template subject + two NAMED predicate-object maps; one triple-set per row.
#[test]
fn rml_csv_multiple_columns() {
    let mapping = r#"
      ex:PersonMap a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:NamePOM, ex:AgePOM .
      ex:Src rml:source "people.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/person/{id}" .
      ex:NamePOM rr:predicate foaf:name ; rr:objectMap ex:NameObj .
      ex:NameObj rml:reference "name" .
      ex:AgePOM rr:predicate foaf:age ; rr:objectMap ex:AgeObj .
      ex:AgeObj rml:reference "age" ."#;
    let (store, n) = run_rml(
        mapping,
        &[("people.csv", "id,name,age\n1,Alice,30\n2,Bob,25\n")],
    );
    assert_eq!(n, 4, "2 rows x 2 predicate-object maps");
    assert_eq!(store.len().unwrap(), 4);
    assert_eq!(
        first(
            &store,
            "SELECT ?n WHERE { <http://example.org/person/1> foaf:name ?n }"
        )
        .as_deref(),
        Some("\"Alice\"")
    );
    assert_eq!(
        first(
            &store,
            "SELECT ?a WHERE { <http://example.org/person/2> foaf:age ?a }"
        )
        .as_deref(),
        Some("\"25\"")
    );
}

// rr:constant with an IRI value yields an IRI object — the term type is inferred
// from the constant (per R2RML), without an explicit rr:termType.
#[test]
fn rml_constant_iri_object() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:TypePOM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" .
      ex:TypePOM rr:predicate rdf:type ; rr:objectMap ex:TypeObj .
      ex:TypeObj rr:constant foaf:Person ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id\n1\n2\n")]);
    assert_eq!(
        count(&store, "SELECT ?s WHERE { ?s a foaf:Person }"),
        2,
        "rr:constant IRI object"
    );
}

// rr:class on the subjectMap (per R2RML) generates rdf:type triples (COMPLEX-08).
#[test]
fn rml_class_generates_rdf_type() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:NamePOM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" ; rr:class foaf:Person .
      ex:NamePOM rr:predicate foaf:name ; rr:objectMap ex:NameObj .
      ex:NameObj rml:reference "name" ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id,name\n1,Alice\n")]);
    assert_eq!(
        count(&store, "SELECT ?s WHERE { ?s a foaf:Person }"),
        1,
        "rr:class => rdf:type"
    );
}

// rr:datatype and rr:language on object maps produce typed/lang literals.
#[test]
fn rml_datatype_and_language() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:AgePOM, ex:LabelPOM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" .
      ex:AgePOM rr:predicate ex:age ; rr:objectMap ex:AgeObj .
      ex:AgeObj rml:reference "age" ; rr:datatype xsd:integer .
      ex:LabelPOM rr:predicate ex:label ; rr:objectMap ex:LabelObj .
      ex:LabelObj rml:reference "label" ; rr:language "en" ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id,age,label\n1,30,Hello\n")]);
    let age = first(
        &store,
        "SELECT ?a WHERE { <http://example.org/r/1> ex:age ?a }",
    )
    .unwrap_or_default();
    let label = first(
        &store,
        "SELECT ?l WHERE { <http://example.org/r/1> ex:label ?l }",
    )
    .unwrap_or_default();
    assert!(
        age.contains("integer"),
        "rr:datatype xsd:integer, got {age:?}"
    );
    assert!(label.contains("@en"), "rr:language en, got {label:?}");
}

// JSON source via JSONPath iterator + relative references.
#[test]
fn rml_json_source() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:NamePOM .
      ex:Src rml:source "p.json" ; rml:referenceFormulation ql:JSONPath ; rml:iterator "$.people[*]" .
      ex:Subj rr:template "http://example.org/p/{id}" .
      ex:NamePOM rr:predicate foaf:name ; rr:objectMap ex:NameObj .
      ex:NameObj rml:reference "name" ."#;
    let (store, n) = run_rml(
        mapping,
        &[(
            "p.json",
            r#"{"people":[{"id":"1","name":"Ann"},{"id":"2","name":"Bo"}]}"#,
        )],
    );
    assert_eq!(n, 2, "one triple per JSON array element");
    assert_eq!(
        first(
            &store,
            "SELECT ?n WHERE { <http://example.org/p/1> foaf:name ?n }"
        )
        .as_deref(),
        Some("\"Ann\"")
    );
}

// XML source via XPath iterator + relative references.
#[test]
fn rml_xml_source() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:NamePOM .
      ex:Src rml:source "p.xml" ; rml:referenceFormulation ql:XPath ; rml:iterator "/people/person" .
      ex:Subj rr:template "http://example.org/x/{id}" .
      ex:NamePOM rr:predicate foaf:name ; rr:objectMap ex:NameObj .
      ex:NameObj rml:reference "name" ."#;
    let (_store, n) = run_rml(
        mapping,
        &[(
            "p.xml",
            "<people><person><id>1</id><name>Xy</name></person></people>",
        )],
    );
    assert!(n >= 1, "at least one triple from XML, got {n}");
}

// Duplicate rows mapping to the same triple deduplicate to one (RDF set semantics).
#[test]
fn rml_duplicate_rows_dedup() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/same" .
      ex:POM rr:predicate rdf:type ; rr:objectMap ex:Obj .
      ex:Obj rr:constant foaf:Person ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id\n1\n2\n3\n")]);
    assert_eq!(
        store.len().unwrap(),
        1,
        "three identical triples deduplicate to one"
    );
}

// Inline blank-node term maps (the natural RML authoring form) produce the correct
// graph. Regression guard for blank-node dereferencing in the RML parser.
#[test]
fn rml_inline_blank_node_mapping() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource [ rml:source "d.csv" ; rml:referenceFormulation ql:CSV ] ;
        rr:subjectMap [ rr:template "http://example.org/person/{id}" ] ;
        rr:predicateObjectMap [ rr:predicate foaf:name ; rr:objectMap [ rml:reference "name" ] ] ;
        rr:predicateObjectMap [ rr:predicate foaf:age ; rr:objectMap [ rml:reference "age" ] ] ."#;
    let (store, n) = run_rml(mapping, &[("d.csv", "id,name,age\n1,Alice,30\n2,Bob,25\n")]);
    assert_eq!(n, 4, "2 rows x 2 predicate-object maps");
    assert_eq!(store.len().unwrap(), 4);
    assert_eq!(
        first(
            &store,
            "SELECT ?n WHERE { <http://example.org/person/1> foaf:name ?n }"
        )
        .as_deref(),
        Some("\"Alice\"")
    );
    assert_eq!(
        first(
            &store,
            "SELECT ?a WHERE { <http://example.org/person/2> foaf:age ?a }"
        )
        .as_deref(),
        Some("\"25\"")
    );
}

// Multi-variable subject template: the IRI is assembled from two columns.
#[test]
fn rml_multivariable_template_subject() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/p/{first}-{last}" .
      ex:POM rr:predicate foaf:name ; rr:objectMap ex:Obj .
      ex:Obj rml:reference "first" ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "first,last\nAda,Lovelace\n")]);
    assert_eq!(
        count(
            &store,
            "SELECT ?n WHERE { <http://example.org/p/Ada-Lovelace> foaf:name ?n }"
        ),
        1,
        "subject IRI must combine both template variables"
    );
}

// rr:termType rr:IRI on an object map turns a column VALUE into an IRI object.
#[test]
fn rml_object_term_type_iri() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" .
      ex:POM rr:predicate foaf:homepage ; rr:objectMap ex:Obj .
      ex:Obj rml:reference "url" ; rr:termType rr:IRI ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id,url\n1,http://ada.example\n")]);
    let hp = first(
        &store,
        "SELECT ?h WHERE { <http://example.org/r/1> foaf:homepage ?h }",
    )
    .unwrap_or_default();
    assert!(
        hp.starts_with('<') && hp.contains("ada.example"),
        "object must be an IRI, got {hp:?}"
    );
}

// R2RML §7.4: an object map with no rr:termType is a literal only when it reads
// a column or declares a language or datatype — a template object is an IRI.
#[test]
fn rml_object_template_defaults_to_an_iri() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" .
      ex:POM rr:predicate ex:dept ; rr:objectMap ex:Obj .
      ex:Obj rr:template "http://example.org/dept/{dept}" ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id,dept\n1,Sales\n")]);
    assert_eq!(
        first(
            &store,
            "SELECT ?d WHERE { <http://example.org/r/1> ex:dept ?d }"
        )
        .as_deref(),
        Some("<http://example.org/dept/Sales>")
    );
}

// …and a template object becomes a literal by saying so, or by declaring a
// language or a datatype; its value is not percent-encoded (§7.3).
#[test]
fn rml_object_template_literal() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM, ex:LangPOM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" .
      ex:POM rr:predicate ex:greeting ; rr:objectMap ex:Obj .
      ex:Obj rr:template "Hello {name}!" ; rr:termType rr:Literal .
      ex:LangPOM rr:predicate ex:groet ; rr:objectMap ex:LangObj .
      ex:LangObj rr:template "Hallo {name}" ; rr:language "nl" ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id,name\n1,Sam Lee\n")]);
    assert_eq!(
        first(
            &store,
            "SELECT ?g WHERE { <http://example.org/r/1> ex:greeting ?g }"
        )
        .as_deref(),
        Some("\"Hello Sam Lee!\"")
    );
    assert_eq!(
        first(
            &store,
            "SELECT ?g WHERE { <http://example.org/r/1> ex:groet ?g }"
        )
        .as_deref(),
        Some("\"Hallo Sam Lee\"@nl")
    );
}

// rr:graphMap on the subject map (R2RML §9) routes every triple of the subject
// — rr:class included — into a named graph.
#[test]
fn rml_graph_map_routes_to_named_graph() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" ; rr:graphMap ex:GM .
      ex:POM rr:predicate foaf:name ; rr:objectMap ex:Obj .
      ex:Obj rml:reference "name" .
      ex:GM rr:constant ex:G1 ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id,name\n1,Al\n2,Bo\n")]);
    assert_eq!(
        count(
            &store,
            "SELECT ?s WHERE { GRAPH ex:G1 { ?s foaf:name ?n } }"
        ),
        2,
        "triples must land in the rr:graphMap-named graph"
    );
    assert_eq!(
        count(&store, "SELECT ?s WHERE { ?s foaf:name ?n }"),
        0,
        "and NOT in the default graph"
    );
}

// Two independent triples maps in one document each contribute their triples.
#[test]
fn rml_multiple_triples_maps() {
    let mapping = r#"
      ex:M1 a rr:TriplesMap ;
        rml:logicalSource ex:S1 ; rr:subjectMap ex:Su1 ; rr:predicateObjectMap ex:P1 .
      ex:S1 rml:source "a.csv" ; rml:referenceFormulation ql:CSV .
      ex:Su1 rr:template "http://example.org/a/{id}" .
      ex:P1 rr:predicate ex:p1 ; rr:objectMap ex:O1 . ex:O1 rml:reference "v" .
      ex:M2 a rr:TriplesMap ;
        rml:logicalSource ex:S2 ; rr:subjectMap ex:Su2 ; rr:predicateObjectMap ex:P2 .
      ex:S2 rml:source "b.csv" ; rml:referenceFormulation ql:CSV .
      ex:Su2 rr:template "http://example.org/b/{id}" .
      ex:P2 rr:predicate ex:p2 ; rr:objectMap ex:O2 . ex:O2 rml:reference "v" ."#;
    let (store, n) = run_rml(
        mapping,
        &[("a.csv", "id,v\n1,X\n"), ("b.csv", "id,v\n1,Y\n")],
    );
    assert_eq!(n, 2, "one triple from each of two triples maps");
    assert_eq!(
        first(
            &store,
            "SELECT ?v WHERE { <http://example.org/a/1> ex:p1 ?v }"
        )
        .as_deref(),
        Some("\"X\"")
    );
    assert_eq!(
        first(
            &store,
            "SELECT ?v WHERE { <http://example.org/b/1> ex:p2 ?v }"
        )
        .as_deref(),
        Some("\"Y\"")
    );
}

// A blank-node subject (rr:termType rr:BlankNode) is shared across the row's
// predicate-object maps, so both properties hang off the same blank node.
#[test]
fn rml_blank_node_subject_shared_across_poms() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:P1, ex:P2 .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "node{id}" ; rr:termType rr:BlankNode .
      ex:P1 rr:predicate foaf:name ; rr:objectMap ex:O1 . ex:O1 rml:reference "name" .
      ex:P2 rr:predicate foaf:age ; rr:objectMap ex:O2 . ex:O2 rml:reference "age" ."#;
    let (store, n) = run_rml(mapping, &[("d.csv", "id,name,age\n1,Al,30\n")]);
    assert_eq!(n, 2, "two POMs over one row");
    assert_eq!(
        count(
            &store,
            "SELECT ?s WHERE { ?s foaf:name \"Al\" ; foaf:age \"30\" . FILTER(isBlank(?s)) }"
        ),
        1,
        "both properties must share one blank-node subject"
    );
}

// Referencing object maps (`rr:parentTriplesMap` + `rr:joinCondition`) on
// FILE sources: the parent's rows are indexed by the parent side of the join,
// and each child row links to the subject of every parent row whose key
// equals its own (R2RML §8, §11.1). The file executor used to drop the link
// and report success, and then refused the mapping.
const CSV_JOIN: &str = r#"
  ex:Child a rr:TriplesMap ;
    rml:logicalSource ex:CSrc ; rr:subjectMap ex:CSubj ;
    rr:predicateObjectMap ex:ParentPOM, ex:OwnPOM .
  ex:CSrc rml:source "c.csv" ; rml:referenceFormulation ql:CSV .
  ex:CSubj rr:template "http://example.org/c/{id}" .
  ex:ParentPOM rr:predicate ex:parent ; rr:objectMap ex:ParentObj .
  ex:ParentObj rr:parentTriplesMap ex:Parent ;
    rr:joinCondition ex:Join .
  ex:Join rr:child "pid" ; rr:parent "id" .
  ex:OwnPOM rr:predicate ex:own ; rr:objectMap ex:OwnObj .
  ex:OwnObj rml:reference "pid" .
  ex:Parent a rr:TriplesMap ;
    rml:logicalSource ex:PSrc ; rr:subjectMap ex:PSubj ;
    rr:predicateObjectMap ex:NamePOM .
  ex:PSrc rml:source "p.csv" ; rml:referenceFormulation ql:CSV .
  ex:PSubj rr:template "http://example.org/p/{id}" .
  ex:NamePOM rr:predicate foaf:name ; rr:objectMap ex:NameObj .
  ex:NameObj rml:reference "name" ."#;

#[test]
fn rml_referencing_object_map_joins_csv_sources() {
    let (store, n) = run_rml(
        CSV_JOIN,
        &[
            ("c.csv", "id,pid\n1,10\n2,10\n3,11\n4,99\n"),
            ("p.csv", "id,name\n10,Pat\n11,Sam\n"),
        ],
    );
    assert_eq!(
        count(
            &store,
            "SELECT ?c WHERE { ?c ex:parent <http://example.org/p/10> }"
        ),
        2,
        "two children of one parent both link to it"
    );
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.org/c/3> ex:parent <http://example.org/p/11> }"
        ),
        1
    );
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.org/c/4> ex:parent ?p }"
        ),
        0,
        "a key no parent row has joins to nothing"
    );
    // 4 own + 3 links + 2 names.
    assert_eq!(n, 9);
}

#[test]
fn rml_referencing_object_map_joins_json_sources() {
    let mapping = r#"
      ex:Student a rr:TriplesMap ;
        rml:logicalSource [ rml:source "students.json" ; rml:referenceFormulation ql:JSONPath ;
                            rml:iterator "$.students[*]" ] ;
        rr:subjectMap [ rr:template "http://example.org/student/{ID}" ] ;
        rr:predicateObjectMap [ rr:predicate ex:practises ; rr:objectMap [
            rr:parentTriplesMap ex:Sport ;
            rr:joinCondition [ rr:child "Sport" ; rr:parent "ID" ] ] ] .
      ex:Sport a rr:TriplesMap ;
        rml:logicalSource [ rml:source "sports.json" ; rml:referenceFormulation ql:JSONPath ;
                            rml:iterator "$.sports[*]" ] ;
        rr:subjectMap [ rr:template "http://example.org/sport/{ID}" ] ."#;
    let (store, n) = run_rml(
        mapping,
        &[
            (
                "students.json",
                r#"{"students":[{"ID":10,"Sport":100},{"ID":20},{"ID":30,"Sport":null}]}"#,
            ),
            ("sports.json", r#"{"sports":[{"ID":100},{"ID":200}]}"#),
        ],
    );
    assert_eq!(n, 1, "only student 10 names a sport");
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.org/student/10> ex:practises \
             <http://example.org/sport/100> }"
        ),
        1,
        "a JSON number joins by its value"
    );
}

#[test]
fn rml_referencing_object_map_joins_xml_sources() {
    let mapping = r#"
      ex:Student a rr:TriplesMap ;
        rml:logicalSource [ rml:source "students.xml" ; rml:referenceFormulation ql:XPath ;
                            rml:iterator "/students/student" ] ;
        rr:subjectMap [ rr:template "http://example.org/student/{ID}" ] ;
        rr:predicateObjectMap [ rr:predicate ex:practises ; rr:objectMap [
            rr:parentTriplesMap ex:Sport ;
            rr:joinCondition [ rr:child "Sport" ; rr:parent "ID" ] ] ] .
      ex:Sport a rr:TriplesMap ;
        rml:logicalSource [ rml:source "sports.xml" ; rml:referenceFormulation ql:XPath ;
                            rml:iterator "/sports/sport" ] ;
        rr:subjectMap [ rr:template "http://example.org/sport/{ID}" ] ."#;
    let (store, n) = run_rml(
        mapping,
        &[
            (
                "students.xml",
                "<students><student><ID>10</ID><Sport>100</Sport></student>\
                 <student><ID>20</ID></student></students>",
            ),
            (
                "sports.xml",
                "<sports><sport><ID>100</ID></sport><sport><ID>200</ID></sport></sports>",
            ),
        ],
    );
    assert_eq!(n, 1);
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.org/student/10> ex:practises \
             <http://example.org/sport/100> }"
        ),
        1
    );
}

#[test]
fn rml_composite_key_join_needs_every_condition() {
    // Two join conditions: both must hold (R2RML §8).
    let mapping = r#"
      ex:Enrolment a rr:TriplesMap ;
        rml:logicalSource [ rml:source "e.csv" ; rml:referenceFormulation ql:CSV ] ;
        rr:subjectMap [ rr:template "http://example.org/e/{id}" ] ;
        rr:predicateObjectMap [ rr:predicate ex:course ; rr:objectMap [
            rr:parentTriplesMap ex:Course ;
            rr:joinCondition [ rr:child "dept" ; rr:parent "dept" ] ;
            rr:joinCondition [ rr:child "code" ; rr:parent "code" ] ] ] .
      ex:Course a rr:TriplesMap ;
        rml:logicalSource [ rml:source "c.csv" ; rml:referenceFormulation ql:CSV ] ;
        rr:subjectMap [ rr:template "http://example.org/course/{dept}/{code}" ] ."#;
    let (store, n) = run_rml(
        mapping,
        &[
            ("e.csv", "id,dept,code\n1,CS,101\n2,CS,102\n3,MA,101\n"),
            ("c.csv", "dept,code\nCS,101\nMA,101\nMA,102\n"),
        ],
    );
    assert_eq!(n, 2, "enrolment 2 names CS 102, which no course is");
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.org/e/3> ex:course <http://example.org/course/MA/101> }"
        ),
        1
    );
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.org/e/1> ex:course ?c }"
        ),
        1,
        "matching one condition of two is no match"
    );
}

#[test]
fn rml_a_null_join_key_matches_nothing_and_an_empty_one_is_a_value() {
    // A key named by rml:null is NULL on either side, and NULL never equals
    // NULL (R2RML §8 joins by SQL equality). Without rml:null an empty CSV
    // cell is the value "" (RML-IO), which an empty parent key equals.
    let mapping = r#"
      ex:Child a rr:TriplesMap ;
        rml:logicalSource [ rml:source "c.csv" ; rml:referenceFormulation ql:CSV ; rml:null "NULL" ] ;
        rr:subjectMap [ rr:template "http://example.org/c/{id}" ] ;
        rr:predicateObjectMap [ rr:predicate ex:parent ; rr:objectMap [
            rr:parentTriplesMap ex:Parent ;
            rr:joinCondition [ rr:child "pid" ; rr:parent "id" ] ] ] .
      ex:Parent a rr:TriplesMap ;
        rml:logicalSource [ rml:source "p.csv" ; rml:referenceFormulation ql:CSV ; rml:null "NULL" ] ;
        rr:subjectMap [ rr:template "http://example.org/p/{name}" ] ."#;
    let (store, _) = run_rml(
        mapping,
        &[
            ("c.csv", "id,pid\n1,NULL\n2,\n3,7\n"),
            ("p.csv", "id,name\nNULL,nobody\n,empty\n7,seven\n"),
        ],
    );
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.org/c/1> ex:parent ?p }"
        ),
        0,
        "NULL joins to nothing, not to the parent whose key is NULL too"
    );
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.org/c/2> ex:parent <http://example.org/p/empty> }"
        ),
        1
    );
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.org/c/3> ex:parent <http://example.org/p/seven> }"
        ),
        1
    );
}

#[test]
fn rml_join_less_reference_is_the_same_row_not_a_cross_join() {
    // R2RML §8: with no join condition, the joint query is the child query
    // itself — every row links to the parent subject of the same row.
    let mapping = r#"
      ex:Person a rr:TriplesMap ;
        rml:logicalSource ex:Src ;
        rr:subjectMap [ rr:template "http://example.org/person/{id}" ] ;
        rr:predicateObjectMap [ rr:predicate ex:lives ;
            rr:objectMap [ rr:parentTriplesMap ex:City ] ] .
      ex:City a rr:TriplesMap ;
        rml:logicalSource ex:Src ;
        rr:subjectMap [ rr:template "http://example.org/city/{city}" ] .
      ex:Src rml:source "p.csv" ; rml:referenceFormulation ql:CSV ."#;
    let (store, n) = run_rml(mapping, &[("p.csv", "id,city\n1,Ghent\n2,Delft\n")]);
    assert_eq!(n, 2, "one link per row, not rows × rows");
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.org/person/2> ex:lives <http://example.org/city/Delft> }"
        ),
        1
    );
}

// A relational logical source is executed through a connection, so the
// file-based entry point refuses it rather than silently producing nothing.
#[test]
fn rml_relational_source_is_refused_by_the_file_executor() {
    let m = parse_rml(&format!(
        "{PFX}
         ex:M a rr:TriplesMap ;
           rml:logicalSource [ rml:source <urn:source:legacy> ; rr:tableName \"products\" ] ;
           rr:subjectMap [ rr:template \"http://example.org/p/{{id}}\" ] ;
           rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column \"name\" ] ] ."
    ))
    .expect("a relational mapping parses");
    let store = TripleStore::in_memory().unwrap();
    let err = execute(&m, &HashMap::new(), &store, None).unwrap_err();
    assert!(
        err.contains("/api/sources/"),
        "the error names the path that can run it: {err}"
    );
    assert_eq!(store.len().unwrap(), 0, "nothing was written");
}

// A quoted CSV field containing a newline must produce a correctly escaped
// literal, not break the whole mapping.
//
// Literals were serialised by hand-escaping only `\` and `"`, leaving raw
// newlines, carriage returns and tabs in the generated Turtle. Turtle's
// STRING_LITERAL_QUOTE forbids those, so ONE multi-line source value made the
// entire document unparseable and `execute` failed with "Failed to load
// generated triples" — not a skipped row, the whole batch.
#[test]
fn rml_literal_with_newline_does_not_break_the_mapping() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM .
      ex:Src rml:source "notes.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/note/{id}" .
      ex:POM rr:predicate ex:body ; rr:objectMap ex:Obj .
      ex:Obj rml:reference "body" ."#;
    // Row 2's body spans two lines inside quotes, and row 3 carries a tab.
    let csv = "id,body\n1,plain\n2,\"first line\nsecond line\"\n3,\"has\ttab\"\n";
    let (store, n) = run_rml(mapping, &[("notes.csv", csv)]);

    assert_eq!(
        n, 3,
        "every row must be mapped, not just the ones without control characters"
    );
    assert_eq!(
        first(
            &store,
            "SELECT ?b WHERE { <http://example.org/note/2> ex:body ?b }"
        )
        .as_deref(),
        Some("\"first line\\nsecond line\""),
        "the newline must be escaped, and the value preserved"
    );
    assert_eq!(
        first(
            &store,
            "SELECT ?b WHERE { <http://example.org/note/1> ex:body ?b }"
        )
        .as_deref(),
        Some("\"plain\""),
        "neighbouring rows must be unaffected"
    );
}

// A reference-derived IRI whose value is not a valid IRI is a data error
// (R2RML §4.3, §11.2): the run aborts, writes nothing, and names the row.
//
// `rr:termType rr:IRI` on an `rml:reference` once interpolated the raw cell
// value between angle brackets, so a value with a space produced an
// unparseable document and one containing `>` could close the IRI and inject
// further triples. Then it skipped the term silently. Now it is reported.
#[test]
fn rml_invalid_iri_is_a_data_error_that_aborts_and_names_the_row() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM .
      ex:Src rml:source "links.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/link/{id}" .
      ex:POM rr:predicate ex:target ; rr:objectMap ex:Obj .
      ex:Obj rml:reference "target" ; rr:termType rr:IRI ."#;
    let csv = "id,target\n1,http://example.org/ok\n2,not a valid iri\n3,http://example.org/ok2\n";
    let m = parse_rml(&format!("{PFX}{mapping}")).unwrap();
    let src = HashMap::from([("links.csv".to_string(), csv.to_string())]);

    let store = TripleStore::in_memory().unwrap();
    let err = execute(&m, &src, &store, None).unwrap_err();
    assert!(err.contains("data error"), "{err}");
    assert!(err.contains("row 2"), "the offending row is named: {err}");
    assert!(err.contains("\"not a valid iri\""), "and its value: {err}");
    assert!(err.contains("column \"target\""), "and the term map: {err}");
    assert!(err.contains("on_data_error=skip"), "and the way out: {err}");
    assert_eq!(store.len().unwrap(), 0, "an aborted run writes nothing");

    // The opt-in lenient mode leaves the term out and reports the row.
    let store = TripleStore::in_memory().unwrap();
    let outcome = execute_with(&m, &src, &store, None, OnDataError::Skip, |_| Ok(())).unwrap();
    assert_eq!(outcome.triples, 2, "the valid rows are mapped");
    assert_eq!(outcome.data_errors.rows, 1);
    assert!(
        outcome.data_errors.first[0].contains("row 2"),
        "{:?}",
        outcome.data_errors
    );
    assert_eq!(
        count(
            &store,
            "SELECT ?t WHERE { <http://example.org/link/2> ex:target ?t }"
        ),
        0,
        "the unrepresentable IRI is skipped"
    );
}

// R2RML §10.3: a literal whose rr:datatype overrides the natural one and whose
// value is outside that datatype's lexical space is ill-typed — a data error.
#[test]
fn r2rml_ill_typed_datatype_override_is_a_data_error() {
    let ddl = "CREATE TABLE m (id INTEGER PRIMARY KEY, v TEXT);
               INSERT INTO m VALUES (1, '42'), (2, 'forty-two'), (3, '7');";
    let rml = format!(
        "{PFX}
         ex:M a rr:TriplesMap ;
           rr:logicalTable [ rr:tableName \"m\" ] ;
           rr:subjectMap [ rr:template \"http://example.org/m/{{id}}\" ] ;
           rr:predicateObjectMap [ rr:predicate ex:v ;
             rr:objectMap [ rr:column \"v\" ; rr:datatype xsd:integer ] ] ."
    );
    let err = relational(ddl, &rml, OnDataError::Abort)
        .err()
        .expect("the run fails");
    assert!(
        err.contains("row 2") && err.contains("\"forty-two\""),
        "{err}"
    );
    assert!(err.contains("integer"), "{err}");

    let (store, outcome) = relational(ddl, &rml, OnDataError::Skip).unwrap();
    assert_eq!(outcome.data_errors.rows, 1);
    assert_eq!(
        count(
            &store,
            "SELECT ?v WHERE { GRAPH <urn:run:test> { ?s ex:v ?v } }"
        ),
        2
    );
}

// ── Mapping validation (R2RML: a non-conforming mapping is refused) ──

#[test]
fn r2rml_non_conforming_mappings_are_refused_naming_the_construct() {
    let ls = "rml:logicalSource [ rml:source \"d.csv\" ; rml:referenceFormulation ql:CSV ]";
    let sm = "rr:subjectMap [ rr:template \"http://example.org/r/{id}\" ]";
    let om = |body: &str| {
        format!(
            "{ls} ; {sm} ; rr:predicateObjectMap [ rr:predicate ex:p ; rr:objectMap [ {body} ] ]"
        )
    };
    let cases: Vec<(String, &str)> = vec![
        (
            format!("{ls} ; {sm} ; rr:subject ex:Other"),
            "2 subject maps",
        ),
        (
            format!("{ls} ; rr:logicalTable [ rr:tableName \"t\" ] ; {sm}"),
            "2 logical sources",
        ),
        (om("rr:column \"a\" ; rr:template \"{b}\""), "both rr:template and rr:column"),
        (om("rr:column \"a\", \"b\""), "2 values of rr:column"),
        (
            om("rr:column \"a\" ; rr:language \"en\" ; rr:datatype xsd:string"),
            "both rr:language and rr:datatype",
        ),
        (om("rr:column \"a\" ; rr:language \"not a tag\""), "not a valid BCP 47"),
        (om("rr:column \"a\" ; rr:datatype \"xsd:int\""), "rr:datatype"),
        (om("rr:column \"a\" ; rr:termType rr:Thing"), "rr:termType"),
        (
            om("rr:column \"a\" ; rr:termType rr:IRI ; rr:language \"en\""),
            "only a literal takes a language tag",
        ),
        (
            om("rr:parentTriplesMap ex:M ; rr:column \"a\""),
            "both rr:parentTriplesMap and rr:column",
        ),
        (
            om("rr:parentTriplesMap ex:M ; rr:joinCondition [ rr:child \"a\", \"b\" ; rr:parent \"a\" ]"),
            "2 values of rr:child",
        ),
        (
            format!("{ls} ; rr:subjectMap [ rr:column \"id\" ; rr:termType rr:Literal ]"),
            "the subject map generates rr:Literal",
        ),
        (
            format!("{ls} ; {sm} ; rr:predicateObjectMap [ rr:predicateMap [ rr:column \"p\" ; rr:termType rr:BlankNode ] ; rr:object ex:o ]"),
            "the predicate map generates rr:BlankNode",
        ),
        (
            format!("{ls} ; rr:subjectMap [ rr:template \"http://example.org/r/{{id}}\" ; rr:graphMap [ rr:template \"g{{id}}\" ; rr:termType rr:BlankNode ] ]"),
            "the graph map generates rr:BlankNode",
        ),
        (
            format!("{ls} ; rr:subjectMap [ rr:template \"http://example.org/r/{{id}}\" ; rr:class \"Person\" ]"),
            "rr:class \"Person\" is not an IRI",
        ),
        (
            "rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"t\" ; rr:sqlQuery \"SELECT 1\" ] ; rr:subjectMap [ rr:template \"http://example.org/r/{id}\" ]".to_string(),
            "both rr:tableName and a query",
        ),
    ];
    for (body, construct) in cases {
        let doc = format!("{PFX}ex:M a rr:TriplesMap ; {body} .");
        let err = parse_rml(&doc).expect_err(construct);
        assert!(
            err.contains(construct),
            "expected '{construct}' in: {err}\n---\n{doc}"
        );
        assert!(
            err.contains("<http://example.org/M>"),
            "the triples map is named: {err}"
        );
    }
    // rr:sqlVersion and rr:inverseExpression are R2RML and are accepted.
    parse_rml(&format!(
        "{PFX}ex:M a rr:TriplesMap ;
           rr:logicalTable [ rr:sqlQuery \"SELECT 1 AS id\" ; rr:sqlVersion rr:SQL2008 ] ;
           rr:subjectMap [ rr:template \"http://example.org/r/{{id}}\" ;
                           rr:inverseExpression \"{{id}}\" ] ."
    ))
    .expect("rr:sqlVersion and rr:inverseExpression are accepted");
}

// R2RML §6: every column a triples map names must exist in its logical table.
// A typo used to produce no triples at all and report success.
#[test]
fn rml_a_column_the_csv_header_lacks_is_an_error() {
    let m = parse_rml(&format!(
        "{PFX}ex:M a rr:TriplesMap ;
           rml:logicalSource [ rml:source \"d.csv\" ; rml:referenceFormulation ql:CSV ] ;
           rr:subjectMap [ rr:template \"http://example.org/r/{{id}}\" ] ;
           rr:predicateObjectMap [ rr:predicate ex:n ; rr:objectMap [ rml:reference \"nmae\" ] ] ."
    ))
    .unwrap();
    let src = HashMap::from([("d.csv".to_string(), "id,name\n1,Al\n".to_string())]);
    let err = execute(&m, &src, &TripleStore::in_memory().unwrap(), None).unwrap_err();
    assert!(err.contains("\"nmae\" (in an object map)"), "{err}");
    assert!(
        err.contains("\"id\", \"name\""),
        "the columns it has: {err}"
    );
}

#[test]
fn r2rml_a_column_the_sql_source_lacks_is_an_error() {
    let ddl = "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, alt TEXT);
               INSERT INTO t VALUES (1, NULL, NULL);";
    for ls in [
        "rr:tableName \"t\"",
        "rr:sqlQuery \"SELECT id, name FROM t\"",
    ] {
        let rml = format!(
            "{PFX}ex:M a rr:TriplesMap ;
               rr:logicalTable [ {ls} ] ;
               rr:subjectMap [ rr:template \"http://example.org/r/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:n ; rr:objectMap [ rr:column \"nmae\" ] ] ."
        );
        let err = relational(ddl, &rml, OnDataError::Abort)
            .err()
            .expect("the run fails");
        assert!(err.contains("\"nmae\""), "{ls}: {err}");
    }
    // A column that exists but is NULL in every row is not an error: it
    // generates no term.
    let rml = format!(
        "{PFX}ex:M a rr:TriplesMap ;
           rr:logicalTable [ rr:tableName \"t\" ] ;
           rr:subjectMap [ rr:template \"http://example.org/r/{{id}}\" ] ;
           rr:predicateObjectMap [ rr:predicate ex:n ; rr:objectMap [ rr:column \"name\" ] ] ."
    );
    relational(ddl, &rml, OnDataError::Abort).expect("an all-NULL column is a column");
    // R2RML §5.2: a query's result must not repeat a column name.
    let rml = format!(
        "{PFX}ex:M a rr:TriplesMap ;
           rr:logicalTable [ rr:sqlQuery \"SELECT id, name AS v, alt AS v FROM t\" ] ;
           rr:subjectMap [ rr:template \"http://example.org/r/{{id}}\" ] ."
    );
    let err = relational(ddl, &rml, OnDataError::Abort)
        .err()
        .expect("the run fails");
    assert!(err.contains("two columns named \"v\""), "{err}");
}

// ── Empty values and rml:null (RML-IO) ──

// RML-IO: CSV has no NULL, so an empty cell is a value — an empty literal —
// unless the source lists it under rml:null. A legacy mapping version keeps
// generating no term from an empty value.
#[test]
fn rml_empty_csv_cell_is_a_value_unless_rml_null_says_otherwise() {
    let mapping = |nulls: &str| {
        format!(
            "{PFX}ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source \"d.csv\" ; rml:referenceFormulation ql:CSV {nulls} ] ;
               rr:subjectMap [ rr:template \"http://example.org/r/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:nickname ; rr:objectMap [ rml:reference \"nickname\" ] ] ."
        )
    };
    let src = HashMap::from([(
        "d.csv".to_string(),
        "id,nickname\n1,Ace\n2,\n3,n/a\n".to_string(),
    )]);
    let nicknames = |m: &RmlMapping| -> Vec<String> {
        let store = TripleStore::in_memory().unwrap();
        execute(m, &src, &store, None).unwrap();
        let QueryResults::Solutions(sols) = store
            .query(&format!(
                "{SPARQL_PFX} SELECT ?s ?o WHERE {{ ?s ex:nickname ?o }} ORDER BY ?s"
            ))
            .unwrap()
        else {
            panic!()
        };
        sols.map(|s| s.unwrap().get("o").unwrap().to_string())
            .collect()
    };

    let plain = parse_rml(&mapping("")).unwrap();
    assert_eq!(nicknames(&plain), vec!["\"Ace\"", "\"\"", "\"n/a\""]);

    for ns in ["<http://w3id.org/rml/null>", "rml:null"] {
        let nulled = parse_rml(&mapping(&format!("; {ns} \"\", \"n/a\""))).unwrap();
        assert_eq!(nicknames(&nulled), vec!["\"Ace\""], "{ns}");
    }

    let doc = TripleStore::in_memory().unwrap();
    doc.load_str(&mapping(""), oxigraph::io::RdfFormat::Turtle, None)
        .unwrap();
    let legacy = parse_from_store_as(&doc, None, Semantics::Legacy).unwrap();
    assert_eq!(
        nicknames(&legacy),
        vec!["\"Ace\"", "\"n/a\""],
        "legacy: an empty value generates no term"
    );
}

// R2RML: only a SQL NULL generates no term; '' is a string like any other.
// rml:null adds values that count as NULL.
#[test]
fn r2rml_sql_empty_string_is_a_value_and_only_null_is_not() {
    let ddl = "CREATE TABLE t (id INTEGER PRIMARY KEY, nick TEXT);
               INSERT INTO t VALUES (1, 'Ace'), (2, ''), (3, NULL);";
    let rml = |nulls: &str| {
        format!(
            "{PFX}ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"t\" {nulls} ] ;
               rr:subjectMap [ rr:template \"http://example.org/r/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:nick ; rr:objectMap [ rr:column \"nick\" ] ] ."
        )
    };
    let nicks = |store: &TripleStore| {
        count(
            store,
            "SELECT ?o WHERE { GRAPH <urn:run:test> { ?s ex:nick ?o } }",
        )
    };
    assert_eq!(nicks(&run_relational(ddl, &rml(""))), 2, "'Ace' and ''");
    assert_eq!(
        nicks(&run_relational(
            ddl,
            &rml("; <http://w3id.org/rml/null> \"\"")
        )),
        1,
        "rml:null \"\" makes '' a NULL"
    );
}

// RML-IO: in JSON only `null` is NULL; in XML nothing is, so an empty element
// is the empty string.
#[test]
fn rml_json_null_is_null_and_empty_xml_text_is_a_value() {
    let json = r#"
      ex:J a rr:TriplesMap ;
        rml:logicalSource [ rml:source "d.json" ; rml:referenceFormulation ql:JSONPath ; rml:iterator "$.items[*]" ] ;
        rr:subjectMap [ rr:template "http://example.org/j/{id}" ] ;
        rr:predicateObjectMap [ rr:predicate ex:v ; rr:objectMap [ rml:reference "v" ] ] ."#;
    let (store, _) = run_rml(
        json,
        &[(
            "d.json",
            r#"{"items": [{"id": "1", "v": ""}, {"id": "2", "v": null}, {"id": "3"}]}"#,
        )],
    );
    assert_eq!(
        count(&store, "SELECT ?s WHERE { ?s ex:v \"\" }"),
        1,
        "only the empty string, not null or a missing key"
    );

    let xml = r#"
      ex:X a rr:TriplesMap ;
        rml:logicalSource [ rml:source "d.xml" ; rml:referenceFormulation ql:XPath ; rml:iterator "/people/person" ] ;
        rr:subjectMap [ rr:template "http://example.org/x/{id}" ] ;
        rr:predicateObjectMap [ rr:predicate ex:nick ; rr:objectMap [ rml:reference "nick" ] ] ."#;
    let (store, _) = run_rml(
        xml,
        &[(
            "d.xml",
            "<people><person><id>1</id><nick></nick></person>\
             <person><id>2</id><nick/></person><person><id>3</id></person></people>",
        )],
    );
    assert_eq!(
        count(&store, "SELECT ?s WHERE { ?s ex:nick \"\" }"),
        2,
        "<nick></nick> and <nick/> are empty strings; a missing element is no value"
    );
}

// ── Relational runs (the path a registered mapping version takes) ──

/// Run `rml` against a fresh SQLite database built from `ddl`, into
/// `urn:run:test`.
fn run_relational(ddl: &str, rml: &str) -> TripleStore {
    relational(ddl, rml, OnDataError::Abort)
        .unwrap_or_else(|e| panic!("run fails: {e}"))
        .0
}

/// [`run_relational`], with a data-error policy and the run's result.
fn relational(
    ddl: &str,
    rml: &str,
    on_data_error: OnDataError,
) -> Result<(TripleStore, SqlOutcome), String> {
    use ots_plugin_api::sources::{ConnectParams, SourceConnector};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(ddl)
        .unwrap();
    let connector = open_triplestore::sources::sqlite::SqliteConnector;
    let mut conn = connector
        .connect(&ConnectParams {
            dialect: "sqlite".into(),
            host: None,
            port: None,
            database: path.to_string_lossy().into_owned(),
            username: None,
            password: None,
            read_only: true,
            statement_timeout_ms: 5_000,
            tls: false,
            options: Default::default(),
        })
        .unwrap();
    let mapping = parse_rml(rml).unwrap_or_else(|e| panic!("{e}\n---\n{rml}"));
    let store = TripleStore::in_memory().unwrap();
    let quote = |i: &str| connector.quote_identifier(i);
    let outcome = execute_relational_filtered(
        &mapping,
        conn.as_mut(),
        &quote,
        &store,
        "urn:run:test",
        100,
        "run-1",
        None,
        on_data_error,
    )?;
    Ok((store, outcome))
}

// YARRRML `[ex:category, ex:Hardware]` names a term, and the translator emits
// it as `rr:object <…Hardware>`. R2RML §7.4: the type of a constant's term is
// the type of the constant itself, so the object is an IRI — not the string
// "http://example.org/Hardware".
#[test]
fn yarrrml_constant_iri_object_is_an_iri_when_run() {
    let rml = open_triplestore::sources::yarrrml::to_rml(
        r#"
prefixes: {ex: "http://example.org/"}
mappings:
  product:
    table: product
    s: ex:p$(pid)
    po:
      - [ex:category, ex:Hardware]
      - [ex:note, "just text"]
"#,
        Some("s"),
    )
    .expect("translates");
    let store = run_relational(
        "CREATE TABLE product (pid INTEGER PRIMARY KEY); INSERT INTO product VALUES (1);",
        &rml,
    );
    assert_eq!(
        first(
            &store,
            "SELECT ?o WHERE { GRAPH <urn:run:test> { <http://example.org/p1> ex:category ?o } }"
        )
        .as_deref(),
        Some("<http://example.org/Hardware>"),
        "a constant IRI object is an IRI\n---\n{rml}"
    );
    assert_eq!(
        first(
            &store,
            "SELECT ?o WHERE { GRAPH <urn:run:test> { <http://example.org/p1> ex:note ?o } }"
        )
        .as_deref(),
        Some("\"just text\""),
        "a constant literal stays a literal"
    );
}

// `rr:object <IRI>` is a constant: the IRI stays an IRI (R2RML §7.4, "if it is
// an IRI, then an IRI will be generated").
#[test]
fn rml_object_shortcut_iri_constant_is_an_iri() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" .
      ex:POM rr:predicate ex:status ; rr:object ex:Active ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id\n1\n")]);
    assert_eq!(
        first(
            &store,
            "SELECT ?o WHERE { <http://example.org/r/1> ex:status ?o }"
        )
        .as_deref(),
        Some("<http://example.org/Active>")
    );
}

// A constant literal keeps its datatype and its language tag, in rr:constant
// and in the rr:object shortcut alike.
#[test]
fn rml_constant_literal_keeps_datatype_and_language() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:P1, ex:P2, ex:P3 .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" .
      ex:P1 rr:predicate ex:rank ; rr:object "5"^^xsd:integer .
      ex:P2 rr:predicate ex:label ; rr:objectMap [ rr:constant "hallo"@nl ] .
      ex:P3 rr:predicate ex:note ; rr:objectMap [ rr:constant "http://not/an/iri" ] ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id\n1\n")]);
    let get = |p: &str| {
        first(
            &store,
            &format!("SELECT ?o WHERE {{ <http://example.org/r/1> ex:{p} ?o }}"),
        )
    };
    assert_eq!(
        get("rank").as_deref(),
        Some("\"5\"^^<http://www.w3.org/2001/XMLSchema#integer>")
    );
    assert_eq!(get("label").as_deref(), Some("\"hallo\"@nl"));
    assert_eq!(
        get("note").as_deref(),
        Some("\"http://not/an/iri\""),
        "a literal that looks like an IRI is still a literal"
    );
}

// R2RML §7.3: a template value is encoded only outside RFC 3987 iunreserved —
// the specification's own examples — and only when the term is an IRI.
#[test]
fn rml_template_values_are_iri_safe() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/v/{v}" .
      ex:POM rr:predicate ex:raw ; rr:objectMap [ rml:reference "v" ] ."#;
    let csv = "v\n42\nHello World!\n2011-08-23T22:17:00Z\n~A_17.1-2\n葉篤正\n";
    let (store, _) = run_rml(mapping, &[("d.csv", csv)]);
    for (value, encoded) in [
        ("42", "42"),
        ("Hello World!", "Hello%20World%21"),
        ("2011-08-23T22:17:00Z", "2011-08-23T22%3A17%3A00Z"),
        ("~A_17.1-2", "~A_17.1-2"),
        ("葉篤正", "葉篤正"),
    ] {
        assert_eq!(
            count(
                &store,
                &format!(
                    "SELECT * WHERE {{ <http://example.org/v/{encoded}> ex:raw \"{value}\" }}"
                )
            ),
            1,
            "{value:?} must become {encoded:?}"
        );
    }
}

// R2RML §11.2 and §9.1: a blank node is unique to its value, so two rows with
// the same value share it; in another graph the same value is another node.
#[test]
fn rml_blank_nodes_are_one_per_value_and_graph() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM, ex:GPOM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "dept{dept}" ; rr:termType rr:BlankNode .
      ex:POM rr:predicate ex:member ; rr:objectMap [ rml:reference "name" ] .
      ex:GPOM rr:predicate ex:tagged ; rr:objectMap [ rml:reference "name" ] ;
        rr:graph ex:Tags ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "dept,name\nA,Ann\nA,Bob\nB,Cy\n")]);
    assert_eq!(
        count(
            &store,
            "SELECT ?d WHERE { ?d ex:member \"Ann\" , \"Bob\" . FILTER(isBlank(?d)) }"
        ),
        1,
        "rows with the same value share one node"
    );
    assert_eq!(
        count(&store, "SELECT DISTINCT ?d WHERE { ?d ex:member ?n }"),
        2,
        "two departments, two nodes"
    );
    assert_eq!(
        count(
            &store,
            "SELECT ?d WHERE { ?d ex:member \"Ann\" . GRAPH ex:Tags { ?d ex:tagged \"Ann\" } }"
        ),
        0,
        "the node in another graph is another node"
    );
    assert_eq!(
        count(
            &store,
            "SELECT ?d WHERE { GRAPH ex:Tags { ?d ex:tagged \"Ann\" , \"Bob\" } }"
        ),
        1,
        "…which the same value shares within that graph"
    );
}

// R2RML §11.1: a triple goes to the union of its subject map's and its
// predicate-object map's graphs; rr:class triples to the subject's graphs; a
// graph map generating rr:defaultGraph names the default graph; with no graph
// map at all, the default graph.
#[test]
fn rml_graph_maps_take_the_union_of_subject_and_predicate_object_graphs() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:Both, ex:Plain .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" ; rr:class foaf:Person ;
        rr:graph ex:People ; rr:graphMap [ rr:template "http://example.org/g/{team}" ] .
      ex:Both rr:predicate foaf:name ; rr:objectMap [ rml:reference "name" ] ;
        rr:graph ex:Names, rr:defaultGraph .
      ex:Plain rr:predicate foaf:nick ; rr:objectMap [ rml:reference "nick" ] .
      ex:N a rr:TriplesMap ;
        rml:logicalSource ex:Src ;
        rr:subjectMap [ rr:template "http://example.org/n/{id}" ] ;
        rr:predicateObjectMap [ rr:predicate ex:plain ; rr:objectMap [ rml:reference "nick" ] ] ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id,name,nick,team\n1,Al,A,red\n")]);
    let graphs_of = |pattern: &str| -> Vec<String> {
        let QueryResults::Solutions(sols) = store
            .query(&format!(
                "{SPARQL_PFX} SELECT ?g WHERE {{ {{ GRAPH ?g {{ {pattern} }} }} UNION \
                 {{ {pattern} BIND(\"default\" AS ?g) }} }} ORDER BY STR(?g)"
            ))
            .unwrap()
        else {
            panic!()
        };
        sols.map(|s| s.unwrap().get("g").unwrap().to_string())
            .collect()
    };
    assert_eq!(
        graphs_of("<http://example.org/r/1> foaf:name \"Al\""),
        vec![
            "\"default\"",
            "<http://example.org/Names>",
            "<http://example.org/People>",
            "<http://example.org/g/red>"
        ],
        "subject graphs ∪ predicate-object graphs, rr:defaultGraph included"
    );
    assert_eq!(
        graphs_of("<http://example.org/r/1> foaf:nick \"A\""),
        vec!["<http://example.org/People>", "<http://example.org/g/red>"],
        "a predicate-object map with no graph map takes the subject's"
    );
    assert_eq!(
        graphs_of("<http://example.org/r/1> a foaf:Person"),
        vec!["<http://example.org/People>", "<http://example.org/g/red>"],
        "rr:class goes to the subject's graphs"
    );
    assert_eq!(
        graphs_of("<http://example.org/n/1> ex:plain \"A\""),
        vec!["\"default\""],
        "no graph map anywhere: the default graph"
    );
}

// A graph map that cannot be parsed is a mapping error, not a silent re-route
// into the default graph.
#[test]
fn rml_a_malformed_graph_map_is_an_error() {
    let err = parse_rml(&format!(
        "{PFX}
         ex:M a rr:TriplesMap ;
           rml:logicalSource [ rml:source \"d.csv\" ; rml:referenceFormulation ql:CSV ] ;
           rr:subjectMap [ rr:template \"http://example.org/r/{{id}}\" ; rr:graphMap [ rr:termType rr:IRI ] ] ."
    ))
    .unwrap_err();
    assert!(err.contains("rr:graphMap"), "{err}");
}

// R2RML §11.2 / RML-Core rml:baseIRI: a generated value that is not an
// absolute IRI is appended to the base IRI.
#[test]
fn rml_relative_iris_resolve_against_the_base_iri() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM ;
        rml:baseIRI <http://example.com/base/> .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "Student/{id}" .
      ex:POM rr:predicate foaf:name ; rr:objectMap [ rml:reference "name" ] .
      ex:Other a rr:TriplesMap ;
        rml:logicalSource ex:Src ;
        rr:subjectMap [ rr:template "Person/{id}" ] ;
        rr:predicateObjectMap [ rr:predicate foaf:nick ; rr:objectMap [ rml:reference "name" ] ] ."#;
    let m = parse_rml(&format!("{PFX}{mapping}")).expect("parse_rml");
    let src = HashMap::from([("d.csv".to_string(), "id,name\n10,Venus\n".to_string())]);

    // No base IRI for ex:Other: "Person/10" is not an IRI, which is a data
    // error (§11.2) — the run aborts, naming the row…
    let err = execute(&m, &src, &TripleStore::in_memory().unwrap(), None).unwrap_err();
    assert!(
        err.contains("<http://example.org/Other> row 1") && err.contains("\"Person/10\""),
        "{err}"
    );
    // …or, skipping it, generates nothing for that row.
    let store = TripleStore::in_memory().unwrap();
    let outcome = execute_with(&m, &src, &store, None, OnDataError::Skip, |_| Ok(())).unwrap();
    assert_eq!(outcome.data_errors.rows, 1);
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.com/base/Student/10> foaf:name \"Venus\" }"
        ),
        1,
        "the triples map's own rml:baseIRI"
    );
    assert_eq!(
        count(&store, "SELECT * WHERE { ?s foaf:nick ?n }"),
        0,
        "no base IRI, no IRI: the relative subject generates nothing"
    );

    // The run's base IRI applies where a triples map names none.
    let mut with_base = m.clone();
    with_base.base_iri = Some("http://example.org/run/".to_string());
    let store = TripleStore::in_memory().unwrap();
    execute(&with_base, &src, &store, None).unwrap();
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.org/run/Person/10> foaf:nick \"Venus\" }"
        ),
        1
    );
    assert_eq!(
        count(
            &store,
            "SELECT * WHERE { <http://example.com/base/Student/10> foaf:name \"Venus\" }"
        ),
        1,
        "rml:baseIRI wins over the run's"
    );
}

// SQL identifiers (R2RML §5): delimiters are spelling, not name. A delimited
// table, a schema-qualified table, a delimited column and a delimited column
// in a template all name what the database holds.
#[test]
fn r2rml_delimited_and_qualified_sql_identifiers() {
    let store = run_relational(
        r#"CREATE TABLE "Student" ("ID" INTEGER PRIMARY KEY, "Name" TEXT);
           INSERT INTO "Student" VALUES (10, 'Venus');
           CREATE TABLE dept (id INTEGER PRIMARY KEY, name TEXT);
           INSERT INTO dept VALUES (1, 'Physics');"#,
        &format!(
            "{PFX}
             ex:S a rr:TriplesMap ;
               rr:logicalTable [ rml:source <urn:source:s> ; rr:tableName \"\\\"Student\\\"\" ] ;
               rr:subjectMap [ rr:template \"http://example.org/s/{{\\\"ID\\\"}}\" ] ;
               rr:predicateObjectMap [ rr:predicate foaf:name ; rr:objectMap [ rr:column \"\\\"Name\\\"\" ] ] .
             ex:D a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"main.dept\" ] ;
               rr:subjectMap [ rr:template \"http://example.org/d/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column \"name\" ] ] ."
        ),
    );
    let ask = |pattern: &str| {
        count(
            &store,
            &format!("SELECT * WHERE {{ GRAPH <urn:run:test> {{ {pattern} }} }}"),
        )
    };
    assert_eq!(ask("<http://example.org/s/10> foaf:name \"Venus\""), 1);
    assert_eq!(ask("<http://example.org/d/1> ex:name \"Physics\""), 1);
}

// A version frozen before the R2RML term rules keeps the old ones: a template
// object map defaults to a literal, every non-alphanumeric template character
// is percent-encoded, and blank nodes are minted per row.
#[test]
fn rml_legacy_semantics_keep_the_old_terms() {
    let mapping = format!(
        "{PFX}
         ex:M a rr:TriplesMap ;
           rml:logicalSource [ rml:source \"d.csv\" ; rml:referenceFormulation ql:CSV ] ;
           rr:subjectMap [ rr:template \"http://example.org/r/{{id}}\" ] ;
           rr:predicateObjectMap [ rr:predicate ex:code ; rr:objectMap [ rr:template \"http://example.org/c/{{code}}\" ] ] ;
           rr:predicateObjectMap [ rr:predicate ex:status ; rr:object ex:Active ] .
         ex:B a rr:TriplesMap ;
           rml:logicalSource [ rml:source \"d.csv\" ; rml:referenceFormulation ql:CSV ] ;
           rr:subjectMap [ rr:template \"g{{grp}}\" ; rr:termType rr:BlankNode ] ;
           rr:predicateObjectMap [ rr:predicate ex:has ; rr:objectMap [ rml:reference \"id\" ] ] ."
    );
    let doc = TripleStore::in_memory().unwrap();
    doc.load_str(&mapping, oxigraph::io::RdfFormat::Turtle, None)
        .unwrap();
    let m = parse_from_store_as(&doc, None, Semantics::Legacy).unwrap();
    let src = HashMap::from([(
        "d.csv".to_string(),
        "id,code,grp\nA-1,x.y,1\nA-2,z,1\n".to_string(),
    )]);
    let store = TripleStore::in_memory().unwrap();
    execute(&m, &src, &store, None).unwrap();
    assert_eq!(
        first(
            &store,
            "SELECT ?c WHERE { <http://example.org/r/A%2D1> ex:code ?c }"
        )
        .as_deref(),
        Some("\"http://example.org/c/x%2Ey\""),
        "legacy: a literal, and the old encoding — of the subject too"
    );
    assert_eq!(
        first(
            &store,
            "SELECT ?s WHERE { <http://example.org/r/A%2D1> ex:status ?s }"
        )
        .as_deref(),
        Some("<http://example.org/Active>"),
        "a constant IRI is an IRI under every version's rules"
    );
    assert_eq!(
        count(&store, "SELECT DISTINCT ?g WHERE { ?g ex:has ?id }"),
        2,
        "legacy: one blank node per row, even for the same value"
    );
}

// ── Many predicate maps and object maps per predicate-object map ──

// R2RML §6.3, §11.1: a predicate-object map generates one triple for every
// predicate map × every object map — `rr:predicate` and `rr:object` shortcuts
// and full maps alike — into each of its graphs.
#[test]
fn rml_predicate_object_map_generates_every_predicate_object_pair() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" .
      ex:POM rr:predicate ex:p1, ex:p2 ;
        rr:predicateMap [ rr:template "http://example.org/p/{kind}" ] ;
        rr:object ex:Const ;
        rr:objectMap [ rml:reference "name" ] ;
        rr:graph ex:G1, ex:G2 ."#;
    let (store, n) = run_rml(mapping, &[("d.csv", "id,kind,name\n1,k,Alice\n")]);
    assert_eq!(n, 12, "3 predicates × 2 objects × 2 graphs");
    for p in ["ex:p1", "ex:p2", "<http://example.org/p/k>"] {
        for o in ["ex:Const", "\"Alice\""] {
            assert_eq!(
                count(
                    &store,
                    &format!(
                        "SELECT ?g WHERE {{ GRAPH ?g {{ <http://example.org/r/1> {p} {o} }} }}"
                    )
                ),
                2,
                "{p} {o} in both graphs"
            );
        }
    }
}

// Each predicate × each referencing object map, and a term object map beside
// it in the same predicate-object map (R2RML §8, §11.1).
#[test]
fn r2rml_every_predicate_takes_every_referencing_object_map() {
    let store = run_relational(
        "CREATE TABLE emp (eid INTEGER PRIMARY KEY, dept INTEGER, boss INTEGER);
         CREATE TABLE dept (did INTEGER PRIMARY KEY);
         INSERT INTO dept VALUES (7);
         INSERT INTO emp VALUES (1, 7, 2), (2, 7, NULL);",
        &format!(
            "{PFX}
             ex:Emp a rr:TriplesMap ;
               rr:logicalTable [ rr:tableName \"emp\" ] ;
               rr:subjectMap [ rr:template \"http://example.org/e/{{eid}}\" ] ;
               rr:predicateObjectMap [
                 rr:predicate ex:linked, ex:related ;
                 rr:objectMap [ rr:parentTriplesMap ex:Dept ;
                                rr:joinCondition [ rr:child \"dept\" ; rr:parent \"did\" ] ] ;
                 rr:objectMap [ rr:parentTriplesMap ex:Emp ;
                                rr:joinCondition [ rr:child \"boss\" ; rr:parent \"eid\" ] ] ;
                 rr:object ex:Staff ] .
             ex:Dept a rr:TriplesMap ;
               rr:logicalTable [ rr:tableName \"dept\" ] ;
               rr:subjectMap [ rr:template \"http://example.org/d/{{did}}\" ] ."
        ),
    );
    for p in ["ex:linked", "ex:related"] {
        let objects = |s: &str| -> usize {
            count(
                &store,
                &format!("SELECT ?o WHERE {{ GRAPH <urn:run:test> {{ <{s}> {p} ?o }} }}"),
            )
        };
        assert_eq!(
            objects("http://example.org/e/1"),
            3,
            "{p}: dept, boss, ex:Staff"
        );
        assert_eq!(
            objects("http://example.org/e/2"),
            2,
            "{p}: dept, ex:Staff — no boss"
        );
    }
}
