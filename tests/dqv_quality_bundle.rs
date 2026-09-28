//! The W3C DQV quality profile and its well-formedness shapes
//! (examples/seed-bundles/dqv-quality). DQV is a Working Group Note that
//! declares no cardinality at all and ships one dimension, zero categories
//! and zero metrics — so a usable deployment needs a profile naming them and
//! shapes saying what a well-formed measurement is. The bundle is both, with
//! a sample that plants one violation per shape.
//!
//! Everything is vendored (DQV itself is already the seeded `dqv`
//! vocabulary), so unlike the NEN bundles this test never skips.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::seed_bundles::load_seed_dir;
use open_triplestore::shacl::report::Severity;
use open_triplestore::shacl::validate;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use serde_json::Value;
use tower::ServiceExt as _;

const BUNDLE: &str = "examples/seed-bundles/dqv-quality";
const PROFILE: &str = "https://example.org/dqv-quality/profile";
const SHAPES: &str = "https://example.org/dqv-quality/shapes";
const DATA: &str = "https://example.org/dqv-quality/instances";
const EX: &str = "https://example.org/dqv-quality/";
const DEF: &str = "https://example.org/dqv-quality/def#";

fn bundle_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(BUNDLE)
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Profile + shapes + sample, each in its own graph, in a fresh store.
fn sample_store() -> TripleStore {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            &bundle_file("quality-profile.ttl"),
            RdfFormat::Turtle,
            Some(PROFILE),
        )
        .unwrap();
    store
        .load_str(&bundle_file("shapes.ttl"), RdfFormat::Turtle, Some(SHAPES))
        .unwrap();
    store
        .load_str(&bundle_file("instances.ttl"), RdfFormat::Turtle, Some(DATA))
        .unwrap();
    store
}

/// `(shape or path local name, focus node local name)` → count. A result
/// from a node shape is keyed by the shape; one from a blank-node property
/// shape (whose `source_shape` is the blank node) by its `sh:path`, which is
/// the more precise statement of which constraint fired anyway.
fn results_by_shape_and_focus(
    report: &open_triplestore::shacl::report::ValidationReport,
) -> BTreeMap<(String, String), usize> {
    let mut out = BTreeMap::new();
    for r in &report.results {
        let local = |iri: &str| {
            iri.trim_matches(['<', '>'])
                .rsplit(['#', '/'])
                .next()
                .unwrap_or("")
                .to_string()
        };
        let shape = if r.source_shape.starts_with("_:") {
            r.path.as_deref().map(local).unwrap_or_default()
        } else {
            local(&r.source_shape)
        };
        let focus = r
            .focus_node
            .strip_prefix(DEF)
            .or_else(|| r.focus_node.strip_prefix(EX))
            .unwrap_or(&r.focus_node)
            .to_string();
        *out.entry((shape, focus)).or_insert(0) += 1;
    }
    out
}

/// The validate route hands the engine the dataset's graphs plus the graphs
/// of the model it conforms to; the store-level test does the same, because
/// the profile is where the metrics are typed and `sh:class dqv:Metric`
/// reads that type.
#[test]
fn dqv_shapes_find_the_seven_planted_violations() {
    let store = sample_store();
    let report = validate(&store, SHAPES, &[PROFILE.to_string(), DATA.to_string()]).unwrap();
    let got = results_by_shape_and_focus(&report);
    let key = |shape: &str, focus: &str| (shape.to_string(), focus.to_string());

    assert!(!report.conforms, "the planted violations are found");
    // 1. A measurement with no metric (QualityMeasurementShape, sh:minCount
    //    on dqv:isMeasurementOf).
    assert_eq!(
        got.get(&key("isMeasurementOf", "m-no-metric")),
        Some(&1),
        "{got:?}"
    );
    // 2. A measurement that does not say what it was computed on
    //    (QualityMeasurementShape, sh:minCount on dqv:computedOn).
    assert_eq!(
        got.get(&key("computedOn", "m-no-computed-on")),
        Some(&1),
        "{got:?}"
    );
    // 3. A metric in no dimension (MetricShape, dqv:inDimension).
    assert_eq!(
        got.get(&key("inDimension", "undimensionedMetric")),
        Some(&1),
        "{got:?}"
    );
    // 4. A dimension in no category (DimensionShape, dqv:inCategory).
    assert_eq!(
        got.get(&key("inCategory", "orphanDimension")),
        Some(&1),
        "{got:?}"
    );
    // 5. A well-formed measurement whose value datatype contradicts its
    //    metric's dqv:expectedDataType — SHACL-SPARQL, because core cannot
    //    compare a literal's datatype with an IRI on another node.
    assert_eq!(
        got.get(&key("MeasurementDatatypeShape", "m-wrong-datatype")),
        Some(&1),
        "{got:?}"
    );
    // 6. hasQualityMeasurement and computedOn are inverses that disagree.
    assert_eq!(
        got.get(&key("MeasurementInverseShape", "distribution-1")),
        Some(&1),
        "{got:?}"
    );
    // 7. Computed on something that is neither a dataset nor a distribution:
    //    DQV says "generally expected", so a warning, not a violation.
    assert_eq!(
        got.get(&key("QualityTargetShape", "sparql-service")),
        Some(&1),
        "{got:?}"
    );
    let target_result = report
        .results
        .iter()
        .find(|r| r.source_shape.ends_with("QualityTargetShape"))
        .unwrap();
    assert!(
        matches!(target_result.severity, Severity::Warning),
        "the target shape warns: {target_result:?}"
    );

    // The conforming core produces nothing: the dataset, its distribution,
    // the three measurements on it, and the profile's own metrics,
    // dimensions and categories.
    for focus in [
        "dataset-1",
        "m-conformance",
        "m-violations",
        "m-triples",
        "m-detached",
        "m-on-service",
        "shaclConformanceMetric",
        "shaclViolationCountMetric",
        "tripleCountMetric",
        "endpointAvailabilityMetric",
        "consistency",
        "completeness",
        "availability",
        "intrinsic",
    ] {
        assert!(
            !got.keys().any(|(_, f)| f == focus),
            "{focus} conforms: {got:?}"
        );
    }
    assert_eq!(
        report.results.len(),
        7,
        "exactly the planted violations: {got:?}"
    );
}

