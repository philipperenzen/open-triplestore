//! Plugin trait + host context for Open Triplestore compile-time plugins.
//!
//! This is the stable, minimal surface a plugin crate is written against. It
//! deliberately does **not** depend on the `open-triplestore` crate: a plugin
//! crate depends on `ots-plugin-api`, and the host binary depends on both the
//! plugin crate and `open-triplestore` — if the plugin depended on
//! `open-triplestore` directly, the host package would form a dependency
//! cycle with its own plugin (host → plugin → host). Routing everything
//! through this narrow, capability-based [`PluginContext`] avoids that while
//! still giving plugins what they actually need: SPARQL query/update against
//! the shared store, and the instance's base URL.
//!
//! See `plugins/hello` for a minimal template plugin and `docs/plugins.md`
//! for the full guide (including how a mature plugin graduates into core).

use std::sync::Arc;

/// SPARQL capability a plugin gets against the host's shared store, without
/// depending on the host crate's store types directly.
pub trait PluginStore: Send + Sync {
    /// Run a SPARQL SELECT/ASK query. Returns the raw SPARQL 1.1 JSON Results
    /// body (`application/sparql-results+json`) as text, so a plugin handler
    /// can return it to a caller as-is or parse it with any JSON library.
    fn query_json(&self, sparql: &str) -> Result<String, String>;

    /// Run a SPARQL UPDATE against the shared store.
    fn update(&self, sparql: &str) -> Result<(), String>;
}

/// Everything a plugin needs from the running instance. Cheap to clone (every
/// field is `Arc`-backed) — used directly as the Axum state for plugin routes.
#[derive(Clone)]
pub struct PluginContext {
    /// The instance's public base URL (no trailing slash) — for minting or
    /// echoing back canonical IRIs.
    pub base_url: Arc<String>,
    /// SPARQL query/update capability against the shared store.
    pub store: Arc<dyn PluginStore>,
}

/// A compile-time Open Triplestore plugin.
///
/// Implement this trait, register an instance in the host's plugin registry
/// (`src/plugins.rs`) behind your crate's own `feature = "plugin-<name>"`
/// flag, and your plugin's routes are mounted under `/ext/<name()>`.
pub trait Plugin: Send + Sync + 'static {
    /// Short, URL-safe identifier — becomes the `/ext/<name>` mount prefix and
    /// the id shown by `GET /api/plugins`. Keep it stable across versions.
    fn name(&self) -> &'static str;

    /// Plugin version — typically `env!("CARGO_PKG_VERSION")` of the plugin crate.
    fn version(&self) -> &'static str;

    /// Routes mounted under `/ext/<name()>`. Default: no routes.
    fn routes(&self) -> axum::Router<PluginContext> {
        axum::Router::new()
    }

    /// Called once at boot, after the host context is ready. Use it for
    /// idempotent setup (e.g. ensuring a dataset exists). Implementations
    /// should log and swallow their own errors rather than panic — a broken
    /// plugin must never take the host process down.
    fn on_boot(&self, _ctx: &PluginContext) {}

    /// Spawn a long-running background task, if the plugin needs one (e.g. a
    /// periodic sync). Called once at boot, after `on_boot`. The plugin owns
    /// whatever task it spawns (typically via `tokio::spawn`).
    fn spawn_background(&self, _ctx: PluginContext) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NullStore;
    impl PluginStore for NullStore {
        fn query_json(&self, _sparql: &str) -> Result<String, String> {
            Ok("{}".to_string())
        }
        fn update(&self, _sparql: &str) -> Result<(), String> {
            Ok(())
        }
    }

    struct Minimal;
    impl Plugin for Minimal {
        fn name(&self) -> &'static str {
            "minimal"
        }
        fn version(&self) -> &'static str {
            "0.0.0"
        }
    }

    #[test]
    fn default_hooks_are_inert() {
        let p = Minimal;
        let ctx = PluginContext {
            base_url: Arc::new("http://localhost".to_string()),
            store: Arc::new(NullStore),
        };
        // Defaults must not panic and must produce an empty router.
        p.on_boot(&ctx);
        p.spawn_background(ctx);
        let _router: axum::Router<PluginContext> = p.routes();
    }
}
