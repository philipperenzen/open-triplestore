//! SHACL Compact Syntax (SHACL-C) conformance tests.
//!
//! Grounded in the W3C SHACL-C Community Group report
//! (https://w3c.github.io/shacl/shacl-compact-syntax/), whose grammar and
//! production rules the parser implements; the report's own test cases run
//! in `tests/w3c_shaclc_conformance.rs`. These tests pin what that corpus
//! does not: the HTTP contract (strict parsing, the deprecated
//! `dialect=legacy`, `422` with `losses` instead of a silently thinner
//! document), the serializer's round trip over store-loaded graphs, and the
//! places the old dialect and the W3C syntax disagree.

mod common;

use open_triplestore::shaclc::{parse, parse_as, serialize, Dialect, SerializeError};
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

shape ex:PersonShape -> ex:Person {
    ex:name xsd:string [1..1] .
    ex:email xsd:string [0..*] .
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

shape ex:S -> ex:T {
    ex:exact xsd:string [1..1] .
    ex:range xsd:integer [2..5] .
    ex:unbounded xsd:string [0..*] .
}
"#,
    )
    .expect("parse");
    assert!(turtle.contains("sh:minCount 2"), "[2..5] => minCount 2");
    assert!(turtle.contains("sh:maxCount 5"), "[2..5] => maxCount 5");
    // [0..*] writes neither bound (the report: no minCount for "0", no
    // maxCount for "*").
    assert!(!turtle.contains("sh:minCount 0"), "{turtle}");
    assert!(!turtle.contains("sh:maxCount 0"), "no spurious maxCount 0");
}

// `closed=true` is a node parameter, as are `ignoredProperties=[…]`.
#[test]
fn shaclc_closed_shape() {
    let turtle = parse(
        r#"
PREFIX ex: <http://example.org/>

shape ex:ClosedShape -> ex:Thing {
    closed=true ignoredProperties=[rdf:type] .
    ex:name xsd:string [1..1] .
}
"#,
    )
    .expect("parse");
    assert!(turtle.contains("sh:closed true"), "{turtle}");
    assert!(turtle.contains("sh:ignoredProperties"), "{turtle}");
}

// `message="…"` attaches an sh:message to the property shape.
#[test]
fn shaclc_message() {
    let turtle = parse(
        r#"
PREFIX ex: <http://example.org/>

shape ex:S -> ex:Thing {
    ex:name xsd:string [1..1] message="Name is required" .
}
"#,
    )
    .expect("parse");
    assert!(
        turtle.contains("sh:message \"Name is required\""),
        "{turtle}"
    );
}

// Multiple shapes in one document each produce a node shape.
#[test]
fn shaclc_multiple_shapes() {
    let turtle = parse(
        r#"
PREFIX ex: <http://example.org/>

shape ex:AShape -> ex:A {
    ex:p1 xsd:string [1..1] .
}
shape ex:BShape -> ex:B {
    ex:p2 xsd:integer [0..1] .
}
"#,
    )
    .expect("parse");
    assert!(turtle.contains("ex:AShape"));
    assert!(turtle.contains("ex:BShape"));
    assert!(turtle.contains("sh:targetClass ex:A"));
    assert!(turtle.contains("sh:targetClass ex:B"));
}

/// Graph isomorphism of two Turtle documents.
fn same_graph(a: &str, b: &str) -> bool {
    use oxrdf::dataset::CanonicalizationAlgorithm;
    let g = |t: &str| {
        let mut g = oxrdf::Graph::new();
        for q in oxigraph::io::RdfParser::from_format(RdfFormat::Turtle).for_slice(t.as_bytes()) {
            g.insert(q.unwrap().as_ref());
        }
        g.canonicalize(CanonicalizationAlgorithm::Unstable);
        g
    };
    g(a) == g(b)
}

