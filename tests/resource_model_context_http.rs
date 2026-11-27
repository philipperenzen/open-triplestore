//! A resource page for a term of a model, and for an instance of one, through
//! the real router:
//!
//! * `/api/browse/resource?graph=<version base graph>` reads the version's
//!   sub-graphs when the version keeps its content there. Scoped to the base
//!   graph alone it answered with no statement, no label and nothing to
//!   follow, which is what the model viewer's "Open" link produced for every
//!   model stored as several graphs;
//! * `/api/browse/resource/context` says where a resource belongs: the model
//!   that defines it, the classes it is an instance of with their models, the
//!   dataset holding it and the model that dataset conforms to, and where a
//!   class's instances are — from graphs the caller may read only.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::registry as dmr;
use open_triplestore::server::AppState;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const EX: &str = "http://ex.org/def#";
const ID: &str = "http://ex.org/id/";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";

async fn get(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(token) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

fn insert(state: &AppState, graph: &str, triples: &str) {
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{graph}> {{ {triples} }} }}"
        ))
        .unwrap();
}

/// The public model `bridges`, owned by the admin of [`admin_state`].
fn model(state: &AppState) {
    dmr::insert_data_model(
        &state.store,
        &state.base_url,
        "bridges",
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
    dmr::version_record_iri(&state.base_url, "bridges", ver)
}

/// Publish version `ver` with the named sub-graphs; `ex:Bridge` is declared
/// and labelled in the `classes` sub-graph (or in the base graph when the
/// version has no sub-graphs), and `ex:crosses` points at it from there.
fn publish(state: &AppState, ver: &str, subs: &[&str]) -> String {
    let graph_iri = version_graph(state, ver);
    dmr::insert_version(
        &state.store,
        &state.base_url,
        &DataModelVersion {
            data_model_id: "bridges".to_string(),
            version: ver.to_string(),
            status: VersionStatus::Published,
            sub_graphs: subs.iter().map(|s| format!("{graph_iri}/{s}")).collect(),
            graph_iri: graph_iri.clone(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: vec![],
        },
    )
    .unwrap();
    dmr::update_latest_published(&state.store, &state.base_url, "bridges", ver).unwrap();
    let classes = if subs.is_empty() {
        graph_iri.clone()
    } else {
        format!("{graph_iri}/classes")
    };
    insert(
        state,
        &classes,
        &format!(
            "<{EX}Bridge> <{RDF_TYPE}> <{OWL_CLASS}> ; <{RDFS_LABEL}> \"Bridge\" . \
             <{EX}crosses> <http://www.w3.org/2000/01/rdf-schema#domain> <{EX}Bridge> ."
        ),
    );
    graph_iri
}

/// The dataset `inspections` with one graph holding `id:b1 a ex:Bridge`,
/// declared to conform to `bridges` 2.0.
fn dataset(state: &AppState, visibility: Visibility) {
    state
        .auth_db
        .create_dataset(
            "inspections",
            "Inspections",
            None,
            OwnerType::User,
            "adm",
            visibility,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("inspections", "http://ex.org/graph/inspections")
        .unwrap();
    state
        .auth_db
        .update_dataset_conformance("inspections", Some("bridges"), Some("2.0"))
        .unwrap();
    insert(
        state,
        "http://ex.org/graph/inspections",
        &format!("<{ID}b1> <{RDF_TYPE}> <{EX}Bridge> ; <{RDFS_LABEL}> \"Old bridge\" ."),
    );
    state.auth_db.invalidate_accessible_graphs_cache();
}

fn resource_uri(iri: &str, graph: Option<&str>) -> String {
    let mut uri = format!("/api/browse/resource?iri={}", url_encode(iri));
    if let Some(g) = graph {
        uri.push_str(&format!("&graph={}", url_encode(g)));
    }
    uri
}

fn context_uri(iri: &str, graph: Option<&str>) -> String {
    let mut uri = format!("/api/browse/resource/context?iri={}", url_encode(iri));
    if let Some(g) = graph {
        uri.push_str(&format!("&graph={}", url_encode(g)));
    }
    uri
}

fn predicates(rows: &Value) -> Vec<String> {
    let mut out: Vec<String> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["p"]["value"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

/// The base graph of a version kept in sub-graphs holds nothing. A resource
/// asked for in that graph is read from the version's sub-graphs.
#[tokio::test]
async fn a_version_base_graph_reads_the_versions_sub_graphs() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    model(&state);
    let base = publish(&state, "2.0", &["classes", "shapes"]);

    let (status, body) = get(
        &app,
        &resource_uri(&format!("{EX}Bridge"), Some(&base)),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        predicates(&body["outgoing"]),
        [RDF_TYPE, RDFS_LABEL],
        "{body}"
    );
    assert_eq!(body["label"]["value"], json!("Bridge"), "{body}");
    // What points at it is found as well: something to follow from the page.
    assert_eq!(body["incoming"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(
        body["incoming"][0]["s"]["value"],
        json!(format!("{EX}crosses"))
    );
}

/// A graph that is not the base graph of a version kept in sub-graphs stands
/// for itself: a sub-graph, and the base graph of a single-graph version.
#[tokio::test]
async fn any_other_graph_is_read_as_itself() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    model(&state);
    let layered = publish(&state, "2.0", &["classes", "shapes"]);
    let single = publish(&state, "1.0", &[]);

    // The shapes sub-graph says nothing about the class.
    let (_, in_shapes) = get(
        &app,
        &resource_uri(&format!("{EX}Bridge"), Some(&format!("{layered}/shapes"))),
        Some(&token),
    )
    .await;
    assert_eq!(in_shapes["outgoing"], json!([]), "{in_shapes}");

    for graph in [format!("{layered}/classes"), single] {
        let (status, body) = get(
            &app,
            &resource_uri(&format!("{EX}Bridge"), Some(&graph)),
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{graph}: {body}");
        assert_eq!(
            body["outgoing"].as_array().unwrap().len(),
            2,
            "{graph}: {body}"
        );
    }
}

/// The registry lookup behind it: sub-graphs only for the base graph of a
/// version that lists them.
#[test]
fn only_a_layered_versions_base_graph_names_its_content_graphs() {
    let (state, _) = admin_state();
    model(&state);
    let layered = publish(&state, "2.0", &["classes", "shapes"]);
    let single = publish(&state, "1.0", &[]);

    assert_eq!(
        dmr::version_content_graphs(&state.store, &layered),
        Some(vec![
            layered.clone(),
            format!("{layered}/classes"),
            format!("{layered}/shapes"),
        ])
    );
    assert_eq!(dmr::version_content_graphs(&state.store, &single), None);
    assert_eq!(
        dmr::version_content_graphs(&state.store, &format!("{layered}/classes")),
        None
    );
    assert_eq!(
        dmr::version_content_graphs(&state.store, "http://ex.org/graph/inspections"),
        None
    );
    assert_eq!(
        dmr::version_content_graphs(&state.store, "not an iri"),
        None
    );
}

/// An instance: the class it is typed with and the model that defines the
/// class, the dataset it is in and the model that dataset conforms to.
#[tokio::test]
async fn an_instance_names_its_class_model_and_dataset() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    model(&state);
    publish(&state, "1.0", &[]);
    publish(&state, "2.0", &["classes", "shapes"]);
    dataset(&state, Visibility::Public);

    let (status, ctx) = get(&app, &context_uri(&format!("{ID}b1"), None), Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{ctx}");
    // An instance is defined in no model.
    assert_eq!(ctx["defined_in"], json!([]), "{ctx}");

    let classes = ctx["instance_of"].as_array().unwrap();
    assert_eq!(classes.len(), 1, "{ctx}");
    assert_eq!(classes[0]["iri"], json!(format!("{EX}Bridge")));
    // Both versions define the class: the latest published one is named.
    assert_eq!(
        classes[0]["defined_in"],
        json!([{
            "model_id": "bridges",
            "title": "Bridges",
            "kind": null,
            "version": "2.0",
            "status": "published",
            "other_versions": 1,
        }]),
        "{ctx}"
    );

    assert_eq!(
        ctx["datasets"],
        json!([{
            "id": "inspections",
            "name": "Inspections",
            "conforms_to": { "model_id": "bridges", "version": "2.0", "title": "Bridges" },
        }]),
        "{ctx}"
    );
    // Nothing is typed with an instance.
    assert_eq!(ctx["instances"]["total"], json!(0), "{ctx}");
}

/// A class: the model that defines it, the datasets that conform to that
/// model, and where its instances are.
#[tokio::test]
async fn a_class_names_its_model_and_where_its_instances_are() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    model(&state);
    let old = publish(&state, "1.0", &[]);
    publish(&state, "2.0", &["classes", "shapes"]);
    dataset(&state, Visibility::Public);

    let (status, ctx) = get(
        &app,
        &context_uri(&format!("{EX}Bridge"), None),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ctx}");
    assert_eq!(ctx["defined_in"][0]["model_id"], json!("bridges"), "{ctx}");
    assert_eq!(ctx["defined_in"][0]["version"], json!("2.0"), "{ctx}");
    // `owl:Class` is the modelling language, not a class the term is an instance of.
    assert_eq!(ctx["instance_of"], json!([]), "{ctx}");
    assert_eq!(
        ctx["conforming_datasets"],
        json!([{ "id": "inspections", "name": "Inspections", "version": "2.0" }]),
        "{ctx}"
    );
    assert_eq!(ctx["instances"]["total"], json!(1), "{ctx}");
    assert_eq!(
        ctx["instances"]["sources"][0]["dataset"]["id"],
        json!("inspections"),
        "{ctx}"
    );
    assert_eq!(ctx["instances"]["sources"][0]["count"], json!(1), "{ctx}");

    // Scoped to an older version's graph, that version is the one named.
    let (_, scoped) = get(
        &app,
        &context_uri(&format!("{EX}Bridge"), Some(&old)),
        Some(&token),
    )
    .await;
    assert_eq!(scoped["defined_in"][0]["version"], json!("1.0"), "{scoped}");
    assert_eq!(
        scoped["defined_in"][0]["other_versions"],
        json!(1),
        "{scoped}"
    );
}

/// A private dataset is not named, nor counted, for a caller who may not open
/// it; the public model still is.
#[tokio::test]
async fn a_dataset_the_caller_may_not_open_is_left_out() {
    let (state, _) = admin_state();
    let app = test_app(state.clone());
    model(&state);
    publish(&state, "2.0", &["classes", "shapes"]);
    dataset(&state, Visibility::Private);

    let (status, ctx) = get(&app, &context_uri(&format!("{EX}Bridge"), None), None).await;
    assert_eq!(status, StatusCode::OK, "{ctx}");
    assert_eq!(ctx["defined_in"][0]["model_id"], json!("bridges"), "{ctx}");
    assert_eq!(ctx["conforming_datasets"], json!([]), "{ctx}");
    assert_eq!(ctx["instances"]["total"], json!(0), "{ctx}");
    assert_eq!(ctx["instances"]["sources"], json!([]), "{ctx}");

    // The instance itself: nothing about it is in a graph this caller may read.
    let (_, instance) = get(&app, &context_uri(&format!("{ID}b1"), None), None).await;
    assert_eq!(instance["instance_of"], json!([]), "{instance}");
    assert_eq!(instance["datasets"], json!([]), "{instance}");
}

/// A blank node, and an IRI that could not be written into a query.
#[tokio::test]
async fn names_that_cannot_be_looked_up() {
    let (state, token) = admin_state();
    let app = test_app(state);

    let (status, ctx) = get(&app, &context_uri("_:b0", None), Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{ctx}");
    assert_eq!(ctx, json!({ "iri": "_:b0" }));

    let (status, _) = get(
        &app,
        &context_uri("http://ex.org/a> } INSERT { <x", None),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
