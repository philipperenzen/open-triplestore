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

// ─── SHACL-SPARQL: pre-bound terms, ?failure, messages, deactivation ─────────
//
// Pre-bound variables are seeded as terms (oxigraph's `substitute_variable`),
// not pasted into the query text. No SPARQL syntax names a stored blank node,
// so the text form skipped blank-node focus and value nodes: their
// constraints never ran and the node conformed.

/// The nested-scope semantics of `prebinding_reaches_nested_scopes` hold for a
/// blank-node focus too: it reaches a pattern inside a UNION branch.
#[test]
fn prebinding_reaches_nested_scopes_for_a_blank_node_focus() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:T ;
  sh:sparql [ sh:select """SELECT $this WHERE { { FILTER (false) } UNION { { $this <http://example.org/bad> true } FILTER bound($this) } }""" ] .
"#;
    let r = run(
        shapes,
        "[ a ex:T ; ex:bad true ] . [ a ex:T ; ex:bad false ] . ex:named a ex:T ; ex:bad false .",
    );
    let blank: Vec<_> = r
        .results
        .iter()
        .filter(|x| x.focus_node.starts_with("_:"))
        .collect();
    assert_eq!(
        blank.len(),
        1,
        "exactly the blank node with ex:bad true violates: {:?}",
        r.results
    );
    assert_eq!(r.results_count, 1, "{:?}", r.results);
}

/// Pre-binding keeps triple patterns bound. The optimizer cannot tell that
/// the rewrite's table binds `$this`, so without the seed every pattern was
/// scanned in full for each focus node (seconds per shape on 20 000 triples).
/// Counted, not timed: with one focus node on a ring of 1 000, each pattern of
/// the plan yields one row, not one per subject.
#[test]
fn prebound_triple_patterns_are_looked_up_by_the_value() {
    use open_triplestore::sparql::prebind;
    use oxigraph::model::{NamedNode, Term};
    use oxigraph::sparql::{QueryResults, SparqlEvaluator};
    use oxigraph::store::Store;

    fn quad_pattern_rows(node: &serde_json::Value, out: &mut Vec<(String, u64)>) {
        let name = node["name"].as_str().unwrap_or_default();
        if name.starts_with("QuadPattern") {
            out.push((
                name.to_string(),
                node["number of results"].as_u64().unwrap(),
            ));
        }
        for child in node["children"].as_array().into_iter().flatten() {
            quad_pattern_rows(child, out);
        }
    }

    let store = Store::new().unwrap();
    let ring: String = (0..1000)
        .map(|i| {
            format!(
                "<http://example.org/n{i}> <http://example.org/next> <http://example.org/n{}> .\n",
                (i + 1) % 1000
            )
        })
        .collect();
    store
        .load_from_reader(RdfFormat::NTriples, ring.as_bytes())
        .unwrap();
    let this: Term = NamedNode::new("http://example.org/n1").unwrap().into();
    for text in [
        "CONSTRUCT { $this <http://example.org/one> ?a } WHERE { $this <http://example.org/next> ?a }",
        "CONSTRUCT { $this <http://example.org/two> ?b } WHERE { $this <http://example.org/next> ?a . ?a <http://example.org/next> ?b FILTER (bound($this)) }",
    ] {
        let mut query = open_triplestore::sparql::parser().parse_query(text).unwrap();
        prebind::rewrite(&mut query, &["this"]).unwrap();
        let (results, explanation) =
            prebind::prepare(SparqlEvaluator::new(), query, &[("this", &this)])
                .unwrap()
                .on_store(&store)
                .compute_statistics()
                .explain();
        let Ok(QueryResults::Graph(triples)) = results else {
            panic!("{text}: no graph");
        };
        assert_eq!(triples.count(), 1, "{text}");
        let mut json = Vec::new();
        explanation.write_in_json(&mut json).unwrap();
        let plan: serde_json::Value = serde_json::from_slice(&json).unwrap();
        let mut rows = Vec::new();
        quad_pattern_rows(&plan["plan"], &mut rows);
        assert!(!rows.is_empty(), "{text}: {plan}");
        assert!(
            rows.iter().all(|(_, n)| *n <= 1),
            "{text}: a triple pattern was scanned instead of looked up: {rows:?}"
        );
    }
}

/// A `sh:sparql` constraint on a property shape checks a blank-node focus,
/// with `$PATH` replaced by the shape's path.
#[test]
fn a_sparql_constraint_checks_a_blank_node_focus() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:T ;
  sh:property [ sh:path ex:size ;
    sh:sparql [ sh:select """SELECT $this ?value WHERE { $this $PATH ?value . FILTER (?value > 10) }""" ] ] .
"#;
    let r = run(
        shapes,
        "[ a ex:T ; ex:size 20 ] . ex:ok a ex:T ; ex:size 5 .",
    );
    assert_eq!(r.results_count, 1, "{:?}", r.results);
    assert!(r.results[0].focus_node.starts_with("_:"), "{:?}", r.results);
    assert!(!violates(&r, "/ok"));
}

/// An ASK validator checks a blank-node value node: `$value` is bound to it.
#[test]
fn an_ask_validator_checks_a_blank_node_value() {
    let shapes = r#"
ex:IntactComponent a sh:ConstraintComponent ;
  sh:parameter [ sh:path ex:intact ] ;
  sh:validator [ a sh:SPARQLAskValidator ;
    sh:ask """ASK { FILTER ($intact = false || NOT EXISTS { $value <http://example.org/broken> true }) }""" ] .
ex:S a sh:NodeShape ; sh:targetClass ex:T ;
  sh:property [ sh:path ex:part ; ex:intact true ] .
"#;
    let r = run(
        shapes,
        "ex:a a ex:T ; ex:part [ ex:broken true ] . ex:b a ex:T ; ex:part [ ex:broken false ] .",
    );
    assert_eq!(flagged(&r), set(&["a"]), "{:?}", r.results);
    assert!(r.results[0]
        .value
        .as_deref()
        .unwrap_or("")
        .starts_with("_:"));
}

/// A solution binding `?failure` to true is a failure of the constraint —
/// reported, never a pass — while the other solutions stay ordinary results
/// (SHACL §5.3).
#[test]
fn a_failure_binding_fails_the_constraint() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetSubjectsOf ex:p ;
  sh:sparql [ sh:message "p is not positive" ;
    sh:select """SELECT $this ?failure WHERE { $this <http://example.org/p> ?v . FILTER (?v <= 0) BIND (?v = 0 AS ?failure) }""" ] .
"#;
    let r = run(shapes, "ex:zero ex:p 0 . ex:neg ex:p -1 . ex:pos ex:p 1 .");
    assert!(!r.conforms);
    assert_eq!(flagged(&r), set(&["zero", "neg"]), "{:?}", r.results);
    let zero = r
        .results
        .iter()
        .find(|x| x.focus_node.ends_with("/zero"))
        .unwrap();
    assert!(zero.message.contains("?failure"), "{zero:?}");
    let neg = r
        .results
        .iter()
        .find(|x| x.focus_node.ends_with("/neg"))
        .unwrap();
    assert_eq!(neg.message, "p is not positive", "{neg:?}");

    // The same for a SELECT validator of a constraint component.
    let shapes = r#"
ex:PositiveComponent a sh:ConstraintComponent ;
  sh:parameter [ sh:path ex:positive ] ;
  sh:nodeValidator [ a sh:SPARQLSelectValidator ;
    sh:select """SELECT $this ?failure WHERE { $this <http://example.org/p> ?v . FILTER ($positive && ?v <= 0) BIND (?v = 0 AS ?failure) }""" ] .
ex:S a sh:NodeShape ; sh:targetSubjectsOf ex:p ; ex:positive true .
"#;
    let r = run(shapes, "ex:zero ex:p 0 . ex:pos ex:p 1 .");
    assert_eq!(flagged(&r), set(&["zero"]), "{:?}", r.results);
    assert!(r.results[0].message.contains("?failure"), "{:?}", r.results);
}

/// `sh:resultMessage` (SHACL §5.3.2): a `?message` binding wins; otherwise the
/// `sh:message` template is filled from the solution's `{?var}` / `{$var}`.
/// A block naming no binding is left as written.
#[test]
fn sparql_messages_are_filled_from_the_solution() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetSubjectsOf ex:size ;
  sh:sparql [ sh:message "Size {?value} of {$this} exceeds {?limit}{?nope}" ;
    sh:select """SELECT $this ?value ?limit WHERE { $this <http://example.org/size> ?value . BIND (10 AS ?limit) FILTER (?value > ?limit && ?value < 100) }""" ] ;
  sh:sparql [ sh:message "not used" ;
    sh:select """SELECT $this ?message WHERE { $this <http://example.org/size> ?v . FILTER (?v >= 100) BIND (CONCAT("huge: ", STR(?v)) AS ?message) }""" ] .
