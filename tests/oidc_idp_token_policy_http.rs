//! OIDC resource-server mode: what an access token issued by an external IdP
//! may do here.
//!
//! - By default (`OTS_OIDC_IDP_TOKEN_POLICY=session`) it reads and writes like an
//!   interactive session but may not mint a long-lived `ots_` API token, the
//!   same rule `OTS_OIDC_SESSION_POLICY` sets for this store's own provider.
//! - `scoped` takes write from the token's `scope`/`scp` claim; `full` is the
//!   legacy escape hatch that restores minting.
//! - `OIDC_DEFAULT_ROLE` never makes an IdP account an administrator.
//!
//! Own binary: the policy settings are process-wide environment variables.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::oidc_provider::ProviderKeys;
use open_triplestore::auth::oidc_rs::{AuthExt, OidcVerifier};
use open_triplestore::server::AppState;
use serde_json::Value;
use tower::ServiceExt as _;

const AUDIENCE: &str = "ots-api";

/// The policy variables are process-wide; the tests in this binary set them,
/// so they run one at a time.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn clear_policy_env() {
    for var in [
        "OTS_OIDC_IDP_TOKEN_POLICY",
        "OTS_OIDC_IDP_WRITE_SCOPES",
        "OIDC_DEFAULT_ROLE",
    ] {
        std::env::remove_var(var);
    }
}

/// A stand-in IdP: an instance serving only its discovery document and JWKS,
/// whose provider key signs the access tokens.
struct Idp {
    issuer: String,
    keys: Arc<ProviderKeys>,
}

impl Idp {
    fn start() -> Self {
        let (mut state, _token) = admin_state();
        let keys = Arc::new(ProviderKeys::load_or_generate(&state.auth_db, "idp-secret").unwrap());
        state.oidc_provider = Some(keys.clone());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        state.base_url = Arc::new(issuer.clone());
        let app = test_app(state);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let l = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(l, app).await.unwrap();
            });
        });
        Self { issuer, keys }
    }

    /// An access token for `sub` carrying `extra` claims (`scope`, `scp`, …).
    fn token(&self, sub: &str, extra: Value) -> String {
        let now = chrono::Utc::now().timestamp();
        let mut claims = serde_json::json!({
            "iss": self.issuer,
            "sub": sub,
            "aud": AUDIENCE,
            "iat": now,
            "nbf": now - 5,
            "exp": now + 300,
            "email": format!("{sub}@example.org"),
            "name": sub,
        });
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            claims[k] = v;
        }
        self.keys.sign_claims(&claims).unwrap()
    }
}

/// The resource server: trusts `idp` for audience [`AUDIENCE`].
fn resource_server(idp: &Idp, configure: impl FnOnce(&mut AuthExt)) -> (AppState, Router) {
    let (mut state, _token) = admin_state();
    let mut ext = AuthExt::disabled();
    ext.oidc = Some(OidcVerifier::new(
        idp.issuer.clone(),
        Some(AUDIENCE.to_string()),
    ));
    configure(&mut ext);
    state.auth_ext = Arc::new(ext);
    let app = test_app(state.clone());
    (state, app)
}

async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    content_type: &str,
    body: String,
) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

async fn me(app: &Router, token: &str) -> Value {
    let (status, body) = call(
        app,
        Method::GET,
        "/api/auth/me",
        token,
        "text/plain",
        "".into(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the IdP token authenticates: {body}"
    );
    serde_json::from_str(&body).unwrap()
}

async fn mint(app: &Router, token: &str) -> (StatusCode, String) {
    call(
        app,
        Method::POST,
        "/api/auth/tokens",
        token,
        "application/json",
        r#"{"name":"ci","scopes":["read","write"]}"#.into(),
    )
    .await
}

/// Whether a SPARQL UPDATE got past the write-scope check. A denial there
/// names the missing write scope; anything else (success, or a graph ACL
/// refusal further on) means the credential counted as write-capable.
async fn may_write(app: &Router, token: &str) -> bool {
    let (status, body) = call(
        app,
        Method::POST,
        "/sparql",
        token,
        "application/sparql-update",
        "INSERT DATA { GRAPH <https://example.org/idp/g> { <urn:s> <urn:p> \"o\" } }".into(),
    )
    .await;
    let refused = matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN);
    !(refused && body.contains("write scope"))
}

