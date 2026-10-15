//! LDES publishing and client sync (6.1), over the real HTTP API.
//!
//! Publishing: enabling a stream seeds a member per entity; every write path
//! (Graph Store PUT/POST/DELETE, SPARQL Update) yields members for exactly the
//! entities it changed, deletions become tombstones, and fragments chain with
//! `tree:GreaterThanOrEqualToRelation`. Sync: a second instance of this server
//! on a local listener publishes a dataset; the local instance mirrors it,
//! then mirrors the increments (an update and a deletion). Scheduled sync: a
//! job with a schedule runs on its own, waits its interval (with a floor),
//! backs off after failures and pauses when its owner may no longer write.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use std::sync::Arc;

use axum::Router;
use chrono::{DateTime, Utc};
use common::*;
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const G: &str = "https://example.org/ldes-test/instances";
const EX: &str = "https://example.org/ldes-test/";

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    ct: Option<&str>,
    accept: Option<&str>,
    body: &str,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(c) = ct {
        b = b.header(header::CONTENT_TYPE, c);
    }
    if let Some(a) = accept {
        b = b.header(header::ACCEPT, a);
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    let hdrs = resp.headers().clone();
    (st, hdrs, body_text(resp.into_body()).await)
}

fn setup(state: &AppState, dataset: &str, visibility: Visibility) {
    state
        .auth_db
        .create_dataset(
            dataset,
            "LDES test",
            None,
            OwnerType::User,
            "adm",
            visibility,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph(dataset, G).unwrap();
    state
        .store
        .load_str(
            &format!("<{EX}b1> a <{EX}Bridge> ; <{EX}name> \"one\" . <{EX}b2> a <{EX}Bridge> ; <{EX}name> \"two\" ."),
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
}

/// Members `(entity, tombstone?)` listed on a node, from its Turtle, in
/// member order (the member IRI ends in the member id).
fn members(turtle: &str) -> Vec<(String, bool)> {
    let tmp = open_triplestore::store::TripleStore::in_memory().unwrap();
    tmp.load_str(turtle, RdfFormat::Turtle, Some("urn:n"))
        .expect("fragment is valid Turtle");
    let q = "SELECT ?m ?e ?tomb WHERE { GRAPH <urn:n> { ?c <https://w3id.org/tree#member> ?m . ?m <http://purl.org/dc/terms/isVersionOf> ?e . OPTIONAL { ?m a ?tomb . FILTER(?tomb = <https://opentriplestore.org/ns#Tombstone>) } } }";
    let mut rows: Vec<(i64, String, bool)> = match tmp.query(q) {
        Ok(QueryResults::Solutions(s)) => s
            .flatten()
            .map(|r| {
                let m = r.get("m").unwrap().to_string();
                let id: i64 = m
                    .trim_end_matches('>')
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap();
                (
                    id,
                    r.get("e")
                        .unwrap()
                        .to_string()
                        .trim_matches(['<', '>'])
                        .to_string(),
                    r.get("tomb").is_some(),
                )
            })
            .collect(),
        _ => vec![],
    };
    rows.sort();
    rows.into_iter().map(|(_, e, t)| (e, t)).collect()
}

#[tokio::test]
async fn publishing_captures_every_write_path_and_fragments_the_stream() {
    let (state, token) = admin_state();
    setup(&state, "pub", Visibility::Public);
    let app = test_app(state.clone());

    // Not enabled: 404.
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/datasets/pub/ldes",
        None,
        None,
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Enable with tiny pages; the two existing entities are seeded.
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/pub/ldes",
        Some(&token),
        Some("application/json"),
        None,
        &json!({ "enabled": true, "page_size": 2 }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let v: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(v["members_seeded"], 2, "{txt}");

    let (st, _, ttl) = req(
        &app,
        Method::GET,
        "/api/datasets/pub/ldes",
        None,
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{ttl}");
    assert!(
        ttl.contains("ldes:EventStream")
            && ttl.contains("tree:view")
            && ttl.contains("ldes:timestampPath"),
        "{ttl}"
    );
    let (st, hdrs, n1) = req(
        &app,
        Method::GET,
        "/api/datasets/pub/ldes/nodes/1",
        None,
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{n1}");
    let m1 = members(&n1);
    assert_eq!(m1.len(), 2, "{n1}");
    assert!(
        n1.contains("dct:isVersionOf") && n1.contains("\"one\""),
        "a member is a version object carrying the entity's properties: {n1}"
    );
    assert!(
        !n1.contains("tree:relation"),
        "a single page has no relation yet: {n1}"
    );
    assert!(
        hdrs.get("cache-control")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("no-cache"),
        "the last page is mutable"
    );

    // POST adds an entity → exactly one new member.
    let (st, _, _) = req(
        &app,
        Method::POST,
        &format!("/store?graph={}", url_encode(G)),
        Some(&token),
        Some("text/turtle"),
        None,
        &format!("<{EX}b3> a <{EX}Bridge> ; <{EX}name> \"three\" ."),
    )
    .await;
    assert!(st.is_success());
    // PUT changes one entity and keeps the others → one member for b1 only.
    let (st, _, _) = req(&app, Method::PUT, &format!("/store?graph={}", url_encode(G)), Some(&token), Some("text/turtle"), None, &format!("<{EX}b1> a <{EX}Bridge> ; <{EX}name> \"uno\" . <{EX}b2> a <{EX}Bridge> ; <{EX}name> \"two\" . <{EX}b3> a <{EX}Bridge> ; <{EX}name> \"three\" .")).await;
    assert!(st.is_success());
    // SPARQL Update touches b2 → one member.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/sparql",
        Some(&token),
        Some("application/sparql-update"),
        None,
        &format!("INSERT DATA {{ GRAPH <{G}> {{ <{EX}b2> <{EX}name> \"deux\" }} }}"),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");

    // 2 seeded + 1 (POST b3) + 1 (PUT b1) + 1 (UPDATE b2) = 5 members over 3 pages of 2.
    let (_, hdrs1, n1) = req(
        &app,
        Method::GET,
        "/api/datasets/pub/ldes/nodes/1",
        None,
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    assert!(
        !n1.contains("tree:relation"),
        "a sealed page links nowhere; the root links every page: {n1}"
    );
    let (_, _, root) = req(
        &app,
        Method::GET,
        "/api/datasets/pub/ldes/nodes/0",
        None,
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    assert!(
        root.contains("tree:GreaterThanOrEqualToRelation")
            && root.contains("tree:LessThanOrEqualToRelation")
            && root.contains("ldes/nodes/1>")
            && root.contains("ldes/nodes/2>"),
        "the root bounds page 1 and links on: {root}"
    );
    assert!(
        hdrs1
            .get("cache-control")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("immutable"),
        "a full page is immutable"
    );
    let (_, _, n2) = req(
        &app,
        Method::GET,
        "/api/datasets/pub/ldes/nodes/2",
        None,
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    let (st3, _, n3) = req(
        &app,
        Method::GET,
        "/api/datasets/pub/ldes/nodes/3",
        None,
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    assert_eq!(st3, StatusCode::OK);
    let all: Vec<(String, bool)> = [members(&n1), members(&n2), members(&n3)].concat();
    assert_eq!(all.len(), 5, "{n1}\n{n2}\n{n3}");
    let changed: Vec<&str> = all[2..].iter().map(|(e, _)| e.as_str()).collect();
    assert_eq!(
        changed,
        vec![format!("{EX}b3"), format!("{EX}b1"), format!("{EX}b2")],
        "one member per changed entity, in write order"
    );
    assert!(
        n3.contains("\"deux\""),
        "the newest member carries the updated value: {n3}"
    );
    let (st4, _, _) = req(
        &app,
        Method::GET,
        "/api/datasets/pub/ldes/nodes/4",
        None,
        None,
        None,
        "",
    )
    .await;
    assert_eq!(st4, StatusCode::NOT_FOUND);

    // DELETE the graph → three tombstones.
    let (st, _, _) = req(
        &app,
        Method::DELETE,
        &format!("/store?graph={}", url_encode(G)),
        Some(&token),
        None,
        None,
        "",
    )
    .await;
    assert!(st.is_success());
    // Page 3 gained a member since it was last fetched; page 4 is new.
    let (_, _, n3) = req(
        &app,
        Method::GET,
        "/api/datasets/pub/ldes/nodes/3",
        None,
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    let (_, _, n4) = req(
        &app,
        Method::GET,
        "/api/datasets/pub/ldes/nodes/4",
        None,
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    let tombs: Vec<(String, bool)> = [members(&n3), members(&n4)]
        .concat()
        .into_iter()
        .filter(|(_, t)| *t)
        .collect();
    assert_eq!(
        tombs.len(),
        3,
        "every entity of the deleted graph is tombstoned:\n{n3}\n{n4}"
    );

    // JSON-LD is negotiable too.
    let (st, hdrs, body) = req(
        &app,
        Method::GET,
        "/api/datasets/pub/ldes",
        None,
        None,
        Some("application/ld+json"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(hdrs
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("ld+json"));
    assert!(serde_json::from_str::<Value>(&body).is_ok(), "{body}");
}

/// The settings form reads the stream's configuration as JSON: off with the
/// defaults before it is enabled, then what was saved, with the member and
/// sealed-page counts; `can_write` follows the caller.
#[tokio::test]
async fn stream_settings_read_back_as_json() {
    let (state, token) = admin_state();
    setup(&state, "cfg", Visibility::Public);
    let app = test_app(state.clone());
    let (st, _, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/cfg/ldes/config",
        Some(&token),
        None,
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let v: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(v["enabled"], false, "{txt}");
    assert_eq!(v["configured"], false, "{txt}");
    assert_eq!(v["page_size"], 100, "{txt}");
    assert_eq!(v["polling_interval"], 60, "{txt}");
    assert!(v["retention"].is_null(), "{txt}");
    assert_eq!(v["can_write"], true, "{txt}");

    let (st, _, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/cfg/ldes",
        Some(&token),
        Some("application/json"),
        None,
        &json!({ "enabled": true, "page_size": 1, "polling_interval": 30, "retention": { "version_amount": 2, "full_log_duration": "P30D" } }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let (_, _, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/cfg/ldes/config",
        None,
        None,
        None,
        "",
    )
    .await;
    let v: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(v["enabled"], true, "{txt}");
    assert_eq!(v["page_size"], 1, "{txt}");
    assert_eq!(v["polling_interval"], 30, "{txt}");
    assert_eq!(v["retention"]["version_amount"], 2, "{txt}");
    assert_eq!(v["retention"]["full_log_duration"], "P30D", "{txt}");
    assert_eq!(v["members"], 2, "{txt}");
    assert_eq!(
        v["sealed_nodes"], 1,
        "the first one-member page is sealed; the last stays the mutable tail: {txt}"
    );
    assert_eq!(
        v["can_write"], false,
        "an anonymous reader cannot change it: {txt}"
    );
    assert!(
        v["stream"]
            .as_str()
            .unwrap()
            .ends_with("/api/datasets/cfg/ldes"),
        "{txt}"
    );

    // A private dataset's settings are as invisible as its stream.
    setup(&state, "cfg-priv", Visibility::Private);
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/datasets/cfg-priv/ldes/config",
        None,
        None,
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_private_stream_is_invisible_to_strangers() {
    let (state, token) = admin_state();
    setup(&state, "priv", Visibility::Private);
    let app = test_app(state);
    req(
        &app,
        Method::PUT,
        "/api/datasets/priv/ldes",
        Some(&token),
        Some("application/json"),
        None,
        &json!({ "enabled": true }).to_string(),
    )
    .await;
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/datasets/priv/ldes",
        None,
        None,
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/datasets/priv/ldes",
        Some(&token),
        None,
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn private_graphs_are_never_published_to_the_stream() {
    let (state, token) = admin_state();
    setup(&state, "pv", Visibility::Public);
    let secret = "https://example.org/ldes-test/secret";
    state.auth_db.add_dataset_graph("pv", secret).unwrap();
    state
        .auth_db
        .set_dataset_graph_private("pv", secret, true)
        .unwrap();
    state
        .store
        .load_str(
            &format!("<{EX}s1> a <{EX}Bridge> ; <{EX}name> \"hidden\" ."),
            RdfFormat::Turtle,
            Some(secret),
        )
        .unwrap();
    let app = test_app(state.clone());

    // Seeding skips the private graph.
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/pv/ldes",
        Some(&token),
        Some("application/json"),
        None,
        &json!({ "enabled": true }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let v: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(v["members_seeded"], 2, "only the public graph seeds: {txt}");

    // A write to the private graph yields no member; one to the public graph does.
    let (st, _, _) = req(
        &app,
        Method::PUT,
        &format!("/store?graph={}", url_encode(secret)),
        Some(&token),
        Some("text/turtle"),
        None,
        &format!(
            "<{EX}s1> a <{EX}Bridge> ; <{EX}name> \"still hidden\" . <{EX}s2> a <{EX}Bridge> ."
        ),
    )
    .await;
    assert!(st.is_success(), "{st}");
    let (st, _, _) = req(
        &app,
        Method::POST,
        &format!("/store?graph={}", url_encode(G)),
        Some(&token),
        Some("text/turtle"),
        None,
        &format!("<{EX}b3> a <{EX}Bridge> ."),
    )
    .await;
    assert!(st.is_success(), "{st}");

    let (st, _, n1) = req(
        &app,
        Method::GET,
        "/api/datasets/pv/ldes/nodes/1",
        None,
        None,
        Some("text/turtle"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{n1}");
    let entities: Vec<String> = members(&n1).into_iter().map(|(e, _)| e).collect();
    assert_eq!(
        entities,
        vec![format!("{EX}b1"), format!("{EX}b2"), format!("{EX}b3")],
        "{n1}"
    );
    assert!(
        !n1.contains("hidden") && !n1.contains(secret),
        "private-graph entities never reach the stream: {n1}"
    );
}

/// `OTS_LDES_ALLOWLIST` is one variable for the whole process, and a sync
/// test points it at its own publisher (and the first one clears it at the
/// end): the tests that sync run one at a time.
fn net() -> &'static tokio::sync::Mutex<()> {
    static NET: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    NET.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// A second instance on a local listener publishes; this instance syncs.
/// The publisher's base URL is the listener's origin, as a deployment's
/// OTS_BASE_URL is its public origin — the stream's node IRIs are built from it.
fn publisher() -> (AppState, String, String) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (mut state, token) = admin_state();
    state.base_url = std::sync::Arc::new(origin.clone());
    setup(&state, "src", Visibility::Public);
    open_triplestore::ldes::store::set_stream(&state.auth_db, "src", true, 2).unwrap();
    open_triplestore::ldes::capture::publish_all(&state, "src", &[G.to_string()]);
    let app = test_app(state.clone());
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    (state, token, origin)
}

#[tokio::test]
async fn sync_mirrors_a_remote_stream_and_its_increments() {
    let _net = net().lock().await;
    let (remote_state, remote_token, origin) = publisher();
    std::env::set_var("OTS_LDES_ALLOWLIST", format!("{origin}/"));
    let remote_app = test_app(remote_state.clone());

    let (local, token) = admin_state();
    local
        .auth_db
        .create_dataset(
            "mirror",
            "Mirror",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    let app = test_app(local.clone());
    let target = "https://example.org/mirror/instances";
    let ask = |q: &str| matches!(local.store.query(q), Ok(QueryResults::Boolean(true)));

    let stream_url = format!("{origin}/api/datasets/src/ldes");
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/ldes/sync",
        Some(&token),
        Some("application/json"),
        None,
        &json!({ "url": stream_url, "dataset_id": "mirror", "graph_iri": target }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(r["entities_updated"], 2, "{txt}");
    assert!(
        ask(&format!(
            "ASK {{ GRAPH <{target}> {{ <{EX}b1> <{EX}name> \"one\" }} }}"
        )),
        "b1 mirrored"
    );
    assert!(
        ask(&format!(
            "ASK {{ GRAPH <{target}> {{ <{EX}b2> a <{EX}Bridge> }} }}"
        )),
        "b2 mirrored"
    );

    // The remote changes b1 and deletes b2; a second sync applies just that.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let (st, _, _) = req(
        &remote_app,
        Method::PUT,
        &format!("/store?graph={}", url_encode(G)),
        Some(&remote_token),
        Some("text/turtle"),
        None,
        &format!("<{EX}b1> a <{EX}Bridge> ; <{EX}name> \"uno\" ."),
    )
    .await;
    assert!(st.is_success());
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/ldes/sync",
        Some(&token),
        Some("application/json"),
        None,
        &json!({ "url": stream_url, "dataset_id": "mirror", "graph_iri": target }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(
        r["members_skipped_older"], 2,
        "the first two members are behind the bookmark: {txt}"
    );
    assert_eq!(r["entities_updated"], 1, "{txt}");
    assert_eq!(r["entities_deleted"], 1, "{txt}");
    assert!(
        ask(&format!(
            "ASK {{ GRAPH <{target}> {{ <{EX}b1> <{EX}name> \"uno\" }} }}"
        )),
        "b1 updated"
    );
    assert!(
        !ask(&format!(
            "ASK {{ GRAPH <{target}> {{ <{EX}b1> <{EX}name> \"one\" }} }}"
        )),
        "the old value is gone"
    );
    assert!(
        !ask(&format!("ASK {{ GRAPH <{target}> {{ <{EX}b2> ?p ?o }} }}")),
        "b2 tombstoned away"
    );

    // The sync is in the mirror's history, and the mirror graph was registered.
    let (_, _, commits) = req(
        &app,
        Method::GET,
        "/api/datasets/mirror/commits",
        Some(&token),
        None,
        None,
        "",
    )
    .await;
    assert!(commits.contains("LDES sync"), "{commits}");

    // The admin's list of sync jobs: the job, its graph, its bookmark and the
    // last report.
    let (st, _, txt) = req(
        &app,
        Method::GET,
        "/api/admin/ldes/syncs",
        Some(&token),
        None,
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let jobs: Value = serde_json::from_str(&txt).unwrap();
    let job = jobs
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["source_url"] == stream_url.as_str())
        .unwrap_or_else(|| panic!("the sync is listed: {txt}"));
    assert_eq!(job["dataset_id"], "mirror", "{txt}");
    assert_eq!(job["dataset_name"], "Mirror", "{txt}");
    assert_eq!(job["graph_iri"], target, "{txt}");
    assert_eq!(job["created_by"], "adm", "{txt}");
    assert_eq!(job["last_status"], "ok", "{txt}");
    assert!(job["last_error"].is_null(), "{txt}");
    assert_eq!(job["members_applied"], 4, "{txt}");
    assert_eq!(job["last_report"]["entities_deleted"], 1, "{txt}");
    assert!(job["last_timestamp"].is_string(), "{txt}");

    // A run that fails is recorded with its error; nobody had to watch it.
    let gone = format!("{origin}/api/datasets/nope/ldes");
    let (st, _, _) = req(
        &app,
        Method::POST,
        "/api/ldes/sync",
        Some(&token),
        Some("application/json"),
        None,
        &json!({ "url": gone, "dataset_id": "mirror", "graph_iri": target }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_GATEWAY);
    let (_, _, txt) = req(
        &app,
        Method::GET,
        "/api/admin/ldes/syncs",
        Some(&token),
        None,
        None,
        "",
    )
    .await;
    let jobs: Value = serde_json::from_str(&txt).unwrap();
    let failed = jobs
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["source_url"] == gone.as_str())
        .unwrap_or_else(|| panic!("the failed run is listed: {txt}"));
    assert_eq!(failed["last_status"], "error", "{txt}");
    assert!(
        failed["last_error"].as_str().is_some_and(|e| !e.is_empty()),
        "{txt}"
    );
    assert!(failed["last_report"].is_null(), "{txt}");
    // The list is the admin's: a plain user is refused.
    local
        .auth_db
        .create_user(
            "u2",
            "plain",
            "plain@test.com",
            "hash",
            open_triplestore::auth::models::SystemRole::User,
        )
        .unwrap();
    let plain = mint_token("u2", "plain", "user");
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/admin/ldes/syncs",
        Some(&plain),
        None,
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Forgetting a job drops it and its bookmark: the next sync from the
    // source starts from the beginning of the stream again.
    let forget = format!(
        "/api/admin/ldes/syncs?dataset_id=mirror&url={}",
        url_encode(&stream_url)
    );
    let (st, _, txt) = req(&app, Method::DELETE, &forget, Some(&token), None, None, "").await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{txt}");
    let (st, _, _) = req(&app, Method::DELETE, &forget, Some(&token), None, None, "").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/ldes/sync",
        Some(&token),
        Some("application/json"),
        None,
        &json!({ "url": stream_url, "dataset_id": "mirror", "graph_iri": target }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(
        r["members_skipped_older"], 0,
        "no bookmark after forgetting: {txt}"
    );

    // Not allowlisted: refused before any request.
    std::env::set_var("OTS_LDES_ALLOWLIST", "");
    let (st, _, _) = req(
        &app,
        Method::POST,
        "/api/ldes/sync",
        Some(&token),
        Some("application/json"),
        None,
        &json!({ "url": stream_url, "dataset_id": "mirror", "graph_iri": target }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

// ─── Scheduled sync (#530) ──────────────────────────────────────────────────

const TARGET: &str = "https://example.org/mirror/instances";

/// A local instance with a private dataset `mirror`, owned by `adm`, to sync
/// into.
fn mirror() -> (AppState, String) {
    let (local, token) = admin_state();
    local
        .auth_db
        .create_dataset(
            "mirror",
            "Mirror",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    (local, token)
}

async fn run_sync(app: &Router, token: &str, url: &str) -> (StatusCode, String) {
    let (st, _, txt) = req(
        app,
        Method::POST,
        "/api/ldes/sync",
        Some(token),
        Some("application/json"),
        None,
        &json!({ "url": url, "dataset_id": "mirror", "graph_iri": TARGET }).to_string(),
    )
    .await;
    (st, txt)
}

async fn set_schedule(app: &Router, token: &str, url: &str, body: Value) -> (StatusCode, String) {
    let uri = format!(
        "/api/admin/ldes/syncs/schedule?dataset_id=mirror&url={}",
        url_encode(url)
    );
    let (st, _, txt) = req(
        app,
        Method::PUT,
        &uri,
        Some(token),
        Some("application/json"),
        None,
        &body.to_string(),
    )
    .await;
    (st, txt)
}

/// The admin's view of the job that syncs `url` into `mirror`.
async fn job(app: &Router, token: &str, url: &str) -> Value {
    let (st, _, txt) = req(
        app,
        Method::GET,
        "/api/admin/ldes/syncs",
        Some(token),
        None,
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let jobs: Value = serde_json::from_str(&txt).unwrap();
    jobs.as_array()
        .unwrap()
        .iter()
        .find(|j| j["source_url"] == url && j["dataset_id"] == "mirror")
        .cloned()
        .unwrap_or_else(|| panic!("no job for {url}: {txt}"))
}

fn at(v: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(v.as_str().unwrap_or_else(|| panic!("not a time: {v}")))
        .unwrap()
        .with_timezone(&Utc)
}

fn secs(n: i64) -> chrono::Duration {
    chrono::Duration::seconds(n)
}

/// The remote renames b1 (and so drops b2). Member timestamps have a
/// resolution of a second: one is waited out first, so the change is newer
/// than the mirror's bookmark.
async fn rename_b1(remote_app: &Router, remote_token: &str, name: &str) {
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (st, _, _) = req(
        remote_app,
        Method::PUT,
        &format!("/store?graph={}", url_encode(G)),
        Some(remote_token),
        Some("text/turtle"),
        None,
        &format!("<{EX}b1> a <{EX}Bridge> ; <{EX}name> \"{name}\" ."),
    )
    .await;
    assert!(st.is_success());
}

fn has_b1(local: &AppState, name: &str) -> bool {
    matches!(
        local.store.query(&format!(
            "ASK {{ GRAPH <{TARGET}> {{ <{EX}b1> <{EX}name> \"{name}\" }} }}"
        )),
        Ok(QueryResults::Boolean(true))
    )
}

/// #530: a job with a schedule runs without anyone asking — the server's own
/// scheduler starts it once it is due. A schedule is `off` (the default),
/// `auto` (the stream's `ldes:pollingInterval`) or a fixed interval of at
/// least 60 seconds.
#[tokio::test]
async fn a_scheduled_job_runs_without_a_manual_trigger() {
    let _net = net().lock().await;
    let (remote_state, remote_token, origin) = publisher();
    std::env::set_var("OTS_LDES_ALLOWLIST", format!("{origin}/"));
    let remote_app = test_app(remote_state.clone());
    let (local, token) = mirror();
    let app = test_app(local.clone());
    let stream_url = format!("{origin}/api/datasets/src/ldes");

    // A job exists once it has run, and its schedule is off until set.
    let (st, txt) = run_sync(&app, &token, &stream_url).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let j = job(&app, &token, &stream_url).await;
    assert_eq!(j["schedule"], "off", "{j}");
    assert!(
        j["next_run_at"].is_null() && j["paused_at"].is_null(),
        "{j}"
    );
    assert_eq!(j["consecutive_failures"], 0, "{j}");

    // What a schedule may be, and who may set it.
    for bad in [
        json!({ "schedule": "hourly" }),
        json!({ "schedule": "interval" }),
        json!({ "schedule": "interval", "interval_secs": 59 }),
    ] {
        let (st, txt) = set_schedule(&app, &token, &stream_url, bad.clone()).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}: {txt}");
    }
    let (st, _) = set_schedule(
        &app,
        &token,
        &format!("{origin}/elsewhere"),
        json!({ "schedule": "auto" }),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    local
        .auth_db
        .create_user("u2", "plain", "plain@test.com", "hash", SystemRole::User)
        .unwrap();
    let plain = mint_token("u2", "plain", "user");
    let (st, _) = set_schedule(&app, &plain, &stream_url, json!({ "schedule": "auto" })).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // `auto`: the stream's polling interval (60 s, the publisher's default)
    // after the last run.
    let (st, txt) = set_schedule(&app, &token, &stream_url, json!({ "schedule": "auto" })).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let j: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(j["schedule"], "auto", "{txt}");
    assert!(j["interval_secs"].is_null(), "{txt}");
    assert_eq!(
        at(&j["next_run_at"]) - at(&j["last_run_at"]),
        secs(60),
        "{txt}"
    );

    // The remote changes and nobody runs the job: the scheduler, on a clock
    // an hour ahead, finds it due and runs it.
    rename_b1(&remote_app, &remote_token, "uno").await;
    let ahead = chrono::Duration::hours(1);
    let scheduler = open_triplestore::ldes::schedule::spawn(
        local.clone(),
        std::time::Duration::from_millis(50),
        Arc::new(move || Utc::now() + ahead),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let j = loop {
        let j = job(&app, &token, &stream_url).await;
        // Planned from the scheduler's clock: the run has finished.
        if j["next_run_at"].is_string()
            && at(&j["next_run_at"]) > Utc::now() + chrono::Duration::minutes(30)
        {
            break j;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the scheduler never ran the job: {j}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    scheduler.abort();
    assert_eq!(j["last_status"], "ok", "{j}");
    assert_eq!(j["consecutive_failures"], 0, "{j}");
    assert_eq!(j["last_report"]["entities_updated"], 1, "{j}");
    assert_eq!(j["last_report"]["entities_deleted"], 1, "{j}");
    assert!(has_b1(&local, "uno"), "b1 updated by the scheduled run");
    assert!(!has_b1(&local, "one"), "the old value is gone");
    let (_, _, commits) = req(
        &app,
        Method::GET,
        "/api/datasets/mirror/commits",
        Some(&token),
        None,
        None,
        "",
    )
    .await;
    assert!(commits.contains("Scheduled LDES sync"), "{commits}");

    // Off again: nothing is planned.
    let (st, txt) = set_schedule(&app, &token, &stream_url, json!({ "schedule": "off" })).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let j: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(j["schedule"], "off", "{txt}");
    assert!(j["next_run_at"].is_null(), "{txt}");
}

/// A schedule waits its interval after every run — a fixed one, or the
/// stream's `ldes:pollingInterval` — and never less than 60 seconds.
#[tokio::test]
async fn a_schedule_honours_its_interval_and_the_floor() {
    use open_triplestore::ldes::schedule::run_due;
    let _net = net().lock().await;
    let (remote_state, _, origin) = publisher();
    std::env::set_var("OTS_LDES_ALLOWLIST", format!("{origin}/"));
    open_triplestore::ldes::store::set_polling_interval(&remote_state.auth_db, "src", Some(600))
        .unwrap();
    let (local, token) = mirror();
    let app = test_app(local.clone());
    let stream_url = format!("{origin}/api/datasets/src/ldes");
    let (st, txt) = run_sync(&app, &token, &stream_url).await;
    assert_eq!(st, StatusCode::OK, "{txt}");

    // Every 120 s: not a second before, then 120 s after that run.
    let (st, txt) = set_schedule(
        &app,
        &token,
        &stream_url,
        json!({ "schedule": "interval", "interval_secs": 120 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let j: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(j["schedule"], "interval", "{txt}");
    assert_eq!(j["interval_secs"], 120, "{txt}");
    let due = at(&j["next_run_at"]);
    assert_eq!(due - at(&j["last_run_at"]), secs(120), "{txt}");
    assert_eq!(run_due(&local, due - secs(1)).await, 0, "not due yet");
    assert_eq!(
        job(&app, &token, &stream_url).await["last_run_at"],
        j["last_run_at"],
        "nothing ran"
    );
    let now = due + secs(1);
    assert_eq!(run_due(&local, now).await, 1);
    let j = job(&app, &token, &stream_url).await;
    assert_eq!(j["last_status"], "ok", "{j}");
    assert_eq!(at(&j["next_run_at"]), now + secs(120), "{j}");
    assert_eq!(run_due(&local, now).await, 0, "ran once, not again");

    // `auto`: the stream declares 600 s.
    let (st, txt) = set_schedule(&app, &token, &stream_url, json!({ "schedule": "auto" })).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let j: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(j["last_report"]["polling_interval"], 600, "{txt}");
    let now = at(&j["next_run_at"]);
    assert_eq!(now - at(&j["last_run_at"]), secs(600), "{txt}");
    assert_eq!(run_due(&local, now - secs(1)).await, 0, "not due yet");
    assert_eq!(run_due(&local, now).await, 1);
    let j = job(&app, &token, &stream_url).await;
    assert_eq!(at(&j["next_run_at"]), now + secs(600), "{j}");

    // A stream that asks for less than the floor is read every 60 s.
    open_triplestore::ldes::store::set_polling_interval(&remote_state.auth_db, "src", Some(5))
        .unwrap();
    let now = now + secs(600);
    assert_eq!(run_due(&local, now).await, 1);
    let j = job(&app, &token, &stream_url).await;
    assert_eq!(j["last_report"]["polling_interval"], 5, "{j}");
    assert_eq!(at(&j["next_run_at"]), now + secs(60), "{j}");
}

/// A failing job waits twice as long after each failure in a row, and its
/// first success brings it back to its interval.
#[tokio::test]
async fn a_failing_schedule_backs_off_and_recovers() {
    use open_triplestore::ldes::schedule::run_due;
    let _net = net().lock().await;
    let (remote_state, _, origin) = publisher();
    std::env::set_var("OTS_LDES_ALLOWLIST", format!("{origin}/"));
    let (local, token) = mirror();
    let app = test_app(local.clone());

    // A stream the publisher does not serve yet: the run fails, and is kept.
    let later = format!("{origin}/api/datasets/later/ldes");
    let (st, txt) = run_sync(&app, &token, &later).await;
    assert_eq!(st, StatusCode::BAD_GATEWAY, "{txt}");
    let (st, txt) = set_schedule(
        &app,
        &token,
        &later,
        json!({ "schedule": "interval", "interval_secs": 60 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let j: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(j["consecutive_failures"], 0, "{txt}");
    let mut now = at(&j["next_run_at"]);
    for (failures, wait) in [(1, 120), (2, 240), (3, 480)] {
        assert_eq!(run_due(&local, now).await, 1);
        let j = job(&app, &token, &later).await;
        assert_eq!(j["last_status"], "error", "{j}");
        assert_eq!(j["consecutive_failures"], failures, "{j}");
        let next = at(&j["next_run_at"]);
        assert_eq!(next - now, secs(wait), "{j}");
        assert_eq!(
            run_due(&local, next - secs(1)).await,
            0,
            "not before the back-off: {j}"
        );
        now = next;
    }

    // The stream appears; the next run succeeds and the wait is the interval
    // again.
    let later_graph = "https://example.org/ldes-test/later";
    remote_state
        .auth_db
        .create_dataset(
            "later",
            "Later",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    remote_state
        .auth_db
        .add_dataset_graph("later", later_graph)
        .unwrap();
    remote_state
        .store
        .load_str(
            &format!("<{EX}b9> a <{EX}Bridge> ; <{EX}name> \"nine\" ."),
            RdfFormat::Turtle,
            Some(later_graph),
        )
        .unwrap();
    open_triplestore::ldes::store::set_stream(&remote_state.auth_db, "later", true, 2).unwrap();
    open_triplestore::ldes::capture::publish_all(
        &remote_state,
        "later",
        &[later_graph.to_string()],
    );
    assert_eq!(run_due(&local, now).await, 1);
    let j = job(&app, &token, &later).await;
    assert_eq!(j["last_status"], "ok", "{j}");
    assert!(j["last_error"].is_null(), "{j}");
    assert_eq!(j["consecutive_failures"], 0, "{j}");
    assert_eq!(j["last_report"]["entities_updated"], 1, "{j}");
    assert_eq!(at(&j["next_run_at"]), now + secs(60), "{j}");
}

/// A scheduled run acts as the job's owner, checked every time as a request
/// is: an owner who can no longer write the dataset, or no longer exists,
/// pauses the job with the reason in `last_error`, and it stays paused until
/// its schedule is set again.
#[tokio::test]
async fn a_schedule_pauses_when_its_owner_loses_write_access() {
    use open_triplestore::ldes::schedule::run_due;
    let _net = net().lock().await;
    let (remote_state, remote_token, origin) = publisher();
    std::env::set_var("OTS_LDES_ALLOWLIST", format!("{origin}/"));
    let remote_app = test_app(remote_state.clone());
    let (local, token) = mirror();
    let app = test_app(local.clone());
    let stream_url = format!("{origin}/api/datasets/src/ldes");

    // Ed, an administrator, adds the job: it is ed's.
    local
        .auth_db
        .create_user("ed", "ed", "ed@test.com", "hash", SystemRole::Admin)
        .unwrap();
    let ed = mint_token("ed", "ed", "admin");
    let (st, txt) = run_sync(&app, &ed, &stream_url).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(job(&app, &token, &stream_url).await["created_by"], "ed");
    let (st, txt) = set_schedule(
        &app,
        &token,
        &stream_url,
        json!({ "schedule": "interval", "interval_secs": 60 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let due = at(&serde_json::from_str::<Value>(&txt).unwrap()["next_run_at"]);

    // Ed is an administrator no more, and `mirror` is adm's: ed may not
    // write it, so the run does not happen and the schedule pauses.
    rename_b1(&remote_app, &remote_token, "uno").await;
    local
        .auth_db
        .update_user_role("ed", SystemRole::User)
        .unwrap();
    assert_eq!(run_due(&local, due).await, 1);
    let j = job(&app, &token, &stream_url).await;
    assert!(j["paused_at"].is_string(), "{j}");
    assert!(j["next_run_at"].is_null(), "{j}");
    assert_eq!(
        j["schedule"], "interval",
        "the schedule itself is kept: {j}"
    );
    assert_eq!(j["last_status"], "error", "{j}");
    let why = j["last_error"].as_str().unwrap_or_default();
    assert!(
        why.contains("paused") && why.contains("may no longer write"),
        "{j}"
    );
    assert!(!has_b1(&local, "uno"), "nothing was synced as ed");
    assert_eq!(
        run_due(&local, due + chrono::Duration::days(30)).await,
        0,
        "a paused job is never due"
    );

    // Write access back and the schedule set again: the job resumes.
    local
        .auth_db
        .update_user_role("ed", SystemRole::Admin)
        .unwrap();
    let (st, txt) = set_schedule(
        &app,
        &token,
        &stream_url,
        json!({ "schedule": "interval", "interval_secs": 60 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let j: Value = serde_json::from_str(&txt).unwrap();
    assert!(j["paused_at"].is_null(), "{txt}");
    let now = at(&j["next_run_at"]);
    assert_eq!(run_due(&local, now).await, 1);
    let j = job(&app, &token, &stream_url).await;
    assert_eq!(j["last_status"], "ok", "{j}");
    assert!(has_b1(&local, "uno"), "synced as ed again");

    // An owner who no longer exists pauses it too.
    local
        .auth_db
        .pool()
        .get()
        .unwrap()
        .execute(
            "UPDATE ldes_sync_state SET created_by = 'gone' WHERE dataset_id = 'mirror'",
            [],
        )
        .unwrap();
    assert_eq!(run_due(&local, at(&j["next_run_at"])).await, 1);
    let j = job(&app, &token, &stream_url).await;
    assert!(j["paused_at"].is_string(), "{j}");
    assert!(
        j["last_error"]
            .as_str()
            .is_some_and(|e| e.contains("no longer exists")),
        "{j}"
    );
}

/// Only the node that writes runs scheduled jobs: a follower holds a copy of
/// every job (the identity database is shipped to it) but starts none, where
/// the same job on a node that is not a follower is started.
#[tokio::test]
async fn a_follower_runs_no_scheduled_job() {
    use open_triplestore::ldes::schedule::run_due;
    use open_triplestore::ldes::store::{record_sync_outcome, set_sync_schedule, Schedule};
    use open_triplestore::store::replication::{Mode, ReplicationConfig, Scope};
    let _net = net().lock().await;
    // Never contacted: not in the allowlist, so a run that starts fails at
    // once.
    let url = "http://leader.invalid/api/datasets/src/ldes";
    let with_job = |state: &AppState| {
        state
            .auth_db
            .create_dataset(
                "mirror",
                "Mirror",
                None,
                OwnerType::User,
                "adm",
                Visibility::Private,
                None,
            )
            .unwrap();
        record_sync_outcome(
            &state.auth_db,
            "mirror",
            url,
            TARGET,
            "adm",
            Err("never ran"),
        )
        .unwrap();
        assert!(set_sync_schedule(
            &state.auth_db,
            "mirror",
            url,
            Schedule::Every(60),
            Some(Utc::now())
        )
        .unwrap());
    };
    let follower = admin_state_with_store(
        open_triplestore::store::TripleStore::in_memory()
            .unwrap()
            .with_replication(ReplicationConfig::follower(
                "http://leader.invalid:7878",
                Mode::Hot,
                Scope::All,
            )),
    )
    .0;
    with_job(&follower);
    let later = Utc::now() + chrono::Duration::days(1);
    assert_eq!(run_due(&follower, later).await, 0);
    let j = open_triplestore::ldes::store::sync_job(&follower.auth_db, "mirror", url)
        .unwrap()
        .unwrap();
    assert_eq!(j.last_error.as_deref(), Some("never ran"));
    assert_eq!(j.consecutive_failures, 0);

    let (leader, _) = admin_state();
    with_job(&leader);
    assert_eq!(run_due(&leader, later).await, 1);
    let j = open_triplestore::ldes::store::sync_job(&leader.auth_db, "mirror", url)
        .unwrap()
        .unwrap();
    assert_eq!(j.consecutive_failures, 1, "{j:?}");
}
