//! Compile-time plugin registry.
//!
//! Bridges the host [`AppState`] to the plugin-facing [`ots_plugin_api`] crate:
//! implements [`PluginStore`] for `AppState` (so plugins get SPARQL query/update
//! without depending on this crate's store types — see `ots-plugin-api`'s
//! crate docs for why that boundary exists), and provides the registry every
//! compiled-in plugin is listed in.
//!
//! To add a plugin: copy `plugins/hello` (see its module docs for the exact
//! steps), then add one line to [`registered_plugins`] behind your crate's
//! own `plugin-<name>` feature. See `docs/plugins.md` for the full guide.

use std::sync::Arc;

use axum::Router;
use ots_plugin_api::{Plugin, PluginContext, PluginStore};
use serde::Serialize;

use crate::server::content_negotiation::{serialize_results_to, ResultFormat};
use crate::server::AppState;

impl PluginStore for AppState {
    fn query_json(&self, sparql: &str) -> Result<String, String> {
        let results = self.store.query(sparql).map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        serialize_results_to(results, ResultFormat::Json, &mut buf)?;
        String::from_utf8(buf).map_err(|e| e.to_string())
    }

    fn update(&self, sparql: &str) -> Result<(), String> {
        self.store.update(sparql).map_err(|e| e.to_string())
    }
}

/// Build the [`PluginContext`] handed to every registered plugin.
pub fn plugin_context(state: &AppState) -> PluginContext {
    PluginContext {
        base_url: state.base_url.clone(),
        store: Arc::new(state.clone()),
    }
}

/// Every plugin compiled into this binary. Add a line here, gated by your
/// plugin crate's own `plugin-<name>` feature, to register it.
// Each entry is individually `#[cfg]`-gated, so `vec![...]` (clippy's usual
// suggestion here) can't express this — the list may end up with zero, one,
// or many entries depending on which `plugin-*` features are enabled.
#[allow(clippy::vec_init_then_push, unused_mut)]
pub fn registered_plugins() -> Vec<Arc<dyn Plugin>> {
    let mut plugins: Vec<Arc<dyn Plugin>> = Vec::new();
    #[cfg(feature = "plugin-hello")]
    plugins.push(Arc::new(ots_plugin_hello::HelloPlugin));
    plugins
}

/// Run each registered plugin's `on_boot` then `spawn_background` hook.
/// Called once at server boot, after the context is ready. `Plugin` hook
/// implementations are expected to log and swallow their own errors — a
/// broken plugin must never take the host process down.
pub fn boot_plugins(ctx: &PluginContext) {
    for p in registered_plugins() {
        tracing::info!("plugin '{}' v{} loaded", p.name(), p.version());
        p.on_boot(ctx);
        p.spawn_background(ctx.clone());
    }
}

/// Mount every registered plugin's routes under `/ext/<name>` onto `router`.
///
/// Takes and returns a state-erased `Router` (`Router<()>`, matching how
/// every route group in `build_router` is assembled — each ends in
/// `.with_state(...)` before being merged into the top-level router), so a
/// plugin's own state (its [`PluginContext`], applied via `.with_state`
/// below) never has to unify with [`AppState`].
pub fn mount_plugins(router: Router, ctx: &PluginContext) -> Router {
    let mut router = router;
    for p in registered_plugins() {
        let sub = p.routes().with_state(ctx.clone());
        router = router.nest(&format!("/ext/{}", p.name()), sub);
    }
    router
}

#[derive(Serialize)]
pub struct PluginInfo {
    pub name: &'static str,
    pub version: &'static str,
}

/// `GET /api/plugins` — lists every plugin compiled into this binary
/// (name/version), so operators can see what's enabled without inspecting
/// the build flags used to produce it.
pub async fn list_plugins() -> axum::Json<Vec<PluginInfo>> {
    axum::Json(
        registered_plugins()
            .iter()
            .map(|p| PluginInfo {
                name: p.name(),
                version: p.version(),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;

    #[test]
    fn plugin_store_roundtrips_query_and_update() {
        let state = AppState::test_default_with_store(TripleStore::in_memory().unwrap());
        PluginStore::update(
            &state,
            "INSERT DATA { GRAPH <https://example.org/g> { \
             <https://example.org/s> <https://example.org/p> <https://example.org/o> } }",
        )
        .unwrap();
        let json = state
            .query_json("SELECT (COUNT(*) AS ?c) WHERE { GRAPH ?g { ?s ?p ?o } }")
            .unwrap();
        assert!(json.contains("\"c\""), "got: {json}");
    }

    #[tokio::test]
    async fn api_plugins_handler_returns_valid_list() {
        let axum::Json(list) = list_plugins().await;
        // Whether any plugin is present depends on which `plugin-*` features
        // are enabled for this test run; this just exercises the handler and
        // its shape end to end.
        assert!(list
            .iter()
            .all(|p| !p.name.is_empty() && !p.version.is_empty()));
    }

    #[test]
    fn mount_plugins_nests_routes_under_ext_prefix() {
        let state = AppState::test_default_with_store(TripleStore::in_memory().unwrap());
        let ctx = plugin_context(&state);
        // Just confirms this builds without panicking — per-plugin route
        // behavior is covered in each plugin crate's own tests.
        let _router: Router = mount_plugins(Router::new(), &ctx);
    }
}
