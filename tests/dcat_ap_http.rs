//! DCAT-AP-NL catalogue (7.2): under `DCAT_PROFILE=dcat-ap-nl` the served
//! catalogue satisfies SEMIC's published DCAT-AP 3.0.1 shapes (validated
//! through a SHACL Studio pipeline over the served document; the Geonovum
//! DCAT-AP-NL 3 shapes are checked in `tests/dcat_conformance.rs`), is
//! negotiable in JSON-LD and RDF/XML, advertises LDES streams, counts
//! named-graph data in its VoID statistics, and cannot be corrupted by hostile
//! metadata.
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

/// SEMIC's published DCAT-AP 3.0.1 shapes (mandatory properties,
/// cardinalities, node kinds and ranges), vendored unmodified in
/// `tests/fixtures/semic-dcat-ap-3.0.1/` (CC BY 4.0, see its LICENSE.md).
fn dcat_ap_shapes() -> String {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/semic-dcat-ap-3.0.1"
    );
    let g = "urn:test:shapes";
    let s = TripleStore::in_memory().unwrap();
    for f in ["dcat-ap-SHACL.ttl", "ranges.ttl"] {
        let ttl = std::fs::read_to_string(format!("{dir}/{f}")).unwrap();
        s.load_str(&ttl, RdfFormat::Turtle, Some(g)).unwrap();
    }
    // The two files link five property shapes neither defines (no `sh:path`):
    // an ill-formed shapes graph the engine refuses whole. Drop those links,
    // which constrain nothing (see tests/fixtures/semic-dcat-ap-3.0.1/PROVENANCE.md).
    s.update(&format!(
        "DELETE {{ GRAPH <{g}> {{ ?s <http://www.w3.org/ns/shacl#property> ?ps }} }} \
         WHERE {{ GRAPH <{g}> {{ ?s <http://www.w3.org/ns/shacl#property> ?ps \
         FILTER NOT EXISTS {{ ?ps <http://www.w3.org/ns/shacl#path> ?p }} }} }}"
    ))
    .unwrap();
    String::from_utf8(s.graph_store_get(Some(g), RdfFormat::Turtle).unwrap()).unwrap()
}

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
            "<urn:b1> a <urn:Bridge> ; <urn:name> \"Example Bridge\" . <urn:b2> a <urn:Bridge> .",
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
        json!({ "name": "dcat-ap-3.0.1", "visibility": "private", "turtle": dcat_ap_shapes() }),
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
        "the served catalogue satisfies the DCAT-AP 3.0.1 shapes: {txt}"
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

