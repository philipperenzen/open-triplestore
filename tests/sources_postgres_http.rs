//! The whole pipeline over HTTP against a real PostgreSQL server, through
//! the `plugin-postgres` connector: register, introspect, profile, map,
//! dry-run, run, and read the graph back.
//!
//! Built only with `--features plugin-postgres`; skipped at run time unless
//! `OTS_TEST_POSTGRES_HOST` is set (see `plugins/postgres/tests/live.rs` for
//! the variables).

#![cfg(feature = "plugin-postgres")]

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use ots_plugin_postgres::postgres;
use serde_json::{json, Value};
use tower::ServiceExt as _;

#[derive(Clone)]
struct Target {
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    db: String,
}

fn target() -> Option<Target> {
    let Ok(host) = std::env::var("OTS_TEST_POSTGRES_HOST") else {
        // The CI job that starts the servers sets OTS_TEST_LIVE_REQUIRED, so
        // a missing or renamed variable fails there instead of skipping.
        assert!(
            std::env::var_os("OTS_TEST_LIVE_REQUIRED").is_none(),
            "OTS_TEST_LIVE_REQUIRED is set but OTS_TEST_POSTGRES_HOST is not"
        );
        return None;
    };
    Some(Target {
        host,
        port: std::env::var("OTS_TEST_POSTGRES_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(5432),
        user: std::env::var("OTS_TEST_POSTGRES_USER").unwrap_or_else(|_| "postgres".into()),
        password: std::env::var("OTS_TEST_POSTGRES_PASSWORD").ok(),
        db: std::env::var("OTS_TEST_POSTGRES_DB").unwrap_or_else(|_| "postgres".into()),
    })
}

/// The server may still be starting when a CI job reaches this test: the
/// administrator's connection is retried for up to a minute.
fn patiently<T, E: std::fmt::Display>(mut attempt: impl FnMut() -> Result<T, E>) -> T {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match attempt() {
            Ok(v) => return v,
            Err(e) if std::time::Instant::now() < deadline => {
                eprintln!("waiting for the server: {e}");
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            Err(e) => panic!("admin connection: {e}"),
        }
    }
}

fn fixture(t: &Target) -> String {
    let schema = format!("ots_http_{}", std::process::id());
    let mut admin = postgres::Config::new();
    admin.host(&t.host).port(t.port).user(&t.user).dbname(&t.db);
    if let Some(p) = &t.password {
        admin.password(p);
    }
    let mut client = patiently(|| admin.connect(postgres::NoTls));
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; \
             SET search_path = {schema}; \
             CREATE TABLE products (product_id INTEGER PRIMARY KEY, name TEXT NOT NULL, \
                                    price NUMERIC(10,2), added TIMESTAMPTZ); \
             INSERT INTO products VALUES (1, 'Bolt', 0.25, '2026-01-01T00:00:00Z'), \
                                         (2, 'Nut', 1.50, NULL), (3, 'Washer', 9.00, NULL);"
        ))
        .expect("fixture");
    schema
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

const MAPPING: &str = r#"
@prefix rr:  <http://www.w3.org/ns/r2rml#> .
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix ql:  <http://semweb.mmlab.be/ns/ql#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:  <http://example.org/products/ontology#> .

