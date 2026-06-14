//! HTTP-level regression tests for auth-handler authorization fixes.
//!
//! Covers:
//!   * [S3]  DELETE /api/datasets/:id requires *manage* (not merely write), so a
//!           plain Editor is refused.
//!   * [CB14] PUT/DELETE /api/organisations/:org/groups/:group reject a group that
//!           belongs to a *different* org (cross-org path) with 404.
//!   * [CB3] PUT /api/datasets/:id/shacl rejects a `shapes_graph_iri` that points
//!           at another dataset's namespace for a non-admin caller.
//!
//! Driven through the real Axum router via `tower::ServiceExt::oneshot` (no socket).

mod common;
use common::*;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use open_triplestore::auth::models::{OwnerType, Role, SystemRole, Visibility};
use tower::ServiceExt as _;

/// Create a non-admin user in the auth DB and mint a JWT for them. JWT sessions
/// always carry write scope; the resolved role comes from the DB row, so a
/// `SystemRole::User` here is genuinely non-admin at the handler.
fn make_user(state: &open_triplestore::server::AppState, id: &str) -> String {
    state
        .auth_db
        .create_user(id, id, &format!("{id}@ex.com"), "hash", SystemRole::User)
        .unwrap();
    mint_token(id, id, "user")
}

// ─── [S3] delete_dataset requires manage ──────────────────────────────────────

#[tokio::test]
async fn editor_cannot_delete_dataset() {
    let state = test_state();
    // An org-owned, members-visible dataset: a plain org member resolves to the
    // Editor resource role (can_write == true, can_manage == false).
    let ed = make_user(&state, "ed");
    state
        .auth_db
        .create_organisation("o1", "Acme", "acme", None, None)
        .unwrap();
    state
        .auth_db
        .add_org_member("ed", "o1", Role::Member)
        .unwrap();
    state
        .auth_db
        .create_dataset(
            "d1",
            "Data",
            None,
            OwnerType::Organisation,
            "o1",
            Visibility::Members,
            None,
        )
        .unwrap();

    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/datasets/d1")
                .header(header::AUTHORIZATION, format!("Bearer {ed}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a plain Editor must not be able to delete the dataset"
    );
}

#[tokio::test]
async fn owner_can_delete_dataset() {
    // Positive control: the dataset owner (manage role) still succeeds, proving the
    // tightened gate did not break the legitimate path.
    let state = test_state();
    let owner = make_user(&state, "own");
    state
        .auth_db
        .create_dataset(
            "d1",
            "Data",
            None,
            OwnerType::User,
            "own",
            Visibility::Private,
            None,
        )
        .unwrap();

    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/datasets/d1")
                .header(header::AUTHORIZATION, format!("Bearer {owner}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "owner delete must succeed"
    );
}

// ─── [CB14] group mutations are org-scoped ────────────────────────────────────

#[tokio::test]
async fn cross_org_group_update_is_not_found() {
    // The group lives in o2; updating it via o1's path must 404 even for a
    // super_admin (who bypasses the membership check) — the org-scope guard is the
    // only thing standing between the path's org and the group's real org.
    let (state, admin) = admin_state();
    state
        .auth_db
        .create_organisation("o1", "One", "one", None, None)
        .unwrap();
    state
        .auth_db
        .create_organisation("o2", "Two", "two", None, None)
        .unwrap();
    state
        .auth_db
        .create_group("g2", "o2", "Group2", None)
        .unwrap();

    let body = serde_json::json!({ "name": "Renamed", "parent_group_id": null });
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/api/organisations/o1/groups/g2")
                .header(header::AUTHORIZATION, format!("Bearer {admin}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "updating an o2 group through the o1 path must be 404"
    );

    // And the group must be untouched.
    let g2 = state.auth_db.get_group("g2").unwrap().unwrap();
    assert_eq!(
        g2.name, "Group2",
        "cross-org update must not mutate the group"
    );
}

#[tokio::test]
async fn cross_org_group_delete_is_not_found() {
    let (state, admin) = admin_state();
    state
        .auth_db
        .create_organisation("o1", "One", "one", None, None)
        .unwrap();
    state
        .auth_db
        .create_organisation("o2", "Two", "two", None, None)
        .unwrap();
    state
        .auth_db
        .create_group("g2", "o2", "Group2", None)
        .unwrap();

    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/organisations/o1/groups/g2")
                .header(header::AUTHORIZATION, format!("Bearer {admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "deleting an o2 group through the o1 path must be 404"
    );
    assert!(
        state.auth_db.get_group("g2").unwrap().is_some(),
        "cross-org delete must not remove the group"
    );
}

// ─── [CB3] update_dataset_shacl shapes-graph boundary ─────────────────────────

#[tokio::test]
async fn shapes_graph_pointing_at_foreign_dataset_is_rejected() {
    // An Editor on d1 tries to point its shapes graph at d2's reserved namespace.
    // get_shapes would later dump that graph, so this is a cross-tenant read vector
    // and must be refused with 403.
    let state = test_state();
    let ed = make_user(&state, "ed");
    state
        .auth_db
        .create_organisation("o1", "Acme", "acme", None, None)
        .unwrap();
    state
        .auth_db
        .add_org_member("ed", "o1", Role::Member)
        .unwrap();
    state
        .auth_db
        .create_dataset(
            "d1",
            "Mine",
            None,
            OwnerType::Organisation,
            "o1",
            Visibility::Members,
            None,
        )
        .unwrap();
    // d2 is a separate dataset; its HTTP namespace is reserved to it.
    state
        .auth_db
        .create_dataset(
            "d2",
            "Theirs",
            None,
            OwnerType::Organisation,
            "o1",
            Visibility::Members,
            None,
        )
        .unwrap();

    // base_url in the test harness is http://localhost:7878 (see common::test_state).
    let foreign = "http://localhost:7878/dataset/d2/secret";
    let body = serde_json::json!({ "shacl_on_write": true, "shapes_graph_iri": foreign });
    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/api/datasets/d1/shacl")
                .header(header::AUTHORIZATION, format!("Bearer {ed}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "shapes_graph_iri inside another dataset's namespace must be rejected"
    );
}

#[tokio::test]
async fn shapes_graph_in_own_namespace_is_accepted() {
    // Positive control: the canonical own-namespace shapes IRI is accepted for a
    // non-admin, proving the boundary check is not over-broad.
    let state = test_state();
    let ed = make_user(&state, "ed");
    state
        .auth_db
        .create_organisation("o1", "Acme", "acme", None, None)
        .unwrap();
    state
        .auth_db
        .add_org_member("ed", "o1", Role::Member)
        .unwrap();
    state
        .auth_db
        .create_dataset(
            "d1",
            "Mine",
            None,
            OwnerType::Organisation,
            "o1",
            Visibility::Members,
            None,
        )
        .unwrap();

    let own = "urn:dataset:d1:shapes";
    let body = serde_json::json!({ "shacl_on_write": true, "shapes_graph_iri": own });
    let resp = test_app(state)
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/api/datasets/d1/shacl")
                .header(header::AUTHORIZATION, format!("Bearer {ed}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "own-namespace shapes graph must be accepted"
    );
}
