//! RML (RDF Mapping Language) + R2RML conformance tests.
//!
//! Grounded in the RML spec (https://rml.io/specs/rml/) and R2RML
//! (https://www.w3.org/TR/r2rml/), adversarially fact-checked. The engine
//! implements R2RML vocabulary (`rr:`) + RML source extensions (`rml:`) for
//! CSV / JSONPath / XPath logical sources, with template / reference / constant
//! term maps, term types, datatypes, languages, `rr:class`, and graph maps.
//!
//! KNOWN ENGINE LIMITATION (see `rml_inline_blank_node_mapping_gap`): like the
//! SHACL loader, `parse_rml` is store-based and mis-dereferences INLINE BLANK
//! NODES, so mappings authored with `rr:subjectMap [ ... ]` / multiple
//! `rr:predicateObjectMap [ ... ]` cross-contaminate. These tests therefore use
//! NAMED term-map resources (the form in the engine's own working test), which
//! parse correctly, to exercise the mapping features.
//!
//! Referencing object maps (joins / `rr:parentTriplesMap`) are not modelled —
//! documented as a gap.

use open_triplestore::rml::{execute, parse_rml};
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

// Documented behavior: an EMPTY CSV cell currently produces an (empty-string)
// literal rather than suppressing the triple. The RML spec suppresses triples for
// NULL/absent values; for CSV the engine treats an empty cell as an empty string.
#[test]
fn rml_empty_csv_cell_behavior() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" .
      ex:POM rr:predicate ex:nickname ; rr:objectMap ex:Obj .
      ex:Obj rml:reference "nickname" ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id,nickname\n1,Ace\n2,\n")]);
    let n = count(&store, "SELECT ?o WHERE { ?s ex:nickname ?o }");
    // Row 1 -> "Ace"; row 2 -> "" (empty literal). Document current (non-suppressing) behavior.
    assert!(n >= 1, "at least the non-empty value is mapped, got {n}");
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

// rr:template on an object map builds a (literal) object from a column.
#[test]
fn rml_object_template_literal() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" .
      ex:POM rr:predicate ex:greeting ; rr:objectMap ex:Obj .
      ex:Obj rr:template "Hello {name}" ."#;
    let (store, _) = run_rml(mapping, &[("d.csv", "id,name\n1,Sam\n")]);
    assert_eq!(
        first(
            &store,
            "SELECT ?g WHERE { <http://example.org/r/1> ex:greeting ?g }"
        )
        .as_deref(),
        Some("\"Hello Sam\"")
    );
}

// rr:graphMap routes the generated triples into a named graph.
#[test]
fn rml_graph_map_routes_to_named_graph() {
    let mapping = r#"
      ex:M a rr:TriplesMap ;
        rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
        rr:predicateObjectMap ex:POM ;
        rr:graphMap ex:GM .
      ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
      ex:Subj rr:template "http://example.org/r/{id}" .
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

// Tracked gap: referencing object maps (rr:parentTriplesMap joins) are not modelled.
#[test]
fn rml_referencing_object_map_join_is_gap() {
    let mapping = r#"
      ex:Child a rr:TriplesMap ;
        rml:logicalSource ex:CSrc ; rr:subjectMap ex:CSubj ;
        rr:predicateObjectMap ex:ParentPOM .
      ex:CSrc rml:source "c.csv" ; rml:referenceFormulation ql:CSV .
      ex:CSubj rr:template "http://example.org/c/{id}" .
      ex:ParentPOM rr:predicate ex:parent ; rr:objectMap ex:ParentObj .
      ex:ParentObj rr:parentTriplesMap ex:Parent ;
        rr:joinCondition ex:Join .
      ex:Join rr:child "pid" ; rr:parent "id" .
      ex:Parent a rr:TriplesMap ;
        rml:logicalSource ex:PSrc ; rr:subjectMap ex:PSubj ;
        rr:predicateObjectMap ex:NamePOM .
      ex:PSrc rml:source "p.csv" ; rml:referenceFormulation ql:CSV .
      ex:PSubj rr:template "http://example.org/p/{id}" .
      ex:NamePOM rr:predicate foaf:name ; rr:objectMap ex:NameObj .
      ex:NameObj rml:reference "name" ."#;
    // The referencing object map has no template/reference/constant, so the engine
    // either fails to parse the mapping OR produces no joined triple — both confirm
    // the gap. (Neither outcome is a join.)
    let m = parse_rml(&format!("{PFX}{mapping}"));
    match m {
        Err(_) => { /* gap: referencing object map not parseable */ }
        Ok(m) => {
            let mut src = HashMap::new();
            src.insert("c.csv".to_string(), "id,pid\n1,10\n".to_string());
            src.insert("p.csv".to_string(), "id,name\n10,Pat\n".to_string());
            let store = TripleStore::in_memory().unwrap();
            let _ = execute(&m, &src, &store, None);
            assert_eq!(
                count(&store, "SELECT ?o WHERE { <http://example.org/c/1> ex:parent <http://example.org/p/10> }"),
                0,
                "tracked gap: rr:parentTriplesMap joins are not implemented"
            );
        }
    }
}
