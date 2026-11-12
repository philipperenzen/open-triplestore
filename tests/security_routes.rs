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

use open_triplestore::auth::models::{OwnerType, ResourceRole, SystemRole, Visibility};
use open_triplestore::dataset_versions::{models::DatasetVersion, snapshot_as_version};
use serde_json::Value;

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
    let (state, record) = setup_state();
    (test_app(state), record)
}

/// [`setup`], with the state still in hand for a test that adds to it.
fn setup_state() -> (open_triplestore::server::AppState, DatasetVersion) {
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

    (state, record)
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

// ─── Version data-dump and diff endpoints (CB5, part 2) ──────────────────────
//
// The dataset-service and browse `?version=` paths above (CB5) filter private
// snapshot graphs. The version *data-dump* (`GET …/versions/:ver/data`) and
// *diff* (`GET …/versions/:ver/diff/:other`) handlers in
// `src/dataset_versions/handlers.rs` did not: they served every snapshot graph of
// the version to anyone who could read the dataset, so a viewer — anonymous, on a
// public dataset — could dump or diff a graph the owner flagged private through a
// pinned version. Both now map each snapshot back to its live source graph and
// drop the ones flagged private for a non-writer.

async fn get_auth(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(t) = token {
        b = b.header(axum::http::header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

/// CB5 — version data dump: an anonymous viewer of a public dataset gets the
/// public snapshot's triples but never the private snapshot's.
#[tokio::test]
async fn viewer_cannot_dump_private_snapshot_via_version_data() {
    let (app, _record) = setup();
    let (status, body) = get_auth(
        &app,
        &format!("/api/datasets/ds1/versions/{VERSION}/data?graph=all"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "version data dump should succeed: {body}"
    );
    assert!(
        body.contains(PUBLIC_MARKER),
        "public snapshot triple must be in the dump, body: {body}"
    );
    assert!(
        !body.contains(PRIVATE_MARKER),
        "private snapshot triple must NOT leak through the data dump, body: {body}"
    );
}

/// The writer who owns the dataset still dumps every snapshot graph, private
/// included — the filter narrows only non-writers.
#[tokio::test]
async fn owner_still_dumps_private_snapshot_via_version_data() {
    let (app, _record) = setup();
    let token = mint_token("owner", "owner", "user");
    let (status, body) = get_auth(
        &app,
        &format!("/api/datasets/ds1/versions/{VERSION}/data?graph=all"),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner dump should succeed: {body}");
    assert!(
        body.contains(PUBLIC_MARKER) && body.contains(PRIVATE_MARKER),
        "the owner must still see both snapshot graphs, body: {body}"
    );
}

/// CB5 — version diff: neither the rdf-patch triples nor the JSON graph list may
/// expose a private graph to an anonymous viewer.
#[tokio::test]
async fn viewer_diff_hides_private_snapshot_graphs() {
    let (app, _record) = setup();

    // rdf-patch carries the actual triples: the private marker must be absent.
    let (status, patch) = get_auth(
        &app,
        &format!("/api/datasets/ds1/versions/{VERSION}/diff/live?format=rdf-patch"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "diff (patch) should succeed: {patch}"
    );
    assert!(
        !patch.contains(PRIVATE_MARKER),
        "private triples must NOT appear in the rdf-patch diff, body: {patch}"
    );

    // The JSON diff lists a per-graph breakdown keyed by the live source graph;
    // the private source IRI must not appear there either.
    let (status, json) = get_auth(
        &app,
        &format!("/api/datasets/ds1/versions/{VERSION}/diff/live"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "diff (json) should succeed: {json}");
    assert!(
        !json.contains(PRIV_GRAPH),
        "the private source graph must NOT appear in the JSON diff, body: {json}"
    );

    // The owner sees the private graph in the breakdown (control).
    let token = mint_token("owner", "owner", "user");
    let (status, json) = get_auth(
        &app,
        &format!("/api/datasets/ds1/versions/{VERSION}/diff/live"),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner diff should succeed: {json}");
    assert!(
        json.contains(PRIV_GRAPH),
        "the owner must still see the private source graph in the diff, body: {json}"
    );
}

// ─── Padded version labels (CB5, part 3) ─────────────────────────────────────
//
// The dataset service trimmed `version` before resolving the snapshot graphs but
// handed the raw label to the private filter. The registry refuses a label with
// whitespace, so the filter found no version, withheld nothing, and a viewer read
// every snapshot graph — private ones included — through `?version=1.0.0%20`.
// Both lookups now use one trimmed label, and the filter refuses a pinned version
// it cannot read rather than withholding nothing.

/// `1.0.0` padded with whitespace, spelled as in a URL query or a form body
/// (where `+` is a space).
const PADDED_VERSIONS: [&str; 5] = [
    "1.0.0%20",
    "%201.0.0",
    "1.0.0+",
    "%091.0.0%0A",
    "%20%201.0.0%20%20",
];

/// CB5 — dataset-service `?version=` with a padded label: it still pins `1.0.0`
/// (the public snapshot graph is what gets listed), and the private snapshot
/// stays withheld from the viewer.
#[tokio::test]
async fn viewer_cannot_read_private_snapshot_via_padded_version_param() {
    let (app, record) = setup();
    let priv_snap = snapshot_iri_for(&record, PRIV_GRAPH);
    let pub_snap = snapshot_iri_for(&record, PUB_GRAPH);
    let select = url_encode("SELECT ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } }");
    let listing = url_encode("SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } }");

    for padded in PADDED_VERSIONS {
        let (status, body) = get(
            &app,
            &format!("/api/datasets/ds1/services/default/sparql?query={select}&version={padded}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "version={padded}: {body}");
        assert!(
            body.contains(PUBLIC_MARKER),
            "version={padded}: the public snapshot triple must be visible, body: {body}"
        );
        assert!(
            !body.contains(PRIVATE_MARKER),
            "version={padded}: the private snapshot triple must NOT leak, body: {body}"
        );

        let (status, body) = get(
            &app,
            &format!("/api/datasets/ds1/services/default/sparql?query={listing}&version={padded}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "version={padded}: {body}");
        assert!(
            body.contains(&pub_snap),
            "version={padded}: the pinned version's public snapshot must be listed, body: {body}"
        );
        assert!(
            !body.contains(&priv_snap),
            "version={padded}: the private snapshot graph must NOT be listed, body: {body}"
        );
    }
}

/// CB5 — the same through a POST form body, whose `version` overrides the URL's.
#[tokio::test]
async fn viewer_cannot_read_private_snapshot_via_padded_form_version() {
    let (app, _record) = setup();
    let select = url_encode("SELECT ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } }");

    for padded in PADDED_VERSIONS {
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/datasets/ds1/services/default/sparql")
            .header(
                axum::http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(Body::from(format!("query={select}&version={padded}")))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = body_text(resp.into_body()).await;
        assert_eq!(status, StatusCode::OK, "version={padded}: {body}");
        assert!(
            body.contains(PUBLIC_MARKER),
            "version={padded}: the public snapshot triple must be visible, body: {body}"
        );
        assert!(
            !body.contains(PRIVATE_MARKER),
            "version={padded}: the private snapshot triple must NOT leak, body: {body}"
        );
    }
}

// ─── Version records (CB5, part 4) ───────────────────────────────────────────
//
// The routes above withhold a private graph's *data*. The version record itself
// (`GET …/versions`, `GET …/versions/:ver`) was still returned whole: its
// `source_map` and `snapshot_graphs` name every graph the version covers, so a
// caller who may read the dataset but not write it — anonymous, on a public
// dataset — learned the IRIs of its private graphs and of their snapshots, and
// from the totals how many quads they hold. A non-writer now gets the record of
// the graphs they may read; writers and admins get it whole.

/// [`setup_state`] plus the accounts the audience tests sign in as: `viewer`
/// (a Viewer grant), `editor` (an Editor grant) and `adm` (an administrator),
/// beside the dataset's `owner`.
fn setup_with_accounts() -> (open_triplestore::server::AppState, DatasetVersion) {
    let (state, record) = setup_state();
    for (id, role) in [
        ("viewer", SystemRole::User),
        ("editor", SystemRole::User),
        ("adm", SystemRole::Admin),
    ] {
        state
            .auth_db
            .create_user(id, id, &format!("{id}@test.com"), "hash", role)
            .unwrap();
    }
    for (id, role) in [
        ("viewer", ResourceRole::Viewer),
        ("editor", ResourceRole::Editor),
    ] {
        state
            .auth_db
            .set_resource_grant("dataset", "ds1", "user", id, role, "owner")
            .unwrap();
    }
    (state, record)
}

/// `(label, bearer token, may write the dataset)` for every kind of caller.
fn callers() -> Vec<(&'static str, Option<String>, bool)> {
    vec![
        ("anonymous", None, false),
        (
            "viewer",
            Some(mint_token("viewer", "viewer", "user")),
            false,
        ),
        ("editor", Some(mint_token("editor", "editor", "user")), true),
        ("owner", Some(mint_token("owner", "owner", "user")), true),
        ("admin", Some(mint_token("adm", "adm", "admin")), true),
    ]
}

/// Version `1.0.0`'s record from both routes that serve it: the single-record
/// route, and its entry in the listing. Each as `(route, raw body, record)`.
async fn version_records(app: &Router, token: Option<&str>) -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    let uri = format!("/api/datasets/ds1/versions/{VERSION}");
    let (status, body) = get_auth(app, &uri, token).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    let record: Value = serde_json::from_str(&body).unwrap();
    out.push((uri, body, record));

    let uri = "/api/datasets/ds1/versions".to_string();
    let (status, body) = get_auth(app, &uri, token).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    let list: Value = serde_json::from_str(&body).unwrap();
    let record = list
        .as_array()
        .and_then(|l| l.iter().find(|v| v["version"] == VERSION))
        .unwrap_or_else(|| panic!("{uri} does not list {VERSION}: {body}"))
        .clone();
    out.push((uri, body, record));
    out
}

/// A JSON array of strings, sorted (the registry does not keep an order).
fn strings(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// CB5 — a non-writer's version record names neither the private graph nor its
/// snapshot, and its totals are those of the graphs it lists; a writer's record
/// is the registry's.
#[tokio::test]
async fn version_records_name_private_graphs_only_to_writers() {
    let (state, record) = setup_with_accounts();
    let app = test_app(state);
    let priv_snap = snapshot_iri_for(&record, PRIV_GRAPH);
    let pub_snap = snapshot_iri_for(&record, PUB_GRAPH);
    let pub_mapping = record
        .source_map
        .iter()
        .find(|m| m.source_graph == PUB_GRAPH)
        .unwrap();
    // The setup this test reads its expectations from.
    assert_eq!(record.quad_count, Some(2));
    assert_eq!(pub_mapping.quad_count, Some(1));
    assert!(record.content_hash.is_some() && pub_mapping.content_hash.is_some());
    assert_ne!(record.content_hash, pub_mapping.content_hash);

    for (who, token, writer) in callers() {
        for (uri, body, v) in version_records(&app, token.as_deref()).await {
            let ctx = format!("{who}, GET {uri}: {body}");
            let snapshots = strings(&v["snapshot_graphs"]);
            let sources = sorted(
                v["source_map"]
                    .as_array()
                    .unwrap_or_else(|| panic!("no source_map — {ctx}"))
                    .iter()
                    .map(|m| m["source_graph"].as_str().unwrap_or_default().to_string())
                    .collect(),
            );
            if writer {
                assert_eq!(
                    snapshots,
                    sorted(record.snapshot_graphs.clone()),
                    "a writer gets every snapshot graph — {ctx}"
                );
                assert_eq!(
                    sources,
                    sorted(vec![PUB_GRAPH.to_string(), PRIV_GRAPH.to_string()]),
                    "a writer gets the whole graph map — {ctx}"
                );
                assert_eq!(v["quad_count"], 2, "{ctx}");
                assert_eq!(
                    v["content_hash"].as_str(),
                    record.content_hash.as_deref(),
                    "{ctx}"
                );
            } else {
                assert!(
                    !body.contains(PRIV_GRAPH),
                    "the private graph's IRI must not reach a non-writer — {ctx}"
                );
                assert!(
                    !body.contains(&priv_snap),
                    "the private graph's snapshot IRI must not reach a non-writer — {ctx}"
                );
                assert_eq!(
                    snapshots,
                    vec![pub_snap.clone()],
                    "the public snapshot is still listed — {ctx}"
                );
                assert_eq!(
                    sources,
                    vec![PUB_GRAPH.to_string()],
                    "the public graph is still mapped — {ctx}"
                );
                // The totals are those of the listed graphs: the registry's
                // would give away the private graph's size, and (the content
                // hash being additive) a hash of its content.
                assert_eq!(v["quad_count"], 1, "{ctx}");
                assert_eq!(
                    v["content_hash"].as_str(),
                    pub_mapping.content_hash.as_deref(),
                    "{ctx}"
                );
            }
        }
    }
}

/// A dataset with no private graph: every caller gets the registry's record.
#[tokio::test]
async fn version_records_are_whole_when_no_graph_is_private() {
    let (state, record) = setup_with_accounts();
    state
        .auth_db
        .set_dataset_graph_private("ds1", PRIV_GRAPH, false)
        .unwrap();
    let app = test_app(state);

    for (who, token, _) in callers() {
        for (uri, body, v) in version_records(&app, token.as_deref()).await {
            let ctx = format!("{who}, GET {uri}: {body}");
            assert_eq!(
                strings(&v["snapshot_graphs"]),
                sorted(record.snapshot_graphs.clone()),
                "{ctx}"
            );
            assert_eq!(v["source_map"].as_array().map(Vec::len), Some(2), "{ctx}");
            assert_eq!(v["quad_count"], 2, "{ctx}");
            assert_eq!(
                v["content_hash"].as_str(),
                record.content_hash.as_deref(),
                "{ctx}"
            );
        }
    }
}

/// CB5 — `GET …/branches`: a tip's `ahead` / `behind` against the published
/// version count only the graphs the caller may read. Here the only change
/// since `1.0.0` is one quad in the private graph.
#[tokio::test]
async fn branch_counts_leave_out_private_graphs_for_non_writers() {
    let (state, _record) = setup_with_accounts();
    open_triplestore::dataset_versions::registry::update_latest_published(
        &state.store,
        state.base_url.as_str(),
        "ds1",
        VERSION,
    )
    .unwrap();
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PRIV_GRAPH}> {{ <http://example.org/s3> <http://example.org/p> \"later\" }} }}"
        ))
        .unwrap();
    // created_at orders the tips; keep the two cuts apart.
    std::thread::sleep(std::time::Duration::from_millis(5));
    snapshot_as_version(
        &state.store,
        state.base_url.as_str(),
        "ds1",
        "1.1.0",
        &[PUB_GRAPH.to_string(), PRIV_GRAPH.to_string()],
        open_triplestore::dataset_versions::models::VersionStatus::Draft,
        None,
        None,
    )
    .expect("second snapshot should succeed");
    let app = test_app(state);

    for (who, token, writer) in callers() {
        let (status, body) = get_auth(&app, "/api/datasets/ds1/branches", token.as_deref()).await;
        assert_eq!(status, StatusCode::OK, "{who}: {body}");
        let branches: Value = serde_json::from_str(&body).unwrap();
        let main = branches
            .as_array()
            .and_then(|b| b.iter().find(|b| b["branch"] == "main"))
            .unwrap_or_else(|| panic!("{who}: no main branch in {body}"));
        assert_eq!(main["tip_version"], "1.1.0", "{who}: {body}");
        let expected = if writer { 1 } else { 0 };
        assert_eq!(
            main["ahead"], expected,
            "{who}: `ahead` counts the graphs the caller may read: {body}"
        );
        assert_eq!(main["behind"], 0, "{who}: {body}");
    }
}
