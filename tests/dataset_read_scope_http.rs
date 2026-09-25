//! Security regression tests for private-graph / non-public-asset read scoping
//! across the dataset read endpoints.
//!
//! Several read handlers used to scope on the RAW dataset graph list (via
//! `list_dataset_graphs`), or — for the container export — on *every* asset,
//! after only checking `can_access_dataset`. A caller who can READ a dataset
//! but not WRITE it (a viewer, or an anonymous caller on a *public* dataset)
//! could therefore see private-graph content, and download non-public asset
//! bytes through the container export.
//!
//! The correct rule is the one `GET /api/datasets/:id/graphs` uses: a writer
//! (owner / maintainer / admin) sees private graphs; everyone else sees only
//! non-private ones. It is now centralised in `AuthDb::list_readable_dataset_graphs`
//! and applied by each handler below; the container export additionally drops
//! non-public assets from an anonymous export (matching `list_assets`).
//!
//! Each test asserts the leak scenario directly: an anonymous viewer on the
//! public dataset sees the public graph/asset but NEVER the private one, while
//! the owner still sees everything.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::server::AppState;

// Distinctive marker literals so we can assert presence/absence in raw response
// bodies (JSON, GeoJSON, binary GLB, ZIP) without depending on a specific
// serialisation shape.
const PUBLIC_MARKER: &str = "PUBLIC_TRIPLE_VISIBLE_MARKER";
const PRIVATE_MARKER: &str = "PRIVATE_TRIPLE_SECRET_MARKER";

const PUB_GRAPH: &str = "http://example.org/g/public";
const PRIV_GRAPH: &str = "http://example.org/g/private";

// The element IRI *and* its rdfs:label both carry the marker, so it surfaces in
// every serialisation: the element id / feature IRI (viewer feed, OGC Features,
// 3D-Tiles GLB structural metadata) and the label (viewer feed, OGC properties,
// browse suggestions).
const PUB_EL: &str = "http://example.org/el/PUBLIC_TRIPLE_VISIBLE_MARKER";
const PRIV_EL: &str = "http://example.org/el/PRIVATE_TRIPLE_SECRET_MARKER";

/// A public dataset `ds1` owned by `owner`, with a public graph and a private
/// graph, each holding one labelled, geometry-bearing element (a WGS84 polygon
/// so it both counts as geometry and triangulates for 3D-Tiles). The viewer in
/// the assertions is anonymous — read-only `Viewer` on a public dataset, never
/// a writer, the exact leak scenario.
fn setup() -> AppState {
    let state = test_state();

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

    // Register both graphs; flag the second one private.
    state.auth_db.add_dataset_graph(&ds.id, PUB_GRAPH).unwrap();
    state.auth_db.add_dataset_graph(&ds.id, PRIV_GRAPH).unwrap();
    state
        .auth_db
        .set_dataset_graph_private(&ds.id, PRIV_GRAPH, true)
        .unwrap();

    seed_element(
        &state,
        PUB_GRAPH,
        PUB_EL,
        "POLYGON((0 0, 0.001 0, 0.001 0.001, 0 0.001, 0 0))",
    );
    seed_element(
        &state,
        PRIV_GRAPH,
        PRIV_EL,
        "POLYGON((1 1, 1.001 1, 1.001 1.001, 1 1.001, 1 1))",
    );

    state
}

/// Insert one located, labelled element into `graph`. The element IRI's local
/// name equals its label so the marker is observable however the element is
/// serialised.
fn seed_element(state: &AppState, graph: &str, el: &str, polygon: &str) {
    let label = el.rsplit('/').next().unwrap();
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{graph}> {{ \
               <{el}> a <http://example.org/Thing> ; \
                 <http://www.w3.org/2000/01/rdf-schema#label> \"{label}\" ; \
                 <http://www.opengis.net/ont/geosparql#hasGeometry> _:g . \
               _:g <http://www.opengis.net/ont/geosparql#asWKT> \
                 \"{polygon}\"^^<http://www.opengis.net/ont/geosparql#wktLiteral> . \
             }} }}"
        ))
        .unwrap();
}

fn owner_token() -> String {
    mint_token("owner", "owner", "user")
}

/// GET returning the raw response bytes, optionally as `owner`.
async fn get_bytes(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, Vec<u8>) {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

/// GET returning the body as UTF-8 text.
async fn get_text(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, String) {
    let (status, bytes) = get_bytes(app, uri, token).await;
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

// ─── viewer_feed ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn viewer_feed_hides_private_graph_from_viewer() {
    let app = test_app(setup());

    let (status, body) = get_text(&app, "/api/datasets/ds1/viewer-feed", None).await;
    assert_eq!(status, StatusCode::OK, "viewer-feed should succeed: {body}");
    assert!(
        body.contains(PUBLIC_MARKER),
        "public element must be visible to the viewer: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private element must NOT leak to the viewer: {body}"
    );

    let owner = owner_token();
    let (status, body) = get_text(&app, "/api/datasets/ds1/viewer-feed", Some(&owner)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "owner viewer-feed should succeed: {body}"
    );
    assert!(
        body.contains(PUBLIC_MARKER) && body.contains(PRIVATE_MARKER),
        "owner must see both elements: {body}"
    );
}

// ─── geo_stats ──────────────────────────────────────────────────────────────

fn element_count(body: &str) -> u64 {
    serde_json::from_str::<serde_json::Value>(body)
        .unwrap()
        .get("element_count")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| panic!("no element_count in {body}"))
}

