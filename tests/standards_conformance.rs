//! Per-graph standards & file-format conformance suite.
//!
//! This is the cohesive "does the platform actually support what the docs
//! advertise" overview. It complements — does not replace — the deep
//! per-standard suites:
//!   - SPARQL 1.1 ............ `w3c_sparql11_conformance.rs`, `sparqloscope_conformance.rs`
//!   - RDF 1.1 ............... `rdf11_conformance.rs`
//!   - RDFS / OWL 2 ......... `rdfs_conformance.rs`, `owl2_{rl,el,ql,dl}_conformance.rs`
//!   - GeoSPARQL ............ `geosparql_conformance.rs`
//!   - LDP .................. `ldp_conformance.rs`
//!
//! Two things are checked here that the deep suites don't:
//!   1. **Every advertised file format round-trips on every upload surface.**
//!      For each of the 7 formats (Turtle, N-Triples, N-Quads, TriG, RDF/XML,
//!      OWL/XML, JSON-LD) the same logical data is loaded into its *own named
//!      graph* via the bulk-import API and the Graph Store HTTP protocol, then
//!      read back. This is "a per-graph standards test, testing different file
//!      formats for all uploads".
//!   2. A single smoke per standard against the live HTTP surface, so a
//!      regression in routing/wiring is caught even when the deep algorithmic
//!      suite (which often calls the library directly) still passes.
//!
//! All tests drive an in-memory `AppState` through `tower::ServiceExt::oneshot`;
//! no port is bound and no disk I/O occurs.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt as _;
use oxigraph::sparql::QueryResults;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::Value;
use tower::ServiceExt as _;

use open_triplestore::{
    auth::{
        db::AuthDb,
        jwt::{issue_access_token, JwtConfig},
        models::SystemRole,
        oauth::new_session_store,
    },
    prefixes::PrefixRegistry,
    server::{build_router, AppState},
    storage::ObjectStore,
    store::TripleStore,
};

// ─── Shared harness ────────────────────────────────────────────────────────────

const JWT_SECRET: &str = "test_secret_must_be_32_chars_abcd";

fn test_state() -> AppState {
    let auth_db = Arc::new(AuthDb::in_memory().unwrap());
    let audit = Arc::new(open_triplestore::auth::audit::AuditLogger::new(
        auth_db.pool(),
    ));
    AppState {
        store: TripleStore::in_memory().unwrap(),
        prefix_registry: Arc::new(PrefixRegistry::empty()),
        auth_db,
        audit,
        backup: None,
        jwt_config: Arc::new(JwtConfig::new(JWT_SECRET.to_string(), 30, 30)),
        object_store: Arc::new(ObjectStore::noop()),
        mailer: Arc::new(open_triplestore::email::Mailer::log_only(
            "http://localhost:7878",
        )),
        base_url: Arc::new("http://localhost:7878".to_string()),
        oauth_sessions: new_session_store(),
        passkey_sessions: open_triplestore::auth::passkey::new_session_store(),
        auth_ext: Arc::new(open_triplestore::auth::oidc_rs::AuthExt::disabled()),
        query_timeout_secs: 30,
        write_timeout_secs: 120,
        secure_cookies: false,
        browse_semaphore: Arc::new(tokio::sync::Semaphore::new(64)),
        expensive_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
        #[cfg(feature = "text-search")]
        text_index: None,
        #[cfg(feature = "text-search")]
        text_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        vocab_catalog: Arc::new(open_triplestore::vocab_search::catalog::VocabCatalog::bundled()),
        vocab_registry_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        vocab_corpus: Arc::new(std::sync::RwLock::new(None)),
        #[cfg(feature = "vocab-search")]
        vocab_engine: None,
    }
}

/// Returns (state, admin_token) with a super_admin user — admins bypass graph
/// ACLs, so writes to arbitrary graphs are allowed.
fn admin_state() -> (AppState, String) {
    let state = test_state();
    state
        .auth_db
        .create_user(
            "adm",
            "admin",
            "admin@test.com",
            "hash",
            SystemRole::SuperAdmin,
        )
        .unwrap();
    let token = issue_access_token(
        &JwtConfig::new(JWT_SECRET.to_string(), 30, 30),
        "adm",
        "admin",
        "super_admin",
    )
    .unwrap();
    (state, token)
}