"#;
    let r = run(
        shapes,
        "ex:big ex:size 20 . ex:huge ex:size 200 . ex:ok ex:size 5 .",
    );
    assert_eq!(flagged(&r), set(&["big", "huge"]), "{:?}", r.results);
    let big = r
        .results
        .iter()
        .find(|x| x.focus_node.ends_with("/big"))
        .unwrap();
    assert_eq!(
        big.message,
        "Size 20 of http://example.org/big exceeds 10{?nope}"
    );
    let huge = r
        .results
        .iter()
        .find(|x| x.focus_node.ends_with("/huge"))
        .unwrap();
    assert_eq!(huge.message, "huge: 200");
}

/// A SPARQL-based constraint with `sh:deactivated true` produces no results
/// (SHACL §5.3); its active sibling still does.
#[test]
fn a_deactivated_sparql_constraint_produces_no_results() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetNode ex:n ;
  sh:sparql [ sh:deactivated true ; sh:message "off" ;
    sh:select """SELECT $this WHERE { }""" ] ;
  sh:sparql [ sh:message "on" ;
    sh:select """SELECT $this WHERE { FILTER (false) }""" ] .
"#;
    let r = run(shapes, "ex:n ex:p 1 .");
    assert!(r.conforms, "{:?}", r.results);

    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetNode ex:n ;
  sh:sparql [ sh:deactivated true ; sh:message "off" ;
    sh:select """SELECT $this WHERE { }""" ] ;
  sh:sparql [ sh:message "on" ;
    sh:select """SELECT $this WHERE { }""" ] .
"#;
    let r = run(shapes, "ex:n ex:p 1 .");
    let messages: Vec<_> = r.results.iter().map(|x| x.message.as_str()).collect();
    assert_eq!(messages, ["on"]);
}

/// A validator carrying `sh:deactivated true` is not used: the shape falls
/// back to the component's `sh:validator`, and when no validator is left the
/// component is switched off — not an ill-formed shapes graph.
#[test]
fn a_deactivated_validator_is_not_used() {
    let shapes = r#"
ex:OffComponent a sh:ConstraintComponent ;
  sh:parameter [ sh:path ex:off ] ;
  sh:propertyValidator [ a sh:SPARQLSelectValidator ; sh:deactivated true ;
    sh:select """SELECT $this WHERE { }""" ] .
ex:FallbackComponent a sh:ConstraintComponent ;
  sh:parameter [ sh:path ex:fallback ] ;
  sh:propertyValidator [ a sh:SPARQLSelectValidator ; sh:deactivated true ;
    sh:message "deactivated validator" ; sh:select """SELECT $this WHERE { }""" ] ;
  sh:validator [ a sh:SPARQLAskValidator ; sh:message "fallback validator" ;
    sh:ask """ASK { FILTER ($value != $fallback) }""" ] .
ex:S a sh:NodeShape ; sh:targetSubjectsOf ex:colour ;
  sh:property [ sh:path ex:colour ; ex:off true ; ex:fallback "red" ] .
"#;
    let r = try_run(shapes, "ex:a ex:colour \"red\" . ex:b ex:colour \"blue\" .")
        .expect("a switched-off component is not an ill-formed shapes graph");
    // Only the fallback validator ran: it flags the value equal to
    // ex:fallback, and the switched-off components flag nothing.
    assert_eq!(flagged(&r), set(&["a"]), "{:?}", r.results);
    assert_eq!(r.results[0].message, "fallback validator");
}

/// Every value of a single-parameter component's parameter declares its own
/// constraint (SHACL §4). Only the first value used to be read; which one was
/// store order, so there is one node per value. The component's own
/// `sh:message` is used when the validator has none.
#[test]
fn every_value_of_a_component_parameter_is_its_own_constraint() {
    let shapes = r#"
ex:ForbiddenComponent a sh:ConstraintComponent ;
  sh:parameter [ sh:path ex:forbidden ] ;
  sh:message "{$value} is forbidden ({$forbidden})" ;
  sh:validator [ a sh:SPARQLAskValidator ; sh:ask """ASK { FILTER ($value != $forbidden) }""" ] .
ex:S a sh:NodeShape ; sh:targetSubjectsOf ex:colour ;
  sh:property [ sh:path ex:colour ; ex:forbidden "red" , "blue" ] .
"#;
    let r = run(
        shapes,
        "ex:r ex:colour \"red\" . ex:b ex:colour \"blue\" . ex:g ex:colour \"green\" .",
    );
    assert_eq!(flagged(&r), set(&["r", "b"]), "{:?}", r.results);
    let red = r
        .results
        .iter()
        .find(|x| x.focus_node.ends_with("/r"))
        .unwrap();
    assert_eq!(red.message, "red is forbidden (red)");
}

/// A component with several parameters takes one value for each: a shape
/// giving one of them two values is ill-formed (SHACL §4) and fails the run.
#[test]
fn a_multi_parameter_component_given_two_values_fails_the_shapes_graph() {
    let shapes = r#"
ex:LangComponent a sh:ConstraintComponent ;
  sh:parameter [ sh:path ex:lang ] ;
  sh:parameter [ sh:path ex:note ; sh:optional true ] ;
  sh:propertyValidator [ a sh:SPARQLSelectValidator ;
    sh:select """SELECT $this ?value WHERE { $this $PATH ?value . FILTER (!langMatches(lang(?value), $lang)) }""" ] .
ex:S a sh:NodeShape ; sh:targetSubjectsOf ex:label ;
  sh:property [ sh:path ex:label ; ex:lang "de" , "fr" ] .
"#;
    let r = try_run(shapes, "ex:x ex:label \"x\"@en .");
    let err = r.expect_err("two values for one of several parameters is ill-formed");
    assert!(err.contains("$lang"), "{err}");
}

/// A sub-select must project every pre-bound variable, a component's
/// parameters included (SHACL Appendix A): one that does not would see the
/// parameter unbound. The shapes graph fails instead.
#[test]
fn a_sub_select_must_project_every_prebound_variable() {
    let component = |inner_projection: &str| {
        format!(
            r#"
ex:LimitComponent a sh:ConstraintComponent ;
  sh:parameter [ sh:path ex:limit ] ;
  sh:nodeValidator [ a sh:SPARQLSelectValidator ;
    sh:select """SELECT $this WHERE {{ {{ SELECT {inner_projection} WHERE {{ $this <http://example.org/size> ?s . FILTER (?s > $limit) }} }} }}""" ] .
ex:S a sh:NodeShape ; sh:targetSubjectsOf ex:size ; ex:limit 10 .
"#
        )
    };
    let err = try_run(&component("$this"), "ex:big ex:size 20 .")
        .expect_err("a sub-select hiding $limit is refused");
    assert!(err.contains("$limit"), "{err}");

    let r = try_run(
        &component("$this $limit"),
        "ex:big ex:size 20 . ex:ok ex:size 5 .",
    )
    .expect("projecting every pre-bound variable is fine");
    assert_eq!(flagged(&r), set(&["big"]), "{:?}", r.results);
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
    // The well-formed target still selects its focus nodes. An unqualified
    // pattern is the right form: the run's data graphs are injected as the
    // query's `FROM` prologue, so they are its default graph. A `GRAPH` block
    // would match nothing, because no `FROM NAMED` is injected — the same rule
    // a `sh:sparql` constraint follows.
    let shapes = "ex:S a sh:NodeShape ; sh:target [ sh:select \"SELECT ?this WHERE { ?this a <http://example.org/T> }\" ] ; sh:property [ sh:path ex:p ; sh:minCount 1 ] .";
    let r = run(shapes, "ex:a a ex:T .");
    assert!(violates(&r, "/a"), "{:?}", r.results);
}

