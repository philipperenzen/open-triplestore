//! End-to-end test for the R6 product capabilities (brief §M6 / DoD item 4):
//! load the Waalbrug dataset, fetch the **viewer feed** (per-element geometry,
//! reprojected, + glTF/IFC references), run **validation**, and read the
//! persisted `sh:ValidationReport` back **as RDF** plus the severity rollup.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{admin_state, body_json, test_app};
use open_triplestore::auth::models::{OwnerType, Visibility};
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use tower::ServiceExt as _;

const VOCAB: &str = include_str!("fixtures/waalbrug/vocab.ttl");
const ABOX: &str = include_str!("fixtures/waalbrug/waalbrug.trig");
const SHAPES_CORE: &str = include_str!("fixtures/waalbrug/shapes-core.ttl");
const SHAPES_SPARQL: &str = include_str!("fixtures/waalbrug/shapes-sparql.ttl");
const SHAPES_AF: &str = include_str!("fixtures/waalbrug/shapes-af.ttl");

fn req(method: &str, uri: &str, token: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header("Authorization", format!("Bearer {t}"));
    }
    b.body(Body::empty()).unwrap()
}

/// Build a dataset `wb` holding the Waalbrug ABox (urn:wb:data) + shapes (urn:wb:shapes).
fn waalbrug_state() -> (open_triplestore::server::AppState, String) {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "wb",
            "Waalbrug",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    state
        .auth_db
        .update_dataset_shacl("wb", false, Some("urn:wb:shapes"))
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("wb", "urn:wb:data")
        .unwrap();

    state
        .store
        .load_str(VOCAB, RdfFormat::Turtle, Some("urn:wb:data"))
        .unwrap();
    state
        .store
        .load_str(ABOX, RdfFormat::Turtle, Some("urn:wb:data"))
        .unwrap();
    for shapes in [VOCAB, SHAPES_CORE, SHAPES_SPARQL, SHAPES_AF] {
        state
            .store
            .load_str(shapes, RdfFormat::Turtle, Some("urn:wb:shapes"))
            .unwrap();
    }
    (state, token)
}