fn app(state: AppState) -> Router {
    build_router(state, "", vec![])
}

fn url_encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

async fn body_text(body: Body) -> String {
    let bytes = body.collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn body_json(body: Body) -> Value {
    serde_json::from_str(&body_text(body).await).unwrap_or(Value::Null)
}

/// Build a multipart/form-data body. Each part is `(name, content_type, optional filename, bytes)`.
fn multipart_body(boundary: &str, parts: &[(&str, &str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, content_type, filename, bytes) in parts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match filename {
            Some(fname) => out.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{fname}\"\r\n"
                )
                .as_bytes(),
            ),
            None => out.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n").as_bytes(),
            ),
        }
        out.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// Evaluate an ASK query directly against the store (no HTTP scoping).
fn ask(store: &TripleStore, query: &str) -> bool {
    match store.query(query).unwrap() {
        QueryResults::Boolean(b) => b,
        _ => panic!("expected an ASK/boolean result for: {query}"),
    }
}

// ─── File-format fixtures ───────────────────────────────────────────────────────
//
// Every fixture encodes the SAME three triples (subjects s1..s3, predicate p,
// literal objects v1..v3) so the expected count is always 3, regardless of
// serialization. Quad formats bake the target graph into the document; triple
// formats are routed to their graph via the upload's `default_target_graph`.

const TRIPLES_NT: &str = "\
<http://ex.org/std/s1> <http://ex.org/std/p> \"v1\" .
<http://ex.org/std/s2> <http://ex.org/std/p> \"v2\" .
<http://ex.org/std/s3> <http://ex.org/std/p> \"v3\" .
";

const NQUADS: &str = "\
<http://ex.org/std/s1> <http://ex.org/std/p> \"v1\" <http://ex.org/std/nq> .
<http://ex.org/std/s2> <http://ex.org/std/p> \"v2\" <http://ex.org/std/nq> .
<http://ex.org/std/s3> <http://ex.org/std/p> \"v3\" <http://ex.org/std/nq> .
";

const TRIG: &str = "\
<http://ex.org/std/trig> {
    <http://ex.org/std/s1> <http://ex.org/std/p> \"v1\" .
    <http://ex.org/std/s2> <http://ex.org/std/p> \"v2\" .
    <http://ex.org/std/s3> <http://ex.org/std/p> \"v3\" .
}
";

const RDFXML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://ex.org/std/">
  <rdf:Description rdf:about="http://ex.org/std/s1"><ex:p>v1</ex:p></rdf:Description>
  <rdf:Description rdf:about="http://ex.org/std/s2"><ex:p>v2</ex:p></rdf:Description>
  <rdf:Description rdf:about="http://ex.org/std/s3"><ex:p>v3</ex:p></rdf:Description>
</rdf:RDF>
"#;

const JSONLD: &str = r#"{
  "@context": { "p": "http://ex.org/std/p" },
  "@graph": [
    { "@id": "http://ex.org/std/s1", "p": "v1" },
    { "@id": "http://ex.org/std/s2", "p": "v2" },
    { "@id": "http://ex.org/std/s3", "p": "v3" }
  ]
}"#;

/// One advertised upload format. `embeds_graph` is true for quad formats whose
/// document carries its own graph name (so no `default_target_graph` is sent).
struct FormatCase {
    label: &'static str,
    filename: &'static str,
    mime: &'static str,
    body: &'static str,
    graph: &'static str,
    embeds_graph: bool,
}

fn format_cases() -> Vec<FormatCase> {
    vec![
        FormatCase {
            label: "Turtle",
            filename: "data.ttl",
            mime: "text/turtle",
            body: TRIPLES_NT,
            graph: "http://ex.org/std/ttl",
            embeds_graph: false,
        },
        FormatCase {
            label: "N-Triples",
            filename: "data.nt",
            mime: "application/n-triples",
            body: TRIPLES_NT,
            graph: "http://ex.org/std/nt",
            embeds_graph: false,
        },
        FormatCase {
            label: "RDF/XML",
            filename: "data.rdf",
            mime: "application/rdf+xml",
            body: RDFXML,
            graph: "http://ex.org/std/rdf",
            embeds_graph: false,
        },
        FormatCase {
            label: "OWL/XML",
            filename: "data.owl",
            mime: "application/rdf+xml",
            body: RDFXML,
            graph: "http://ex.org/std/owl",
            embeds_graph: false,
        },
        FormatCase {
            label: "JSON-LD",
            filename: "data.jsonld",
            mime: "application/ld+json",
            body: JSONLD,
            graph: "http://ex.org/std/jsonld",
            embeds_graph: false,
        },
        FormatCase {
            label: "N-Quads",
            filename: "data.nq",
            mime: "application/n-quads",
            body: NQUADS,
            graph: "http://ex.org/std/nq",
            embeds_graph: true,
        },
        FormatCase {
            label: "TriG",
            filename: "data.trig",
            mime: "application/trig",
            body: TRIG,
            graph: "http://ex.org/std/trig",
            embeds_graph: true,
        },
    ]
}