// Round trip: SHACL-C -> Turtle -> store -> SHACL-C -> Turtle gives the same
// graph, not merely "some core constraint survives".
#[test]
fn shaclc_roundtrip_through_the_store_is_isomorphic() {
    let shaclc = r#"
PREFIX ex: <http://example.org/>

shape ex:PersonShape -> ex:Person {
    targetSubjectsOf=ex:knows .
    ex:name xsd:string [1..1] pattern="^[A-Z]" flags="i" message="Needs a \"name\"\n(capitalised)"@en .
    ex:knows/ex:name|^ex:friend IRI @ex:PersonShape .
    ex:age xsd:integer|xsd:decimal minInclusive=0 !hasValue=-1 .
    ex:status in=[ex:Active ex:Retired "other" 42 true] .
}
"#;
    let turtle1 = parse(shaclc).expect("parse 1");
    let store = load_turtle(&turtle1);
    let shaclc2 = serialize(&store, "urn:shapes").expect("lossless serialize");
    let turtle2 = parse(&shaclc2).expect("re-parse serialized SHACL-C");
    assert!(
        same_graph(&turtle1, &turtle2),
        "round trip changed the graph:\n{shaclc2}\n--- before\n{turtle1}\n--- after\n{turtle2}"
    );
}

// The W3C grammar and the old dialect disagree on a bare IRI after a path:
// the W3C rule makes it sh:class (sh:datatype for XSD/RDF datatypes) and
// writes shape references `@ex:Shape`. The old dialect read it as sh:node.
#[test]
fn shaclc_bare_iri_after_a_path_is_sh_class_and_at_iri_is_sh_node() {
    let doc = r#"
PREFIX ex: <http://example.org/>
shape ex:S {
    ex:employer ex:Company .
    ex:address @ex:AddressShape .
}
"#;
    let turtle = parse(doc).expect("parse");
    assert!(turtle.contains("sh:class ex:Company"), "{turtle}");
    assert!(turtle.contains("sh:node ex:AddressShape"), "{turtle}");
    assert!(!turtle.contains("sh:node ex:Company"), "{turtle}");
}

// The deprecated dialect still parses — only when asked for — with its old
// meaning, so a stored document keeps the shapes it had.
#[test]
fn shaclc_legacy_dialect_parses_the_old_syntax_when_asked() {
    let old = r#"
PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

shape ex:S -> ex:Thing closed {
    ex:name xsd:string [1..1] // "Name is required" ;
    ex:address ex:AddressShape ;
}
"#;
    let err = parse(old).expect_err("the old dialect is not W3C SHACL-C");
    assert!(
        err.contains("dialect=legacy"),
        "the error names the switch: {err}"
    );

    let turtle = parse_as(old, Dialect::Legacy, false, None).expect("legacy parse");
    assert!(turtle.contains("sh:closed true"), "{turtle}");
    assert!(
        turtle.contains("sh:message \"Name is required\""),
        "{turtle}"
    );
    assert!(turtle.contains("sh:node ex:AddressShape"), "{turtle}");
}

// The parser is STRICT: input the grammar does not know is an error naming
// its position. (It once dropped unrecognised input, so an upload of a
// document it did not understand replaced the shapes graph with nothing.)
#[test]
fn shaclc_rejects_unrecognised_input_by_default() {
    let err = parse("this is not valid shaclc @@@ {{{")
        .expect_err("garbage must be a parse error, not an empty document");
    assert!(
        err.contains("line 1") && err.contains("parse error"),
        "the error names the position: {err}"
    );
}

// An unknown constraint inside an otherwise valid shape is an error too.
#[test]
fn shaclc_rejects_an_unknown_constraint_inside_a_shape() {
    let input = r#"
PREFIX ex: <http://example.org/>

shape ex:S -> ex:T {
    ex:name xsd:string [1..1] .
    ex:age frobnicate .
}
"#;
    let err = parse(input).expect_err("an unknown constraint keyword is an error");
    assert!(err.contains("line 6"), "position named: {err}");
}

