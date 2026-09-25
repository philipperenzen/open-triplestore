//! Datasource registry, mapping registry and run/rollback flow over HTTP,
//! against the SQLite connector that ships in core.
//!
//! Acceptance checks (written before the implementation):
//! * a datasource registered with `env:` / `file:` / `vault:` references
//!   connects; the stored graph and every API response carry the reference,
//!   never the value; a connection failure message contains neither the
//!   database location nor the password;
//! * a run produces `urn:run:<id>` with a PROV activity, swaps the production
//!   role atomically, and `rollback` restores the previous graph without
//!   re-running;
//! * a failing SHACL gate leaves production untouched and keeps the candidate
//!   graph inspectable.

mod common;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const DB_PASSWORD: &str = "s3cr3t-db-pa55word-never-shown";
const VAULT_TOKEN: &str = "hvs.test-token";

/// One scratch directory per test binary: `OTS_SOURCES_DIR` is process-global,
/// so every test's SQLite file lives under the same root.
fn sources_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("ots-sources-http-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("OTS_SOURCES_DIR", &dir);
        std::env::set_var("OTS_TEST_DB_PASSWORD", DB_PASSWORD);
        let secret_file = dir.join("db-password.txt");
        std::fs::write(&secret_file, format!("{DB_PASSWORD}\n")).unwrap();
        dir
    })
}

