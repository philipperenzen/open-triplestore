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
use ots_plugin_api::{Plugin, PluginAuth, PluginContext, PluginStore};
use serde::Serialize;

use crate::auth::handlers::{GUEST_DISABLED_MESSAGE, GUEST_DISABLED_REASON};
use crate::auth::jwt::{hash_token, verify_token};
use crate::auth::models::User;
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

/// Local (synchronous) credential resolution for the plugin capability:
/// session JWTs, `ots_` API tokens and this store's own provider-issued
/// access tokens. External-IdP tokens are NOT resolved here (that path is
/// async network I/O) — a plugin dashboard is admin tooling on local
/// credentials, not a resource server.
fn resolve_local_user(state: &AppState, bearer: &str) -> Result<User, String> {
    let db = &state.auth_db;
    let user = if bearer.starts_with("ots_") {
        let tok = db
            .get_api_token_by_hash(&hash_token(bearer))
            .map_err(|e| e.to_string())?
            .ok_or("invalid API token")?;
        if tok.revoked {
            return Err("API token has been revoked".to_string());
        }
        if let Some(exp) = &tok.expires_at {
            if *exp < chrono::Utc::now().to_rfc3339() {
                return Err("API token has expired".to_string());
            }
        }
        db.get_user_by_id(&tok.user_id)
            .map_err(|e| e.to_string())?
            .ok_or("user not found")?
    } else if let Ok(claims) = verify_token(&state.jwt_config, bearer) {
        if claims.token_type != "access" {
            return Err("expected an access token".to_string());
        }
        db.get_user_by_id(&claims.sub)
            .map_err(|e| e.to_string())?
            .ok_or("user not found")?
    } else if let Some(keys) = state.oidc_provider.as_deref() {
        let issuer = state.base_url.trim_end_matches('/');
        let sub = crate::auth::oidc_provider::provider_token_subject(keys, issuer, bearer)
            .ok_or("invalid or expired token")?;
        db.get_user_by_id(&sub)
            .map_err(|e| e.to_string())?
            .ok_or("user not found")?
    } else {
        return Err("invalid or expired token".to_string());
    };
    if !user.is_active {
        if matches!(db.deactivation_reason(&user.id), Ok(Some(ref r)) if r == GUEST_DISABLED_REASON)
        {
            return Err(GUEST_DISABLED_MESSAGE.to_string());
        }
        return Err("account is deactivated".to_string());
    }
    Ok(user)
}

fn require_local_admin(state: &AppState, bearer: &str) -> Result<User, String> {
    let user = resolve_local_user(state, bearer)?;
    if !user.role.is_admin() {
        return Err("admin account required".to_string());
    }
    Ok(user)
}

impl PluginAuth for AppState {
    fn introspect_bearer(&self, bearer: &str) -> Result<String, String> {
        let user = resolve_local_user(self, bearer)?;
        let orgs: Vec<serde_json::Value> = self
            .auth_db
            .list_user_membership_summaries(&user.id)
            .unwrap_or_default()
            .into_iter()
            .map(|(slug, name, role)| {
                serde_json::json!({ "slug": slug, "name": name, "role": role })
            })
            .collect();
        let groups: Vec<serde_json::Value> = self
            .auth_db
            .list_user_group_summaries(&user.id)
            .unwrap_or_default()
            .into_iter()
            .map(|(org_slug, id, name)| {
                serde_json::json!({ "org_slug": org_slug, "id": id, "name": name })
            })
            .collect();
        Ok(serde_json::json!({
            "id": user.id,
            "username": user.username,
            "email": user.email,
            "role": user.role.as_str(),
            "is_admin": user.role.is_admin(),
            "organisations": orgs,
            "groups": groups,
        })
        .to_string())
    }

    fn users_json(&self, admin_bearer: &str) -> Result<String, String> {
        require_local_admin(self, admin_bearer)?;
        let users = self.auth_db.list_users().map_err(|e| e.to_string())?;
        let out: Vec<serde_json::Value> = users
            .into_iter()
            .map(|u| {
                serde_json::json!({
                    "id": u.id, "username": u.username, "email": u.email,
                    "role": u.role.as_str(), "is_active": u.is_active,
                })
            })
            .collect();
        Ok(serde_json::Value::Array(out).to_string())
    }

    fn organisations_json(&self, admin_bearer: &str) -> Result<String, String> {
        require_local_admin(self, admin_bearer)?;
        let orgs = self
            .auth_db
            .list_organisations()
            .map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(orgs.len());
        for o in orgs {
            let members = self.auth_db.count_org_members(&o.id).unwrap_or(0);
            let groups = self.auth_db.count_org_groups(&o.id).unwrap_or(0);
            out.push(serde_json::json!({
                "slug": o.slug, "name": o.name,
                "members": members, "groups": groups,
            }));
        }
        Ok(serde_json::Value::Array(out).to_string())
    }

    fn llm_stats_json(&self, admin_bearer: &str) -> Result<String, String> {
        require_local_admin(self, admin_bearer)?;
        self.auth_db
            .llm_request_aggregates()
            .map(|v| v.to_string())
            .map_err(|e| e.to_string())
    }
}

/// Build the [`PluginContext`] handed to every registered plugin.
pub fn plugin_context(state: &AppState) -> PluginContext {
    PluginContext {
        base_url: state.base_url.clone(),
        store: Arc::new(state.clone()),
        auth: Arc::new(state.clone()),
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
    #[cfg(feature = "plugin-accounts-dashboard")]
    plugins.push(Arc::new(
        ots_plugin_accounts_dashboard::AccountsDashboardPlugin,
    ));
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
