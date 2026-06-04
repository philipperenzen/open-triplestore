//! W3C RDF 1.1 Conformance Tests
//!
//! Tests derived from:
//! - https://github.com/w3c/rdf-tests
//! - https://w3c.github.io/rdf-tests/
//! - W3C RDF 1.1 Concepts (https://www.w3.org/TR/rdf11-concepts/)
//! - W3C Turtle test suite (https://w3c.github.io/rdf-tests/rdf/rdf11/rdf-turtle/)
//! - W3C N-Triples test suite
//! - W3C N-Quads test suite
//! - W3C TriG test suite
//! - W3C RDF/XML test suite
//!
//! Each test corresponds to a W3C conformance test class:
//! - rdft:TestTurtlePositiveSyntax  — valid Turtle that must parse
//! - rdft:TestTurtleNegativeSyntax  — invalid Turtle that must fail
//! - rdft:TestTurtleEval            — parse and check resulting triples
//! - rdft:TestNTriplesPositiveSyntax
//! - rdft:TestNTriplesNegativeSyntax
//! - rdft:TestNQuadsPositiveSyntax / Eval
//! - rdft:TestTrigPositiveSyntax / Eval

use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn ts() -> open_triplestore::store::TripleStore {
    open_triplestore::store::TripleStore::in_memory().unwrap()
}

fn count_triples(s: &open_triplestore::store::TripleStore) -> usize {
    s.len().unwrap()
}

fn ask(s: &open_triplestore::store::TripleStore, q: &str) -> bool {
    match s.query(q).unwrap() {
        QueryResults::Boolean(b) => b,
        _ => panic!("Expected ASK"),
    }
}

fn load_ok(s: &open_triplestore::store::TripleStore, data: &str, fmt: RdfFormat) {
    s.load_str(data, fmt, None).expect("Expected valid RDF to parse");
}

fn load_err(data: &str, fmt: RdfFormat) {
    let s = ts();
    assert!(
        s.load_str(data, fmt, None).is_err(),
        "Expected parse error for: {:?}",
        data
    );
}

// ═══════════════════════════════════════════════════════════
// Turtle Positive Syntax Tests
// W3C: rdf-turtle/manifest.ttl — rdft:TestTurtlePositiveSyntax
// ═══════════════════════════════════════════════════════════

#[test]
fn rdf_turtle_pos_base_directive() {
    // turtle-syntax-base-01..04
    let s = ts();
    load_ok(
        &s,
        "@base <http://example.org/> . <s> <p> <o> .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_turtle_pos_prefix_directive() {
    // turtle-syntax-prefix-01..08
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p ex:o .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_turtle_pos_string_escape() {
    // turtle-syntax-string-01..11
    let s = ts();
    load_ok(
        &s,
        r#"@prefix ex: <http://example.org/> . ex:s ex:p "hello\nworld" ."#,
        RdfFormat::Turtle,
    );
    load_ok(
        &s,
        r#"@prefix ex: <http://example.org/> . ex:s ex:p "tab\there" ."#,
        RdfFormat::Turtle,
    );
    load_ok(
        &s,
        r#"@prefix ex: <http://example.org/> . ex:s ex:p "backslash\\" ."#,
        RdfFormat::Turtle,
    );
}

#[test]
fn rdf_turtle_pos_multiline_string() {
    // turtle-syntax-string-07
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p \"\"\"multi\nline\nstring\"\"\" .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_turtle_pos_long_string() {
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p '''single quote long''' .",
        RdfFormat::Turtle,
    );
}

#[test]
fn rdf_turtle_pos_blank_node_label() {
    // turtle-syntax-blank-label-01
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . _:b1 ex:p ex:o .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_turtle_pos_blank_node_property_list() {
    // turtle-syntax-bnode-01..03
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p [ ex:q ex:r ] .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 2);
}

#[test]
fn rdf_turtle_pos_collection() {
    // turtle-syntax-list-01..04: RDF collection (rdf:first, rdf:rest)
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p (1 2 3) .",
        RdfFormat::Turtle,
    );
    // An n-element list generates 2n + 1 triples (n first + n rest + 1 nil)
    assert!(count_triples(&s) >= 6);
}