/// A minimal HashiCorp Vault KV v2 stand-in: `GET /v1/secret/data/sources/legacy`
/// answers the secret when the token header is right.
async fn start_mock_vault() -> String {
    use axum::routing::get;
    async fn kv2(headers: axum::http::HeaderMap) -> (StatusCode, axum::Json<Value>) {
        let tok = headers
            .get("x-vault-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if tok != VAULT_TOKEN {
            return (
                StatusCode::FORBIDDEN,
                axum::Json(json!({"errors": ["permission denied"]})),
            );
        }
        (
            StatusCode::OK,
            axum::Json(
                json!({"data": {"data": {"password": DB_PASSWORD}, "metadata": {"version": 1}}}),
            ),
        )
    }
    let app = Router::new().route("/v1/secret/data/sources/legacy", get(kv2));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn fresh_sqlite(name: &str) -> PathBuf {
    let path = sources_dir().join(format!("{name}-{}.db", uuid::Uuid::new_v4()));
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE suppliers (supplier_id INTEGER PRIMARY KEY, name TEXT NOT NULL, city TEXT);
         CREATE TABLE products (
             product_id INTEGER PRIMARY KEY,
             name TEXT NOT NULL,
             price REAL,
             status TEXT,
             supplier_id INTEGER REFERENCES suppliers(supplier_id),
             updated_at TEXT
         );
         INSERT INTO suppliers VALUES (10, 'Acme', 'Utrecht'), (20, 'Globex', 'Delft');
         INSERT INTO products VALUES
           (1, 'Bolt', 0.25, 'active', 10, '2026-01-01'),
           (2, 'Nut', 0.10, 'ACTIVE ', 10, '2026-01-02'),
           (3, 'Washer', NULL, 'retired', 20, '2026-01-03');",
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
    assert!(
        resp.status().is_success(),
        "PUT graph {graph}: {}",
        resp.status()
    );
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

async fn sparql_json(app: &Router, token: &str, query: &str) -> Value {
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
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

async fn create_dataset(app: &Router, token: &str, name: &str) -> String {
    let (st, v, txt) = req(
        app,
        Method::POST,
        "/api/datasets",
        token,
        json!({ "name": name, "owner_type": "user", "owner_id": "adm", "visibility": "private" }),
    )
    .await;
    assert!(st.is_success(), "create dataset: {st} {txt}");
    v["id"].as_str().unwrap().to_string()
}

fn source_body(id: &str, db: &Path, credential: &str, dataset: Option<&str>) -> Value {
    let mut b = json!({
        "id": id,
        "name": format!("Source {id}"),
        "dialect": "sqlite",
        "database": db.to_string_lossy(),
        "credential": credential,
        "readOnly": true,
        "statementTimeoutMs": 5000,
        "watermarkColumn": "updated_at",
        "allowModelAssist": false,
    });
    if let Some(ds) = dataset {
        b["dataset"] = json!(ds);
    }
    b
}

const MAPPING: &str = r#"
@prefix rr:   <http://www.w3.org/ns/r2rml#> .
@prefix rml:  <http://semweb.mmlab.be/ns/rml#> .
@prefix ql:   <http://semweb.mmlab.be/ns/ql#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:   <http://example.org/products/ontology#> .
@prefix fn:   <https://w3id.org/open-triplestore/fn#> .
@prefix fnml: <http://semweb.mmlab.be/ns/fnml#> .
@prefix fno:  <https://w3id.org/function/ontology#> .

ex:ProductsMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:SOURCE> ; rml:referenceFormulation ql:SQL2008 ;
                      rml:query "SELECT product_id, name, price, status, supplier_id FROM products" ] ;
  rr:subjectMap [ rr:template "http://example.org/products/product_{product_id}" ; rr:class ex:Product ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:hasPrice ; rr:objectMap [ rr:column "price" ; rr:datatype xsd:decimal ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:suppliedBy ;
    rr:objectMap [ rr:parentTriplesMap ex:SuppliersMap ; rr:joinCondition [ rr:child "supplier_id" ; rr:parent "supplier_id" ] ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:hasStatus ;
    rr:objectMap [ fnml:functionValue [
        rr:predicateObjectMap [ rr:predicate fno:executes ; rr:object fn:mapValue ] ;
        rr:predicateObjectMap [ rr:predicate fn:value ; rr:objectMap [ rr:column "status" ] ] ;
        rr:predicateObjectMap [ rr:predicate fn:normalize ; rr:object "lower_trim" ] ;
        rr:predicateObjectMap [ rr:predicate fn:mapping ; rr:object "active=http://example.org/products/ontology#Active" ] ;
        rr:predicateObjectMap [ rr:predicate fn:unmapped ; rr:object "literal" ] ] ] ] .

ex:SuppliersMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:SOURCE> ; rml:referenceFormulation ql:SQL2008 ; rr:tableName "suppliers" ] ;
  rr:subjectMap [ rr:template "http://example.org/products/supplier_{supplier_id}" ; rr:class ex:Supplier ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] .
"#;

fn mapping_for(source_id: &str) -> String {
    MAPPING.replace("SOURCE", source_id)
}

async fn register_mapping(
    app: &Router,
    token: &str,
    id: &str,
    source_id: &str,
    shapes: Option<&str>,
) -> Value {
    let mut body = json!({ "id": id, "title": "Products mapping", "rml": mapping_for(source_id) });
    if let Some(s) = shapes {
        body["shapesGraph"] = json!(s);
    }
    let (st, v, txt) = req(app, Method::POST, "/api/mappings", token, body).await;
    assert_eq!(st, StatusCode::CREATED, "register mapping: {txt}");
    v
}

// ───────────────────────── Registration & secrets ─────────────────────────

#[tokio::test]
async fn env_and_file_references_connect_and_are_never_expanded() {
    let dir = sources_dir().clone();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("refs");

    for (id, credential) in [
        ("src-env", "env:OTS_TEST_DB_PASSWORD".to_string()),
        (
            "src-file",
            format!("file:{}", dir.join("db-password.txt").display()),
        ),
    ] {
        // Test-connection never persists and reports the resolved reference status.
        let (st, v, txt) = req(
            &app,
            Method::POST,
            "/api/sources/test",
            &token,
            source_body(id, &db, &credential, None),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "test connection {id}: {txt}");
        assert_eq!(v["ok"], true, "test connection {id}: {txt}");
        assert!(
            !txt.contains(DB_PASSWORD),
            "test response leaked the secret: {txt}"
        );
        let (st, _, _) = req(
            &app,
            Method::GET,
            &format!("/api/sources/{id}"),
            &token,
            Value::Null,
        )
        .await;
        assert_eq!(
            st,
            StatusCode::NOT_FOUND,
            "test connection must not persist"
        );

        let (st, v, txt) = req(
            &app,
            Method::POST,
            "/api/sources",
            &token,
            source_body(id, &db, &credential, None),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "register {id}: {txt}");
        assert_eq!(
            v["credential"], credential,
            "response carries the reference: {txt}"
        );
        assert_eq!(v["iri"], format!("urn:source:{id}"));
        assert_eq!(v["readOnly"], true);
        assert!(
            !txt.contains(DB_PASSWORD),
            "registration response leaked the secret: {txt}"
        );

        let (st, v, txt) = req(
            &app,
            Method::GET,
            &format!("/api/sources/{id}"),
            &token,
            Value::Null,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert_eq!(v["credential"], credential);
        assert!(!txt.contains(DB_PASSWORD), "GET leaked the secret: {txt}");

        // The schema is reachable through the registered source.
        let (st, v, txt) = req(
            &app,
            Method::GET,
            &format!("/api/sources/{id}/introspect"),
            &token,
            Value::Null,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "introspect {id}: {txt}");
        let tables: Vec<&str> = v["tables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(
            tables.contains(&"products") && tables.contains(&"suppliers"),
            "{txt}"
        );
        let products = v["tables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "products")
            .unwrap();
        assert!(products["columns"].as_array().unwrap().len() >= 5, "{txt}");
        assert_eq!(products["primaryKey"], json!(["product_id"]), "{txt}");
        assert_eq!(products["foreignKeys"][0]["refTable"], "suppliers", "{txt}");
    }

    // The list carries references only.
    let (st, v, txt) = req(&app, Method::GET, "/api/sources", &token, Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    assert!(v.as_array().unwrap().len() >= 2, "{txt}");
    assert!(!txt.contains(DB_PASSWORD));

    // The stored graph carries the reference, never the value.
    let stored = sparql_json(
        &app,
        &token,
        "SELECT ?o WHERE { GRAPH <urn:system:sources> { ?s ?p ?o } }",
    )
    .await;
    let dump = stored.to_string();
    assert!(
        dump.contains("env:OTS_TEST_DB_PASSWORD"),
        "reference stored: {dump}"
    );
    assert!(
        !dump.contains(DB_PASSWORD),
        "stored graph leaked the secret: {dump}"
    );
}

#[tokio::test]
async fn vault_reference_resolves_through_kv_v2() {
    sources_dir();
    let vault = start_mock_vault().await;
    std::env::set_var("VAULT_ADDR", &vault);
    std::env::set_var("VAULT_TOKEN", VAULT_TOKEN);
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("vault");

    let credential = "vault:secret/data/sources/legacy#password";
    let (st, v, txt) = req(
        &app,
        Method::POST,
        "/api/sources",
        &token,
        source_body("src-vault", &db, credential, None),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register with vault ref: {txt}");
    assert_eq!(v["credential"], credential);
    assert!(!txt.contains(DB_PASSWORD));

    // An unresolvable reference (wrong key) is a registration error, not a warning.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources",
        &token,
        source_body(
            "src-vault-bad",
            &db,
            "vault:secret/data/sources/legacy#nope",
            None,
        ),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(!txt.contains(DB_PASSWORD));
}

#[tokio::test]
async fn malformed_or_unresolvable_references_are_rejected() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("badrefs");

    for bad in [
        "env:OTS_DOES_NOT_EXIST_XYZ",
        "file:/nonexistent/secret",
        "nonsense",
        "",
    ] {
        let (st, _, txt) = req(
            &app,
            Method::POST,
            "/api/sources",
            &token,
            source_body("src-bad", &db, bad, None),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "credential {bad:?}: {txt}");
    }
    // A read-write account is refused: read-only is not optional.
    let mut body = source_body("src-rw", &db, "env:OTS_TEST_DB_PASSWORD", None);
    body["readOnly"] = json!(false);
    let (st, _, txt) = req(&app, Method::POST, "/api/sources", &token, body).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "read-write source: {txt}");
    // An unknown dialect is refused with the list of what is compiled in.
    let mut body = source_body("src-dialect", &db, "env:OTS_TEST_DB_PASSWORD", None);
    body["dialect"] = json!("oracle");
    let (st, _, txt) = req(&app, Method::POST, "/api/sources", &token, body).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "unknown dialect: {txt}");
    assert!(
        txt.contains("sqlite"),
        "names the available dialects: {txt}"
    );
}

#[tokio::test]
async fn connection_failure_messages_are_scrubbed() {
    let dir = sources_dir().clone();
    let (state, token) = admin_state();
    let app = test_app(state);
    let missing = dir.join("does-not-exist-9f1c.db");
    let (st, v, txt) = req(
        &app,
        Method::POST,
        "/api/sources/test",
        &token,
        source_body("src-missing", &missing, "env:OTS_TEST_DB_PASSWORD", None),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["ok"], false, "{txt}");
    let msg = v["error"].as_str().unwrap_or("");
    assert!(!msg.is_empty());
    assert!(!msg.contains(DB_PASSWORD), "password in error: {msg}");
    assert!(
        !msg.contains("does-not-exist-9f1c"),
        "database location in error: {msg}"
    );
}

#[tokio::test]
async fn sources_are_admin_only() {
    sources_dir();
    let (state, _admin) = admin_state();
    state
        .auth_db
        .create_user(
            "usr",
            "user",
            "user@test.com",
            "hash",
            open_triplestore::auth::models::SystemRole::User,
        )
        .unwrap();
    let user = mint_token("usr", "user", "user");
    let app = test_app(state);
    let (st, _, _) = req(&app, Method::GET, "/api/sources", &user, Value::Null).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = req(&app, Method::GET, "/api/mappings", &user, Value::Null).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sources")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ───────────────────────────── Runs & rollback ─────────────────────────────

#[tokio::test]
async fn run_materialises_swaps_and_rolls_back_without_rerunning() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("runs");
    let ds = create_dataset(&app, &token, "assets").await;

    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources",
        &token,
        source_body("legacy", &db, "env:OTS_TEST_DB_PASSWORD", Some(&ds)),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let mapping = register_mapping(&app, &token, "products-map", "legacy", None).await;
    assert_eq!(mapping["version"], 1);
    assert_eq!(mapping["iri"], "urn:mapping:products-map");

    // ── First run ──
    let (st, run1, txt) = req(
        &app,
        Method::POST,
        "/api/sources/legacy/runs",
        &token,
        json!({ "mapping": "products-map", "mode": "full" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "run 1: {txt}");
    assert_eq!(run1["status"], "succeeded", "{txt}");
    let run1_id = run1["id"].as_str().unwrap().to_string();
    let graph1 = run1["graph"].as_str().unwrap().to_string();
    assert_eq!(graph1, format!("urn:run:{run1_id}"));
    assert_eq!(run1["rowsExtracted"], 5, "3 products + 2 suppliers: {txt}");
    let triples1 = run1["triplesProduced"].as_u64().unwrap();
    assert!(triples1 >= 12, "{txt}");
    let (_, detail1, _) = req(
        &app,
        Method::GET,
        &format!("/api/runs/{run1_id}"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(
        detail1["graphTriples"].as_u64().unwrap(),
        triples1,
        "the run graph holds what it produced"
    );

    // The generated data: join, typed literal, NULL → no triple, enumeration policy.
    let q = |ask: &str| {
        format!("PREFIX ex: <http://example.org/products/ontology#> ASK {{ GRAPH <{graph1}> {{ {ask} }} }}")
    };
    for ask in [
        "<http://example.org/products/product_1> a ex:Product ; ex:suppliedBy <http://example.org/products/supplier_10> .",
        "<http://example.org/products/product_1> ex:hasPrice \"0.25\"^^<http://www.w3.org/2001/XMLSchema#decimal> .",
        "<http://example.org/products/product_1> ex:hasStatus ex:Active .",
        "<http://example.org/products/product_2> ex:hasStatus ex:Active .",
        "<http://example.org/products/product_3> ex:hasStatus \"retired\" .",
        "<http://example.org/products/supplier_20> a ex:Supplier ; ex:name \"Globex\" .",
    ] {
        let v = sparql_json(&app, &token, &q(ask)).await;
        assert_eq!(v["boolean"], true, "expected in run graph: {ask}");
    }
    let v = sparql_json(
        &app,
        &token,
        &q("<http://example.org/products/product_3> ex:hasPrice ?p ."),
    )
    .await;
    assert_eq!(v["boolean"], false, "NULL price produces no triple");

    // PROV: the run is an activity that used the source and the mapping
    // version, and generated the graph. Served as Turtle, because the records
    // live in a system graph that the SPARQL endpoint scopes out of a
    // caller's dataset.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/runs/{run1_id}/provenance"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // Served as prefixed Turtle; the checks below are about the statements,
    // not the layout, so read it back as N-Triples.
    let prov = as_ntriples(&body_text(resp.into_body()).await);
    for expected in [
        &format!("<urn:run:{run1_id}:activity>"),
        "http://www.w3.org/ns/prov#Activity",
        "urn:source:legacy",
        "urn:mapping:products-map:version:1",
        "http://www.w3.org/ns/prov#generated",
        &graph1,
        "prov#startedAtTime",
        "prov#endedAtTime",
    ] {
        assert!(
            prov.contains(expected),
            "provenance is missing {expected}:\n{prov}"
        );
    }

    // The source now points at the run graph, and the dataset holds it as instances.
    let (st, src, txt) = req(
        &app,
        Method::GET,
        "/api/sources/legacy",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(src["production"]["graph"], graph1, "{txt}");
    assert_eq!(src["production"]["run"], run1_id, "{txt}");
    let (_, graphs, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/graphs"),
        &token,
        Value::Null,
    )
    .await;
    let entries = graphs
        .as_array()
        .cloned()
        .or_else(|| graphs["graphs"].as_array().cloned())
        .unwrap();
    let entry = entries
        .iter()
        .find(|e| e["graph_iri"] == graph1)
        .unwrap_or_else(|| panic!("run graph registered: {txt}"));
    assert_eq!(entry["graph_role"], "instances");

    // ── Second run supersedes the first; the first graph is kept, demoted ──
    let (st, run2, txt) = req(
        &app,
        Method::POST,
        "/api/sources/legacy/runs",
        &token,
        json!({ "mapping": "products-map", "mode": "full" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "run 2: {txt}");
    let run2_id = run2["id"].as_str().unwrap().to_string();
    let graph2 = run2["graph"].as_str().unwrap().to_string();
    assert_ne!(graph2, graph1);
    assert_eq!(run2["previousGraph"], graph1, "{txt}");
    let (_, src, _) = req(
        &app,
        Method::GET,
        "/api/sources/legacy",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(src["production"]["graph"], graph2);
    assert_eq!(src["previous"]["graph"], graph1);
    let (_, demoted, txt) = req(
        &app,
        Method::GET,
        &format!("/api/runs/{run1_id}"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(
        demoted["graphTriples"].as_u64().unwrap(),
        triples1,
        "demoted graph is kept: {txt}"
    );
    let (_, graphs, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/graphs"),
        &token,
        Value::Null,
    )
    .await;
    let entries = graphs
        .as_array()
        .cloned()
        .or_else(|| graphs["graphs"].as_array().cloned())
        .unwrap();
    assert!(entries.iter().any(|e| e["graph_iri"] == graph2));
    assert!(
        !entries.iter().any(|e| e["graph_iri"] == graph1),
        "demoted graph left the dataset"
    );

    // ── Rollback re-points, it does not re-run ──
    let (_, runs_before, _) = req(
        &app,
        Method::GET,
        "/api/sources/legacy/runs",
        &token,
        Value::Null,
    )
    .await;
    let n_before = runs_before.as_array().unwrap().len();
    let (st, rb, txt) = req(
        &app,
        Method::POST,
        &format!("/api/runs/{run2_id}/rollback"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "rollback: {txt}");
    assert_eq!(rb["production"]["graph"], graph1, "{txt}");
    let (_, src, _) = req(
        &app,
        Method::GET,
        "/api/sources/legacy",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(src["production"]["graph"], graph1);
    assert_eq!(src["production"]["run"], run1_id);
    let (_, runs_after, _) = req(
        &app,
        Method::GET,
        "/api/sources/legacy/runs",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(
        runs_after.as_array().unwrap().len(),
        n_before,
        "rollback did not create a run"
    );
    let (_, graphs, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/graphs"),
        &token,
        Value::Null,
    )
    .await;
    let entries = graphs
        .as_array()
        .cloned()
        .or_else(|| graphs["graphs"].as_array().cloned())
        .unwrap();
    assert!(entries
        .iter()
        .any(|e| e["graph_iri"] == graph1 && e["graph_role"] == "instances"));
    assert!(!entries.iter().any(|e| e["graph_iri"] == graph2));
    // The rollback is itself provenance on the source.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sources/legacy/provenance")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let trail = as_ntriples(&body_text(resp.into_body()).await);
    assert!(
        trail.contains("datasource#Rollback"),
        "no rollback recorded:\n{trail}"
    );
    assert!(
        trail.contains(&format!("<urn:run:{run1_id}:activity>")),
        "{trail}"
    );

    // Metrics summarise the runs.
    let (st, m, txt) = req(
        &app,
        Method::GET,
        "/api/sources/metrics",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(m["runs"]["total"].as_u64().unwrap() >= 2, "{txt}");
    assert!(m["rowsExtracted"].as_u64().unwrap() >= 10, "{txt}");

    // Run details are readable, and a run that is not production can be deleted.
    let (st, detail, txt) = req(
        &app,
        Method::GET,
        &format!("/api/runs/{run2_id}"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(detail["mapping"]["version"], 1, "{txt}");
    let (st, _, txt) = req(
        &app,
        Method::DELETE,
        &format!("/api/runs/{run1_id}"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "production graph cannot be deleted: {txt}"
    );
    let (st, _, txt) = req(
        &app,
        Method::DELETE,
        &format!("/api/runs/{run2_id}"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{txt}");
    let (st, _, _) = req(
        &app,
        Method::GET,
        &format!("/api/runs/{run2_id}"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "the deleted run is gone with its graph"
    );

    // Deleting the run `previous` pointed at must not leave a rollback target
    // that no longer exists: the pointer is cleared with the run.
    let (_, src, txt) = req(
        &app,
        Method::GET,
        "/api/sources/legacy",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(
        src["production"]["run"], run1_id,
        "production is untouched by the delete: {txt}"
    );
    assert!(
        src.get("previous").is_none(),
        "the dangling previous pointer was cleared: {txt}"
    );
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/runs/{run1_id}/rollback"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "with nothing to roll back to, rollback refuses rather than dangling: {txt}"
    );

    // Mapping versions: a PUT freezes a new version; runs keep pointing at the old one.
    let (st, m2, txt) = req(
        &app,
        Method::PUT,
        "/api/mappings/products-map",
        &token,
        json!({ "rml": mapping_for("legacy").replace("ex:name", "ex:label") }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(m2["version"], 2, "{txt}");
    let (st, _, txt) = req(
        &app,
        Method::GET,
        "/api/mappings/products-map/rml?version=1",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(
        txt.contains("ex:name") || txt.contains("ontology#name"),
        "version 1 is frozen: {txt}"
    );
    let (_, detail, _) = req(
        &app,
        Method::GET,
        &format!("/api/runs/{run1_id}"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(detail["mapping"]["version"], 1);
}

#[tokio::test]
async fn failing_shacl_gate_leaves_production_untouched_and_keeps_candidate() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("gate");
    let ds = create_dataset(&app, &token, "gated").await;
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources",
        &token,
        source_body("gated", &db, "env:OTS_TEST_DB_PASSWORD", Some(&ds)),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");

    // A first, ungated run establishes production.
    register_mapping(&app, &token, "gated-map", "gated", None).await;
    let (st, run1, txt) = req(
        &app,
        Method::POST,
        "/api/sources/gated/runs",
        &token,
        json!({ "mapping": "gated-map", "mode": "full" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let graph1 = run1["graph"].as_str().unwrap().to_string();

    // Shapes that every product violates (a property the mapping never emits).
    let shapes = "urn:test:shapes:products";
    put_turtle(
        &app,
        &token,
        shapes,
        r#"@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example.org/products/ontology#> .
           <urn:test:ProductShape> a sh:NodeShape ; sh:targetClass ex:Product ;
             sh:property [ sh:path ex:sku ; sh:minCount 1 ] ."#,
    )
    .await;
    register_mapping(&app, &token, "gated-map-2", "gated", Some(shapes)).await;
    let (st, rejected, txt) = req(
        &app,
        Method::POST,
        "/api/sources/gated/runs",
        &token,
        json!({ "mapping": "gated-map-2", "mode": "full" }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "gate must fail: {txt}"
    );
    let run = &rejected["run"];
    assert_eq!(run["status"], "rejected", "{txt}");
    assert_eq!(rejected["report"]["conforms"], false, "{txt}");
    assert!(
        rejected["report"]["results"].as_array().unwrap().len() >= 3,
        "{txt}"
    );
    let candidate = run["graph"].as_str().unwrap().to_string();
    assert_ne!(candidate, graph1);

    // Production is untouched; the candidate stays inspectable.
    let (_, src, _) = req(&app, Method::GET, "/api/sources/gated", &token, Value::Null).await;
    assert_eq!(src["production"]["graph"], graph1);

    let (st, detail, txt) = req(
        &app,
        Method::GET,
        &format!("/api/runs/{}", run["id"].as_str().unwrap()),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(detail["status"], "rejected");
    assert_eq!(detail["shacl"]["conforms"], false, "{txt}");
    assert!(
        detail["graphTriples"].as_u64().unwrap() > 0,
        "the candidate graph is kept for inspection: {txt}"
    );
    let (_, graphs, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/graphs"),
        &token,
        Value::Null,
    )
    .await;
    let entries = graphs
        .as_array()
        .cloned()
        .or_else(|| graphs["graphs"].as_array().cloned())
        .unwrap();
    assert!(
        !entries.iter().any(|e| e["graph_iri"] == candidate),
        "candidate never joined the dataset"
    );
    assert!(entries.iter().any(|e| e["graph_iri"] == graph1));
}

#[tokio::test]
async fn run_refuses_a_mapping_bound_to_another_source() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("other");
    for id in ["alpha", "beta"] {
        let (st, _, txt) = req(
            &app,
            Method::POST,
            "/api/sources",
            &token,
            source_body(id, &db, "env:OTS_TEST_DB_PASSWORD", None),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{txt}");
    }
    register_mapping(&app, &token, "alpha-map", "alpha", None).await;
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources/beta/runs",
        &token,
        json!({ "mapping": "alpha-map", "mode": "full" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    // An incremental run builds on the graph in production; without one there
    // is nothing to build on, and it says so rather than silently running full.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources/alpha/runs",
        &token,
        json!({ "mapping": "alpha-map", "mode": "watermark" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(
        txt.contains("full"),
        "the message names the way forward: {txt}"
    );
}

#[tokio::test]
async fn a_watermark_run_re_maps_only_what_moved_and_keeps_the_rest() {
    // A table-sourced mapping, so the catalogue can confirm the cursor column
    // exists. A `rml:query` source is opaque to it and is read in full.
    const TABLE_MAPPING: &str = r#"
@prefix rr:  <http://www.w3.org/ns/r2rml#> .
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix ex:  <http://example.org/products/ontology#> .

ex:ProductsMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:inc> ; rr:tableName "products" ] ;
  rr:subjectMap [ rr:template "http://example.org/products/product_{product_id}" ; rr:class ex:Product ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] .

ex:SuppliersMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:inc> ; rr:tableName "suppliers" ] ;
  rr:subjectMap [ rr:template "http://example.org/products/supplier_{supplier_id}" ; rr:class ex:Supplier ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] .
"#;
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("incremental");
    let ds = create_dataset(&app, &token, "incremental").await;
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources",
        &token,
        source_body("inc", &db, "env:OTS_TEST_DB_PASSWORD", Some(&ds)),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/mappings",
        &token,
        json!({ "id": "inc-map", "title": "Incremental", "rml": TABLE_MAPPING }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register mapping: {txt}");

    // A full run establishes production and records the cursor.
    let (st, full, txt) = req(
        &app,
        Method::POST,
        "/api/sources/inc/runs",
        &token,
        json!({ "mapping": "inc-map", "mode": "full" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(full["rowsExtracted"], 5, "3 products + 2 suppliers");
    assert_eq!(
        full["watermark"], "2026-01-03",
        "a full run records the cursor an incremental one resumes from: {txt}"
    );
    let full_triples = full["triplesProduced"].as_u64().unwrap();

    // One row changes and one appears, both past the cursor.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "UPDATE products SET name = 'Bolt Mk2', updated_at = '2026-02-01' WHERE product_id = 1;
             INSERT INTO products VALUES (4, 'Rivet', 0.40, 'active', 20, '2026-02-02');",
        )
        .unwrap();
    }

    let (st, inc, txt) = req(
        &app,
        Method::POST,
        "/api/sources/inc/runs",
        &token,
        json!({ "mapping": "inc-map", "mode": "watermark" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "incremental run: {txt}");
    assert_eq!(inc["status"], "succeeded");
    assert_eq!(inc["mode"], "watermark");
    assert_eq!(
        inc["rowsExtracted"], 4,
        "2 changed products + 2 suppliers, which carry no watermark column: {txt}"
    );
    assert_eq!(inc["watermark"], "2026-02-02", "the cursor advanced: {txt}");

    let graph = inc["graph"].as_str().unwrap().to_string();
    let ask = |pattern: &str| {
        format!(
            "PREFIX ex: <http://example.org/products/ontology#> ASK {{ GRAPH <{graph}> {{ {pattern} }} }}"
        )
    };

    // The untouched rows came along from the previous graph…
    let v = sparql_json(
        &app,
        &token,
        &ask("<http://example.org/products/product_2> ex:name \"Nut\" ."),
    )
    .await;
    assert_eq!(
        v["boolean"], true,
        "an unchanged entity survives the increment"
    );
    // …the changed one was re-mapped…
    let v = sparql_json(
        &app,
        &token,
        &ask("<http://example.org/products/product_1> ex:name \"Bolt Mk2\" ."),
    )
    .await;
    assert_eq!(
        v["boolean"], true,
        "the changed entity carries its new value"
    );
    // …and its stale value is gone, rather than sitting beside the new one.
    let v = sparql_json(
        &app,
        &token,
        &ask("<http://example.org/products/product_1> ex:name \"Bolt\" ."),
    )
    .await;
    assert_eq!(
        v["boolean"], false,
        "the superseded value was replaced, not merged"
    );
    // …and the new row is there.
    let v = sparql_json(
        &app,
        &token,
        &ask("<http://example.org/products/product_4> ex:name \"Rivet\" ."),
    )
    .await;
    assert_eq!(v["boolean"], true);

    // The candidate is a COMPLETE graph, so the gate, the swap and rollback all
    // behave as they do for a full run.
    let inc_id = inc["id"].as_str().unwrap().to_string();
    let (_, detail, _) = req(
        &app,
        Method::GET,
        &format!("/api/runs/{inc_id}"),
        &token,
        Value::Null,
    )
    .await;
    assert!(
        detail["graphTriples"].as_u64().unwrap() > full_triples,
        "the increment added an entity to the whole graph, it did not replace it: {detail}"
    );
    let (_, src, _) = req(&app, Method::GET, "/api/sources/inc", &token, Value::Null).await;
    assert_eq!(src["production"]["run"], inc_id);

    let (st, rb, txt) = req(
        &app,
        Method::POST,
        &format!("/api/runs/{inc_id}/rollback"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(
        rb["production"]["run"], full["id"],
        "rollback returns the pre-increment graph"
    );
}

#[tokio::test]
async fn a_watermark_run_needs_a_watermark_column() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("nocursor");
    let mut body = source_body("nocursor", &db, "env:OTS_TEST_DB_PASSWORD", None);
    body.as_object_mut().unwrap().remove("watermarkColumn");
    let (st, _, txt) = req(&app, Method::POST, "/api/sources", &token, body).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    register_mapping(&app, &token, "nocursor-map", "nocursor", None).await;
    let (st, _, _) = req(
        &app,
        Method::POST,
        "/api/sources/nocursor/runs",
        &token,
        json!({ "mapping": "nocursor-map", "mode": "full" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources/nocursor/runs",
        &token,
        json!({ "mapping": "nocursor-map", "mode": "watermark" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("watermarkColumn"), "{txt}");
}
