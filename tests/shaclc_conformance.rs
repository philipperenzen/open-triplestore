//! SHACL Compact Syntax (SHACL-C) conformance tests.
//!
//! Grounded in the W3C SHACL-C Community Group report
//! (https://w3c.github.io/shacl/shacl-compact-syntax/) and adversarially
//! fact-checked. The engine ships a custom `nom` parser + serializer that
//! implements a pragmatic SUBSET/variant of SHACL-C:
//!   * counts use the bracket range `[min..max]` (e.g. `[1..1]`, `[0..*]`)
//!     — matching the spec's `propertyCount` production;
//!   * `shape X -> Class { ... }` is the targetClass shorthand;
//!   * messages use the `// "msg"` convention (the engine's variant; the W3C
//!     grammar uses `message="..."`).
//!
//! These tests exercise the supported productions and verify a Turtle ->
//! SHACL-C -> Turtle round-trip preserves the core constraints.

mod common;

use open_triplestore::shaclc::{parse, parse_lenient, serialize};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;

fn load_turtle(turtle: &str) -> TripleStore {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(turtle, RdfFormat::Turtle, Some("urn:shapes"))
        .unwrap();
    store
}

// Basic node shape with targetClass shorthand and typed property constraints.
#[test]
fn shaclc_parse_basic_shape() {
    let turtle = parse(
        r#"
PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

shape ex:PersonShape -> ex:Person {
    ex:name xsd:string [1..1] ;
    ex:email xsd:string [0..*] ;
}
"#,
    )
    .expect("parse");
    assert!(turtle.contains("sh:NodeShape"));
    assert!(turtle.contains("sh:targetClass"));
    assert!(turtle.contains("sh:path ex:name"));
    assert!(turtle.contains("sh:datatype xsd:string"));
    assert!(turtle.contains("sh:minCount 1"));
    assert!(turtle.contains("sh:maxCount 1"));
}

// Count-range translation: [1..1], [2..5], and unbounded/zero forms.
#[test]
fn shaclc_count_ranges() {
    let turtle = parse(
        r#"
PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

shape ex:S -> ex:T {
    ex:exact xsd:string [1..1] ;
    ex:range xsd:integer [2..5] ;
    ex:unbounded xsd:string [0..*] ;
}
"#,
    )
    .expect("parse");
    // Exact and explicit bounds appear.
    assert!(turtle.contains("sh:minCount 2"), "[2..5] => minCount 2");
    assert!(turtle.contains("sh:maxCount 5"), "[2..5] => maxCount 5");
    // [0..*] is unbounded: no minCount 0 and no maxCount for the unbounded property.
    // (We can't easily isolate per-property here; assert there is no maxCount 0 / spurious bound.)
    assert!(!turtle.contains("sh:maxCount 0"), "no spurious maxCount 0");
}

// `closed` keyword translates to sh:closed true.
#[test]
fn shaclc_closed_shape() {
    let turtle = parse(
        r#"
PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

shape ex:ClosedShape -> ex:Thing closed {
    ex:name xsd:string [1..1] ;
}
"#,
    )
    .expect("parse");
    assert!(turtle.contains("sh:closed true"));
}

// `// "msg"` attaches an sh:message to the property shape.
#[test]
fn shaclc_message() {
    let turtle = parse(
        r#"
PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

shape ex:S -> ex:Thing {
    ex:name xsd:string [1..1] // "Name is required" ;
}
"#,
    )
    .expect("parse");
    assert!(turtle.contains("sh:message \"Name is required\""));
}

// Multiple shapes in one document each produce a node shape.
#[test]
fn shaclc_multiple_shapes() {
    let turtle = parse(
        r#"
PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

shape ex:AShape -> ex:A {
    ex:p1 xsd:string [1..1] ;
}
shape ex:BShape -> ex:B {
    ex:p2 xsd:integer [0..1] ;
}
"#,
    )
    .expect("parse");
    assert!(turtle.contains("ex:AShape"));
    assert!(turtle.contains("ex:BShape"));
    assert!(turtle.contains("sh:targetClass ex:A"));
    assert!(turtle.contains("sh:targetClass ex:B"));
}

// Round-trip: SHACL-C -> Turtle -> (load) -> SHACL-C -> Turtle preserves the
// core constraints (path + cardinality survive serialization).
#[test]
fn shaclc_roundtrip_preserves_core_constraints() {
    let shaclc = r#"
PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

shape ex:PersonShape -> ex:Person {
    ex:name xsd:string [1..1] ;
}
"#;
    let turtle1 = parse(shaclc).expect("parse 1");
    let store = load_turtle(&turtle1);
    let shaclc2 = serialize(&store, "urn:shapes").expect("serialize");
    // The serialized SHACL-C must mention the path and re-parse cleanly.
    assert!(
        shaclc2.contains("name"),
        "serialized SHACL-C mentions the path, got:\n{shaclc2}"
    );
    let turtle2 = parse(&shaclc2).expect("re-parse serialized SHACL-C");
    assert!(turtle2.contains("sh:path"), "round-trip preserves sh:path");
    assert!(
        turtle2.contains("sh:minCount 1") || turtle2.contains("sh:datatype"),
        "round-trip preserves a core constraint, got:\n{turtle2}"
    );
}

