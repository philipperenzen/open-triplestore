//! `POST /api/shaclc/serialize` reads the graph it is told to read.
//!
//! It took an arbitrary graph IRI from the request body and handed it to the
//! serialiser with no authentication and no access check, so anyone could name
//! a private dataset's shapes graph and read it back. Confirmed against a
//! running instance before the fix: a private graph holding one `sh:NodeShape`
//! came back in full — shape IRI, target class, property path and datatype — to
//! a client with no token at all.
//!
//! A token by itself would only narrow that from everyone to every signed-in
//! user, so the handler also asks the same visibility helper that gates
//! `/store` and `/sparql`.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use oxigraph::io::RdfFormat;
use tower::ServiceExt as _;

/// A shapes graph nobody but its owner should see.
const PRIVATE_SHAPES: &str = "urn:test:private:shapes";
/// …and one that belongs to a public dataset.
const PUBLIC_SHAPES: &str = "urn:test:public:shapes";

const SHAPE_TTL: &str = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
<urn:test:SalaryShape> a sh:NodeShape ;
  sh:targetClass <urn:test:ConfidentialPayroll> ;
  sh:property [ sh:path <urn:test:amount> ; sh:datatype xsd:integer ] .
"#;

async fn serialize(app: &axum::Router, token: Option<&str>, graph: &str) -> (StatusCode, String) {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri("/api/shaclc/serialize")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let body = format!(r#"{{"shapesGraphIri":"{graph}"}}"#);
    let resp = app
        .clone()
        .oneshot(b.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

/// The leak: an anonymous caller naming a private graph.
#[tokio::test]
async fn an_anonymous_caller_cannot_serialise_a_private_shapes_graph() {
    let (state, _admin) = admin_state();
    state
        .store
        .graph_store_put(Some(PRIVATE_SHAPES), SHAPE_TTL, RdfFormat::Turtle)
        .unwrap();
    let app = test_app(state);

    let (status, body) = serialize(&app, None, PRIVATE_SHAPES).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED, "body was: {body}");
    assert!(
        !body.contains("ConfidentialPayroll") && !body.contains("SalaryShape"),
        "the refusal must not carry the graph it refused: {body}"
    );
}

/// A token is not enough on its own: one tenant must not read another's.
#[tokio::test]
async fn a_signed_in_stranger_cannot_serialise_someone_elses_private_graph() {
    let (state, _admin) = admin_state();
    state
        .store
        .graph_store_put(Some(PRIVATE_SHAPES), SHAPE_TTL, RdfFormat::Turtle)
        .unwrap();
    state
        .auth_db
        .create_user("out", "outsider", "out@test.com", "hash", SystemRole::User)
        .unwrap();
    let stranger = mint_token("out", "outsider", "user");
    let app = test_app(state);

    let (status, body) = serialize(&app, Some(&stranger), PRIVATE_SHAPES).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "body was: {body}");
    assert!(!body.contains("ConfidentialPayroll"), "{body}");
}

/// And the endpoint still does its job for a graph the caller may read.
#[tokio::test]
async fn a_reader_of_the_graph_still_gets_its_shaclc() {
    let (state, admin) = admin_state();
    let ds = state
        .auth_db
        .create_dataset(
            "public-shapes",
            "Public shapes",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph(&ds.id, PUBLIC_SHAPES)
        .unwrap();
    state
        .store
        .graph_store_put(Some(PUBLIC_SHAPES), SHAPE_TTL, RdfFormat::Turtle)
        .unwrap();
    let app = test_app(state);

    let (status, body) = serialize(&app, Some(&admin), PUBLIC_SHAPES).await;

    assert_eq!(status, StatusCode::OK, "body was: {body}");
    assert!(
        body.contains("SalaryShape") && body.contains("ConfidentialPayroll"),
        "the shapes should serialise for someone allowed to read them: {body}"
    );
}

/// A graph that does not exist answers exactly like one the caller may not
/// read, so the endpoint cannot be used to discover which graph IRIs exist.
#[tokio::test]
async fn a_missing_graph_is_indistinguishable_from_a_forbidden_one() {
    let (state, _admin) = admin_state();
    state
        .store
        .graph_store_put(Some(PRIVATE_SHAPES), SHAPE_TTL, RdfFormat::Turtle)
        .unwrap();
    state
        .auth_db
        .create_user("out", "outsider", "out@test.com", "hash", SystemRole::User)
        .unwrap();
    let stranger = mint_token("out", "outsider", "user");
    let app = test_app(state);

    let (existing, _) = serialize(&app, Some(&stranger), PRIVATE_SHAPES).await;
    let (absent, _) = serialize(&app, Some(&stranger), "urn:test:no:such:graph").await;

    assert_eq!(
        existing, absent,
        "a graph that exists but is private must answer like one that does not exist"
    );
}
