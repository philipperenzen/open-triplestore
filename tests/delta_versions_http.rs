//! Delta versions over HTTP (docs/notes/delta-versioning-design.md §3).
//!
//! With change capture recording, the first version of a dataset is a
//! checkpoint — a physical copy — and the next ones are deltas on it: a base,
//! a range of the change log and a content hash, nothing copied. A delta
//! must read exactly like the checkpoint it replaces: its data is what the
//! graphs held when it was cut, whatever happened to them since. These tests
//! pin that, and everything built on it: chain diffs, branches without
//! copies, publish making a version physical, the two restore tiers, the
//! rule that a base outlives its deltas, the version lock, and the fallback
//! to checkpoints when capture is off.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::server::AppState;
use open_triplestore::store::changes::{DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const G: &str = "urn:dv:g1";

fn recording() -> TripleStore {
    TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD)
}

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
    let retry = resp.headers().get(header::RETRY_AFTER).is_some();
    let text = body_text(resp.into_body()).await;
    let mut json = serde_json::from_str(&text).unwrap_or(Value::Null);
    if retry {
        json = json!({ "retry_after": true, "text": text });
    }
    (status, json, text)
}

/// A dataset owned by the admin with `G` registered and holding two triples.
async fn dataset(state: &AppState, app: &Router, token: &str, visibility: &str) -> String {
    let (st, v, txt) = req(
        app,
        Method::POST,
        "/api/datasets",
        Some(token),
        json!({ "name": "dv", "owner_type": "user", "owner_id": "adm", "visibility": visibility }),
    )
    .await;
    assert!(st.is_success(), "create dataset: {st} {txt}");
    let id = v["id"].as_str().expect("dataset id").to_string();
    let (st, _, txt) = req(
        app,
        Method::POST,
        &format!("/api/datasets/{id}/graphs"),
        Some(token),
        json!({ "graph_iri": G }),
    )
    .await;
    assert!(st.is_success(), "register graph: {st} {txt}");
    state
        .store
        .load_str(
            "<urn:dv:a> <urn:dv:p> \"one\" . <urn:dv:b> <urn:dv:p> \"two\" .",
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
    id
}

async fn cut(app: &Router, token: &str, ds: &str, version: &str) -> Value {
    let (st, v, txt) = req(
        app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions"),
        Some(token),
        json!({ "version": version }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "cut {version}: {txt}");
    v
}

/// The sorted N-Triples lines of a version's data.
async fn data(
    app: &Router,
    token: Option<&str>,
    ds: &str,
    version: &str,
) -> (StatusCode, Vec<String>) {
    let (st, _, text) = req(
        app,
        Method::GET,
        &format!("/api/datasets/{ds}/versions/{version}/data?format=ntriples"),
        token,
        Value::Null,
    )
    .await;
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
        .collect();
    lines.sort();
    (st, lines)
}

fn subjects(lines: &[String]) -> Vec<String> {
    let mut s: Vec<String> = lines
        .iter()
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect();
    s.sort();
    s
}

fn live_subjects(state: &AppState) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = state
        .store
        .query(&format!("SELECT ?s WHERE {{ GRAPH <{G}> {{ ?s ?p ?o }} }}"))
    {
        for row in sols.flatten() {
            out.push(row.get("s").unwrap().to_string());
        }
    }
    out.sort();
    out
}

fn count(state: &AppState, graph: &str) -> usize {
    state.store.count_graph(Some(graph)).unwrap_or(0)
}

fn edit_after_v1(state: &AppState) {
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{G}> {{ <urn:dv:c> <urn:dv:p> \"three\" }} }} ; \
             DELETE DATA {{ GRAPH <{G}> {{ <urn:dv:a> <urn:dv:p> \"one\" }} }}"
        ))
        .unwrap();
}

fn edit_after_v2(state: &AppState) {
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{G}> {{ <urn:dv:d> <urn:dv:p> \"four\" }} }}"
        ))
        .unwrap();
}

#[tokio::test]
async fn the_second_cut_is_a_delta_that_reads_like_the_checkpoint_it_replaces() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let ds = dataset(&state, &app, &token, "private").await;

    let v1 = cut(&app, &token, &ds, "1.0.0").await;
    assert_eq!(v1["kind"], "checkpoint");
    assert!(
        v1["anchor"].is_object(),
        "a checkpoint cut while the log records is anchored: {v1}"
    );
    assert!(
        v1["checkpoint_reason"]
            .as_str()
            .unwrap()
            .contains("checkpoint"),
        "{v1}"
    );
    assert_eq!(count(&state, v1["snapshot_graphs"][0].as_str().unwrap()), 2);

    edit_after_v1(&state);
    let v2 = cut(&app, &token, &ds, "1.1.0").await;
    assert_eq!(v2["kind"], "delta", "{v2}");
    assert_eq!(v2["base_version"], "1.0.0");
    assert!(v2.get("checkpoint_reason").is_none());
    assert_eq!(v2["change_count"], 2);
    assert_eq!(v2["patch_range"]["from_seq"], v1["anchor"]["seq"]);
    assert_eq!(v2["quad_count"], 2);
    // Nothing was copied: its graphs hold nothing until read.
    let snap = v2["snapshot_graphs"][0].as_str().unwrap().to_string();
    assert_eq!(count(&state, &snap), 0);

    // A later change must not leak into 1.1.0.
    edit_after_v2(&state);
    let (st, lines) = data(&app, Some(&token), &ds, "1.1.0").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(subjects(&lines), vec!["<urn:dv:b>", "<urn:dv:c>"]);
    let (_, rec, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/versions/1.1.0"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(rec["materialised"], true);
    assert_eq!(count(&state, &snap), 2);
    // The base still reads as itself.
    let (_, base) = data(&app, Some(&token), &ds, "1.0.0").await;
    assert_eq!(subjects(&base), vec!["<urn:dv:a>", "<urn:dv:b>"]);
    // Two deltas on one base: the third cut is a delta on the same base.
    let v3 = cut(&app, &token, &ds, "1.2.0").await;
    assert_eq!(v3["kind"], "delta");
    assert_eq!(v3["base_version"], "1.0.0");
    assert_eq!(v3["quad_count"], 3);
    let (_, l3) = data(&app, Some(&token), &ds, "1.2.0").await;
    assert_eq!(
        subjects(&l3),
        vec!["<urn:dv:b>", "<urn:dv:c>", "<urn:dv:d>"]
    );
}

#[tokio::test]
async fn a_diff_between_versions_on_one_chain_comes_from_the_change_log() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let ds = dataset(&state, &app, &token, "private").await;
    cut(&app, &token, &ds, "1.0.0").await;
    edit_after_v1(&state);
    cut(&app, &token, &ds, "1.1.0").await;
    edit_after_v2(&state);

    let (st, d, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/versions/1.0.0/diff/1.1.0"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(d["source"], "chain");
    assert_eq!(
        (d["added"].as_u64(), d["removed"].as_u64()),
        (Some(1), Some(1))
    );
    // Backwards is the inverse.
    let (_, back, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/versions/1.1.0/diff/1.0.0"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(
        (back["added"].as_u64(), back["removed"].as_u64()),
        (Some(1), Some(1))
    );
    // Against the live graphs.
    let (_, live, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/versions/1.1.0/diff/live"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(live["source"], "chain");
    assert_eq!(
        (live["added"].as_u64(), live["removed"].as_u64()),
        (Some(1), Some(0))
    );
    // As a patch.
    let (_, _, patch) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/versions/1.0.0/diff/1.1.0?format=rdf-patch"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert!(patch.contains("D <urn:dv:a> <urn:dv:p> \"one\""), "{patch}");
    assert!(
        patch.contains("A <urn:dv:c> <urn:dv:p> \"three\""),
        "{patch}"
    );
    // The versions were never built for any of it.
    assert!(state.store.changes().materialisations(None).is_empty());
}

