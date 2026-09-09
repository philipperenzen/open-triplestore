//! Spark without a reachable LLM gateway: the chat endpoints must degrade the
//! way `/api/llm/health` does — a 503 that names the gateway and the knob to
//! set — not a bare 500 "Internal server error", which is what they returned
//! (every "endpoint unreachable" mapping was `AppError::Internal`).
//!
//! Own test binary on purpose: `LLM_GATEWAY_URL` is process-wide, and the
//! orientation suite points it at a mock gateway for its whole process.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::json;
use tower::ServiceExt as _;

/// Nothing listens on port 1, so the connection is refused immediately.
const DEAD_GATEWAY: &str = "http://127.0.0.1:1";

#[tokio::test]
async fn chat_without_a_gateway_is_a_503_that_names_the_knob() {
    std::env::set_var("LLM_GATEWAY_URL", DEAD_GATEWAY);
    let (state, token) = admin_state();
    let app = test_app(state);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/llm/chat")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({ "messages": [{ "role": "user", "content": "How many datasets are there?" }] })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    let body = body_text(resp.into_body()).await;
    assert_eq!(
        st,
        StatusCode::SERVICE_UNAVAILABLE,
        "an unreachable gateway is a 503, not a crash: {body}"
    );
    assert!(
        body.contains("LLM_GATEWAY_URL") && body.contains("127.0.0.1:1"),
        "the error names the endpoint it tried and the knob to fix it: {body}"
    );

    // The streaming variant fails the same way, or — if the failure lands after
    // the stream has opened — carries the same explanation in the stream.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/llm/chat/stream")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({ "messages": [{ "role": "user", "content": "How many datasets are there?" }] })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    let body = body_text(resp.into_body()).await;
    assert!(
        st == StatusCode::SERVICE_UNAVAILABLE
            || (st == StatusCode::OK && body.contains("LLM_GATEWAY_URL")),
        "stream: {st} {body}"
    );

    // Health keeps reporting instead of failing.
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/llm/health")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp.into_body()).await;
    assert_eq!(v["reachable"], false, "{v}");
    assert_eq!(v["gateway"], DEAD_GATEWAY, "{v}");
}