/// The catalogue's VoID statistics count what the caller may read, and
/// nothing else: a private dataset's graphs, a public dataset's private graph
/// and the store's own system graphs stay out of an anonymous caller's
/// totals, as they do in the service description, and a private graph is not
/// listed as a public dataset's subset. A caller who may read more sees more;
/// an administrator sees the whole store.
#[tokio::test]
async fn the_aggregate_statistics_count_only_what_the_caller_may_read() {
    let (state, admin) = admin_state();
    state
        .auth_db
        .create_user(
            "insider",
            "insider",
            "insider@test.com",
            "hash",
            open_triplestore::auth::models::SystemRole::User,
        )
        .unwrap();
    let insider = mint_token("insider", "insider", "user");
    for (id, visibility, graph, data) in [
        (
            "open",
            Visibility::Public,
            "https://example.org/open/g",
            "<urn:a> <urn:p> 1 . <urn:b> <urn:p> 2 .",
        ),
        (
            "closed",
            Visibility::Private,
            "https://example.org/closed/g",
            "<urn:c> <urn:q> 3 . <urn:d> <urn:q> 4 . <urn:e> <urn:q> 5 .",
        ),
    ] {
        state
            .auth_db
            .create_dataset(id, id, None, OwnerType::User, "insider", visibility, None)
            .unwrap();
        state.auth_db.add_dataset_graph(id, graph).unwrap();
        state
            .store
            .load_str(data, RdfFormat::Turtle, Some(graph))
            .unwrap();
    }
    // A private graph inside the public dataset: its writers' only.
    let hidden = "https://example.org/open/private";
    state.auth_db.add_dataset_graph("open", hidden).unwrap();
    state
        .auth_db
        .set_dataset_graph_private("open", hidden, true)
        .unwrap();
    state
        .store
        .load_str(
            "<urn:f> <urn:r> 6 . <urn:g> <urn:r> 7 . <urn:h> <urn:r> 8 . <urn:i> <urn:r> 9 .",
            RdfFormat::Turtle,
            Some(hidden),
        )
        .unwrap();
    let app = test_app(state);
    let stat_of = |cat: &TripleStore, dataset: &str, property: &str| -> u64 {
        let q = format!(
            "SELECT ?n WHERE {{ GRAPH <urn:cat> {{ <http://localhost:7878/{dataset}> <http://rdfs.org/ns/void#{property}> ?n }} }}"
        );
        match cat.query(&q).unwrap() {
            QueryResults::Solutions(mut rows) => match rows.next().unwrap().unwrap().get("n") {
                Some(oxigraph::model::Term::Literal(l)) => l.value().parse().unwrap(),
                other => panic!("{dataset} {property}: {other:?}"),
            },
            _ => unreachable!(),
        }
    };
    let stat = |cat: &TripleStore, property: &str| stat_of(cat, "dataset", property);

    let (st, _, ttl) = fetch(&app, "/.well-known/void", "text/turtle", None).await;
    assert_eq!(st, StatusCode::OK, "{ttl}");
    let cat = parse(&ttl, RdfFormat::Turtle);
    assert_eq!(stat(&cat, "triples"), 2, "{ttl}");
    assert_eq!(stat(&cat, "distinctSubjects"), 2, "{ttl}");
    assert_eq!(stat(&cat, "properties"), 1, "{ttl}");
    assert_eq!(stat(&cat, "documents"), 1, "{ttl}");
    // The public dataset's own entry leaves its private graph out too.
    assert_eq!(stat_of(&cat, "dataset/open", "triples"), 2, "{ttl}");
    assert!(
        !ttl.contains(hidden),
        "a private graph is not listed:\n{ttl}"
    );

    // The datasets' owner writes both and reads every graph of them.
    let (_, _, ttl) = fetch(&app, "/.well-known/void", "text/turtle", Some(&insider)).await;
    let cat = parse(&ttl, RdfFormat::Turtle);
    assert_eq!(stat(&cat, "triples"), 9, "{ttl}");
    assert_eq!(stat(&cat, "distinctSubjects"), 9, "{ttl}");
    assert_eq!(stat(&cat, "properties"), 3, "{ttl}");
    assert_eq!(stat(&cat, "documents"), 3, "{ttl}");
    assert_eq!(stat_of(&cat, "dataset/open", "triples"), 6, "{ttl}");
    assert!(ttl.contains(hidden), "{ttl}");

    let (_, _, ttl) = fetch(&app, "/.well-known/void", "text/turtle", Some(&admin)).await;
    let cat = parse(&ttl, RdfFormat::Turtle);
    assert!(stat(&cat, "triples") >= 9, "{ttl}");
    assert!(stat(&cat, "distinctSubjects") >= 9, "{ttl}");
}

/// SPARQL 1.1 Service Description §2: `GET /sparql` without a query returns
/// the service description — the document the catalogue names as the
/// endpoint's `dcat:endpointDescription`. A browser still gets the web UI.
#[tokio::test]
async fn the_sparql_endpoint_describes_itself() {
    let (state, _) = admin_state();
    let app = test_app(state);
    let (st, ct, body) = fetch(&app, "/sparql", "text/turtle", None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(ct.starts_with("text/turtle"), "{ct}");
    // The description's IRIs are relative to the request (`<>`, `<sparql>`).
    let sd = TripleStore::in_memory().unwrap();
    sd.load_str_with_base(
        &body,
        RdfFormat::Turtle,
        "http://localhost:7878/sparql",
        None,
    )
    .unwrap_or_else(|e| panic!("{e}\n{body}"));
    assert!(
        matches!(
            sd.query("ASK { ?s a <http://www.w3.org/ns/sparql-service-description#Service> ; <http://www.w3.org/ns/sparql-service-description#endpoint> ?e }"),
            Ok(QueryResults::Boolean(true))
        ),
        "{body}"
    );
    let (_, _, cat) = fetch(&app, "/.well-known/void", "text/turtle", None).await;
    let cat = parse(&cat, RdfFormat::Turtle);
    assert!(ask(&cat, "<http://localhost:7878/sparql> <http://www.w3.org/ns/dcat#endpointDescription> <http://localhost:7878/sparql>"));
}

/// The catalogue is scoped to its caller, so a signed-in caller's copy is
/// `Cache-Control: private` — a shared cache must not serve it to anyone else —
/// while the anonymous one stays `public`.
#[tokio::test]
async fn a_signed_in_callers_catalogue_is_not_shared_cacheable() {
    let (state, token) = admin_state();
    let app = test_app(state);
    for (auth, expected) in [(None, "public"), (Some(token.as_str()), "private")] {
        let mut b = Request::builder()
            .method(Method::GET)
            .uri("/.well-known/void")
            .header(header::ACCEPT, "text/turtle");
        if let Some(t) = auth {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let resp = app
            .clone()
            .oneshot(b.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cc = resp.headers()[header::CACHE_CONTROL].to_str().unwrap();
        assert!(cc.starts_with(expected), "{auth:?}: {cc}");
        let vary = resp.headers()[header::VARY].to_str().unwrap();
        assert!(vary.contains("Authorization"), "{vary}");
    }
}