/// A SHACL-AF SPARQL target must not see graphs outside the run's data graphs.
/// It used to run against the bare store, so a `sh:target` in any shapes graph
/// the caller could write selected focus nodes from every graph in the store —
/// another tenant's included — and `sh:value` carried their terms back.
#[test]
fn a_sparql_target_cannot_reach_outside_the_runs_data_graphs() {
    let store = TripleStore::in_memory().unwrap();
    let shapes = "ex:S a sh:NodeShape ; sh:target [ sh:select \"SELECT ?this WHERE { ?this a <http://example.org/T> }\" ] ; sh:property [ sh:path ex:secret ; sh:maxCount 0 ] .";
    store
        .load_str(
            &format!("{PFX}{shapes}"),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    store
        .load_str(
            &format!("{PFX}ex:mine a ex:T ; ex:secret \"mine\" ."),
            RdfFormat::Turtle,
            Some("urn:data"),
        )
        .unwrap();
    // Another tenant's graph, not in this run's data graphs.
    store
        .load_str(
            &format!("{PFX}ex:theirs a ex:T ; ex:secret \"classified\" ."),
            RdfFormat::Turtle,
            Some("urn:other-tenant"),
        )
        .unwrap();

    let r = validate(&store, "urn:shapes", &["urn:data".to_string()]).unwrap();
    assert!(
        violates(&r, "/mine"),
        "the in-scope node is still targeted: {:?}",
        r.results
    );
    assert!(
        !r.results.iter().any(|x| x.focus_node.contains("theirs")),
        "a target must not select focus nodes from a graph outside the run: {:?}",
        r.results
    );
    assert!(
        !r.results
            .iter()
            .any(|x| x.value.as_deref() == Some("classified")),
        "no out-of-scope value may reach the report: {:?}",
        r.results
    );
}

// ─── Graph reach: the class hierarchy is read across the run's data graphs ────

/// `sh:targetClass` and `sh:class` are the same specification relation —
/// "SHACL instance of C in the data graph" (§2.1.3.2 / §4.1.1) — evaluated at
/// two moments, so they must agree. The subclass chain used to be read per
/// graph for targets and across all graphs for `sh:class`, so a dataset that
/// keeps its model in one graph and its instances in another had
/// `sh:targetClass ex:Asset` target nothing while `sh:class ex:Asset` held.
/// Nothing in the repository separated a subclass axiom from its type triple,
/// which is why the disagreement was never observed.
#[test]
fn a_subclass_axiom_in_another_graph_still_targets_its_instances() {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            &format!("{PFX}ex:S a sh:NodeShape ; sh:targetClass ex:Asset ; sh:property [ sh:path ex:name ; sh:minCount 1 ] ."),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    // The model layer: only the subclass axiom.
    store
        .load_str(
            &format!("{PFX}ex:Bridge rdfs:subClassOf ex:Asset ."),
            RdfFormat::Turtle,
            Some("urn:model"),
        )
        .unwrap();
    // The instance layer: only the type triple. No name, so the shape bites.
    store
        .load_str(
            &format!("{PFX}ex:b1 a ex:Bridge ."),
            RdfFormat::Turtle,
            Some("urn:instances"),
        )
        .unwrap();

    let r = validate(
        &store,
        "urn:shapes",
        &["urn:instances".to_string(), "urn:model".to_string()],
    )
    .unwrap();
    assert!(
        violates(&r, "/b1"),
        "a subclass axiom in a sibling data graph must still make ex:b1 a target of ex:Asset: {:?}",
        r.results
    );

    // Control: with the model graph out of scope there is no chain to walk and
    // nothing is targeted. That is a scoping question, not a reach one.
    let out_of_scope = validate(&store, "urn:shapes", &["urn:instances".to_string()]).unwrap();
    assert!(
        out_of_scope.conforms,
        "without the model graph in scope the superclass target has no instances: {:?}",
        out_of_scope.results
    );
}

/// The same rule written two ways gives one answer. SHACL validates one data
/// graph (§3.4), so a run over several validates their merge. A `sh:path` used
/// to be evaluated inside each data graph in turn: a path that had to cross a
/// graph boundary found nothing, while the identical rule as a `sh:sparql`
/// constraint read the graphs merged and found the value.
#[test]
fn a_path_and_an_equivalent_sparql_constraint_agree_across_graphs() {
    let store = TripleStore::in_memory().unwrap();
    let shapes = r#"
ex:PathShape a sh:NodeShape ; sh:targetNode ex:bridge1 ;
  sh:property [ sh:path ( ex:hasDeck ex:width ) ; sh:minCount 1 ;
                sh:message "path: no deck width" ] .
ex:SparqlShape a sh:NodeShape ; sh:targetNode ex:bridge1 ;
  sh:sparql [ sh:select """SELECT $this WHERE { FILTER NOT EXISTS { $this <http://example.org/hasDeck>/<http://example.org/width> ?w } }""" ;
              sh:message "sparql: no deck width" ] .
"#;
    store
        .load_str(
            &format!("{PFX}{shapes}"),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    store
        .load_str(
            &format!("{PFX}ex:bridge1 a ex:Bridge ; ex:hasDeck ex:deck1 ."),
            RdfFormat::Turtle,
            Some("urn:instances"),
        )
        .unwrap();
    store
        .load_str(
            &format!("{PFX}ex:deck1 ex:width 12 ."),
            RdfFormat::Turtle,
            Some("urn:details"),
        )
        .unwrap();

    let r = validate(
        &store,
        "urn:shapes",
        &["urn:instances".to_string(), "urn:details".to_string()],
    )
    .unwrap();
    assert!(
        r.conforms,
        "both spellings follow ex:hasDeck into urn:instances and ex:width into urn:details: {:?}",
        r.results
    );

    // Without the deck's width anywhere, both spellings report it.
    let missing = TripleStore::in_memory().unwrap();
    missing
        .load_str(
            &format!("{PFX}{shapes}"),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    missing
        .load_str(
            &format!("{PFX}ex:bridge1 a ex:Bridge ; ex:hasDeck ex:deck1 ."),
            RdfFormat::Turtle,
            Some("urn:instances"),
        )
        .unwrap();
    missing
        .load_str(
            &format!("{PFX}ex:deck1 ex:length 40 ."),
            RdfFormat::Turtle,
            Some("urn:details"),
        )
        .unwrap();
    let r = validate(
        &missing,
        "urn:shapes",
        &["urn:instances".to_string(), "urn:details".to_string()],
    )
    .unwrap();
    assert!(
        r.results.iter().any(|x| x.message.starts_with("path:")),
        "{:?}",
        r.results
    );
    assert!(
        r.results.iter().any(|x| x.message.starts_with("sparql:")),
        "{:?}",
        r.results
    );

    // Control: in a single graph the two spellings agree too.
    let single = TripleStore::in_memory().unwrap();
    single
        .load_str(
            &format!("{PFX}{shapes}"),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    single
        .load_str(
            &format!("{PFX}ex:bridge1 a ex:Bridge ; ex:hasDeck ex:deck1 . ex:deck1 ex:width 12 ."),
            RdfFormat::Turtle,
            Some("urn:data"),
        )
        .unwrap();
    let r1 = validate(&single, "urn:shapes", &["urn:data".to_string()]).unwrap();
    assert!(
        r1.conforms,
        "in one graph both spellings find the width: {:?}",
        r1.results
    );
}

/// A recursive shapes graph must still validate. SHACL leaves recursive shapes
/// undefined (§3.4.3) and this engine bounds its loader at
/// `MAX_SHAPE_LOAD_DEPTH`; hitting that bound drops the member being loaded,
/// it does not fail the run. Making every inline-load error fail the shapes
/// graph briefly made a legitimate `sh:node` cycle unvalidatable — and, through
/// the write gate, made every write to such a dataset a 422.
#[test]
fn a_recursive_shapes_graph_still_validates() {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            &format!(
                "{PFX}ex:A a sh:NodeShape ; sh:targetClass ex:T ; sh:node ex:B ; \
                 sh:property [ sh:path ex:name ; sh:minCount 1 ] . \
                 ex:B a sh:NodeShape ; sh:node ex:A ."
            ),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    store
        .load_str(
            &format!("{PFX}ex:x a ex:T ."),
            RdfFormat::Turtle,
            Some("urn:data"),
        )
        .unwrap();

    let r = validate(&store, "urn:shapes", &["urn:data".to_string()])
        .expect("a recursive shapes graph must return a report, not an error");
    assert!(
        violates(&r, "/x"),
        "the non-recursive constraints of the shape are still enforced: {:?}",
        r.results
    );
}

/// Splitting the data across graphs cannot change what `sh:closed` reports:
/// it enumerates the focus node's outgoing quads in a SINGLE hop over the
/// merge of the data graphs, and every matching quad lives in exactly one
/// graph. The same argument covers `sh:targetSubjectsOf` and
/// `sh:targetObjectsOf`. This test exists so the equivalence is asserted
/// rather than argued.
#[test]
fn sh_closed_reports_the_same_across_graphs_as_within_them() {
    let shapes = "ex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:closed true ; \
                  sh:property [ sh:path ex:allowed ] .";
    let split = TripleStore::in_memory().unwrap();
    split
        .load_str(
            &format!("{PFX}{shapes}"),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    split
        .load_str(
            &format!("{PFX}ex:n ex:allowed 1 ; ex:stray \"a\" ."),
            RdfFormat::Turtle,
            Some("urn:g1"),
        )
        .unwrap();
    split
        .load_str(
            &format!("{PFX}ex:n ex:other \"b\" ."),
            RdfFormat::Turtle,
            Some("urn:g2"),
        )
        .unwrap();
    let across = validate(
        &split,
        "urn:shapes",
        &["urn:g1".to_string(), "urn:g2".to_string()],
    )
    .unwrap();

    // The same triples in one graph.
    let merged = run(
        shapes,
        "ex:n ex:allowed 1 ; ex:stray \"a\" ; ex:other \"b\" .",
    );

    let key = |r: &ValidationReport| {
        let mut v: Vec<String> = r
            .results
            .iter()
            .map(|x| format!("{}|{:?}|{:?}", x.focus_node, x.path, x.value))
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        key(&across),
        key(&merged),
        "sh:closed is a single hop, so splitting the data across graphs cannot change its report"
    );
    assert_eq!(across.results_count, 2, "ex:stray and ex:other: {across:?}");
}

/// A single-hop path reads the same split or merged, for the same reason.
#[test]
fn a_single_hop_path_reads_the_same_across_graphs_as_within_them() {
    let shapes =
        "ex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:property [ sh:path ex:p ; sh:minCount 3 ] .";
    let split = TripleStore::in_memory().unwrap();
    split
        .load_str(
            &format!("{PFX}{shapes}"),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    split
        .load_str(
            &format!("{PFX}ex:n ex:p 1, 2 ."),
            RdfFormat::Turtle,
            Some("urn:g1"),
        )
        .unwrap();
    split
        .load_str(
            &format!("{PFX}ex:n ex:p 3 ."),
            RdfFormat::Turtle,
            Some("urn:g2"),
        )
        .unwrap();
    let across = validate(
        &split,
        "urn:shapes",
        &["urn:g1".to_string(), "urn:g2".to_string()],
    )
    .unwrap();
    assert!(
        across.conforms,
        "three values spread over two graphs still satisfy minCount 3: {:?}",
        across.results
    );
}

/// The reach of a composite path does not depend on the kind of the focus
/// node. An IRI focus used to be walked inside each data graph in turn while a
/// blank-node focus took the merged walk, so the same path over structurally
/// identical data answered differently for `ex:iri` and for a blank node.
#[test]
fn a_composite_path_reaches_the_same_for_an_iri_and_a_blank_node_focus() {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            &format!(
                "{PFX}ex:S a sh:NodeShape ; sh:targetSubjectsOf ex:hasDeck ; \
                 sh:property [ sh:path ( ex:hasDeck ex:width ) ; sh:minCount 1 ] ."
            ),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    store
        .load_str(
            &format!("{PFX}ex:iri ex:hasDeck ex:deck1 . [] ex:hasDeck ex:deck2 ."),
            RdfFormat::Turtle,
            Some("urn:instances"),
        )
        .unwrap();
    store
        .load_str(
            &format!("{PFX}ex:deck1 ex:width 12 . ex:deck2 ex:width 12 ."),
            RdfFormat::Turtle,
            Some("urn:details"),
        )
        .unwrap();

    let r = validate(
        &store,
        "urn:shapes",
        &["urn:instances".to_string(), "urn:details".to_string()],
    )
    .unwrap();
    assert!(
        r.conforms,
        "both focus nodes reach their deck's width in the other graph: {:?}",
        r.results
    );
}

/// A closure path follows its chain through every data graph, and a value
/// count adds up values from all of them: `sh:maxCount` sees both widths.
#[test]
fn closure_and_sequence_paths_cross_data_graphs() {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            &format!(
                "{PFX}ex:Root a sh:NodeShape ; sh:targetNode ex:leaf ; \
                 sh:property [ sh:path [ sh:oneOrMorePath ex:partOf ] ; sh:hasValue ex:root ] . \
                 ex:Width a sh:NodeShape ; sh:targetNode ex:bridge ; \
                 sh:property [ sh:path ( ex:hasDeck ex:width ) ; sh:maxCount 1 ] ."
            ),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    store
        .load_str(
            &format!("{PFX}ex:leaf ex:partOf ex:mid . ex:bridge ex:hasDeck ex:d1 , ex:d2 ."),
            RdfFormat::Turtle,
            Some("urn:g1"),
        )
        .unwrap();
    store
        .load_str(
            &format!("{PFX}ex:mid ex:partOf ex:root . ex:d1 ex:width 12 ."),
            RdfFormat::Turtle,
            Some("urn:g2"),
        )
        .unwrap();
    store
        .load_str(
            &format!("{PFX}ex:d2 ex:width 14 ."),
            RdfFormat::Turtle,
            Some("urn:g3"),
        )
        .unwrap();
    let r = validate(
        &store,
        "urn:shapes",
        &[
            "urn:g1".to_string(),
            "urn:g2".to_string(),
            "urn:g3".to_string(),
        ],
    )
    .unwrap();
    assert!(
        !violates(&r, "/leaf"),
        "ex:leaf reaches ex:root through ex:mid across graphs: {:?}",
        r.results
    );
    assert!(
        violates(&r, "/bridge"),
        "two deck widths, one in each of two graphs, exceed maxCount 1: {:?}",
        r.results
    );
}

// ─── sh:SPARQLFunction scope ───────────────────────────────────────────────
//
// A `sh:SPARQLFunction` belongs to the runs of the shapes graph that declares
// it. It never reaches another shapes graph's run, and it can never redefine
// an `xsd:` cast or a function the server registers itself (GeoSPARQL, 3D,
// RDF 1.2, ADJUST): a writer of any graph could otherwise change what every
// other tenant's constraints, gates and pipelines compute.

/// A function definition with one parameter `$x`.
fn sparql_function(iri: &str, select: &str) -> String {
    format!(
        "<{iri}> a sh:SPARQLFunction ;\n\
           sh:parameter [ sh:path ex:x ; sh:order 0 ] ;\n\
           sh:select \"\"\"{select}\"\"\" .\n"
    )
}

const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";

/// `ex:big` violates when its value, cast with the built-in `xsd:integer`, is
/// over 5 — and it is 10.
fn cast_shapes() -> String {
    format!(
        "ex:CastShape a sh:NodeShape ; sh:targetNode ex:big ;\n\
           sh:sparql [ sh:select \"\"\"SELECT $this ?value WHERE {{ $this <http://example.org/v> ?value . FILTER(<{XSD_INTEGER}>(?value) > 5) }}\"\"\" ] .\n"
    )
}

fn load(store: &TripleStore, graph: &str, ttl: &str) {
    store
        .load_str(&format!("{PFX}{ttl}"), RdfFormat::Turtle, Some(graph))
        .unwrap();
}

/// Another graph in the store — any dataset's — defines `xsd:integer` to
/// always return 0. The shapes graph's own constraint still casts with the
/// built-in and still catches the violation.
#[test]
fn sparql_function_in_another_graph_cannot_redefine_a_cast() {
    let store = TripleStore::in_memory().unwrap();
    load(&store, "urn:shapes", &cast_shapes());
    load(&store, "urn:data", "ex:big ex:v \"10\" .");
    load(
        &store,
        "urn:attacker",
        &sparql_function(XSD_INTEGER, "SELECT (0 AS ?r) WHERE {}"),
    );
    let r = validate(&store, "urn:shapes", &["urn:data".to_string()]).unwrap();
    assert!(
        violates(&r, "/big"),
        "a graph outside the run redefined xsd:integer: {:?}",
        r.results
    );
}

/// A shapes graph that declares a function at a reserved IRI (an `xsd:`
/// cast, a registered GeoSPARQL function) fails its own run: the definition
/// would silently be ignored otherwise, and the author would never learn it.
#[test]
fn sparql_function_redefining_a_builtin_fails_the_run() {
    for reserved in [
        XSD_INTEGER,
        "http://www.opengis.net/def/function/geosparql/sfWithin",
        "http://www.w3.org/ns/sparql#adjust",
    ] {
        let store = TripleStore::in_memory().unwrap();
        load(
            &store,
            "urn:shapes",
            &format!(
                "{}{}",
                cast_shapes(),
                sparql_function(reserved, "SELECT (0 AS ?r) WHERE {}")
            ),
        );
        load(&store, "urn:data", "ex:big ex:v \"10\" .");
        match validate(&store, "urn:shapes", &["urn:data".to_string()]) {
            Err(e) => assert!(e.contains(reserved), "{reserved}: {e}"),
            Ok(r) => panic!(
                "{reserved} was redefined or ignored silently: {:?}",
                r.results
            ),
        }
    }
}

/// A result value is the N-Triples term: `"20"^^xsd:integer`.
fn is_twenty(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|v| v.starts_with("\"20\""))
}

/// Two shapes graphs: A declares `ex:double` and uses it; B uses it without
/// declaring it. A's run computes with it; B's run does not see it.
#[test]
fn sparql_function_is_scoped_to_its_shapes_graph() {
    let store = TripleStore::in_memory().unwrap();
    let uses_double = "SELECT $this ?value WHERE { $this <http://example.org/v> ?v . BIND(<http://example.org/double>(?v) AS ?value) FILTER(?value > 15) }";
    load(
        &store,
        "urn:shapes-a",
        &format!(
            "{}ex:A a sh:NodeShape ; sh:targetNode ex:big ; sh:sparql [ sh:select \"\"\"{uses_double}\"\"\" ] .\n",
            sparql_function("http://example.org/double", "SELECT ($x * 2 AS ?r) WHERE {}")
        ),
    );
    load(
        &store,
        "urn:shapes-b",
        &format!("ex:B a sh:NodeShape ; sh:targetNode ex:big ; sh:sparql [ sh:select \"\"\"{uses_double}\"\"\" ] .\n"),
    );
    load(&store, "urn:data", "ex:big ex:v 10 .");
    let data = ["urn:data".to_string()];

    let a = validate(&store, "urn:shapes-a", &data).unwrap();
    assert!(
        a.results
            .iter()
            .any(|x| x.focus_node.ends_with("/big") && is_twenty(&x.value)),
        "the declaring graph's run computes with its function: {:?}",
        a.results
    );

    let b = validate(&store, "urn:shapes-b", &data).unwrap();
    assert!(
        !b.results.iter().any(|x| is_twenty(&x.value)),
        "another shapes graph's function reached this run: {:?}",
        b.results
    );
    assert!(
        b.results
            .iter()
            .any(|x| x.focus_node.ends_with("/big") && x.message.contains("double")),
        "an undeclared function makes the constraint unevaluable, not a pass: {:?}",
        b.results
    );
}

// ─── Fail open: every value of a multi-valued parameter is a constraint ──────
//
// SHACL §4: when a component has a single parameter, "each value of such a
// parameter declares an individual constraint". The loader used to read only
// the first value of sh:not, sh:and, sh:or, sh:xone, sh:hasValue, sh:pattern
// and sh:qualifiedValueShape, so the others were never checked and a write
// gate let data through that one of them forbids. Which value came "first"
// depended on store order, so each test has one node per value: whichever
// value the old loader kept, another node still had to be flagged.

fn try_run(shapes: &str, data: &str) -> Result<ValidationReport, String> {
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
    validate(&store, "urn:shapes", &["urn:data".to_string()])
}

/// Focus nodes with at least one result, by local name.
fn flagged(r: &ValidationReport) -> std::collections::BTreeSet<String> {
    r.results
        .iter()
        .map(|v| {
            v.focus_node
                .trim_matches(|c| c == '<' || c == '>')
                .trim_start_matches("http://example.org/")
                .to_string()
        })
        .collect()
}

fn set(names: &[&str]) -> std::collections::BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn every_value_of_sh_not_is_its_own_constraint() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:T ;
  sh:not [ sh:class ex:A ], [ sh:class ex:B ] ."#;
    let data = r#"
ex:a a ex:T, ex:A .
ex:b a ex:T, ex:B .
ex:ok a ex:T ."#;
    let r = run(shapes, data);
    assert_eq!(flagged(&r), set(&["a", "b"]), "{:?}", r.results);
}

#[test]
fn every_list_of_sh_and_sh_or_sh_xone_is_its_own_constraint() {
    // sh:and — two lists, each with one member.
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetClass ex:T ;
           sh:and ( [ sh:class ex:A ] ), ( [ sh:class ex:B ] ) .",
        "ex:a a ex:T, ex:A . ex:b a ex:T, ex:B . ex:ok a ex:T, ex:A, ex:B .",
    );
    assert_eq!(flagged(&r), set(&["a", "b"]), "sh:and: {:?}", r.results);

    // sh:or — conforming to one member of the first list says nothing about
    // the second.
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetClass ex:T ;
           sh:or ( [ sh:class ex:A ] [ sh:class ex:B ] ), ( [ sh:class ex:C ] [ sh:class ex:D ] ) .",
        "ex:a a ex:T, ex:A . ex:c a ex:T, ex:C . ex:ok a ex:T, ex:B, ex:D .",
    );
    assert_eq!(flagged(&r), set(&["a", "c"]), "sh:or: {:?}", r.results);

    // sh:xone — exactly one member of EACH list.
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetClass ex:T ;
           sh:xone ( [ sh:class ex:A ] [ sh:class ex:B ] ), ( [ sh:class ex:C ] [ sh:class ex:D ] ) .",
        "ex:a a ex:T, ex:A . ex:c a ex:T, ex:C . ex:ok a ex:T, ex:A, ex:D .",
    );
    assert_eq!(flagged(&r), set(&["a", "c"]), "sh:xone: {:?}", r.results);
}

#[test]
fn every_value_of_sh_has_value_is_its_own_constraint() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:T ;
  sh:property [ sh:path ex:p ; sh:hasValue ex:x, "y" ] ."#;
    let data = r#"
ex:a a ex:T ; ex:p ex:x .
ex:b a ex:T ; ex:p "y" .
ex:ok a ex:T ; ex:p ex:x, "y" ."#;
    let r = run(shapes, data);
    assert_eq!(flagged(&r), set(&["a", "b"]), "{:?}", r.results);
}