// ─── 1. Parser-level: the shared helper handles every format ─────────────────────
//
// Both the bulk-import and model-registry upload paths funnel through
// `data_models::upload::parse_rdf`, so proving it parses all 7 formats covers
// every upload surface at the parser layer.

#[test]
fn shared_parser_handles_all_formats() {
    for c in format_cases() {
        let quads =
            open_triplestore::data_models::upload::parse_rdf(c.body.as_bytes(), c.mime, c.filename)
                .unwrap_or_else(|e| panic!("{}: parse failed: {e}", c.label));
        assert_eq!(
            quads.len(),
            3,
            "{}: expected 3 triples, got {}",
            c.label,
            quads.len()
        );
    }
}

#[test]
fn extension_only_detection_for_owl_and_jsonld() {
    // No usable Content-Type: detection must fall back to the filename so that
    // a Protégé `.owl` (RDF/XML) and a `.jsonld` upload still parse.
    let owl = open_triplestore::data_models::upload::parse_rdf(
        RDFXML.as_bytes(),
        "application/octet-stream",
        "ontology.owl",
    )
    .expect(".owl must be detected by extension");
    assert_eq!(owl.len(), 3);

    let jsonld = open_triplestore::data_models::upload::parse_rdf(
        JSONLD.as_bytes(),
        "application/octet-stream",
        "graph.jsonld",
    )
    .expect(".jsonld must be detected by extension");
    assert_eq!(jsonld.len(), 3);
}

// ─── 2. Bulk import: each format → its own named graph ───────────────────────────

#[tokio::test]
async fn bulk_import_all_formats_per_graph() {
    let (state, token) = admin_state();

    for c in format_cases() {
        let boundary = "BNDfmt";
        let parts: Vec<(&str, &str, Option<&str>, &[u8])> = if c.embeds_graph {
            vec![("file", c.mime, Some(c.filename), c.body.as_bytes())]
        } else {
            vec![
                (
                    "default_target_graph",
                    "text/plain",
                    None,
                    c.graph.as_bytes(),
                ),
                ("file", c.mime, Some(c.filename), c.body.as_bytes()),
            ]
        };
        let body = multipart_body(boundary, &parts);

        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/import/bulk")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{}: bulk import HTTP status",
            c.label
        );
        let json = body_json(resp.into_body()).await;
        assert_eq!(
            json["success"], true,
            "{}: import not successful: {json}",
            c.label
        );

        let count = state.store.count_graph(Some(c.graph)).unwrap();
        assert_eq!(
            count, 3,
            "{}: graph <{}> should hold 3 triples",
            c.label, c.graph
        );
    }
}

// ─── 3. Graph Store HTTP Protocol: PUT each single-graph format ──────────────────

#[tokio::test]
async fn graph_store_put_all_triple_formats() {
    let (state, token) = admin_state();

    // Quad formats target multiple graphs and aren't meaningful for the
    // single-graph GSP PUT; the bulk path above covers them.
    for c in format_cases().into_iter().filter(|c| !c.embeds_graph) {
        let gsp_graph = format!("{}-gsp", c.graph);
        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/store?graph={}", url_encode(&gsp_graph)))
                    .header(header::CONTENT_TYPE, c.mime)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(c.body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::NO_CONTENT,
            "{}: Graph Store PUT should return 204",
            c.label
        );
        let count = state.store.count_graph(Some(&gsp_graph)).unwrap();
        assert_eq!(
            count, 3,
            "{}: GSP graph <{}> should hold 3 triples",
            c.label, gsp_graph
        );
    }
}

