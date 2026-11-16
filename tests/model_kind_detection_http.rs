//! The kind a registry entry gets from `POST /api/models/:id/versions`.
//!
//! Without a `kind` field the store detects it from the uploaded RDF: an OWL
//! model whose properties and value list outnumber its classes is a data
//! model, not a vocabulary. An explicit `kind` field still wins, and a later
//! version upload sets the entry's kind again.
//!
//! Drives the real Axum router via `tower::ServiceExt::oneshot`.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::data_models::registry as dmr;
use open_triplestore::kind_detector::RegistryKind;
use tower::ServiceExt as _;

/// Five classes, ten properties and a four-member SKOS value list with labels,
/// notations and definitions.
const OWL_MODEL: &str = r#"
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:   <http://example.org/asset#> .

ex: a owl:Ontology .
ex:Asset a owl:Class . ex:Component a owl:Class . ex:Location a owl:Class .
ex:Inspection a owl:Class . ex:Status a owl:Class .
ex:name a owl:DatatypeProperty ; rdfs:domain ex:Asset ; rdfs:range xsd:string .
ex:code a owl:DatatypeProperty ; rdfs:domain ex:Asset ; rdfs:range xsd:string .
ex:builtIn a owl:DatatypeProperty ; rdfs:domain ex:Asset ; rdfs:range xsd:gYear .
ex:length a owl:DatatypeProperty ; rdfs:domain ex:Component ; rdfs:range xsd:decimal .
ex:inspectedOn a owl:DatatypeProperty ; rdfs:domain ex:Inspection ; rdfs:range xsd:date .
ex:remark a owl:DatatypeProperty ; rdfs:domain ex:Inspection ; rdfs:range xsd:string .
ex:partOf a owl:ObjectProperty ; rdfs:domain ex:Component ; rdfs:range ex:Asset .
ex:locatedAt a owl:ObjectProperty ; rdfs:domain ex:Asset ; rdfs:range ex:Location .
ex:inspects a owl:ObjectProperty ; rdfs:domain ex:Inspection ; rdfs:range ex:Asset .
ex:status a owl:ObjectProperty ; rdfs:domain ex:Asset ; rdfs:range ex:Status .
ex:StatusList a skos:ConceptScheme ; skos:prefLabel "Status" .
ex:planned a ex:Status, skos:Concept ; skos:inScheme ex:StatusList ;
    skos:prefLabel "Planned" ; skos:notation "P" ; skos:definition "Not built yet." .
ex:inUse a ex:Status, skos:Concept ; skos:inScheme ex:StatusList ;
    skos:prefLabel "In use" ; skos:notation "U" ; skos:definition "In service." .
ex:closed a ex:Status, skos:Concept ; skos:inScheme ex:StatusList ;
    skos:prefLabel "Closed" ; skos:notation "C" ; skos:definition "Out of service." .
ex:removed a ex:Status, skos:Concept ; skos:inScheme ex:StatusList ;
    skos:prefLabel "Removed" ; skos:notation "R" ; skos:definition "Taken away." .
"#;

/// `multipart/form-data` with the text fields `fields` and the Turtle `file`.
fn multipart_body(boundary: &str, fields: &[(&str, &str)], file: &str) -> Vec<u8> {
    let mut out = String::new();
    for (name, value) in fields {
        out.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    out.push_str(&format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"model.ttl\"\r\n\
         Content-Type: text/turtle\r\n\r\n{file}\r\n--{boundary}--\r\n"
    ));
    out.into_bytes()
}

async fn upload(
    state: &open_triplestore::server::AppState,
    token: &str,
    fields: &[(&str, &str)],
) -> (StatusCode, serde_json::Value) {
    let boundary = "KINDBOUNDARY";
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/models/asset/versions")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(multipart_body(boundary, fields, OWL_MODEL)))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

fn entry_kind(state: &open_triplestore::server::AppState) -> RegistryKind {
    dmr::get_data_model(&state.store, &state.base_url, "asset")
        .expect("entry exists")
        .kind
}

#[tokio::test]
async fn an_owl_model_is_detected_as_a_data_model_and_an_explicit_kind_still_wins() {
    let (state, token) = admin_state();
    dmr::insert_data_model(
        &state.store,
        &state.base_url,
        "asset",
        "Asset model",
        "http://example.org/asset#",
        None,
        false,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();

    // No kind field: detected from the content.
    let (st, body) = upload(&state, &token, &[("version", "1.0.0")]).await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(body["detected"], "data-model", "{body}");
    assert_eq!(body["evidence"]["skos_concepts"], 4, "{body}");
    assert_eq!(body["evidence"]["skos_relations"], 4, "{body}");
    assert_eq!(entry_kind(&state), RegistryKind::DataModel);

    // An explicit kind wins over the detected one …
    let (st, body) = upload(
        &state,
        &token,
        &[("version", "1.1.0"), ("kind", "vocabulary")],
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(
        body["detected"], "data-model",
        "detection is still reported"
    );
    assert_eq!(entry_kind(&state), RegistryKind::Vocabulary);

    // … and a later version sets the entry's kind again.
    let (st, body) = upload(
        &state,
        &token,
        &[("version", "1.2.0"), ("kind", "data-model")],
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(entry_kind(&state), RegistryKind::DataModel);

    // An unknown kind is refused, and changes nothing.
    let (st, _) = upload(
        &state,
        &token,
        &[("version", "1.3.0"), ("kind", "nonsense")],
    )
    .await;
    assert!(
        st.is_client_error(),
        "unknown kind must be refused, got {st}"
    );
    assert_eq!(entry_kind(&state), RegistryKind::DataModel);
}
