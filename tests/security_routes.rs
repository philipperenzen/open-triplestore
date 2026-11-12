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

// ─── A private graph detached from the dataset (CB5, part 5) ─────────────────
//
// Every filter above asks whether a snapshot's source graph is flagged private
// *now*. The flag was a column of the graph's registration, and detaching the
// graph deleted the registration: from then on every version that held a
// snapshot of the graph served it to anyone who could read the dataset. A
// private graph now stays hidden in the versions that cover it after it is
// detached.

const LATER_MARKER: &str = "PRIVATE_TRIPLE_LATER_MARKER";
const LATER_VERSION: &str = "1.1.0";

/// [`setup_state`], plus what the detach tests need: the accounts they sign in
/// as beside the dataset's `owner` — `viewer` (a Viewer grant), `editor` (an
/// Editor grant) and `adm` (an administrator) — and a second version `1.1.0`,
/// cut after one more quad went into the private graph. Returns the state and
/// the records of `1.0.0` and `1.1.0`.
fn detach_state() -> (
    open_triplestore::server::AppState,
    DatasetVersion,
    DatasetVersion,
) {
    use open_triplestore::auth::models::{ResourceRole, SystemRole};
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
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PRIV_GRAPH}> {{ <http://example.org/s3> <http://example.org/p> \"{LATER_MARKER}\" }} }}"
        ))
        .unwrap();
    // created_at orders the versions; keep the two cuts apart.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let later = snapshot_as_version(
        &state.store,
        state.base_url.as_str(),
        "ds1",
        LATER_VERSION,
        &[PUB_GRAPH.to_string(), PRIV_GRAPH.to_string()],
        open_triplestore::dataset_versions::models::VersionStatus::Draft,
        None,
        None,
    )
    .expect("second snapshot should succeed");
    (state, record, later)
}

