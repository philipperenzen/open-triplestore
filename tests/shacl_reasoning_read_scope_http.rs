//! Read-boundary regression tests for two SHACL/reasoning endpoints in
//! `src/server/routes.rs` that used to reach a caller-named or a dataset's
//! private graph on a read-level (or no) check:
//!
//! * **V13** `GET /api/shacl/detect-shapes?graph=<iri>` did NO read check on the
//!   caller-named graph, so any signed-in principal could learn the SHACL shape
//!   count of any graph in the store — another tenant's private shapes graph or
//!   a `urn:system:*` graph included. It now applies the same gate as
//!   `shaclc_serialize` (admins bypass, since that gate denies `urn:system:*`
//!   and unregistered graphs even to admins).
//! * **V14** `POST /api/reasoning/materialize` with a `dataset` checked only
//!   `can_access_dataset`, then reasoned over the dataset's whole layer — its
//!   private graphs included — and wrote the consequences into a caller-chosen
//!   target the caller can read, laundering private triples out. The reasoning
//!   source set is now filtered to graphs the caller may read.
//!
//! The fixture is a *public* dataset (so a plain viewer, and even an anonymous
//! caller, can access it) carrying one public and one private graph. The
//! attacker in the assertions is `mallory`, a signed-in user who is only a
//! viewer of that dataset; the owner and an admin are the positive controls.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};

const PUB_GRAPH: &str = "http://example.org/g/pub";
const PRIV_GRAPH: &str = "http://example.org/g/priv";
const TARGET_GRAPH: &str = "http://example.org/g/target";
// A distinctively-named subject that lives ONLY in the private graph, so its
// appearance anywhere the attacker can read is proof the private graph leaked.
const SECRET_SUBJECT: &str = "http://example.org/secretInstance";

const NODESHAPE: &str = "http://www.w3.org/ns/shacl#NodeShape";
const SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";

/// A public dataset `victim` (owner `owner`) with a public graph and a private
/// graph, each holding a SHACL NodeShape (so `detect-shapes` counts >0) and an
/// `rdf:type`/`rdfs:subClassOf` pair (so RDFS reasoning has something to
/// entail). `mallory` is a signed-in viewer; both `owner` and `mallory` are
/// granted write on `TARGET_GRAPH` so a materialize run can reach the source
/// scoping past the target-write gate. Returns `(state, admin, owner, mallory)`
/// tokens.
fn setup() -> (open_triplestore::server::AppState, String, String, String) {
    let (state, admin_token) = admin_state();

    for id in ["owner", "mallory"] {
        state
            .auth_db
            .create_user(id, id, &format!("{id}@test.com"), "hash", SystemRole::User)
            .unwrap();
    }
    let owner_token = mint_token("owner", "owner", "user");
    let mallory_token = mint_token("mallory", "mallory", "user");

    let ds = state
        .auth_db
        .create_dataset(
            "victim",
            "Victim",
            None,
            OwnerType::User,
            "owner",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph(&ds.id, PUB_GRAPH).unwrap();
    state.auth_db.add_dataset_graph(&ds.id, PRIV_GRAPH).unwrap();
    state
        .auth_db
        .set_dataset_graph_private(&ds.id, PRIV_GRAPH, true)
        .unwrap();

    // Public graph: a shape + a public class hierarchy.
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PUB_GRAPH}> {{ \
                <http://example.org/shapePub> a <{NODESHAPE}> . \
                <http://example.org/pubInstance> a <http://example.org/PubClass> . \
                <http://example.org/PubClass> <{SUBCLASS_OF}> <http://example.org/PubSuper> . \
             }} }}"
        ))
        .unwrap();
    // Private graph: a shape + a class hierarchy over the secret subject.
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{PRIV_GRAPH}> {{ \
                <http://example.org/shapePriv> a <{NODESHAPE}> . \
                <{SECRET_SUBJECT}> a <http://example.org/SecretClass> . \
                <http://example.org/SecretClass> <{SUBCLASS_OF}> <http://example.org/SecretSuper> . \
             }} }}"
        ))
        .unwrap();

    // Both principals may write the materialize target (a graph outside the
    // victim dataset); the target-write gate is not what these tests exercise.
    state
        .auth_db
        .grant_graph_permission("acl-t-owner", TARGET_GRAPH, "user", "owner", "write", "adm")
        .unwrap();
    state
        .auth_db
        .grant_graph_permission(
            "acl-t-mallory",
            TARGET_GRAPH,
            "user",
            "mallory",
            "write",
            "adm",
        )
        .unwrap();

    (state, admin_token, owner_token, mallory_token)
}