#[test]
fn rdf_turtle_pos_anonymous_blank_node() {
    // [ ] as subject
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . [] ex:p ex:o .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_turtle_pos_numeric_literals() {
    // turtle-syntax-number-01..11
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p 42 . ex:s ex:q 3.14 . ex:s ex:r 1.0e10 . ex:s ex:t -5 . ex:s ex:u +3.5 .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 5);
}

#[test]
fn rdf_turtle_pos_boolean_literals() {
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p true . ex:s ex:q false .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 2);
}

#[test]
fn rdf_turtle_pos_datatype() {
    // turtle-syntax-datatypes-01..02
    let s = ts();
    load_ok(
        &s,
        "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> . @prefix ex: <http://example.org/> . ex:s ex:p \"2024-01-15\"^^xsd:date .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_turtle_pos_language_tag() {
    // turtle-syntax-ln-dots: language tags
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p \"hello\"@en . ex:s ex:p \"bonjour\"@fr .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 2);
}

#[test]
fn rdf_turtle_pos_bcp47_language_tag() {
    // BCP 47 language tags (subtags, scripts)
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p \"hello\"@en-US . ex:s ex:q \"hello\"@zh-Hant-TW .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 2);
}

#[test]
fn rdf_turtle_pos_semicolon_predicate_list() {
    // Predicate list with semicolon shorthand
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p ex:o ; ex:q ex:r ; ex:t ex:u .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 3);
}

#[test]
fn rdf_turtle_pos_comma_object_list() {
    // Multiple objects with comma shorthand
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p ex:a , ex:b , ex:c .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 3);
}

#[test]
fn rdf_turtle_pos_unicode_iri() {
    // IRIs with Unicode percent-encoding
    let s = ts();
    load_ok(
        &s,
        "<http://example.org/\u{00E9}l\u{00E8}ve> <http://example.org/name> \"student\" .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_turtle_pos_relative_iri_with_base() {
    let s = ts();
    load_ok(
        &s,
        "@base <http://example.org/base/> . <subject> <predicate> <object> .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 1);
    // Verify the IRI was resolved against base
    assert!(ask(
        &s,
        "ASK { <http://example.org/base/subject> <http://example.org/base/predicate> <http://example.org/base/object> }"
    ));
}

#[test]
fn rdf_turtle_pos_prefix_in_datatype() {
    let s = ts();
    load_ok(
        &s,
        "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> . @prefix ex: <http://example.org/> . ex:s ex:p \"42\"^^xsd:integer .",
        RdfFormat::Turtle,
    );
    assert!(ask(
        &s,
        "ASK { ?s ?p 42 }"
    ));
}

#[test]
fn rdf_turtle_pos_nested_blank_nodes() {
    // Deeply nested blank node property lists
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p [ ex:q [ ex:r ex:v ] ] .",
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 3);
}

#[test]
fn rdf_turtle_pos_rdf_type_shorthand() {
    // 'a' as shorthand for rdf:type
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:alice a ex:Person .",
        RdfFormat::Turtle,
    );
    assert!(ask(
        &s,
        "ASK { <http://example.org/alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Person> }"
    ));
}

// ═══════════════════════════════════════════════════════════
// Turtle Negative Syntax Tests
// W3C: rdft:TestTurtleNegativeSyntax
// ═══════════════════════════════════════════════════════════

#[test]
fn rdf_turtle_neg_missing_dot() {
    // turtle-syntax-bad-01: missing terminal dot
    load_err(
        "@prefix ex: <http://example.org/> . ex:s ex:p ex:o",
        RdfFormat::Turtle,
    );
}