// `lenient` survives only with the legacy dialect; the W3C grammar has no
// lenient mode, and asking for one is refused rather than ignored.
#[test]
fn shaclc_lenient_mode_is_legacy_only() {
    let r = parse_as(
        "this is not valid shaclc @@@ {{{",
        Dialect::Legacy,
        true,
        None,
    );
    assert!(r.is_ok(), "legacy lenient mode does not hard-error");
    assert!(!r.unwrap().contains("sh:NodeShape"));
    let err = parse_as("shape <urn:x:S> { }", Dialect::W3c, true, None).unwrap_err();
    assert!(err.contains("legacy"), "{err}");
}

// Serializing a shape with SEVERAL blank-node property shapes — the standard
// `sh:property [ … ]` idiom — must give each property its own path and
// datatype (an old bug resolved blank nodes through SPARQL, where they are
// variables, so every property drew an arbitrary path/datatype).
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

    let line = |needle: &str| {
        out.lines()
            .find(|l| l.contains(needle))
            .unwrap_or_default()
            .to_string()
    };
    let (name_line, age_line) = (line("name"), line("age"));
    assert!(
        name_line.contains("string") && !name_line.contains("integer"),
        "ex:name must keep xsd:string, got: {name_line:?}\nfull output:\n{out}"
    );
    assert!(
        age_line.contains("integer") && !age_line.contains("string"),
        "ex:age must keep xsd:integer, got: {age_line:?}\nfull output:\n{out}"
    );
}

// What the compact syntax cannot say is reported, never dropped: SPARQL
// constraints, sh:name, named property shapes. The implied triples
// (rdf:type sh:PropertyShape / sh:NodeShape, sh:minCount 0) are not losses.
#[test]
fn shaclc_serializer_reports_every_loss() {
    let turtle = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .

ex:S a sh:NodeShape ;
    sh:sparql [ sh:select "SELECT $this WHERE { }" ] ;
    sh:property ex:NamedProp ;
    sh:property [ a sh:PropertyShape ; sh:path ex:p ; sh:minCount 0 ; sh:name "p" ] .
ex:NamedProp sh:path ex:q .
"#;
    let store = load_turtle(turtle);
    let losses = match serialize(&store, "urn:shapes") {
        Err(SerializeError::Losses(l)) => l,
        other => panic!("expected a losses report, got {other:?}"),
    };
    let has = |pred: &str| losses.iter().any(|l| l.predicate.contains(pred));
    for pred in ["#sparql", "#select", "#name"] {
        assert!(has(pred), "{pred} must be reported: {losses:#?}");
    }
    for pred in ["#minCount", "22-rdf-syntax-ns#type"] {
        assert!(!has(pred), "{pred} is implied, not a loss: {losses:#?}");
    }
    assert!(
        losses.iter().any(|l| l.object.contains("NamedProp")),
        "a named property shape is a loss: {losses:#?}"
    );
    assert!(losses.iter().all(|l| !l.reason.is_empty()));
    // The path and the shape itself are written.
    assert!(!losses.iter().any(|l| l.object == "<http://example.org/p>"));
    assert!(!losses
        .iter()
        .any(|l| l.predicate.ends_with("#type>") && l.object.contains("NodeShape")));
}

// An ordinary hand-written shapes graph — typed property shapes, typed
// nested shapes, sh:minCount 0 — serializes without a 422.
#[test]
fn shaclc_typical_typed_shapes_graph_is_lossless() {
    let turtle = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ a sh:PropertyShape ; sh:path ex:name ; sh:datatype xsd:string ;
                  sh:minCount 1 ; sh:maxCount 1 ] ;
    sh:property [ a sh:PropertyShape ; sh:path ex:nickname ; sh:minCount 0 ] ;
    sh:property [ a sh:PropertyShape ; sh:path ex:address ;
                  sh:node [ a sh:NodeShape ; sh:property [ a sh:PropertyShape ; sh:path ex:city ; sh:minCount 1 ] ] ] .
"#;
    let store = load_turtle(turtle);
    let out = serialize(&store, "urn:shapes").expect("lossless: only implied triples are omitted");
    assert!(out.contains("[1..1]") && out.contains("{"), "{out}");
}

