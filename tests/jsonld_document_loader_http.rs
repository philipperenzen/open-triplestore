//! The JSON-LD document loader behind every JSON-LD parse: bundled W3C
//! contexts resolve offline, any other remote `@context` is refused unless
//! `OTS_REMOTE_ALLOWLIST` covers it, and an allowlisted context is fetched
//! (redirects and `Link: rel="alternate"` followed within the allowlist),
//! size-capped and cached.
//!
//! One test function: the allowlist and the size cap are process-wide
//! environment settings, so the steps must not run concurrently with each
//! other.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use common::*;
use oxigraph::sparql::QueryResults;
use tower::ServiceExt as _;

const AS: &str = "https://www.w3.org/ns/activitystreams#";

async fn put_json_ld(app: &Router, token: &str, graph: &str, body: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/store?graph={}", url_encode(graph)))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/ld+json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    (st, body_text(resp.into_body()).await)
}

/// A loopback server publishing contexts, counting the requests for
/// `/ctx.jsonld`.
async fn context_server() -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    let ctx = r#"{"@context": {"name": "http://schema.org/name", "@vocab": "http://schema.org/"}}"#;
    let app = Router::new()
        .route(
            "/ctx.jsonld",
            axum::routing::get(move || {
                let counted = counted.clone();
                async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    ([(header::CONTENT_TYPE, "application/ld+json")], ctx)
                }
            }),
        )
        .route(
            "/moved",
            axum::routing::get(|| async {
                (StatusCode::FOUND, [(header::LOCATION, "/ctx2.jsonld")], "").into_response()
            }),
        )
        .route(
            "/ctx2.jsonld",
            axum::routing::get(move || async move {
                ([(header::CONTENT_TYPE, "application/ld+json")], ctx)
            }),
        )
        .route(
            "/page",
            axum::routing::get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "text/html"),
                        (
                            header::LINK,
                            r#"</ctx2.jsonld>; rel="alternate"; type="application/ld+json""#,
                        ),
                    ],
                    "<html></html>",
                )
            }),
        )
        .route(
            "/big.jsonld",
            axum::routing::get(|| async {
                let pad = "x".repeat(4096);
                (
                    [(header::CONTENT_TYPE, "application/ld+json")],
                    format!(
                        r#"{{"@context": {{"name": "http://schema.org/name"}}, "pad": "{pad}"}}"#
                    ),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (origin, hits)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_contexts_are_bundled_refused_or_fetched_from_the_allowlist() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let ask = |q: &str| matches!(state.store.query(q), Ok(QueryResults::Boolean(true)));
    std::env::remove_var("OTS_REMOTE_ALLOWLIST");
    std::env::remove_var("OTS_JSONLD_CONTEXT_MAX_BYTES");

    // 1. A bundled context parses with no allowlist at all.
    let (st, body) = put_json_ld(
        &app,
        &token,
        "https://example.org/jsonld/as",
        r#"{"@context": "https://www.w3.org/ns/activitystreams",
            "id": "https://example.org/notes/1", "type": "Note", "name": "Hello"}"#,
    )
    .await;
    assert!(st.is_success(), "{st} {body}");
    assert!(ask(&format!(
        "ASK {{ GRAPH <https://example.org/jsonld/as> {{ \
           <https://example.org/notes/1> a <{AS}Note> ; <{AS}name> \"Hello\" }} }}"
    )));

    // 2. Any other remote context is refused by default, without a request.
    let (origin, hits) = context_server().await;
    let doc = |path: &str| {
        format!(
            r#"{{"@context": "{origin}{path}", "@id": "https://example.org/p/1", "name": "Ada"}}"#
        )
    };
    let (st, body) = put_json_ld(
        &app,
        &token,
        "https://example.org/jsonld/a",
        &doc("/ctx.jsonld"),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("OTS_REMOTE_ALLOWLIST"), "{body}");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "a denied context is never fetched"
    );

    // 3. Allowlisted: fetched once, then served from the cache.
    std::env::set_var("OTS_REMOTE_ALLOWLIST", format!("{origin}/"));
    let name = |g: &str| {
        format!("ASK {{ GRAPH <{g}> {{ <https://example.org/p/1> <http://schema.org/name> \"Ada\" }} }}")
    };
    for g in [
        "https://example.org/jsonld/a",
        "https://example.org/jsonld/b",
    ] {
        let (st, body) = put_json_ld(&app, &token, g, &doc("/ctx.jsonld")).await;
        assert!(st.is_success(), "{st} {body}");
        assert!(ask(&name(g)), "{g}");
    }
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the second parse uses the cache"
    );

    // 4. A redirect and a Link alternate are followed within the allowlist.
    for (path, g) in [
        ("/moved", "https://example.org/jsonld/c"),
        ("/page", "https://example.org/jsonld/d"),
    ] {
        let (st, body) = put_json_ld(&app, &token, g, &doc(path)).await;
        assert!(st.is_success(), "{path}: {st} {body}");
        assert!(ask(&name(g)), "{path}");
    }

    // 5. The size cap.
    std::env::set_var("OTS_JSONLD_CONTEXT_MAX_BYTES", "1024");
    let (st, body) = put_json_ld(
        &app,
        &token,
        "https://example.org/jsonld/e",
        &doc("/big.jsonld"),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("larger than 1024 bytes"), "{body}");

    std::env::remove_var("OTS_JSONLD_CONTEXT_MAX_BYTES");
    std::env::remove_var("OTS_REMOTE_ALLOWLIST");
}