// ─── 4. SPARQL 1.1 Query: SELECT / ASK / CONSTRUCT / DESCRIBE ────────────────────

#[tokio::test]
async fn sparql_query_forms() {
    let (state, token) = admin_state();
    let g = "http://ex.org/std/query";
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{g}> {{ <http://ex.org/std/a> <http://ex.org/std/knows> <http://ex.org/std/b> }} }}"
        ))
        .unwrap();

    // SELECT
    let resp = sparql_get(
        &state,
        &token,
        &format!("SELECT * WHERE {{ GRAPH <{g}> {{ ?s ?p ?o }} }}"),
        "application/sparql-results+json",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let txt = body_text(resp.into_body()).await;
    assert!(
        txt.contains("http://ex.org/std/b"),
        "SELECT must return the object: {txt}"
    );

    // ASK
    let resp = sparql_get(
        &state,
        &token,
        &format!("ASK WHERE {{ GRAPH <{g}> {{ ?s ?p ?o }} }}"),
        "application/sparql-results+json",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        body_json(resp.into_body()).await["boolean"],
        true,
        "ASK must be true"
    );

    // CONSTRUCT
    let resp = sparql_get(
        &state,
        &token,
        &format!("CONSTRUCT {{ ?s ?p ?o }} WHERE {{ GRAPH <{g}> {{ ?s ?p ?o }} }}"),
        "text/turtle",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        body_text(resp.into_body()).await.contains("knows"),
        "CONSTRUCT must echo the predicate"
    );

    // DESCRIBE
    let resp = sparql_get(
        &state,
        &token,
        "DESCRIBE <http://ex.org/std/a>",
        "text/turtle",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "DESCRIBE must return 200");
}

async fn sparql_get(
    state: &AppState,
    token: &str,
    query: &str,
    accept: &str,
) -> axum::response::Response {
    app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/sparql?query={}", url_encode(query)))
                .header(header::ACCEPT, accept)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn sparql_update(state: &AppState, token: &str, update: &str) -> axum::response::Response {
    app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sparql")
                .header(header::CONTENT_TYPE, "application/sparql-update")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(update.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

// ─── 5. SPARQL 1.1 Update: INSERT / DELETE / COPY / CLEAR over HTTP ──────────────

#[tokio::test]
async fn sparql_update_operations() {
    let (state, token) = admin_state();
    let g = "http://ex.org/std/upd";
    let g_copy = "http://ex.org/std/upd-copy";

    let resp = sparql_update(
        &state,
        &token,
        &format!(
            "INSERT DATA {{ GRAPH <{g}> {{ \
                <http://ex.org/std/a> <http://ex.org/std/p> <http://ex.org/std/b> . \
                <http://ex.org/std/a> <http://ex.org/std/p> <http://ex.org/std/c> }} }}"
        ),
    )
    .await;
    assert!(resp.status().is_success(), "INSERT DATA must succeed");
    assert_eq!(state.store.count_graph(Some(g)).unwrap(), 2, "after INSERT");

    let resp = sparql_update(
        &state,
        &token,
        &format!(
            "DELETE DATA {{ GRAPH <{g}> {{ <http://ex.org/std/a> <http://ex.org/std/p> <http://ex.org/std/b> }} }}"
        ),
    )
    .await;
    assert!(resp.status().is_success(), "DELETE DATA must succeed");
    assert_eq!(state.store.count_graph(Some(g)).unwrap(), 1, "after DELETE");

    let resp = sparql_update(&state, &token, &format!("COPY <{g}> TO <{g_copy}>")).await;
    assert!(resp.status().is_success(), "COPY must succeed");
    assert_eq!(
        state.store.count_graph(Some(g_copy)).unwrap(),
        1,
        "after COPY"
    );

    let resp = sparql_update(&state, &token, &format!("CLEAR GRAPH <{g}>")).await;
    assert!(resp.status().is_success(), "CLEAR must succeed");
    assert_eq!(state.store.count_graph(Some(g)).unwrap(), 0, "after CLEAR");
}

// ─── 6. SPARQL 1.1 Graph Store HTTP Protocol: PUT / GET / DELETE ─────────────────

#[tokio::test]
async fn graph_store_protocol_crud() {
    let (state, token) = admin_state();
    let g = "http://ex.org/std/gsp-crud";
    let enc = url_encode(g);

    // PUT
    let resp = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/store?graph={enc}"))
                .header(header::CONTENT_TYPE, "text/turtle")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    "<http://ex.org/std/x> <http://ex.org/std/p> \"y\" .",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "PUT");
    assert_eq!(state.store.count_graph(Some(g)).unwrap(), 1);

    // GET
    let resp = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/store?graph={enc}"))
                .header(header::ACCEPT, "text/turtle")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET");
    assert!(body_text(resp.into_body())
        .await
        .contains("http://ex.org/std/x"));

    // DELETE
    let resp = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/store?graph={enc}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "DELETE");
    assert_eq!(state.store.count_graph(Some(g)).unwrap(), 0);
}

// ─── 7. SPARQL 1.1 Service Description at / ──────────────────────────────────────

#[tokio::test]
async fn service_description_advertises_capabilities() {
    let (state, token) = admin_state();
    let resp = app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let txt = body_text(resp.into_body()).await;
    assert!(
        txt.contains("sd:Service"),
        "must advertise an sd:Service: {txt}"
    );
    assert!(
        txt.contains("SPARQL11Query"),
        "must advertise SPARQL 1.1 Query"
    );
    assert!(
        txt.contains("SPARQL11Update"),
        "must advertise SPARQL 1.1 Update"
    );
}

// ─── 8. DCAT 2 / VoID dataset description at /.well-known/void ───────────────────

#[tokio::test]
async fn void_dataset_description() {
    let state = test_state();
    let resp = app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/.well-known/void")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let txt = body_text(resp.into_body()).await;
    assert!(
        txt.contains("void"),
        "VoID description must reference the void vocabulary: {txt}"
    );
    assert!(
        txt.contains("Dataset") || txt.contains("sparqlEndpoint"),
        "VoID description must describe the dataset/endpoint"
    );
}

// ─── 9. GeoSPARQL 1.1: WKT literals + topology function ──────────────────────────

#[tokio::test]
async fn geosparql_topology_function() {
    let (state, token) = admin_state();
    // Self-contained: a point inside a polygon. No data needed — exercises the
    // geof: extension function machinery and the geo:wktLiteral datatype.
    let query = "\
PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
ASK {
  BIND(\"POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))\"^^geo:wktLiteral AS ?poly)
  BIND(\"POINT(5 5)\"^^geo:wktLiteral AS ?pt)
  FILTER(geof:sfContains(?poly, ?pt))
}";
    let resp = sparql_get(&state, &token, query, "application/sparql-results+json").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        body_json(resp.into_body()).await["boolean"],
        true,
        "geof:sfContains(polygon, interior point) must be true"
    );
}

// ─── 10. SHACL Core: structural constraint validation ───────────────────────────

#[tokio::test]
async fn shacl_core_min_count() {
    let (state, _token) = admin_state();
    let shapes = "http://ex.org/std/shapes";
    let data = "http://ex.org/std/shdata";

    // The engine resolves property-shape constraints by IRI, so the property
    // shape must be a named node (not a blank `sh:property [ ... ]`).
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{shapes}> {{ \
                <http://ex.org/PersonShape> a <http://www.w3.org/ns/shacl#NodeShape> ; \
                    <http://www.w3.org/ns/shacl#targetClass> <http://ex.org/Person> ; \
                    <http://www.w3.org/ns/shacl#property> <http://ex.org/NameProp> . \
                <http://ex.org/NameProp> <http://www.w3.org/ns/shacl#path> <http://ex.org/name> ; \
                    <http://www.w3.org/ns/shacl#minCount> 1 }} }}"
        ))
        .unwrap();
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{data}> {{ <http://ex.org/alice> a <http://ex.org/Person> }} }}"
        ))
        .unwrap();

    let report =
        open_triplestore::shacl::validate(&state.store, shapes, &[data.to_string()]).unwrap();
    assert!(
        !report.conforms,
        "a Person with no name must violate minCount 1"
    );
    assert!(
        report.results_count >= 1,
        "expected at least one validation result"
    );

    // Fixing the data should make it conform.
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{data}> {{ <http://ex.org/alice> <http://ex.org/name> \"Alice\" }} }}"
        ))
        .unwrap();
    let report =
        open_triplestore::shacl::validate(&state.store, shapes, &[data.to_string()]).unwrap();
    assert!(report.conforms, "a Person with a name must conform");
}

// ─── 11. RDF 1.1: typed + language-tagged literals ───────────────────────────────

#[tokio::test]
async fn rdf11_typed_and_language_literals() {
    let (state, _token) = admin_state();
    let g = "http://ex.org/std/rdf11";
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{g}> {{ \
                <http://ex.org/std/s> <http://ex.org/std/age> \"42\"^^<http://www.w3.org/2001/XMLSchema#integer> ; \
                    <http://ex.org/std/label> \"hello\"@en }} }}"
        ))
        .unwrap();

    assert!(
        ask(
            &state.store,
            &format!(
                "ASK {{ GRAPH <{g}> {{ ?s <http://ex.org/std/age> ?a . \
                FILTER(DATATYPE(?a) = <http://www.w3.org/2001/XMLSchema#integer>) }} }}"
            )
        ),
        "xsd:integer datatype must be preserved"
    );
    assert!(
        ask(
            &state.store,
            &format!(
                "ASK {{ GRAPH <{g}> {{ ?s <http://ex.org/std/label> ?l . FILTER(LANG(?l) = \"en\") }} }}"
            )
        ),
        "language tag must be preserved"
    );
}

// ─── 12. JWT session authentication gates writes ─────────────────────────────────

#[tokio::test]
async fn jwt_required_for_writes() {
    let (state, token) = admin_state();
    let update =
        "INSERT DATA { <http://ex.org/std/a> <http://ex.org/std/b> <http://ex.org/std/c> }";

    // No token → rejected.
    let resp = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sparql")
                .header(header::CONTENT_TYPE, "application/sparql-update")
                .body(Body::from(update))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "anonymous UPDATE must be 401"
    );

    // Valid JWT → accepted.
    let resp = sparql_update(&state, &token, update).await;
    assert!(
        resp.status().is_success(),
        "JWT-authenticated UPDATE must succeed"
    );
}

// ─── 13. RDF-star / RDF 1.2: quoted (triple-term) statements ─────────────────────

#[cfg(feature = "rdf-12")]
#[tokio::test]
async fn rdf_star_quoted_triple_roundtrip() {
    let (state, _token) = admin_state();
    let g = "http://ex.org/std/rdfstar";
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{g}> {{ \
                << <http://ex.org/s> <http://ex.org/p> <http://ex.org/o> >> \
                <http://ex.org/note> \"asserted-by-test\" }} }}"
        ))
        .unwrap();
    assert!(
        ask(
            &state.store,
            &format!("ASK {{ GRAPH <{g}> {{ << ?s ?p ?o >> <http://ex.org/note> ?v }} }}")
        ),
        "a quoted triple must be queryable as a subject"
    );
}

// ─── 14. RDFS entailment (subClassOf) ────────────────────────────────────────────

#[cfg(feature = "rdfs-entailment")]
#[tokio::test]
async fn rdfs_subclass_entailment() {
    use open_triplestore::reasoning::common::RDFS_ENTAILMENT_GRAPH;
    use open_triplestore::reasoning::rdfs::RdfsMaterializer;

    let (state, token) = admin_state();
    state
        .store
        .update(
            "INSERT DATA { \
                <http://ex.org/Dog> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex.org/Animal> . \
                <http://ex.org/rex> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex.org/Dog> }",
        )
        .unwrap();
    RdfsMaterializer::with_target(&state.store, RDFS_ENTAILMENT_GRAPH)
        .materialize()
        .unwrap();

    assert!(
        ask(
            &state.store,
            &format!(
                "ASK {{ GRAPH <{RDFS_ENTAILMENT_GRAPH}> {{ \
                <http://ex.org/rex> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex.org/Animal> }} }}"
            )
        ),
        "RDFS must infer rex a Animal"
    );

    // The HTTP surface must accept the entailment regime selector.
    let resp = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sparql?query={}&entailment=rdfs",
                    url_encode("ASK { <http://ex.org/rex> a <http://ex.org/Animal> }")
                ))
                .header(header::ACCEPT, "application/sparql-results+json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "entailment=rdfs must be accepted"
    );
}

