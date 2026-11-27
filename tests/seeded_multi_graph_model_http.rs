//! A model a seed bundle registers from several graphs, through the real
//! router:
//!
//! * the version holds every one of its graphs: the download serves them all,
//!   the content revision (`ETag`) covers them all, a graph can be asked for by
//!   its IRI, and deleting the entry drops them all — the first graph of the
//!   manifest included, which the registry used to record as the version's
//!   base graph and leave out of its sub-graphs, so a version of three graphs
//!   served, hashed and deleted two;
//! * a version an earlier build recorded that way is repaired by the next
//!   start (the seeder runs again), and nothing else is touched;
//! * a model of one graph is recorded as before.

mod common;

use std::path::Path;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::data_models::{content_digest, registry};
use open_triplestore::seed_bundles::load_seed_dir;
use open_triplestore::server::AppState;
use oxigraph::model::{GraphNameRef, NamedNodeRef};
use tower::ServiceExt as _;

const NAMES: &str = "https://example.org/parts/names";
const CONCEPTS: &str = "https://example.org/parts/concepts";
const RULES: &str = "https://example.org/parts/rules";
const ALPHA: &str = "https://example.org/parts/names#Alpha";
const CONCEPT: &str = "https://example.org/parts/concepts#Bolt";
const RULE: &str = "https://example.org/parts/rules#Torque";

/// A bundle with one model of three graphs (and no dataset), written to `dir`.
fn write_bundle(dir: &Path) {
    let bundle = dir.join("parts");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(
        bundle.join("manifest.toml"),
        format!(
            r#"id = "parts"

[organisation]
slug = "parts"
name = "Parts"

[[data_models]]
id = "parts"
title = "Parts"
namespace = "https://example.org/parts/"
version = "1.0.0"

[[data_models.graphs]]
iri = "{NAMES}"
file = "names.ttl"

[[data_models.graphs]]
iri = "{CONCEPTS}"
file = "concepts.ttl"

[[data_models.graphs]]
iri = "{RULES}"
file = "rules.ttl"
"#
        ),
    )
    .unwrap();
    std::fs::write(
        bundle.join("names.ttl"),
        format!("<{ALPHA}> a <http://www.w3.org/2002/07/owl#Class> .\n"),
    )
    .unwrap();
    std::fs::write(
        bundle.join("concepts.ttl"),
        format!("<{CONCEPT}> a <http://www.w3.org/2004/02/skos/core#Concept> .\n"),
    )
    .unwrap();
    std::fs::write(
        bundle.join("rules.ttl"),
        format!("<{RULE}> <http://www.w3.org/2000/01/rdf-schema#label> \"Torque\" .\n"),
    )
    .unwrap();
}

/// The state of a store that has started once with the bundle.
fn seeded(dir: &Path) -> (AppState, String) {
    let (state, token) = admin_state();
    write_bundle(dir);
    load_seed_dir(&state, dir);
    (state, token)
}

fn request(method: Method, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

/// `(status, ETag, body)` of the version's download.
async fn download(state: &AppState, token: &str, query: &str) -> (StatusCode, String, String) {
    let resp = test_app(state.clone())
        .oneshot(request(
            Method::GET,
            &format!("/api/models/parts/versions/1.0.0/data{query}"),
            token,
        ))
        .await
        .unwrap();
    let status = resp.status();
    let etag = resp
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, etag, body_text(resp.into_body()).await)
}

fn triples_in(state: &AppState, graph: &str) -> usize {
    state
        .store
        .quads_for_graph(GraphNameRef::NamedNode(NamedNodeRef::new(graph).unwrap()))
        .unwrap()
        .len()
}

fn version(state: &AppState) -> open_triplestore::data_models::models::DataModelVersion {
    registry::get_version(&state.store, &state.base_url, "parts", "1.0.0").unwrap()
}