#[test]
fn every_value_of_sh_pattern_is_its_own_constraint() {
    // Both patterns take the shape's one sh:flags.
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:T ;
  sh:property [ sh:path ex:p ; sh:pattern "^a", "z$" ; sh:flags "i" ] ."#;
    let data = r#"
ex:a a ex:T ; ex:p "Abc" .
ex:z a ex:T ; ex:p "xyZ" .
ex:ok a ex:T ; ex:p "AZ" ."#;
    let r = run(shapes, data);
    assert_eq!(flagged(&r), set(&["a", "z"]), "{:?}", r.results);
}

#[test]
fn every_value_of_sh_qualified_value_shape_is_its_own_constraint() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:T ;
  sh:property [ sh:path ex:p ;
      sh:qualifiedValueShape [ sh:class ex:A ], [ sh:class ex:B ] ;
      sh:qualifiedMinCount 1 ] ."#;
    let data = r#"
ex:va a ex:A . ex:vb a ex:B .
ex:a a ex:T ; ex:p ex:va .
ex:b a ex:T ; ex:p ex:vb .
ex:ok a ex:T ; ex:p ex:va, ex:vb ."#;
    let r = run(shapes, data);
    assert_eq!(flagged(&r), set(&["a", "b"]), "{:?}", r.results);
}