#[tokio::test]
async fn idp_tokens_write_but_cannot_mint_api_tokens_by_default() {
    let _env = ENV_LOCK.lock().await;
    clear_policy_env();
    let idp = Idp::start();
    let (_state, app) = resource_server(&idp, |_| {});

    // A typical first-party SPA token: identity scopes only.
    let token = idp.token(
        "alice",
        serde_json::json!({ "scope": "openid profile email" }),
    );
    me(&app, &token).await;
    assert!(
        may_write(&app, &token).await,
        "session semantics: the IdP token may write"
    );
    let (status, body) = mint(&app, &token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a delegated IdP token must not become a permanent credential: {body}"
    );

    // An unknown policy value falls back to the default, not to the permissive one.
    std::env::set_var("OTS_OIDC_IDP_TOKEN_POLICY", "banana");
    let (status, body) = mint(&app, &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // The provider-token policy does not reach IdP tokens: `full` there keeps
    // IdP tokens at the default.
    std::env::remove_var("OTS_OIDC_IDP_TOKEN_POLICY");
    std::env::set_var("OTS_OIDC_SESSION_POLICY", "full");
    let (status, body) = mint(&app, &token).await;
    std::env::remove_var("OTS_OIDC_SESSION_POLICY");
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn scoped_idp_policy_reads_write_from_scope_and_scp() {
    let _env = ENV_LOCK.lock().await;
    clear_policy_env();
    std::env::set_var("OTS_OIDC_IDP_TOKEN_POLICY", "scoped");
    let idp = Idp::start();
    let (_state, app) = resource_server(&idp, |_| {});

    let reader = idp.token("reader", serde_json::json!({ "scope": "openid profile" }));
    me(&app, &reader).await;
    assert!(!may_write(&app, &reader).await, "no write scope, no write");

    let writer = idp.token("writer", serde_json::json!({ "scope": "openid write" }));
    assert!(
        may_write(&app, &writer).await,
        "`write` in `scope` grants write"
    );
    let (status, body) = mint(&app, &writer).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "scoped never mints: {body}");

    // Entra ID / Okta style: `scp`, as a string or an array, with namespaced
    // scope names the deployment lists in OTS_OIDC_IDP_WRITE_SCOPES.
    let scp_array = idp.token(
        "scp-array",
        serde_json::json!({ "scp": ["ots.read", "ots.write"] }),
    );
    let scp_string = idp.token(
        "scp-string",
        serde_json::json!({ "scp": "ots.read ots.write" }),
    );
    assert!(
        !may_write(&app, &scp_array).await,
        "a namespaced scope is not write until configured"
    );
    std::env::set_var("OTS_OIDC_IDP_WRITE_SCOPES", "ots.write");
    assert!(may_write(&app, &scp_array).await, "scp array");
    assert!(may_write(&app, &scp_string).await, "scp string");
    clear_policy_env();
}

#[tokio::test]
async fn full_idp_policy_restores_minting() {
    let _env = ENV_LOCK.lock().await;
    clear_policy_env();
    std::env::set_var("OTS_OIDC_IDP_TOKEN_POLICY", "full");
    let idp = Idp::start();
    let (_state, app) = resource_server(&idp, |_| {});

    let token = idp.token("legacy", serde_json::json!({ "scope": "openid" }));
    let (status, body) = mint(&app, &token).await;
    clear_policy_env();
    assert!(
        status.is_success(),
        "the legacy escape hatch still mints: {status} {body}"
    );
}

#[tokio::test]
async fn oidc_default_role_never_makes_an_administrator() {
    let _env = ENV_LOCK.lock().await;
    clear_policy_env();

    // As configured: an admin default is read as `user`, a lower one is kept.
    for (raw, expected) in [
        ("admin", "user"),
        ("super_admin", "user"),
        ("SUPER_ADMIN", "user"),
        ("guest", "guest"),
        ("user", "user"),
    ] {
        std::env::set_var("OIDC_DEFAULT_ROLE", raw);
        assert_eq!(
            AuthExt::from_env().default_role,
            expected,
            "OIDC_DEFAULT_ROLE={raw}"
        );
    }
    clear_policy_env();

    // Whatever reaches provisioning — a hand-built config, or an env-OIDC
    // provider row created by an earlier boot with an admin default — a new
    // IdP account is provisioned no higher than `user`. Each resource server
    // gets its own IdP: a fresh verifier fetches discovery and JWKS, which
    // share the stand-in's auth rate limit (burst 8 per client).
    for default in ["admin", "super_admin"] {
        let idp = Idp::start();
        let (_state, app) = resource_server(&idp, |ext| ext.default_role = default.to_string());
        let token = idp.token(&format!("new-{default}"), serde_json::json!({}));
        let who = me(&app, &token).await;
        assert_eq!(who["role"], "user", "default {default}: {who}");

        // A provider row left with an admin default by an earlier boot.
        let idp = Idp::start();
        let (stale_state, stale_app) = resource_server(&idp, |_| {});
        open_triplestore::auth::oidc_rs::ensure_env_provider(
            &stale_state.auth_db,
            &idp.issuer,
            default,
        )
        .unwrap();
        let token = idp.token(&format!("stale-{default}"), serde_json::json!({}));
        let who = me(&stale_app, &token).await;
        assert_eq!(who["role"], "user", "stale provider row {default}: {who}");
    }

    // Claim-mapped roles are unaffected: an explicit mapping may still make an admin.
    let idp = Idp::start();
    let (_state, app) = resource_server(&idp, |ext| {
        ext.role_claim_map = Some(r#"{"app-admins":"admin"}"#.to_string());
    });
    let token = idp.token("mapped", serde_json::json!({ "groups": ["app-admins"] }));
    assert_eq!(me(&app, &token).await["role"], "admin");
}
