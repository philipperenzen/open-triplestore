//! What a registry version holds (`facets`), through the real router.
//!
//! * an upload is counted from the scan it was given: an OWL model reports its
//!   classes and properties and its SKOS scheme reports its concepts, in the
//!   entry list and on the version;
//! * content that is no model at all is counted as what it is: a SKOS scheme
//!   alone has concepts and no classes, a profile of individuals has
//!   individuals and no classes or properties;
//! * a version a seed bundle registers is counted once its graphs are loaded;
//! * a version registered before the counts existed is counted by the
//!   start-up pass, and one whose content changed since is counted again when
//!   it is opened.

mod common;

use std::path::Path;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::{facets, registry as dmr};
use open_triplestore::seed_bundles::load_seed_dir;
use open_triplestore::server::AppState;
use oxigraph::io::RdfFormat;
use serde_json::Value;
use tower::ServiceExt as _;

const OWL_MODEL: &str = r#"
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:   <http://example.org/asset#> .

ex: a owl:Ontology .
ex:Asset a owl:Class . ex:Component a owl:Class . ex:Location a owl:Class .
ex:name a owl:DatatypeProperty ; rdfs:domain ex:Asset ; rdfs:range xsd:string .
ex:length a owl:DatatypeProperty ; rdfs:domain ex:Component ; rdfs:range xsd:decimal .
ex:partOf a owl:ObjectProperty ; rdfs:domain ex:Component ; rdfs:range ex:Asset .
ex:StatusList a skos:ConceptScheme ; skos:prefLabel "Status" .
ex:planned a skos:Concept ; skos:inScheme ex:StatusList ; skos:prefLabel "Planned" .
ex:inUse a skos:Concept ; skos:inScheme ex:StatusList ; skos:prefLabel "In use" .
"#;

/// A SKOS scheme and nothing else.
const SCHEME_ONLY: &str = r#"
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix ex:   <http://example.org/list#> .
ex:List a skos:ConceptScheme .
ex:a a skos:Concept ; skos:inScheme ex:List ; skos:prefLabel "A" .
ex:b a skos:Concept ; skos:inScheme ex:List ; skos:prefLabel "B" ; skos:broader ex:a .
ex:c a skos:Concept ; skos:inScheme ex:List ; skos:prefLabel "C" ; skos:broader ex:a .
"#;

/// Two dimensions and a metric, individuals of the quality vocabulary's
/// classes, which this graph does not declare.
const INDIVIDUALS_ONLY: &str = r#"
@prefix dqv:  <http://www.w3.org/ns/dqv#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex:   <http://example.org/quality#> .
ex:accuracy a dqv:Dimension ; rdfs:label "Accuracy" .
ex:timeliness a dqv:Dimension ; rdfs:label "Timeliness" .
ex:onTime a dqv:Metric ; rdfs:label "On time" ; dqv:inDimension ex:timeliness .
"#;

fn multipart_body(boundary: &str, version: &str, file: &str) -> Vec<u8> {
    format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"version\"\r\n\r\n{version}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"model.ttl\"\r\n\
         Content-Type: text/turtle\r\n\r\n{file}\r\n--{boundary}--\r\n"
    )
    .into_bytes()
}