// ─── Fail open: sh:deactivated below the top level ───────────────────────────
//
// SHACL §2.1.6: "All RDF terms conform to a deactivated shape." That held only
// for top-level shapes. A deactivated property shape still produced results,
// and an inline shape under sh:node / sh:not / sh:or was evaluated as if
// active — so sh:not of a deactivated shape passed when it must fail.

#[test]
fn a_deactivated_property_shape_produces_no_results() {
    // Named (the spec's own example) and blank.
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Person ;
  sh:property ex:S-name ;
  sh:property [ sh:path ex:age ; sh:minCount 1 ; sh:deactivated true ] .
ex:S-name a sh:PropertyShape ; sh:path ex:name ; sh:minCount 1 ; sh:deactivated true ."#;
    let r = run(shapes, "ex:JohnDoe a ex:Person .");
    assert!(r.conforms, "{:?}", r.results);

    // A top-level property shape, and one nested under a property shape.
    let shapes = r#"
ex:P a sh:PropertyShape ; sh:targetClass ex:Person ; sh:path ex:name ;
  sh:minCount 1 ; sh:deactivated true .
ex:Q a sh:NodeShape ; sh:targetClass ex:Person ;
  sh:property [ sh:path ex:knows ;
      sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:deactivated true ] ] ."#;
    let r = run(shapes, "ex:JohnDoe a ex:Person ; ex:knows ex:Jane .");
    assert!(r.conforms, "{:?}", r.results);
}

#[test]
fn every_term_conforms_to_a_deactivated_inline_shape() {
    let data = "ex:a a ex:T .";
    // sh:node of a deactivated shape: nothing to report.
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetClass ex:T ;
           sh:node [ sh:class ex:Missing ; sh:deactivated true ] .",
        data,
    );
    assert!(r.conforms, "sh:node: {:?}", r.results);
    // sh:or with a deactivated member: that member always conforms.
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetClass ex:T ;
           sh:or ( [ sh:class ex:Missing ; sh:deactivated true ] [ sh:class ex:Other ] ) .",
        data,
    );
    assert!(r.conforms, "sh:or: {:?}", r.results);
    // sh:not of a deactivated shape: the node conforms to it, so sh:not fails.
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetClass ex:T ;
           sh:not [ sh:class ex:Missing ; sh:deactivated true ] .",
        data,
    );
    assert_eq!(flagged(&r), set(&["a"]), "sh:not: {:?}", r.results);
    // sh:deactivated false is the default: the shape stays active.
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetClass ex:T ;
           sh:node [ sh:class ex:Missing ; sh:deactivated false ] .",
        data,
    );
    assert_eq!(
        flagged(&r),
        set(&["a"]),
        "deactivated false: {:?}",
        r.results
    );
}

