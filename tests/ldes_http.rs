//! LDES publishing and client sync (6.1), over the real HTTP API.
//!
//! Publishing: enabling a stream seeds a member per entity; every write path
//! (Graph Store PUT/POST/DELETE, SPARQL Update) yields members for exactly the
//! entities it changed, deletions become tombstones, and fragments chain with
//! `tree:GreaterThanOrEqualToRelation`. Sync: a second instance of this server
//! on a local listener publishes a dataset; the local instance mirrors it,
//! then mirrors the increments (an update and a deletion).

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{OwnerType, Visibility};
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
        n1.contains("tree:GreaterThanOrEqualToRelation") && n1.contains("ldes/nodes/2"),
        "page 1 links to page 2: {n1}"
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

/// A second instance on a local listener publishes; this instance syncs.
/// The publisher's base URL is the listener's origin, as a deployment's
/// BASE_URL is its public origin — the stream's node IRIs are built from it.
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
    let (remote_state, remote_token, origin) = publisher();
    std::env::set_var("OTS_REMOTE_ALLOWLIST", format!("{origin}/"));
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

    // Not allowlisted: refused before any request.
    std::env::set_var("OTS_REMOTE_ALLOWLIST", "");
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
