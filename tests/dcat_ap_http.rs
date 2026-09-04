//! DCAT-AP-NL catalogue (7.2): under `DCAT_PROFILE=dcat-ap-nl` the catalogue
//! carries the application profile's mandatory properties — proven by
//! validating the served document against a SHACL shape set that encodes
//! the DCAT-AP 3 / DCAT-AP-NL 3 mandatory-property tables — is negotiable in
//! JSON-LD and RDF/XML, advertises LDES streams, counts named-graph data in
//! its VoID statistics, and cannot be corrupted by hostile metadata.
//!
//! Own binary: `DCAT_PROFILE` and `CATALOG_*` are process-wide.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

/// The mandatory-property tables of DCAT-AP 3 (Catalogue, Dataset,
/// Distribution, Agent, Data Service) plus DCAT-AP-NL 3's additions on
/// Dataset (publisher, identifier, language) and Distribution (format,
/// media type, licence).
const AP_NL_SHAPES: &str = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix dcat: <http://www.w3.org/ns/dcat#> .
@prefix dct: <http://purl.org/dc/terms/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix ex: <urn:dcat-ap-nl:> .
ex:Catalog a sh:NodeShape ; sh:targetClass dcat:Catalog ;
  sh:property [ sh:path dct:title ; sh:minCount 1 ] ,
              [ sh:path dct:description ; sh:minCount 1 ] ,
              [ sh:path dct:publisher ; sh:minCount 1 ; sh:class foaf:Agent ] ,
              [ sh:path dcat:dataset ; sh:minCount 1 ] ,
              [ sh:path dct:language ; sh:minCount 1 ; sh:nodeKind sh:IRI ] ,
              [ sh:path dct:modified ; sh:minCount 1 ] ,
              [ sh:path foaf:homepage ; sh:minCount 1 ] .
ex:Dataset a sh:NodeShape ; sh:targetClass dcat:Dataset ;
  sh:property [ sh:path dct:title ; sh:minCount 1 ] ,
              [ sh:path dct:description ; sh:minCount 1 ] ,
              [ sh:path dct:publisher ; sh:minCount 1 ; sh:class foaf:Agent ] ,
              [ sh:path dct:identifier ; sh:minCount 1 ] ,
              [ sh:path dct:language ; sh:minCount 1 ; sh:nodeKind sh:IRI ] ,
              [ sh:path dct:accessRights ; sh:minCount 1 ; sh:nodeKind sh:IRI ] ,
              [ sh:path dcat:distribution ; sh:minCount 1 ] .
ex:Distribution a sh:NodeShape ; sh:targetClass dcat:Distribution ;
  sh:property [ sh:path dcat:accessURL ; sh:minCount 1 ; sh:nodeKind sh:IRI ] ,
              [ sh:path dct:format ; sh:minCount 1 ; sh:nodeKind sh:IRI ] ,
              [ sh:path dcat:mediaType ; sh:minCount 1 ; sh:nodeKind sh:IRI ] ,
              [ sh:path dct:license ; sh:minCount 1 ; sh:nodeKind sh:IRI ] .
ex:Agent a sh:NodeShape ; sh:targetClass foaf:Agent ;
  sh:property [ sh:path foaf:name ; sh:minCount 1 ] .
ex:DataService a sh:NodeShape ; sh:targetClass dcat:DataService ;
  sh:property [ sh:path dct:title ; sh:minCount 1 ] ,
              [ sh:path dcat:endpointURL ; sh:minCount 1 ; sh:nodeKind sh:IRI ] .
"#;

async fn fetch(
    app: &Router,
    uri: &str,
    accept: &str,
    token: Option<&str>,
) -> (StatusCode, String, String) {
    let mut b = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::ACCEPT, accept);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    (st, ct, body_text(resp.into_body()).await)
}

async fn send_json(
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
    let st = resp.status();
    let text = body_text(resp.into_body()).await;
    (st, serde_json::from_str(&text).unwrap_or(Value::Null), text)
}

fn parse(text: &str, fmt: RdfFormat) -> TripleStore {
    let s = TripleStore::in_memory().unwrap();
    s.load_str(text, fmt, Some("urn:cat"))
        .unwrap_or_else(|e| panic!("catalogue is not valid {fmt:?}: {e}\n{text}"));
    s
}
fn ask(s: &TripleStore, q: &str) -> bool {
    matches!(
        s.query(&format!("ASK {{ GRAPH <urn:cat> {{ {q} }} }}")),
        Ok(QueryResults::Boolean(true))
    )
}

