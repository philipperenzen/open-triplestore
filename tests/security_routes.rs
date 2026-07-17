//! Security regression tests for the version-snapshot read boundary in
//! `src/server/routes.rs`.
//!
//! Covers [CB5]: a dataset *version* snapshot copies **all** of the dataset's
//! graphs into version-scoped IRIs (`{base}/dataset/{id}/version/{v}/{slug}`),
//! including graphs the owner flagged `private`. Those snapshot IRIs are absent
//! from `list_dataset_graph_entries` (which records only the LIVE graphs), so the
//! private filter used to be a no-op on a pinned-version read — leaking private
//! data to viewers. A viewer (here: anonymous on a *public* dataset) must NOT be
//! able to read a private graph's triples through a pinned `?version=` snapshot,
//! via either the dataset-service SPARQL endpoint or the triple-browser.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use common::*;
use tower::ServiceExt as _;

use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::dataset_versions::{models::DatasetVersion, snapshot_as_version};

// Distinctive marker literals so we can assert presence/absence in raw response
// bodies without depending on a specific result-serialisation shape.
const PUBLIC_MARKER: &str = "PUBLIC_TRIPLE_VISIBLE_MARKER";
const PRIVATE_MARKER: &str = "PRIVATE_TRIPLE_SECRET_MARKER";

const PUB_GRAPH: &str = "http://example.org/g/public";
const PRIV_GRAPH: &str = "http://example.org/g/private";
const VERSION: &str = "1.0.0";

/// Build a public dataset with one public graph and one private graph (each
/// holding a distinctively-marked triple), snapshot both into version `1.0.0`,
/// and register a SPARQL service. Returns `(app, version_record)`.
fn setup() -> (Router, DatasetVersion) {
    let state = test_state();

    // Owner user (a non-anonymous principal that owns the dataset). The viewer in
    // the assertions below is anonymous, which gets read-only `Viewer` on a public
    // dataset but can never write — the exact leak scenario.
    state
        .auth_db
        .create_user(
            "owner",
            "owner",
            "owner@test.com",
            "hash",
            open_triplestore::auth::models::SystemRole::User,
        )
        .unwrap();

    let ds = state
        .auth_db
        .create_dataset(
            "ds1",
            "DS1",
            None,
            OwnerType::User,
            "owner",
            Visibility::Public,
            None,
        )
        .unwrap();

    // Register both graphs to the dataset; flag the second one private.
    state.auth_db.add_dataset_graph(&ds.id, PUB_GRAPH).unwrap();
    state.auth_db.add_dataset_graph(&ds.id, PRIV_GRAPH).unwrap();
    state
        .auth_db
        .set_dataset_graph_private(&ds.id, PRIV_GRAPH, true)
        .unwrap();

    // Seed each live graph with a distinctively-marked triple.
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PUB_GRAPH}> {{ <http://example.org/s1> <http://example.org/p> \"{PUBLIC_MARKER}\" }} }}"
        ))
        .unwrap();
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PRIV_GRAPH}> {{ <http://example.org/s2> <http://example.org/p> \"{PRIVATE_MARKER}\" }} }}"
        ))
        .unwrap();

    // Snapshot BOTH live graphs into version 1.0.0 (this is what copies the
    // private graph into a version-scoped snapshot IRI).
    let record = snapshot_as_version(
        &state.store,
        state.base_url.as_str(),
        &ds.id,
        VERSION,
        &[PUB_GRAPH.to_string(), PRIV_GRAPH.to_string()],
        open_triplestore::dataset_versions::models::VersionStatus::Published,
        // created_by is a full user IRI in production ({base}/users/{id}); a bare
        // value would be an invalid relative IRI. It's irrelevant to CB5, so None.
        None,
        None,
    )
    .expect("snapshot_as_version should succeed");

    // A SPARQL service must exist for the dataset-service endpoint to resolve
    // (the pinned-version path bypasses its graph list but still requires it).
    state
        .auth_db
        .create_sparql_service("svc1", &ds.id, "Default", "default", None)
        .unwrap();

    (test_app(state), record)
}

