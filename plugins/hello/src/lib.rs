//! `ots-plugin-hello` — the template plugin.
//!
//! Copy this crate to start a new plugin ("cookiecutter" flow):
//!
//! 1. `cp -r plugins/hello plugins/my-plugin`
//! 2. Rename the package in `plugins/my-plugin/Cargo.toml` (`name = "ots-plugin-my-plugin"`).
//! 3. Add it to the root `Cargo.toml`:
//!    ```toml
//!    [dependencies]
//!    ots-plugin-my-plugin = { path = "plugins/my-plugin", optional = true }
//!
//!    [features]
//!    plugin-my-plugin = ["dep:ots-plugin-my-plugin"]
//!    ```
//! 4. Register an instance in `src/plugins.rs`'s `registered_plugins()`,
//!    gated by `#[cfg(feature = "plugin-my-plugin")]`.
//! 5. `cargo build --features plugin-my-plugin` and hit `/ext/my-plugin`.
//!
//! See `docs/plugins.md` for the full guide, including the promotion path
//! into core once a plugin is broadly useful.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use ots_plugin_api::{Plugin, PluginContext};
use serde::Serialize;

/// The template plugin: a static greeting plus a small instance-info endpoint
/// that demonstrates running a SPARQL query through [`PluginContext::store`].
#[derive(Default)]
pub struct HelloPlugin;

impl Plugin for HelloPlugin {
    fn name(&self) -> &'static str {
        "hello"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn routes(&self) -> Router<PluginContext> {
        Router::new()
            .route("/", get(hello))
            .route("/info", get(info))
            .route("/whoami", get(whoami))
    }

    fn on_boot(&self, ctx: &PluginContext) {
        tracing::info!("plugin 'hello' booted (base_url={})", ctx.base_url);
    }
}

#[derive(Serialize)]
struct HelloResponse {
    message: &'static str,
    plugin: &'static str,
    version: &'static str,
}

async fn hello() -> Json<HelloResponse> {
    Json(HelloResponse {
        message: "hello from ots-plugin-hello",
        plugin: "hello",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize)]
struct InfoResponse {
    base_url: String,
    named_graph_count: Option<u64>,
}

/// Demonstrates the store capability: counts distinct named graphs via a
/// SPARQL query run through `PluginContext::store`, entirely from within the
/// plugin crate (no access to the host's internal store types).
async fn info(State(ctx): State<PluginContext>) -> Json<InfoResponse> {
    let named_graph_count = ctx
        .store
        .query_json("SELECT (COUNT(DISTINCT ?g) AS ?c) WHERE { GRAPH ?g { ?s ?p ?o } }")
        .ok()
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
        .and_then(|v| {
            v["results"]["bindings"]
                .get(0)?
                .get("c")?
                .get("value")?
                .as_str()?
                .parse::<u64>()
                .ok()
        });
    Json(InfoResponse {
        base_url: ctx.base_url.as_str().to_string(),
        named_graph_count,
    })
}

/// `GET /ext/hello/whoami` — demonstrates [`ots_plugin_api::PluginAuth`]: the
/// host resolves the caller's bearer credential (session, API token, or a
/// provider access token) and the plugin just relays the principal JSON.
async fn whoami(
    State(ctx): State<PluginContext>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, (axum::http::StatusCode, String)> {
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .unwrap_or("");
    let principal = ctx
        .auth
        .introspect_bearer(bearer)
        .map_err(|e| (axum::http::StatusCode::UNAUTHORIZED, e))?;
    Ok(axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(principal))
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt as _;
    use ots_plugin_api::PluginStore;
    use std::sync::Arc;
    use tower::ServiceExt as _;

    struct FakeStore;
    impl PluginStore for FakeStore {
        fn query_json(&self, _sparql: &str) -> Result<String, String> {
            Ok(r#"{"results":{"bindings":[{"c":{"value":"3"}}]}}"#.to_string())
        }
        fn update(&self, _sparql: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn test_ctx() -> PluginContext {
        PluginContext {
            base_url: Arc::new("http://localhost:7878".to_string()),
            store: Arc::new(FakeStore),
            auth: Arc::new(ots_plugin_api::NoAuth),
        }
    }

    #[tokio::test]
    async fn whoami_rejects_without_a_resolvable_credential() {
        let app = HelloPlugin.routes().with_state(test_ctx());
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/whoami")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn hello_route_returns_greeting() {
        let app = HelloPlugin.routes().with_state(test_ctx());
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["plugin"], "hello");
    }

    #[tokio::test]
    async fn info_route_surfaces_store_query_result() {
        let app = HelloPlugin.routes().with_state(test_ctx());
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/info")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["base_url"], "http://localhost:7878");
        assert_eq!(v["named_graph_count"], 3);
    }

    #[test]
    fn name_and_version_are_stable() {
        let p = HelloPlugin;
        assert_eq!(p.name(), "hello");
        assert!(!p.version().is_empty());
    }
}