// ─── 15. OWL 2 RL entailment (subPropertyOf) ─────────────────────────────────────

#[cfg(feature = "owl2-rl")]
#[tokio::test]
async fn owl2_rl_subproperty_entailment() {
    use open_triplestore::reasoning::common::OWL2_RL_ENTAILMENT_GRAPH;
    use open_triplestore::reasoning::owl2_rl::Owl2RLReasoner;

    let (state, _token) = admin_state();
    state
        .store
        .update(
            "INSERT DATA { \
                <http://ex.org/p> <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> <http://ex.org/q> . \
                <http://ex.org/a> <http://ex.org/p> <http://ex.org/b> }",
        )
        .unwrap();
    Owl2RLReasoner::new(&state.store).materialize().unwrap();

    assert!(
        ask(
            &state.store,
            &format!(
                "ASK {{ GRAPH <{OWL2_RL_ENTAILMENT_GRAPH}> {{ \
                <http://ex.org/a> <http://ex.org/q> <http://ex.org/b> }} }}"
            )
        ),
        "OWL2-RL must infer a q b from subPropertyOf"
    );
}

// ─── 16. OWL 2 EL classification (subClassOf transitivity) ───────────────────────

#[cfg(feature = "owl2-el")]
#[tokio::test]
async fn owl2_el_classification() {
    use open_triplestore::reasoning::common::OWL2_EL_ENTAILMENT_GRAPH;
    use open_triplestore::reasoning::owl2_el::El2Classifier;

    let (state, _token) = admin_state();
    state
        .store
        .update(
            "INSERT DATA { \
                <http://ex.org/A> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex.org/B> . \
                <http://ex.org/B> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex.org/C> }",
        )
        .unwrap();
    El2Classifier::new(&state.store).classify().unwrap();

    assert!(
        ask(
            &state.store,
            &format!(
                "ASK {{ GRAPH <{OWL2_EL_ENTAILMENT_GRAPH}> {{ \
                <http://ex.org/A> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex.org/C> }} }}"
            )
        ),
        "OWL2-EL must classify A ⊑ C"
    );
}

