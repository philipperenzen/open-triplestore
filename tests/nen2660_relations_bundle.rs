//! The NEN 2660-2 relation profile and its consistency shapes
//! (examples/seed-bundles/nen2660-relations): part-whole, containment and
//! connection relations with the characteristics OWL can carry (transitivity
//! on proper parthood only) and the ones it cannot (acyclicity,
//! irreflexivity, spatial consistency) as SHACL-SPARQL shapes over the
//! GeoSPARQL functions — Keet et al., OntoPartS.
//!
//! The profile, shapes and sample are vendored and run on every CI run. The
//! NEN 2660-2 RDFS file the *bundle* also ships is not (run the bundle's
//! fetch.sh once); without it the bundle test reports that it skipped and
//! passes, like tests/nen2660_imbor_bundle.rs.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::seed_bundles::load_seed_dir;
use open_triplestore::shacl::validate;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::Value;
use tower::ServiceExt as _;

const BUNDLE: &str = "examples/seed-bundles/nen2660-relations";
const PROFILE: &str = "https://example.org/nen2660-relations/profile";
const SHAPES: &str = "https://example.org/nen2660-relations/shapes";
const DATA: &str = "https://example.org/nen2660-relations/instances";
const EX: &str = "https://example.org/nen2660-relations/";
const NEN: &str = "https://w3id.org/nen2660/def#";

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
            &bundle_file("relations-profile.ttl"),
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

/// `(shape local name, focus node local name)` → count.
fn results_by_shape_and_focus(
    report: &open_triplestore::shacl::report::ValidationReport,
) -> BTreeMap<(String, String), usize> {
    let mut out = BTreeMap::new();
    for r in &report.results {
        let shape = r.source_shape.rsplit('#').next().unwrap_or("").to_string();
        let focus = r
            .focus_node
            .strip_prefix(EX)
            .unwrap_or(&r.focus_node)
            .to_string();
        *out.entry((shape, focus)).or_insert(0) += 1;
    }
    out
}

#[test]
fn relation_shapes_find_the_planted_cycle_escaped_part_and_self_containment() {
    let store = sample_store();
    let report = validate(&store, SHAPES, &[DATA.to_string()]).unwrap();
    let got = results_by_shape_and_focus(&report);
    let key = |shape: &str, focus: &str| (shape.to_string(), focus.to_string());

    assert!(!report.conforms, "the planted violations are found");
    // The cycle: each of loop-a and loop-b is a part of itself through the chain
    // (hasPart, then hasFunctionalPart — the sub-relations count).
    assert_eq!(
        got.get(&key("AcyclicDecompositionShape", "loop-a")),
        Some(&1),
        "{got:?}"
    );
    assert_eq!(
        got.get(&key("AcyclicDecompositionShape", "loop-b")),
        Some(&1),
        "{got:?}"
    );
    // The cabinet contains itself.
    assert_eq!(
        got.get(&key("IrreflexiveRelationShape", "cabinet-1")),
        Some(&1),
        "{got:?}"
    );
    // The lamp head lies outside the lamp post: not within, and RCC8-disconnected.
    assert_eq!(
        got.get(&key("PartWithinWholeShape", "lamp-post-1")),
        Some(&1),
        "{got:?}"
    );
    assert_eq!(
        got.get(&key("PartRcc8Shape", "lamp-post-1")),
        Some(&1),
        "{got:?}"
    );
    // pump-2 is "contained" by the service area but lies outside it.
    assert_eq!(
        got.get(&key("ContainedWithinRegionShape", "service-area-1")),
        Some(&1),
        "{got:?}"
    );
    // The conforming decomposition and containment produce nothing.
    for focus in ["bridge-1", "deck-1", "bearing-1", "pump-1", "joint-1"] {
        assert!(
            !got.keys().any(|(_, f)| f == focus),
            "{focus} conforms: {got:?}"
        );
    }
    assert_eq!(
        report.results.len(),
        6,
        "exactly the planted violations: {got:?}"
    );
}

/// The profile makes proper parthood transitive, and the functional/technical
/// sub-relations inherit it: the bearing is a part of the bridge through the
/// deck. Containment does not chain.
#[cfg(feature = "owl2-rl")]
#[test]
fn proper_parthood_is_transitive_through_the_profile_containment_is_not() {
    use open_triplestore::reasoning::owl2_rl::Owl2RLReasoner;
    let store = sample_store();
    // The cycle and the self-containment are shape violations, not OWL
    // inconsistencies: materialisation must succeed.
    Owl2RLReasoner::new(&store)
        .with_target("urn:entailment:owl2-rl:relations")
        .with_sources(vec![PROFILE.to_string(), DATA.to_string()])
        .materialize()
        .unwrap();
    let ask = |pattern: &str| {
        matches!(
            store.query(&format!(
                "ASK {{ GRAPH <urn:entailment:owl2-rl:relations> {{ {pattern} }} }}"
            )),
            Ok(QueryResults::Boolean(true))
        )
    };
    assert!(
        ask(&format!("<{EX}bridge-1> <{NEN}hasPart> <{EX}bearing-1>")),
        "prp-trp over hasPart with hasTechnicalPart ⊑ hasPart"
    );
    assert!(
        ask(&format!("<{EX}deck-1> <{NEN}hasPart> <{EX}bearing-1>")),
        "hasTechnicalPart ⊑ hasPart (prp-spo1)"
    );
    assert!(
        !ask(&format!("<{EX}bridge-1> <{NEN}contains> ?x")),
        "containment is not derived from parthood"
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

/// The bundle end to end — model registered, dataset conforming to it, shapes
/// bound, validation through the real API — when the NEN 2660-2 payload is
/// present. Skips (green) when it is not.
#[tokio::test]
async fn bundle_loads_and_validates_when_the_nen_payload_is_present() {
    let bundles = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/seed-bundles");
    if !bundles
        .join(BUNDLE.rsplit('/').next().unwrap())
        .join("nen2660-rdfs.ttl")
        .exists()
    {
        eprintln!("SKIP: nen2660-rdfs.ttl is not present — run examples/seed-bundles/nen2660-relations/fetch.sh to run the bundle test");
        return;
    }
    let (state, token) = admin_state();
    load_seed_dir(&state, &bundles);
    let app = test_app(state.clone());

    let (st, layer, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/nen2660-relations-sample/conformance",
        &token,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(
        layer["conforms_to_model"]["id"], "nen2660-relations",
        "{txt}"
    );
    assert_eq!(layer["conforms_to_model"]["version"], "2022", "{txt}");
    let shapes: Vec<&str> = layer["validation_shapes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(shapes, vec![SHAPES], "{txt}");
    let sources: Vec<&str> = layer["reasoning_sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        sources.contains(&DATA) && sources.contains(&PROFILE),
        "{txt}"
    );

    let (st, vreport, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/nen2660-relations-sample/validate",
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
    for planted in ["loop-a", "lamp-post-1", "cabinet-1", "service-area-1"] {
        assert!(txt.contains(planted), "the report names {planted}: {txt}");
    }
    assert!(!txt.contains("bridge-1"), "bridge-1 conforms: {txt}");
}
