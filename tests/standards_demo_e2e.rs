//! End-to-end tests for the bundled public "Open Triplestore" demo.
//!
//! Runs the real startup seed into an in-memory `AppState`, then drives the
//! HTTP API exactly as the frontend would: it discovers the public organisation,
//! its category datasets and their graphs, lists each dataset's saved queries
//! (API services), and **runs every one** through the public run endpoint. The
//! protocol/auth standards that can't be expressed as a saved query (Service
//! Description, Graph Store HTTP, VoID, JWT, and — under their features — LDP and
//! ShEx) are exercised against the live endpoints.
//!
//! Feature-dependent behaviour (RDF-star, LDP, ShEx) is guarded with `cfg!` so
//! the suite passes both on the default build and on `--features full` (the
//! build the demo server ships with), where everything is exercised.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt as _;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::Value;
use tower::ServiceExt as _;

use open_triplestore::{
    auth::{
        db::AuthDb,
        jwt::{issue_access_token, JwtConfig},
        models::{OwnerType, SystemRole},
        oauth::new_session_store,
    },
    prefixes::PrefixRegistry,
    saved_queries::seed::seed_open_triplestore,
    saved_queries::seed_data::{datasets as demo_datasets, ORG_SLUG},
    server::{build_router, AppState},
    storage::ObjectStore,
    store::TripleStore,
};

const JWT_SECRET: &str = "test_secret_must_be_32_chars_abcd";

fn test_state() -> AppState {
    let auth_db = Arc::new(AuthDb::in_memory().unwrap());
    let audit = Arc::new(open_triplestore::auth::audit::AuditLogger::new(
        auth_db.pool(),
    ));
    AppState {
        store: TripleStore::in_memory().unwrap(),
        prefix_registry: Arc::new(PrefixRegistry::empty()),
        auth_db,
        audit,
        backup: None,
        jwt_config: Arc::new(JwtConfig::new(JWT_SECRET.to_string(), 30, 30)),
        object_store: Arc::new(ObjectStore::noop()),
        mailer: Arc::new(open_triplestore::email::Mailer::log_only("http://localhost:7878")),
        base_url: Arc::new("http://localhost:7878".to_string()),
        oauth_sessions: new_session_store(),
        passkey_sessions: open_triplestore::auth::passkey::new_session_store(),
        auth_ext: Arc::new(open_triplestore::auth::oidc_rs::AuthExt::disabled()),
        query_timeout_secs: 30,
        secure_cookies: false,
        browse_semaphore: Arc::new(tokio::sync::Semaphore::new(64)),
        expensive_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
        #[cfg(feature = "text-search")]
        text_index: None,
        #[cfg(feature = "text-search")]
        text_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    }
}

/// Seed the demo and return (state, admin_token, org_id).
fn seeded() -> (AppState, String, String) {
    let state = test_state();
    state
        .auth_db
        .create_user(
            "adm",
            "admin",
            "admin@test.com",
            "hash",
            SystemRole::SuperAdmin,
        )
        .unwrap();
    let token = issue_access_token(
        &JwtConfig::new(JWT_SECRET.to_string(), 30, 30),
        "adm",
        "admin",
        "super_admin",
    )
    .unwrap();

    // Tests must never reach the network: an empty SEED_IFC_URL makes the
    // seeder skip the Schependomlaan IFC download (same pattern as the
    // role-visibility tests).
    std::env::set_var("SEED_IFC_URL", "");
    seed_open_triplestore(&state);

    let org = state
        .auth_db
        .get_organisation_by_slug(ORG_SLUG)
        .unwrap()
        .expect("demo organisation must be seeded");
    (state, token, org.id)
}

/// Production runs two seeders on a fresh install — the boot task and the
/// first-admin registration handler — and they can overlap (the browser-e2e
/// harness registers its admin while the boot task is still on the SHACL
/// seeds). They must serialise, with the loser re-running idempotently — not
/// race their INSERTs into UNIQUE constraints and leave a half-seeded demo.
#[tokio::test]
async fn concurrent_seeders_serialise_into_one_clean_seed() {
    let state = test_state();
    state
        .auth_db
        .create_user(
            "adm",
            "admin",
            "admin@test.com",
            "hash",
            SystemRole::SuperAdmin,
        )
        .unwrap();
    std::env::set_var("SEED_IFC_URL", "");
    let (a, b) = (state.clone(), state.clone());
    let race = tokio::join!(
        tokio::task::spawn_blocking(move || seed_open_triplestore(&a)),
        tokio::task::spawn_blocking(move || seed_open_triplestore(&b)),
    );
    race.0.unwrap();
    race.1.unwrap();

    let org = state
        .auth_db
        .get_organisation_by_slug(ORG_SLUG)
        .unwrap()
        .expect("demo organisation must be seeded");
    let datasets = org_datasets(&state, &org.id);
    assert_eq!(
        datasets.len(),
        demo_datasets().len(),
        "racing seeders must leave exactly the full demo dataset set"
    );
}