async fn get(app: &Router, uri: &str, token: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

async fn post_json(app: &Router, uri: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

fn detect_uri(graph: &str) -> String {
    format!("/api/shacl/detect-shapes?graph={}", url_encode(graph))
}

// ─── V13: detect-shapes read boundary ────────────────────────────────────────

/// A signed-in viewer may not learn the shape count of the dataset's *private*
/// graph, while the owner and an admin still can.
#[tokio::test]
async fn detect_shapes_hides_private_graph_from_viewer_but_not_owner() {
    let (state, admin, owner, mallory) = setup();
    let app = test_app(state);

    let (status, body) = get(&app, &detect_uri(PRIV_GRAPH), &mallory).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a viewer must not probe the private graph's shape count: {body}"
    );

    let (status, body) = get(&app, &detect_uri(PRIV_GRAPH), &owner).await;
    assert_eq!(status, StatusCode::OK, "owner may probe: {body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["shapes_detected"], json!(true), "{v}");
    assert!(v["shape_count"].as_u64().unwrap() >= 1, "{v}");

    let (status, body) = get(&app, &detect_uri(PRIV_GRAPH), &admin).await;
    assert_eq!(status, StatusCode::OK, "admin bypass may probe: {body}");
}

/// The gate does not over-block: a viewer may still probe a graph they may read
/// (the dataset's *public* graph).
#[tokio::test]
async fn detect_shapes_allows_viewer_on_readable_graph() {
    let (state, _admin, _owner, mallory) = setup();
    let app = test_app(state);

    let (status, body) = get(&app, &detect_uri(PUB_GRAPH), &mallory).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "viewer may probe a readable graph: {body}"
    );
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["shapes_detected"], json!(true), "{v}");
}

/// A `urn:system:*` graph is denied to a signed-in non-admin (the gate denies
/// system graphs even before ACLs are consulted).
#[tokio::test]
async fn detect_shapes_denies_system_graph_to_non_admin() {
    let (state, _admin, _owner, mallory) = setup();
    let app = test_app(state);

    let (status, body) = get(&app, &detect_uri("urn:system:sources"), &mallory).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a signed-in non-admin must not probe a system graph: {body}"
    );
}

// ─── V14: reasoning materialize read boundary ────────────────────────────────

/// Query the materialize target directly (unscoped, in-process) for any triple
/// about the secret subject that lives only in the private graph.
fn target_mentions_secret(state: &open_triplestore::server::AppState) -> bool {
    matches!(
        state.store.query(&format!(
            "ASK {{ GRAPH <{TARGET_GRAPH}> {{ <{SECRET_SUBJECT}> ?p ?o }} }}"
        )),
        Ok(QueryResults::Boolean(true))
    )
}

/// A viewer materializing the dataset must reason over only the graphs they may
/// read: the private graph is neither listed as a source nor entailed into the
/// (readable) target.
#[tokio::test]
async fn viewer_cannot_materialize_private_dataset_graph() {
    let (state, _admin, _owner, mallory) = setup();
    let app = test_app(state.clone());

    let (status, body) = post_json(
        &app,
        "/api/reasoning/materialize",
        &mallory,
        json!({ "regime": "rdfs", "dataset": "victim", "target_graph": TARGET_GRAPH }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "materialize should run: {body}");

    let sources: Vec<String> = body["sources"]
        .as_array()
        .expect("sources array")
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    assert!(
        sources.contains(&PUB_GRAPH.to_string()),
        "the readable public graph is a source: {sources:?}"
    );
    assert!(
        !sources.contains(&PRIV_GRAPH.to_string()),
        "the private graph must NOT be a reasoning source for a viewer: {sources:?}"
    );
    assert!(
        !target_mentions_secret(&state),
        "no private-graph triple may be entailed into the target the viewer can read"
    );
}

/// The owner (a writer, who may read the whole dataset) still reasons over the
/// private graph — the fix scopes to read access, it does not disable dataset
/// reasoning.
#[tokio::test]
async fn owner_materialize_includes_private_dataset_graph() {
    let (state, _admin, owner, _mallory) = setup();
    let app = test_app(state.clone());

    let (status, body) = post_json(
        &app,
        "/api/reasoning/materialize",
        &owner,
        json!({ "regime": "rdfs", "dataset": "victim", "target_graph": TARGET_GRAPH }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "materialize should run: {body}");

    let sources: Vec<String> = body["sources"]
        .as_array()
        .expect("sources array")
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    assert!(
        sources.contains(&PRIV_GRAPH.to_string()),
        "the owner reasons over the private graph too: {sources:?}"
    );
}
