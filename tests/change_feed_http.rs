//! A dataset's change feed over HTTP (docs/notes/delta-versioning-design.md
//! §7): the change log's rows for the dataset's graphs, one change as an RDF
//! Patch and its inverse, consumer cursors, the admin statistics, and the
//! commit trail's pointer into the log.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::server::AppState;
use open_triplestore::store::changes::{DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const G: &str = "urn:cf:public";
const P: &str = "urn:cf:private";
const OTHER: &str = "urn:cf:elsewhere";

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        b = b.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    )
}

/// A public dataset holding a public and a private graph, plus a graph of
/// no dataset at all; each written once.
async fn setup(store: TripleStore) -> (AppState, String, Router, String) {
    let (state, token) = admin_state_with_store(store);
    let app = test_app(state.clone());
    let (st, v, txt) = req(
        &app,
        Method::POST,
        "/api/datasets",
        Some(&token),
        json!({ "name": "cf", "owner_type": "user", "owner_id": "adm", "visibility": "public" }),
    )
    .await;
    assert!(st.is_success(), "{txt}");
    let id = v["id"].as_str().unwrap().to_string();
    for g in [G, P] {
        let (st, _, txt) = req(
            &app,
            Method::POST,
            &format!("/api/datasets/{id}/graphs"),
            Some(&token),
            json!({ "graph_iri": g }),
        )
        .await;
        assert!(st.is_success(), "{txt}");
    }
    let (st, _, txt) = req(
        &app,
        Method::PATCH,
        &format!("/api/datasets/{id}/graphs"),
        Some(&token),
        json!({ "graph_iri": P, "private": true }),
    )
    .await;
    assert!(st.is_success(), "{txt}");
    for g in [G, P, OTHER] {
        state
            .store
            .load_str(
                &format!("<urn:cf:s> <urn:cf:in> <{g}> ."),
                RdfFormat::Turtle,
                Some(g),
            )
            .unwrap();
    }
    (state, token, app, id)
}

fn recording() -> TripleStore {
    TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD)
}

