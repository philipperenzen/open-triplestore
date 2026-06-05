//! SHACL Core + SHACL-SPARQL conformance tests (high-complexity).
//!
//! Grounded in the W3C SHACL Recommendation (20 July 2017) + SHACL-SPARQL, and
//! adversarially fact-checked. Verifier corrections applied (e.g. hc-14: an
//! absent `sh:targetNode` is still a focus node, so `sh:minCount` yields a
//! violation → conforms FALSE).
//!
//! Blank-node property shapes (`sh:property [ … ]`, the standard SHACL idiom) and
//! inline blank nested shapes are enforced correctly: the loader dereferences
//! blank nodes through the raw quad index rather than via invalid `<_:bn>` SPARQL.
//! See `shacl_blank_node_property_shapes_enforced` for the regression guard.
//!
//! Shapes load into `urn:shapes`, data into `urn:data`, then
//! `shacl::validate(store, "urn:shapes", &["urn:data"])`.

use open_triplestore::shacl::report::ValidationReport;
use open_triplestore::shacl::validate;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;

const PFX: &str = "@prefix ex: <http://example.org/> .\n\
@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n";

fn run(shapes: &str, data: &str) -> ValidationReport {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            &format!("{PFX}{shapes}"),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    store
        .load_str(&format!("{PFX}{data}"), RdfFormat::Turtle, Some("urn:data"))
        .unwrap();
    validate(&store, "urn:shapes", &["urn:data".to_string()]).unwrap()
}

fn violates(r: &ValidationReport, suffix: &str) -> bool {
    r.results.iter().any(|v| v.focus_node.contains(suffix))
}

// Cardinality + value type via a named property shape.
#[test]
fn shacl_cardinality_and_datatype() {
    let shapes = r#"
      ex:S a sh:NodeShape ; sh:targetClass ex:Person ; sh:property ex:NameProp .
      ex:NameProp a sh:PropertyShape ; sh:path ex:name ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ."#;
    let data = r#"
      ex:ok    a ex:Person ; ex:name "Ann" .
      ex:none  a ex:Person .
      ex:twice a ex:Person ; ex:name "A", "B" .
      ex:wrong a ex:Person ; ex:name 42 ."#;
    let r = run(shapes, data);
    assert!(!r.conforms);
    assert!(violates(&r, "/none"), "minCount 1 violated");
    assert!(violates(&r, "/twice"), "maxCount 1 violated");
    assert!(violates(&r, "/wrong"), "datatype xsd:string violated");
    assert!(!violates(&r, "/ok"), "valid node conforms");
}

// hc-14 (CORRECTED): a sh:targetNode absent from the data is still a focus node.
#[test]
fn shacl_target_node_absent_still_validated() {
    let shapes = r#"
      ex:S a sh:NodeShape ; sh:targetNode ex:Alice, ex:Bob, ex:Charlie ; sh:property ex:NameProp .
      ex:NameProp a sh:PropertyShape ; sh:path ex:name ; sh:minCount 1 ."#;
    let data = r#"
      ex:Alice ex:name "Alice" .
      ex:Bob ex:name "Bob" ."#; // ex:Charlie absent
    let r = run(shapes, data);
    assert!(
        !r.conforms,
        "absent targetNode is still validated => minCount violation"
    );
    assert!(
        violates(&r, "/Charlie"),
        "Charlie (absent) must produce a violation"
    );
    assert!(!violates(&r, "/Alice") && !violates(&r, "/Bob"));
}

// hc-10: sh:deactivated true suppresses ALL results for the shape.
#[test]
fn shacl_deactivated_shape() {
    let shapes = r#"
      ex:S a sh:NodeShape ; sh:targetClass ex:Product ; sh:deactivated true ; sh:property ex:PriceProp .
      ex:PriceProp a sh:PropertyShape ; sh:path ex:price ; sh:datatype xsd:decimal ; sh:minCount 1 ."#;
    let data = r#"
      ex:p1 a ex:Product .
      ex:p2 a ex:Product ; ex:price "free"^^xsd:string ."#;
    let r = run(shapes, data);
    assert!(r.conforms, "deactivated shape produces no results");
    assert_eq!(r.results_count, 0);
}