fn entry(state: &AppState, id: &str) {
    dmr::insert_data_model(
        &state.store,
        &state.base_url,
        id,
        id,
        "http://example.org/ns#",
        None,
        false,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
}

async fn upload(state: &AppState, token: &str, id: &str, version: &str, file: &str) {
    let boundary = "FACETBOUNDARY";
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/models/{id}/versions"))
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(multipart_body(boundary, version, file)))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = body_text(resp.into_body()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

async fn get_json(state: &AppState, token: &str, uri: &str) -> Value {
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "{uri}");
    body_json(resp.into_body()).await
}

/// The `facets` of the list entry `id`.
async fn listed(state: &AppState, token: &str, id: &str) -> Value {
    let list = get_json(state, token, "/api/models").await;
    list.as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap_or_else(|| panic!("{id} is not listed: {list}"))["facets"]
        .clone()
}

#[tokio::test]
async fn an_upload_is_counted_in_the_list_and_on_the_version() {
    let (state, token) = admin_state();
    entry(&state, "asset");
    upload(&state, &token, "asset", "1.0.0", OWL_MODEL).await;

    let f = listed(&state, &token, "asset").await;
    assert_eq!(f["classes"], 3, "{f}");
    assert_eq!(f["properties"], 3, "{f}");
    assert_eq!(f["concepts"], 2, "{f}");
    assert_eq!(f["concept_schemes"], 1, "{f}");
    assert_eq!(f["individuals"], 0, "{f}");
    assert_eq!(f["capped"], false);
    assert!(f["triples"].as_u64().unwrap() > 10, "{f}");

    let v = get_json(&state, &token, "/api/models/asset/versions/1.0.0").await;
    assert_eq!(v["facets"], f, "the version and the list agree");
    let versions = get_json(&state, &token, "/api/models/asset/versions").await;
    assert_eq!(versions[0]["facets"], f);
    let one = get_json(&state, &token, "/api/models/asset").await;
    assert_eq!(one["facets"], f);
}

#[tokio::test]
async fn a_scheme_and_a_profile_of_individuals_are_counted_as_what_they_are() {
    let (state, token) = admin_state();
    entry(&state, "list");
    entry(&state, "quality");
    upload(&state, &token, "list", "1.0.0", SCHEME_ONLY).await;
    upload(&state, &token, "quality", "1.0.0", INDIVIDUALS_ONLY).await;

    let scheme = listed(&state, &token, "list").await;
    assert_eq!(scheme["concepts"], 3, "{scheme}");
    assert_eq!(scheme["concept_schemes"], 1, "{scheme}");
    assert_eq!(scheme["classes"], 0, "{scheme}");
    assert_eq!(scheme["individuals"], 0, "{scheme}");

    let quality = listed(&state, &token, "quality").await;
    assert_eq!(quality["individuals"], 3, "{quality}");
    assert_eq!(quality["classes"], 0, "{quality}");
    assert_eq!(quality["properties"], 0, "{quality}");
    assert_eq!(quality["concepts"], 0, "{quality}");
}

/// A bundle with one model of two graphs: a scheme and a profile of
/// individuals.
fn write_bundle(dir: &Path) {
    let bundle = dir.join("lists");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(
        bundle.join("manifest.toml"),
        r#"id = "lists"

[organisation]
slug = "lists"
name = "Lists"

[[data_models]]
id = "lists"
title = "Lists"
namespace = "http://example.org/list#"
kind = "vocabulary"
version = "1.0.0"

[[data_models.graphs]]
iri = "https://example.org/lists/scheme"
file = "scheme.ttl"

[[data_models.graphs]]
iri = "https://example.org/lists/profile"
file = "profile.ttl"
"#,
    )
    .unwrap();
    std::fs::write(bundle.join("scheme.ttl"), SCHEME_ONLY).unwrap();
    std::fs::write(bundle.join("profile.ttl"), INDIVIDUALS_ONLY).unwrap();
}

#[tokio::test]
async fn a_seeded_version_is_counted_over_all_its_graphs() {
    let (state, token) = admin_state();
    let dir = tempfile::tempdir().unwrap();
    write_bundle(dir.path());
    load_seed_dir(&state, dir.path());

    let f = listed(&state, &token, "lists").await;
    assert_eq!(f["concepts"], 3, "{f}");
    assert_eq!(f["individuals"], 3, "{f}");
    assert_eq!(f["classes"], 0, "{f}");
}

const OLD_GRAPH: &str = "https://example.org/old/content";

/// A published version recorded the way an earlier build did: content in its
/// graph, no facets.
fn old_version(state: &AppState) {
    entry(state, "old");
    state
        .store
        .load_str(SCHEME_ONLY, RdfFormat::Turtle, Some(OLD_GRAPH))
        .unwrap();
    dmr::insert_version(
        &state.store,
        &state.base_url,
        &DataModelVersion {
            data_model_id: "old".into(),
            version: "1.0.0".into(),
            status: VersionStatus::Published,
            graph_iri: OLD_GRAPH.into(),
            sub_graphs: vec![],
            created_at: "2026-01-01T00:00:00Z".into(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: vec![],
        },
    )
    .unwrap();
    dmr::update_latest_published(&state.store, &state.base_url, "old", "1.0.0").unwrap();
}

#[tokio::test]
async fn the_start_up_pass_counts_a_version_that_has_no_counts_yet() {
    let (state, token) = admin_state();
    old_version(&state);
    assert!(listed(&state, &token, "old").await.is_null());

    let counted = tokio::task::spawn_blocking({
        let state = state.clone();
        move || facets::backfill(&state)
    })
    .await
    .unwrap();
    assert_eq!(counted, 1);
    let f = listed(&state, &token, "old").await;
    assert_eq!(f["concepts"], 3, "{f}");

    // Nothing left to count.
    let again = tokio::task::spawn_blocking({
        let state = state.clone();
        move || facets::backfill(&state)
    })
    .await
    .unwrap();
    assert_eq!(again, 0);
}

#[tokio::test]
async fn opening_a_version_counts_it_again_when_its_content_changed() {
    let (state, token) = admin_state();
    old_version(&state);

    // Opened before any pass has run: counted on the spot.
    let v = get_json(&state, &token, "/api/models/old/versions/1.0.0").await;
    assert_eq!(v["facets"]["concepts"], 3, "{v}");
    assert_eq!(v["facets"]["individuals"], 0, "{v}");

    // Content added since: the stored counts no longer fit the version, so the
    // entry's own page does not serve them (the list serves what is stored,
    // without looking at the content); opening the version counts it again.
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{OLD_GRAPH}> {{ \
             <http://example.org/list#x> a <http://example.org/list#Thing> . \
             <http://example.org/list#y> a <http://example.org/list#Thing> }} }}"
        ))
        .unwrap();
    assert_eq!(listed(&state, &token, "old").await["individuals"], 0);
    let entry = get_json(&state, &token, "/api/models/old").await;
    assert!(entry["facets"].is_null(), "{entry}");
    let v = get_json(&state, &token, "/api/models/old/versions/1.0.0").await;
    assert_eq!(v["facets"]["individuals"], 2, "{v}");
    assert_eq!(listed(&state, &token, "old").await["individuals"], 2);
    assert_eq!(
        get_json(&state, &token, "/api/models/old").await["facets"]["individuals"],
        2
    );
}

#[test]
fn content_too_large_to_scan_is_reported_as_capped() {
    let store = open_triplestore::store::TripleStore::in_memory().unwrap();
    store
        .load_str(SCHEME_ONLY, RdfFormat::Turtle, Some(OLD_GRAPH))
        .unwrap();
    let graphs = vec![OLD_GRAPH.to_string()];
    let capped = facets::compute(&store, &graphs, 3).unwrap();
    assert!(capped.capped);
    assert!(capped.triples > 3);
    assert_eq!(
        capped,
        facets::ContentFacets {
            triples: capped.triples,
            capped: true,
            ..Default::default()
        },
        "a capped record has no counts"
    );
    let counted = facets::compute(&store, &graphs, 1000).unwrap();
    assert!(!counted.capped);
    assert_eq!(counted.concepts, 3);
}