/// The profile is an example of its own rules: validated alone against the
/// shapes it ships with, it conforms. This is what keeps the profile honest
/// when a metric or dimension is added later.
#[test]
fn the_profile_conforms_to_its_own_shapes() {
    let store = sample_store();
    let report = validate(&store, SHAPES, &[PROFILE.to_string()]).unwrap();
    assert!(
        report.conforms,
        "the profile conforms to the DQV shapes: {:?}",
        report.results
    );
}

/// The conforming core alone — the sample without its planted violations —
/// also conforms, so every result in the full run is a planted one.
#[test]
fn the_conforming_core_alone_conforms() {
    let store = sample_store();
    // Drop every planted node from the sample graph and validate what is left.
    let planted = [
        "m-no-metric",
        "m-no-computed-on",
        "m-wrong-datatype",
        "m-detached",
        "m-on-service",
        "sparql-service",
    ];
    for local in planted {
        store
            .update(&format!(
                "DELETE WHERE {{ GRAPH <{DATA}> {{ <{EX}{local}> ?p ?o }} }} ; \
                 DELETE WHERE {{ GRAPH <{DATA}> {{ ?s ?p <{EX}{local}> }} }}"
            ))
            .unwrap();
    }
    for local in ["undimensionedMetric", "orphanDimension"] {
        store
            .update(&format!(
                "DELETE WHERE {{ GRAPH <{DATA}> {{ <{DEF}{local}> ?p ?o }} }}"
            ))
            .unwrap();
    }
    let report = validate(&store, SHAPES, &[PROFILE.to_string(), DATA.to_string()]).unwrap();
    assert!(
        report.conforms,
        "the conforming core conforms: {:?}",
        report.results
    );
}

async fn req(app: &Router, method: Method, uri: &str, token: &str) -> (StatusCode, Value, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    )
}

/// The bundle end to end — profile registered, dataset conforming to it,
/// shapes bound, validation through the real API.
#[tokio::test]
async fn bundle_loads_and_validates_through_the_api() {
    let bundles = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/seed-bundles");
    let (state, token) = admin_state();
    load_seed_dir(&state, &bundles);
    let app = test_app(state.clone());

    let (st, layer, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/dqv-quality-sample/conformance",
        &token,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(
        layer["conforms_to_model"]["id"], "dqv-quality-profile",
        "{txt}"
    );
    assert_eq!(layer["conforms_to_model"]["version"], "1.0.0", "{txt}");
    let shapes: Vec<&str> = layer["validation_shapes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(shapes, vec![SHAPES], "{txt}");

    let (st, vreport, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/dqv-quality-sample/validate",
        &token,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let report = if vreport["report"].is_object() {
        &vreport["report"]
    } else {
        &vreport
    };
    assert_eq!(report["conforms"], false, "{txt}");
    for planted in [
        "m-no-metric",
        "m-no-computed-on",
        "undimensionedMetric",
        "orphanDimension",
        "m-wrong-datatype",
        "distribution-1",
        "sparql-service",
    ] {
        assert!(txt.contains(planted), "the report names {planted}: {txt}");
    }
    let focus_nodes: Vec<&str> = report["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["focus_node"].as_str())
        .collect();
    for conforming in [
        "/dataset-1",
        "/m-conformance",
        "/m-violations",
        "/m-triples",
    ] {
        assert!(
            !focus_nodes.iter().any(|f| f.ends_with(conforming)),
            "{conforming} conforms: {focus_nodes:?}"
        );
    }
}
