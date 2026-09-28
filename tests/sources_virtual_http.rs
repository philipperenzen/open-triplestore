//! A virtual source — a SPARQL endpoint standing in for an Ontop virtual
//! knowledge graph — behind the same connector trait as a database, over
//! HTTP: registered with a secret reference, introspected as class-tables,
//! profiled, mapped with a SPARQL `rml:query` and with a class as
//! `rr:tableName`, snapshotted whole into a gated run graph, and queried
//! live through `SERVICE <urn:source:id>` with its credential supplied by
//! the store.
//!
//! The endpoint is an in-process mock: a second triple store behind
//! `POST /sparql`, answering results JSON or N-Triples and demanding HTTP
//! Basic. Everything the store sends it goes through the remote allowlist.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use base64::Engine as _;
use common::*;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::results::{QueryResultsFormat, QueryResultsSerializer};
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const EX: &str = "http://example.org/products/ontology#";
const REMOTE_PASSWORD: &str = "vkg-reader-pa55";

struct Mock {
    store: TripleStore,
    basic: String,
}

/// The endpoint: the query's own result form, behind Basic auth.
async fn sparql(State(mock): State<Arc<Mock>>, headers: HeaderMap, body: String) -> Response {
    let authorised = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == mock.basic);
    if !authorised {
        return (StatusCode::UNAUTHORIZED, "who are you").into_response();
    }
    match mock.store.query(&body) {
        Ok(QueryResults::Graph(triples)) => {
            let mut out = Vec::new();
            let mut writer =
                oxigraph::io::RdfSerializer::from_format(RdfFormat::NTriples).for_writer(&mut out);
            for t in triples.flatten() {
                writer.serialize_triple(t.as_ref()).unwrap();
            }
            writer.finish().unwrap();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/n-triples")],
                out,
            )
                .into_response()
        }
        Ok(QueryResults::Boolean(value)) => {
            let mut out = Vec::new();
            QueryResultsSerializer::from_format(QueryResultsFormat::Json)
                .serialize_boolean_to_writer(&mut out, value)
                .unwrap();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/sparql-results+json")],
                out,
            )
                .into_response()
        }
        Ok(QueryResults::Solutions(solutions)) => {
            let mut out = Vec::new();
            let mut sink = QueryResultsSerializer::from_format(QueryResultsFormat::Json)
                .serialize_solutions_to_writer(&mut out, solutions.variables().to_vec())
                .unwrap();
            for solution in solutions {
                sink.serialize(&solution.unwrap()).unwrap();
            }
            sink.finish().unwrap();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/sparql-results+json")],
                out,
            )
                .into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// Start the mock on a free port and return its port.