// ─── 17. OWL 2 QL query rewriting (DL-Lite_R) ────────────────────────────────────

#[cfg(feature = "owl2-ql")]
#[tokio::test]
async fn owl2_ql_query_rewriting() {
    use open_triplestore::reasoning::owl2_ql::QLQueryRewriter;

    let (state, _token) = admin_state();
    // TBox: Manager ⊑ Employee ; ABox: x a Manager. A query for Employees must
    // return x once the QL rewriter expands the subclass axiom.
    state
        .store
        .update(
            "INSERT DATA { \
                <http://ex.org/Manager> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex.org/Employee> . \
                <http://ex.org/x> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex.org/Manager> }",
        )
        .unwrap();

    let rewriter = QLQueryRewriter::new(&state.store);
    let rewritten = rewriter
        .rewrite_query(
            "SELECT ?e WHERE { ?e <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex.org/Employee> }",
        )
        .unwrap();

    let rows = match state.store.query(&rewritten).unwrap() {
        QueryResults::Solutions(s) => s.count(),
        _ => 0,
    };
    assert!(
        rows >= 1,
        "QL rewriting must return the Manager instance as an Employee"
    );
}

// ─── 18. OWL 2 DL materialization (RL pipeline + bridge) ─────────────────────────

#[cfg(feature = "owl2-dl")]
#[tokio::test]
async fn owl2_dl_materialization() {
    use open_triplestore::reasoning::common::OWL2_DL_ENTAILMENT_GRAPH;
    use open_triplestore::reasoning::owl2_dl::Owl2DLReasoner;

    let (state, _token) = admin_state();
    state
        .store
        .update(
            "INSERT DATA { \
                <http://ex.org/A> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex.org/B> . \
                <http://ex.org/i> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex.org/A> }",
        )
        .unwrap();
    Owl2DLReasoner::new(&state.store).materialize().unwrap();

    assert!(
        ask(
            &state.store,
            &format!(
                "ASK {{ GRAPH <{OWL2_DL_ENTAILMENT_GRAPH}> {{ \
                <http://ex.org/i> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex.org/B> }} }}"
            )
        ),
        "OWL2-DL must infer i a B"
    );
}