// hc-05: sh:languageIn accepts BCP47 subtags; rejects other langs and untagged literals.
#[test]
fn shacl_language_in_subtags() {
    let shapes = r#"
      ex:S a sh:NodeShape ; sh:targetClass ex:Place ; sh:property ex:NameProp .
      ex:NameProp a sh:PropertyShape ; sh:path ex:placeName ; sh:languageIn ( "en" "mi" ) ."#;
    let data = r#"
      ex:p1 a ex:Place ; ex:placeName "Aotearoa"@mi .
      ex:p2 a ex:Place ; ex:placeName "New Zealand"@en-NZ .
      ex:p3 a ex:Place ; ex:placeName "Neuseeland"@de .
      ex:p4 a ex:Place ; ex:placeName "NoTag" ."#;
    let r = run(shapes, data);
    assert!(!r.conforms);
    assert!(violates(&r, "/p3"), "@de not allowed");
    assert!(!violates(&r, "/p1"), "@mi conforms");
    assert!(!violates(&r, "/p2"), "@en-NZ subtag conforms");
}

// hc-09: sh:not — a node satisfying the (named) inner shape violates the outer shape.
#[test]
fn shacl_not_constraint() {
    let shapes = r#"
      ex:WarnShape a sh:NodeShape ; sh:property ex:OptProp .
      ex:OptProp a sh:PropertyShape ; sh:path ex:optField ; sh:minCount 1 .
      ex:OuterShape a sh:NodeShape ; sh:targetClass ex:Doc ; sh:not ex:WarnShape ."#;
    let data = r#"
      ex:doc1 a ex:Doc ; ex:optField "present" .
      ex:doc2 a ex:Doc ."#;
    let r = run(shapes, data);
    assert!(!r.conforms);
    assert!(
        violates(&r, "/doc1"),
        "doc1 satisfies inner shape => violates sh:not"
    );
    assert!(!violates(&r, "/doc2"), "doc2 conforms");
}

// hc-06: sh:xone requires EXACTLY one — zero and two matches both violate.
#[test]
fn shacl_xone_exactly_one() {
    let shapes = r#"
      ex:ContactShape a sh:NodeShape ; sh:targetClass ex:Contact ; sh:xone ( ex:HasEmail ex:HasPhone ) .
      ex:HasEmail a sh:NodeShape ; sh:property ex:EmailProp .
      ex:EmailProp a sh:PropertyShape ; sh:path ex:email ; sh:minCount 1 .
      ex:HasPhone a sh:NodeShape ; sh:property ex:PhoneProp .
      ex:PhoneProp a sh:PropertyShape ; sh:path ex:phone ; sh:minCount 1 ."#;
    let data = r#"
      ex:c1 a ex:Contact ; ex:email "a@b.com" .
      ex:c2 a ex:Contact ; ex:phone "+1234" .
      ex:c3 a ex:Contact .
      ex:c4 a ex:Contact ; ex:email "x@y.com" ; ex:phone "+5678" ."#;
    let r = run(shapes, data);
    assert!(!r.conforms);
    assert!(violates(&r, "/c3"), "zero matches violate xone");
    assert!(violates(&r, "/c4"), "two matches violate xone");
    assert!(
        !violates(&r, "/c1") && !violates(&r, "/c2"),
        "exactly-one conforms"
    );
}

