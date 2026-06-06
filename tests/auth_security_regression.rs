//! Cross-tenant authorization regression tests — CI gate (`cargo test … security`).
//!
//! Locks in the dataset-graph write/registration boundary and the dataset-create
//! owner gate against the cross-tenant IDOR vectors found in the 2026-06 review
//! follow-up:
//!   * **graph-claim read escalation** via `POST /api/datasets/:id/graphs` — a
//!     writer attaching another tenant's private graph IRI to their own dataset,
//!     which `get_accessible_graph_iris` would then expose to them;
//!   * **foreign-graph write** via `POST /api/datasets/:id/mappings/execute` — RML
//!     `?graph=` / `rml:graphMap` targeting another tenant's graph;
//!   * **owner forgery** on dataset creation.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{admin_state, mint_token, test_app};
use open_triplestore::auth::dataset_graph::authorize_dataset_graph_target;
use open_triplestore::auth::db::AuthDb;
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use tower::ServiceExt as _;

const BASE: &str = "http://localhost:7878";

fn make_user(state: &AppState, id: &str) -> String {
    state
        .auth_db
        .create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
        .unwrap();
    mint_token(id, id, "user")
}

fn make_dataset(state: &AppState, id: &str, owner: &str) {
    state
        .auth_db
        .create_dataset(
            id,
            id,
            None,
            OwnerType::User,
            owner,
            Visibility::Private,
            None,
        )
        .unwrap();
}

