//! HTTP-level regression tests for auth-handler authorization fixes.
//!
//! Covers:
//! * [S3]  DELETE /api/datasets/:id requires *manage* (not merely write), so a
//!   plain Editor is refused.
//! * [CB14] PUT/DELETE /api/organisations/:org/groups/:group reject a group that
//!   belongs to a *different* org (cross-org path) with 404.
//! * [CB3] PUT /api/datasets/:id/shacl rejects a `shapes_graph_iri` that points
//!   at another dataset's namespace for a non-admin caller.
//! * [P1-2] The client IP in audit rows and in the LLM guard's guest budget is
//!   the TCP peer; `X-Forwarded-For` counts only from a `TRUSTED_PROXY_CIDRS`
//!   peer. The peer is injected as `ConnectInfo`, as the real server does.
//!
//! Driven through the real Axum router via `tower::ServiceExt::oneshot` (no socket).

mod common;
use common::*;

use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{header, Method, Request, StatusCode},
    Router,
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

// ─── [P1-2] client IP: TCP peer, forwarded headers only from trusted proxies ──

/// A request as the server sees it: arriving from TCP peer `peer`.
fn from_peer(mut req: Request<Body>, peer: &str) -> Request<Body> {
    let addr: SocketAddr = format!("{peer}:40000").parse().unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

fn failed_login(peer: &str, forwarded: &[(&str, &str)], username: &str) -> Request<Body> {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in forwarded {
        b = b.header(*k, *v);
    }
    let body = serde_json::json!({ "username": username, "password": "wrong-password" });
    from_peer(b.body(Body::from(body.to_string())).unwrap(), peer)
}

/// The IP recorded on the `login_failure` row for `username`.
fn audited_login_ip(state: &open_triplestore::server::AppState, username: &str) -> Option<String> {
    let rows = state
        .audit
        .list(100, 0, Some("login_failure"), None, None)
        .unwrap();
    let row = rows
        .into_iter()
        .find(|e| e.actor_username.as_deref() == Some(username))
        .expect("a login_failure row for the user");
    row.ip_address
}

fn app_behind_proxy(state: open_triplestore::server::AppState) -> Router {
    open_triplestore::server::build_router(state, "", vec!["10.0.0.0/8".parse().unwrap()])
}

#[tokio::test]
async fn audit_ignores_forwarded_headers_from_untrusted_peer() {
    let state = test_state();
    // A proxy is configured, but this request does not come from it.
    let resp = app_behind_proxy(state.clone())
        .oneshot(failed_login(
            "203.0.113.9",
            &[
                ("x-forwarded-for", "192.0.2.66"),
                ("x-real-ip", "192.0.2.67"),
            ],
            "forger",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        audited_login_ip(&state, "forger").as_deref(),
        Some("203.0.113.9"),
        "a client-written X-Forwarded-For must not choose the audited IP"
    );
}

#[tokio::test]
async fn audit_records_peer_on_direct_connection() {
    let state = test_state();
    // No proxy configured: the TCP peer is the client, headers or not.
    let resp = test_app(state.clone())
        .oneshot(failed_login(
            "198.51.100.7",
            &[("x-forwarded-for", "192.0.2.66")],
            "direct",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        audited_login_ip(&state, "direct").as_deref(),
        Some("198.51.100.7"),
        "without a proxy the audit row carries the TCP peer, not nothing"
    );
}

#[tokio::test]
async fn audit_believes_trusted_proxy_chain_right_to_left() {
    let state = test_state();
    // The client forged the left-most entry; the trusted proxy appended the
    // address it really saw.
    let resp = app_behind_proxy(state.clone())
        .oneshot(failed_login(
            "10.0.0.5",
            &[("x-forwarded-for", "192.0.2.66, 198.51.100.20")],
            "proxied",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        audited_login_ip(&state, "proxied").as_deref(),
        Some("198.51.100.20"),
        "behind a trusted proxy the right-most untrusted hop is the client"
    );
}

fn anonymous_nl_sparql(peer: &str, forwarded_for: &str) -> Request<Body> {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/llm/sparql")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", forwarded_for)
        .body(Body::from(
            serde_json::json!({ "question": "How many datasets are there?" }).to_string(),
        ))
        .unwrap();
    from_peer(req, peer)
}

#[tokio::test]
async fn llm_guest_budget_cannot_be_reset_with_a_forged_header() {
    // Nothing listens on port 1: a request the guard lets through fails fast
    // with 503 at the gateway; one the guard refuses is a 429.
    std::env::set_var("LLM_GATEWAY_URL", "http://127.0.0.1:1");
    let app = test_app(test_state());

    // The guest budget defaults to 5 per minute per client IP.
    for i in 0..5 {
        let resp = app
            .clone()
            .oneshot(anonymous_nl_sparql("203.0.113.50", &format!("192.0.2.{i}")))
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "request {i} is within the guest budget"
        );
    }
    let resp = app
        .clone()
        .oneshot(anonymous_nl_sparql("203.0.113.50", "192.0.2.99"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "a fresh X-Forwarded-For from the same peer must not mint a fresh budget"
    );

    // Another guest on a direct connection has a budget of their own: guests
    // are no longer pooled in one shared "unknown" bucket.
    let resp = app
        .oneshot(anonymous_nl_sparql("203.0.113.51", "192.0.2.99"))
        .await
        .unwrap();
    assert_ne!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}
