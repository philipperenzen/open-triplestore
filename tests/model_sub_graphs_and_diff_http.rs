//! Naming a sub-graph of a model version, and the diff of versions that hold
//! several values for one subject and predicate, through the real router:
//!
//! * a sub-graph goes by its place in its version — `shapes` for
//!   `{version graph}/shapes`, with or without a leading `/` — or by its full
//!   IRI, never by the tail of its IRI: next to a sub-graph `core-shapes`, the
//!   download, an edit and a lifecycle change addressed to `shapes` reach
//!   `shapes` only, and where there is no `shapes` they do not fall on
//!   `core-shapes` instead;
//! * a nested sub-graph `ontology/core` also answers to `core`, as it does in
//!   the diff's `graph=`; an edit or a lifecycle change takes the sub-graph
//!   named exactly so first, and refuses a name several nested ones share;
//! * the diff names every triple that differs once: a removed value pairs
//!   with one added value of its subject and predicate as a change, and the
//!   values left over are plain removals and additions.

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

fn request(method: Method, uri: &str, token: &str, body: Value) -> Request<Body> {
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
    b.body(body).unwrap()
}

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(request(method, uri, token, body))
        .await
        .unwrap();
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

/// Register version `ver` of `m1` in `status`, with the sub-graphs named in
/// `subs` (none: the version has its base graph only).
fn version(state: &AppState, ver: &str, status: VersionStatus, subs: &[&str]) {
    let graph_iri = version_graph(state, ver);
    dmr::insert_version(
        &state.store,
        &state.base_url,
        &DataModelVersion {
            data_model_id: "m1".to_string(),
            version: ver.to_string(),
            status,
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

// ─── A sub-graph by name ─────────────────────────────────────────────────────

/// Version `1.0` of `m1` in `status` with the sub-graphs `subs`, each holding
/// one label: the class `Shape` labelled with the sub-graph's own name.
fn version_with_sub_graphs(state: &AppState, status: VersionStatus, subs: &[&str]) -> String {
    model(state);
    version(state, "1.0", status, subs);
    let base = version_graph(state, "1.0");
    for sub in subs {
        label(state, &format!("{base}/{sub}"), "Shape", sub);
    }
    base
}

/// The labels in the answer of `GET /api/models/m1/versions/1.0/data?{query}`,
/// sorted: every sub-graph of [`version_with_sub_graphs`] holds one label, its
/// own name, so these are the sub-graphs the download took.
async fn download(app: &Router, token: &str, query: &str) -> Vec<String> {
    let uri = format!("/api/models/m1/versions/1.0/data?{query}");
    let resp = app
        .clone()
        .oneshot(request(Method::GET, &uri, token, Value::Null))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "{query}");
    let text = body_text(resp.into_body()).await;
    // The quoted strings of the document: its literals.
    let mut found: Vec<String> = text
        .split('"')
        .skip(1)
        .step_by(2)
        .map(String::from)
        .collect();
    found.sort();
    found
}

/// `PATCH /api/models/m1/versions/1.0/data`.
async fn patch(app: &Router, token: &str, body: Value) -> (StatusCode, Value) {
    req(
        app,
        Method::PATCH,
        "/api/models/m1/versions/1.0/data",
        token,
        body,
    )
    .await
}

/// A label of the class `{EX}{class}` as a triple of a patch, filed under the
/// graph `graph` when one is given.
fn label_edit(class: &str, text: &str, graph: Option<&str>) -> Value {
    let mut triple = json!({
        "s": format!("{EX}{class}"),
        "p": RDFS_LABEL,
        "o": { "value": text },
    });
    if let Some(graph) = graph {
        triple["graph"] = json!(graph);
    }
    triple
}

/// `POST /api/models/m1/versions/1.0/subgraph/{action}` for the sub-graph
/// `graph`.
async fn transition(app: &Router, token: &str, action: &str, graph: &str) -> (StatusCode, Value) {
    req(
        app,
        Method::POST,
        &format!("/api/models/m1/versions/1.0/subgraph/{action}"),
        token,
        json!({ "graph": graph }),
    )
    .await
}

/// The status recorded for each sub-graph of version `1.0`, as sorted
/// `graph IRI=status` pairs.
async fn sub_graph_statuses(app: &Router, token: &str) -> Vec<String> {
    let (status, version) = req(
        app,
        Method::GET,
        "/api/models/m1/versions/1.0",
        token,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{version}");
    let mut out: Vec<String> = version["sub_graph_status"]
        .as_array()
        .expect("sub_graph_status")
        .iter()
        .map(|s| {
            format!(
                "{}={}",
                s["graph_iri"].as_str().unwrap(),
                s["status"].as_str().unwrap()
            )
        })
        .collect();
    out.sort();
    out
}

/// `graph=shapes` downloads the sub-graph `shapes`, not `core-shapes` with it;
/// the name may carry a leading `/` and the full IRI names the same graph.
#[tokio::test]
async fn a_download_takes_the_sub_graph_of_that_name_only() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let base =
        version_with_sub_graphs(&state, VersionStatus::Published, &["core-shapes", "shapes"]);

    for query in [
        "graph=shapes".to_string(),
        "graph=/shapes".to_string(),
        format!("graph={}", url_encode(&format!("{base}/shapes"))),
        "format=turtle&graph=shapes".to_string(),
    ] {
        assert_eq!(download(&app, &token, &query).await, ["shapes"], "{query}");
    }
    assert_eq!(
        download(&app, &token, "graph=core-shapes").await,
        ["core-shapes"]
    );

    // Both sub-graphs still come without a name, and with `all`.
    for query in ["", "graph=all", "graph="] {
        assert_eq!(
            download(&app, &token, query).await,
            ["core-shapes", "shapes"],
            "{query}"
        );
    }
}

/// The two orders a version can list `core-shapes` and `shapes` in: which of
/// them a name that fits both reaches must not depend on it.
const BOTH_ORDERS: [[&str; 2]; 2] = [["core-shapes", "shapes"], ["shapes", "core-shapes"]];

/// An edit addressed to `shapes`, for the whole request or for one triple,
/// lands in `shapes` and leaves `core-shapes` as it was.
#[tokio::test]
async fn an_edit_lands_in_the_sub_graph_of_that_name() {
    for subs in BOTH_ORDERS {
        let (state, token) = admin_state();
        let app = test_app(state.clone());
        let base = version_with_sub_graphs(&state, VersionStatus::Draft, &subs);
        let shapes = format!("{base}/shapes");
        let core = format!("{base}/core-shapes");

        // The request's graph.
        let (status, body) = patch(
            &app,
            &token,
            json!({ "add": [label_edit("Bridge", "one", None)], "remove": [], "graph": "shapes" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        // A triple's own graph, with a leading slash.
        let (status, body) = patch(
            &app,
            &token,
            json!({ "add": [label_edit("Tunnel", "two", Some("/shapes"))], "remove": [] }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        // A removal, by the triple's own graph.
        let (status, body) = patch(
            &app,
            &token,
            json!({ "add": [], "remove": [label_edit("Shape", "shapes", Some("shapes"))] }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        assert_eq!(
            labels(&state, &shapes),
            ["Bridge=one", "Tunnel=two"],
            "{subs:?}"
        );
        assert_eq!(labels(&state, &core), ["Shape=core-shapes"], "{subs:?}");
    }
}

/// With no sub-graph `shapes`, an edit addressed to `shapes` does not fall on
/// `core-shapes`: it goes to the graph of that name under the version's base
/// graph, as an edit addressed to any name the version does not have does.
#[tokio::test]
async fn an_edit_does_not_fall_on_a_sub_graph_whose_name_ends_the_same() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let base = version_with_sub_graphs(&state, VersionStatus::Draft, &["core-shapes"]);

    let (status, body) = patch(
        &app,
        &token,
        json!({
            "add": [
                label_edit("Bridge", "one", None),
                label_edit("Tunnel", "two", Some("shapes")),
            ],
            "remove": [label_edit("Shape", "core-shapes", Some("shapes"))],
            "graph": "shapes",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(
        labels(&state, &format!("{base}/core-shapes")),
        ["Shape=core-shapes"]
    );
    assert_eq!(
        labels(&state, &format!("{base}/shapes")),
        ["Bridge=one", "Tunnel=two"]
    );
}

/// Staging, publishing and deprecating `shapes` set the status of `shapes`;
/// `core-shapes` keeps its own.
#[tokio::test]
async fn a_lifecycle_change_reaches_the_sub_graph_of_that_name() {
    for subs in BOTH_ORDERS {
        let (state, token) = admin_state();
        let app = test_app(state.clone());
        let base = version_with_sub_graphs(&state, VersionStatus::Published, &subs);
        let shapes = format!("{base}/shapes");
        let core = format!("{base}/core-shapes");

        for (action, expected) in [
            ("stage", "staged"),
            ("publish", "published"),
            ("deprecate", "deprecated"),
        ] {
            let (status, body) = transition(&app, &token, action, "shapes").await;
            assert_eq!(status, StatusCode::OK, "{action}: {body}");
            assert_eq!(body["graph"], json!(shapes), "{action}: {body}");
            assert_eq!(
                sub_graph_statuses(&app, &token).await,
                [format!("{shapes}={expected}")],
                "{action}, {subs:?}"
            );
        }

        // The other sub-graph by its name, and `shapes` by its full IRI and
        // with a leading slash.
        let (status, body) = transition(&app, &token, "stage", "core-shapes").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["graph"], json!(core), "{body}");
        let (status, body) = transition(&app, &token, "publish", &shapes).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["graph"], json!(shapes), "{body}");
        let (status, body) = transition(&app, &token, "stage", "/shapes").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["graph"], json!(shapes), "{body}");
        assert_eq!(
            sub_graph_statuses(&app, &token).await,
            [format!("{core}=staged"), format!("{shapes}=staged")],
            "{subs:?}"
        );
    }
}

/// With no sub-graph `shapes`, a lifecycle change of `shapes` is refused and
/// `core-shapes` keeps the status of its version.
#[tokio::test]
async fn a_lifecycle_change_does_not_fall_on_a_sub_graph_whose_name_ends_the_same() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    version_with_sub_graphs(&state, VersionStatus::Published, &["core-shapes"]);

    for action in ["stage", "publish", "deprecate"] {
        let (status, body) = transition(&app, &token, action, "shapes").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{action}: {body}");
    }
    assert!(sub_graph_statuses(&app, &token).await.is_empty());
}

/// A nested sub-graph answers to its whole name and to its trailing segments,
/// as in the diff's `graph=`. Where one graph is wanted, the sub-graph named
/// exactly so comes before a nested one ending in the name, and a name that
/// only several nested ones end in is refused.
#[tokio::test]
async fn a_nested_sub_graph_answers_to_its_trailing_segments() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let subs = [
        "ontology/core",
        "shapes",
        "extra/shapes",
        "a/rules",
        "b/rules",
    ];
    let base = version_with_sub_graphs(&state, VersionStatus::Draft, &subs);
    let unchanged = |sub: &str| -> bool {
        labels(&state, &format!("{base}/{sub}")) == [format!("Shape={sub}")]
    };

    // The download keeps every sub-graph that answers to the name, by whole
    // segments: `a/shapes` is not the end of `extra/shapes`.
    for name in ["core", "ontology/core", "/ontology/core"] {
        assert_eq!(
            download(&app, &token, &format!("graph={name}")).await,
            ["ontology/core"],
            "{name}"
        );
    }
    assert_eq!(
        download(&app, &token, "graph=rules").await,
        ["a/rules", "b/rules"]
    );
    assert_eq!(
        download(&app, &token, "graph=shapes").await,
        ["extra/shapes", "shapes"]
    );
    assert_eq!(
        download(&app, &token, "graph=a/shapes").await,
        Vec::<String>::new()
    );

    // An edit: the only sub-graph ending in `core`; `shapes` itself, not
    // `extra/shapes`; and `extra/shapes` by its whole name.
    for (graph, class) in [
        ("core", "Bridge"),
        ("shapes", "Tunnel"),
        ("extra/shapes", "Lock"),
    ] {
        let (status, body) = patch(
            &app,
            &token,
            json!({ "add": [label_edit(class, "new", Some(graph))], "remove": [] }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{graph}: {body}");
    }
    assert_eq!(
        labels(&state, &format!("{base}/ontology/core")),
        ["Bridge=new", "Shape=ontology/core"]
    );
    assert_eq!(
        labels(&state, &format!("{base}/shapes")),
        ["Shape=shapes", "Tunnel=new"]
    );
    assert_eq!(
        labels(&state, &format!("{base}/extra/shapes")),
        ["Lock=new", "Shape=extra/shapes"]
    );
    assert!(!graph_exists(&state, &format!("{base}/core")));

    // `rules` is the end of two sub-graphs and the name of none: nothing is
    // written, as the request's graph or as a triple's — not even the other
    // triples of the request — and no status is set.
    for body in [
        json!({ "add": [label_edit("Weir", "new", None)], "remove": [], "graph": "rules" }),
        json!({ "add": [label_edit("Weir", "new", Some("rules"))], "remove": [] }),
        json!({
            "add": [label_edit("Weir", "new", Some("rules"))],
            "remove": [label_edit("Shape", "shapes", Some("shapes"))],
        }),
    ] {
        let (status, answer) = patch(&app, &token, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    }
    let (status, answer) = transition(&app, &token, "stage", "rules").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(unchanged("a/rules") && unchanged("b/rules"));
    assert!(!graph_exists(&state, &format!("{base}/rules")));
    assert_eq!(
        labels(&state, &format!("{base}/shapes")),
        ["Shape=shapes", "Tunnel=new"]
    );
    assert!(sub_graph_statuses(&app, &token).await.is_empty());
}

// ─── GET /api/models/:id/diff ────────────────────────────────────────────────

/// `(class, label)` pairs in order — with repeats, when a list names one twice.
type Labels = Vec<(String, String)>;

/// The pairs of `these` that `those` does not hold.
fn only_in(these: &[(&str, &str)], those: &[(&str, &str)]) -> Labels {
    let mut out: Labels = these
        .iter()
        .filter(|pair| !those.contains(pair))
        .map(|(class, text)| (class.to_string(), text.to_string()))
        .collect();
    out.sort();
    out
}

/// The versions `1.0` and `1.1` of `m1`, one graph each, holding the labels
/// `from` and `to`, and their diff from `1.0` to `1.1`.
async fn diff_of(from: &[(&str, &str)], to: &[(&str, &str)]) -> Value {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    model(&state);
    for (ver, content) in [("1.0", from), ("1.1", to)] {
        version(&state, ver, VersionStatus::Published, &[]);
        for (class, text) in content {
            label(&state, &version_graph(&state, ver), class, text);
        }
    }
    diff(&app, &token, "from=1.0&to=1.1").await
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

/// The class and label a diff names: the subject of `item` and its `value`
/// (`o`, `before` or `after`).
fn class_and_label(item: &Value, value: &str) -> (String, String) {
    assert_eq!(item["p"], json!(format!("<{RDFS_LABEL}>")), "{item}");
    let class = item["s"]
        .as_str()
        .and_then(|s| s.strip_prefix(&format!("<{EX}")))
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or_else(|| panic!("the subject of {item}"));
    let text = item[value]
        .as_str()
        .and_then(|o| o.strip_prefix('"'))
        .and_then(|o| o.strip_suffix('"'))
        .unwrap_or_else(|| panic!("the {value} of {item}"));
    (class.to_string(), text.to_string())
}

/// Every label `list` (`added` or `removed`) holds, with the `side` (`after`
/// or `before`) of every changed value: what the diff says one version has
/// and the other does not.
fn named(diff: &Value, list: &str, side: &str) -> Labels {
    let items = |key: &str| diff[key].as_array().expect("a diff list").clone();
    let mut out: Labels = items(list)
        .iter()
        .map(|item| class_and_label(item, "o"))
        .chain(
            items("changed")
                .iter()
                .map(|item| class_and_label(item, side)),
        )
        .collect();
    out.sort();
    out
}

/// The changed values of `diff`, as sorted `(class, before, after)`.
fn changes(diff: &Value) -> Vec<(String, String, String)> {
    let mut out: Vec<_> = diff["changed"]
        .as_array()
        .expect("changed")
        .iter()
        .map(|item| {
            let (class, before) = class_and_label(item, "before");
            (class, before, class_and_label(item, "after").1)
        })
        .collect();
    out.sort();
    out
}

fn change(class: &str, before: &str, after: &str) -> (String, String, String) {
    (class.to_string(), before.to_string(), after.to_string())
}

/// `diff` names every label only `from` has once, as removed or as the old
/// side of a change, and every label only `to` has once, as added or as the
/// new side of a change; nothing else; and its summary counts its lists.
fn assert_every_difference_once(diff: &Value, from: &[(&str, &str)], to: &[(&str, &str)]) {
    assert_eq!(
        named(diff, "removed", "before"),
        only_in(from, to),
        "what only the older version has: {diff}"
    );
    assert_eq!(
        named(diff, "added", "after"),
        only_in(to, from),
        "what only the newer version has: {diff}"
    );
    let count = |key: &str| diff[key].as_array().expect("a diff list").len();
    assert_eq!(
        diff["summary"],
        json!({
            "added": count("added"),
            "removed": count("removed"),
            "changed": count("changed"),
        }),
        "{diff}"
    );
}

/// One value replaced by two: one change, and the other new value is added.
#[tokio::test]
async fn one_value_replaced_by_two_is_a_change_and_an_addition() {
    let from = [("Bridge", "a"), ("Tunnel", "same")];
    let to = [("Bridge", "b"), ("Bridge", "c"), ("Tunnel", "same")];
    let diff = diff_of(&from, &to).await;

    assert_every_difference_once(&diff, &from, &to);
    assert_eq!(changes(&diff), [change("Bridge", "a", "b")], "{diff}");
    assert_eq!(
        diff["summary"],
        json!({ "added": 1, "removed": 0, "changed": 1 }),
        "{diff}"
    );
}

/// Two values replaced by one: one change, and the other old value is
/// removed — not two changes to the same new value.
#[tokio::test]
async fn two_values_replaced_by_one_is_a_change_and_a_removal() {
    let from = [("Bridge", "a"), ("Bridge", "b"), ("Tunnel", "same")];
    let to = [("Bridge", "c"), ("Tunnel", "same")];
    let diff = diff_of(&from, &to).await;

    assert_every_difference_once(&diff, &from, &to);
    assert_eq!(changes(&diff), [change("Bridge", "a", "c")], "{diff}");
    assert_eq!(
        diff["summary"],
        json!({ "added": 0, "removed": 1, "changed": 1 }),
        "{diff}"
    );
}

/// Two values replaced by two: two changes, each old value with a new value
/// of its own, paired in the order of the values.
#[tokio::test]
async fn two_values_replaced_by_two_are_two_changes() {
    let from = [("Bridge", "a"), ("Bridge", "b"), ("Tunnel", "same")];
    let to = [("Bridge", "c"), ("Bridge", "d"), ("Tunnel", "same")];
    let diff = diff_of(&from, &to).await;

    assert_every_difference_once(&diff, &from, &to);
    assert_eq!(
        changes(&diff),
        [change("Bridge", "a", "c"), change("Bridge", "b", "d")],
        "{diff}"
    );
    assert_eq!(
        diff["summary"],
        json!({ "added": 0, "removed": 0, "changed": 2 }),
        "{diff}"
    );
}

/// Several subjects at once, each with its own mix of kept, replaced, added
/// and dropped values: the pairing of one subject's values takes none of
/// another's.
#[tokio::test]
async fn each_subject_pairs_its_own_values() {
    let from = [
        ("Bridge", "a"),
        ("Lock", "k1"),
        ("Lock", "k2"),
        ("Tunnel", "kept"),
        ("Tunnel", "t1"),
        ("Tunnel", "t2"),
        ("Weir", "dropped"),
    ];
    let to = [
        ("Bridge", "b"),
        ("Bridge", "c"),
        ("Culvert", "new"),
        ("Lock", "k3"),
        ("Tunnel", "kept"),
        ("Tunnel", "t3"),
        ("Tunnel", "t4"),
    ];
    let diff = diff_of(&from, &to).await;

    assert_every_difference_once(&diff, &from, &to);
    assert_eq!(
        changes(&diff),
        [
            change("Bridge", "a", "b"),
            change("Lock", "k1", "k3"),
            change("Tunnel", "t1", "t3"),
            change("Tunnel", "t2", "t4"),
        ],
        "{diff}"
    );
    assert_eq!(
        diff["summary"],
        json!({ "added": 2, "removed": 2, "changed": 4 }),
        "{diff}"
    );
}

/// A value dropped from one sub-graph and another added to a different one
/// are a removal and an addition, each in its own graph: values pair within
/// a graph only.
#[tokio::test]
async fn values_pair_within_their_own_graph() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    model(&state);
    for (ver, sub, text) in [("1.0", "ontology", "a"), ("1.1", "shapes", "b")] {
        version(
            &state,
            ver,
            VersionStatus::Published,
            &["ontology", "shapes"],
        );
        let graph = format!("{}/{sub}", version_graph(&state, ver));
        label(&state, &graph, "Bridge", text);
    }

    let diff = diff(&app, &token, "from=1.0&to=1.1").await;
    assert_eq!(
        diff["summary"],
        json!({ "added": 1, "removed": 1, "changed": 0 }),
        "{diff}"
    );
    assert_eq!(diff["removed"][0]["graph"], "ontology", "{diff}");
    assert_eq!(diff["removed"][0]["o"], "\"a\"", "{diff}");
    assert_eq!(diff["added"][0]["graph"], "shapes", "{diff}");
    assert_eq!(diff["added"][0]["o"], "\"b\"", "{diff}");
}