fn post_json(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

// ───────────────── HIGH-1: graph-claim cross-tenant read escalation ─────────────────

#[tokio::test]
async fn cannot_register_another_datasets_graph_security() {
    let (state, _admin) = admin_state();
    make_user(&state, "victim");
    make_dataset(&state, "victimds", "victim");
    let victim_graph = "http://victim.example/private-data";
    // The victim owns this graph (DB-level setup, as the owner/admin path would).
    state
        .auth_db
        .add_dataset_graph("victimds", victim_graph)
        .unwrap();

    let attacker = make_user(&state, "attacker");
    make_dataset(&state, "attackerds", "attacker");

    // Attacker (a writer of their OWN dataset) tries to attach the victim's graph.
    let resp = test_app(state.clone())
        .oneshot(post_json(
            "/api/datasets/attackerds/graphs",
            &attacker,
            serde_json::json!({ "graph_iri": victim_graph }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "registering another dataset's graph must be rejected"
    );

    // …and it must NOT have been attached to the attacker's dataset.
    let attacker_graphs = state.auth_db.list_dataset_graphs("attackerds").unwrap();
    assert!(
        !attacker_graphs.iter().any(|g| g == victim_graph),
        "victim graph must not be registered to attacker dataset: {attacker_graphs:?}"
    );
}

#[tokio::test]
async fn cannot_register_foreign_reserved_namespace_security() {
    let (state, _admin) = admin_state();
    make_user(&state, "victim2");
    make_dataset(&state, "victimds2", "victim2");
    let attacker = make_user(&state, "attacker2");
    make_dataset(&state, "attackerds2", "attacker2");

    let base_graph = format!("{BASE}/dataset/victimds2/instances");
    for foreign in [
        "urn:dataset:victimds2:shapes",
        base_graph.as_str(),
        "urn:system:metadata:dataset:victimds2",
    ] {
        let resp = test_app(state.clone())
            .oneshot(post_json(
                "/api/datasets/attackerds2/graphs",
                &attacker,
                serde_json::json!({ "graph_iri": foreign }),
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "foreign/reserved graph <{foreign}> must be rejected"
        );
    }
}

#[tokio::test]
async fn can_register_own_and_unclaimed_graphs_security() {
    let (state, _admin) = admin_state();
    let owner = make_user(&state, "owner3");
    make_dataset(&state, "ds3", "owner3");

    // Own namespaced graphs (both schemes) and an unclaimed external graph are OK —
    // the boundary must not break legitimate registration.
    for g in [
        format!("{BASE}/dataset/ds3/instances"),
        "urn:dataset:ds3:rml-output".to_string(),
        "http://my.example/new-graph".to_string(),
    ] {
        let resp = test_app(state.clone())
            .oneshot(post_json(
                "/api/datasets/ds3/graphs",
                &owner,
                serde_json::json!({ "graph_iri": g }),
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "legitimate graph <{g}> must be allowed"
        );
    }
}

// ───────────────────────── HIGH-2: RML foreign-graph write ─────────────────────────

#[tokio::test]
async fn rml_execute_cannot_target_foreign_graph_security() {
    let (state, _admin) = admin_state();
    make_user(&state, "rvictim");
    make_dataset(&state, "rvictimds", "rvictim");
    let attacker = make_user(&state, "rattacker");
    make_dataset(&state, "rattackerds", "rattacker");

    // A foreign `?graph=` target is rejected before any mapping work (403).
    let uri = format!(
        "/api/datasets/rattackerds/mappings/execute?graph={}",
        common::url_encode("urn:dataset:rvictimds:rml-output")
    );
    let req = Request::builder()
        .method("POST")
        .uri(&uri)
        .header("Authorization", format!("Bearer {attacker}"))
        .header("content-type", "multipart/form-data; boundary=X")
        .body(Body::from("--X--\r\n"))
        .unwrap();
    let resp = test_app(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "RML output into another tenant's graph must be rejected"
    );
}

// ─────────────────── dataset-create owner forgery (can_act_as_owner) ───────────────────

#[tokio::test]
async fn cannot_forge_dataset_owner_security() {
    let (state, _admin) = admin_state();
    make_user(&state, "alice");
    let bob = make_user(&state, "bob");

    // Bob tries to create a dataset OWNED BY alice → rejected.
    let resp = test_app(state.clone())
        .oneshot(post_json(
            "/api/datasets",
            &bob,
            serde_json::json!({ "name": "Forged", "owner_type": "user", "owner_id": "alice" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "attributing ownership to another user must be rejected"
    );

    // Bob creating a dataset owned by himself → allowed.
    let resp = test_app(state.clone())
        .oneshot(post_json(
            "/api/datasets",
            &bob,
            serde_json::json!({ "name": "Mine", "owner_type": "user", "owner_id": "bob" }),
        ))
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "a self-owned dataset must be creatable, got {}",
        resp.status()
    );
}

// ───────────────────── the boundary helper directly (invariant) ─────────────────────

#[test]
fn authorize_dataset_graph_target_invariants_security() {
    let db = AuthDb::in_memory().unwrap();
    db.create_user("u", "u", "u@t.com", "h", SystemRole::User)
        .unwrap();
    db.create_dataset(
        "mine",
        "mine",
        None,
        OwnerType::User,
        "u",
        Visibility::Private,
        None,
    )
    .unwrap();
    db.create_dataset(
        "other",
        "other",
        None,
        OwnerType::User,
        "u",
        Visibility::Private,
        None,
    )
    .unwrap();
    db.add_dataset_graph("other", "http://shared.example/claimed")
        .unwrap();

    let ok = |g: &str| authorize_dataset_graph_target(&db, BASE, "mine", g).is_ok();

    // Own namespaces (both schemes) and unclaimed external graphs are allowed.
    assert!(ok(&format!("{BASE}/dataset/mine/instances")));
    assert!(ok("urn:dataset:mine:shapes"));
    assert!(ok("http://my.example/g"));
    // Foreign reserved namespaces are rejected.
    assert!(!ok("urn:dataset:other:shapes"));
    assert!(!ok(&format!("{BASE}/dataset/other/instances")));
    assert!(!ok("urn:system:metadata:dataset:other"));
    // A graph already claimed by another dataset is rejected.
    assert!(!ok("http://shared.example/claimed"));
    // Prefix-collision guard: `mine` must not match `mine2`.
    assert!(!ok(&format!("{BASE}/dataset/mine2/instances")));
    assert!(!ok("urn:dataset:mine2:shapes"));
}