fn graphs_of(feed: &Value) -> Vec<String> {
    feed["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["graph_iri"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn the_feed_lists_the_rows_of_graphs_the_caller_may_read() {
    let (_state, token, app, id) = setup(recording()).await;
    // The owner sees both of the dataset's graphs, never another dataset's.
    let (st, feed, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{id}/changes"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(feed["enabled"], true);
    let gs = graphs_of(&feed);
    assert!(
        gs.contains(&G.to_string()) && gs.contains(&P.to_string()),
        "{gs:?}"
    );
    assert!(!gs.contains(&OTHER.to_string()));
    // Rows carry counts, not quads.
    assert!(feed["rows"][0]["added"].is_null());
    assert!(feed["rows"][0]["added_n"].is_i64());
    // An anonymous reader of the public dataset does not see the private graph.
    let (st, anon, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{id}/changes"),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let anon_graphs = graphs_of(&anon);
    assert!(anon_graphs.contains(&G.to_string()), "{anon_graphs:?}");
    assert!(!anon_graphs.contains(&P.to_string()), "{anon_graphs:?}");
    // Resuming from `next` returns nothing new.
    let next = feed["next"].as_i64().unwrap();
    let (_, rest, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{id}/changes?since={next}"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert!(rest["rows"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn one_change_is_a_patch_and_so_is_its_inverse() {
    let (state, token, app, id) = setup(recording()).await;
    let rows = state.store.changes().rows_after(0, 100);
    let seq_of = |g: &str| {
        rows.iter()
            .find(|r| r.graph_iri.as_deref() == Some(g))
            .and_then(|r| r.seq)
            .unwrap()
    };
    let (st, _, patch) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{id}/changes/{}", seq_of(G)),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{patch}");
    assert!(
        patch.contains(&format!("A <urn:cf:s> <urn:cf:in> <{G}> <{G}> .")),
        "{patch}"
    );
    assert!(patch.contains("H seq "), "{patch}");
    let (st, _, inverse) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{id}/changes/{}/inverse", seq_of(G)),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        inverse.contains(&format!("D <urn:cf:s> <urn:cf:in> <{G}> <{G}> .")),
        "{inverse}"
    );
    assert!(inverse.contains(":inverse"), "{inverse}");
    // Not readable: the private graph's change for an anonymous caller, and
    // another dataset's change for anyone.
    for (seq, who) in [(seq_of(P), None), (seq_of(OTHER), Some(token.as_str()))] {
        let (st, _, _) = req(
            &app,
            Method::GET,
            &format!("/api/datasets/{id}/changes/{seq}"),
            who,
            Value::Null,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
    let (st, _, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{id}/changes/999999"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_change_too_large_to_carry_its_quads_has_no_patch() {
    let store = TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, 1);
    let (state, token, app, id) = setup(store).await;
    state
        .store
        .load_str(
            "<urn:cf:a> <urn:cf:p> 1 . <urn:cf:b> <urn:cf:p> 2 .",
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
    let seq = state.store.changes().last_seq();
    let (st, body, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{id}/changes/{seq}"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{txt}");
    assert_eq!(body["extent"], "counts");
    assert_eq!(body["added_n"], 2);
}

#[tokio::test]
async fn a_cursor_belongs_to_its_owner() {
    let (state, token, app, id) = setup(recording()).await;
    state
        .auth_db
        .create_user("mo", "mo", "mo@t.com", "h", SystemRole::User)
        .unwrap();
    let mo = mint_token("mo", "mo", "user");
    let uri = format!("/api/datasets/{id}/changes/cursors/mirror");
    let (st, c, txt) = req(&app, Method::PUT, &uri, Some(&mo), json!({ "seq": 1 })).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(c["owner"], "mo");
    assert!(state
        .store
        .changes()
        .cursor(&format!("dataset:{id}:mirror"))
        .is_some());
    // Someone else can neither move nor delete it; an admin can delete it.
    state
        .auth_db
        .create_user("eve", "eve", "eve@t.com", "h", SystemRole::User)
        .unwrap();
    let eve = mint_token("eve", "eve", "user");
    let (st, _, _) = req(&app, Method::PUT, &uri, Some(&eve), json!({ "seq": 1 })).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = req(&app, Method::DELETE, &uri, Some(&eve), Value::Null).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _, _) = req(&app, Method::PUT, &uri, Some(&mo), json!({ "seq": 99999 })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _, _) = req(&app, Method::DELETE, &uri, Some(&token), Value::Null).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, _) = req(&app, Method::PUT, &uri, None, json!({ "seq": 1 })).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_admin_statistics_account_for_the_log_per_dataset() {
    let (_state, token, app, id) = setup(recording()).await;
    let (st, s, txt) = req(
        &app,
        Method::GET,
        "/api/admin/changes/stats",
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(s["datasets"][&id]["rows"].as_i64().unwrap() >= 2, "{s}");
    assert!(s["datasets"][&id]["payload_bytes"].as_i64().unwrap() > 0);
    assert!(
        s["unregistered_graphs"]["rows"].as_i64().unwrap() >= 1,
        "the graph of no dataset: {s}"
    );
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/admin/changes/stats",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_commit_points_at_the_rows_its_write_made() {
    let (_state, token, app, id) = setup(recording()).await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/store?graph={}", url_encode(G)))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "text/turtle")
                .body(Body::from("<urn:cf:n> <urn:cf:p> \"new\" ."))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let (_, commits, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{id}/commits"),
        Some(&token),
        Value::Null,
    )
    .await;
    let c = commits
        .as_array()
        .unwrap_or_else(|| panic!("{txt}"))
        .iter()
        .find(|c| c["kind"] == "graph-store")
        .expect("the POST's commit");
    assert!(c["seq_to"].is_i64(), "{c}");
    assert_eq!(c["seq_from"], c["seq_to"]);
    assert_eq!(c["net_added"], 1);
    assert_eq!(c["net_removed"], 0);
}
