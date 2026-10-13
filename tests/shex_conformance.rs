//! ShEx conformance — semantics over HTTP, not wiring.
//!
//! The only prior coverage of `/api/shex/validate` was a smoke test that
//! accepted HTTP 200 *or* 400, which passes whether validation works, fails or
//! rejects every request. These tests assert what the validator decides:
//! conforming and non-conforming focus nodes across cardinality, datatype,
//! node kind, value sets, string and numeric facets, regex patterns, CLOSED
//! shapes and shape references; then the ShEx 2.1 semantics the shexTest
//! corpus (`tests/shextest_conformance.rs`) checks on parsed data, here
//! through the HTTP API and the store: partitions, recursion, stratified
//! negation, the ShapeMap language and START, ShExJ, store-only IMPORT and
//! the Test semantic-action extension.

#![cfg(feature = "shex")]

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::server::AppState;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const PREFIXES: &str = "PREFIX ex: <http://example.org/>\n\
                        PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n";

const DATA: &str = r#"
@prefix ex: <http://example.org/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:ada  a ex:Person ; ex:name "Ada" ; ex:age 36 ; ex:email "ada@example.org" ;
        ex:status ex:Active ; ex:knows ex:bob .
ex:bob  a ex:Person ; ex:name "Bob" ; ex:age 200 ; ex:email "not-an-email" ;
        ex:status ex:Retired .
ex:cy   a ex:Person ; ex:age "thirty"^^xsd:string ; ex:nick "cy" ; ex:knows ex:nobody .
ex:dee  a ex:Person ; ex:name "Dee" ; ex:name "D." ; ex:age 12 .
"#;

fn seeded() -> (AppState, String) {
    let (state, token) = admin_state();
    state
        .store
        .load_str(DATA, oxigraph::io::RdfFormat::Turtle, None)
        .unwrap();
    (state, token)
}

/// Validate `focus` against `shape` under `schema` (ShExC, prefixes prepended).
async fn validate(state: &AppState, token: &str, schema: &str, shape: &str, focus: &str) -> Value {
    let body = json!({
        "schema": format!("{PREFIXES}{schema}"),
        "shape_map": { shape: [focus] }
    });
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/shex/validate")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "validate must answer 200");
    body_json(resp.into_body()).await
}

fn conforms(report: &Value) -> bool {
    report["conforms"].as_bool().unwrap_or(false)
}

fn reason(report: &Value) -> String {
    report["results"][0]["status"]["NonConformant"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

const EX: &str = "http://example.org/";

#[tokio::test]
async fn cardinality_required_property_present_and_missing() {
    let (state, token) = seeded();
    let schema = "ex:Named { ex:name xsd:string }";
    let ok = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Named"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(conforms(&ok), "ada has exactly one name: {ok}");

    let missing = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Named"),
        &format!("{EX}cy"),
    )
    .await;
    assert!(!conforms(&missing), "cy has no name: {missing}");

    // Default cardinality is exactly one; dee has two names.
    let two = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Named"),
        &format!("{EX}dee"),
    )
    .await;
    assert!(
        !conforms(&two),
        "dee has two names, default cardinality is 1: {two}"
    );

    let plus = "ex:MultiNamed { ex:name xsd:string + }";
    let ok2 = validate(
        &state,
        &token,
        plus,
        &format!("{EX}MultiNamed"),
        &format!("{EX}dee"),
    )
    .await;
    assert!(conforms(&ok2), "`+` admits two names: {ok2}");
}

#[tokio::test]
async fn datatype_constraint() {
    let (state, token) = seeded();
    let schema = "ex:Aged { ex:age xsd:integer }";
    let ok = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Aged"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(conforms(&ok), "36 is an xsd:integer: {ok}");
    let bad = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Aged"),
        &format!("{EX}cy"),
    )
    .await;
    assert!(
        !conforms(&bad),
        "\"thirty\"^^xsd:string is not an integer: {bad}"
    );
}

#[tokio::test]
async fn node_kind_constraint() {
    let (state, token) = seeded();
    let schema = "ex:Social { ex:knows IRI }";
    let ok = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Social"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(conforms(&ok), "ada knows an IRI: {ok}");

    let lit = "ex:Nicked { ex:nick IRI }";
    let bad = validate(
        &state,
        &token,
        lit,
        &format!("{EX}Nicked"),
        &format!("{EX}cy"),
    )
    .await;
    assert!(!conforms(&bad), "a literal nick is not an IRI: {bad}");
}

