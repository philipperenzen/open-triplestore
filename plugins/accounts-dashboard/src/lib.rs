//! `ots-plugin-accounts-dashboard` — the suite's account-platform overview.
//!
//! One admin page (`/ext/accounts-dashboard/ui`) and one JSON endpoint
//! (`/ext/accounts-dashboard/api/overview`) that put the whole account
//! platform on a single screen:
//!
//! * every account (role, active state) and organisation (member/team counts),
//! * **per-app entitlements** — which accounts/roles/teams unlock which suite
//!   app, derived from configurable well-known group slugs
//!   (`ACCOUNTS_DASHBOARD_APP_GROUPS`, e.g.
//!   `"validation=suite-validation,forms-design=suite-form-designers"`),
//! * the store's own LLM request aggregates, merged (fail-soft) with an
//!   external LLM gateway's usage ledger when
//!   `ACCOUNTS_DASHBOARD_GATEWAY_USAGE_URL` is set (the Linked Data LLMs
//!   gateway's `GET /v1/usage`; `ACCOUNTS_DASHBOARD_GATEWAY_KEY` rides along
//!   as its bearer when set).
//!
//! Everything is gated on an ADMIN bearer via [`ots_plugin_api::PluginAuth`] —
//! the host enforces the check, the plugin never sees credentials beyond
//! relaying the header. The UI is a single self-contained page with its own
//! same-origin sign-in (the SPA keeps its tokens in memory, deliberately
//! unreadable from here).

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use ots_plugin_api::{Plugin, PluginContext};

const UI_HTML: &str = include_str!("ui.html");

/// The dashboard plugin (feature `plugin-accounts-dashboard`, off by default).
#[derive(Default)]
pub struct AccountsDashboardPlugin;

impl Plugin for AccountsDashboardPlugin {
    fn name(&self) -> &'static str {
        "accounts-dashboard"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn routes(&self) -> Router<PluginContext> {
        Router::new()
            .route("/", get(|| async { Redirect::permanent("ui") }))
            .route("/ui", get(ui))
            .route("/api/overview", get(overview))
    }

    fn on_boot(&self, _ctx: &PluginContext) {
        tracing::info!(
            "accounts-dashboard: /ext/accounts-dashboard/ui (gateway usage: {})",
            if gateway_usage_url().is_some() {
                "configured"
            } else {
                "off"
            }
        );
    }
}

fn gateway_usage_url() -> Option<String> {
    std::env::var("ACCOUNTS_DASHBOARD_GATEWAY_USAGE_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
}

/// `"app=group-slug,app2=slug2"` → `[{app, group}]`; malformed pairs skipped.
fn app_group_map() -> Vec<serde_json::Value> {
    let raw = std::env::var("ACCOUNTS_DASHBOARD_APP_GROUPS").unwrap_or_default();
    raw.split(',')
        .filter_map(|pair| {
            let (app, group) = pair.split_once('=')?;
            let (app, group) = (app.trim(), group.trim());
            (!app.is_empty() && !group.is_empty())
                .then(|| serde_json::json!({ "app": app, "group": group }))
        })
        .collect()
}

fn bearer_of(headers: &HeaderMap) -> &str {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .unwrap_or("")
}

async fn ui() -> Html<&'static str> {
    Html(UI_HTML)
}

async fn overview(State(ctx): State<PluginContext>, headers: HeaderMap) -> Response {
    let bearer = bearer_of(&headers).to_string();

    // Admin gate + the three host-side overviews (the host enforces admin on
    // each call; one failed call fails the request with the host's message).
    let users = match ctx.auth.users_json(&bearer) {
        Ok(v) => v,
        Err(e) => return (StatusCode::UNAUTHORIZED, e).into_response(),
    };
    let organisations = ctx
        .auth
        .organisations_json(&bearer)
        .unwrap_or_else(|_| "[]".into());
    let store_llm = ctx
        .auth
        .llm_stats_json(&bearer)
        .ok()
        .and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok())
        .unwrap_or(serde_json::Value::Null);

    // External gateway ledger — fail-soft: unreachable = marked unavailable.
    let gateway_llm = match gateway_usage_url() {
        None => serde_json::json!({ "configured": false }),
        Some(base) => fetch_gateway_usage(&base).await,
    };

    let users: serde_json::Value = serde_json::from_str(&users).unwrap_or_default();
    let organisations: serde_json::Value = serde_json::from_str(&organisations).unwrap_or_default();

    Json(serde_json::json!({
        "base_url": ctx.base_url.as_str(),
        "users": users,
        "organisations": organisations,
        "entitlements": { "app_groups": app_group_map() },
        "llm": { "store": store_llm, "gateway": gateway_llm },
    }))
    .into_response()
}

async fn fetch_gateway_usage(base: &str) -> serde_json::Value {
    let url = format!("{base}/v1/usage?group_by=user");
    let client = reqwest::Client::new();
    let mut req = client.get(&url).timeout(std::time::Duration::from_secs(5));
    if let Ok(key) = std::env::var("ACCOUNTS_DASHBOARD_GATEWAY_KEY") {
        if !key.trim().is_empty() {
            req = req.bearer_auth(key.trim());
        }
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => serde_json::json!({ "configured": true, "available": true, "usage": body }),
            Err(e) => unavailable(&e.to_string()),
        },
        Ok(resp) => unavailable(&format!("gateway answered {}", resp.status())),
        Err(e) => unavailable(&e.to_string()),
    }
}

fn unavailable(reason: &str) -> serde_json::Value {
    serde_json::json!({
        "configured": true,
        "available": false,
        "reason": reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt as _;
    use ots_plugin_api::{NoAuth, PluginStore};
    use std::sync::Arc;
    use tower::ServiceExt as _;

    struct NullStore;
    impl PluginStore for NullStore {
        fn query_json(&self, _s: &str) -> Result<String, String> {
            Ok("{}".into())
        }
        fn update(&self, _s: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn ctx() -> PluginContext {
        PluginContext {
            base_url: Arc::new("http://localhost:7878".to_string()),
            store: Arc::new(NullStore),
            auth: Arc::new(NoAuth),
        }
    }

    #[tokio::test]
    async fn ui_serves_html() {
        let app = AccountsDashboardPlugin.routes().with_state(ctx());
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/ui")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("Accounts dashboard"),
            "embedded UI must render"
        );
    }

    #[tokio::test]
    async fn overview_requires_a_resolvable_admin() {
        let app = AccountsDashboardPlugin.routes().with_state(ctx());
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/overview")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn app_group_map_parses_and_skips_malformed() {
        std::env::set_var(
            "ACCOUNTS_DASHBOARD_APP_GROUPS",
            "validation=suite-validation, forms-design=suite-form-designers,broken,=x,y=",
        );
        let m = app_group_map();
        std::env::remove_var("ACCOUNTS_DASHBOARD_APP_GROUPS");
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!(m[0]["app"], "validation");
        assert_eq!(m[1]["group"], "suite-form-designers");
    }
}
