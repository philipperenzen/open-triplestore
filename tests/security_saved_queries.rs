//! Security regression: saved-query (dataset API service) private-graph leak.
//!
//! Finding CB4 — `saved_queries::exec::prepare_run` resolved a dataset-scoped
//! query's graph set (live or snapshot) without dropping graphs flagged
//! `private`. A viewer or anonymous caller of a *public* dataset's API service
//! could therefore read triples from a sub-graph the owner had marked private by
//! running a query that spans `GRAPH ?g { ?s ?p ?o }`.
//!
//! The fix subtracts private graphs for any caller who cannot write the dataset
//! (mirroring the dataset-service path in `routes.rs`), while preserving full
//! visibility for writers and admins. These tests lock both halves in at the HTTP
//! level by driving the real run endpoint.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{OwnerType, Role, SystemRole, Visibility};
use open_triplestore::dataset_versions::models::VersionStatus;
use open_triplestore::dataset_versions::snapshot_as_version;
use open_triplestore::saved_queries::models::{CreateSavedQueryRequest, QueryScope};
use open_triplestore::saved_queries::store::SavedQueryStore;
use open_triplestore::server::AppState;
use serde_json::json;
use tower::ServiceExt as _;

const PUBLIC_GRAPH: &str = "http://example.org/ds/public";
const PRIVATE_GRAPH: &str = "http://example.org/ds/secret";
const PUBLIC_MARKER: &str = "public-value";
const PRIVATE_MARKER: &str = "private-value";

/// Public dataset `apids` with one public graph and one *private* graph, plus a
/// dataset-scoped API service whose SPARQL spans every graph. Returns the run
/// path for the created service.
fn setup_service(state: &AppState) -> String {
    // A public dataset so an anonymous caller passes the read gate.
    state
        .auth_db
        .create_dataset(
            "apids",
            "API Dataset",
            None,
            OwnerType::User,
            "owner",
            Visibility::Public,
            None,
        )
        .unwrap();

    // Two graphs registered to the dataset; the second is flagged private.
    state
        .auth_db
        .add_dataset_graph("apids", PUBLIC_GRAPH)
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("apids", PRIVATE_GRAPH)
        .unwrap();
    state
        .auth_db
        .set_dataset_graph_private("apids", PRIVATE_GRAPH, true)
        .unwrap();

    // Data in each graph.
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PUBLIC_GRAPH}> {{ <http://example.org/a> <http://example.org/p> \"{PUBLIC_MARKER}\" }} }}"
        ))
        .unwrap();
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PRIVATE_GRAPH}> {{ <http://example.org/b> <http://example.org/p> \"{PRIVATE_MARKER}\" }} }}"
        ))
        .unwrap();

    // A public, active dataset-scoped API service that reads across all graphs.
    let sq_store = SavedQueryStore::new(state.auth_db.pool());
    let req = CreateSavedQueryRequest {
        name: "All Graphs".to_string(),
        slug: Some("all-graphs".to_string()),
        description: None,
        sparql: "SELECT ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } }".to_string(),
        parameters: vec![],
        test_parameters: None,
        visibility: Some("public".to_string()),
        version_name: None,
        note: None,
    };
    let sq = sq_store
        .create(QueryScope::Dataset, "apids", &req, "owner")
        .unwrap();

    format!(
        "/api/datasets/apids/api-services/{}/run?version=latest",
        sq.slug
    )
}