/// The snapshot IRI whose SOURCE is `source` (from the version's graph map).
fn snapshot_iri_for(record: &DatasetVersion, source: &str) -> String {
    record
        .source_map
        .iter()
        .find(|m| m.source_graph == source)
        .map(|m| m.snapshot_graph.clone())
        .unwrap_or_else(|| panic!("no snapshot mapping for source {source}"))
}

async fn get(app: &Router, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    (status, text)
}

/// CB5 — dataset-service `?version=` path: an anonymous viewer reading a public
/// dataset's pinned snapshot must see the public graph's triples but NEVER the
/// private graph's triples.
#[tokio::test]
async fn viewer_cannot_read_private_snapshot_via_dataset_service_version() {
    let (app, _record) = setup();

    // SELECT projecting 3 vars avoids the graph-listing fast path and runs the
    // real engine over the (private-filtered) snapshot graph set.
    let query = url_encode("SELECT ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } }");
    let (status, body) = get(
        &app,
        &format!("/api/datasets/ds1/services/default/sparql?query={query}&version={VERSION}"),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "version read should succeed: {body}"
    );
    assert!(
        body.contains(PUBLIC_MARKER),
        "public snapshot triple must be visible to the viewer, body: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private snapshot triple must NOT leak to the viewer, body: {body}"
    );
}

/// CB5 — dataset-service graph-listing fast path: the private snapshot graph IRI
/// must not appear in the listed graphs for a viewer.
#[tokio::test]
async fn viewer_version_graph_listing_excludes_private_snapshot() {
    let (app, record) = setup();
    let priv_snap = snapshot_iri_for(&record, PRIV_GRAPH);
    let pub_snap = snapshot_iri_for(&record, PUB_GRAPH);

    let query = url_encode("SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } }");
    let (status, body) = get(
        &app,
        &format!("/api/datasets/ds1/services/default/sparql?query={query}&version={VERSION}"),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "graph listing should succeed: {body}"
    );
    assert!(
        body.contains(&pub_snap),
        "public snapshot graph must be listed, body: {body}"
    );
    assert!(
        !body.contains(&priv_snap),
        "private snapshot graph IRI must NOT be listed for a viewer, body: {body}"
    );
}

/// CB5 — browse `versions=` path (`scope_dataset_graphs`): an anonymous viewer
/// browsing a pinned snapshot must see public triples but not private ones.
#[tokio::test]
async fn viewer_cannot_browse_private_snapshot_via_versions_param() {
    let (app, _record) = setup();

    let (status, body) = get(
        &app,
        &format!("/api/browse/triples?dataset_id=ds1&versions=ds1:{VERSION}&limit=100"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "browse should succeed: {body}");
    assert!(
        body.contains(PUBLIC_MARKER),
        "public snapshot triple must be browsable by the viewer, body: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private snapshot triple must NOT leak through browse, body: {body}"
    );
}

/// CB5 — browse single-graph drill-down (`is_authorized_version_graph`): a viewer
/// passing the private snapshot's IRI directly via `graph=` must get nothing,
/// while the public snapshot IRI returns its triple.
#[tokio::test]
async fn viewer_cannot_drill_into_private_snapshot_graph() {
    let (app, record) = setup();
    let priv_snap = snapshot_iri_for(&record, PRIV_GRAPH);
    let pub_snap = snapshot_iri_for(&record, PUB_GRAPH);

    // Private snapshot graph: the drill-down access check denies it (404), and
    // the private triple never appears in the response either way.
    let (status, body) = get(
        &app,
        &format!(
            "/api/browse/triples?graph={}&versions=ds1:{VERSION}&limit=100",
            url_encode(&priv_snap)
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "drilling into a private snapshot IRI must be denied (404), body: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "drilling into the private snapshot IRI must NOT leak its triple, body: {body}"
    );

    // Public snapshot graph: allowed → its triple is returned (sanity check that
    // the version drill-down still works for non-private snapshots).
    let (status, body) = get(
        &app,
        &format!(
            "/api/browse/triples?graph={}&versions=ds1:{VERSION}&limit=100",
            url_encode(&pub_snap)
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "browse should succeed: {body}");
    assert!(
        body.contains(PUBLIC_MARKER),
        "public snapshot drill-down must still return its triple, body: {body}"
    );
}