// hc-01: sh:qualifiedValueShape enforces per-value-shape min/max counts. A valid
// hand (1 thumb + 4 fingers) conforms; a deficient one violates qualifiedMinCount.
#[test]
fn shacl_qualified_value_shapes() {
    let shapes = r#"
      ex:HandShape a sh:NodeShape ; sh:targetClass ex:Hand ;
        sh:property ex:ThumbDigit, ex:FingerDigit .
      ex:ThumbDigit a sh:PropertyShape ; sh:path ex:digit ;
        sh:qualifiedValueShape ex:ThumbShape ; sh:qualifiedMinCount 1 ; sh:qualifiedMaxCount 1 .
      ex:FingerDigit a sh:PropertyShape ; sh:path ex:digit ;
        sh:qualifiedValueShape ex:FingerShape ; sh:qualifiedMinCount 4 ; sh:qualifiedMaxCount 4 .
      ex:ThumbShape a sh:NodeShape ; sh:class ex:Thumb .
      ex:FingerShape a sh:NodeShape ; sh:class ex:Finger ."#;
    let ok = run(
        shapes,
        r#"ex:hand1 a ex:Hand ; ex:digit ex:d1, ex:d2, ex:d3, ex:d4, ex:d5 .
           ex:d1 a ex:Thumb . ex:d2 a ex:Finger . ex:d3 a ex:Finger . ex:d4 a ex:Finger . ex:d5 a ex:Finger ."#,
    );
    assert!(
        ok.conforms,
        "1 thumb + 4 fingers conforms, got {:?}",
        ok.results
            .iter()
            .map(|r| r.source_constraint.clone())
            .collect::<Vec<_>>()
    );
    let bad = run(
        shapes,
        r#"ex:hand2 a ex:Hand ; ex:digit ex:t1, ex:f1, ex:f2 .
           ex:t1 a ex:Thumb . ex:f1 a ex:Finger . ex:f2 a ex:Finger ."#,
    );
    assert!(
        !bad.conforms,
        "1 thumb + 2 fingers violates qualifiedMinCount 4 (Finger)"
    );
    assert!(violates(&bad, "/hand2"));
}

// hc-11: a node-level sh:sparql constraint with SUM/HAVING aggregation. $this is
// pre-bound to the focus node, so the aggregate validator fires correctly.
#[test]
fn shacl_sparql_aggregation_constraint() {
    let shapes = r#"
      ex:FractionShape a sh:NodeShape ; sh:targetClass ex:Mixture ; sh:sparql ex:SumConstraint .
      ex:SumConstraint a sh:SPARQLConstraint ; sh:message "fractions must sum to 1.0" ;
        sh:select """SELECT $this (SUM(?frac) AS ?total) WHERE { $this <http://example.org/hasFraction> ?frac . } GROUP BY $this HAVING (SUM(?frac) != 1.0)""" ."#;
    // Distinct fraction values per node (identical triples would collapse under RDF
    // set semantics): m1 = 0.4+0.6 = 1.0 (conforms); m2 = 0.2+0.3 = 0.5 (violates).
    let data = r#"
      ex:m1 a ex:Mixture ; ex:hasFraction 0.4 ; ex:hasFraction 0.6 .
      ex:m2 a ex:Mixture ; ex:hasFraction 0.2 ; ex:hasFraction 0.3 ."#;
    let r = run(shapes, data);
    assert!(!r.conforms);
    assert!(violates(&r, "/m2"), "m2 sum=0.5 != 1.0 violates");
    assert!(!violates(&r, "/m1"), "m1 sum=1.0 conforms");
}

// Blank-node property shapes (the standard SHACL idiom, `sh:property [ … ]`) are
// enforced exactly like named property shapes. Regression guard for the loader's
// blank-node dereferencing (objects_for_subject_in_graph).
#[test]
fn shacl_blank_node_property_shapes_enforced() {
    let named = run(
        r#"ex:S1 a sh:NodeShape ; sh:targetClass ex:T ; sh:property ex:P1 .
           ex:P1 a sh:PropertyShape ; sh:path ex:name ; sh:minCount 1 ."#,
        r#"ex:a a ex:T ."#,
    );
    assert!(!named.conforms, "named property shape enforced");

    let blank = run(
        r#"ex:S2 a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path ex:name ; sh:minCount 1 ] ."#,
        r#"ex:b a ex:T ."#,
    );
    assert!(
        !blank.conforms,
        "blank-node property shape must be enforced (minCount 1 violated)"
    );
    assert!(violates(&blank, "/b"));
}
