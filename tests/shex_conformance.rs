//! ShEx conformance — semantics over HTTP, not wiring.
//!
//! The only prior coverage of `/api/shex/validate` was a smoke test that
//! accepted HTTP 200 *or* 400, which passes whether validation works, fails or
//! rejects every request. These tests assert what the validator decides:
//! conforming and non-conforming focus nodes across cardinality, datatype,
//! node kind, value sets, string and numeric facets, regex patterns, CLOSED
//! shapes and shape references.

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
    let schema = r#"ex:Mail { ex:email xsd:string PATTERN "^[^@]+@[^@]+\.[a-z]+$" }"#;
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

    // EXTRA lets a named property through.
    let schema2 = "ex:NameAndType CLOSED EXTRA <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
                   { ex:name xsd:string ; ex:age xsd:integer ; ex:email xsd:string ; \
                     ex:status IRI ; ex:knows IRI }";
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
        "with every property listed (and rdf:type EXTRA) ada conforms: {ok}"
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
