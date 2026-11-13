//! Which graphs a model version holds, through the real router:
//!
//! * a draft copies the graphs of its source version only — the base graph and
//!   the sub-graphs under it — and never those of a version whose name merely
//!   starts with the source's (`9.10` next to `9.1`), and it leaves the graph
//!   of every other version as it was;
//! * the diff of two versions compares each graph with the graph in the same
//!   place of the other version — base graph with base graph, sub-graph with
//!   the sub-graph of that name — so two single-graph versions differ by what
//!   was edited, not by every triple they hold.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::registry as dmr;
use open_triplestore::server::AppState;
use oxigraph::model::{GraphNameRef, NamedNodeRef};
use serde_json::{json, Value};
use tower::ServiceExt as _;

const EX: &str = "http://ex.org/def#";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
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
    (status, body_json(resp.into_body()).await)
}

/// The model `m1`, owned by the admin of [`admin_state`].
fn model(state: &AppState) {
    dmr::insert_data_model(
        &state.store,
        &state.base_url,
        "m1",
        "Bridges",
        EX,
        None,
        true,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
}

fn version_graph(state: &AppState, ver: &str) -> String {
    dmr::version_record_iri(&state.base_url, "m1", ver)
}

/// Register version `ver` of `m1` with the sub-graphs named in `subs` (none:
/// the version has its base graph only, recorded with no sub-graphs).
fn version(state: &AppState, ver: &str, subs: &[&str]) {
    let graph_iri = version_graph(state, ver);
    dmr::insert_version(
        &state.store,
        &state.base_url,
        &DataModelVersion {
            data_model_id: "m1".to_string(),
            version: ver.to_string(),
            status: VersionStatus::Published,
            sub_graphs: subs.iter().map(|s| format!("{graph_iri}/{s}")).collect(),
            graph_iri,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: vec![],
        },
    )
    .unwrap();
}

/// Give the class `{EX}{class}` the label `label` in `graph`.
fn label(state: &AppState, graph: &str, class: &str, label: &str) {
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{graph}> {{ <{EX}{class}> <{RDFS_LABEL}> \"{label}\" }} }}"
        ))
        .unwrap();
}

/// The labels in `graph`, as sorted `Class=label` pairs.
fn labels(state: &AppState, graph: &str) -> Vec<String> {
    let graph = NamedNodeRef::new(graph).unwrap();
    let mut out: Vec<String> = state
        .store
        .quads_for_graph(GraphNameRef::NamedNode(graph))
        .unwrap()
        .into_iter()
        .map(|q| {
            let subject = q.subject.to_string();
            let class = subject
                .trim_start_matches(&format!("<{EX}"))
                .trim_end_matches('>');
            format!("{class}={}", q.object.to_string().trim_matches('"'))
        })
        .collect();
    out.sort();
    out
}

fn graph_exists(state: &AppState, graph: &str) -> bool {
    state
        .store
        .named_graphs()
        .unwrap()
        .iter()
        .any(|g| g.as_str() == graph)
}

/// A graph belongs to a version when it is the version's base graph or lies
/// under it; a version name that only starts the same does not make it one.
#[test]
fn a_versions_graphs_end_at_a_path_boundary() {
    let base = "http://localhost/data-model/m1/version/9.1";
    assert_eq!(dmr::version_graph_suffix(base, base), Some(""));
    assert_eq!(
        dmr::version_graph_suffix(base, &format!("{base}/shapes")),
        Some("shapes")
    );
    assert_eq!(
        dmr::version_graph_suffix(base, &format!("{base}/a/b")),
        Some("a/b")
    );
    for other in ["9.10", "9.1-rc1", "9.1.1", "9.10/shapes", "9"] {
        let graph = format!("http://localhost/data-model/m1/version/{other}");
        assert_eq!(dmr::version_graph_suffix(base, &graph), None, "{other}");
    }
}

/// `9.1` is a prefix of `9.10`: a draft `9.2` of `9.1` holds 9.1's triples
/// only and lists its own graph only, and the existing version `9.20` — whose
/// graph is what 9.10's graph would be renamed to — keeps its content.
#[tokio::test]
async fn a_draft_leaves_out_versions_whose_name_extends_the_sources() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    model(&state);
    for (ver, class, text) in [
        ("9.1", "Bridge", "nine one"),
        ("9.10", "Tunnel", "nine ten"),
        ("9.20", "Lock", "nine twenty"),
    ] {
        version(&state, ver, &[]);
        label(&state, &version_graph(&state, ver), class, text);
    }

    let (status, draft) = req(
        &app,
        Method::POST,
        "/api/models/m1/versions/9.1/draft",
        &token,
        json!({ "target_version": "9.2" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{draft}");

    let draft_graph = version_graph(&state, "9.2");
    assert_eq!(draft["sub_graphs"], json!([draft_graph]), "{draft}");
    assert_eq!(labels(&state, &draft_graph), ["Bridge=nine one"]);

    // The other versions are as they were.
    for (ver, expected) in [
        ("9.1", "Bridge=nine one"),
        ("9.10", "Tunnel=nine ten"),
        ("9.20", "Lock=nine twenty"),
    ] {
        assert_eq!(
            labels(&state, &version_graph(&state, ver)),
            [expected],
            "{ver}"
        );
    }
    let (status, listed) = req(
        &app,
        Method::GET,
        "/api/models/m1/versions/9.2",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["sub_graphs"], json!([draft_graph]), "{listed}");
}

/// A draft still copies every sub-graph of its source, and none of a
/// pre-release named after it (`1.0-rc1` next to `1.0`).
#[tokio::test]
async fn a_draft_copies_the_sources_sub_graphs_and_no_others() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    model(&state);
    version(&state, "1.0", &["ontology", "shapes"]);
    version(&state, "1.0-rc1", &["ontology"]);
    let released = version_graph(&state, "1.0");
    let candidate = version_graph(&state, "1.0-rc1");
    label(
        &state,
        &format!("{released}/ontology"),
        "Bridge",
        "released",
    );
    label(
        &state,
        &format!("{released}/shapes"),
        "BridgeShape",
        "released shape",
    );
    label(
        &state,
        &format!("{candidate}/ontology"),
        "Bridge",
        "candidate",
    );

    let (status, draft) = req(
        &app,
        Method::POST,
        "/api/models/m1/versions/1.0/draft",
        &token,
        json!({ "target_version": "1.1" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{draft}");

    let draft_graph = version_graph(&state, "1.1");
    assert_eq!(
        draft["sub_graphs"],
        json!([
            format!("{draft_graph}/ontology"),
            format!("{draft_graph}/shapes")
        ]),
        "{draft}"
    );
    assert_eq!(
        labels(&state, &format!("{draft_graph}/ontology")),
        ["Bridge=released"]
    );
    assert_eq!(
        labels(&state, &format!("{draft_graph}/shapes")),
        ["BridgeShape=released shape"]
    );
    assert!(
        !graph_exists(&state, &format!("{draft_graph}-rc1/ontology")),
        "the release candidate's graph was copied as a graph of version 1.1-rc1"
    );
    assert_eq!(
        labels(&state, &format!("{candidate}/ontology")),
        ["Bridge=candidate"]
    );
}

// ─── GET /api/models/:id/diff ────────────────────────────────────────────────

fn class(name: &str) -> String {
    format!("<{EX}{name}>")
}

fn label_triple(class_name: &str, text: &str, graph: &str) -> Value {
    json!({
        "s": class(class_name),
        "p": format!("<{RDFS_LABEL}>"),
        "o": format!("\"{text}\""),
        "graph": graph,
    })
}

/// A diff list in a stable order.
fn sorted(list: &Value) -> Vec<Value> {
    let mut items = list.as_array().expect("a diff list").clone();
    items.sort_by_key(|item| item.to_string());
    items
}

async fn diff(app: &Router, token: &str, query: &str) -> Value {
    let (status, body) = req(
        app,
        Method::GET,
        &format!("/api/models/m1/diff?{query}"),
        token,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{query}: {body}");
    body
}

/// Two versions that each have one graph (1.0.0 recorded with no sub-graphs,
/// its draft 1.0.1 listing its base graph, as a draft does) and differ by one
/// relabelled class, one new class and one dropped class: the diff holds those
/// three, labelled with the base graph on both sides, in either direction.
#[tokio::test]
async fn the_diff_of_single_graph_versions_holds_only_what_changed() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    model(&state);
    version(&state, "1.0.0", &[]);
    let before = version_graph(&state, "1.0.0");
    for (class_name, text) in [
        ("Bridge", "Bridge"),
        ("Tunnel", "Tunnel"),
        ("Lock", "Lock"),
        ("Culvert", "Culvert"),
    ] {
        label(&state, &before, class_name, text);
    }

    let (status, draft) = req(
        &app,
        Method::POST,
        "/api/models/m1/versions/1.0.0/draft",
        &token,
        json!({ "target_version": "1.0.1" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{draft}");
    let after = version_graph(&state, "1.0.1");
    assert_eq!(draft["sub_graphs"], json!([after]), "{draft}");

    // Nothing edited yet: no difference.
    let same = diff(&app, &token, "from=1.0.0&to=1.0.1").await;
    assert_eq!(
        same["summary"],
        json!({ "added": 0, "removed": 0, "changed": 0 }),
        "{same}"
    );

    state
        .store
        .update(&format!(
            "DELETE DATA {{ GRAPH <{after}> {{ \
               <{EX}Bridge> <{RDFS_LABEL}> \"Bridge\" . \
               <{EX}Culvert> <{RDFS_LABEL}> \"Culvert\" }} }} ; \
             INSERT DATA {{ GRAPH <{after}> {{ \
               <{EX}Bridge> <{RDFS_LABEL}> \"Road bridge\" . \
               <{EX}Viaduct> <{RDFS_LABEL}> \"Viaduct\" }} }}"
        ))
        .unwrap();

    let forward = diff(&app, &token, "from=1.0.0&to=1.0.1").await;
    assert_eq!(
        forward["summary"],
        json!({ "added": 1, "removed": 1, "changed": 1 }),
        "{forward}"
    );
    assert_eq!(
        forward["added"],
        json!([label_triple("Viaduct", "Viaduct", "")]),
        "{forward}"
    );
    assert_eq!(
        forward["removed"],
        json!([label_triple("Culvert", "Culvert", "")]),
        "{forward}"
    );
    assert_eq!(
        forward["changed"],
        json!([{
            "s": class("Bridge"),
            "p": format!("<{RDFS_LABEL}>"),
            "before": "\"Bridge\"",
            "after": "\"Road bridge\"",
            "graph": "",
        }]),
        "{forward}"
    );

    let backward = diff(&app, &token, "from=1.0.1&to=1.0.0").await;
    assert_eq!(
        backward["summary"],
        json!({ "added": 1, "removed": 1, "changed": 1 }),
        "{backward}"
    );
    assert_eq!(
        backward["added"],
        json!([label_triple("Culvert", "Culvert", "")]),
        "{backward}"
    );
    assert_eq!(
        backward["removed"],
        json!([label_triple("Viaduct", "Viaduct", "")]),
        "{backward}"
    );
    assert_eq!(backward["changed"][0]["before"], "\"Road bridge\"");
    assert_eq!(backward["changed"][0]["after"], "\"Bridge\"");
}

/// Versions with sub-graphs are still compared sub-graph by sub-graph, each
/// triple labelled with its sub-graph's name; a sub-graph only the newer
/// version has is all additions, and `graph=` narrows the diff to one.
#[tokio::test]
async fn the_diff_of_multi_graph_versions_pairs_sub_graphs_by_name() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    model(&state);
    version(&state, "2.0", &["ontology", "shapes"]);
    version(&state, "2.1", &["ontology", "shapes", "examples"]);
    let old = version_graph(&state, "2.0");
    let new = version_graph(&state, "2.1");
    for graph in [&old, &new] {
        label(&state, &format!("{graph}/ontology"), "Bridge", "Bridge");
        label(
            &state,
            &format!("{graph}/shapes"),
            "BridgeShape",
            "Bridge shape",
        );
    }
    label(&state, &format!("{new}/ontology"), "Viaduct", "Viaduct");
    label(&state, &format!("{new}/examples"), "Example", "An example");

    let all = diff(&app, &token, "from=2.0&to=2.1").await;
    assert_eq!(
        all["summary"],
        json!({ "added": 2, "removed": 0, "changed": 0 }),
        "{all}"
    );
    assert_eq!(
        sorted(&all["added"]),
        sorted(&json!([
            label_triple("Viaduct", "Viaduct", "ontology"),
            label_triple("Example", "An example", "examples"),
        ])),
        "{all}"
    );

    let one = diff(&app, &token, "from=2.0&to=2.1&graph=ontology").await;
    assert_eq!(
        one["added"],
        json!([label_triple("Viaduct", "Viaduct", "ontology")]),
        "{one}"
    );
    assert_eq!(
        one["summary"],
        json!({ "added": 1, "removed": 0, "changed": 0 }),
        "{one}"
    );
}