// Over HTTP: the parse endpoint, the dialect switch, and the GET handlers'
// 422-or-lossless contract for datasets and Studio shape graphs.
mod http {
    use super::common::*;
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use axum::Router;
    use open_triplestore::auth::models::{OwnerType, Visibility};
    use open_triplestore::server::AppState;
    use serde_json::{json, Value};
    use tower::ServiceExt as _;

    async fn call(
        app: &Router,
        method: Method,
        uri: &str,
        token: &str,
        content_type: &str,
        body: &str,
    ) -> (StatusCode, String, Option<String>) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, content_type)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let st = resp.status();
        let losses = resp
            .headers()
            .get("x-shaclc-losses")
            .map(|v| v.to_str().unwrap().to_string());
        (st, body_text(resp.into_body()).await, losses)
    }

    const LEGACY: &str = "PREFIX ex: <http://example.org/>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\nshape ex:S -> ex:T { ex:p xsd:string [1..1] ; }\nnonsense here";

    #[tokio::test]
    async fn shaclc_parse_endpoint_is_strict_and_legacy_needs_the_dialect_switch() {
        let (state, token) = admin_state();
        let app = test_app(state);
        let post = |uri: &'static str, body: &'static str| {
            let app = app.clone();
            let token = token.clone();
            async move { call(&app, Method::POST, uri, &token, "text/shaclc", body).await }
        };

        let (st, txt, _) = post("/api/shaclc/parse", LEGACY).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
        assert!(txt.contains("line 3"), "{txt}");

        let (st, txt, _) = post("/api/shaclc/parse?lenient=true", LEGACY).await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "lenient alone is refused: {txt}"
        );

        let (st, txt, _) = post("/api/shaclc/parse?dialect=legacy&lenient=true", LEGACY).await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert!(
            txt.contains("sh:NodeShape"),
            "the parsable shape is kept: {txt}"
        );

        let (st, txt, _) = post("/api/shaclc/parse?dialect=shexc", "").await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");

        let (st, txt, _) = post(
            "/api/shaclc/parse",
            "PREFIX ex: <http://example.org/>\nshape ex:S -> ex:T { ex:p xsd:string [1..1] . }",
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert!(txt.contains("sh:datatype xsd:string"), "{txt}");

        let (st, txt, _) = post(
            "/api/shaclc/parse?base=http://example.org/doc",
            "shape <S> { }",
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert!(
            txt.contains("http://example.org/S") && txt.contains("Ontology"),
            "{txt}"
        );
    }

    fn dataset(state: &AppState) {
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();
    }

    const EXPRESSIBLE: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.org/> .\nex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:property [ sh:path ex:p ; sh:minCount 1 ] .\n";
    const LOSSY: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.org/> .\nex:S a sh:NodeShape ; sh:targetClass ex:T ; sh:sparql ex:Check ; sh:property [ a sh:PropertyShape ; sh:path ex:p ; sh:minCount 0 ] .\n";

    async fn check_get(app: &Router, token: &str, put: &str, put_ct: &str, get: &str) {
        // Lossy graph: 422 with the losses, never a thinner 200.
        let (st, txt, _) = call(app, Method::PUT, put, token, put_ct, LOSSY).await;
        assert!(st.is_success(), "{put}: {txt}");
        let (st, txt, _) = call(
            app,
            Method::GET,
            &format!("{get}?format=shaclc"),
            token,
            "text/plain",
            "",
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{get}: {txt}");
        let v: Value = serde_json::from_str(&txt).unwrap();
        let losses = v["losses"].as_array().expect("losses list");
        assert_eq!(losses.len(), 1, "{txt}");
        assert!(
            losses[0]["predicate"].as_str().unwrap().contains("#sparql"),
            "{txt}"
        );
        assert!(
            losses[0]["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "{txt}"
        );

        // ?lossy=true: the partial document, flagged.
        let (st, txt, count) = call(
            app,
            Method::GET,
            &format!("{get}?format=shaclc&lossy=true"),
            token,
            "text/plain",
            "",
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert_eq!(count.as_deref(), Some("1"));
        assert!(txt.starts_with("# INCOMPLETE"), "{txt}");
        assert!(txt.contains("shape ") && txt.contains(" -> "), "{txt}");

        // Lossless graph: 200 text/shaclc that parses back.
        let (st, txt, _) = call(app, Method::PUT, put, token, put_ct, EXPRESSIBLE).await;
        assert!(st.is_success(), "{put}: {txt}");
        let (st, txt, _) = call(
            app,
            Method::GET,
            &format!("{get}?format=shaclc"),
            token,
            "text/plain",
            "",
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{get}: {txt}");
        assert!(txt.contains("shape ") && txt.contains(" [1..*] ."), "{txt}");
        let back = open_triplestore::shaclc::parse(&txt).expect("the served document parses");
        assert!(super::same_graph(&back, EXPRESSIBLE), "{txt}\n---\n{back}");
    }

    #[tokio::test]
    async fn dataset_shapes_get_format_shaclc_is_lossless_or_422() {
        let (state, token) = admin_state();
        dataset(&state);
        let app = test_app(state);
        check_get(
            &app,
            &token,
            "/api/datasets/d1/shapes",
            "text/turtle",
            "/api/datasets/d1/shapes",
        )
        .await;

        // A W3C SHACL-C upload, and a legacy one behind the switch.
        let (st, txt, _) = call(
            &app,
            Method::PUT,
            "/api/datasets/d1/shapes",
            &token,
            "text/shaclc",
            "PREFIX ex: <http://example.org/>\nshape ex:S -> ex:T { ex:p ex:C [1..1] . }",
        )
        .await;
        assert!(st.is_success(), "{txt}");
        let (_, txt, _) = call(
            &app,
            Method::GET,
            "/api/datasets/d1/shapes",
            &token,
            "text/plain",
            "",
        )
        .await;
        assert!(
            txt.contains("sh:class") || txt.contains("shacl#class>"),
            "a bare IRI is sh:class: {txt}"
        );
        let (st, txt, _) = call(
            &app,
            Method::PUT,
            "/api/datasets/d1/shapes",
            &token,
            "text/shaclc",
            "PREFIX ex: <http://example.org/>\nshape ex:S -> ex:T { ex:p ex:C [1..1] ; }",
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
        let (st, txt, _) = call(
            &app,
            Method::PUT,
            "/api/datasets/d1/shapes?dialect=legacy",
            &token,
            "text/shaclc",
            "PREFIX ex: <http://example.org/>\nshape ex:S -> ex:T { ex:p ex:C [1..1] ; }",
        )
        .await;
        assert!(st.is_success(), "{txt}");
        let (_, txt, _) = call(
            &app,
            Method::GET,
            "/api/datasets/d1/shapes",
            &token,
            "text/plain",
            "",
        )
        .await;
        assert!(
            txt.contains("sh:node") || txt.contains("shacl#node>"),
            "legacy keeps its meaning: {txt}"
        );
    }

    #[tokio::test]
    async fn studio_shape_graph_get_format_shaclc_is_lossless_or_422() {
        let (state, token) = admin_state();
        let app = test_app(state);
        let (st, txt, _) = call(
            &app,
            Method::POST,
            "/api/shacl/shape-graphs",
            &token,
            "application/json",
            &json!({ "name": "s", "visibility": "private", "turtle": EXPRESSIBLE }).to_string(),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{txt}");
        let id = serde_json::from_str::<Value>(&txt).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let uri = format!("/api/shacl/shape-graphs/{id}/turtle");
        check_get(&app, &token, &uri, "text/turtle", &uri).await;

        // Studio PUT takes W3C SHACL-C too.
        let (st, txt, _) = call(
            &app,
            Method::PUT,
            &uri,
            &token,
            "text/shaclc",
            "PREFIX ex: <http://example.org/>\nshape ex:S { ex:q xsd:integer . }",
        )
        .await;
        assert!(st.is_success(), "{txt}");
        let (st, txt, _) = call(
            &app,
            Method::PUT,
            &uri,
            &token,
            "text/shaclc",
            "shape ex:S { }",
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
        assert!(
            txt.contains("undeclared prefix") && !txt.contains("error: SHACL"),
            "{txt}"
        );
    }
}