// The parser is STRICT by default: input the grammar does not know is an
// error naming its position. It used to be lenient — unrecognised input was
// silently dropped — so a spec-conformant SHACL-C document using forms this
// mini-grammar does not implement parsed to an EMPTY document, and uploading
// it replaced the dataset's shapes graph with nothing while answering 200.
#[test]
fn shaclc_rejects_unrecognised_input_by_default() {
    let r = parse("this is not valid shaclc @@@ {{{");
    let err = r.expect_err("garbage must be a parse error, not an empty document");
    assert!(
        err.contains("line 1") && err.contains("unrecognised input"),
        "the error names the position: {err}"
    );
    assert!(
        err.contains("lenient=true"),
        "the error names the opt-out: {err}"
    );
}

// An unknown constraint inside an otherwise valid shape is an error too —
// the whole shape (not just the token) would have been dropped before.
#[test]
fn shaclc_rejects_an_unknown_constraint_inside_a_shape() {
    let input = r#"
PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

shape ex:S -> ex:T {
    ex:name xsd:string [1..1] ;
    ex:age frobnicate ;
}
"#;
    let err = parse(input).expect_err("an unknown constraint keyword is an error");
    assert!(
        err.contains("line 5") || err.contains("line 4"),
        "position named: {err}"
    );
}

// `parse_lenient` keeps the old drop-what-you-cannot-parse behaviour for
// callers that opt in (`?lenient=true` over HTTP).
#[test]
fn shaclc_lenient_mode_ignores_unrecognised_input() {
    let r = parse_lenient("this is not valid shaclc @@@ {{{");
    assert!(r.is_ok(), "lenient mode does not hard-error");
    assert!(
        !r.unwrap().contains("sh:NodeShape"),
        "no shapes are produced from non-shape input"
    );
}

// Over HTTP: `POST /api/shaclc/parse` is strict (400 with the position) and
// `?lenient=true` restores the old behaviour.
mod http {
    use super::common::*;
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use tower::ServiceExt as _;

    async fn post(uri: &str, body: &str) -> (StatusCode, String) {
        // The endpoint is authenticated compute now, so these strictness cases
        // carry a token; the auth contract itself lives in
        // tests/api_auth_exposure.rs.
        let (state, token) = admin_state();
        let app = test_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "text/shaclc")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let st = resp.status();
        (st, body_text(resp.into_body()).await)
    }

    #[tokio::test]
    async fn shaclc_parse_endpoint_is_strict_unless_lenient_is_requested() {
        let garbage = "PREFIX ex: <http://example.org/>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\nshape ex:S -> ex:T { ex:p xsd:string [1..1] ; }\nnonsense here";
        let (st, txt) = post("/api/shaclc/parse", garbage).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
        assert!(txt.contains("unrecognised input"), "{txt}");
        let (st, txt) = post("/api/shaclc/parse?lenient=true", garbage).await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert!(
            txt.contains("sh:NodeShape"),
            "the parsable shape is kept: {txt}"
        );
    }
}

// Serializing a shape with SEVERAL blank-node property shapes — the standard
// `sh:property [ … ]` idiom — must give each property its own path and datatype.
//
// The serializer resolved a blank-node property shape by interpolating `_:bN`
// into a SPARQL query. In SPARQL a blank node is an existential VARIABLE, not a
// reference to the stored node, so the pattern matched every subject in the
// graph and each property drew an arbitrary path/datatype from whichever
// property shape matched first. The existing round-trip test uses a shape with
// ONE property, where "arbitrary" and "correct" coincide, so it never fired.
#[test]
fn shaclc_serializes_each_blank_node_property_shape_distinctly() {
    let turtle = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:name ; sh:datatype xsd:string ] ;
    sh:property [ sh:path ex:age  ; sh:datatype xsd:integer ] .
"#;
    let store = load_turtle(turtle);
    let out = serialize(&store, "urn:shapes").expect("serialize");

    assert!(
        out.contains("name") && out.contains("age"),
        "both property paths must appear, got:\n{out}"
    );

    // Each path must be paired with ITS OWN datatype. Under the old lookup both
    // lines got whichever datatype the wildcard match returned first.
    let name_line = out
        .lines()
        .find(|l| l.contains("name"))
        .unwrap_or_default()
        .to_string();
    let age_line = out
        .lines()
        .find(|l| l.contains("age"))
        .unwrap_or_default()
        .to_string();
    assert!(
        name_line.contains("string") && !name_line.contains("integer"),
        "ex:name must keep xsd:string, got: {name_line:?}\nfull output:\n{out}"
    );
    assert!(
        age_line.contains("integer") && !age_line.contains("string"),
        "ex:age must keep xsd:integer, got: {age_line:?}\nfull output:\n{out}"
    );
}
