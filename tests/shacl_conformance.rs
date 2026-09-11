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

// ─── SHACL-AF §6: custom constraint components ────────────────────────────────

/// A component with an ASK validator on a property shape: `$PATH` is the
/// property path, `$value` each value node, the parameter its local name.
#[test]
fn custom_component_ask_validator_on_a_property_shape() {
    let shapes = r#"
ex:MaxWordsComponent a sh:ConstraintComponent ;
  sh:parameter [ sh:path ex:maxWords ] ;
  sh:propertyValidator [ a sh:SPARQLAskValidator ;
    sh:message "Too many words (max {$maxWords})" ;
    sh:ask """ASK { $this $PATH ?v . FILTER (?v = $value && STRLEN(REPLACE(STR($value), "[^ ]", "")) < $maxWords) }""" ] .
ex:TitleShape a sh:NodeShape ; sh:targetClass ex:Doc ;
  sh:property [ sh:path ex:title ; ex:maxWords 3 ] .
"#;
    let data = r#"
ex:short a ex:Doc ; ex:title "One two" .
ex:long a ex:Doc ; ex:title "One two three four" .
"#;
    let r = run(shapes, data);
    assert!(!r.conforms);
    assert!(violates(&r, "ex:long") || r.results.iter().any(|x| x.focus_node.ends_with("long")));
    assert!(!r.results.iter().any(|x| x.focus_node.ends_with("short")));
    let res = r
        .results
        .iter()
        .find(|x| x.focus_node.ends_with("long"))
        .unwrap();
    assert!(
        res.source_constraint.ends_with("MaxWordsComponent"),
        "{res:?}"
    );
    assert_eq!(
        res.message, "Too many words (max 3)",
        "the message template is rendered"
    );
    assert_eq!(res.value.as_deref(), Some("One two three four"));
}

/// A SELECT validator reports rows as violations; an optional parameter may
/// be absent, a mandatory one must be present for the component to apply.
#[test]
fn custom_component_select_validator_and_optional_parameter() {
    let shapes = r#"
ex:LangComponent a sh:ConstraintComponent ;
  sh:parameter [ sh:path ex:lang ] ;
  sh:parameter [ sh:path ex:note ; sh:optional true ] ;
  sh:propertyValidator [ a sh:SPARQLSelectValidator ;
    sh:select """SELECT $this ?value WHERE { $this $PATH ?value . FILTER (!isLiteral(?value) || !langMatches(lang(?value), $lang)) }""" ] .
ex:LabelShape a sh:NodeShape ; sh:targetClass ex:Country ;
  sh:property [ sh:path ex:label ; ex:lang "de" ] ;
  sh:property [ sh:path ex:name ; ex:note "no lang parameter: the component does not apply" ] .
"#;
    let data = r#"
ex:nl a ex:Country ; ex:label "Niederlande"@de ; ex:name "Netherlands" .
ex:be a ex:Country ; ex:label "Belgium"@en ; ex:name "Belgium" .
"#;
    let r = run(shapes, data);
    assert!(!r.conforms);
    let be: Vec<_> = r
        .results
        .iter()
        .filter(|x| x.focus_node.ends_with("/be"))
        .collect();
    assert_eq!(
        be.len(),
        1,
        "one violation for be's English label: {:?}",
        r.results
    );
    assert_eq!(be[0].value.as_deref(), Some("Belgium"));
    assert!(
        !r.results.iter().any(|x| x.focus_node.ends_with("/nl")),
        "{:?}",
        r.results
    );
}

/// Pre-binding: `$this` reaches a FILTER in a UNION branch and a nested
/// group (the spec's substitution semantics), and `bound($this)` is true.
#[test]
fn prebinding_reaches_nested_scopes() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetNode ex:bad , ex:good ;
  sh:sparql [ sh:select """SELECT $this WHERE { { FILTER (false) } UNION { { FILTER ($this = <http://example.org/bad>) } FILTER bound($this) } }""" ] .
"#;
    let r = run(shapes, "ex:bad ex:p 1 . ex:good ex:p 2 .");
    assert!(violates(&r, "/bad"), "{:?}", r.results);
    assert!(!violates(&r, "/good"), "{:?}", r.results);
}