// ─── 19. SHACL Advanced: SPARQL-based constraint ─────────────────────────────────

#[tokio::test]
async fn shacl_advanced_sparql_constraint() {
    let (state, _token) = admin_state();
    // Shapes in a named graph; data in the default graph and validated with an
    // empty data_graphs list — the configuration the engine evaluates SPARQL
    // constraints under.
    state
        .store
        .update(
            r#"INSERT DATA { GRAPH <http://ex.org/adv/shapes> {
                <http://ex.org/PersonShape> a <http://www.w3.org/ns/shacl#NodeShape> ;
                    <http://www.w3.org/ns/shacl#targetClass> <http://ex.org/Person> ;
                    <http://www.w3.org/ns/shacl#sparql> [
                        a <http://www.w3.org/ns/shacl#SPARQLConstraint> ;
                        <http://www.w3.org/ns/shacl#message> "Person must be at least 18." ;
                        <http://www.w3.org/ns/shacl#select> "SELECT ?value WHERE { $this <http://ex.org/age> ?value . FILTER(?value < 18) }"
                    ] .
            } }"#,
        )
        .unwrap();
    state
        .store
        .update(
            "INSERT DATA { <http://ex.org/bob> a <http://ex.org/Person> ; <http://ex.org/age> 15 }",
        )
        .unwrap();

    let report =
        open_triplestore::shacl::validate(&state.store, "http://ex.org/adv/shapes", &[]).unwrap();
    assert!(
        !report.conforms,
        "the SPARQL-based constraint must flag the under-18 person"
    );
}