/// Put the record back the way an earlier build wrote it: the first graph is
/// the base graph and appears nowhere among the sub-graphs.
fn record_the_old_way(state: &AppState) {
    let ver = registry::version_record_iri(&state.base_url, "parts", "1.0.0");
    state
        .store
        .update(&format!(
            "DELETE DATA {{ GRAPH <{}> {{ <{ver}> <urn:system:vocab/subGraph> <{NAMES}> }} }}",
            registry::REGISTRY_GRAPH
        ))
        .unwrap();
    assert_eq!(
        sorted(version(state).sub_graphs),
        [CONCEPTS, RULES],
        "the record is in the old layout"
    );
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[tokio::test]
async fn a_seeded_version_of_several_graphs_serves_hashes_and_deletes_all_of_them() {
    let dir = tempfile::tempdir().unwrap();
    let (state, token) = seeded(dir.path());

    // The download serves every graph.
    let (status, etag, body) = download(&state, &token, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for term in [ALPHA, CONCEPT, RULE] {
        assert!(body.contains(term), "{term} is missing from the download");
    }

    // The first graph can be asked for by its IRI, like any other.
    let (status, _, body) =
        download(&state, &token, &format!("?graph={}", url_encode(NAMES))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains(ALPHA), "{body}");
    assert!(!body.contains(CONCEPT), "only the graph asked for: {body}");

    // The revision covers the first graph: a change there changes the ETag.
    assert!(!etag.is_empty());
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{NAMES}> {{ <{ALPHA}> <http://www.w3.org/2000/01/rdf-schema#label> \"Alpha\" }} }}"
        ))
        .unwrap();
    let (_, changed, _) = download(&state, &token, "").await;
    assert_ne!(
        etag, changed,
        "an edit of the first graph left the ETag as it was"
    );

    // The record lists all three graphs, the first (the base graph) too, and
    // the wider set a guard reads does not count it twice.
    let v = version(&state);
    assert_eq!(v.graph_iri, NAMES);
    assert_eq!(sorted(v.sub_graphs.clone()), [CONCEPTS, NAMES, RULES]);
    assert_eq!(
        sorted(v.content_graphs()),
        [CONCEPTS, NAMES, RULES],
        "the content graphs are the sub-graphs"
    );
    assert_eq!(
        content_digest::version_graphs(&v.graph_iri, &v.sub_graphs).len(),
        3
    );

    // Deleting the entry drops all three graphs.
    let resp = test_app(state.clone())
        .oneshot(request(Method::DELETE, "/api/models/parts", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    for graph in [NAMES, CONCEPTS, RULES] {
        assert_eq!(triples_in(&state, graph), 0, "{graph} outlived its model");
    }
}

#[tokio::test]
async fn a_version_recorded_the_old_way_is_repaired_by_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let (state, token) = seeded(dir.path());
    record_the_old_way(&state);

    // As it was: the first graph is not served.
    let (_, _, body) = download(&state, &token, "").await;
    assert!(
        !body.contains(ALPHA),
        "the old layout loses the first graph"
    );

    // The next start repairs the record, not the data.
    let before: Vec<usize> = [NAMES, CONCEPTS, RULES]
        .map(|g| triples_in(&state, g))
        .into();
    load_seed_dir(&state, dir.path());
    assert_eq!(sorted(version(&state).sub_graphs), [CONCEPTS, NAMES, RULES]);
    let after: Vec<usize> = [NAMES, CONCEPTS, RULES]
        .map(|g| triples_in(&state, g))
        .into();
    assert_eq!(before, after);

    let (_, _, body) = download(&state, &token, "").await;
    for term in [ALPHA, CONCEPT, RULE] {
        assert!(body.contains(term), "{term} is missing after the repair");
    }

    // And once more: nothing to repair, nothing changes.
    load_seed_dir(&state, dir.path());
    assert_eq!(sorted(version(&state).sub_graphs), [CONCEPTS, NAMES, RULES]);
}

#[tokio::test]
async fn a_version_that_is_not_the_bundles_is_left_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = seeded(dir.path());
    record_the_old_way(&state);

    // The record is somebody else's now: it names a creator and no seed
    // marker, as a version a user registered under the same id would.
    let ver = registry::version_record_iri(&state.base_url, "parts", "1.0.0");
    state
        .store
        .update(&format!(
            "DELETE WHERE {{ GRAPH <{reg}> {{ <{ver}> <urn:system:vocab/seededBy> ?m }} }};\n\
             INSERT DATA {{ GRAPH <{reg}> {{ <{ver}> <http://purl.org/dc/terms/creator> <{}/users/someone> }} }}",
            state.base_url,
            reg = registry::REGISTRY_GRAPH,
        ))
        .unwrap();

    load_seed_dir(&state, dir.path());
    assert_eq!(
        sorted(version(&state).sub_graphs),
        [CONCEPTS, RULES],
        "a record the bundle did not register is not repaired"
    );
}
