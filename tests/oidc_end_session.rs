//! `end_session_endpoint` (RP-initiated logout) of the OIDC provider.
//!
//! * discovery advertises it;
//! * GET /oauth/logout clears both session cookies;
//! * the post-logout destination is honoured only on a registered client's
//!   origin (state echoed) — anything else lands on the store's sign-in page,
//!   so the endpoint is never an open redirector.
//!
//! Driven through the real Axum router via `tower::ServiceExt::oneshot`.

mod common;
use common::*;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use tower::ServiceExt as _;

fn state_with_client() -> open_triplestore::server::AppState {
    let state = test_state();
    state
        .auth_db
        .upsert_oauth_client(
            "otl-viewer",
            "OTL Viewer",
            &["http://localhost:5190/auth/callback".to_string()],
            true,
            None,
        )
        .unwrap();
    state
}

fn set_cookie_values(resp: &axum::response::Response) -> Vec<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn discovery_advertises_end_session_endpoint() {
    let app = test_app(test_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/.well-known/openid-configuration")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_text(resp.into_body()).await).unwrap();
    assert_eq!(body["end_session_endpoint"], "http://localhost:7878/oauth/logout");
}

#[tokio::test]
async fn logout_clears_cookies_and_returns_to_registered_client_with_state() {
    let app = test_app(state_with_client());
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/oauth/logout?client_id=otl-viewer&post_logout_redirect_uri=http%3A%2F%2Flocalhost%3A5190%2F&state=abc%20123")
                .header(header::COOKIE, "access_token=whatever; refresh_token=whatever")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(
        resp.headers()[header::LOCATION].to_str().unwrap(),
        "http://localhost:5190/?state=abc%20123"
    );
    let cookies = set_cookie_values(&resp);
    assert!(cookies.iter().any(|c| c.starts_with("access_token=;") && c.contains("Max-Age=0")), "{cookies:?}");
    assert!(cookies.iter().any(|c| c.starts_with("refresh_token=;") && c.contains("Max-Age=0")), "{cookies:?}");
}

#[tokio::test]
async fn logout_refuses_unregistered_destination_and_lands_on_login() {
    let app = test_app(state_with_client());
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/oauth/logout?client_id=otl-viewer&post_logout_redirect_uri=https%3A%2F%2Fevil.example%2Fphish")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()[header::LOCATION].to_str().unwrap(), "http://localhost:7878/login");
    // Even a refused destination still ends the session.
    assert!(set_cookie_values(&resp).iter().any(|c| c.starts_with("access_token=;")));
}

#[tokio::test]
async fn logout_without_parameters_lands_on_login() {
    let app = test_app(test_state());
    let resp = app
        .oneshot(Request::builder().method(Method::GET).uri("/oauth/logout").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()[header::LOCATION].to_str().unwrap(), "http://localhost:7878/login");
}