#[tokio::test]
async fn geo_stats_counts_only_readable_graphs() {
    let app = test_app(setup());

    let (status, body) = get_text(&app, "/api/datasets/ds1/geo-stats", None).await;
    assert_eq!(status, StatusCode::OK, "geo-stats should succeed: {body}");
    assert_eq!(
        element_count(&body),
        1,
        "viewer must only count the public graph's element: {body}"
    );

    let owner = owner_token();
    let (status, body) = get_text(&app, "/api/datasets/ds1/geo-stats", Some(&owner)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "owner geo-stats should succeed: {body}"
    );
    assert_eq!(
        element_count(&body),
        2,
        "owner must count both graphs' elements: {body}"
    );
}

// ─── geo_stats_batch ──────────────────────────────────────────────────────────

#[tokio::test]
async fn geo_stats_batch_counts_only_readable_graphs() {
    let app = test_app(setup());

    let (status, body) = get_text(&app, "/api/geo-stats?datasets=ds1", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "geo-stats batch should succeed: {body}"
    );
    assert_eq!(
        element_count(&body),
        1,
        "viewer must only count the public graph's element: {body}"
    );

    let owner = owner_token();
    let (status, body) = get_text(&app, "/api/geo-stats?datasets=ds1", Some(&owner)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "owner geo-stats batch should succeed: {body}"
    );
    assert_eq!(
        element_count(&body),
        2,
        "owner must count both graphs' elements: {body}"
    );
}

// ─── browse_suggest ─────────────────────────────────────────────────────────

#[tokio::test]
async fn browse_suggest_scopes_to_readable_graphs() {
    let app = test_app(setup());

    // field=object surfaces the rdfs:label literals (the markers) of every
    // triple in the scoped graphs.
    let uri = "/api/browse/suggest?field=object&dataset=ds1&limit=200";
    let (status, body) = get_text(&app, uri, None).await;
    assert_eq!(status, StatusCode::OK, "suggest should succeed: {body}");
    assert!(
        body.contains(PUBLIC_MARKER),
        "public graph's values must be suggested to the viewer: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private graph's values must NOT be suggested to the viewer: {body}"
    );

    let owner = owner_token();
    let (status, body) = get_text(&app, uri, Some(&owner)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "owner suggest should succeed: {body}"
    );
    assert!(
        body.contains(PUBLIC_MARKER) && body.contains(PRIVATE_MARKER),
        "owner must be suggested both graphs' values: {body}"
    );
}

// ─── OGC API – Features items ─────────────────────────────────────────────────

#[tokio::test]
async fn ogc_items_hide_private_graph_from_viewer() {
    let app = test_app(setup());

    let uri = "/api/ogc/collections/ds1/items";
    let (status, body) = get_text(&app, uri, None).await;
    assert_eq!(status, StatusCode::OK, "OGC items should succeed: {body}");
    assert!(
        body.contains(PUBLIC_MARKER),
        "public feature must be visible to the viewer: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private feature must NOT leak to the viewer: {body}"
    );

    let owner = owner_token();
    let (status, body) = get_text(&app, uri, Some(&owner)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "owner OGC items should succeed: {body}"
    );
    assert!(
        body.contains(PUBLIC_MARKER) && body.contains(PRIVATE_MARKER),
        "owner must see both features: {body}"
    );
}

// ─── list_dataset_commits ─────────────────────────────────────────────────────

#[tokio::test]
async fn commits_scope_hides_private_graph_history_from_viewer() {
    use open_triplestore::commit_log::{record, CommitKind};

    let state = setup();
    // One commit touching only the public graph, one touching only the private
    // graph. The commit-log scope filters on the affected graph, so a viewer
    // scoped to readable (public) graphs must never see the private commit —
    // not its message, not the private graph IRI.
    record(
        &state.store,
        state.base_url.as_str(),
        CommitKind::GraphStore,
        PUBLIC_MARKER,
        Some("owner"),
        None,
        vec![PUB_GRAPH.to_string()],
        1,
        0,
        None,
    );
    record(
        &state.store,
        state.base_url.as_str(),
        CommitKind::GraphStore,
        PRIVATE_MARKER,
        Some("owner"),
        None,
        vec![PRIV_GRAPH.to_string()],
        1,
        0,
        None,
    );
    let app = test_app(state);

    let (status, body) = get_text(&app, "/api/datasets/ds1/commits", None).await;
    assert_eq!(status, StatusCode::OK, "commits should succeed: {body}");
    assert!(
        body.contains(PUBLIC_MARKER),
        "public graph's commit must be visible to the viewer: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private graph's commit message must NOT leak to the viewer: {body}"
    );
    assert!(
        !body.contains(PRIV_GRAPH),
        "private graph IRI must NOT leak through the commit log: {body}"
    );

    let owner = owner_token();
    let (status, body) = get_text(&app, "/api/datasets/ds1/commits", Some(&owner)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "owner commits should succeed: {body}"
    );
    assert!(
        body.contains(PUBLIC_MARKER) && body.contains(PRIVATE_MARKER),
        "owner must see both graphs' commit history: {body}"
    );
}

