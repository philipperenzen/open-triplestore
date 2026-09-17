//! Workload telemetry (P2 phase 0): the query path labels which exit answered
//! and stamps two shape bits per uncached evaluation, a cache hit inherits
//! them, validation runs record their source and duration, writes record
//! their spacing — and `GET /api/admin/telemetry` reports it all, to admins
//! only. These are the inputs to the go/no-go thresholds of
//! docs/notes/analytical-mirror-design.md §1.4.

mod common;

use std::path::Path;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::seed_bundles::load_seed_dir;
use serde_json::Value;
use tower::ServiceExt as _;

const SHAPES: &str = "https://example.org/dqv-quality/shapes";
const PROFILE: &str = "https://example.org/dqv-quality/profile";
const DATA: &str = "https://example.org/dqv-quality/instances";

async fn get_telemetry(app: &axum::Router, token: Option<&str>) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method(Method::GET)
        .uri("/api/admin/telemetry");
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    let txt = body_text(resp.into_body()).await;
    (st, serde_json::from_str(&txt).unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_summary_counts_exits_shape_bits_validations_and_write_gaps() {
    let (state, token) = admin_state();
    // Seeding the bundles is a burst of writes; the DQV bundle gives a dataset
    // with bound shapes for the validate route.
    load_seed_dir(
        &state,
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/seed-bundles"),
    );
    let store = &state.store;

    // A point query: a miss on the engine, then a hit.
    let point = "SELECT ?s WHERE { ?s ?p ?o } LIMIT 3";
    store.query(point).unwrap();
    store.query(point).unwrap();
    // The count index answers this without evaluating.
    store
        .query("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }")
        .unwrap();
    // An aggregate, twice: the second is a cache hit that must still count as
    // analytical — the bit is stamped on the entry, not recomputed.
    let agg = "SELECT ?t (COUNT(?s) AS ?n) WHERE { ?s a ?t } GROUP BY ?t";
    store.query(agg).unwrap();
    store.query(agg).unwrap();

    // A direct engine run, then one through the route.
    let report =
        open_triplestore::shacl::validate(store, SHAPES, &[PROFILE.to_string(), DATA.to_string()])
            .unwrap();
    let m = report
        .metrics
        .as_ref()
        .expect("the report carries its run metrics");
    assert_eq!(m.path, "engine");
    assert_eq!(m.graphs, 2);
    assert!(m.quads > 0, "{m:?}");
    assert!(!m.source.is_empty(), "{m:?}");

    let app = test_app(state.clone());
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/datasets/dqv-quality-sample/validate")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_str(&body_text(resp.into_body()).await).unwrap();
    let route_report = if body["report"].is_object() {
        &body["report"]
    } else {
        &body
    };
    assert_eq!(
        route_report["metrics"]["path"], "dataset",
        "the route labels its runs: {body}"
    );

    let (st, s) = get_telemetry(&app, Some(&token)).await;
    assert_eq!(st, StatusCode::OK, "{s}");
    let q = &s["queries"];
    assert!(q["total"].as_u64().unwrap() >= 5, "{s}");
    assert!(q["by_served"]["cache_hit"].as_u64().unwrap() >= 2, "{s}");
    assert!(q["by_served"]["fast_count"].as_u64().unwrap() >= 1, "{s}");
    assert!(
        q["by_served"]["engine"].as_u64().unwrap()
            + q["by_served"]["full_copy"].as_u64().unwrap()
            + q["by_served"]["columnar"].as_u64().unwrap()
            >= 1,
        "{s}"
    );
    for exit in [
        "cache_hit",
        "columnar",
        "fast_count",
        "shards",
        "full_copy",
        "engine",
    ] {
        assert!(q["by_served"][exit].is_u64(), "every exit is listed: {s}");
    }
    assert!(q["analytical"]["count"].as_u64().unwrap() >= 2, "{s}");
    assert!(
        q["analytical"]["by_served"]["cache_hit"]
            .as_u64()
            .unwrap_or(0)
            >= 1,
        "a hit inherits the analytical bit: {s}"
    );
    assert!(q["aggregate_text"].as_u64().unwrap() >= 3, "{s}");
    assert!(q["analytical"]["p95_us"].is_u64() && q["other"]["p50_us"].is_u64());
    let share = q["analytical"]["share"].as_f64().unwrap();
    assert!(share > 0.0 && share <= 1.0, "{s}");

    let v = &s["validations"];
    assert!(v["total"].as_u64().unwrap() >= 2, "{s}");
    assert!(v["by_path"]["engine"].as_u64().unwrap() >= 1, "{s}");
    assert!(v["by_path"]["dataset"].as_u64().unwrap() >= 1, "{s}");
    assert!(
        v["by_source"]
            .as_object()
            .map(|o| !o.is_empty())
            .unwrap_or(false),
        "{s}"
    );
    assert!(v["max_quads"].as_u64().unwrap() > 0, "{s}");

    let w = &s["writes"];
    let writes = w["total"].as_u64().unwrap();
    assert!(writes >= 1, "{s}");
    let gaps: u64 = w["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["count"].as_u64().unwrap())
        .sum();
    assert_eq!(
        gaps,
        writes - 1,
        "every write after the first records a gap: {s}"
    );
    assert_eq!(w["gaps"][0]["label"], "lt_100ms");
    assert!(
        w["gaps"][7]["upper_ms"].is_null(),
        "the last bucket is open-ended"
    );
}

/// The summary is an operator's view: admins only.
#[tokio::test]
async fn the_summary_is_for_admins_only() {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_user("u1", "user", "u1@test.com", "hash", SystemRole::User)
        .unwrap();
    let user_token = mint_token("u1", "user", "user");
    let app = test_app(state);
    let (st, _) = get_telemetry(&app, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = get_telemetry(&app, Some(&user_token)).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, s) = get_telemetry(&app, Some(&token)).await;
    assert_eq!(st, StatusCode::OK, "{s}");
    assert!(s["uptime_secs"].is_u64(), "{s}");
}
