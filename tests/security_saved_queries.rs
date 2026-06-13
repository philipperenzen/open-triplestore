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
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::saved_queries::models::{CreateSavedQueryRequest, QueryScope};
use open_triplestore::saved_queries::store::SavedQueryStore;
use open_triplestore::server::AppState;
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
