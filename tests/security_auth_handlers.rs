//! HTTP-level regression tests for auth-handler authorization fixes.
//!
//! Covers:
//! * [S3]  DELETE /api/datasets/:id requires *manage* (not merely write), so a
//!   plain Editor is refused.
//! * [CB14] PUT/DELETE /api/organisations/:org/groups/:group reject a group that
//!   belongs to a *different* org (cross-org path) with 404.
//! * [CB3] PUT /api/datasets/:id/shacl rejects a `shapes_graph_iri` that points
//!   at another dataset's namespace for a non-admin caller.
//! * PUT /api/admin/oauth/providers/:id accepts the body the admin form sends
//!   and keeps the redacted client secret and SAML certificate when absent.
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

// ─── Identity-provider edits keep what a read redacts ─────────────────────────

async fn admin_json(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

/// Two throwaway self-signed certificates (public parts only). A build with
/// the `saml` feature refuses an IdP certificate that does not parse.
const CERT_A: &str = "-----BEGIN CERTIFICATE-----\nMIIBjzCCATWgAwIBAgIUIjqfh5zqhmH1xnwGUq/QcTy7rPswCgYIKoZIzj0EAwIw\nHDEaMBgGA1UEAwwRaWRwLWEuZXhhbXBsZS5vcmcwIBcNMjYxMDAzMDAyNzA4WhgP\nMjEyNjA5MDkwMDI3MDhaMBwxGjAYBgNVBAMMEWlkcC1hLmV4YW1wbGUub3JnMFkw\nEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE3gQC74t8Dyc98b51OQlvEftoy+4HlVSE\nvoHKz+fzNiY2rWZK969Yb+Bsf7q2dWw7KVVWn3bl7H1W6bMA0wkxn6NTMFEwHQYD\nVR0OBBYEFIPTHvOwMbf1YMxyLehMubBLTMD4MB8GA1UdIwQYMBaAFIPTHvOwMbf1\nYMxyLehMubBLTMD4MA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDSAAwRQIh\nAKtoJKTjRYIJvlqraaOnA4u4Dm81rkPAy9WKQyTgEat6AiB0zWiQdhhGsYLv8y+N\nqgJVeVw09c139bMic0vS5a5/3A==\n-----END CERTIFICATE-----";
const CERT_B: &str = "-----BEGIN CERTIFICATE-----\nMIIBjjCCATWgAwIBAgIUF3enFtZkx9UNk5vj2pOriUKBaGAwCgYIKoZIzj0EAwIw\nHDEaMBgGA1UEAwwRaWRwLWIuZXhhbXBsZS5vcmcwIBcNMjYxMDAzMDAyNzA4WhgP\nMjEyNjA5MDkwMDI3MDhaMBwxGjAYBgNVBAMMEWlkcC1iLmV4YW1wbGUub3JnMFkw\nEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEl9Vo8reNxWbXZC5hsJumwizXE1aS/5g/\nZt5D0bx6mMUamhppRsjtpOcVCsn5Lo527hgJUMp32HZEr0x0ZFP+x6NTMFEwHQYD\nVR0OBBYEFIlKIFm6M3FiAUYsgd7SMPtf/o2BMB8GA1UdIwQYMBaAFIlKIFm6M3Fi\nAUYsgd7SMPtf/o2BMA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDRwAwRAIg\nGxZNifKtz8jgwWUkn9r+uasILugXZPdwhhGNzAccE2oCIEIXfFsoZRQrAB9d1u8H\nvB03ylhxZHXiMGF0eqGRGy2E\n-----END CERTIFICATE-----";

/// The admin form cannot send back the client secret or the SAML IdP
/// certificate (no read returns them), so an edit that omits them must keep
/// the stored values. The bodies are the shape the form sends: `scopes` and
/// `role_claim_map` as strings, `is_active` rather than `enabled`.
#[tokio::test]
async fn provider_edit_keeps_redacted_secret_and_certificate() {
    let (state, token) = admin_state();
    let db = state.auth_db.clone();
    let app = test_app(state);
    let base = serde_json::json!({
        "name": "Example SAML", "slug": "example-saml", "provider_type": "saml",
        "client_id": null, "discovery_url": null, "tenant_id": "tenant-1",
        "entity_id": "https://idp.example.org/saml", "sso_url": "https://idp.example.org/sso",
        "scopes": "openid email profile",
        "role_claim_map": "{\"staff\":\"user\"}",
        "auto_provision": true, "default_role": "user", "is_active": true,
    });
    let mut create = base.clone();
    create["idp_certificate"] = CERT_A.into();
    create["client_secret"] = "secret-a".into();
    let (st, txt) = admin_json(
        &app,
        Method::POST,
        "/api/admin/oauth/providers",
        &token,
        Some(create),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let id = serde_json::from_str::<serde_json::Value>(&txt).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let stored = db.get_oauth_provider_by_id(&id).unwrap().unwrap();
    let secret_a = stored.client_secret_enc.clone();
    assert!(secret_a.is_some());

    // Disable it without resending either secret value.
    let mut edit = base.clone();
    edit["is_active"] = false.into();
    let uri = format!("/api/admin/oauth/providers/{id}");
    let (st, txt) = admin_json(&app, Method::PUT, &uri, &token, Some(edit)).await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{txt}");
    let p = db.get_oauth_provider_by_id(&id).unwrap().unwrap();
    assert!(!p.is_active);
    assert_eq!(p.idp_certificate.as_deref(), Some(CERT_A));
    assert_eq!(p.client_secret_enc, secret_a);
    assert_eq!(p.tenant_id.as_deref(), Some("tenant-1"));
    assert_eq!(p.role_claim_map.as_deref(), Some("{\"staff\":\"user\"}"));

    // A supplied certificate replaces the stored one.
    let mut edit = base.clone();
    edit["idp_certificate"] = CERT_B.into();
    let (st, txt) = admin_json(&app, Method::PUT, &uri, &token, Some(edit)).await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{txt}");
    let p = db.get_oauth_provider_by_id(&id).unwrap().unwrap();
    assert!(p.is_active);
    assert_eq!(p.idp_certificate.as_deref(), Some(CERT_B));

    // The list the form reads carries `is_active` and string `scopes`, and
    // never the certificate.
    let (st, txt) = admin_json(
        &app,
        Method::GET,
        "/api/admin/oauth/providers",
        &token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let list: serde_json::Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(list[0]["is_active"], true);
    assert_eq!(list[0]["scopes"], "openid email profile");
    assert!(!txt.contains("BEGIN CERTIFICATE"), "{txt}");
}