ex:ProductsMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:pgsrc> ; rml:referenceFormulation ql:SQL2008 ; rr:tableName "products" ] ;
  rr:subjectMap [ rr:template "http://example.org/products/product_{product_id}" ; rr:class ex:Product ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:hasPrice ; rr:objectMap [ rr:column "price" ; rr:datatype xsd:decimal ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:added ; rr:objectMap [ rr:column "added" ] ] .
"#;

#[tokio::test]
async fn a_postgresql_datasource_is_profiled_mapped_and_materialised_over_http() {
    let Some(t) = target() else {
        eprintln!("OTS_TEST_POSTGRES_HOST unset; skipping the live test");
        return;
    };
    // The synchronous driver runs its own runtime: build the fixture off the
    // test's, the way the host opens every connection.
    let fixture_target = t.clone();
    let schema = tokio::task::spawn_blocking(move || fixture(&fixture_target))
        .await
        .unwrap();
    // What the server does at boot: the compiled-in plugins hand over their
    // drivers.
    open_triplestore::sources::connector::register_plugin_connectors();
    std::env::set_var(
        "OTS_PG_HTTP_TEST_PASSWORD",
        t.password.clone().unwrap_or_default(),
    );

    let (state, admin) = admin_state();
    let app = test_app(state);
    let (st, src, txt) = req(
        &app,
        Method::POST,
        "/api/sources",
        &admin,
        json!({
            "id": "pgsrc", "name": "A PostgreSQL source", "dialect": "postgresql",
            "host": t.host, "port": t.port, "database": t.db, "username": t.user,
            "credential": "env:OTS_PG_HTTP_TEST_PASSWORD", "statementTimeoutMs": 5000,
            "options": { "search_path": schema },
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(src["credential"], "env:OTS_PG_HTTP_TEST_PASSWORD");
    assert!(!txt.contains(&t.password.clone().unwrap_or_else(|| "\u{0}".into())));

    let (st, tables, txt) = req(
        &app,
        Method::GET,
        "/api/sources/pgsrc/introspect",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let products = tables["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tb| tb["name"] == "products")
        .unwrap_or_else(|| panic!("{txt}"));
    assert_eq!(products["primaryKey"][0], "product_id");
    let price = products["columns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "price")
        .unwrap();
    assert_eq!(price["genericType"], "decimal");
    assert_eq!(price["nativeType"], "numeric(10,2)");

    let (st, profile, txt) = req(
        &app,
        Method::POST,
        "/api/sources/pgsrc/profile",
        &admin,
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(profile["version"], 1);
    let (st, _, ttl) = req(
        &app,
        Method::GET,
        "/api/sources/pgsrc/profile",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(ttl.contains("products"), "{ttl}");
    assert!(
        ttl.contains("\"3\"^^xsd:integer") || ttl.contains("3 ;") || ttl.contains(" 3 "),
        "{ttl}"
    );

    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/mappings",
        &admin,
        json!({ "id": "pg-products", "title": "Products", "rml": MAPPING }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");

    let (st, dry, txt) = req(
        &app,
        Method::POST,
        "/api/sources/pgsrc/dry-run",
        &admin,
        json!({ "mapping": "pg-products", "sampleSize": 2 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(dry["rows"], 2, "{txt}");

    let (st, run, txt) = req(
        &app,
        Method::POST,
        "/api/sources/pgsrc/runs",
        &admin,
        json!({ "mapping": "pg-products" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(run["status"], "succeeded");
    assert_eq!(run["rowsExtracted"], 3);
    let graph = run["graph"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/store?graph={}", url_encode(&graph)))
                .header(header::AUTHORIZATION, format!("Bearer {admin}"))
                .header(header::ACCEPT, "application/n-triples")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let nt = body_text(resp.into_body()).await;
    assert!(nt.contains("<http://example.org/products/product_2> <http://example.org/products/ontology#hasPrice> \"1.5\"^^<http://www.w3.org/2001/XMLSchema#decimal>"), "{nt}");
    assert!(nt.contains("\"Bolt\""), "{nt}");
    // The timestamp arrived in XSD shape, took the natural datatype, and the
    // store canonicalised it (as it did the decimals above).
    assert!(
        nt.contains("\"2026-01-01T00:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>"),
        "{nt}"
    );
    // A NULL produced no triple.
    assert!(
        !nt.contains(
            "<http://example.org/products/product_2> <http://example.org/products/ontology#added>"
        ),
        "{nt}"
    );

    let (st, src, _) = req(&app, Method::GET, "/api/sources/pgsrc", &admin, Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(src["production"]["graph"], graph);
}