/// The SPARQL features SHACL forbids under pre-binding (MINUS, VALUES,
/// SERVICE, a nested SELECT not projecting $this, `AS $this`) make the shapes
/// graph invalid — validation fails, it does not silently pass.
#[test]
fn unsupported_prebinding_features_fail_the_shapes_graph() {
    for (what, select) in [
        (
            "MINUS",
            "SELECT $this WHERE { $this ?p ?o . MINUS { $this ?p \"x\" } }",
        ),
        (
            "VALUES",
            "SELECT $this WHERE { VALUES ?x { 1 } FILTER($this = <http://example.org/a>) }",
        ),
        (
            "SERVICE",
            "SELECT $this WHERE { SERVICE <http://example.org/sparql> { $this ?p ?o } }",
        ),
        (
            "nested SELECT *",
            "SELECT $this WHERE { { SELECT * WHERE { $this ?p ?o } } }",
        ),
        (
            "AS $this",
            "SELECT $this WHERE { BIND (<http://example.org/a> AS $this) }",
        ),
    ] {
        let shapes = format!(
            "ex:S a sh:NodeShape ; sh:targetNode ex:a ; sh:sparql [ sh:select \"\"\"{select}\"\"\" ] ."
        );
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(
                &format!("{PFX}{shapes}"),
                RdfFormat::Turtle,
                Some("urn:shapes"),
            )
            .unwrap();
        store
            .load_str(
                &format!("{PFX}ex:a ex:p 1 ."),
                RdfFormat::Turtle,
                Some("urn:data"),
            )
            .unwrap();
        let r = validate(&store, "urn:shapes", &["urn:data".to_string()]);
        assert!(
            r.is_err(),
            "{what} must be rejected under pre-binding, got {r:?}"
        );
    }
}

// ─── Fail closed: inline shapes and SPARQL targets that cannot be loaded ──────

/// An inline `sh:node` body whose constraint cannot be evaluated used to be
/// skipped at load (`if let Ok(..)`), so the shape validated nothing and
/// passed. It fails the shapes graph now, like a top-level shape does.
#[test]
fn an_inline_sh_node_with_a_malformed_constraint_fails_the_shapes_graph() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:T ;
  sh:node [ sh:sparql [ sh:select "THIS IS NOT SPARQL" ] ] .
"#;
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            &format!("{PFX}{shapes}"),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    store
        .load_str(
            &format!("{PFX}ex:a a ex:T ."),
            RdfFormat::Turtle,
            Some("urn:data"),
        )
        .unwrap();
    let r = validate(&store, "urn:shapes", &["urn:data".to_string()]);
    assert!(
        r.is_err(),
        "a malformed inline shape must fail the run, got {r:?}"
    );
}

/// A SPARQL target that does not parse, or does not project `?this`, used to
/// yield no focus nodes — the shape passed over nothing.
#[test]
fn a_sparql_target_that_cannot_select_focus_nodes_fails_the_shapes_graph() {
    for (what, select) in [
        ("garbage", "THIS IS NOT SPARQL"),
        (
            "no ?this",
            "SELECT ?x WHERE { ?x a <http://example.org/T> }",
        ),
    ] {
        let shapes = format!(
            "ex:S a sh:NodeShape ; sh:target [ sh:select \"{select}\" ] ; sh:property [ sh:path ex:p ; sh:minCount 1 ] ."
        );
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(
                &format!("{PFX}{shapes}"),
                RdfFormat::Turtle,
                Some("urn:shapes"),
            )
            .unwrap();
        store
            .load_str(
                &format!("{PFX}ex:a a ex:T ."),
                RdfFormat::Turtle,
                Some("urn:data"),
            )
            .unwrap();
        let r = validate(&store, "urn:shapes", &["urn:data".to_string()]);
        assert!(
            r.is_err(),
            "{what}: the target must fail the run, got {r:?}"
        );
    }
    // The well-formed target still selects its focus nodes. It has to name the
    // data graph itself: a SPARQL target is evaluated against the raw store
    // (`execute_select_terms`) with no `FROM` prologue for the run's data
    // graphs, so an unqualified pattern sees only the default graph and
    // silently matches nothing. That scoping gap is pre-existing, is a
    // behaviour change to close, and is recorded in the improvement log.
    let shapes = "ex:S a sh:NodeShape ; sh:target [ sh:select \"SELECT ?this WHERE { GRAPH <urn:data> { ?this a <http://example.org/T> } }\" ] ; sh:property [ sh:path ex:p ; sh:minCount 1 ] .";
    let r = run(shapes, "ex:a a ex:T .");
    assert!(violates(&r, "/a"), "{:?}", r.results);
}
