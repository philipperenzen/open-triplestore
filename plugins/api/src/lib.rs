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

pub mod sources;

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

/// Account/identity capability for plugins, in the same JSON-string idiom as
/// [`PluginStore`] (no dependency on the host crate's auth types). Every
/// method takes the caller's raw bearer credential; the HOST enforces
/// authentication/authorization inside the capability — a plugin can neither
/// skip the checks nor see anything an equivalent API call wouldn't return.
pub trait PluginAuth: Send + Sync {
    /// Resolve a bearer credential (session JWT, `ots_` API token, or a
    /// provider-issued access token) to its account, as JSON:
    /// `{ "id", "username", "email", "role", "is_admin", "organisations":
    /// [{"slug","name","role"}], "groups": [{"org_slug","id","name"}] }`.
    /// `Err` = not a valid/active credential (message is safe to surface).
    fn introspect_bearer(&self, bearer: &str) -> Result<String, String>;

    /// Admin-gated: all accounts, as a JSON array of
    /// `{ "id", "username", "email", "role", "is_active" }`.
    fn users_json(&self, admin_bearer: &str) -> Result<String, String>;

    /// Admin-gated: all organisations with their member/group counts, as a
    /// JSON array of `{ "slug", "name", "members", "groups" }`.
    fn organisations_json(&self, admin_bearer: &str) -> Result<String, String>;

    /// Admin-gated: the instance's own LLM request-log aggregates, as
    /// `{ "last_24h": { "by_status": {..}, "requests": n },
    ///    "top_users_7d": [{"user","requests"}] }`.
    fn llm_stats_json(&self, admin_bearer: &str) -> Result<String, String>;
}

/// Secret-resolution capability: the host's secrets module, so a plugin's
/// credentials follow the same rules as the server's own. A setting holds a
/// secret **reference** (`env:NAME`, `file:/path`, `vault:…`) that the host
/// resolves at the moment of use; a raw value is accepted in the development
/// posture with a one-time warning and refused under `OTS_ENV=production`.
pub trait PluginSecrets: Send + Sync {
    /// The secret an environment variable configures: `Ok(None)` when the
    /// variable is unset or empty, `Ok(Some(value))` when it holds a
    /// reference that resolves (or, in development, a raw value), `Err` with
    /// a message safe to log — never the value — when it holds a raw value in
    /// production or a reference that does not resolve. A plugin that gets
    /// `Err` proceeds without the credential; it never falls back to reading
    /// the variable itself.
    fn env_secret(&self, setting: &str) -> Result<Option<String>, String>;
}

/// A [`PluginSecrets`] that resolves nothing — for plugin unit tests.
pub struct NoSecrets;

impl PluginSecrets for NoSecrets {
    fn env_secret(&self, _setting: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
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
    /// Account/identity capability (introspection + admin-gated overviews).
    pub auth: Arc<dyn PluginAuth>,
    /// Secret resolution through the host's secrets module.
    pub secrets: Arc<dyn PluginSecrets>,
}

/// A [`PluginAuth`] that rejects everything — for plugin unit tests that don't
/// exercise authentication.
pub struct NoAuth;

impl PluginAuth for NoAuth {
    fn introspect_bearer(&self, _bearer: &str) -> Result<String, String> {
        Err("auth unavailable".to_string())
    }
    fn users_json(&self, _admin_bearer: &str) -> Result<String, String> {
        Err("auth unavailable".to_string())
    }
    fn organisations_json(&self, _admin_bearer: &str) -> Result<String, String> {
        Err("auth unavailable".to_string())
    }
    fn llm_stats_json(&self, _admin_bearer: &str) -> Result<String, String> {
        Err("auth unavailable".to_string())
    }
}

/// What values a [`PluginSetting`] takes. The host validates the setting with
/// the parser it uses for its own settings of that kind and refuses to start
/// on a malformed value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKind {
    /// `1/0/true/false/yes/no/on/off`, any case.
    Bool,
    /// Free text.
    Text,
    /// An absolute `http(s)` URL.
    Url,
    /// A comma-separated list of words.
    List,
    /// A secret: a reference (`env:`, `file:`, `vault:`) or, outside the
    /// production posture, a raw value. Never shown; a raw value is refused
    /// at startup under `OTS_ENV=production`. Read it through
    /// [`PluginSecrets::env_secret`].
    Secret,
}

/// One environment variable a plugin reads, declared through
/// [`Plugin::settings`]. The host validates it at startup with its own
/// settings, lists it in `GET /api/admin/config` (in a section named after
/// the plugin), and does not report it as an unknown name. Unset means not
/// configured; the plugin keeps its own default.
#[derive(Clone, Copy, Debug)]
pub struct PluginSetting {
    /// The variable, `OTS_<PLUGIN>_…`.
    pub name: &'static str,
    pub kind: SettingKind,
    /// One line for the admin view.
    pub doc: &'static str,
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

    /// Datasource drivers this plugin contributes (see [`sources`]). The host
    /// registers each under its dialect next to the built-in SQLite driver;
    /// a dialect already provided by core or an earlier plugin is refused
    /// at boot with a log line, not silently overridden. Default: none.
    fn connectors(&self) -> Vec<Arc<dyn sources::SourceConnector>> {
        Vec::new()
    }

    /// The environment variables this plugin reads. A name the host already
    /// declares, or one an earlier plugin declared, is left out. Default:
    /// none.
    fn settings(&self) -> &'static [PluginSetting] {
        &[]
    }
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
            auth: Arc::new(NoAuth),
            secrets: Arc::new(NoSecrets),
        };
        // Defaults must not panic and must produce an empty router.
        p.on_boot(&ctx);
        p.spawn_background(ctx);
        let _router: axum::Router<PluginContext> = p.routes();
        assert!(p.settings().is_empty());
    }
}
