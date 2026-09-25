//! Phase 3 over HTTP: the proposer's scoped access, review decisions as
//! PROV, calibration data, review items with the deterministic fixer, and
//! promotion of a corrected candidate.
//!
//! Acceptance checks (written before the implementation):
//! * a service token scoped `sources:read` + `mappings:propose` reads profiles
//!   and gates, sees no datasource location or credential reference, writes a
//!   `proposed` mapping and dry-runs it — and can neither approve nor run;
//! * approve / edit / reject decisions are distinct PROV outcomes on the
//!   mapping, listed for the proposer's training job;
//! * the calibration endpoint refuses one-class data and otherwise returns a
//!   monotone curve;
//! * a run the gate refuses opens one review item per subject; the
//!   deterministic fixer clamps to `sh:maxInclusive` and reads a negative
//!   where a positive is required as a sign typo, applies through the normal
//!   write path, and never fabricates a value; promotion re-gates the
//!   corrected candidate and swaps it in with provenance.

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

const SHAPES_GRAPH: &str = "urn:shapes:review-products";

fn sources_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("ots-review-http-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("OTS_SOURCES_DIR", &dir);
        dir
    })
}

/// Three products: a sound one, one with a negative price (a sign typo where
/// a positive is required) and one priced above the shape's ceiling.
fn fresh_sqlite(name: &str) -> PathBuf {
    let path = sources_dir().join(format!("{name}-{}.db", uuid::Uuid::new_v4()));
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE products (product_id INTEGER PRIMARY KEY, name TEXT NOT NULL, price REAL);
         INSERT INTO products VALUES (1, 'Bolt', 0.25), (2, 'Nut', -0.10), (3, 'Washer', 5000);",
    )
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

async fn get_graph(app: &Router, token: &str, graph: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/store?graph={}", url_encode(graph)))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::ACCEPT, "application/n-triples")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_text(resp.into_body()).await
}

/// A served Turtle document as N-Triples, so an assertion about a statement
/// does not depend on which prefixes the serializer declared.
fn as_ntriples(turtle: &str) -> String {
    let store = open_triplestore::store::TripleStore::in_memory().unwrap();
    store
        .load_str(turtle, oxigraph::io::RdfFormat::Turtle, None)
        .unwrap_or_else(|e| panic!("not valid Turtle: {e}\n{turtle}"));
    String::from_utf8(store.dump(oxigraph::io::RdfFormat::NTriples, None).unwrap()).unwrap()
}

const SHAPES: &str = r#"
@prefix sh:  <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:  <http://example.org/products/ontology#> .

ex:ProductShape a sh:NodeShape ;
  sh:targetClass ex:Product ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ] ;
  sh:property [ sh:path ex:hasPrice ; sh:datatype xsd:decimal ; sh:minInclusive 0 ; sh:maxInclusive 1000 ] .
"#;

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
  rr:predicateObjectMap [ rr:predicate ex:hasPrice ; rr:objectMap [ rr:column "price" ; rr:datatype xsd:decimal ] ] .
"#;

fn mapping_for(source_id: &str) -> String {
    MAPPING.replace("SOURCE", source_id)
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
            "credential": "env:OTS_REVIEW_TEST_SECRET",
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register {id}: {txt}");
}