// ─── Fail closed: a property shape without a usable path ─────────────────────
//
// A property shape whose sh:path was missing or did not parse was skipped with
// a warning, while every other load error fails the run (the write gate turns
// it into 422). Skipping it made the shapes graph conform by omission.

#[test]
fn a_property_shape_without_a_usable_path_fails_the_shapes_graph() {
    for (what, shapes) in [
        (
            "no sh:path",
            "ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:minCount 1 ] .",
        ),
        (
            "a blank node that is no path",
            "ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path [ ex:foo ex:bar ] ; sh:minCount 1 ] .",
        ),
        (
            "a literal path",
            "ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path \"ex:p\" ; sh:minCount 1 ] .",
        ),
        (
            "two paths",
            "ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path ex:p, ex:q ; sh:minCount 1 ] .",
        ),
        (
            "a sequence with a member that is no path",
            "ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path ( ex:p [ ex:foo ex:bar ] ) ; sh:minCount 1 ] .",
        ),
        (
            "an alternative with a member that is no path",
            "ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path [ sh:alternativePath ( ex:p \"q\" ) ] ; sh:minCount 1 ] .",
        ),
        (
            "a top-level property shape with a path that is no path",
            "ex:S a sh:PropertyShape ; sh:targetClass ex:T ; sh:path [ ex:foo ex:bar ] ; sh:minCount 1 .",
        ),
        (
            "a top-level sh:PropertyShape with no path",
            "ex:S a sh:PropertyShape ; sh:targetClass ex:T ; sh:minCount 1 .",
        ),
        (
            "an inline shape with a path that is no path",
            "ex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:node [ sh:path [ ex:foo ex:bar ] ; sh:minCount 1 ] .",
        ),
    ] {
        let r = try_run(shapes, "ex:a a ex:T .");
        assert!(r.is_err(), "{what}: must fail the run, got {r:?}");
    }
    // Every well-formed path form still loads.
    let r = try_run(
        "ex:S a sh:NodeShape ; sh:targetClass ex:T ;
           sh:property [ sh:path ( ex:p [ sh:inversePath ex:q ] ) ] ;
           sh:property [ sh:path [ sh:alternativePath ( ex:p [ sh:zeroOrMorePath ex:q ] ) ] ] ;
           sh:property [ sh:path [ sh:oneOrMorePath ex:p ] ] ;
           sh:property [ sh:path [ sh:zeroOrOnePath ex:p ] ] ;
           sh:property [ sh:path ( ex:p ex:q ) ; sh:minCount 1 ] .",
        "ex:a a ex:T .",
    )
    .expect("well-formed paths load");
    assert_eq!(flagged(&r), set(&["a"]), "{:?}", r.results);
}

// ─── Fail closed: a SPARQL target that errors at run time ────────────────────

/// A target that parses and projects `?this` but fails when evaluated used to
/// yield no focus nodes (`if let Ok(..)`, and per-solution errors were dropped
/// too): the shape validated nothing and the gate let the write through.
#[test]
fn a_sparql_target_that_errors_at_run_time_fails_the_run() {
    let shapes = r#"
ex:S a sh:NodeShape ;
  sh:target [ sh:select "SELECT ?this WHERE { SERVICE <http://example.org/nowhere> { ?this ?p ?o } }" ] ;
  sh:property [ sh:path ex:p ; sh:minCount 1 ] ."#;
    let r = try_run(shapes, "ex:a a ex:T .");
    assert!(
        r.is_err(),
        "an erroring target must fail the run, got {r:?}"
    );
}

// ─── Literal forms: what the store keeps is what the engine sees ─────────────
//
// The store keeps every literal as written (vendor/README.md). Until it did,
// every derived integer type read back as xsd:integer and "1"^^xsd:boolean
// as true; the two tests below pinned that and are now flipped.

/// SHACL §4.1.2: a valid `"5"^^xsd:nonNegativeInteger` conforms to
/// `sh:datatype xsd:nonNegativeInteger`, because the store hands it back with
/// its datatype (it used to read back as `"5"^^xsd:integer`, and a write gate
/// answered 422 on valid data).
#[test]
fn a_derived_integer_type_conforms_to_its_own_sh_datatype() {
    for dt in [
        "nonNegativeInteger",
        "positiveInteger",
        "int",
        "short",
        "byte",
        "unsignedLong",
    ] {
        let shapes = format!(
            "ex:S a sh:NodeShape ; sh:targetNode ex:a ; sh:property [ sh:path ex:n ; sh:datatype xsd:{dt} ] ."
        );
        let r = run(&shapes, &format!("ex:a ex:n \"5\"^^xsd:{dt} ."));
        assert!(r.conforms, "xsd:{dt}: {:?}", r.results);
        // The stored term keeps the datatype; DATATYPE() in a query is the
        // value's type, xsd:integer, as SPARQL's value semantics have it.
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(
                &format!("{PFX}ex:a ex:n \"5\"^^xsd:{dt} ."),
                RdfFormat::Turtle,
                Some("urn:data"),
            )
            .unwrap();
        let Ok(oxigraph::sparql::QueryResults::Solutions(mut rows)) =
            store.query("SELECT ?o WHERE { GRAPH <urn:data> { ?s ?p ?o } }")
        else {
            panic!("query failed");
        };
        let o = rows.next().unwrap().unwrap().get("o").unwrap().to_string();
        assert_eq!(
            o,
            format!("\"5\"^^<http://www.w3.org/2001/XMLSchema#{dt}>"),
            "xsd:{dt} reads back as written"
        );
    }
    // xsd:dateTimeStamp keeps its datatype the same way.
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetNode ex:a ; sh:property [ sh:path ex:t ; sh:datatype xsd:dateTimeStamp ] .",
        "ex:a ex:t \"2026-10-01T12:00:00Z\"^^xsd:dateTimeStamp .",
    );
    assert!(r.conforms, "xsd:dateTimeStamp: {:?}", r.results);
    // xsd:integer itself is unaffected.
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetNode ex:a ; sh:property [ sh:path ex:n ; sh:datatype xsd:integer ] .",
        "ex:a ex:n 5 .",
    );
    assert!(r.conforms, "{:?}", r.results);
}

/// W3C core/property/uniqueLang-002: only the literal `true` activates a
/// flag. The store keeps `"1"^^xsd:boolean` as written, so it activates
/// neither `sh:uniqueLang` nor `sh:closed`, nor deactivates a shape (it used
/// to read back as `true` and do all three).
#[test]
fn a_non_canonical_true_activates_no_flag() {
    let r = run(
        "ex:S a sh:PropertyShape ; sh:targetNode ex:i ; sh:path ex:m ; sh:uniqueLang \"1\"^^xsd:boolean .",
        "ex:i ex:m \"HI\"@en, \"Hi\"@en .",
    );
    assert!(
        r.conforms,
        "\"1\" activates no sh:uniqueLang: {:?}",
        r.results
    );
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetNode ex:i ; sh:closed \"1\"^^xsd:boolean .",
        "ex:i ex:m 1 .",
    );
    assert!(r.conforms, "\"1\" closes no shape: {:?}", r.results);
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetNode ex:i ; sh:class ex:Missing ; sh:deactivated \"1\"^^xsd:boolean .",
        "ex:i ex:m 1 .",
    );
    assert!(!r.conforms, "\"1\" deactivates nothing");
    // The literal true does all three.
    for (shapes, conforms) in [
        ("ex:S a sh:PropertyShape ; sh:targetNode ex:i ; sh:path ex:m ; sh:uniqueLang true .", false),
        ("ex:S a sh:NodeShape ; sh:targetNode ex:i ; sh:closed true .", false),
        ("ex:S a sh:NodeShape ; sh:targetNode ex:i ; sh:class ex:Missing ; sh:deactivated true .", true),
    ] {
        let r = run(shapes, "ex:i ex:m \"HI\"@en, \"Hi\"@en .");
        assert_eq!(r.conforms, conforms, "{shapes}: {:?}", r.results);
    }
}

// ---------------------------------------------------------------------------
// Result fidelity: constraint-component IRIs and typed terms (SHACL §3.6)
// ---------------------------------------------------------------------------

const SH: &str = "http://www.w3.org/ns/shacl#";