// ─── 20. SHACL Compact Syntax (SHACL-C) parsing ──────────────────────────────────

#[tokio::test]
async fn shaclc_compact_syntax_parses() {
    let (state, token) = admin_state();
    let shaclc = "PREFIX schema: <http://schema.org/>\n\
                  PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\n\
                  shape schema:PersonShape -> schema:Person {\n\
                  \tschema:name xsd:string [1..1] ;\n\
                  }\n";
    let resp = app(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/shaclc/parse")
                .header(header::CONTENT_TYPE, "text/shaclc")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(shaclc))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "SHACL-C must parse");
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains("NodeShape") && body.contains("schema:name"),
        "SHACL-C should expand into a SHACL NodeShape: {body}"
    );
}

// ─── 21. OAuth 2.0 / OIDC provider advertisement ─────────────────────────────────

#[tokio::test]
async fn oauth_oidc_providers_endpoint() {
    let state = test_state();
    let resp = app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/auth/oauth/providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "OIDC providers list must be reachable"
    );
    assert!(
        body_json(resp.into_body()).await.is_array(),
        "providers endpoint must return a JSON array"
    );
}

// ─── 22. Linked Data Platform 1.0 ────────────────────────────────────────────────

#[cfg(feature = "ldp")]
#[tokio::test]
async fn ldp_routes_are_mounted() {
    // The LDP HTTP layer only exists when the `ldp` feature is compiled in. A
    // GET to the container root therefore reaches the LDP handler (any non-404
    // status) rather than falling through as an unknown route. Container CRUD
    // semantics are covered exhaustively by `ldp_conformance.rs`.
    let (state, token) = admin_state();
    let resp = app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/ldp/")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "LDP routes must be mounted when the ldp feature is enabled"
    );
}

// ─── 23. ShEx (Shape Expressions) ────────────────────────────────────────────────

#[cfg(feature = "shex")]
#[tokio::test]
async fn shex_validation_endpoint() {
    let (state, token) = admin_state();
    // shape_map maps a shape IRI to the focus nodes to validate against it.
    let body = serde_json::json!({
        "schema": "PREFIX ex: <http://ex.org/> ex:S { ex:name . }",
        "shape_map": { "http://ex.org/S": ["http://ex.org/a"] }
    });
    let resp = app(state)
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
    assert!(
        resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_REQUEST,
        "ShEx validate endpoint must respond, got {}",
        resp.status()
    );
}

// ─── 24. SWRL (Semantic Web Rule Language) ───────────────────────────────────────

#[cfg(feature = "swrl")]
#[tokio::test]
async fn swrl_execute_endpoint_wired() {
    let (state, token) = admin_state();
    // The SWRL execution route is compiled in; an empty body exercises wiring
    // (the handler/extractor responds rather than 404).
    let resp = app(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/swrl/execute")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "SWRL route must be mounted"
    );
    assert!(
        resp.status() != StatusCode::INTERNAL_SERVER_ERROR,
        "SWRL endpoint must handle the request gracefully, got {}",
        resp.status()
    );
}