#[tokio::test]
async fn value_set_constraint() {
    let (state, token) = seeded();
    let schema = "ex:Current { ex:status [ex:Active ex:Inactive] }";
    let ok = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Current"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(conforms(&ok), "Active is in the set: {ok}");
    let bad = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Current"),
        &format!("{EX}bob"),
    )
    .await;
    assert!(!conforms(&bad), "Retired is not in the set: {bad}");
}

#[tokio::test]
async fn string_facets() {
    let (state, token) = seeded();
    let schema = "ex:ShortName { ex:name xsd:string MINLENGTH 1 MAXLENGTH 2 }";
    let bad = validate(
        &state,
        &token,
        schema,
        &format!("{EX}ShortName"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(!conforms(&bad), "\"Ada\" is longer than 2: {bad}");
    assert!(
        reason(&bad).contains("long"),
        "the reason names the violated facet: {}",
        reason(&bad)
    );
}

/// PATTERN is a regular expression, anchored where the pattern says so. It used
/// to be a substring test, which both rejected conforming anchored matches and
/// accepted mid-string ones.
#[tokio::test]
async fn pattern_facet_is_a_regex() {
    let (state, token) = seeded();
    let schema = r#"ex:Mail { ex:email xsd:string /^[^@]+@[^@]+\.[a-z]+$/ }"#;
    let ok = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Mail"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(conforms(&ok), "ada@example.org matches: {ok}");
    let bad = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Mail"),
        &format!("{EX}bob"),
    )
    .await;
    assert!(!conforms(&bad), "\"not-an-email\" must not match: {bad}");
}

/// Numeric facets were parsed nowhere and evaluated nowhere.
#[tokio::test]
async fn numeric_facets() {
    let (state, token) = seeded();
    let schema = "ex:Adult { ex:age xsd:integer MININCLUSIVE 18 MAXINCLUSIVE 120 }";
    let ok = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Adult"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(conforms(&ok), "36 is within [18, 120]: {ok}");
    let young = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Adult"),
        &format!("{EX}dee"),
    )
    .await;
    assert!(!conforms(&young), "12 is below 18: {young}");
    let old = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Adult"),
        &format!("{EX}bob"),
    )
    .await;
    assert!(!conforms(&old), "200 is above 120: {old}");
}