/// Every result names the IRI of the constraint component that produced it,
/// next to the display label the UI groups by (which does not change).
#[test]
fn results_name_their_constraint_component() {
    let shapes = r#"
      ex:S a sh:NodeShape ; sh:targetNode ex:a ;
        sh:property [ sh:path ex:name ; sh:minCount 1 ] ;
        sh:property [ sh:path ex:age ; sh:datatype xsd:integer ; sh:maxInclusive 150 ] ;
        sh:property [ sh:path ex:tag ; sh:in ( "x" ) ; sh:pattern "^x$" ] ;
        sh:property [ sh:path ex:knows ;
                      sh:qualifiedValueShape [ sh:class ex:Person ] ; sh:qualifiedMaxCount 0 ] ;
        sh:sparql [ sh:select "SELECT $this WHERE { $this <http://example.org/flag> true }" ] ."#;
    let data = r#"
      ex:a ex:age "200"^^xsd:integer , "old" ; ex:tag "y" ; ex:knows ex:p ; ex:flag true .
      ex:p a ex:Person ."#;
    let r = run(shapes, data);
    let components: std::collections::BTreeSet<(String, String)> = r
        .results
        .iter()
        .map(|v| {
            (
                v.source_constraint_component
                    .strip_prefix(SH)
                    .unwrap_or(&v.source_constraint_component)
                    .to_string(),
                v.source_constraint.clone(),
            )
        })
        .collect();
    for (component, label) in [
        ("MinCountConstraintComponent", "sh:minCount 1"),
        (
            "DatatypeConstraintComponent",
            "sh:datatype <http://www.w3.org/2001/XMLSchema#integer>",
        ),
        ("MaxInclusiveConstraintComponent", "sh:maxInclusive 150"),
        ("InConstraintComponent", "sh:in"),
        ("PatternConstraintComponent", "sh:pattern \"^x$\""),
        (
            "QualifiedMaxCountConstraintComponent",
            "sh:qualifiedMaxCount 0",
        ),
        ("SPARQLConstraintComponent", "sh:SPARQLConstraint"),
    ] {
        assert!(
            components.contains(&(component.to_string(), label.to_string())),
            "missing ({component}, {label}) in {components:?}"
        );
    }
    // The JSON form carries the IRI and not the typed terms.
    let json = serde_json::to_value(&r.results[0]).unwrap();
    assert!(json["source_constraint_component"]
        .as_str()
        .is_some_and(|c| c.starts_with(SH)));
    assert!(json.get("terms").is_none(), "{json}");
    // A stored report from before the field existed still reads back.
    let mut old = json.clone();
    old.as_object_mut()
        .unwrap()
        .remove("source_constraint_component");
    let back: open_triplestore::shacl::report::ValidationResult =
        serde_json::from_value(old).unwrap();
    assert_eq!(back.source_constraint_component, "");
}

/// The RDF report keeps what the display strings drop: literal datatypes and
/// language tags, path structures, the `sh:sparql` node as
/// `sh:sourceConstraint`, and a custom `sh:severity` IRI.
#[test]
fn rdf_report_keeps_typed_terms() {
    use open_triplestore::shacl_studio::report_rdf::report_to_turtle;
    let shapes = r#"
      ex:Ages a sh:NodeShape ; sh:targetNode ex:a ; sh:severity ex:MySeverity ;
        sh:property [ sh:path ex:age ; sh:maxInclusive 150 ] .
      ex:Labels a sh:NodeShape ; sh:targetNode ex:a ;
        sh:property ex:Labels-label .
      ex:Labels-label sh:path ( [ sh:inversePath ex:child ] ex:label ) ; sh:languageIn ( "nl" ) .
      ex:Flags a sh:NodeShape ; sh:targetNode ex:a ; sh:sparql ex:Flags-sparql .
      ex:Flags-sparql sh:select "SELECT $this ?value WHERE { $this <http://example.org/flag> ?value }" ."#;
    let data = r#"
      ex:a ex:age 200 ; ex:flag "on"^^xsd:token .
      ex:parent ex:child ex:a ; ex:label "parent"@en ."#;
    let r = run(shapes, data);
    assert_eq!(r.results_count, 3, "{:#?}", r.results);
    // Display strings stay as they were.
    let age = r
        .results
        .iter()
        .find(|v| v.source_constraint.starts_with("sh:maxInclusive"))
        .unwrap();
    assert_eq!(age.value.as_deref(), Some("200"));
    assert_eq!(age.path.as_deref(), Some("<http://example.org/age>"));

    let ttl = report_to_turtle(&r, "urn:report#run-1");
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(&ttl, RdfFormat::Turtle, Some("urn:report"))
        .unwrap_or_else(|e| panic!("{e}\n{ttl}"));
    let ask = |pattern: &str| -> bool {
        let q = format!(
            "PREFIX ex: <http://example.org/> PREFIX sh: <{SH}> \
             PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> \
             PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> \
             ASK {{ GRAPH <urn:report> {{ ?res a sh:ValidationResult ; sh:focusNode ex:a . {pattern} }} }}"
        );
        matches!(
            store.query(&q).unwrap(),
            oxigraph::sparql::QueryResults::Boolean(true)
        )
    };
    assert!(
        ask(
            "?res sh:value 200 ; sh:resultPath ex:age ; sh:resultSeverity ex:MySeverity ; \
             sh:sourceConstraintComponent sh:MaxInclusiveConstraintComponent ."
        ),
        "{ttl}"
    );
    assert!(
        ask(
            "?res sh:value \"parent\"@en ; sh:sourceShape ex:Labels-label ; \
             sh:resultSeverity sh:Violation ; \
             sh:sourceConstraintComponent sh:LanguageInConstraintComponent ; \
             sh:resultPath ?p . ?p rdf:first [ sh:inversePath ex:child ] ; \
             rdf:rest ( ex:label ) ."
        ),
        "{ttl}"
    );
    assert!(
        ask(
            "?res sh:value \"on\"^^xsd:token ; sh:sourceShape ex:Flags ; \
             sh:sourceConstraint ex:Flags-sparql ; \
             sh:sourceConstraintComponent sh:SPARQLConstraintComponent ."
        ),
        "{ttl}"
    );
}

// ─── SHACL-AF: expression constraints (§7) ──────────────────────────────────
//
// `sh:expression` holds a node expression evaluated with each value node as
// the focus node; there is a result for every value node whose expression
// does not produce exactly `{ true }`.

/// After TopQuadrant's `expression/booleans-001`: `sh:expression sh:this`
/// passes `true` and flags `false`.
#[test]
fn expression_constraint_requires_exactly_true() {
    let r = run(
        "ex:S a sh:NodeShape ; sh:expression sh:this ; sh:targetNode true, false .",
        "",
    );
    assert!(!r.conforms);
    assert_eq!(r.results.len(), 1, "{:?}", r.results);
    assert_eq!(r.results[0].focus_node, "false");
    assert_eq!(
        r.results[0].value.as_deref(),
        Some("false"),
        "{:?}",
        r.results
    );
}

/// A function expression over a path: the clearance must be at least 9.10.
/// No value at all is an empty result, which is not `{ true }` either.
#[test]
fn expression_constraint_with_a_function_expression() {
    let shapes = r#"
ex:atLeast a sh:SPARQLFunction ;
  sh:parameter [ sh:path ex:value ; sh:order 1 ] ; sh:parameter [ sh:path ex:minimum ; sh:order 2 ] ;
  sh:returnType xsd:boolean ; sh:ask "ASK { FILTER ($value >= $minimum) }" .
ex:S a sh:NodeShape ; sh:targetClass ex:Bridge ;
  sh:expression [ sh:message "Clearance below 9.10 m" ; ex:atLeast ( [ sh:path ex:clearance ] 9.10 ) ] ."#;
    let data = r#"
ex:ok a ex:Bridge ; ex:clearance 9.50 .
ex:low a ex:Bridge ; ex:clearance 8.50 .
ex:none a ex:Bridge ."#;
    let r = run(shapes, data);
    assert!(!violates(&r, "/ok"), "{:?}", r.results);
    assert!(violates(&r, "/low"), "{:?}", r.results);
    assert!(violates(&r, "/none"), "{:?}", r.results);
    assert!(
        r.results
            .iter()
            .all(|x| x.message.contains("Clearance below")),
        "the expression's sh:message is the result message: {:?}",
        r.results
    );
}

/// The proprietary form this engine used to read — `sh:expression [ sh:path
/// P ; sh:minExclusive … ]`, a path with comparison constraints on the
/// expression node — is a standard path expression now (decision D6): its
/// outputs are the path's values, which are not `true`.
#[test]
fn the_former_path_comparison_expression_form_has_standard_semantics() {
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetNode ex:b ; sh:expression [ sh:path ex:h ; sh:minExclusive 9 ] .",
        "ex:b ex:h 10 .",
    );
    assert!(violates(&r, "/b"), "{:?}", r.results);
}

/// On a property shape the expression is evaluated with every value node as
/// the focus node.
#[test]
fn expression_on_a_property_shape_checks_each_value_node() {
    let r = run(
        "ex:S a sh:NodeShape ; sh:targetNode ex:t ; sh:property [ sh:path ex:flag ; sh:expression sh:this ] .",
        "ex:t ex:flag true, false .",
    );
    assert_eq!(r.results.len(), 1, "{:?}", r.results);
    assert_eq!(
        r.results[0].value.as_deref(),
        Some("false"),
        "{:?}",
        r.results
    );
}