#[tokio::test]
async fn dcat_ap_nl_catalogue_validates_negotiates_and_survives_hostile_metadata() {
    std::env::set_var("DCAT_PROFILE", "dcat-ap-nl");
    std::env::set_var("CATALOG_PUBLISHER_NAME", "Gemeente Voorbeeld");
    std::env::set_var("CATALOG_PUBLISHER_IDENTIFIER", "00000001234567890000");
    std::env::set_var(
        "CATALOG_LICENSE",
        "http://creativecommons.org/publicdomain/zero/1.0/",
    );
    let (state, token) = admin_state();
    state
        .auth_db
        .create_organisation(
            "o1",
            "Waterschap Voorbeeld",
            "wsv",
            Some("Beheert de dijken."),
            None,
        )
        .unwrap();
    state
        .auth_db
        .create_dataset(
            "assets",
            "Kunstwerken \"2026\" .\n<urn:x> <urn:y> <urn:z> .",
            Some("Bruggen > sluizen ; dcat:theme <urn:evil>"),
            OwnerType::Organisation,
            "o1",
            Visibility::Public,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("assets", "https://example.org/assets/instances")
        .unwrap();
    state
        .auth_db
        .update_dataset_metadata("assets", None, Some("[\"not an iri\", \"http://publications.europa.eu/resource/authority/data-theme/TRAN\"]"), Some("[\"bruggen\"]"), Some("Beheer"), Some("beheer@example.org"), None, Some("completed"), None, None, None)
        .unwrap();
    // Data in a *named* graph only — the old aggregate statistics missed it.
    state
        .store
        .load_str(
            "<urn:b1> a <urn:Bridge> ; <urn:name> \"Waalbrug\" . <urn:b2> a <urn:Bridge> .",
            RdfFormat::Turtle,
            Some("https://example.org/assets/instances"),
        )
        .unwrap();
    // A user-owned dataset too (its publisher must still be a named agent), with an LDES stream.
    state
        .auth_db
        .create_dataset(
            "mine",
            "Personal notes",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("mine", "https://example.org/mine/g")
        .unwrap();
    open_triplestore::ldes::store::set_stream(&state.auth_db, "mine", true, 100).unwrap();
    let app = test_app(state.clone());

    // 1. Turtle, parsed: hostile metadata is inert; the bad theme is dropped, the good one kept.
    let (st, ct, ttl) = fetch(&app, "/.well-known/void", "text/turtle", None).await;
    assert_eq!(st, StatusCode::OK, "{ttl}");
    assert!(ct.starts_with("text/turtle"), "{ct}");
    let cat = parse(&ttl, RdfFormat::Turtle);
    assert!(
        !ask(&cat, "<urn:x> <urn:y> <urn:z>"),
        "title text must not become triples:\n{ttl}"
    );
    assert!(
        !ask(&cat, "?d <http://www.w3.org/ns/dcat#theme> <urn:evil>"),
        "{ttl}"
    );
    assert!(ask(&cat, "<http://localhost:7878/dataset/assets> <http://www.w3.org/ns/dcat#theme> <http://publications.europa.eu/resource/authority/data-theme/TRAN>"), "{ttl}");
    assert!(ask(&cat, "<http://localhost:7878/dataset/assets> <http://purl.org/dc/terms/title> \"Kunstwerken \\\"2026\\\" .\\n<urn:x> <urn:y> <urn:z> .\"@nl"), "the title is a Dutch-tagged literal, verbatim:\n{ttl}");

    // 2. The profile's mandatory properties, checked with SHACL over the served document.
    assert!(ask(&cat, "<http://localhost:7878/catalog> <http://purl.org/dc/terms/language> <http://publications.europa.eu/resource/authority/language/NLD>"), "{ttl}");
    assert!(ask(&cat, "<http://localhost:7878/publisher> a <http://xmlns.com/foaf/0.1/Agent> ; <http://xmlns.com/foaf/0.1/name> \"Gemeente Voorbeeld\" ; <http://purl.org/dc/terms/identifier> \"00000001234567890000\""), "{ttl}");
    assert!(ask(&cat, "<http://localhost:7878/dataset/assets> <http://www.w3.org/ns/adms#status> <http://publications.europa.eu/resource/authority/dataset-status/COMPLETED>"), "{ttl}");
    assert!(ask(&cat, "<http://localhost:7878/dataset/mine> <http://purl.org/dc/terms/publisher> <http://localhost:7878/user/adm> . <http://localhost:7878/user/adm> a <http://xmlns.com/foaf/0.1/Agent> ; <http://xmlns.com/foaf/0.1/name> ?n"), "a user-owned dataset still has a named publisher agent:\n{ttl}");
    assert!(ask(&cat, "<http://localhost:7878/dataset/mine> <http://www.w3.org/ns/dcat#distribution> ?d . ?d <http://purl.org/dc/terms/conformsTo> <https://w3id.org/ldes/specification> ; <http://www.w3.org/ns/dcat#accessURL> <http://localhost:7878/api/datasets/mine/ldes>"), "the LDES stream is a distribution:\n{ttl}");
    assert!(ask(&cat, "<http://localhost:7878/sparql> a <http://www.w3.org/ns/dcat#DataService> ; <http://www.w3.org/ns/dcat#endpointURL> <http://localhost:7878/sparql>"), "{ttl}");
    // Load the catalogue as data, the shapes as a Studio shape graph, run a pipeline.
    state
        .store
        .load_str(&ttl, RdfFormat::Turtle, Some("urn:cat:data"))
        .unwrap();
    let (st, sg, txt) = send_json(
        &app,
        Method::POST,
        "/api/shacl/shape-graphs",
        &token,
        json!({ "name": "dcat-ap-nl", "visibility": "private", "turtle": AP_NL_SHAPES }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let sg = sg["id"].as_str().unwrap().to_string();
    let (st, pl, txt) = send_json(&app, Method::POST, "/api/shacl/pipelines", &token, json!({ "name": "ap-nl-check", "targets": [{ "kind": "graph", "id": "urn:cat:data" }], "shape_graph_ids": [sg] })).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let pid = pl["id"].as_str().unwrap().to_string();
    let (st, run, txt) = send_json(
        &app,
        Method::POST,
        &format!("/api/shacl/pipelines/{pid}/run"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(
        run["conforms"], true,
        "the catalogue satisfies the DCAT-AP-NL mandatory properties: {txt}"
    );

    // 3. VoID statistics see the named graph.
    assert!(ask(&cat, "<http://localhost:7878/dataset> <http://rdfs.org/ns/void#distinctSubjects> ?n . FILTER(?n >= 2)"), "{ttl}");
    assert!(
        ask(
            &cat,
            "<http://localhost:7878/dataset/assets> <http://rdfs.org/ns/void#triples> 3"
        ),
        "{ttl}"
    );

    // 4. JSON-LD and RDF/XML by Accept, both parseable and equivalent on a key fact.
    let (st, ct, jsonld) = fetch(&app, "/.well-known/void", "application/ld+json", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ct.contains("ld+json"), "{ct}");
    let j = parse(
        &jsonld,
        RdfFormat::from_media_type("application/ld+json").unwrap(),
    );
    assert!(
        ask(
            &j,
            "<http://localhost:7878/catalog> a <http://www.w3.org/ns/dcat#Catalog>"
        ),
        "{jsonld}"
    );
    let (st, ct, xml) = fetch(&app, "/.well-known/void?format=rdfxml", "*/*", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ct.contains("rdf+xml"), "{ct}");
    let x = parse(&xml, RdfFormat::RdfXml);
    assert!(
        ask(
            &x,
            "<http://localhost:7878/dataset/assets> a <http://www.w3.org/ns/dcat#Dataset>"
        ),
        "{xml}"
    );

    // 5. The organisation's own catalogue is published by the organisation agent.
    let (st, _, org_ttl) = fetch(&app, "/wsv/.well-known/void", "text/turtle", None).await;
    assert_eq!(st, StatusCode::OK, "{org_ttl}");
    let o = parse(&org_ttl, RdfFormat::Turtle);
    assert!(ask(&o, "<http://localhost:7878/wsv/catalog> a <http://www.w3.org/ns/dcat#Catalog> ; <http://purl.org/dc/terms/publisher> <http://localhost:7878/org/o1> . <http://localhost:7878/org/o1> a <http://xmlns.com/foaf/0.1/Agent> ; <http://xmlns.com/foaf/0.1/name> \"Waterschap Voorbeeld\""), "{org_ttl}");
    assert!(
        !ask(
            &o,
            "<http://localhost:7878/dataset/mine> a <http://www.w3.org/ns/dcat#Dataset>"
        ),
        "the user's dataset is not in the organisation's catalogue"
    );
}
