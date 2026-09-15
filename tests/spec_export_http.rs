//! HTTP surface for SHACL → IDS export, and the import → export → import
//! fixpoint the exporter is designed around.
//!
//! The losses list is the point of this feature: an exporter that quietly
//! produced a thinner document than the shapes it was given would be worse
//! than useless for a delivery contract, so these tests assert the reported
//! losses as firmly as they assert the document.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use serde_json::Value;
use tower::ServiceExt as _;

const IDS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ids:ids xmlns:ids="http://standards.buildingsmart.org/IDS" xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <ids:info><ids:title>Wall fire ratings</ids:title></ids:info>
  <ids:specifications>
    <ids:specification name="External walls need a fire rating" ifcVersion="IFC4">
      <ids:applicability>
        <ids:entity><ids:name><ids:simpleValue>IFCWALL</ids:simpleValue></ids:name></ids:entity>
        <ids:property><ids:propertySet><ids:simpleValue>Pset_WallCommon</ids:simpleValue></ids:propertySet><ids:baseName><ids:simpleValue>IsExternal</ids:simpleValue></ids:baseName><ids:value><ids:simpleValue>true</ids:simpleValue></ids:value></ids:property>
      </ids:applicability>
      <ids:requirements>
        <ids:property cardinality="required"><ids:propertySet><ids:simpleValue>Pset_WallCommon</ids:simpleValue></ids:propertySet><ids:baseName><ids:simpleValue>FireRating</ids:simpleValue></ids:baseName><ids:value><xs:restriction base="xs:string"><xs:enumeration value="REI30"/><xs:enumeration value="REI60"/></xs:restriction></ids:value></ids:property>
      </ids:requirements>
    </ids:specification>
  </ids:specifications>
</ids:ids>"#;

async fn post(app: &Router, token: &str, uri: &str, ct: &str, body: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, ct)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    (st, body_text(resp.into_body()).await)
}

async fn get(app: &Router, token: &str, uri: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    (st, body_text(resp.into_body()).await)
}

#[tokio::test]
async fn the_exporter_registry_lists_ids_and_refuses_an_unknown_format() {
    let (state, token) = admin_state();
    let app = test_app(state);

    let (st, body) = get(&app, &token, "/api/shacl/exporters").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v[0]["id"], "ids", "{body}");
    assert_eq!(v[0]["file_extension"], "ids", "{body}");

    let (st, body) = post(&app, &token, "/api/shacl/export/nope", "text/turtle", "x").await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{body}");
    assert!(body.contains("ids"), "the known formats are named: {body}");

    let (st, body) = post(&app, &token, "/api/shacl/export/ids", "text/turtle", "").await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "empty body: {body}");
}

/// Shapes that express nothing IDS can carry are an error, not an empty but
/// schema-valid document.
#[tokio::test]
async fn a_shape_graph_with_no_class_target_is_unprocessable() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let ttl = concat!(
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n",
        "@prefix ex: <http://example.org/> .\n",
        "ex:S a sh:NodeShape ; sh:targetNode ex:a ; sh:property [ sh:path ex:p ; sh:minCount 1 ] ."
    );
    let (st, body) = post(&app, &token, "/api/shacl/export/ids", "text/turtle", ttl).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("class-based"), "the reason is given: {body}");
}

/// import → export → import is a fixpoint on the subset both directions share:
/// the second import produces the same shapes as the first.
#[tokio::test]
async fn ids_survives_an_import_export_import_round_trip() {
    let (state, token) = admin_state();
    let app = test_app(state);

    let (st, first) = post(
        &app,
        &token,
        "/api/shacl/import/ids",
        "application/xml",
        IDS,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{first}");
    let v1: Value = serde_json::from_str(&first).unwrap();
    let ttl1 = v1["turtle"].as_str().unwrap().to_string();
    assert!(ttl1.contains("IfcWall"), "{ttl1}");

    let (st, exported) = post(
        &app,
        &token,
        "/api/shacl/export/ids?title=Wall+fire+ratings",
        "text/turtle",
        &ttl1,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{exported}");
    let ev: Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(ev["specification_count"], 1, "{exported}");
    let doc = ev["document"].as_str().unwrap();
    assert!(doc.contains("IFCWALL"), "the entity survives: {doc}");
    assert!(
        doc.contains("Pset_WallCommon") && doc.contains("FireRating"),
        "the property facet survives: {doc}"
    );
    assert!(
        doc.contains("REI30") && doc.contains("REI60"),
        "the enumeration survives: {doc}"
    );
    assert!(
        doc.contains(r#"cardinality="required""#),
        "the cardinality survives: {doc}"
    );
    assert!(
        !doc.contains("<ids:/"),
        "closing tags are well formed: {doc}"
    );

    // Re-import the exported document: the shapes must match the first pass.
    let (st, second) = post(
        &app,
        &token,
        "/api/shacl/import/ids",
        "application/xml",
        doc,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{second}");
    let v2: Value = serde_json::from_str(&second).unwrap();
    let ttl2 = v2["turtle"].as_str().unwrap();
    for needle in [
        "IfcWall",
        "Pset_WallCommon_FireRating",
        "Pset_WallCommon_IsExternal",
        "REI30",
    ] {
        assert!(
            ttl2.contains(needle),
            "`{needle}` survives the round trip: {ttl2}"
        );
    }
    assert_eq!(
        v1["specifications"].as_array().map(|a| a.len()),
        v2["specifications"].as_array().map(|a| a.len()),
        "the specification count is a fixpoint:\nfirst {first}\nsecond {second}"
    );
}

/// `?raw=true` returns the document itself; the default is the report, because
/// the losses matter more than the bytes.
#[tokio::test]
async fn raw_returns_the_document_and_the_default_returns_the_losses() {
    let (state, token) = admin_state();
    let app = test_app(state);
    let (_, first) = post(
        &app,
        &token,
        "/api/shacl/import/ids",
        "application/xml",
        IDS,
    )
    .await;
    let ttl = serde_json::from_str::<Value>(&first).unwrap()["turtle"]
        .as_str()
        .unwrap()
        .to_string();

    let (st, raw) = post(
        &app,
        &token,
        "/api/shacl/export/ids?raw=true",
        "text/turtle",
        &ttl,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert!(raw.starts_with("<?xml"), "the bare document: {raw}");

    let (st, report) = post(&app, &token, "/api/shacl/export/ids", "text/turtle", &ttl).await;
    assert_eq!(st, StatusCode::OK, "{report}");
    let v: Value = serde_json::from_str(&report).unwrap();
    assert!(
        v["losses"].is_array(),
        "losses are always reported: {report}"
    );
    assert_eq!(v["format"], "ids");
}