async fn start_mock() -> u16 {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            &format!(
                "@prefix ex: <{EX}> .\n\
                 @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                 <http://example.org/products/product_1> a ex:Product ; ex:name \"Bolt\" ; ex:price \"0.25\"^^xsd:decimal .\n\
                 <http://example.org/products/product_2> a ex:Product ; ex:name \"Nut\" ; ex:price \"1.5\"^^xsd:decimal .\n\
                 <http://example.org/products/product_3> a ex:Product ; ex:name \"Washer\" .\n\
                 <http://example.org/products/category_1> a ex:Category ; ex:label \"Fasteners\" .\n"
            ),
            RdfFormat::Turtle,
            None,
        )
        .unwrap();
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("reader:{REMOTE_PASSWORD}"))
    );
    let app = Router::new()
        .route("/sparql", post(sparql))
        .with_state(Arc::new(Mock { store, basic }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
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

async fn local_sparql(app: &Router, token: &str, query: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sparql")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/sparql-query")
                .header(header::ACCEPT, "application/sparql-results+json")
                .body(Body::from(query.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let json = body_json(resp.into_body()).await;
    (status, json)
}

/// A local query whose SERVICE may fail mid-stream: `None` when the store
/// refused it, in the status or in the body.
async fn local_sparql_lenient(app: &Router, token: &str, query: &str) -> Option<Value> {
    use http_body_util::BodyExt as _;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sparql")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/sparql-query")
                .header(header::ACCEPT, "application/sparql-results+json")
                .body(Body::from(query.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.into_body().collect().await.ok()?.to_bytes();
    serde_json::from_slice(&bytes).ok()
}

fn query_mapping() -> String {
    format!(
        r#"
@prefix rr:  <http://www.w3.org/ns/r2rml#> .
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix ql:  <http://semweb.mmlab.be/ns/ql#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix out: <http://example.org/out#> .

out:ProductsMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:vkg> ; rml:referenceFormulation ql:SQL2008 ;
    rml:query "SELECT ?s ?name ?price WHERE {{ ?s a <{EX}Product> ; <{EX}name> ?name . OPTIONAL {{ ?s <{EX}price> ?price }} }}" ] ;
  rr:subjectMap [ rr:column "s" ; rr:termType rr:IRI ; rr:class out:Item ] ;
  rr:predicateObjectMap [ rr:predicate out:label ; rr:objectMap [ rr:column "name" ] ] ;
  rr:predicateObjectMap [ rr:predicate out:cost ; rr:objectMap [ rr:column "price" ; rr:datatype xsd:decimal ] ] .
"#
    )
}

fn class_mapping() -> String {
    format!(
        r#"
@prefix rr:  <http://www.w3.org/ns/r2rml#> .
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix ql:  <http://semweb.mmlab.be/ns/ql#> .
@prefix out: <http://example.org/out#> .

out:CategoriesMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:vkg> ; rml:referenceFormulation ql:SQL2008 ; rr:tableName "{EX}Category" ] ;
  rr:subjectMap [ rr:column "subject" ; rr:termType rr:IRI ; rr:class out:Group ] ;
  rr:predicateObjectMap [ rr:predicate out:label ; rr:objectMap [ rr:column "{EX}label" ] ] .
"#
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sparql_endpoint_is_a_datasource_a_snapshot_and_a_service() {
    let port = start_mock().await;
    let origin = format!("http://127.0.0.1:{port}");
    std::env::set_var("OTS_REMOTE_ALLOWLIST", &origin);
    std::env::set_var("OTS_VKG_TEST_PASSWORD", REMOTE_PASSWORD);
    let (state, admin) = admin_state();
    let app = test_app(state);

    // Registered like any datasource: a host, a path, an account whose
    // secret is a reference. The store never returns the value.
    let (st, src, txt) = req(
        &app,
        Method::POST,
        "/api/sources",
        &admin,
        json!({
            "id": "vkg", "name": "A virtual knowledge graph", "dialect": "sparql",
            "host": "127.0.0.1", "port": port, "database": "/sparql", "username": "reader",
            "credential": "env:OTS_VKG_TEST_PASSWORD", "statementTimeoutMs": 5000,
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(src["credential"], "env:OTS_VKG_TEST_PASSWORD");
    assert!(!txt.contains(REMOTE_PASSWORD));
    assert_eq!(src["allowlisted"], true);

    // A wrong account is a plain connection failure, without the secret.
    let (st, probe, txt) = req(
        &app,
        Method::POST,
        "/api/sources/test",
        &admin,
        json!({ "id": "x", "dialect": "sparql", "host": "127.0.0.1", "port": port, "database": "/sparql",
                "username": "nobody", "statementTimeoutMs": 5000 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(probe["ok"], false, "{txt}");
    assert!(!txt.contains(REMOTE_PASSWORD));

    // The catalogue is the classes.
    let (st, tables, txt) = req(
        &app,
        Method::GET,
        "/api/sources/vkg/introspect",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let tables = tables["tables"].as_array().unwrap();
    let names: Vec<&str> = tables.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec![format!("{EX}Category"), format!("{EX}Product")],
        "{txt}"
    );
    let product = &tables[1];
    assert_eq!(product["kind"], "view");
    assert_eq!(product["rowEstimate"], 3);
    assert_eq!(product["primaryKey"][0], "subject");
    let columns: Vec<&str> = product["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(
        columns.contains(&"subject")
            && columns.contains(&format!("{EX}name").as_str())
            && columns.contains(&format!("{EX}price").as_str()),
        "{txt}"
    );
    let price = product["columns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == format!("{EX}price"))
        .unwrap();
    assert_eq!(price["genericType"], "decimal");
    assert_eq!(price["nativeType"], "decimal");
    assert_eq!(price["nullable"], true, "one product has no price");

    let (st, profile, txt) = req(
        &app,
        Method::POST,
        "/api/sources/vkg/profile",
        &admin,
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(profile["version"], 1);

    // A mapping over a SPARQL query, run like any other.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/mappings",
        &admin,
        json!({ "id": "vkg-products", "rml": query_mapping() }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (st, run, txt) = req(
        &app,
        Method::POST,
        "/api/sources/vkg/runs",
        &admin,
        json!({ "mapping": "vkg-products" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(run["rowsExtracted"], 3, "{txt}");
    assert_eq!(run["mapping"]["id"], "vkg-products");
    let graph = run["graph"].as_str().unwrap().to_string();
    let (st, rows) = local_sparql(&app, &admin, &format!(
        "SELECT ?s ?cost WHERE {{ GRAPH <{graph}> {{ ?s a <http://example.org/out#Item> . OPTIONAL {{ ?s <http://example.org/out#cost> ?cost }} }} }} ORDER BY ?s"
    )).await;
    assert_eq!(st, StatusCode::OK);
    let bindings = rows["results"]["bindings"].as_array().unwrap();
    assert_eq!(bindings.len(), 3, "{rows}");
    assert_eq!(bindings[1]["cost"]["value"], "1.5");
    assert_eq!(
        bindings[1]["cost"]["datatype"],
        "http://www.w3.org/2001/XMLSchema#decimal"
    );
    assert!(
        bindings[2].get("cost").is_none(),
        "no price, no triple: {rows}"
    );

    // A class as a table: `subject` plus the predicates as columns.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/mappings",
        &admin,
        json!({ "id": "vkg-categories", "rml": class_mapping() }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (st, run, txt) = req(
        &app,
        Method::POST,
        "/api/sources/vkg/runs",
        &admin,
        json!({ "mapping": "vkg-categories" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(run["rowsExtracted"], 1, "{txt}");
    assert_eq!(run["triplesProduced"], 2, "{txt}");

    // A snapshot: the endpoint's whole graph, as a run with no mapping.
    let (st, snap, txt) = req(
        &app,
        Method::POST,
        "/api/sources/vkg/runs",
        &admin,
        json!({ "mode": "snapshot" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(snap["mode"], "snapshot");
    assert_eq!(snap["status"], "succeeded");
    assert!(snap.get("mapping").is_none(), "{txt}");
    assert_eq!(snap["triplesProduced"], 10, "{txt}");
    let snap_id = snap["id"].as_str().unwrap().to_string();
    let snap_graph = snap["graph"].as_str().unwrap().to_string();
    let (_, src, _) = req(&app, Method::GET, "/api/sources/vkg", &admin, Value::Null).await;
    assert_eq!(src["production"]["run"], snap_id);
    let (st, listed, txt) = req(
        &app,
        Method::GET,
        "/api/sources/vkg/runs",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(
        listed.as_array().unwrap().len(),
        3,
        "a snapshot is listed with the mapped runs: {txt}"
    );
    let (st, _, prov) = req(
        &app,
        Method::GET,
        &format!("/api/runs/{snap_id}/provenance"),
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        prov.contains("<urn:source:vkg>") && prov.contains("prov:used"),
        "{prov}"
    );
    let (st, count) = local_sparql(
        &app,
        &admin,
        &format!("SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{snap_graph}> {{ ?s ?p ?o }} }}"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(count["results"]["bindings"][0]["n"]["value"], "10");

    // A snapshot of a database is not a thing.
    std::env::set_var("OTS_SOURCES_DIR", std::env::temp_dir());
    let db = std::env::temp_dir().join(format!("ots-virtual-{}.db", uuid::Uuid::new_v4()));
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("CREATE TABLE t (id INTEGER)")
        .unwrap();
    let (st, _, txt) = req(&app, Method::POST, "/api/sources", &admin, json!({ "id": "db", "dialect": "sqlite", "database": db.to_string_lossy(), "statementTimeoutMs": 1000 })).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources/db/runs",
        &admin,
        json!({ "mode": "snapshot" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources/db/runs",
        &admin,
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "a run needs a mapping: {txt}");

    // Live: SERVICE <urn:source:vkg> reaches the endpoint with the source's
    // account — the query names no URL and no credential.
    let (st, live) = local_sparql(&app, &admin, &format!(
        "SELECT ?name WHERE {{ SERVICE <urn:source:vkg> {{ ?s a <{EX}Product> ; <{EX}name> ?name }} }} ORDER BY ?name"
    )).await;
    assert_eq!(st, StatusCode::OK, "{live}");
    let names: Vec<&str> = live["results"]["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"]["value"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Bolt", "Nut", "Washer"], "{live}");
    // An unregistered source resolves to nothing and is refused as any
    // unknown endpoint would be.
    let none = local_sparql_lenient(
        &app,
        &admin,
        "SELECT ?x WHERE { SERVICE <urn:source:nope> { ?x ?p ?o } }",
    )
    .await;
    assert!(
        none.as_ref().is_none_or(|v| v["results"]["bindings"]
            .as_array()
            .is_none_or(|b| b.is_empty())),
        "{none:?}"
    );

    // Deleting the source forgets its endpoint: the runs, the mappings and
    // then the source go, and the SERVICE that named it resolves to nothing.
    for run in listed.as_array().unwrap() {
        if run["mapping"].is_object() {
            let (st, _, txt) = req(
                &app,
                Method::DELETE,
                &format!("/api/runs/{}", run["id"].as_str().unwrap()),
                &admin,
                Value::Null,
            )
            .await;
            assert_eq!(st, StatusCode::NO_CONTENT, "{txt}");
        }
    }
    for id in ["vkg-products", "vkg-categories"] {
        let (st, _, txt) = req(
            &app,
            Method::DELETE,
            &format!("/api/mappings/{id}"),
            &admin,
            Value::Null,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT, "{txt}");
    }
    let (st, _, txt) = req(
        &app,
        Method::DELETE,
        "/api/sources/vkg",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{txt}");
    let gone = local_sparql_lenient(
        &app,
        &admin,
        &format!("SELECT ?name WHERE {{ SERVICE <urn:source:vkg> {{ ?s <{EX}name> ?name }} }}"),
    )
    .await;
    assert!(
        gone.as_ref().is_none_or(|v| v["results"]["bindings"]
            .as_array()
            .is_none_or(|b| b.is_empty())),
        "{gone:?}"
    );
}