// ─── SHACL-AF: custom targets (§3) ──────────────────────────────────────────

const BORN_IN: &str = r#"
ex:BornIn a sh:SPARQLTargetType ; rdfs:subClassOf sh:Target ;
  sh:parameter [ sh:path ex:country ] ;
  sh:select "SELECT ?this WHERE { ?this <http://example.org/bornIn> $country . }" .
"#;

/// A SPARQL-based target type (§3.2): the target's parameter values are
/// pre-bound in the type's query.
#[test]
fn sparql_target_type_prebinds_its_parameters() {
    let shapes = format!(
        "{BORN_IN}ex:S a sh:NodeShape ; sh:target [ a ex:BornIn ; ex:country ex:NL ] ;\n\
           sh:property [ sh:path ex:name ; sh:minCount 1 ] ."
    );
    let r = run(&shapes, "ex:a ex:bornIn ex:NL . ex:b ex:bornIn ex:US .");
    assert!(violates(&r, "/a"), "{:?}", r.results);
    assert!(!violates(&r, "/b"), "{:?}", r.results);
}

/// A target that lacks a value for a non-optional parameter produces no
/// target nodes (§3.2); a literal value is a term, not query text.
#[test]
fn sparql_target_type_without_its_parameter_targets_nothing() {
    for target in [
        "[ a ex:BornIn ]",
        r#"[ a ex:BornIn ; ex:country "x\" . } UNION { ?this ?p ?o" ]"#,
    ] {
        let shapes = format!(
            "{BORN_IN}ex:S a sh:NodeShape ; sh:target {target} ;\n\
               sh:property [ sh:path ex:name ; sh:minCount 1 ] ."
        );
        let r = run(&shapes, "ex:a ex:bornIn ex:NL .");
        assert!(r.conforms, "{target}: {:?}", r.results);
    }
}

/// `sh:target` makes its subject a shape (§3) even with no `rdf:type
/// sh:NodeShape` and no other target or property.
#[test]
fn a_shape_whose_only_target_is_sh_target_is_discovered() {
    let shapes = r#"
ex:S sh:target [ a sh:SPARQLTarget ; sh:select "SELECT ?this WHERE { ?this a <http://example.org/Thing> }" ] ;
  sh:class ex:Named ."#;
    let r = run(shapes, "ex:t a ex:Thing .");
    assert!(violates(&r, "/t"), "{:?}", r.results);
}

// ─── SHACL-AF: function bodies read the run's data graphs ───────────────────

/// A constraint calls a function whose body counts labels: it sees the run's
/// data graph (two labels), not another graph in the store (five more), and a
/// `GRAPH` block in the body reaches nothing.
#[test]
fn sparql_function_body_in_a_constraint_reads_the_run_data_graphs() {
    let store = TripleStore::in_memory().unwrap();
    load(
        &store,
        "urn:shapes",
        r#"
ex:countLabels a sh:SPARQLFunction ;
  sh:select "SELECT (COUNT(?l) AS ?r) WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l }" .
ex:anyGraph a sh:SPARQLFunction ;
  sh:select "SELECT (COUNT(?l) AS ?r) WHERE { GRAPH ?g { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l } }" .
ex:S a sh:NodeShape ; sh:targetNode ex:t ;
  sh:sparql [ sh:select "SELECT $this ?value WHERE { BIND (<http://example.org/countLabels>() AS ?value) }" ] ;
  sh:sparql [ sh:select "SELECT $this ?value WHERE { BIND (<http://example.org/anyGraph>() AS ?value) FILTER (?value > 0) }" ] ."#,
    );
    load(&store, "urn:data", r#"ex:t rdfs:label "a", "b" ."#);
    load(
        &store,
        "urn:other",
        r#"ex:u rdfs:label "c", "d", "e", "f", "g" ."#,
    );
    let r = validate(&store, "urn:shapes", &["urn:data".to_string()]).unwrap();
    let values: Vec<_> = r.results.iter().filter_map(|x| x.value.clone()).collect();
    assert_eq!(values.len(), 1, "{:?}", r.results);
    assert!(values[0].starts_with("\"2\""), "{:?}", r.results);
}

// ─── Literals as written (the store keeps lexical forms and datatypes) ────────

/// SHACL §4.1.2: `sh:datatype` holds for a literal whose datatype is the given
/// IRI and whose lexical form is valid for it. The store keeps the derived
/// integer types and `xsd:dateTimeStamp` as written, so valid data of those
/// types conforms, and their range and time-zone rules are checked.
#[test]
fn sh_datatype_holds_for_derived_types_as_stored() {
    let shapes = r#"
      ex:S a sh:NodeShape ; sh:targetClass ex:Item ;
        sh:property [ sh:path ex:count ; sh:datatype xsd:nonNegativeInteger ] ;
        sh:property [ sh:path ex:n ; sh:datatype xsd:int ] ;
        sh:property [ sh:path ex:small ; sh:datatype xsd:byte ] ;
        sh:property [ sh:path ex:at ; sh:datatype xsd:dateTimeStamp ] ."#;
    let data = r#"
      ex:ok a ex:Item ; ex:count "5"^^xsd:nonNegativeInteger ; ex:n "-7"^^xsd:int ;
        ex:small "12"^^xsd:byte ; ex:at "2020-01-01T00:00:00+02:00"^^xsd:dateTimeStamp .
      ex:integer a ex:Item ; ex:count 5 .
      ex:negative a ex:Item ; ex:count "-5"^^xsd:nonNegativeInteger .
      ex:range a ex:Item ; ex:small "300"^^xsd:byte .
      ex:notz a ex:Item ; ex:at "2020-01-01T00:00:00"^^xsd:dateTimeStamp ."#;
    let r = run(shapes, data);
    assert!(
        !violates(&r, "/ok"),
        "valid derived types conform: {:?}",
        r.results
    );
    for bad in ["/integer", "/negative", "/range", "/notz"] {
        assert!(
            violates(&r, bad),
            "{bad} violates sh:datatype: {:?}",
            r.results
        );
    }
}

/// `sh:hasValue` and `sh:in` compare RDF terms: the literal as written, not
/// its value (`"05"^^xsd:integer` is not `5`, `"5"^^xsd:int` is not `5`).
#[test]
fn sh_has_value_and_sh_in_compare_terms_as_written() {
    let shapes = r#"
      ex:H a sh:NodeShape ; sh:targetClass ex:Item ;
        sh:property [ sh:path ex:v ; sh:hasValue 5 ] ;
        sh:property [ sh:path ex:w ; sh:in ( "05"^^xsd:integer ) ] ."#;
    let data = r#"
      ex:exact a ex:Item ; ex:v 5 ; ex:w "05"^^xsd:integer .
      ex:padded a ex:Item ; ex:v "05"^^xsd:integer ; ex:w 5 .
      ex:derived a ex:Item ; ex:v "5"^^xsd:int ; ex:w "05"^^xsd:integer ."#;
    let r = run(shapes, data);
    assert!(!violates(&r, "/exact"), "{:?}", r.results);
    assert!(violates(&r, "/padded"), "{:?}", r.results);
    assert!(violates(&r, "/derived"), "{:?}", r.results);
}

/// SHACL §2.1.3.3: a shape that is a SHACL instance of `rdfs:Class` targets
/// its instances, also through `rdfs:subClassOf` in the shapes graph.
#[test]
fn an_implicit_class_target_follows_subclass_of_rdfs_class() {
    let shapes = r#"
      ex:MyClass rdfs:subClassOf rdfs:Class .
      ex:Person a ex:MyClass , sh:NodeShape ;
        sh:property [ sh:path ex:name ; sh:minCount 1 ] ."#;
    let r = run(shapes, "ex:p a ex:Person .");
    assert!(violates(&r, "/p"), "{:?}", r.results);
}

/// A literal focus node is pre-bound as the term it is: `$this` =
/// `"5"^^xsd:int` (kept as written) finds the triple that holds it, so the
/// `sh:sparql` constraint reports it. Bound through its value instead, it
/// would be `5`, find nothing and let the node pass.
#[test]
fn a_sparql_constraint_sees_a_literal_focus_node_as_written() {
    let shapes = r#"
      ex:S a sh:NodeShape ; sh:targetObjectsOf ex:code ;
        sh:sparql [ sh:select """
          SELECT $this WHERE { <http://example.org/a> <http://example.org/code> $this }
        """ ] ."#;
    let r = run(
        shapes,
        r#"ex:a ex:code "5"^^xsd:int , "05"^^xsd:integer . ex:b ex:code 7 ."#,
    );
    let flagged: Vec<&str> = r.results.iter().map(|v| v.focus_node.as_str()).collect();
    assert_eq!(
        flagged.len(),
        2,
        "both of ex:a's codes violate: {flagged:?}"
    );
}
