//! Dry-run and mapping gates over HTTP, against the SQLite connector in core.
//!
//! Acceptance checks (written before the implementation):
//! * a dry-run on one row of a child table pulls in the referenced parent row
//!   and reports zero systematic defects for a sound mapping;
//! * a deliberately wrong datatype in the mapping is reported as a systematic
//!   defect (≥ 90 % of the type's subjects) and not as data issues;
//! * a violation on one row is a data issue, and the split follows the gates
//!   config graph an administrator edits;
//! * an unregistered mapping — RML or YARRRML in the request — dry-runs
//!   without leaving anything behind;
//! * the scratch graph is readable while it lives, and the endpoints are
//!   admin-gated like the rest of `/api/sources`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::SystemRole;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const SHAPES_GRAPH: &str = "urn:shapes:dryrun-products";

fn sources_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("ots-dryrun-http-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("OTS_SOURCES_DIR", &dir);
        dir
    })
}

/// Two suppliers, three priced products. `broken` points product 2 at a
/// supplier that does not exist.
fn fresh_sqlite(name: &str, broken: bool) -> PathBuf {
    let path = sources_dir().join(format!("{name}-{}.db", uuid::Uuid::new_v4()));
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "CREATE TABLE suppliers (supplier_id INTEGER PRIMARY KEY, name TEXT NOT NULL);
         CREATE TABLE products (
             product_id INTEGER PRIMARY KEY,
             name TEXT NOT NULL,
             price REAL NOT NULL,
             supplier_id INTEGER
         );
         INSERT INTO suppliers VALUES (10, 'Acme'), (20, 'Globex');
         INSERT INTO products VALUES
           (1, 'Bolt', 0.25, 10),
           (2, 'Nut', 0.10, {}),
           (3, 'Washer', 0.05, 20);",
        if broken { 99 } else { 10 }
    ))
    .unwrap();
    path
}

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = if body.is_null() {
        Body::empty()
    } else {
        b = b.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

async fn put_turtle(app: &Router, token: &str, graph: &str, turtle: &str) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/store?graph={}", url_encode(graph)))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "text/turtle")
                .body(Body::from(turtle.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success(), "PUT {graph}: {}", resp.status());
}

async fn get_graph(app: &Router, token: &str, graph: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/store?graph={}", url_encode(graph)))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::ACCEPT, "text/turtle")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

const SHAPES: &str = r#"
@prefix sh:  <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:  <http://example.org/products/ontology#> .

ex:ProductShape a sh:NodeShape ;
  sh:targetClass ex:Product ;
  sh:property [ sh:path ex:hasPrice ; sh:datatype xsd:decimal ; sh:minCount 1 ] ;
  sh:property [ sh:path ex:suppliedBy ; sh:class ex:Supplier ; sh:minCount 1 ] .
"#;

/// `PRICE_DATATYPE` is `xsd:decimal` for the sound mapping and `xsd:string`
/// for the deliberately wrong one.
const MAPPING: &str = r#"
@prefix rr:  <http://www.w3.org/ns/r2rml#> .
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix ql:  <http://semweb.mmlab.be/ns/ql#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:  <http://example.org/products/ontology#> .

ex:ProductsMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:SOURCE> ; rml:referenceFormulation ql:SQL2008 ; rr:tableName "products" ] ;
  rr:subjectMap [ rr:template "http://example.org/products/product_{product_id}" ; rr:class ex:Product ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:hasPrice ; rr:objectMap [ rr:column "price" ; rr:datatype PRICE_DATATYPE ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:suppliedBy ;
    rr:objectMap [ rr:parentTriplesMap ex:SuppliersMap ; rr:joinCondition [ rr:child "supplier_id" ; rr:parent "supplier_id" ] ] ] .

ex:SuppliersMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:SOURCE> ; rml:referenceFormulation ql:SQL2008 ; rr:tableName "suppliers" ] ;
  rr:subjectMap [ rr:template "http://example.org/products/supplier_{supplier_id}" ; rr:class ex:Supplier ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] .
"#;

fn mapping_for(source_id: &str, price_datatype: &str) -> String {
    MAPPING
        .replace("SOURCE", source_id)
        .replace("PRICE_DATATYPE", price_datatype)
}

async fn register_source(app: &Router, token: &str, id: &str, db: &Path) {
    let (st, _, txt) = req(
        app,
        Method::POST,
        "/api/sources",
        token,
        json!({
            "id": id, "name": id, "dialect": "sqlite",
            "database": db.to_string_lossy(), "readOnly": true, "statementTimeoutMs": 5000,
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register {id}: {txt}");
}

async fn register_mapping(app: &Router, token: &str, id: &str, _source_id: &str, rml: &str) {
    let (st, _, txt) = req(
        app,
        Method::POST,
        "/api/mappings",
        token,
        json!({ "id": id, "title": id, "rml": rml, "shapesGraph": SHAPES_GRAPH }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register mapping {id}: {txt}");
}

async fn dry_run(
    app: &Router,
    token: &str,
    source: &str,
    body: Value,
) -> (StatusCode, Value, String) {
    req(
        app,
        Method::POST,
        &format!("/api/sources/{source}/dry-run"),
        token,
        body,
    )
    .await
}

fn map_entry<'a>(v: &'a Value, local: &str) -> &'a Value {
    v["maps"]
        .as_array()
        .expect("maps")
        .iter()
        .find(|m| m["triplesMap"] == format!("http://example.org/products/ontology#{local}"))
        .unwrap_or_else(|| panic!("no entry for {local}: {v}"))
}

// ───────────────────────── The acceptance checks ─────────────────────────

#[tokio::test]
async fn one_row_of_a_child_table_pulls_in_its_parent_and_a_sound_mapping_has_no_defects() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("sound", false);
    register_source(&app, &token, "sound", &db).await;
    put_turtle(&app, &token, SHAPES_GRAPH, SHAPES).await;
    register_mapping(
        &app,
        &token,
        "sound-map",
        "sound",
        &mapping_for("sound", "xsd:decimal"),
    )
    .await;

    let (st, v, txt) = dry_run(
        &app,
        &token,
        "sound",
        json!({ "mapping": "sound-map", "table": "products", "sampleSize": 1 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");

    // One product row was sampled; the supplier it references came with it,
    // even though the supplier table was not sampled at all.
    assert_eq!(v["sampleSize"], 1, "{txt}");
    assert_eq!(map_entry(&v, "ProductsMap")["sampledRows"], 1, "{txt}");
    assert_eq!(map_entry(&v, "SuppliersMap")["sampledRows"], 0, "{txt}");
    assert_eq!(map_entry(&v, "SuppliersMap")["pulledInRows"], 1, "{txt}");
    assert_eq!(v["rows"], 2, "{txt}");
    assert!(v["triples"].as_u64().unwrap() >= 6, "{txt}");

    // Validated against the mapping's shapes graph: the `sh:class ex:Supplier`
    // on the reference holds because the parent is there.
    assert_eq!(v["shapesGraphs"], json!([SHAPES_GRAPH]), "{txt}");
    assert_eq!(v["report"]["conforms"], true, "{txt}");
    assert_eq!(v["report"]["resultsCount"], 0, "{txt}");
    assert!(
        v["classification"]["mappingDefects"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{txt}"
    );
    assert!(
        v["classification"]["dataIssues"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{txt}"
    );
    assert_eq!(
        v["classification"]["systematicShare"], 0.9,
        "the rule applied: {txt}"
    );
    assert_eq!(v["classification"]["systematicMinSubjects"], 2, "{txt}");

    // Per-entity Turtle, with the entity's types and its (absent) violations.
    let entities = v["entities"].as_array().expect("entities");
    let product = entities
        .iter()
        .find(|e| e["subject"] == "http://example.org/products/product_1")
        .unwrap_or_else(|| panic!("product_1 described: {txt}"));
    assert_eq!(
        product["types"],
        json!(["http://example.org/products/ontology#Product"])
    );
    assert!(
        product["turtle"].as_str().unwrap().contains("Bolt"),
        "{txt}"
    );
    assert!(product["violations"].as_array().unwrap().is_empty());
    assert!(
        entities
            .iter()
            .any(|e| e["subject"] == "http://example.org/products/supplier_10"),
        "the pulled-in supplier is an entity too: {txt}"
    );
    assert_eq!(entities.len(), 2, "{txt}");

    // The scratch graph is a real graph while it lives, and the response says
    // for how long.
    let graph = v["graph"].as_str().unwrap();
    assert!(graph.starts_with("urn:dryrun:"), "{txt}");
    assert!(
        v["expiresAt"].as_str().is_some_and(|s| s.contains('T')),
        "{txt}"
    );
    let (st, ttl) = get_graph(&app, &token, graph).await;
    assert_eq!(st, StatusCode::OK, "{ttl}");
    assert!(
        ttl.contains("product_1") && ttl.contains("supplier_10"),
        "{ttl}"
    );
    assert_eq!(v["mapping"]["id"], "sound-map");
    assert_eq!(v["mapping"]["version"], 1);
}

#[tokio::test]
async fn a_wrong_datatype_is_a_mapping_defect_not_a_set_of_data_issues() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("wrong", false);
    register_source(&app, &token, "wrong", &db).await;
    put_turtle(&app, &token, SHAPES_GRAPH, SHAPES).await;
    // The mapping types every price as a string; the shape wants a decimal.
    register_mapping(
        &app,
        &token,
        "wrong-map",
        "wrong",
        &mapping_for("wrong", "xsd:string"),
    )
    .await;

    let (st, v, txt) = dry_run(
        &app,
        &token,
        "wrong",
        json!({ "mapping": "wrong-map", "sampleSize": 3 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["report"]["conforms"], false, "{txt}");

    let defects = v["classification"]["mappingDefects"].as_array().unwrap();
    assert_eq!(defects.len(), 1, "one kind of defect: {txt}");
    let d = &defects[0];
    assert!(
        d["path"].as_str().unwrap().contains("hasPrice"),
        "the defect names the property: {txt}"
    );
    assert_eq!(d["affected"], 3, "{txt}");
    assert_eq!(d["population"], 3, "{txt}");
    assert_eq!(d["share"], 1.0, "{txt}");
    assert!(
        d["constraint"].as_str().unwrap().starts_with("sh:datatype"),
        "{txt}"
    );
    assert_eq!(d["focusNodes"].as_array().unwrap().len(), 3, "{txt}");
    assert!(
        v["classification"]["dataIssues"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a systematic violation is not three data issues: {txt}"
    );
    // …and every product entity carries its own violation.
    let violated = v["entities"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| !e["violations"].as_array().unwrap().is_empty())
        .count();
    assert_eq!(violated, 3, "{txt}");
}

#[tokio::test]
async fn a_violation_on_one_row_is_a_data_issue_and_the_split_follows_the_gates() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("sparse", true);
    register_source(&app, &token, "sparse", &db).await;
    put_turtle(&app, &token, SHAPES_GRAPH, SHAPES).await;
    register_mapping(
        &app,
        &token,
        "sparse-map",
        "sparse",
        &mapping_for("sparse", "xsd:decimal"),
    )
    .await;

    // Product 2 points at a supplier that does not exist: no reference is
    // produced, so `sh:minCount 1` fails for that one product.
    let body = json!({ "mapping": "sparse-map", "sampleSize": 3 });
    let (st, v, txt) = dry_run(&app, &token, "sparse", body.clone()).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["report"]["conforms"], false, "{txt}");
    assert!(
        v["classification"]["mappingDefects"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{txt}"
    );
    let issues = v["classification"]["dataIssues"].as_array().unwrap();
    assert_eq!(issues.len(), 1, "{txt}");
    assert_eq!(issues[0]["affected"], 1, "{txt}");
    assert_eq!(issues[0]["population"], 3, "{txt}");
    assert_eq!(
        issues[0]["focusNodes"],
        json!(["http://example.org/products/product_2"]),
        "{txt}"
    );

    // Lower the bar in the gates graph and the same violation is systematic.
    let (st, g, txt) = req(
        &app,
        Method::PUT,
        "/api/sources/gates",
        &token,
        json!({ "systematicShare": 0.3, "systematicMinSubjects": 1 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(g["source"], "configured", "{txt}");
    assert_eq!(g["systematicShare"], 0.3, "{txt}");
    assert_eq!(g["autoThreshold"], 0.9, "the rest keeps its default: {txt}");

    let (st, v, txt) = dry_run(&app, &token, "sparse", body).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["classification"]["systematicShare"], 0.3, "{txt}");
    assert_eq!(
        v["classification"]["mappingDefects"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{txt}"
    );
    assert!(
        v["classification"]["dataIssues"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{txt}"
    );
}

#[tokio::test]
async fn an_unregistered_mapping_dry_runs_without_leaving_anything_behind() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("inline", false);
    register_source(&app, &token, "inline", &db).await;
    put_turtle(&app, &token, SHAPES_GRAPH, SHAPES).await;

    let yarrrml = r#"
prefixes:
  ex: http://example.org/products/ontology#
  prod: http://example.org/products/
mappings:
  product:
    table: products
    s: prod:product_$(product_id)
    po:
      - [a, ex:Product]
      - [ex:name, $(name)]
      - [ex:hasPrice, $(price), xsd:decimal]
"#;
    let (st, v, txt) = dry_run(
        &app,
        &token,
        "inline",
        json!({ "yarrrml": yarrrml, "shapesGraph": SHAPES_GRAPH, "sampleSize": 2 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(v["mapping"].is_null(), "nothing was registered: {txt}");
    assert_eq!(v["rows"], 2, "{txt}");
    // No supplier reference in this mapping, so the shape's sh:minCount on it
    // fails for both rows — systematic, and correctly so: the mapping lacks
    // the property.
    assert_eq!(
        v["classification"]["mappingDefects"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{txt}"
    );
    let (st, list, _) = req(&app, Method::GET, "/api/mappings", &token, Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        list.as_array().unwrap().is_empty(),
        "a dry-run registers nothing"
    );

    // The same, as RML in the body.
    let (st, v, txt) = dry_run(
        &app,
        &token,
        "inline",
        json!({ "rml": mapping_for("inline", "xsd:decimal"), "shapesGraph": SHAPES_GRAPH }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["report"]["conforms"], true, "{txt}");
}

#[tokio::test]
async fn the_request_is_checked_and_the_endpoint_is_admin_only() {
    sources_dir();
    let (state, token) = admin_state();
    state
        .auth_db
        .create_user(
            "reader",
            "reader",
            "reader@test.com",
            "hash",
            SystemRole::User,
        )
        .unwrap();
    let reader = mint_token("reader", "reader", "user");
    let app = test_app(state);
    let db = fresh_sqlite("checked", false);
    register_source(&app, &token, "checked", &db).await;
    put_turtle(&app, &token, SHAPES_GRAPH, SHAPES).await;
    register_mapping(
        &app,
        &token,
        "checked-map",
        "checked",
        &mapping_for("checked", "xsd:decimal"),
    )
    .await;

    // Two forms at once.
    let (st, _, txt) = dry_run(
        &app,
        &token,
        "checked",
        json!({ "mapping": "checked-map", "rml": "x" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("exactly one"), "{txt}");

    // A table nothing reads.
    let (st, _, txt) = dry_run(
        &app,
        &token,
        "checked",
        json!({ "mapping": "checked-map", "table": "nothing" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("no triples map reads"), "{txt}");

    // A field the request does not have is refused (axum's JSON rejection),
    // never ignored: a misspelled `sampleSize` silently meaning the default
    // is the kind of surprise this exists to prevent.
    let (st, _, txt) = dry_run(
        &app,
        &token,
        "checked",
        json!({ "mapping": "checked-map", "sampelSize": 3 }),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{txt}");
    assert!(txt.contains("sampelSize"), "{txt}");

    // A mapping registered against another datasource.
    let other = fresh_sqlite("other", false);
    register_source(&app, &token, "other", &other).await;
    let (st, _, txt) = dry_run(&app, &token, "other", json!({ "mapping": "checked-map" })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("registered against"), "{txt}");

    // The version-graph form.
    let (st, v, txt) = dry_run(
        &app,
        &token,
        "checked",
        json!({ "mappingGraph": "urn:mapping:checked-map:version:1", "sampleSize": 1 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["mapping"]["version"], 1, "{txt}");

    // Not an admin: refused before anything is read.
    let (st, _, _) = dry_run(
        &app,
        &reader,
        "checked",
        json!({ "mapping": "checked-map" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/sources/gates",
        &reader,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, _, _) = dry_run(&app, &token, "missing", json!({ "mapping": "checked-map" })).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_gates_default_until_configured_and_are_served_as_a_graph() {
    let (state, token) = admin_state();
    let app = test_app(state);

    let (st, g, txt) = req(&app, Method::GET, "/api/sources/gates", &token, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(g["source"], "default", "{txt}");
    assert_eq!(g["graph"], "urn:config:mapping-gates", "{txt}");
    assert_eq!(g["autoThreshold"], 0.9, "{txt}");
    assert_eq!(g["reviewThreshold"], 0.7, "{txt}");
    assert_eq!(g["lexical"]["nameWeight"], 0.6, "{txt}");

    // A misspelled gate is refused (axum's JSON rejection) rather than ignored.
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        "/api/sources/gates",
        &token,
        json!({ "autoTreshold": 0.5 }),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{txt}");
    assert!(txt.contains("autoTreshold"), "{txt}");
    // …and so are bands that cross.
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        "/api/sources/gates",
        &token,
        json!({ "reviewThreshold": 0.95 }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("reviewThreshold"), "{txt}");

    let (st, g, txt) = req(
        &app,
        Method::PUT,
        "/api/sources/gates",
        &token,
        json!({ "autoThreshold": 0.85, "lexical": { "minimumScore": 0.5 } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(g["source"], "configured", "{txt}");
    assert_eq!(g["autoThreshold"], 0.85, "{txt}");
    assert_eq!(g["lexical"]["minimumScore"], 0.5, "{txt}");
    assert_eq!(g["lexical"]["nameWeight"], 0.6, "untouched: {txt}");

    // The graph itself, for a reader that wants RDF.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sources/gates")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::ACCEPT, "text/turtle")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ttl = body_text(resp.into_body()).await;
    assert!(
        ttl.contains("ds:autoThreshold \"0.8500\"^^xsd:decimal"),
        "{ttl}"
    );
    assert!(
        ttl.contains("<urn:config:mapping-gates> a ds:MappingGates"),
        "{ttl}"
    );
}

#[tokio::test]
async fn the_mapping_list_filters_by_datasource_in_either_spelling() {
    // The Studio asks with the IRI, a script with the id. Both must find the
    // mapping; the IRI form found nothing, so the workspace listed none.
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("listed", false);
    register_source(&app, &token, "listed", &db).await;
    put_turtle(&app, &token, SHAPES_GRAPH, SHAPES).await;
    register_mapping(
        &app,
        &token,
        "listed-map",
        "listed",
        &mapping_for("listed", "xsd:decimal"),
    )
    .await;

    for query in ["listed", "urn:source:listed", "urn%3Asource%3Alisted"] {
        let (st, list, txt) = req(
            &app,
            Method::GET,
            &format!("/api/mappings?source={query}"),
            &token,
            Value::Null,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert_eq!(list.as_array().unwrap().len(), 1, "?source={query}: {txt}");
        assert_eq!(list[0]["id"], "listed-map", "{txt}");
    }
    let (_, other, _) = req(
        &app,
        Method::GET,
        "/api/mappings?source=urn:source:other",
        &token,
        Value::Null,
    )
    .await;
    assert!(other.as_array().unwrap().is_empty());
}