// ─── 3D Tiles content (gated on the geometry feature that mounts the routes) ──

#[cfg(feature = "geometry3d")]
#[tokio::test]
async fn tiles3d_content_excludes_private_features_from_viewer() {
    let app = test_app(setup());

    // The GLB embeds each feature's IRI as raw UTF-8 bytes (EXT_structural_metadata
    // STRING column), so the private element's IRI marker must be absent for a viewer.
    let uri = "/api/datasets/ds1/3dtiles/content.glb";
    let (status, glb) = get_bytes(&app, uri, None).await;
    assert_eq!(status, StatusCode::OK, "content.glb should succeed");
    let blob = String::from_utf8_lossy(&glb);
    assert!(
        blob.contains(PUBLIC_MARKER),
        "public feature's IRI must be in the viewer's GLB"
    );
    assert!(
        !blob.contains(PRIVATE_MARKER),
        "private feature's IRI must NOT leak into the viewer's GLB"
    );

    let owner = owner_token();
    let (status, glb) = get_bytes(&app, uri, Some(&owner)).await;
    assert_eq!(status, StatusCode::OK, "owner content.glb should succeed");
    let blob = String::from_utf8_lossy(&glb);
    assert!(
        blob.contains(PUBLIC_MARKER) && blob.contains(PRIVATE_MARKER),
        "owner's GLB must carry both features"
    );
}

// ─── Container export (gated on the ZIP feature that backs the export) ────────

#[cfg(feature = "asset-archive")]
#[tokio::test]
async fn container_export_excludes_nonpublic_assets_from_anonymous() {
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let mut state = test_state();
    // A real (local) object store so the export can actually read asset bytes;
    // the default no-op store fails every download.
    state.object_store =
        Arc::new(open_triplestore::storage::ObjectStore::local(dir.path().to_path_buf()).unwrap());

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
    state
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

    // A public asset and a non-public one; each file's bytes carry its marker.
    seed_asset(&state, "pub-asset", "public.txt", true, PUBLIC_MARKER).await;
    seed_asset(&state, "priv-asset", "secret.txt", false, PRIVATE_MARKER).await;

    let app = test_app(state);
    let uri = "/api/datasets/ds1/containers/export?profile=icdd";

    let (status, zip) = get_bytes(&app, uri, None).await;
    assert_eq!(status, StatusCode::OK, "anonymous export should succeed");
    assert!(
        zip_contains(&zip, PUBLIC_MARKER),
        "public asset must be in the anonymous export"
    );
    assert!(
        !zip_contains(&zip, PRIVATE_MARKER),
        "non-public asset must NOT be in the anonymous export"
    );

    let owner = owner_token();
    let (status, zip) = get_bytes(&app, uri, Some(&owner)).await;
    assert_eq!(status, StatusCode::OK, "owner export should succeed");
    assert!(
        zip_contains(&zip, PUBLIC_MARKER) && zip_contains(&zip, PRIVATE_MARKER),
        "owner export must carry both assets"
    );
}

/// Register an asset row and upload its bytes (containing `marker`) so the
/// export's `object_store.download` returns them.
#[cfg(feature = "asset-archive")]
async fn seed_asset(state: &AppState, id: &str, filename: &str, public: bool, marker: &str) {
    let s3_key = format!("datasets/ds1/{id}/{filename}");
    let body = format!("asset body {marker}");
    state
        .auth_db
        .create_asset(
            id,
            "ds1",
            filename,
            "text/plain",
            &s3_key,
            body.len() as i64,
            "owner",
            public,
            "",
        )
        .unwrap();
    state
        .object_store
        .upload(&s3_key, axum::body::Bytes::from(body), "text/plain")
        .await
        .unwrap();
}

/// True if any ZIP entry's name or (decompressed) contents contain `needle`.
#[cfg(feature = "asset-archive")]
fn zip_contains(bytes: &[u8], needle: &str) -> bool {
    use std::io::Read as _;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).unwrap();
        if f.name().contains(needle) {
            return true;
        }
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).ok();
        if String::from_utf8_lossy(&buf).contains(needle) {
            return true;
        }
    }
    false
}