#[test]
fn rdf_turtle_neg_bad_iri() {
    // turtle-syntax-bad-02: bad IRI with space
    load_err(
        "<http://example.org/bad iri> <http://example.org/p> <http://example.org/o> .",
        RdfFormat::Turtle,
    );
}

#[test]
fn rdf_turtle_neg_bad_literal_datatype() {
    // Missing closing quote
    load_err(
        "@prefix ex: <http://example.org/> . ex:s ex:p \"unclosed .",
        RdfFormat::Turtle,
    );
}

#[test]
fn rdf_turtle_neg_bad_prefix() {
    // Using undefined prefix
    load_err(
        "undefined:s undefined:p undefined:o .",
        RdfFormat::Turtle,
    );
}

#[test]
fn rdf_turtle_neg_literal_as_subject() {
    // Literals cannot be subjects in RDF 1.1
    load_err(
        "\"literal\" <http://example.org/p> <http://example.org/o> .",
        RdfFormat::Turtle,
    );
}

// ═══════════════════════════════════════════════════════════
// N-Triples Positive Syntax Tests
// W3C: rdf-n-triples/manifest.ttl
// ═══════════════════════════════════════════════════════════

#[test]
fn rdf_ntriples_pos_simple() {
    // ntriples-syntax-file-01
    let s = ts();
    load_ok(
        &s,
        "<http://example.org/s> <http://example.org/p> <http://example.org/o> .\n",
        RdfFormat::NTriples,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_ntriples_pos_literal() {
    let s = ts();
    load_ok(
        &s,
        "<http://example.org/s> <http://example.org/p> \"hello\" .\n",
        RdfFormat::NTriples,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_ntriples_pos_typed_literal() {
    let s = ts();
    load_ok(
        &s,
        "<http://example.org/s> <http://example.org/p> \"42\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n",
        RdfFormat::NTriples,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_ntriples_pos_lang_tagged() {
    let s = ts();
    load_ok(
        &s,
        "<http://example.org/s> <http://example.org/p> \"hello\"@en .\n",
        RdfFormat::NTriples,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_ntriples_pos_blank_node() {
    let s = ts();
    load_ok(
        &s,
        "_:bn <http://example.org/p> \"value\" .\n<http://example.org/s> <http://example.org/q> _:bn .\n",
        RdfFormat::NTriples,
    );
    assert_eq!(count_triples(&s), 2);
}

#[test]
fn rdf_ntriples_pos_string_escapes() {
    let s = ts();
    load_ok(
        &s,
        "<http://example.org/s> <http://example.org/p> \"tab\\there\" .\n",
        RdfFormat::NTriples,
    );
    load_ok(
        &s,
        "<http://example.org/s> <http://example.org/p> \"newline\\nhere\" .\n",
        RdfFormat::NTriples,
    );
}

#[test]
fn rdf_ntriples_pos_unicode_escape() {
    let s = ts();
    load_ok(
        &s,
        "<http://example.org/s> <http://example.org/p> \"\\u0041BC\" .\n", // ABC
        RdfFormat::NTriples,
    );
    load_ok(
        &s,
        "<http://example.org/s> <http://example.org/p> \"\\U0001F600\" .\n", // emoji
        RdfFormat::NTriples,
    );
}

#[test]
fn rdf_ntriples_pos_comments() {
    // Comments (#) are valid
    let s = ts();
    load_ok(
        &s,
        "# This is a comment\n<http://example.org/s> <http://example.org/p> \"v\" . # inline comment\n",
        RdfFormat::NTriples,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_ntriples_pos_multiple() {
    let s = ts();
    load_ok(
        &s,
        "<http://example.org/s1> <http://example.org/p> <http://example.org/o1> .\n<http://example.org/s2> <http://example.org/p> <http://example.org/o2> .\n<http://example.org/s3> <http://example.org/p> <http://example.org/o3> .\n",
        RdfFormat::NTriples,
    );
    assert_eq!(count_triples(&s), 3);
}

// ═══════════════════════════════════════════════════════════
// N-Triples Negative Syntax Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn rdf_ntriples_neg_missing_iri_close() {
    load_err(
        "<http://example.org/s <http://example.org/p> <http://example.org/o> .\n",
        RdfFormat::NTriples,
    );
}

#[test]
fn rdf_ntriples_neg_relative_iri() {
    // Relative IRIs are not valid in N-Triples
    load_err("<s> <p> <o> .\n", RdfFormat::NTriples);
}

#[test]
fn rdf_ntriples_neg_prefix() {
    // Prefix declarations are not valid in N-Triples
    load_err(
        "@prefix ex: <http://example.org/> . ex:s ex:p ex:o .",
        RdfFormat::NTriples,
    );
}

// ═══════════════════════════════════════════════════════════
// N-Quads Positive Syntax Tests
// W3C: rdf-n-quads/manifest.ttl
// ═══════════════════════════════════════════════════════════

#[test]
fn rdf_nquads_pos_simple() {
    let s = ts();
    s.load_str(
        "<http://example.org/s> <http://example.org/p> <http://example.org/o> <http://example.org/g> .\n",
        RdfFormat::NQuads,
        None,
    ).unwrap();
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_nquads_pos_default_graph() {
    // Triple without graph name goes to default graph
    let s = ts();
    s.load_str(
        "<http://example.org/s> <http://example.org/p> \"value\" .\n",
        RdfFormat::NQuads,
        None,
    ).unwrap();
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_nquads_pos_multiple_graphs() {
    let s = ts();
    s.load_str(
        "<http://example.org/s> <http://example.org/p> \"v1\" <http://example.org/g1> .\n<http://example.org/s> <http://example.org/p> \"v2\" <http://example.org/g2> .\n",
        RdfFormat::NQuads,
        None,
    ).unwrap();
    let graphs = s.named_graphs().unwrap();
    assert_eq!(graphs.len(), 2);
}

#[test]
fn rdf_nquads_pos_blank_subject_in_named_graph() {
    let s = ts();
    s.load_str(
        "_:b1 <http://example.org/p> \"v\" <http://example.org/g> .\n",
        RdfFormat::NQuads,
        None,
    ).unwrap();
    assert_eq!(count_triples(&s), 1);
}

// ═══════════════════════════════════════════════════════════
// TriG Positive Syntax Tests
// W3C: rdf-trig/manifest.ttl
// ═══════════════════════════════════════════════════════════

#[test]
fn rdf_trig_pos_simple() {
    let s = ts();
    s.load_str(
        "@prefix ex: <http://example.org/> . GRAPH ex:g1 { ex:s ex:p ex:o . }",
        RdfFormat::TriG,
        None,
    ).unwrap();
    assert_eq!(count_triples(&s), 1);
    let graphs = s.named_graphs().unwrap();
    assert_eq!(graphs.len(), 1);
}

#[test]
fn rdf_trig_pos_default_graph() {
    let s = ts();
    s.load_str(
        "@prefix ex: <http://example.org/> . { ex:s ex:p ex:o . }",
        RdfFormat::TriG,
        None,
    ).unwrap();
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_trig_pos_multiple_graphs() {
    let s = ts();
    s.load_str(
        r#"@prefix ex: <http://example.org/> .
           GRAPH ex:g1 { ex:s1 ex:p ex:o1 . }
           GRAPH ex:g2 { ex:s2 ex:p ex:o2 . }
           { ex:s3 ex:p ex:o3 . }"#,
        RdfFormat::TriG,
        None,
    ).unwrap();
    assert_eq!(count_triples(&s), 3);
    assert_eq!(s.named_graphs().unwrap().len(), 2);
}

#[test]
fn rdf_trig_pos_directives() {
    // @base and @prefix work in TriG
    let s = ts();
    s.load_str(
        "@base <http://example.org/> . @prefix ex: <http://example.org/> . GRAPH <g> { ex:s ex:p <o> . }",
        RdfFormat::TriG,
        None,
    ).unwrap();
    assert_eq!(count_triples(&s), 1);
}

// ═══════════════════════════════════════════════════════════
// RDF/XML Positive Syntax Tests
// W3C: rdf-xml/manifest.ttl (limited subset)
// ═══════════════════════════════════════════════════════════

#[test]
fn rdf_xml_pos_simple() {
    let s = ts();
    load_ok(
        &s,
        r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:ex="http://example.org/">
  <rdf:Description rdf:about="http://example.org/s">
    <ex:p rdf:resource="http://example.org/o"/>
  </rdf:Description>
</rdf:RDF>"#,
        RdfFormat::RdfXml,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_xml_pos_literal() {
    let s = ts();
    load_ok(
        &s,
        r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:ex="http://example.org/">
  <rdf:Description rdf:about="http://example.org/s">
    <ex:name>Alice</ex:name>
  </rdf:Description>
</rdf:RDF>"#,
        RdfFormat::RdfXml,
    );
    assert_eq!(count_triples(&s), 1);
}

#[test]
fn rdf_xml_pos_typed_node() {
    let s = ts();
    load_ok(
        &s,
        r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:ex="http://example.org/"
         xmlns:foaf="http://xmlns.com/foaf/0.1/">
  <foaf:Person rdf:about="http://example.org/alice">
    <foaf:name>Alice</foaf:name>
  </foaf:Person>
</rdf:RDF>"#,
        RdfFormat::RdfXml,
    );
    assert_eq!(count_triples(&s), 2); // rdf:type + foaf:name
}

#[test]
fn rdf_xml_pos_datatype_attribute() {
    let s = ts();
    load_ok(
        &s,
        r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:xsd="http://www.w3.org/2001/XMLSchema#"
         xmlns:ex="http://example.org/">
  <rdf:Description rdf:about="http://example.org/s">
    <ex:age rdf:datatype="http://www.w3.org/2001/XMLSchema#integer">42</ex:age>
  </rdf:Description>
</rdf:RDF>"#,
        RdfFormat::RdfXml,
    );
    assert!(ask(&s, "ASK { <http://example.org/s> <http://example.org/age> 42 }"));
}

#[test]
fn rdf_xml_pos_language_attribute() {
    let s = ts();
    load_ok(
        &s,
        r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:ex="http://example.org/" xml:lang="en">
  <rdf:Description rdf:about="http://example.org/s">
    <ex:name>Alice</ex:name>
  </rdf:Description>
</rdf:RDF>"#,
        RdfFormat::RdfXml,
    );
    assert!(ask(&s, "ASK { ?s ?p \"Alice\"@en }"));
}

#[test]
fn rdf_xml_pos_blank_node() {
    let s = ts();
    load_ok(
        &s,
        r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:ex="http://example.org/">
  <rdf:Description rdf:about="http://example.org/s">
    <ex:related>
      <rdf:Description>
        <ex:name>anon</ex:name>
      </rdf:Description>
    </ex:related>
  </rdf:Description>
</rdf:RDF>"#,
        RdfFormat::RdfXml,
    );
    assert_eq!(count_triples(&s), 2);
}

// ═══════════════════════════════════════════════════════════
// RDF 1.1 Data Model Semantics Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn rdf11_blank_node_identity() {
    // Blank nodes within the same document have the same identity
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . _:x ex:a \"Alice\" . _:x ex:b \"Bob\" .",
        RdfFormat::Turtle,
    );
    // Both triples reference the same blank node
    let q = "SELECT ?a ?b WHERE { ?x <http://example.org/a> ?a . ?x <http://example.org/b> ?b }";
    match s.query(q).unwrap() {
        QueryResults::Solutions(sols) => {
            let rows: Vec<_> = sols.collect();
            assert_eq!(rows.len(), 1);
        }
        _ => panic!(),
    }
}

#[test]
fn rdf11_blank_node_scope() {
    // Blank nodes across documents are distinct
    let s = ts();
    s.load_str(
        "_:x <http://example.org/p> \"value1\" .",
        RdfFormat::NTriples,
        Some("http://example.org/g1"),
    ).unwrap();
    s.load_str(
        "_:x <http://example.org/p> \"value2\" .",
        RdfFormat::NTriples,
        Some("http://example.org/g2"),
    ).unwrap();
    // The _:x in each graph are different blank nodes
    let q = "SELECT (COUNT(DISTINCT ?s) AS ?c) WHERE { ?s <http://example.org/p> ?o }";
    match s.query(q).unwrap() {
        QueryResults::Solutions(sols) => {
            let row = sols.into_iter().next().unwrap().unwrap();
            let c = row.get("c").unwrap().to_string();
            assert!(c.contains("2"), "Expected 2 distinct blank nodes, got: {}", c);
        }
        _ => panic!(),
    }
}

#[test]
fn rdf11_iri_equality() {
    // Two IRIs are equal iff they are character-by-character equal
    let s = ts();
    load_ok(
        &s,
        "<http://example.org/a> <http://example.org/p> <http://example.org/o> .",
        RdfFormat::Turtle,
    );
    // Same IRI different representation — only exact match works
    assert!(ask(&s, "ASK { <http://example.org/a> <http://example.org/p> <http://example.org/o> }"));
    assert!(!ask(&s, "ASK { <HTTP://EXAMPLE.ORG/a> <http://example.org/p> <http://example.org/o> }"));
}

#[test]
fn rdf11_literal_equality() {
    // Literals are equal if same value + datatype + (for lang tags) language
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:s ex:p \"hello\"@en . ex:s ex:q \"hello\"@fr .",
        RdfFormat::Turtle,
    );
    // @en and @fr literals are different
    let q = "SELECT (COUNT(DISTINCT ?o) AS ?c) WHERE { ?s ?p ?o }";
    match s.query(q).unwrap() {
        QueryResults::Solutions(sols) => {
            let row = sols.into_iter().next().unwrap().unwrap();
            let c = row.get("c").unwrap().to_string();
            assert!(c.contains("2"), "Expected 2 distinct literals, got: {}", c);
        }
        _ => panic!(),
    }
}

#[test]
fn rdf11_typed_literal_values() {
    // xsd:integer "1" == xsd:integer "01" (same value, canonical form differs)
    let s = ts();
    load_ok(
        &s,
        "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> . @prefix ex: <http://example.org/> . ex:s ex:p \"01\"^^xsd:integer .",
        RdfFormat::Turtle,
    );
    // SPARQL numeric comparison works on value
    assert!(ask(&s, "ASK { ?s <http://example.org/p> ?v . FILTER(?v = 1) }"));
}

#[test]
fn rdf11_rdf_type_shorthand() {
    // 'a' in Turtle = rdf:type IRI
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:alice a ex:Person .",
        RdfFormat::Turtle,
    );
    assert!(ask(
        &s,
        "ASK { <http://example.org/alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Person> }"
    ));
}

// ═══════════════════════════════════════════════════════════
// Format Conversion Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn rdf11_roundtrip_turtle_ntriples() {
    let s = ts();
    let ttl = "@prefix ex: <http://example.org/> . ex:a ex:b ex:c . ex:d ex:e ex:f .";
    load_ok(&s, ttl, RdfFormat::Turtle);
    let nt = s.dump(RdfFormat::NTriples, None).unwrap();
    let nt_str = String::from_utf8(nt).unwrap();
    // Both triples should appear in N-Triples output
    assert!(nt_str.contains("example.org/a"));
    assert!(nt_str.contains("example.org/d"));

    // Reload from N-Triples into fresh store
    let s2 = ts();
    load_ok(&s2, &nt_str, RdfFormat::NTriples);
    assert_eq!(count_triples(&s2), 2);
}

#[test]
fn rdf11_roundtrip_turtle_rdfxml() {
    let s = ts();
    load_ok(
        &s,
        "@prefix ex: <http://example.org/> . ex:alice a ex:Person ; ex:name \"Alice\" .",
        RdfFormat::Turtle,
    );
    let xml = s.dump(RdfFormat::RdfXml, None).unwrap();
    let xml_str = String::from_utf8(xml).unwrap();
    assert!(xml_str.contains("Alice"));

    let s2 = ts();
    load_ok(&s2, &xml_str, RdfFormat::RdfXml);
    assert_eq!(count_triples(&s2), 2);
}

#[test]
fn rdf11_roundtrip_with_literals() {
    let s = ts();
    load_ok(
        &s,
        r#"@prefix ex: <http://example.org/> .
           @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
           ex:s ex:p "hello"@en ;
                ex:q 42 ;
                ex:r 3.14 ;
                ex:t true ;
                ex:d "2024-01-15"^^xsd:date ."#,
        RdfFormat::Turtle,
    );
    assert_eq!(count_triples(&s), 5);
    let nt = s.dump(RdfFormat::NTriples, None).unwrap();
    let s2 = ts();
    load_ok(&s2, &String::from_utf8(nt).unwrap(), RdfFormat::NTriples);
    assert_eq!(count_triples(&s2), 5);
}

// ═══════════════════════════════════════════════════════════
// RDF 1.1 Graph Management Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn rdf11_named_graph_put_get() {
    let s = ts();
    s.load_str(
        "<http://example.org/s> <http://example.org/p> \"graph1\" .",
        RdfFormat::NTriples,
        Some("http://example.org/g1"),
    ).unwrap();
    s.load_str(
        "<http://example.org/s> <http://example.org/p> \"graph2\" .",
        RdfFormat::NTriples,
        Some("http://example.org/g2"),
    ).unwrap();

    let graphs = s.named_graphs().unwrap();
    assert_eq!(graphs.len(), 2);

    // Dump just one named graph
    let g1_data = s.dump(RdfFormat::NTriples, Some("http://example.org/g1")).unwrap();
    let g1_str = String::from_utf8(g1_data).unwrap();
    assert!(g1_str.contains("graph1"));
    assert!(!g1_str.contains("graph2"));
}

#[test]
fn rdf11_dataset_default_and_named() {
    let s = ts();
    // Load into default graph
    s.load_str(
        "<http://example.org/s> <http://example.org/p> \"default\" .",
        RdfFormat::NTriples,
        None,
    ).unwrap();
    // Load into named graph
    s.load_str(
        "<http://example.org/s> <http://example.org/p> \"named\" .",
        RdfFormat::NTriples,
        Some("http://example.org/g"),
    ).unwrap();

    // Total triples count
    assert_eq!(count_triples(&s), 2);

    // Query default graph (no GRAPH clause)
    let q_default = match s.query("SELECT ?v WHERE { <http://example.org/s> <http://example.org/p> ?v }").unwrap() {
        QueryResults::Solutions(sols) => {
            sols.into_iter()
                .map(|r| r.unwrap().get("v").unwrap().to_string())
                .collect::<Vec<_>>()
        }
        _ => panic!(),
    };
    assert!(q_default.iter().any(|v| v.contains("default")));

    // Query named graph
    let q_named = match s.query("SELECT ?v WHERE { GRAPH <http://example.org/g> { <http://example.org/s> <http://example.org/p> ?v } }").unwrap() {
        QueryResults::Solutions(sols) => {
            sols.into_iter()
                .map(|r| r.unwrap().get("v").unwrap().to_string())
                .collect::<Vec<_>>()
        }
        _ => panic!(),
    };
    assert!(q_named.iter().any(|v| v.contains("named")));
}