#[tokio::test]
async fn closed_shape_rejects_extra_properties() {
    let (state, token) = seeded();
    // ada also has ex:age, ex:email, ex:status, ex:knows and rdf:type.
    let schema = "ex:OnlyName CLOSED { ex:name xsd:string }";
    let bad = validate(
        &state,
        &token,
        schema,
        &format!("{EX}OnlyName"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(
        !conforms(&bad),
        "a CLOSED shape rejects ada's extra properties: {bad}"
    );

    // EXTRA does not open a CLOSED shape: an arc whose predicate no triple
    // constraint names is never allowed in one (ShEx 2.1 §5.5.2) …
    let extra_only =
        "ex:NameAndType CLOSED EXTRA <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
                      { ex:name xsd:string ; ex:age xsd:integer ; ex:email xsd:string ; \
                        ex:status IRI ; ex:knows IRI }";
    let bad = validate(
        &state,
        &token,
        extra_only,
        &format!("{EX}NameAndType"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(!conforms(&bad), "rdf:type is named by no constraint: {bad}");
    // … EXTRA lets values a constraint does not accept through.
    let schema2 = "ex:NameAndType CLOSED EXTRA a \
                   { a [ex:Robot] ? ; ex:name xsd:string ; ex:age xsd:integer ; \
                     ex:email xsd:string ; ex:status IRI ; ex:knows IRI }";
    let ok = validate(
        &state,
        &token,
        schema2,
        &format!("{EX}NameAndType"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(
        conforms(&ok),
        "ex:Person is not a Robot, and rdf:type is EXTRA: ada conforms: {ok}"
    );
}

#[tokio::test]
async fn shape_reference_is_followed() {
    let (state, token) = seeded();
    let schema = "ex:Person { ex:name xsd:string }\n\
                  ex:Connected { ex:knows @ex:Person }";
    let ok = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Connected"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(conforms(&ok), "ada knows bob, who has a name: {ok}");
    // cy knows ex:nobody, which has no name.
    let bad = validate(
        &state,
        &token,
        schema,
        &format!("{EX}Connected"),
        &format!("{EX}cy"),
    )
    .await;
    assert!(
        !conforms(&bad),
        "cy's acquaintance does not conform to Person: {bad}"
    );
}

#[tokio::test]
async fn unparseable_schema_is_a_400_not_a_pass() {
    let (state, token) = seeded();
    let body = json!({ "schema": "this is not shexc {{{", "shape_map": {} });
    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/shex/validate")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// POST any body to `/api/shex/validate`.
async fn post(state: &AppState, token: &str, body: Value) -> (StatusCode, Value) {
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/shex/validate")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes).to_string();
    (
        status,
        serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text })),
    )
}

fn statuses(report: &Value) -> Vec<(String, bool)> {
    report["results"]
        .as_array()
        .map(|rs| {
            rs.iter()
                .map(|r| {
                    (
                        r["focus_node"].as_str().unwrap_or("").to_string(),
                        r["status"].as_str() == Some("Conformant"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A triple whose predicate the shape constrains but whose value fails the
/// constraint is not ignored: it fails the shape (the partition must place
/// it), unless the predicate is EXTRA. The old engine skipped such triples.
#[tokio::test]
async fn a_constrained_predicate_with_a_bad_value_fails_unless_extra() {
    let (state, token) = seeded();
    // bob's status is ex:Retired.
    let schema = "ex:S { ex:status [ex:Active] }";
    let bad = validate(
        &state,
        &token,
        schema,
        &format!("{EX}S"),
        &format!("{EX}bob"),
    )
    .await;
    assert!(
        !conforms(&bad),
        "Retired fails [Active] and cannot stay unmatched: {bad}"
    );
    let extra = "ex:S EXTRA ex:status { ex:status [ex:Active] ? }";
    let ok = validate(
        &state,
        &token,
        extra,
        &format!("{EX}S"),
        &format!("{EX}bob"),
    )
    .await;
    assert!(
        conforms(&ok),
        "EXTRA lets the non-matching status stay unmatched: {ok}"
    );
    // A triple that *does* match may not be left over, EXTRA or not.
    let greedy = "ex:S EXTRA ex:name { ex:name . {1} }";
    let two = validate(
        &state,
        &token,
        greedy,
        &format!("{EX}S"),
        &format!("{EX}dee"),
    )
    .await;
    assert!(
        !conforms(&two),
        "both names match, only one may be matched: {two}"
    );
}

/// One triple cannot satisfy two constraints: `ex:name . ; ex:name .` needs
/// two names.
#[tokio::test]
async fn one_triple_matches_one_constraint() {
    let (state, token) = seeded();
    let schema = "ex:TwoNames { ex:name . ; ex:name . }";
    let ada = validate(
        &state,
        &token,
        schema,
        &format!("{EX}TwoNames"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(!conforms(&ada), "ada has one name: {ada}");
    let dee = validate(
        &state,
        &token,
        schema,
        &format!("{EX}TwoNames"),
        &format!("{EX}dee"),
    )
    .await;
    assert!(conforms(&dee), "dee has two: {dee}");
}

/// `CLOSED {}` admits no arcs out, and `{m,}` is unbounded (it parsed as
/// exactly m).
#[tokio::test]
async fn closed_empty_shape_and_open_ranges() {
    let (state, token) = seeded();
    let closed = validate(
        &state,
        &token,
        "ex:Nothing CLOSED {}",
        &format!("{EX}Nothing"),
        &format!("{EX}ada"),
    )
    .await;
    assert!(
        !conforms(&closed),
        "CLOSED {{}} rejects any property: {closed}"
    );
    let open = validate(
        &state,
        &token,
        "ex:Names { ex:name . {1,} }",
        &format!("{EX}Names"),
        &format!("{EX}dee"),
    )
    .await;
    assert!(conforms(&open), "{{1,}} allows two names: {open}");
}

/// Recursion is a greatest fixpoint: a cycle of acquaintances conforms when
/// every member does, and a member that fails takes its referrers with it.
#[tokio::test]
async fn recursive_shapes_are_a_greatest_fixpoint() {
    let (state, token) = admin_state();
    state
        .store
        .load_str(
            "@prefix ex: <http://example.org/> .\n\
             ex:a ex:name \"a\" ; ex:knows ex:b . ex:b ex:name \"b\" ; ex:knows ex:c .\n\
             ex:c ex:name \"c\" ; ex:knows ex:a .\n\
             ex:x ex:name \"x\" ; ex:knows ex:y . ex:y ex:knows ex:x .",
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();
    // y has no name, so y fails, and x, who knows y, fails with it.
    let schema = format!("{PREFIXES}ex:P {{ ex:name . ; ex:knows @ex:P * }}");
    let (st, report) = post(
        &state,
        &token,
        json!({ "schema": schema, "shape_map": "ex:a@ex:P, ex:x@ex:P" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{report}");
    assert_eq!(
        statuses(&report),
        vec![(format!("{EX}a"), true), (format!("{EX}x"), false)],
        "{report}"
    );
}

/// The negation requirement: a cycle through NOT (or an EXTRA predicate) is
/// not a schema, so the request is a 400 rather than an arbitrary verdict.
#[tokio::test]
async fn a_cycle_through_negation_is_rejected() {
    let (state, token) = seeded();
    for schema in [
        "ex:S { ex:knows NOT @ex:S }",
        "ex:S EXTRA ex:knows { ex:knows @ex:S }",
        "ex:S { ex:knows @ex:T } ex:T { ex:knows NOT @ex:S }",
    ] {
        let (st, body) = post(
            &state,
            &token,
            json!({ "schema": format!("{PREFIXES}{schema}"), "shape_map": {} }),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{schema}: {body}");
        assert!(body.to_string().contains("negation"), "{schema}: {body}");
    }
    // Negating a lower stratum is fine.
    let (st, report) = post(
        &state,
        &token,
        json!({
            "schema": format!("{PREFIXES}ex:Named {{ ex:name . }} ex:Anon NOT @ex:Named"),
            "shape_map": "ex:cy@ex:Anon, ex:ada@ex:Anon"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{report}");
    assert_eq!(
        statuses(&report),
        vec![(format!("{EX}cy"), true), (format!("{EX}ada"), false)]
    );
}

/// The ShapeMap language: prefixed names from the schema, `{FOCUS p o}`
/// selectors read from the data, and `@START`.
#[tokio::test]
async fn shape_map_language_and_start() {
    let (state, token) = seeded();
    let schema = format!("{PREFIXES}start = @ex:Named\nex:Named {{ ex:name xsd:string + }}");
    let (st, report) = post(
        &state,
        &token,
        json!({ "schema": schema, "shape_map": "{FOCUS ex:status _}@ex:Named, ex:cy@START" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{report}");
    let mut got = statuses(&report);
    got.sort();
    assert_eq!(
        got,
        vec![
            (format!("{EX}ada"), true),
            (format!("{EX}bob"), true),
            (format!("{EX}cy"), false),
        ],
        "{report}"
    );
    assert_eq!(report["results"][2]["shape"], "START", "{report}");
    let (st, body) = post(
        &state,
        &token,
        json!({ "schema": schema, "shape_map": "ex:ada@" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "a malformed shape map: {body}");
}

/// ShExJ is accepted as an object or as text, with the same verdicts.
#[tokio::test]
async fn shexj_schemas() {
    let (state, token) = seeded();
    let shexj = json!({
        "type": "Schema",
        "shapes": [{
            "type": "ShapeDecl", "id": format!("{EX}Named"),
            "shapeExpr": { "type": "Shape", "expression": {
                "type": "TripleConstraint", "predicate": format!("{EX}name"),
                "valueExpr": { "type": "NodeConstraint", "datatype": "http://www.w3.org/2001/XMLSchema#string" }
            } }
        }]
    });
    for schema in [shexj.clone(), Value::String(shexj.to_string())] {
        let (st, report) = post(
            &state,
            &token,
            json!({ "schema": schema, "shape_map": [{ "node": format!("{EX}ada"), "shape": format!("{EX}Named") }, { "node": format!("{EX}cy"), "shape": format!("{EX}Named") }] }),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{report}");
        assert_eq!(
            statuses(&report),
            vec![(format!("{EX}ada"), true), (format!("{EX}cy"), false)]
        );
    }
}

/// IMPORT reads a ShExR schema from a named graph of the store, and nothing
/// from the network: an IRI that is no readable graph is a 400.
#[tokio::test]
async fn imports_come_from_the_store_only() {
    let (state, token) = seeded();
    let lib = "http://example.org/schemas/person";
    state
        .store
        .load_str(
            "PREFIX sx: <http://www.w3.org/ns/shex#>\n\
             PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\
             [] a sx:Schema ; sx:shapes ( <http://example.org/Named> ) .\n\
             <http://example.org/Named> a sx:ShapeDecl ; sx:shapeExpr [ a sx:Shape ;\n\
               sx:expression [ a sx:TripleConstraint ; sx:predicate <http://example.org/name> ;\n\
                 sx:valueExpr [ a sx:NodeConstraint ; sx:datatype xsd:string ] ] ] .",
            oxigraph::io::RdfFormat::Turtle,
            Some(lib),
        )
        .unwrap();
    let schema = format!("{PREFIXES}IMPORT <{lib}>\nex:Friendly {{ ex:knows @ex:Named }}");
    let (st, report) = post(
        &state,
        &token,
        json!({ "schema": schema, "shape_map": "ex:ada@ex:Friendly, ex:cy@ex:Friendly" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{report}");
    assert_eq!(
        statuses(&report),
        vec![(format!("{EX}ada"), true), (format!("{EX}cy"), false)]
    );

    let remote = format!("{PREFIXES}IMPORT <https://shex.example/remote>\nex:S {{ }}");
    let (st, body) = post(&state, &token, json!({ "schema": remote, "shape_map": {} })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("never fetched"), "{body}");
}

/// Only the Test extension is evaluated; other actions' code is never run.
#[tokio::test]
async fn semantic_actions_run_only_the_test_extension() {
    let (state, token) = seeded();
    let shape = format!("{EX}S");
    let focus = format!("{EX}ada");
    let fail = r#"ex:S { ex:name . %<http://shex.io/extensions/Test/>{ fail(s) %} }"#;
    let r = validate(&state, &token, fail, &shape, &focus).await;
    assert!(!conforms(&r), "a Test fail() fails the constraint: {r}");
    let print = r#"ex:S { ex:name . %<http://shex.io/extensions/Test/>{ print(o) %} }"#;
    let r = validate(&state, &token, print, &shape, &focus).await;
    assert!(conforms(&r), "print() succeeds: {r}");
    let other = r#"ex:S { ex:name . %<http://example.org/js>{ throw new Error("ran") %} }"#;
    let r = validate(&state, &token, other, &shape, &focus).await;
    assert!(
        conforms(&r),
        "another extension's code is not executed: {r}"
    );
}

/// ShEx 2.next is refused by name, not misread as 2.1.
#[tokio::test]
async fn shex_next_syntax_is_refused() {
    let (state, token) = seeded();
    for schema in [
        "ex:A { ex:name . } ex:B EXTENDS @ex:A { }",
        "ABSTRACT ex:A { ex:name . }",
    ] {
        let (st, body) = post(
            &state,
            &token,
            json!({ "schema": format!("{PREFIXES}{schema}"), "shape_map": {} }),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{schema}: {body}");
        assert!(body.to_string().contains("2.next"), "{schema}: {body}");
    }
}

/// Validation recurses through the data. A long chain of references is
/// validated (on a thread with room for it); one deeper than the engine's
/// limit is a 422 — an error, not a verdict, and not a crashed server.
#[tokio::test]
async fn deep_reference_chains_are_validated_or_refused() {
    let (state, token) = admin_state();
    let chain = |n: usize, prefix: &str| {
        let mut ttl = String::from("@prefix ex: <http://example.org/> .\n");
        for i in 0..n {
            ttl.push_str(&format!("ex:{prefix}{i} ex:next ex:{prefix}{} .\n", i + 1));
        }
        ttl
    };
    let limit = open_triplestore::shex::validate::MAX_DEPTH;
    for (n, p) in [(5_000, "a"), (limit + 10, "b")] {
        state
            .store
            .load_str(&chain(n, p), oxigraph::io::RdfFormat::Turtle, None)
            .unwrap();
    }
    let schema = format!("{PREFIXES}ex:Chain {{ ex:next @ex:Chain ? }}");
    let (st, report) = post(
        &state,
        &token,
        json!({ "schema": schema, "shape_map": "ex:a0@ex:Chain" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{report}");
    assert_eq!(statuses(&report), vec![(format!("{EX}a0"), true)]);

    let (st, body) = post(
        &state,
        &token,
        json!({ "schema": schema, "shape_map": "ex:b0@ex:Chain" }),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.to_string().contains("deeper than"), "{body}");
}