fn run_request(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::ACCEPT, "application/sparql-results+json");
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    builder.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn private_graph_hidden_from_anonymous_dataset_api_service_security() {
    let (state, _admin) = admin_state();
    let uri = setup_service(&state);

    // Anonymous caller (no token) runs the public service.
    let resp = test_app(state.clone())
        .oneshot(run_request(&uri, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "anonymous run of a public dataset API service must succeed"
    );
    let body = body_text(resp.into_body()).await;

    // The public graph is readable; the private graph must NOT leak.
    assert!(
        body.contains(PUBLIC_MARKER),
        "public graph data should be visible to anonymous callers: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private graph data must NOT be exposed to an anonymous caller: {body}"
    );
}

#[tokio::test]
async fn private_graph_visible_to_writer_dataset_api_service_security() {
    // Positive control: a caller who CAN write the dataset (here a super-admin)
    // keeps full visibility, so the private-graph filter does not over-reach.
    let (state, admin) = admin_state();
    let uri = setup_service(&state);

    let resp = test_app(state.clone())
        .oneshot(run_request(&uri, Some(&admin)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;

    assert!(
        body.contains(PUBLIC_MARKER),
        "writer should see public graph data: {body}"
    );
    assert!(
        body.contains(PRIVATE_MARKER),
        "a dataset writer/admin must still see private graph data: {body}"
    );
}

// ─── V4: version-snapshot leak ───────────────────────────────────────────────
//
// The CB4 tests above cover the LIVE graph set (?version=latest). A snapshot
// copies every graph — private included — into version-scoped IRIs that the live
// private filter never matched, so a `?version=<label>` run leaked them. The
// filter is now version-aware (maps snapshot → live source, drops private ones).

#[tokio::test]
async fn private_snapshot_graph_hidden_from_anonymous_version_run() {
    let (state, admin) = admin_state();
    setup_service(&state); // dataset `apids`, public + private graph, service `all-graphs`

    // Snapshot BOTH graphs into version 1.0.0 (copies the private graph too).
    snapshot_as_version(
        &state.store,
        state.base_url.as_str(),
        "apids",
        "1.0.0",
        &[PUBLIC_GRAPH.to_string(), PRIVATE_GRAPH.to_string()],
        VersionStatus::Published,
        None,
        None,
    )
    .unwrap();

    let uri = "/api/datasets/apids/api-services/all-graphs/run?version=1.0.0";

    // Anonymous caller: public snapshot visible, private snapshot hidden.
    let resp = test_app(state.clone())
        .oneshot(run_request(uri, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains(PUBLIC_MARKER),
        "public snapshot missing: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private snapshot leaked through a pinned-version run: {body}"
    );

    // Writer/admin still sees both.
    let resp = test_app(state.clone())
        .oneshot(run_request(uri, Some(&admin)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains(PUBLIC_MARKER) && body.contains(PRIVATE_MARKER),
        "a writer must still see the private snapshot: {body}"
    );
}

// ─── V5: organisation-scoped union leak ──────────────────────────────────────

/// An org `acme` with one public dataset that has a public and a private graph,
/// and a public org-scoped API service spanning every graph. Returns the run URL.
fn setup_org_service(state: &AppState) -> String {
    state
        .auth_db
        .create_organisation("acme", "Acme", "acme", None, None)
        .unwrap();
    state
        .auth_db
        .create_dataset(
            "acme-ds",
            "Acme DS",
            None,
            OwnerType::Organisation,
            "acme",
            Visibility::Public,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("acme-ds", PUBLIC_GRAPH)
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("acme-ds", PRIVATE_GRAPH)
        .unwrap();
    state
        .auth_db
        .set_dataset_graph_private("acme-ds", PRIVATE_GRAPH, true)
        .unwrap();
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PUBLIC_GRAPH}> {{ <http://example.org/a> <http://example.org/p> \"{PUBLIC_MARKER}\" }} }}"
        ))
        .unwrap();
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PRIVATE_GRAPH}> {{ <http://example.org/b> <http://example.org/p> \"{PRIVATE_MARKER}\" }} }}"
        ))
        .unwrap();

    let sq_store = SavedQueryStore::new(state.auth_db.pool());
    let req = CreateSavedQueryRequest {
        name: "Org All".to_string(),
        slug: Some("org-all".to_string()),
        description: None,
        sparql: "SELECT ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } }".to_string(),
        parameters: vec![],
        test_parameters: None,
        visibility: Some("public".to_string()),
        version_name: None,
        note: None,
    };
    let sq = sq_store
        .create(QueryScope::Organisation, "acme", &req, "owner")
        .unwrap();
    format!("/api/organisations/acme/api-services/{}/run", sq.slug)
}

#[tokio::test]
async fn org_service_union_hides_private_graphs_from_anonymous() {
    let (state, admin) = admin_state();
    let uri = setup_org_service(&state);

    let resp = test_app(state.clone())
        .oneshot(run_request(&uri, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(body.contains(PUBLIC_MARKER), "public graph missing: {body}");
    assert!(
        !body.contains(PRIVATE_MARKER),
        "org union leaked a private graph to an anonymous caller: {body}"
    );

    // Admin (writer over every dataset) still sees the private graph.
    let resp = test_app(state.clone())
        .oneshot(run_request(&uri, Some(&admin)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains(PRIVATE_MARKER),
        "an admin must still see the org's private graph: {body}"
    );
}

// ─── V6: visibility=public bypass ────────────────────────────────────────────

fn create_org_service_request(token: &str, visibility: &str) -> Request<Body> {
    let body = json!({
        "name": "Member Svc",
        "slug": "member-svc",
        "sparql": "SELECT * WHERE { ?s ?p ?o } LIMIT 1",
        "visibility": visibility,
    });
    Request::builder()
        .method(Method::POST)
        .uri("/api/organisations/acme/api-services")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn org_member_cannot_publish_a_service_but_admin_can() {
    let (state, _admin) = admin_state();
    state
        .auth_db
        .create_organisation("acme", "Acme", "acme", None, None)
        .unwrap();
    for u in ["member", "boss"] {
        state
            .auth_db
            .create_user(u, u, &format!("{u}@t.com"), "h", SystemRole::User)
            .unwrap();
    }
    state
        .auth_db
        .add_org_member("member", "acme", Role::Member)
        .unwrap();
    state
        .auth_db
        .add_org_member("boss", "acme", Role::Admin)
        .unwrap();

    // A Member may create a service, but not a public one.
    let member = mint_token("member", "member", "user");
    let resp = test_app(state.clone())
        .oneshot(create_org_service_request(&member, "public"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a Member must not be able to publish (visibility=public): {}",
        body_text(resp.into_body()).await
    );

    // The same Member may create a non-public service.
    let resp = test_app(state.clone())
        .oneshot(create_org_service_request(&member, "members"))
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "a Member must still create a non-public service: {} {}",
        resp.status(),
        body_text(resp.into_body()).await
    );

    // An Admin may publish.
    let boss = mint_token("boss", "boss", "user");
    let resp = test_app(state.clone())
        .oneshot(create_org_service_request(&boss, "public"))
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "an org Admin must be able to publish: {} {}",
        resp.status(),
        body_text(resp.into_body()).await
    );
}

#[tokio::test]
async fn invalid_visibility_is_rejected() {
    let (state, _admin) = admin_state();
    state
        .auth_db
        .create_organisation("acme", "Acme", "acme", None, None)
        .unwrap();
    state
        .auth_db
        .create_user("boss", "boss", "boss@t.com", "h", SystemRole::User)
        .unwrap();
    state
        .auth_db
        .add_org_member("boss", "acme", Role::Admin)
        .unwrap();
    let boss = mint_token("boss", "boss", "user");
    let resp = test_app(state.clone())
        .oneshot(create_org_service_request(&boss, "world-readable"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "an unknown visibility value must be rejected: {}",
        body_text(resp.into_body()).await
    );
}

// ─── V7: orphaned service + dataset-id reuse ─────────────────────────────────

#[tokio::test]
async fn deleting_a_dataset_removes_its_api_services() {
    let (state, _admin) = admin_state();
    setup_service(&state); // dataset `apids` + public service `all-graphs`
    let sq_store = SavedQueryStore::new(state.auth_db.pool());
    assert!(
        sq_store
            .get_by_slug(QueryScope::Dataset, "apids", "all-graphs")
            .unwrap()
            .is_some(),
        "service should exist before delete"
    );

    state.auth_db.delete_dataset("apids").unwrap();

    assert!(
        sq_store
            .get_by_slug(QueryScope::Dataset, "apids", "all-graphs")
            .unwrap()
            .is_none(),
        "deleting the dataset must delete its API services"
    );

    // Re-create a DIFFERENT dataset that reuses the id, private, owned by someone
    // else. The old public service must NOT resurrect and expose it.
    state
        .auth_db
        .create_user("victim", "victim", "v@t.com", "h", SystemRole::User)
        .unwrap();
    state
        .auth_db
        .create_dataset(
            "apids",
            "Reused",
            None,
            OwnerType::User,
            "victim",
            Visibility::Private,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph("apids", PRIVATE_GRAPH)
        .unwrap();

    let resp = test_app(state.clone())
        .oneshot(run_request(
            "/api/datasets/apids/api-services/all-graphs/run?version=latest",
            None,
        ))
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "a service planted on a since-deleted dataset id must not run against the reused dataset: {}",
        body_text(resp.into_body()).await
    );
}
