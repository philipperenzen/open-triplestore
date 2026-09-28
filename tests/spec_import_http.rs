//! Constraint-specification import (6.3): an IDS document becomes a SHACL
//! Studio shape graph over HTTP, and the generated shapes catch a violating
//! wall in IFC-shaped RDF while passing a conforming one.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use oxigraph::io::RdfFormat;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const IDS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ids:ids xmlns:ids="http://standards.buildingsmart.org/IDS" xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <ids:info><ids:title>Wall fire ratings</ids:title></ids:info>
  <ids:specifications>
    <ids:specification name="External walls need a fire rating" ifcVersion="IFC4">
      <ids:applicability>
        <ids:entity><ids:name><ids:simpleValue>IFCWALL</ids:simpleValue></ids:name></ids:entity>
        <ids:property dataType="IFCBOOLEAN"><ids:propertySet><ids:simpleValue>Pset_WallCommon</ids:simpleValue></ids:propertySet><ids:baseName><ids:simpleValue>IsExternal</ids:simpleValue></ids:baseName><ids:value><ids:simpleValue>true</ids:simpleValue></ids:value></ids:property>
      </ids:applicability>
      <ids:requirements>
        <ids:property cardinality="required" dataType="IFCLABEL"><ids:propertySet><ids:simpleValue>Pset_WallCommon</ids:simpleValue></ids:propertySet><ids:baseName><ids:simpleValue>FireRating</ids:simpleValue></ids:baseName><ids:value><xs:restriction base="xs:string"><xs:enumeration value="REI30"/><xs:enumeration value="REI60"/></xs:restriction></ids:value></ids:property>
      </ids:requirements>
    </ids:specification>
  </ids:specifications>
</ids:ids>"#;

const DATA_GRAPH: &str = "urn:ids:data";

/// IFC-shaped RDF as the built-in importer emits it: an external wall with a
/// valid rating, an external wall without one, and an internal wall (out of
/// scope of the requirement).
const DATA: &str = r#"
@prefix ifc: <https://standards.buildingsmart.org/IFC/DEV/IFC4/ADD2_TC1/OWL#> .
@prefix props: <https://w3id.org/props#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
<urn:w:1> a ifc:IfcWall ; props:ifcName "W1" ; props:Pset_WallCommon_IsExternal true ; props:Pset_WallCommon_FireRating "REI60" .
<urn:w:2> a ifc:IfcWall ; props:ifcName "W2" ; props:Pset_WallCommon_IsExternal true .
<urn:w:3> a ifc:IfcWall ; props:ifcName "W3" ; props:Pset_WallCommon_IsExternal false .
"#;

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    ct: &str,
    body: &str,
) -> (StatusCode, Value, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, ct)
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let st = resp.status();
    let text = body_text(resp.into_body()).await;
    (st, serde_json::from_str(&text).unwrap_or(Value::Null), text)
}

#[tokio::test]
async fn ids_import_creates_a_shape_graph_whose_shapes_validate_ifc_rdf() {
    let (state, token) = admin_state();
    state
        .store
        .load_str(DATA, RdfFormat::Turtle, Some(DATA_GRAPH))
        .unwrap();
    let app = test_app(state);

    // The registry lists IDS.
    let (st, v, txt) = send(
        &app,
        Method::GET,
        "/api/shacl/importers",
        &token,
        "application/json",
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(
        v.as_array().unwrap().iter().any(|i| i["id"] == "ids"),
        "{txt}"
    );

    // Unknown format → 404 naming the known ones; garbage → 422.
    let (st, _, txt) = send(
        &app,
        Method::POST,
        "/api/shacl/import/nope",
        &token,
        "application/xml",
        IDS,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{txt}");
    assert!(txt.contains("ids"), "{txt}");
    let (st, _, txt) = send(
        &app,
        Method::POST,
        "/api/shacl/import/ids",
        &token,
        "application/xml",
        "<root/>",
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{txt}");

    // Convert only.
    let (st, v, txt) = send(
        &app,
        Method::POST,
        "/api/shacl/import/ids",
        &token,
        "application/xml",
        IDS,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["title"], "Wall fire ratings");
    assert!(
        v["turtle"]
            .as_str()
            .unwrap()
            .contains("sh:targetClass ifc:IfcWall"),
        "{txt}"
    );
    assert!(v["shape_graph"].is_null());

    // Convert and create in SHACL Studio.
    let (st, v, txt) = send(
        &app,
        Method::POST,
        "/api/shacl/import/ids?create=true",
        &token,
        "application/xml",
        IDS,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let sg = v["shape_graph"]["id"]
        .as_str()
        .expect("shape graph id")
        .to_string();
    assert_eq!(v["shape_graph"]["source"], "imported", "{txt}");
    let (st, g, txt) = send(
        &app,
        Method::GET,
        &format!("/api/shacl/shape-graphs/{sg}"),
        &token,
        "application/json",
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(g["name"], "Wall fire ratings");

    // The generated shapes, run as a Studio pipeline over the IFC-shaped data:
    // exactly the external wall without a rating is a violation.
    let (st, p, txt) = send(
        &app,
        Method::POST,
        "/api/shacl/pipelines",
        &token,
        "application/json",
        &json!({ "name": "ids-check", "targets": [{ "kind": "graph", "id": DATA_GRAPH }], "shape_graph_ids": [sg] }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let pid = p["id"].as_str().unwrap().to_string();
    let (st, run, txt) = send(
        &app,
        Method::POST,
        &format!("/api/shacl/pipelines/{pid}/run"),
        &token,
        "application/json",
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(run["conforms"], false, "W2 lacks a fire rating: {txt}");
    assert_eq!(
        run["violation_count"], 1,
        "only the external wall without a rating: {txt}"
    );
    let report = run.to_string();
    assert!(
        report.contains("urn:w:2"),
        "the report names the violating wall: {txt}"
    );
    assert!(
        !report.contains("urn:w:3"),
        "the internal wall is out of scope: {txt}"
    );
}