#[tokio::test]
async fn a_branch_of_a_delta_copies_nothing_and_reads_the_same() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let ds = dataset(&state, &app, &token, "private").await;
    cut(&app, &token, &ds, "1.0.0").await;
    edit_after_v1(&state);
    cut(&app, &token, &ds, "1.1.0").await;
    let (st, b, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/branches"),
        Some(&token),
        json!({ "branch": "fix", "from_version": "1.1.0" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(b["kind"], "delta");
    assert_eq!(b["base_version"], "1.0.0");
    assert_eq!(b["derived_from"], "1.1.0");
    assert_eq!(count(&state, b["snapshot_graphs"][0].as_str().unwrap()), 0);
    let (_, lines) = data(&app, Some(&token), &ds, "1.1.0-fix").await;
    assert_eq!(subjects(&lines), vec!["<urn:dv:b>", "<urn:dv:c>"]);
    // A branch of the checkpoint itself: an empty range on it.
    let (st, b0, _) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/branches"),
        Some(&token),
        json!({ "branch": "alt", "from_version": "1.0.0" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(b0["kind"], "delta");
    assert_eq!(b0["patch_range"]["from_seq"], b0["patch_range"]["to_seq"]);
    let (_, l0) = data(&app, Some(&token), &ds, "1.0.0-alt").await;
    assert_eq!(subjects(&l0), vec!["<urn:dv:a>", "<urn:dv:b>"]);
}

#[tokio::test]
async fn publishing_a_delta_makes_it_physical_and_releases_its_range() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let ds = dataset(&state, &app, &token, "private").await;
    cut(&app, &token, &ds, "1.0.0").await;
    edit_after_v1(&state);
    let v2 = cut(&app, &token, &ds, "1.1.0").await;
    let iri = v2["graph_iri"].as_str().unwrap().to_string();
    assert!(state
        .store
        .changes()
        .pins()
        .iter()
        .any(|p| p.name == format!("version:{iri}")));
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions/1.1.0/publish"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let (_, rec, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/versions/1.1.0"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(rec["kind"], "checkpoint", "{rec}");
    assert_eq!(rec["status"], "published");
    assert!(rec.get("base_version").is_none());
    assert_eq!(
        count(&state, rec["snapshot_graphs"][0].as_str().unwrap()),
        2
    );
    assert!(!state
        .store
        .changes()
        .pins()
        .iter()
        .any(|p| p.name == format!("version:{iri}")));
    assert!(state.store.changes().materialisation(&iri).is_none());
}

#[tokio::test]
async fn a_restore_applies_the_inverse_of_the_chain_in_one_transaction() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let ds = dataset(&state, &app, &token, "private").await;
    cut(&app, &token, &ds, "1.0.0").await;
    edit_after_v1(&state);
    cut(&app, &token, &ds, "1.1.0").await;
    edit_after_v2(&state);
    assert_eq!(
        live_subjects(&state),
        vec!["<urn:dv:b>", "<urn:dv:c>", "<urn:dv:d>"]
    );
    let (st, r, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions/1.0.0/restore"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(r["tier"], 1, "{r}");
    assert_eq!(live_subjects(&state), vec!["<urn:dv:a>", "<urn:dv:b>"]);
    // And forward again, to the delta.
    let (_, r2, _) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions/1.1.0/restore"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(r2["tier"], 1, "{r2}");
    assert_eq!(live_subjects(&state), vec!["<urn:dv:b>", "<urn:dv:c>"]);
    // The restore is in the change log, and the commit points at it.
    let rows = state.store.changes().rows_after(0, 1000);
    assert!(rows.iter().any(|r| r.origin == "apply_delta"));
    let (_, commits, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/commits"),
        Some(&token),
        Value::Null,
    )
    .await;
    let restore = commits
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["message"] == "Restored version 1.1.0")
        .expect("restore commit");
    assert!(restore["seq_to"].is_i64(), "{restore}");
}

#[tokio::test]
async fn a_restore_without_the_change_log_copies_the_version_back() {
    let store = TripleStore::in_memory()
        .unwrap()
        .with_change_capture_disabled();
    let (state, token) = admin_state_with_store(store);
    let app = test_app(state.clone());
    let ds = dataset(&state, &app, &token, "private").await;
    let v1 = cut(&app, &token, &ds, "1.0.0").await;
    assert_eq!(v1["kind"], "checkpoint");
    assert!(v1.get("anchor").is_none(), "no log, no anchor: {v1}");
    edit_after_v1(&state);
    let v2 = cut(&app, &token, &ds, "1.1.0").await;
    assert_eq!(v2["kind"], "checkpoint");
    assert!(v2["checkpoint_reason"]
        .as_str()
        .unwrap()
        .contains("capture is off"));
    let (_, r, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions/1.0.0/restore"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(r["tier"], 2, "{txt}");
    assert_eq!(live_subjects(&state), vec!["<urn:dv:a>", "<urn:dv:b>"]);
}

#[tokio::test]
async fn a_base_outlives_its_deltas() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let ds = dataset(&state, &app, &token, "private").await;
    cut(&app, &token, &ds, "1.0.0").await;
    edit_after_v1(&state);
    cut(&app, &token, &ds, "1.1.0").await;
    let (st, c, txt) = req(
        &app,
        Method::DELETE,
        &format!("/api/datasets/{ds}/versions/1.0.0"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{txt}");
    assert_eq!(c["dependants"], json!(["1.1.0"]));
    // gc collects the delta first, then the base nothing chains from any more.
    let (st, g, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{ds}/versions/gc"),
        Some(&token),
        json!({ "keep": 0 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(g["deleted"], json!(["1.1.0", "1.0.0"]));
    assert!(state.store.changes().pins().is_empty());
}

#[tokio::test]
async fn two_concurrent_cuts_of_one_label_make_one_version() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let ds = dataset(&state, &app, &token, "private").await;
    let uri = format!("/api/datasets/{ds}/versions");
    let body = json!({ "version": "2.0.0" });
    let (a, b) = tokio::join!(
        req(&app, Method::POST, &uri, Some(&token), body.clone()),
        req(&app, Method::POST, &uri, Some(&token), body.clone()),
    );
    let mut statuses = vec![a.0, b.0];
    statuses.sort();
    assert_eq!(statuses, vec![StatusCode::CREATED, StatusCode::BAD_REQUEST]);
    let n = match state.store.query(
        "SELECT (COUNT(*) AS ?n) WHERE { GRAPH <urn:system:dataset-version-registry> { ?v <urn:system:vocab/status> ?s ; <http://www.w3.org/2002/07/owl#versionInfo> \"2.0.0\" } }",
    ) {
        Ok(QueryResults::Solutions(mut s)) => s.next().unwrap().unwrap().get("n").unwrap().to_string(),
        _ => panic!(),
    };
    assert!(n.starts_with("\"1\""), "one status triple, got {n}");
}

#[tokio::test]
async fn an_anonymous_reader_never_builds_a_delta() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let ds = dataset(&state, &app, &token, "public").await;
    cut(&app, &token, &ds, "1.0.0").await;
    edit_after_v1(&state);
    cut(&app, &token, &ds, "1.1.0").await;
    let (st, v, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{ds}/versions/1.1.0/data"),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(v["retry_after"], true);
    assert!(state.store.changes().materialisations(None).is_empty());
    // Once someone built it, anonymous reads are served.
    data(&app, Some(&token), &ds, "1.1.0").await;
    let (st, lines) = data(&app, None, &ds, "1.1.0").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(subjects(&lines), vec!["<urn:dv:b>", "<urn:dv:c>"]);
    // The checkpoint was always readable.
    let (st, _) = data(&app, None, &ds, "1.0.0").await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn an_unused_build_is_swept_and_rebuilt_on_the_next_read() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let ds = dataset(&state, &app, &token, "private").await;
    cut(&app, &token, &ds, "1.0.0").await;
    edit_after_v1(&state);
    let v2 = cut(&app, &token, &ds, "1.1.0").await;
    let snap = v2["snapshot_graphs"][0].as_str().unwrap().to_string();
    data(&app, Some(&token), &ds, "1.1.0").await;
    assert_eq!(count(&state, &snap), 2);
    std::env::set_var("OTS_MATERIALISATION_TTL_SECS", "0");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let swept = open_triplestore::dataset_versions::materialise::sweep(&state.store);
    std::env::remove_var("OTS_MATERIALISATION_TTL_SECS");
    assert_eq!(swept, 1);
    assert_eq!(count(&state, &snap), 0);
    let (_, lines) = data(&app, Some(&token), &ds, "1.1.0").await;
    assert_eq!(subjects(&lines), vec!["<urn:dv:b>", "<urn:dv:c>"]);
}

#[tokio::test]
async fn deleting_a_dataset_takes_its_versions_with_it() {
    let (state, token) = admin_state_with_store(recording());
    let app = test_app(state.clone());
    let ds = dataset(&state, &app, &token, "private").await;
    let v1 = cut(&app, &token, &ds, "1.0.0").await;
    edit_after_v1(&state);
    cut(&app, &token, &ds, "1.1.0").await;
    data(&app, Some(&token), &ds, "1.1.0").await;
    let snap = v1["snapshot_graphs"][0].as_str().unwrap().to_string();
    let (st, _, txt) = req(
        &app,
        Method::DELETE,
        &format!("/api/datasets/{ds}"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert!(st.is_success(), "{txt}");
    assert_eq!(count(&state, &snap), 0);
    assert!(open_triplestore::dataset_versions::registry::list_versions(
        &state.store,
        &state.base_url,
        &ds
    )
    .is_empty());
    assert!(state.store.changes().pins().is_empty());
    assert!(state.store.changes().materialisations(None).is_empty());
}