async fn register_mapping(app: &Router, token: &str, id: &str, source_id: &str) {
    let (st, _, txt) = req(
        app,
        Method::POST,
        "/api/mappings",
        token,
        json!({ "id": id, "title": id, "rml": mapping_for(source_id), "shapesGraph": SHAPES_GRAPH }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register mapping {id}: {txt}");
}

/// A plain user's API token with the given scopes.
async fn service_token(
    app: &Router,
    state: &open_triplestore::server::AppState,
    name: &str,
    scopes: &[&str],
) -> String {
    let _ = state.auth_db.create_user(
        name,
        name,
        &format!("{name}@test.com"),
        "hash",
        SystemRole::User,
    );
    let session = mint_token(name, name, "user");
    let (st, v, txt) = req(
        app,
        Method::POST,
        "/api/auth/tokens",
        &session,
        json!({ "name": format!("{name} token"), "scopes": scopes }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "mint token: {txt}");
    v["token"].as_str().unwrap().to_string()
}

// ───────────────────────── The proposer's access ─────────────────────────

#[tokio::test]
async fn a_scoped_service_token_reads_profiles_and_proposes_but_never_approves() {
    sources_dir();
    std::env::set_var("OTS_REVIEW_TEST_SECRET", "s3cret-value");
    let (state, admin) = admin_state();
    let app = test_app(state.clone());
    let db = fresh_sqlite("proposer");
    register_source(&app, &admin, "proposer", &db).await;
    put_turtle(&app, &admin, SHAPES_GRAPH, SHAPES).await;
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources/proposer/profile",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");

    let proposer = service_token(
        &app,
        &state,
        "proposer-svc",
        &["sources:read", "mappings:propose"],
    )
    .await;
    let reader = service_token(&app, &state, "reader-svc", &["sources:read"]).await;
    let plain = service_token(&app, &state, "plain-svc", &["read", "write"]).await;

    // Reads: the profile, the gates, the datasource list — with no location
    // and no credential reference: the proposer never sees a DSN.
    let (st, list, txt) = req(&app, Method::GET, "/api/sources", &proposer, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let src = &list.as_array().unwrap()[0];
    assert_eq!(src["id"], "proposer");
    for hidden in ["database", "host", "username", "credential"] {
        assert!(
            src.get(hidden).is_none(),
            "{hidden} reached a non-admin: {txt}"
        );
    }
    assert!(
        !txt.contains("s3cret-value") && !txt.contains("OTS_REVIEW_TEST_SECRET"),
        "{txt}"
    );
    let (st, _, txt) = req(
        &app,
        Method::GET,
        "/api/sources/proposer/profile",
        &proposer,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(txt.contains("dsprof:"), "{txt}");
    let (st, g, txt) = req(
        &app,
        Method::GET,
        "/api/sources/gates",
        &proposer,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(g["autoThreshold"], 0.9);
    // …but not the admin's view of the same datasource.
    let (st, src, _) = req(
        &app,
        Method::GET,
        "/api/sources/proposer",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(src["credential"], "env:OTS_REVIEW_TEST_SECRET");

    // A proposal: written in the `proposed` state, never any other.
    let (st, m, txt) = req(
        &app,
        Method::POST,
        "/api/mappings",
        &proposer,
        json!({ "id": "proposal", "title": "Proposed by the service", "rml": mapping_for("proposer"),
                "shapesGraph": SHAPES_GRAPH, "state": "proposed" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(m["state"], "proposed", "{txt}");
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/mappings",
        &proposer,
        json!({ "id": "sneaky", "rml": mapping_for("proposer"), "state": "approved" }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "a proposer approves nothing: {txt}"
    );
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        "/api/mappings/proposal",
        &proposer,
        json!({ "state": "approved" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{txt}");
    // Refining its own proposal is fine, and stays proposed.
    let (st, m, txt) = req(
        &app,
        Method::PUT,
        "/api/mappings/proposal",
        &proposer,
        json!({ "rml": mapping_for("proposer") }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(m["version"], 2, "{txt}");
    assert_eq!(m["state"], "proposed", "{txt}");
    // A dry-run of the proposal is part of proposing.
    let (st, d, txt) = req(
        &app,
        Method::POST,
        "/api/sources/proposer/dry-run",
        &proposer,
        json!({ "mapping": "proposal", "sampleSize": 3 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(d["rows"], 3, "{txt}");
    // Running, deleting, editing the gates: not the proposer's.
    let (st, _, _) = req(
        &app,
        Method::POST,
        "/api/sources/proposer/runs",
        &proposer,
        json!({ "mapping": "proposal" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = req(
        &app,
        Method::DELETE,
        "/api/mappings/proposal",
        &proposer,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = req(
        &app,
        Method::PUT,
        "/api/sources/gates",
        &proposer,
        json!({ "autoThreshold": 0.5 }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = req(
        &app,
        Method::POST,
        "/api/mappings/proposal/decisions",
        &proposer,
        json!({ "decision": "approve" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Read-only scope reads and proposes nothing; a plain read/write token is
    // not a sources principal at all.
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/sources/proposer/profile",
        &reader,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = req(
        &app,
        Method::POST,
        "/api/mappings",
        &reader,
        json!({ "id": "x", "rml": mapping_for("proposer"), "state": "proposed" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = req(&app, Method::GET, "/api/sources", &plain, Value::Null).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

// ───────────────────────── Decisions and calibration ─────────────────────────

#[tokio::test]
async fn decisions_are_distinct_prov_outcomes_and_reviews_list_them() {
    sources_dir();
    std::env::set_var("OTS_REVIEW_TEST_SECRET", "s3cret-value");
    let (state, admin) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("decisions");
    register_source(&app, &admin, "decisions", &db).await;
    put_turtle(&app, &admin, SHAPES_GRAPH, SHAPES).await;
    register_mapping(&app, &admin, "decided", "decisions").await;

    let decide = |body: Value| {
        req(
            &app,
            Method::POST,
            "/api/mappings/decided/decisions",
            &admin,
            body,
        )
    };
    let (st, d, txt) =
        decide(json!({ "decision": "reject", "confidence": 0.42, "note": "wrong table" })).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(d["outcome"], "reject", "{txt}");
    assert_eq!(d["mappingVersion"], 1, "{txt}");
    let (_, m, _) = req(
        &app,
        Method::GET,
        "/api/mappings/decided",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(m["state"], "rejected");

    let (st, _, txt) = decide(json!({ "decision": "edit", "target": "http://example.org/products/ontology#ProductsMap", "confidence": 0.71 })).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (st, _, txt) = decide(json!({ "decision": "approve", "confidence": 0.93 })).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (_, m, _) = req(
        &app,
        Method::GET,
        "/api/mappings/decided",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(m["state"], "approved");
    let (st, _, txt) = decide(json!({ "decision": "maybe" })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");

    // Listed newest first, each a distinct outcome with its confidence.
    let (st, reviews, txt) = req(
        &app,
        Method::GET,
        "/api/mappings/decided/reviews",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let outcomes: Vec<&str> = reviews
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, vec!["approve", "edit", "reject"], "{txt}");
    assert_eq!(
        reviews[1]["target"], "http://example.org/products/ontology#ProductsMap",
        "{txt}"
    );
    assert_eq!(reviews[2]["confidence"], 0.42, "{txt}");
    assert_eq!(reviews[2]["note"], "wrong table", "{txt}");
    assert!(
        reviews[0]["actor"]
            .as_str()
            .unwrap()
            .ends_with("/users/adm"),
        "{txt}"
    );

    // …and on the mapping's PROV trail, as activities with distinct outcomes.
    let (st, _, prov) = req(
        &app,
        Method::GET,
        "/api/mappings/decided/provenance",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{prov}");
    let nt = as_ntriples(&prov);
    for outcome in ["approve", "edit", "reject"] {
        assert!(
            nt.contains(&format!("datasource#outcome> \"{outcome}\"")),
            "{outcome} missing:\n{nt}"
        );
    }
    assert!(nt.contains("datasource#ReviewDecision"), "{nt}");
    assert!(
        nt.contains("prov#used> <urn:mapping:decided:version:1>"),
        "{nt}"
    );
}

#[tokio::test]
async fn calibration_refuses_one_class_data_and_fits_a_monotone_curve() {
    let (state, admin) = admin_state();
    let app = test_app(state);
    let calibrate = |body: Value| req(&app, Method::POST, "/api/sources/calibration", &admin, body);

    let (st, _, txt) = calibrate(json!({ "points": [
        { "confidence": 0.9, "accepted": true }, { "confidence": 0.6, "accepted": true } ] }))
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "one class: {txt}");
    assert!(
        txt.contains("one-class") || txt.contains("one class"),
        "{txt}"
    );
    let (st, _, txt) =
        calibrate(json!({ "points": [ { "confidence": 0.9, "accepted": false } ] })).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{txt}");

    let (st, c, txt) = calibrate(json!({ "points": [
        { "confidence": 0.95, "accepted": true }, { "confidence": 0.9, "accepted": true },
        { "confidence": 0.8, "accepted": false }, { "confidence": 0.85, "accepted": true },
        { "confidence": 0.5, "accepted": false }, { "confidence": 0.3, "accepted": false },
        { "confidence": 0.6, "accepted": true } ] }))
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(c["points"], 7);
    assert_eq!(c["positives"], 4);
    assert_eq!(c["negatives"], 3);
    let curve = c["curve"].as_array().unwrap();
    assert!(!curve.is_empty(), "{txt}");
    let mut last = -1.0;
    for p in curve {
        let y = p["calibrated"].as_f64().unwrap();
        assert!((0.0..=1.0).contains(&y), "{txt}");
        assert!(y >= last, "isotonic means never decreasing: {txt}");
        last = y;
    }
    assert!(
        curve[0]["confidence"].as_f64().unwrap()
            <= curve.last().unwrap()["confidence"].as_f64().unwrap()
    );
    let (st, _, txt) = calibrate(json!({})).await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "no decisions recorded yet: {txt}"
    );
}

// ───────────────────────── Review items and promotion ─────────────────────────

#[tokio::test]
async fn a_refused_run_opens_review_items_the_fixer_corrects_and_promotion_swaps_in() {
    sources_dir();
    std::env::set_var("OTS_REVIEW_TEST_SECRET", "s3cret-value");
    let (state, admin) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("review");
    register_source(&app, &admin, "review", &db).await;
    put_turtle(&app, &admin, SHAPES_GRAPH, SHAPES).await;
    register_mapping(&app, &admin, "review-map", "review").await;

    // The gate refuses: a negative price and one over the ceiling.
    let (st, v, txt) = req(
        &app,
        Method::POST,
        "/api/sources/review/runs",
        &admin,
        json!({ "mapping": "review-map" }),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{txt}");
    let run_id = v["run"]["id"].as_str().unwrap().to_string();
    let run_graph = v["run"]["graph"].as_str().unwrap().to_string();
    let (_, src, _) = req(
        &app,
        Method::GET,
        "/api/sources/review",
        &admin,
        Value::Null,
    )
    .await;
    assert!(src["production"].is_null(), "production untouched");

    // One review item per subject, carrying the violations and a snapshot.
    let (st, items, txt) = req(
        &app,
        Method::GET,
        "/api/sources/review/reviews",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let items = items.as_array().unwrap();
    assert_eq!(items.len(), 2, "{txt}");
    let item = |subject: &str| -> Value {
        items
            .iter()
            .find(|i| i["subject"] == format!("http://example.org/products/{subject}"))
            .cloned()
            .unwrap_or_else(|| panic!("{subject}: {txt}"))
    };
    let nut = item("product_2");
    let washer = item("product_3");
    assert_eq!(nut["status"], "needsHuman");
    assert_eq!(nut["run"], run_id);
    assert!(
        nut["snapshot"].as_str().unwrap().contains("\"-0.1\""),
        "{nut}"
    );
    assert!(
        nut["violations"].as_array().unwrap()[0]["constraint"]
            .as_str()
            .unwrap()
            .starts_with("sh:minInclusive"),
        "{nut}"
    );
    assert!(
        washer["violations"].as_array().unwrap()[0]["constraint"]
            .as_str()
            .unwrap()
            .starts_with("sh:maxInclusive"),
        "{washer}"
    );
    let (st, listed, _) = req(
        &app,
        Method::GET,
        "/api/sources/review/reviews?status=needsHuman",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(listed.as_array().unwrap().len(), 2);

    // The fixer: a preview first, as an RDF Patch, applying nothing.
    let nut_id = nut["id"].as_str().unwrap();
    let (st, fix, txt) = req(
        &app,
        Method::POST,
        &format!("/api/reviews/{nut_id}/autofix"),
        &admin,
        json!({ "apply": false }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(fix["applied"], false, "{txt}");
    assert_eq!(fix["fixes"][0]["rule"], "sign-typo", "{txt}");
    assert_eq!(fix["fixes"][0]["to"], "0.1", "{txt}");
    let patch = fix["patch"].as_str().unwrap();
    assert!(
        patch.contains("TX .")
            && patch.contains("\nD ")
            && patch.contains("\nA ")
            && patch.contains("TC ."),
        "{patch}"
    );
    assert!(
        patch.contains("\"0.1\"^^<http://www.w3.org/2001/XMLSchema#decimal>"),
        "{patch}"
    );
    let (_, unchanged, _) = req(
        &app,
        Method::GET,
        &format!("/api/reviews/{nut_id}"),
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(
        unchanged["status"], "needsHuman",
        "a preview changes nothing"
    );

    // Applied: through the normal write path, into the candidate graph.
    let (st, fix, txt) = req(
        &app,
        Method::POST,
        &format!("/api/reviews/{nut_id}/autofix"),
        &admin,
        json!({ "apply": true }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(fix["applied"], true, "{txt}");
    assert_eq!(fix["status"], "corrected", "{txt}");
    let graph = get_graph(&app, &admin, &run_graph).await;
    assert!(
        graph.contains("\"0.1\"^^<http://www.w3.org/2001/XMLSchema#decimal>"),
        "{graph}"
    );
    assert!(!graph.contains("\"-0.1\""), "{graph}");
    let washer_id = washer["id"].as_str().unwrap();
    let (st, fix, txt) = req(
        &app,
        Method::POST,
        &format!("/api/reviews/{washer_id}/autofix"),
        &admin,
        json!({ "apply": true }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(fix["fixes"][0]["rule"], "clamp", "{txt}");
    assert_eq!(fix["fixes"][0]["to"], "1000", "{txt}");

    // Promotion re-gates the corrected candidate and swaps it in.
    let (st, p, txt) = req(
        &app,
        Method::POST,
        &format!("/api/runs/{run_id}/promote"),
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(p["run"]["status"], "succeeded", "{txt}");
    assert_eq!(p["source"]["production"]["run"], run_id, "{txt}");
    let (_, items, _) = req(
        &app,
        Method::GET,
        "/api/sources/review/reviews",
        &admin,
        Value::Null,
    )
    .await;
    assert!(
        items
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["status"] == "promoted"),
        "{items}"
    );
    let (st, _, prov) = req(
        &app,
        Method::GET,
        &format!("/api/runs/{run_id}/provenance"),
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let nt = as_ntriples(&prov);
    assert!(nt.contains("datasource#Promotion"), "{nt}");
    assert!(nt.contains("/users/adm>"), "who promoted: {nt}");
    // A second promotion of the same run is refused: it is in production.
    let (st, _, _) = req(
        &app,
        Method::POST,
        &format!("/api/runs/{run_id}/promote"),
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_fixer_never_fabricates_a_value_and_statuses_are_explicit() {
    sources_dir();
    std::env::set_var("OTS_REVIEW_TEST_SECRET", "s3cret-value");
    let (state, admin) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("nofab");
    // A product with no name: minCount 1 fails, and nothing can be invented.
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("UPDATE products SET price = 1 WHERE product_id IN (2, 3); INSERT INTO products VALUES (4, '', 2);")
        .unwrap();
    register_source(&app, &admin, "nofab", &db).await;
    put_turtle(&app, &admin, SHAPES_GRAPH, SHAPES).await;
    register_mapping(&app, &admin, "nofab-map", "nofab").await;
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources/nofab/runs",
        &admin,
        json!({ "mapping": "nofab-map" }),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{txt}");
    let (_, items, txt) = req(
        &app,
        Method::GET,
        "/api/sources/nofab/reviews",
        &admin,
        Value::Null,
    )
    .await;
    let items = items.as_array().unwrap();
    assert_eq!(items.len(), 1, "{txt}");
    let id = items[0]["id"].as_str().unwrap();
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/reviews/{id}/autofix"),
        &admin,
        json!({ "apply": true }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "nothing to fix without inventing: {txt}"
    );

    // A human decides instead, and the decision is recorded with its author.
    let (st, it, txt) = req(
        &app,
        Method::POST,
        &format!("/api/reviews/{id}/status"),
        &admin,
        json!({ "status": "rejected", "note": "no name in the source" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(it["status"], "rejected");
    assert_eq!(it["decision"], "no name in the source");
    assert!(it["reviewer"].as_str().unwrap().ends_with("/users/adm"));
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/reviews/{id}/status"),
        &admin,
        json!({ "status": "sideways" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    // Without a gateway, the LLM corrector says so rather than pretending.
    let (st, _, _) = req(
        &app,
        Method::POST,
        &format!("/api/reviews/{id}/suggest"),
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
}