/// `(label, bearer token, may write the dataset)` for every kind of caller.
fn audience() -> Vec<(&'static str, Option<String>, bool)> {
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

/// A bearer token for one of the accounts [`detach_state`] makes.
fn token_of(who: &str) -> String {
    match who {
        "adm" => mint_token("adm", "adm", "admin"),
        other => mint_token(other, other, "user"),
    }
}

/// A request with a JSON body as `who`.
async fn send_json(
    app: &Router,
    method: Method,
    uri: &str,
    who: &str,
    body: serde_json::Value,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", token_of(who)),
        )
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

/// Detach `graph` from `ds1` as `who`, through the route a writer uses.
async fn detach(app: &Router, who: &str, graph: &str) {
    let (status, body) = send_json(
        app,
        Method::DELETE,
        "/api/datasets/ds1/graphs",
        who,
        serde_json::json!({ "graph_iri": graph }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{who} detaches <{graph}>: {body}"
    );
}

/// Whether the store holds a quad in the live graph `graph`.
fn stored(state: &open_triplestore::server::AppState, graph: &str) -> bool {
    use oxigraph::sparql::QueryResults;
    match state
        .store
        .query(&format!("ASK {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}"))
        .unwrap()
    {
        QueryResults::Boolean(b) => b,
        _ => panic!("ASK returns a boolean"),
    }
}

/// One read of a version, and what it may show of the graph under test.
struct VersionRead {
    /// What the route is, for the report.
    name: &'static str,
    uri: String,
    /// In every caller's response: the route works and still serves the
    /// other graph.
    always: Vec<String>,
    /// Only in the response of a caller who may see the graph under test.
    guarded: Vec<String>,
    /// The route refuses (404) a caller who may not see the graph.
    refuses: bool,
}

/// Every version read of `ds1` that serves the data or the name of a covered
/// graph, set up for a graph whose snapshot holds `markers` and for `other`,
/// a marker of a graph every caller reads.
fn version_reads(
    record: &DatasetVersion,
    source: &str,
    markers: (&str, &str),
    other: &str,
) -> Vec<VersionRead> {
    let (first, later) = markers;
    let snap = snapshot_iri_for(record, source);
    let select = url_encode("SELECT ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } }");
    let listing = url_encode("SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } }");
    let s = |v: &str| v.to_string();
    vec![
        VersionRead {
            name: "version data",
            uri: format!("/api/datasets/ds1/versions/{VERSION}/data?graph=all"),
            always: vec![s(other)],
            guarded: vec![s(first), s(source)],
            refuses: false,
        },
        VersionRead {
            name: "later version data",
            uri: format!("/api/datasets/ds1/versions/{LATER_VERSION}/data?graph=all"),
            always: vec![s(other)],
            guarded: vec![s(first), s(later), s(source)],
            refuses: false,
        },
        VersionRead {
            name: "diff between versions (json)",
            uri: format!("/api/datasets/ds1/versions/{VERSION}/diff/{LATER_VERSION}"),
            always: vec![],
            guarded: vec![s(source)],
            refuses: false,
        },
        VersionRead {
            name: "diff between versions (rdf-patch)",
            uri: format!(
                "/api/datasets/ds1/versions/{VERSION}/diff/{LATER_VERSION}?format=rdf-patch"
            ),
            always: vec![],
            guarded: vec![s(later), s(source)],
            refuses: false,
        },
        VersionRead {
            name: "diff against live (json)",
            uri: format!("/api/datasets/ds1/versions/{VERSION}/diff/live"),
            always: vec![],
            guarded: vec![s(source)],
            refuses: false,
        },
        VersionRead {
            name: "dataset service ?version=",
            uri: format!(
                "/api/datasets/ds1/services/default/sparql?query={select}&version={VERSION}"
            ),
            always: vec![s(other)],
            guarded: vec![s(first)],
            refuses: false,
        },
        VersionRead {
            name: "dataset service ?version= graph listing",
            uri: format!(
                "/api/datasets/ds1/services/default/sparql?query={listing}&version={VERSION}"
            ),
            always: vec![],
            guarded: vec![snap.clone()],
            refuses: false,
        },
        VersionRead {
            name: "browse versions=",
            uri: format!("/api/browse/triples?dataset_id=ds1&versions=ds1:{VERSION}&limit=100"),
            always: vec![s(other)],
            guarded: vec![s(first)],
            refuses: false,
        },
        VersionRead {
            name: "browse graph= drill-down",
            uri: format!(
                "/api/browse/triples?graph={}&versions=ds1:{VERSION}&limit=100",
                url_encode(&snap)
            ),
            always: vec![],
            guarded: vec![s(first)],
            refuses: true,
        },
    ]
}

/// Run `reads` as every caller in [`audience`] and report each response that
/// is not what that caller should get: `hidden` says whether the graph under
/// test is one only the dataset's writers may see. Empty when all is as it
/// should be.
async fn audience_findings(app: &Router, reads: &[VersionRead], hidden: bool) -> Vec<String> {
    let mut findings = Vec::new();
    for (who, token, writer) in audience() {
        let sees = writer || !hidden;
        for read in reads {
            let (status, body) = get_auth(app, &read.uri, token.as_deref()).await;
            let at = format!("{who}, {}", read.name);
            if read.refuses && !sees {
                if status != StatusCode::NOT_FOUND {
                    findings.push(format!("{at}: expected 404, got {status}"));
                }
            } else if status != StatusCode::OK {
                findings.push(format!("{at}: expected 200, got {status}: {body}"));
                continue;
            }
            for needle in &read.always {
                if !body.contains(needle.as_str()) && !(read.refuses && !sees) {
                    findings.push(format!("{at}: `{needle}` is missing"));
                }
            }
            for needle in &read.guarded {
                match (sees, body.contains(needle.as_str())) {
                    (false, true) => findings.push(format!("{at}: LEAK of `{needle}`")),
                    (true, false) => findings.push(format!("{at}: `{needle}` is missing")),
                    _ => {}
                }
            }
        }
    }
    findings
}

/// CB5 — a private graph that is detached from its dataset stays hidden, in
/// every version that covers it, from callers who may not write the dataset.
/// Writers and admins still read it there.
///
/// Detached by the owner, the stored graph stays (the dataset did not create
/// it); detached by an administrator it is deleted with the registration. The
/// versions hold their own copies either way.
#[tokio::test]
async fn detached_private_graph_stays_hidden_in_versions() {
    for (by, kept) in [("owner", true), ("adm", false)] {
        let (state, record, _later) = detach_state();
        let app = test_app(state.clone());
        let reads = version_reads(
            &record,
            PRIV_GRAPH,
            (PRIVATE_MARKER, LATER_MARKER),
            PUBLIC_MARKER,
        );

        // Control: with the graph attached and private, every read is as it
        // should be — so a finding below is the detach's doing.
        let before = audience_findings(&app, &reads, true).await;
        assert!(
            before.is_empty(),
            "before the detach:\n{}",
            before.join("\n")
        );

        detach(&app, by, PRIV_GRAPH).await;
        assert_eq!(
            stored(&state, PRIV_GRAPH),
            kept,
            "the stored graph after a detach by {by}"
        );

        let after = audience_findings(&app, &reads, true).await;
        assert!(
            after.is_empty(),
            "after the detach by {by}:\n{}",
            after.join("\n")
        );
    }
}

/// A public graph that is detached stays readable in the versions that cover
/// it: only a graph that was private when it left is hidden there.
#[tokio::test]
async fn detached_public_graph_stays_readable_in_versions() {
    let (state, record, _later) = detach_state();
    state
        .auth_db
        .set_dataset_graph_private("ds1", PRIV_GRAPH, false)
        .unwrap();
    let app = test_app(state);
    let reads = version_reads(
        &record,
        PRIV_GRAPH,
        (PRIVATE_MARKER, LATER_MARKER),
        PUBLIC_MARKER,
    );
    detach(&app, "owner", PRIV_GRAPH).await;
    let after = audience_findings(&app, &reads, false).await;
    assert!(after.is_empty(), "{}", after.join("\n"));
}

/// What each caller gets of the *live* dataset: the graph listing and a query
/// on the dataset's service. Reports whatever is not as it should be for a
/// private graph holding `marker`.
async fn live_findings(app: &Router, graph: &str, marker: &str) -> Vec<String> {
    let select = url_encode("SELECT ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } }");
    let mut findings = Vec::new();
    for (who, token, writer) in audience() {
        for (name, uri, needle) in [
            (
                "graph listing",
                "/api/datasets/ds1/graphs".to_string(),
                graph,
            ),
            (
                "live dataset service",
                format!("/api/datasets/ds1/services/default/sparql?query={select}"),
                marker,
            ),
        ] {
            let (status, body) = get_auth(app, &uri, token.as_deref()).await;
            if status != StatusCode::OK {
                findings.push(format!("{who}, {name}: expected 200, got {status}: {body}"));
                continue;
            }
            match (writer, body.contains(needle)) {
                (false, true) => findings.push(format!("{who}, {name}: LEAK of `{needle}`")),
                (true, false) => findings.push(format!("{who}, {name}: `{needle}` is missing")),
                _ => {}
            }
        }
    }
    findings
}

/// Restoring a version registers the graphs it covers to the dataset again.
/// A graph that was private when it was detached comes back private: the
/// restore must not publish it.
#[tokio::test]
async fn restore_brings_a_detached_private_graph_back_private() {
    let (state, record, _later) = detach_state();
    let app = test_app(state.clone());
    // An administrator's detach deletes the stored graph with the registration.
    detach(&app, "adm", PRIV_GRAPH).await;
    assert!(!stored(&state, PRIV_GRAPH));

    let (status, body) = send_json(
        &app,
        Method::POST,
        &format!("/api/datasets/ds1/versions/{VERSION}/restore"),
        "owner",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "restore: {body}");
    assert!(
        stored(&state, PRIV_GRAPH),
        "the restore wrote the graph back: {body}"
    );
    assert!(
        state.auth_db.dataset_has_graph("ds1", PRIV_GRAPH).unwrap(),
        "the restore registered the graph again: {body}"
    );

    let live = live_findings(&app, PRIV_GRAPH, PRIVATE_MARKER).await;
    assert!(live.is_empty(), "after the restore:\n{}", live.join("\n"));
    let reads = version_reads(
        &record,
        PRIV_GRAPH,
        (PRIVATE_MARKER, LATER_MARKER),
        PUBLIC_MARKER,
    );
    let versions = audience_findings(&app, &reads, true).await;
    assert!(
        versions.is_empty(),
        "after the restore:\n{}",
        versions.join("\n")
    );
}

/// Attached again under the same IRI, the graph is private again, so the
/// versions keep hiding it. What publishes it — in the versions too, as for a
/// graph that never left — is a writer making it public; and a graph that is
/// public when it leaves stays readable in its versions.
#[tokio::test]
async fn reattached_graph_is_private_until_a_writer_makes_it_public() {
    let (state, record, _later) = detach_state();
    let app = test_app(state.clone());
    let reads = version_reads(
        &record,
        PRIV_GRAPH,
        (PRIVATE_MARKER, LATER_MARKER),
        PUBLIC_MARKER,
    );
    let flag = || {
        state
            .auth_db
            .list_dataset_graph_entries("ds1")
            .unwrap()
            .into_iter()
            .find(|e| e.graph_iri == PRIV_GRAPH)
            .map(|e| e.private)
    };

    detach(&app, "adm", PRIV_GRAPH).await;
    assert_eq!(flag(), None, "detached");
    let (status, body) = send_json(
        &app,
        Method::POST,
        "/api/datasets/ds1/graphs",
        "owner",
        serde_json::json!({ "graph_iri": PRIV_GRAPH }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "attach again: {body}");
    assert_eq!(flag(), Some(true), "attached again, the graph is private");
    let findings = audience_findings(&app, &reads, true).await;
    assert!(
        findings.is_empty(),
        "attached again:\n{}",
        findings.join("\n")
    );
    // The graph list shows it to writers only.
    for (who, token, writer) in audience() {
        let (status, body) = get_auth(&app, "/api/datasets/ds1/graphs", token.as_deref()).await;
        assert_eq!(status, StatusCode::OK, "{who}: {body}");
        assert_eq!(body.contains(PRIV_GRAPH), writer, "{who}: {body}");
    }

    let (status, body) = send_json(
        &app,
        Method::PATCH,
        "/api/datasets/ds1/graphs",
        "owner",
        serde_json::json!({ "graph_iri": PRIV_GRAPH, "private": false }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "make public: {body}");
    assert_eq!(flag(), Some(false));
    let findings = audience_findings(&app, &reads, false).await;
    assert!(findings.is_empty(), "made public:\n{}", findings.join("\n"));

    // Public when it leaves: nothing is on record, the versions stay readable,
    // and the next registration is public as well.
    detach(&app, "adm", PRIV_GRAPH).await;
    let findings = audience_findings(&app, &reads, false).await;
    assert!(
        findings.is_empty(),
        "detached while public:\n{}",
        findings.join("\n")
    );
    state.auth_db.add_dataset_graph("ds1", PRIV_GRAPH).unwrap();
    assert_eq!(flag(), Some(false));
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