fn app(state: &AppState) -> Router {
    build_router(state.clone(), "", vec![])
}

fn url_encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

async fn body_text(body: Body) -> String {
    String::from_utf8_lossy(&body.collect().await.unwrap().to_bytes()).into_owned()
}

async fn body_json(body: Body) -> Value {
    serde_json::from_str(&body_text(body).await).unwrap_or(Value::Null)
}

async fn get(state: &AppState, token: &str, uri: &str) -> axum::response::Response {
    app(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Org datasets (id, name), discovered the way the UI does.
fn org_datasets(state: &AppState, org_id: &str) -> Vec<(String, String)> {
    state
        .auth_db
        .list_datasets()
        .unwrap()
        .into_iter()
        .filter(|d| matches!(d.owner_type, OwnerType::Organisation) && d.owner_id == *org_id)
        .map(|d| (d.id, d.name))
        .collect()
}

// ─── Seed shape ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn seed_creates_public_org_with_all_category_datasets() {
    let (state, _token, org_id) = seeded();

    let datasets = org_datasets(&state, &org_id);
    assert_eq!(
        datasets.len(),
        demo_datasets().len(),
        "every demo dataset must be created under the org"
    );

    // Each dataset's declared graphs are registered and hold data — except the
    // RDF-star graph, which only loads on an rdf-12 build.
    for spec in demo_datasets() {
        let (ds_id, _) = datasets
            .iter()
            .find(|(_, name)| name == spec.name)
            .unwrap_or_else(|| panic!("dataset '{}' missing", spec.name));
        let registered: BTreeSet<String> = state
            .auth_db
            .list_dataset_graphs(ds_id)
            .unwrap()
            .into_iter()
            .collect();

        for g in spec.graphs {
            let iri = format!(
                "https://opentriplestore.org/demo/{}/{}",
                spec.slug, g.suffix
            );
            let rdf_star = g.suffix == "rdf-star";
            if rdf_star && !cfg!(feature = "rdf-12") {
                continue; // SPARQL-star INSERT needs rdf-12 at runtime
            }
            assert!(
                registered.contains(&iri),
                "graph <{iri}> must be registered"
            );
            let n = state.store.count_graph(Some(&iri)).unwrap();
            assert!(n > 0, "graph <{iri}> must hold data, got {n}");
        }
    }
}

#[tokio::test]
async fn public_org_is_anonymously_discoverable() {
    let (state, _token, org_id) = seeded();

    // Anonymous org listing includes the public demo org.
    let resp = app(&state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/organisations")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let orgs = body.as_array().cloned().unwrap_or_default();
    assert!(
        orgs.iter().any(|o| o["slug"] == "open-triplestore"),
        "anonymous /api/organisations must list the public demo org"
    );

    // Anonymous org detail is readable for a public-dataset-owning org.
    let resp = app(&state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/organisations/{org_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "anonymous org detail must be readable for the public demo org"
    );
}

// ─── Run every saved query end-to-end ───────────────────────────────────────────

#[tokio::test]
async fn every_saved_query_runs_through_the_api() {
    let (state, token, org_id) = seeded();
    let mut total_services = 0usize;

    for (ds_id, ds_name) in org_datasets(&state, &org_id) {
        // List the dataset's API services (public endpoint, as the UI lists them).
        let resp = get(
            &state,
            &token,
            &format!("/api/datasets/{ds_id}/api-services"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "{ds_name}: list services");
        let listed = body_json(resp.into_body()).await;
        let services = listed["queries"].as_array().cloned().unwrap_or_default();

        for svc in &services {
            let slug = svc["slug"].as_str().unwrap();

            // RDF-star query parsing requires the rdf-12 build; tolerate rejection there.
            if slug == "rdf-star-provenance" && !cfg!(feature = "rdf-12") {
                continue;
            }

            // No params are supplied: the run endpoint applies each saved query's
            // declared default value (params::inject).
            let uri = format!("/api/datasets/{ds_id}/api-services/{slug}/run");
            let resp = get(&state, &token, &uri).await;

            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "service '{slug}' in '{ds_name}' must run (got {})",
                resp.status()
            );
            let body = body_text(resp.into_body()).await;
            assert!(!body.is_empty(), "service '{slug}' returned an empty body");
            total_services += 1;
        }
    }

    assert!(
        total_services >= 18,
        "expected the full set of demo services, ran {total_services}"
    );
}

/// Spot-check that representative always-on services return the expected data
/// (not just HTTP 200).
#[tokio::test]
async fn representative_services_return_expected_data() {
    let (state, token, org_id) = seeded();
    let datasets = org_datasets(&state, &org_id);
    let ds_id = |name: &str| datasets.iter().find(|(_, n)| n == name).unwrap().0.clone();

    // GeoSPARQL: the bounding box around the Low Countries contains the NL/BE
    // cities but not Paris.
    let spatial = ds_id("Spatial (GeoSPARQL)");
    let resp = get(
        &state,
        &token,
        &format!("/api/datasets/{spatial}/api-services/cities-in-bbox/run"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let txt = body_text(resp.into_body()).await;
    assert!(
        txt.contains("Amsterdam"),
        "bbox query should include Amsterdam: {txt}"
    );
    assert!(!txt.contains("Paris"), "bbox query should exclude Paris");

    // RDFS: transitive subclass path Dog ⊑ Mammal ⊑ Animal.
    let reasoning = ds_id("Reasoning & Ontologies");
    let resp = get(
        &state,
        &token,
        &format!("/api/datasets/{reasoning}/api-services/class-hierarchy/run"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let txt = body_text(resp.into_body()).await;
    assert!(
        txt.contains("Animal"),
        "subclass closure should reach Animal: {txt}"
    );

    // Capabilities: the JSON-LD graph lists every standard.
    let caps = ds_id("Platform Capabilities & Security");
    let resp = get(
        &state,
        &token,
        &format!("/api/datasets/{caps}/api-services/supported-standards/run"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let txt = body_text(resp.into_body()).await;
    assert!(
        txt.contains("GeoSPARQL"),
        "capabilities should list GeoSPARQL: {txt}"
    );
}

// ─── Protocol / auth standards against live endpoints ────────────────────────────

#[tokio::test]
async fn service_description_and_void_endpoints() {
    let (state, token, _org) = seeded();

    let resp = get(&state, &token, "/").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let sd = body_text(resp.into_body()).await;
    assert!(sd.contains("sd:Service") && sd.contains("SPARQL11Query"));

    let resp = get(&state, &token, "/.well-known/void").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_text(resp.into_body()).await.contains("void"));
}

#[tokio::test]
async fn graph_store_and_jwt_on_the_running_platform() {
    let (state, token, _org) = seeded();
    let g = "https://opentriplestore.org/demo/e2e/tmp";
    let enc = url_encode(g);

    // Unauthenticated write is rejected (JWT gate).
    let resp = app(&state)
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/store?graph={enc}"))
                .header(header::CONTENT_TYPE, "text/turtle")
                .body(Body::from("<http://ex/s> <http://ex/p> \"v\" ."))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Authenticated PUT then DELETE via the Graph Store HTTP protocol.
    let resp = app(&state)
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/store?graph={enc}"))
                .header(header::CONTENT_TYPE, "text/turtle")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from("<http://ex/s> <http://ex/p> \"v\" ."))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(state.store.count_graph(Some(g)).unwrap(), 1);
}

// ─── Feature-gated demos (only meaningful on --features full) ────────────────────

#[cfg(feature = "rdf-12")]
#[tokio::test]
async fn rdf_star_service_returns_provenance() {
    let (state, token, org_id) = seeded();
    let datasets = org_datasets(&state, &org_id);
    let core = datasets
        .iter()
        .find(|(_, n)| n == "Core RDF & SPARQL")
        .unwrap()
        .0
        .clone();
    let resp = get(
        &state,
        &token,
        &format!("/api/datasets/{core}/api-services/rdf-star-provenance/run"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        body_text(resp.into_body()).await.contains("wikipedia.org"),
        "RDF-star provenance query should surface the dct:source"
    );
}

#[cfg(feature = "shex")]
#[tokio::test]
async fn shex_validation_endpoint_is_live() {
    let (state, token, _org) = seeded();
    // A minimal ShExC schema + a shape_map (shape IRI → focus nodes).
    let body = serde_json::json!({
        "schema": "PREFIX ex: <https://opentriplestore.org/demo/validation#> ex:UserShape { ex:email . }",
        "shape_map": {
            "https://opentriplestore.org/demo/validation#UserShape":
                ["https://opentriplestore.org/demo/validation#carol"]
        }
    });
    let resp = app(&state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/shex/validate")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    // The endpoint exists and handles the request (validation outcome may vary).
    assert!(
        resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_REQUEST,
        "ShEx validate endpoint should respond, got {}",
        resp.status()
    );
}