#[tokio::test]
async fn viewer_feed_returns_elements_with_gltf_and_reprojected_geometry() {
    let (state, token) = waalbrug_state();
    let resp = test_app(state)
        .oneshot(req("GET", "/api/datasets/wb/viewer-feed", Some(&token)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp.into_body()).await;
    let elements = j["elements"].as_array().expect("elements array");
    assert!(
        elements.len() >= 6,
        "root + 5 contained elements (+ sub-elements), got {}: {j}",
        elements.len()
    );

    let boog = elements
        .iter()
        .find(|e| e["id"].as_str().unwrap_or("").ends_with("Boog-Noord"))
        .expect("Boog-Noord in feed");
    assert_eq!(
        boog["gltf_url"].as_str(),
        Some("https://data.example.nl/files/boog-noord.glb"),
        "boog glTF URL: {boog}"
    );
    assert_eq!(boog["ifc_guid"].as_str(), Some("1aB2cD3eF4gH5iJ6kL7mNo"));
    // RD New point reprojected to WGS84 near Nijmegen (~5.86, ~51.85).
    let wkt = boog["wkt4326"].as_str().expect("wkt4326 present");
    let nums: Vec<f64> = wkt
        .trim_start_matches("POINT(")
        .trim_end_matches(')')
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    assert_eq!(nums.len(), 2, "POINT coords: {wkt}");
    assert!((nums[0] - 5.86).abs() < 0.05, "lon near Nijmegen: {wkt}");
    assert!((nums[1] - 51.85).abs() < 0.05, "lat near Nijmegen: {wkt}");

    // The GML-only Landhoofd-Noord also gets a reprojected geometry (srsName-aware).
    let landhoofd = elements
        .iter()
        .find(|e| e["id"].as_str().unwrap_or("").ends_with("Landhoofd-Noord"))
        .expect("Landhoofd-Noord in feed");
    assert!(
        landhoofd["wkt4326"]
            .as_str()
            .unwrap_or("")
            .starts_with("POINT"),
        "GML geometry reprojected: {landhoofd}"
    );

    // The root has no parent; children point at it.
    let root = elements
        .iter()
        .find(|e| e["id"].as_str().unwrap_or("").ends_with("/Waalbrug"))
        .expect("root in feed");
    assert!(root["parent"].is_null(), "root has no parent: {root}");
    assert!(
        boog["parent"].as_str().unwrap_or("").ends_with("/Waalbrug"),
        "boog parented to root: {boog}"
    );
}

#[tokio::test]
async fn validate_persists_report_as_queryable_rdf_with_rollup() {
    let (state, token) = waalbrug_state();
    let app = test_app(state.clone());

    // Official validation run (canonical dataset → conforms).
    let resp = app
        .clone()
        .oneshot(req("POST", "/api/datasets/wb/validate", Some(&token)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp.into_body()).await;
    assert_eq!(
        j["report"]["conforms"].as_bool(),
        Some(true),
        "canonical Waalbrug conforms: {j}"
    );

    // §7.4 — the report is queryable as RDF from the per-dataset report graph.
    let conforms_as_rdf = matches!(
        state.store.query(
            "PREFIX sh: <http://www.w3.org/ns/shacl#> \
             ASK { GRAPH <urn:system:reports:dataset:wb> { ?r a sh:ValidationReport ; sh:conforms true } }"
        ),
        Ok(QueryResults::Boolean(true))
    );
    assert!(conforms_as_rdf, "sh:ValidationReport persisted as RDF");

    // Severity rollup: the latest stored run carries counts by severity.
    let resp = app
        .oneshot(req(
            "GET",
            "/api/datasets/wb/validation/latest",
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp.into_body()).await;
    for key in ["violation_count", "warning_count", "info_count"] {
        assert!(
            j[key].is_number() || j["run"][key].is_number(),
            "rollup field {key} present: {j}"
        );
    }
}

/// Real-world demo: Wikidata-derived landmarks (CC0; coordinates from P625, 3D
/// models from P4896 on Wikimedia Commons) flow through the same viewer feed —
/// CRS84 geometry passes through, STL file references are exposed via FOG.
#[tokio::test]
async fn viewer_feed_serves_wikidata_landmarks_demo() {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "lm",
            "Landmarks",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("lm", "urn:lm:data")
        .unwrap();
    state
        .store
        .load_str(
            include_str!("fixtures/landmarks/landmarks.ttl"),
            RdfFormat::Turtle,
            Some("urn:lm:data"),
        )
        .unwrap();

    let resp = test_app(state)
        .oneshot(req("GET", "/api/datasets/lm/viewer-feed", Some(&token)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp.into_body()).await;
    let elements = j["elements"].as_array().expect("elements");
    assert_eq!(
        elements.len(),
        7,
        "collection root + 5 landmarks + CityJSON demo block: {j}"
    );

    let bridge = elements
        .iter()
        .find(|e| e["id"].as_str().unwrap_or("").ends_with("DragonBridge"))
        .expect("Dragon Bridge in feed");
    // CRS84 lon/lat passes through reprojection unchanged.
    let wkt = bridge["wkt4326"].as_str().expect("wkt4326");
    assert!(
        wkt.contains("108.226") && wkt.contains("16.061"),
        "Da Nang coordinates preserved: {wkt}"
    );
    // The STL model reference is exposed through the FOG file list.
    let files = bridge["files"].as_array().expect("files");
    assert!(
        files.iter().any(|f| f[0].as_str() == Some("Stl")
            && f[1].as_str().unwrap_or("").contains("Dragon_Bridge")),
        "Commons STL reference present: {files:?}"
    );

    // The synthetic CityJSON block exposes its bundled, site-relative file.
    let block = elements
        .iter()
        .find(|e| e["id"].as_str().unwrap_or("").ends_with("NijmegenCityBlock"))
        .expect("CityJSON demo block in feed");
    let files = block["files"].as_array().expect("files");
    assert!(
        files.iter().any(|f| f[0].as_str() == Some("Cityjson")
            && f[1].as_str() == Some("/samples/nijmegen-buildings.city.json")),
        "CityJSON reference present: {files:?}"
    );
}

/// Drift guard: the seed copies under src/saved_queries/data/ must stay
/// byte-identical to the canonical fixtures (below their 2-line SEED COPY header).
#[test]
fn seed_copies_match_canonical_fixtures() {
    for (seed, fixture) in [
        (include_str!("../src/saved_queries/data/waalbrug.ttl"), ABOX),
        (
            include_str!("../src/saved_queries/data/landmarks.ttl"),
            include_str!("fixtures/landmarks/landmarks.ttl"),
        ),
    ] {
        let body: String = seed.lines().skip(2).collect::<Vec<_>>().join("\n");
        let canon: String = fixture.lines().collect::<Vec<_>>().join("\n");
        assert_eq!(
            body.trim_end(),
            canon.trim_end(),
            "seed copy drifted from its canonical fixture — re-copy it"
        );
    }
}

#[tokio::test]
async fn viewer_feed_requires_access_on_private_dataset() {
    let (state, _token) = waalbrug_state();
    let resp = test_app(state)
        .oneshot(req("GET", "/api/datasets/wb/viewer-feed", None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "anonymous access to a private dataset's feed is denied"
    );
}
