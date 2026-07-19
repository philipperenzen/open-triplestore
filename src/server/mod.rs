#[cfg(test)]
mod account_lifecycle_tests;
pub mod content_negotiation;
pub mod error;
mod linked_data;
pub mod llm_guard;
pub mod llm_history;
pub mod llm_sparql;
pub mod openapi;
#[cfg(test)]
mod passkey_tests;
#[cfg(test)]
mod role_visibility_tests;
pub mod routes;
#[cfg(test)]
mod security_regression_tests;
#[cfg(test)]
mod security_tests;

#[cfg(feature = "text-search")]
use crate::text_search::TextIndex;
#[cfg(feature = "text-search")]
use std::sync::atomic::{AtomicBool, Ordering};

use crate::auth::acl_handlers;
use crate::auth::db::AuthDb;
use crate::auth::handlers;
use crate::auth::jwt::JwtConfig;
use crate::auth::middleware::{
    endpoint_acl_guard, optional_auth, require_admin, require_auth, require_publisher,
};
use crate::auth::oauth::OAuthSessions;
use crate::auth::oauth_handlers;
use crate::catalog::routes::catalog_routes;
use crate::data_models::routes::{data_model_auth_routes, data_model_public_routes};
use crate::dataset_versions::routes::{dataset_version_auth_routes, dataset_version_public_routes};
use crate::prefixes::PrefixRegistry;
use crate::saved_queries::routes::{saved_query_auth_routes, saved_query_public_routes};
use crate::storage::ObjectStore;
use crate::store::TripleStore;
use axum::extract::{ConnectInfo, DefaultBodyLimit};
use axum::http::{HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::middleware;
use axum::routing::{delete, get, post, put};
use axum::Router;
use ipnet::IpNet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::KeyExtractor;
use tower_governor::{GovernorError, GovernorLayer};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

/// IP key extractor that checks X-Forwarded-For / X-Real-IP headers first (for reverse proxies
/// and Docker deployments), then falls back to the TCP peer address.
///
/// H-2: XFF/X-Real-IP headers are only trusted when the TCP peer IP falls within one of the
/// configured `trusted_cidrs`. This prevents attackers from spoofing their IP by injecting
/// an arbitrary X-Forwarded-For header.
#[derive(Clone)]
struct SmartIpExtractor {
    trusted_cidrs: Vec<IpNet>,
}

impl KeyExtractor for SmartIpExtractor {
    type Key = IpAddr;

    fn extract<B>(&self, req: &Request<B>) -> Result<IpAddr, GovernorError> {
        // 3. TCP peer address (available when using into_make_service_with_connect_info)
        let peer_ip: Option<IpAddr> = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip());

        let peer_is_trusted = peer_ip
            .map(|ip| self.trusted_cidrs.iter().any(|cidr| cidr.contains(&ip)))
            .unwrap_or(false);

        if peer_is_trusted {
            // 1. X-Forwarded-For. Walk the chain RIGHT-to-LEFT, skipping trusted
            //    proxy hops; the first untrusted address is the real client. The
            //    left-most entry is fully client-controlled, so trusting it would let
            //    a client behind the proxy forge `X-Forwarded-For: <victim>` to
            //    attribute their request load to (and rate-limit-lock-out) another IP,
            //    or rotate forged IPs to evade their own limit.
            if let Some(xff) = req.headers().get("x-forwarded-for") {
                if let Ok(val) = xff.to_str() {
                    for entry in val.rsplit(',') {
                        if let Ok(ip) = entry.trim().parse::<IpAddr>() {
                            let entry_trusted =
                                self.trusted_cidrs.iter().any(|cidr| cidr.contains(&ip));
                            if !entry_trusted {
                                return Ok(ip);
                            }
                        }
                    }
                }
            }
            // 2. X-Real-IP
            if let Some(xri) = req.headers().get("x-real-ip") {
                if let Ok(val) = xri.to_str() {
                    if let Ok(ip) = val.trim().parse::<IpAddr>() {
                        return Ok(ip);
                    }
                }
            }
        }

        // Use TCP peer IP directly (not behind a trusted proxy)
        if let Some(ip) = peer_ip {
            return Ok(ip);
        }
        // Fallback: bucket all unidentifiable clients together rather than hard-erroring.
        Ok(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
    }
}

/// Build a CORS layer from a comma-separated list of allowed origins.
///
/// Three modes, selected by the `CORS_ORIGINS` value:
/// * empty — same-origin only (no `Access-Control-Allow-Origin` is emitted).
/// * an explicit comma-separated list — only those origins are reflected, with
///   credentials. The tightest option; prefer it in production.
/// * `*` — reflect ANY origin (and the requested headers) with credentials, by mirroring
///   the request's `Origin` rather than the literal `*`, which the Fetch spec forbids
///   alongside credentials. This lets a browser client served from any origin — e.g. the
///   OTL viewer on `http://localhost:5190` — connect to this store.
///
/// Why mirroring any origin *with credentials* is safe here — and what it depends on:
/// this server authenticates a request from EITHER an `Authorization: Bearer <token>`
/// header OR an `access_token` cookie (`auth::middleware::extract_token`), so the cookie
/// **is** a real ambient credential. What keeps mirror mode safe is that both session
/// cookies (`access_token`, `refresh_token`) are `SameSite=Strict`, so the browser
/// withholds them on every cross-site request — even a credentialed `fetch` to a
/// mirrored origin. (The lone `SameSite=Lax` cookie, `oauth_state`, is a short-lived
/// CSRF nonce that is never sent on `fetch`/XHR and confers no access; there is no HTTP
/// Basic auth.) A hostile origin therefore cannot make the browser attach a usable
/// credential, and cannot forge the bearer header — so it gains nothing it could not
/// already reach unauthenticated. **Load-bearing invariant:** if any auth cookie is ever
/// downgraded to `SameSite=Lax`/`None`, mirror mode becomes a credentialed-CORS / CSRF
/// hole. The `auth_session_cookies_are_samesite_strict` regression test pins this.
fn build_cors_layer(cors_origins: &str) -> CorsLayer {
    let origins: Vec<&str> = cors_origins
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let layer = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
            Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            axum::http::header::ACCEPT,
            axum::http::header::IF_MATCH,
        ])
        // Expose the revision token so the browser can read it for optimistic
        // concurrency (If-Match) on draft edits across origins.
        .expose_headers([axum::http::header::ETAG]);

    // `*` ⇒ permissive mirror mode. The literal `*` cannot be paired with
    // `Access-Control-Allow-Credentials: true` (the Fetch spec rejects it), so mirror
    // the caller's `Origin` instead — a concrete value that *is* legal with credentials.
    // Also mirror the requested headers: in "any origin" mode we don't control the client,
    // so a fixed allow-list would silently break a preflight the moment a client sends a
    // header it doesn't enumerate. tower-http adds `Vary: Origin` automatically. See the
    // safety rationale on this fn's doc comment.
    if origins.contains(&"*") {
        tracing::warn!(
            "CORS_ORIGINS contains '*': reflecting ANY request origin with credentials \
             (mirror mode). Safe only while both session cookies stay SameSite=Strict (the \
             browser then withholds them cross-site, leaving the unforgeable Authorization \
             bearer token as the only cross-origin credential). For a tighter posture in \
             production, list explicit origins instead."
        );
        return layer
            .allow_credentials(true)
            .allow_origin(AllowOrigin::mirror_request())
            .allow_headers(AllowHeaders::mirror_request());
    }

    if origins.is_empty() {
        // No extra origins — same-origin only. No `Access-Control-Allow-Origin` is
        // emitted, so `allow_credentials` is intentionally left off here.
        layer
    } else {
        let allowed: Vec<HeaderValue> = origins
            .iter()
            .filter_map(|o| HeaderValue::from_str(o).ok())
            .collect();
        if allowed.is_empty() {
            tracing::error!(
                "CORS_ORIGINS set but no valid origins parsed; allowing same-origin only."
            );
            return layer;
        }
        // Credentials are only meaningful (and only safe) with an explicit,
        // operator-configured origin allow-list.
        layer.allow_credentials(true).allow_origin(allowed)
    }
}

/// Max number of browse queries executing concurrently (see `AppState::browse_semaphore`).
const MAX_CONCURRENT_BROWSE_QUERIES: usize = 64;

/// Capacity for the expensive-op semaphore: half the available parallelism (min 1)
/// so heavy synchronous handlers (reasoning, SHACL validate/infer, RML execute)
/// can never occupy every Tokio worker and starve the async runtime.
pub fn expensive_op_capacity() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(2)
}

/// Shared application state, cloned cheaply via `Arc`.
#[derive(Clone)]
pub struct AppState {
    pub store: TripleStore,
    /// Prefix registry for auto-resolving undeclared SPARQL prefixes.
    pub prefix_registry: Arc<PrefixRegistry>,
    /// SQLite-backed auth/identity database.
    pub auth_db: Arc<AuthDb>,
    /// Append-only audit logger (shares the auth_db connection pool).
    pub audit: Arc<crate::auth::audit::AuditLogger>,
    /// Backup subsystem. `None` when no backup directory is configured.
    pub backup: Option<Arc<crate::backup::BackupManager>>,
    /// JWT configuration.
    pub jwt_config: Arc<JwtConfig>,
    /// S3 object storage for assets.
    pub object_store: Arc<ObjectStore>,
    /// Transactional email for account flows (verification, password reset).
    /// Falls back to a log-only backend when SMTP is not configured.
    pub mailer: Arc<crate::email::Mailer>,
    /// Base URL for minting linked data IRIs (no trailing slash).
    pub base_url: Arc<String>,
    /// In-memory PKCE session store for OAuth 2.0 / OIDC flows.
    pub oauth_sessions: OAuthSessions,
    /// In-memory WebAuthn challenge store for passkey registration/login.
    pub passkey_sessions: crate::auth::passkey::PasskeySessions,
    /// OIDC resource-server config (env-driven): JWT verification + legacy-token flag.
    pub auth_ext: Arc<crate::auth::oidc_rs::AuthExt>,
    /// M-1/W4-21: SPARQL query and update timeout in seconds.
    pub query_timeout_secs: u64,
    /// Write-path timeout in seconds for GSP PUT/POST/DELETE and data-model/dataset
    /// DELETE/PATCH. Larger than `query_timeout_secs`; bulk import is exempt.
    pub write_timeout_secs: u64,
    /// When true, auth cookies are issued with the `Secure` attribute (HTTPS only).
    /// Disabled by default so plain-HTTP local development still works.
    pub secure_cookies: bool,
    /// Bounds concurrent browse query execution so a flood of browse requests
    /// cannot monopolise the `spawn_blocking` thread pool and starve other work.
    pub browse_semaphore: Arc<tokio::sync::Semaphore>,
    /// Caps concurrent EXPENSIVE operations (reasoning, SHACL validate/infer, RML
    /// execute) so a burst cannot occupy every Tokio worker and starve the runtime.
    pub expensive_semaphore: Arc<tokio::sync::Semaphore>,
    /// Tantivy full-text search index (text-search feature).
    #[cfg(feature = "text-search")]
    pub text_index: Option<Arc<TextIndex>>,
    /// Set to `true` after any write; triggers lazy Tantivy reindex before next search.
    #[cfg(feature = "text-search")]
    pub text_dirty: Arc<AtomicBool>,
}

/// Construct a minimal `AppState` for tests (unit and integration).
/// Available when the `test-utils` feature is enabled, or during `cargo test`.
#[cfg(any(test, feature = "test-utils"))]
impl AppState {
    pub fn test_default_with_store(store: TripleStore) -> Self {
        use crate::auth::db::AuthDb;
        use crate::auth::jwt::JwtConfig;
        use crate::storage::ObjectStore;
        let auth_db = Arc::new(AuthDb::in_memory().unwrap());
        let audit = Arc::new(crate::auth::audit::AuditLogger::new(auth_db.pool()));
        AppState {
            store,
            prefix_registry: Arc::new(PrefixRegistry::empty()),
            auth_db,
            audit,
            backup: None,
            jwt_config: Arc::new(JwtConfig::new("test-secret".to_string(), 30, 30)),
            object_store: Arc::new(
                ObjectStore::local(std::env::temp_dir().join("triplestore-test-objects")).unwrap(),
            ),
            mailer: Arc::new(crate::email::Mailer::log_only("http://localhost")),
            base_url: Arc::new("http://localhost".to_string()),
            oauth_sessions: crate::auth::oauth::new_session_store(),
            passkey_sessions: crate::auth::passkey::new_session_store(),
            auth_ext: Arc::new(crate::auth::oidc_rs::AuthExt::disabled()),
            query_timeout_secs: 30,
            write_timeout_secs: 120,
            secure_cookies: false,
            browse_semaphore: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_BROWSE_QUERIES)),
            expensive_semaphore: Arc::new(tokio::sync::Semaphore::new(expensive_op_capacity())),
            #[cfg(feature = "text-search")]
            text_index: None,
            #[cfg(feature = "text-search")]
            text_dirty: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl AppState {
    /// Mark the text index as stale after a write operation.
    ///
    /// The index will be rebuilt lazily on the next query that uses text search.
    #[cfg(feature = "text-search")]
    #[inline]
    pub fn mark_text_dirty(&self) {
        self.text_dirty.store(true, Ordering::Relaxed);
    }

    /// Rebuild the Tantivy index if it has been marked dirty since the last sync.
    ///
    /// Called automatically before each SPARQL query that may use `text:search`.
    #[cfg(feature = "text-search")]
    pub fn sync_text_index_if_dirty(&self) {
        if !self.text_dirty.load(Ordering::Relaxed) {
            return;
        }
        if let Some(ref idx) = self.text_index {
            match idx.reindex_from_store(&self.store) {
                Ok(n) => {
                    tracing::debug!("Text index auto-synced: {} documents", n);
                    self.text_dirty.store(false, Ordering::Relaxed);
                }
                Err(e) => {
                    tracing::warn!("Text index auto-sync failed: {}", e);
                }
            }
        }
    }
}

/// Cookie issuance policy, extracted from `AppState` for the auth handlers.
#[derive(Clone, Copy)]
pub struct CookieConfig {
    /// Issue cookies with the `Secure` attribute (HTTPS-only transport).
    pub secure: bool,
}

impl axum::extract::FromRef<AppState> for CookieConfig {
    fn from_ref(state: &AppState) -> Self {
        CookieConfig {
            secure: state.secure_cookies,
        }
    }
}

impl axum::extract::FromRef<AppState> for Arc<AuthDb> {
    fn from_ref(state: &AppState) -> Self {
        state.auth_db.clone()
    }
}

/// Extractor newtype for the linked-data base URL, so handlers that only take
/// `State<Arc<AuthDb>>` can also obtain `base_url` (e.g. to surface a dataset's
/// canonical IRI) without pulling in the whole `AppState`. A newtype is required
/// because the orphan rule forbids `impl FromRef<AppState> for Arc<String>`.
#[derive(Clone)]
pub struct BaseUrl(pub Arc<String>);

impl axum::extract::FromRef<AppState> for BaseUrl {
    fn from_ref(state: &AppState) -> Self {
        BaseUrl(state.base_url.clone())
    }
}

impl axum::extract::FromRef<AppState> for Arc<crate::auth::audit::AuditLogger> {
    fn from_ref(state: &AppState) -> Self {
        state.audit.clone()
    }
}

impl axum::extract::FromRef<AppState> for Option<Arc<crate::backup::BackupManager>> {
    fn from_ref(state: &AppState) -> Self {
        state.backup.clone()
    }
}

impl axum::extract::FromRef<AppState> for Arc<JwtConfig> {
    fn from_ref(state: &AppState) -> Self {
        state.jwt_config.clone()
    }
}

impl axum::extract::FromRef<AppState> for Arc<crate::auth::oidc_rs::AuthExt> {
    fn from_ref(state: &AppState) -> Self {
        state.auth_ext.clone()
    }
}

/// Surface the LDP capability headers (`Accept-Post`, `Accept-Patch`, `Allow`) on
/// `OPTIONS /ldp/*` responses even when the global CORS layer answers the preflight
/// before the LDP handler runs.
///
/// tower-http's `CorsLayer` short-circuits CORS *preflight* requests (an `OPTIONS`
/// carrying `Access-Control-Request-Method`) and replies directly, so the LDP
/// `OPTIONS` handler never executes and its capability headers are lost. This
/// middleware is layered just outside CORS, so it observes that short-circuited
/// response and fills the headers in when they are missing. A plain (non-preflight)
/// `OPTIONS` still reaches the handler, which sets them — `accept-post` is then
/// already present and this is a no-op. The values match `ldp::handler::ldp_options`.
async fn ldp_options_capabilities(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let is_ldp_options = req.method() == Method::OPTIONS && req.uri().path().starts_with("/ldp/");
    let mut resp = next.run(req).await;
    if is_ldp_options && !resp.headers().contains_key("accept-post") {
        let headers = resp.headers_mut();
        headers.insert(
            axum::http::header::ALLOW,
            HeaderValue::from_static("GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS"),
        );
        headers.insert(
            HeaderName::from_static("accept-post"),
            HeaderValue::from_static("text/turtle, application/ld+json"),
        );
        headers.insert(
            HeaderName::from_static("accept-patch"),
            HeaderValue::from_static("application/sparql-update"),
        );
    }
    resp
}

/// Build the application router.
///
/// Per-IP rate limiting is applied to three groups of endpoints, each with its own
/// quota: the auth endpoints (brute-force protection), the SPARQL / Graph Store
/// endpoints, and bulk import. See the individual configs below.
pub fn build_router(state: AppState, cors_origins: &str, trusted_cidrs: Vec<IpNet>) -> Router {
    // NOTE: `per_second(n)` in tower_governor is misleadingly named — it sets the
    // replenish *period* to n seconds (one token every n seconds), NOT n tokens per
    // second. So `per_second(6)` means one request per 6s, i.e. 10/min sustained.
    //
    // Per-IP rate limiting can be relaxed by setting `RATE_LIMIT_DISABLED=1` (or
    // `=true`). This is for trusted/internal deployments and the e2e/CI harness,
    // which drives many requests from a single IP and would otherwise trip the
    // brute-force and runaway-query limiters. SECURE BY DEFAULT: when the var is
    // unset the production quotas below apply unchanged; when set, every tier gets
    // an effectively-unlimited burst so legitimate automation is never throttled.
    let disable_rate_limit = std::env::var("RATE_LIMIT_DISABLED")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    if disable_rate_limit {
        tracing::warn!(
            "RATE_LIMIT_DISABLED is set — per-IP rate limiting is effectively OFF. \
             Intended for trusted/internal or test deployments only; never enable on a public server."
        );
    }
    // Build a per-IP governor config from a production `(replenish_period_secs, burst)`
    // pair, collapsing to an effectively-unlimited burst when rate limiting is disabled.
    let make_rate_conf = |period_secs: u64, burst: u32| {
        let (period_secs, burst) = if disable_rate_limit {
            (1, 1_000_000)
        } else {
            (period_secs, burst)
        };
        Arc::new(
            GovernorConfigBuilder::default()
                .key_extractor(SmartIpExtractor {
                    trusted_cidrs: trusted_cidrs.clone(),
                })
                .per_second(period_secs)
                .burst_size(burst)
                // Shape the 429 ourselves: tower_governor's default only sets its
                // own `x-ratelimit-after` header, so standards-compliant clients
                // (and our frontend's fetchRetry429) never saw a `Retry-After` to
                // honor and surfaced "Too Many Requests! Wait for 1s" as an error.
                .error_handler(|err| match err {
                    GovernorError::TooManyRequests { wait_time, headers } => {
                        let mut resp = axum::response::Response::new(axum::body::Body::from(
                            format!("Rate limit reached — retry in {wait_time}s."),
                        ));
                        *resp.status_mut() = StatusCode::TOO_MANY_REQUESTS;
                        if let Some(h) = headers {
                            *resp.headers_mut() = h;
                        }
                        let secs = HeaderValue::from_str(&wait_time.to_string())
                            .unwrap_or(HeaderValue::from_static("1"));
                        resp.headers_mut()
                            .insert(axum::http::header::RETRY_AFTER, secs);
                        resp
                    }
                    GovernorError::UnableToExtractKey => {
                        let mut resp = axum::response::Response::new(axum::body::Body::from(
                            "Unable to extract rate-limit key".to_string(),
                        ));
                        *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                        resp
                    }
                    GovernorError::Other { code, msg, headers } => {
                        let mut resp = axum::response::Response::new(axum::body::Body::from(
                            msg.unwrap_or_else(|| "rate limiter error".to_string()),
                        ));
                        *resp.status_mut() = code;
                        if let Some(h) = headers {
                            *resp.headers_mut() = h;
                        }
                        resp
                    }
                })
                .finish()
                .unwrap(),
        )
    };

    // Brute-force protection on auth: 10/min sustained (1 token / 6s), burst of 8.
    // Applied only to login/register/refresh — not to the whole API.
    let auth_rate_conf = make_rate_conf(6, 8);

    // M-6: SPARQL / graph-store rate limiting — looser than auth (these are functional, not auth).
    // 60/min sustained (1 token/s) with a burst of 40: one interactive page can
    // legitimately fire a dozen-plus small scoped queries at once (per-graph
    // content-kind checks, ontology loads, graph expansions, previews), and the
    // old 15-burst tripped real users mid-click. A runaway query loop still gets
    // cut off within seconds, and clients are told when to come back via
    // Retry-After (see make_rate_conf's error_handler).
    let sparql_rate_conf = make_rate_conf(1, 40);

    // Bulk import: a single authenticated upload may legitimately ship many
    // files, and power users ramp through the wizard repeatedly, so the burst
    // allowance is large (30 — bigger than the SPARQL limiter's 15). The sustained
    // refill is deliberately slow, one token every 10s (6/min), because each import
    // is heavy: a bigger burst than SPARQL, but a tighter sustained rate.
    let bulk_import_rate_conf = make_rate_conf(10, 30);

    // Public auth routes (no auth required) — rate-limited against brute force
    let auth_public_routes = Router::new()
        .route("/api/auth/register", post(handlers::register))
        .route("/api/auth/login", post(handlers::login))
        .route("/api/auth/refresh", post(handlers::refresh))
        .route("/api/auth/logout", post(handlers::logout))
        // Account recovery + email confirmation: enumeration-safe by design,
        // but still behind the brute-force limiter (token/code guessing).
        .route("/api/auth/forgot-password", post(handlers::forgot_password))
        .route("/api/auth/forgot-username", post(handlers::forgot_username))
        .route("/api/auth/reset-password", post(handlers::reset_password))
        .route("/api/auth/verify-email", post(handlers::verify_email))
        // Second step of a 2FA login (code guessing → same limiter).
        .route("/api/auth/2fa/verify", post(handlers::verify_2fa))
        // Passkey (WebAuthn) login: discoverable-credential challenge +
        // assertion. Unauthenticated by nature → same brute-force limiter.
        .route(
            "/api/auth/passkeys/login/start",
            post(crate::auth::passkey::login_start),
        )
        .route(
            "/api/auth/passkeys/login/finish",
            post(crate::auth::passkey::login_finish),
        )
        .route("/api/auth/features", get(handlers::auth_features))
        .route_layer(GovernorLayer {
            config: auth_rate_conf.clone(),
        })
        .with_state(state.clone());

    // Sensitive protected auth routes: require auth AND the brute-force limiter.
    // These are the high-value mutations a hijacked session could abuse —
    // password changes, API-token minting/revocation and account destruction —
    // so they share the strict auth limiter. The frequently-polled read routes
    // (e.g. GET /api/auth/me) stay in the unthrottled protected group below.
    let auth_sensitive_routes = Router::new()
        .route("/api/auth/change-password", post(handlers::change_password))
        .route("/api/auth/change-email", post(handlers::change_email))
        .route(
            "/api/auth/verify-email/resend",
            post(handlers::resend_verification),
        )
        .route("/api/auth/2fa/setup", post(handlers::totp_setup))
        .route("/api/auth/2fa/enable", post(handlers::totp_enable))
        .route("/api/auth/2fa/disable", post(handlers::totp_disable))
        // Passkey management: enrolling a new credential is as sensitive as a
        // password change; removal additionally re-proves the password.
        .route(
            "/api/auth/passkeys",
            get(crate::auth::passkey::list_passkeys),
        )
        .route(
            "/api/auth/passkeys/register/start",
            post(crate::auth::passkey::register_start),
        )
        .route(
            "/api/auth/passkeys/register/finish",
            post(crate::auth::passkey::register_finish),
        )
        .route(
            "/api/auth/passkeys/:credential_id",
            delete(crate::auth::passkey::delete_passkey),
        )
        .route(
            "/api/auth/tokens",
            get(handlers::list_api_tokens).post(handlers::create_api_token),
        )
        .route(
            "/api/auth/tokens/:token_id",
            delete(handlers::revoke_api_token),
        )
        // Self-service account management (deactivate / permanently delete own account)
        .route("/api/auth/account", delete(handlers::self_deactivate))
        .route("/api/auth/account/purge", post(handlers::self_purge))
        .route_layer(GovernorLayer {
            config: auth_rate_conf.clone(),
        })
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // Protected auth routes (auth required). Read-mostly / frequently polled by
    // the frontend, so intentionally not rate-limited.
    let auth_protected_routes = Router::new()
        .route("/api/auth/me", get(handlers::me).put(handlers::update_me))
        .route("/api/me/dataset-usage", get(handlers::my_dataset_usage))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // Admin user management routes (auth + admin required)
    let admin_routes = Router::new()
        .route(
            "/api/admin/users",
            get(handlers::admin_list_users).post(handlers::admin_create_user),
        )
        .route(
            "/api/admin/users/:user_id",
            get(handlers::admin_get_user)
                .put(handlers::admin_update_user)
                .delete(handlers::admin_delete_user),
        )
        .route(
            "/api/admin/users/:user_id/identities",
            get(handlers::admin_list_user_identities),
        )
        .route(
            "/api/admin/users/:user_id/reset-password",
            post(handlers::admin_reset_password),
        )
        .route(
            "/api/admin/users/:user_id/purge",
            post(handlers::admin_purge_user),
        )
        .route(
            "/api/admin/dataset-usage",
            get(handlers::admin_dataset_usage),
        )
        .route(
            "/api/admin/audit",
            get(crate::auth::audit::admin_list_audit),
        )
        .route(
            "/api/admin/audit/export",
            get(crate::auth::audit::admin_export_audit),
        )
        .route(
            "/api/admin/backup",
            get(crate::backup::admin_list_backups).post(crate::backup::admin_create_backup),
        )
        .route(
            "/api/admin/backup/:id/verify",
            post(crate::backup::admin_verify_backup),
        )
        .route(
            "/api/admin/llm/requests",
            get(llm_guard::admin_list_llm_requests),
        )
        .route("/api/admin/llm/stats", get(llm_guard::admin_llm_stats))
        .route_layer(middleware::from_fn(require_admin))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // Spark chat history + user memory (strictly per-user, so auth required).
    let llm_history_routes = llm_history::llm_history_routes()
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // Organisation routes – optional_auth so unauthenticated requests reach the handler;
    // write handlers enforce authentication themselves via require_user().
    let org_mixed_routes = Router::new()
        .route(
            "/api/organisations",
            get(handlers::list_organisations).post(handlers::create_organisation),
        )
        .route(
            "/api/organisations/:org_id",
            get(handlers::get_organisation)
                .put(handlers::update_organisation)
                .delete(handlers::delete_organisation),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());

    // Organisation & Group sub-routes (auth required for all methods)
    let org_routes = Router::new()
        .route(
            "/api/organisations/:org_id/members",
            get(handlers::list_org_members).post(handlers::add_org_member),
        )
        .route(
            "/api/organisations/:org_id/members/:user_id",
            delete(handlers::remove_org_member).put(handlers::update_org_member_role),
        )
        .route(
            "/api/organisations/:org_id/groups",
            post(handlers::create_group).get(handlers::list_groups),
        )
        .route(
            "/api/organisations/:org_id/groups/:group_id",
            get(handlers::get_group)
                .put(handlers::update_group)
                .delete(handlers::delete_group),
        )
        .route(
            "/api/organisations/:org_id/groups/:group_id/members",
            get(handlers::list_group_members).post(handlers::add_group_member),
        )
        .route(
            "/api/organisations/:org_id/groups/:group_id/members/:user_id",
            delete(handlers::remove_group_member),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // Public user listing (no auth required — only returns id/username/avatar_key).
    // Must be registered before user_routes so the static segment "public" wins
    // over the dynamic ":user_id" capture.
    let public_user_routes = Router::new()
        .route("/api/users/public", get(handlers::list_public_users))
        .with_state(state.clone());

    // User admin routes (auth required) — legacy, kept for backward compat
    let user_routes = Router::new()
        .route("/api/users", get(handlers::list_users))
        .route(
            "/api/users/:user_id",
            get(handlers::get_user).delete(handlers::delete_user),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // Dataset routes – optional_auth so unauthenticated requests reach the handler;
    // write handlers enforce authentication themselves via require_user().
    let dataset_mixed_routes = Router::new()
        .route(
            "/api/datasets",
            get(handlers::list_datasets).post(handlers::create_dataset),
        )
        .route(
            "/api/datasets/:dataset_id",
            get(handlers::get_dataset)
                .put(handlers::update_dataset)
                .delete(handlers::delete_dataset),
        )
        .route(
            "/api/datasets/:dataset_id/graphs",
            get(handlers::list_dataset_graphs)
                .post(handlers::add_dataset_graph)
                .patch(handlers::patch_dataset_graph_role)
                .delete(handlers::remove_dataset_graph),
        )
        .route(
            "/api/datasets/:dataset_id/commits",
            get(handlers::list_dataset_commits),
        )
        .route(
            "/api/datasets/:dataset_id/services",
            get(handlers::list_services).post(handlers::create_service),
        )
        .route(
            "/api/datasets/:dataset_id/services/:service_id",
            get(handlers::get_service)
                .put(handlers::update_service)
                .delete(handlers::delete_service),
        )
        .route(
            "/api/datasets/:dataset_id/services/:service_id/graphs",
            get(handlers::list_service_graphs)
                .post(handlers::add_service_graph)
                .delete(handlers::remove_service_graph),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());

    // Dataset routes that have no conflicting public variant (auth required for all methods)
    let dataset_protected_routes = Router::new()
        .route(
            "/api/datasets/:dataset_id/shacl",
            put(handlers::update_dataset_shacl),
        )
        .route(
            "/api/datasets/:dataset_id/role",
            put(handlers::update_dataset_role),
        )
        .route(
            "/api/datasets/:dataset_id/access",
            get(handlers::list_access).post(handlers::grant_access),
        )
        .route(
            "/api/datasets/:dataset_id/access/:user_id",
            delete(handlers::revoke_access),
        )
        .route(
            "/api/datasets/:dataset_id/grants",
            get(handlers::list_dataset_grants).put(handlers::set_dataset_grant),
        )
        .route(
            "/api/datasets/:dataset_id/grants/:principal_type/:principal_id",
            delete(handlers::revoke_dataset_grant),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // Asset upload/management — binary uploads, so a larger per-route body limit than the 8 MB
    // global default. The ceiling mirrors the form service's absolute asset max
    // (Settings.asset_max_bytes) so a body the form front-door accepts is not then rejected here;
    // upload_asset additionally streams the part and aborts past ASSET_MAX_BYTES. The +1 MiB is
    // headroom for the multipart envelope so a full-size file isn't refused at the transport edge.
    let asset_routes = Router::new()
        .route(
            "/api/datasets/:dataset_id/assets",
            get(routes::list_assets).post(routes::upload_asset),
        )
        .route(
            "/api/datasets/:dataset_id/assets/:asset_id",
            get(routes::download_asset)
                .delete(routes::delete_asset)
                .patch(routes::patch_asset_metadata),
        )
        .route(
            "/api/datasets/:dataset_id/assets/:asset_id/visibility",
            put(routes::update_asset_visibility),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .layer(DefaultBodyLimit::max(routes::ASSET_MAX_BYTES + 1024 * 1024))
        .with_state(state.clone());

    // Dataset SPARQL service endpoint (optional auth for access control).
    // Anonymously reachable for public datasets, so it carries the SPARQL rate
    // limiter (the global /sparql endpoint already does) to bound query-DoS.
    let dataset_sparql_routes = Router::new()
        .route(
            "/api/datasets/:dataset_id/services/:service_slug/sparql",
            get(routes::dataset_sparql_query).post(routes::dataset_sparql_post),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .route_layer(GovernorLayer {
            config: sparql_rate_conf.clone(),
        })
        .with_state(state.clone());

    // User avatar — upload requires auth, download is public
    let avatar_routes = Router::new()
        .route("/api/users/me/avatar", put(handlers::upload_user_avatar))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    let avatar_get_routes = Router::new()
        .route("/api/users/:user_id/avatar", get(handlers::get_user_avatar))
        .with_state(state.clone());

    // Org image
    let org_image_routes = Router::new()
        .route(
            "/api/organisations/:org_id/image",
            put(handlers::upload_org_image).get(handlers::get_org_image),
        )
        .route(
            "/api/organisations/:org_id/banner",
            put(handlers::upload_org_banner).get(handlers::get_org_banner),
        )
        .route(
            "/api/organisations/:org_id/banner-preset",
            put(handlers::set_org_banner_preset),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());

    // Dataset image
    let dataset_image_routes = Router::new()
        .route(
            "/api/datasets/:dataset_id/image",
            put(handlers::upload_dataset_image).get(handlers::get_dataset_image),
        )
        .route(
            "/api/datasets/:dataset_id/banner",
            put(handlers::upload_dataset_banner).get(handlers::get_dataset_banner),
        )
        .route(
            "/api/datasets/:dataset_id/banner-preset",
            put(handlers::set_dataset_banner_preset),
        )
        // Viewer feed: per-element geometry + 3D-file references (map/3D viewers).
        // Optional auth so public datasets are viewable anonymously.
        .route(
            "/api/datasets/:dataset_id/viewer-feed",
            get(routes::viewer_feed),
        )
        // Geo capability summary (gates the map / 3D-viewer UI affordances).
        .route(
            "/api/datasets/:dataset_id/geo-stats",
            get(routes::geo_stats),
        )
        // Batched, scope-wide geo capability — one OR-aggregated probe instead of
        // one `/geo-stats` request per dataset (the triple browser's Map gate).
        .route("/api/geo-stats", get(routes::geo_stats_batch))
        // Anonymous-capable asset download (dataset visibility decides) — the
        // viewer fetches e.g. the original IFC file through this without auth.
        .route(
            "/api/datasets/:dataset_id/assets/:asset_id/download",
            get(routes::download_asset_public),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());

    // SHACL validation routes
    let shacl_routes = Router::new()
        .route(
            "/api/datasets/:dataset_id/validate",
            post(routes::validate_dataset),
        )
        .route(
            "/api/datasets/:dataset_id/validation/latest",
            get(routes::get_latest_validation_run),
        )
        .route(
            "/api/datasets/:dataset_id/validation/history",
            get(routes::get_validation_history),
        )
        .route(
            "/api/datasets/:dataset_id/validation/runs/:run_id",
            get(routes::get_validation_run),
        )
        .route(
            "/api/shacl/validation/latest",
            post(routes::list_latest_validation_runs),
        )
        .route(
            "/api/datasets/:dataset_id/shapes",
            get(routes::get_shapes).put(routes::put_shapes),
        )
        .route(
            "/api/datasets/:dataset_id/infer",
            post(routes::infer_dataset),
        )
        .route("/api/shacl/detect-shapes", get(routes::detect_shapes))
        // Legacy "datasets that have a shapes graph" selector (Validation page).
        // Distinct from the Studio's /api/shacl/shape-graphs Library CRUD.
        .route(
            "/api/shacl/dataset-shape-graphs",
            get(routes::list_accessible_shape_graphs),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // SHACLC standalone conversion routes (no auth required). Anonymous parser
    // surface — rate-limited so it can't be used for cheap CPU-DoS / fuzzing.
    let shaclc_routes = Router::new()
        .route("/api/shaclc/parse", post(routes::shaclc_parse))
        .route("/api/shaclc/serialize", post(routes::shaclc_serialize))
        .route_layer(GovernorLayer {
            config: sparql_rate_conf.clone(),
        })
        .with_state(state.clone());

    // SHACL Studio: shape graphs (Library), pipelines, runs, model-context, derive.
    let studio_auth = crate::shacl_studio::routes::studio_auth_routes()
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // SHACL Studio (optional auth) — the form-manifest is anonymous-readable
    // for public datasets and auth-gated otherwise (enforced inside the handler).
    let studio_optional = crate::shacl_studio::routes::studio_optional_auth_routes()
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());

    // RML mapping routes (auth required)
    let rml_routes = Router::new()
        .route(
            "/api/datasets/:dataset_id/mappings",
            get(routes::get_rml_mapping).put(routes::put_rml_mapping),
        )
        .route(
            "/api/datasets/:dataset_id/mappings/execute",
            post(routes::execute_rml_mapping),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // RML standalone preview (no auth required). Anonymous RML execution into a
    // throwaway store — rate-limited to bound anonymous CPU/RAM work.
    let rml_preview_routes = Router::new()
        .route("/api/rml/preview", post(routes::rml_preview))
        .route_layer(GovernorLayer {
            config: sparql_rate_conf.clone(),
        })
        .with_state(state.clone());

    // Triple browsing API (optional auth)
    let browse_routes = Router::new()
        .route("/api/browse/graphs", get(routes::browse_graphs))
        .route("/api/browse/triples", get(routes::browse_triples))
        .route("/api/browse/facets", get(routes::browse_facets))
        .route("/api/browse/resource", get(routes::browse_resource))
        .route("/api/browse/stats", get(routes::browse_stats))
        .route("/api/browse/suggest", get(routes::browse_suggest))
        // endpoint_acl_guard is the inner layer (runs after optional_auth sets the user)
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            endpoint_acl_guard,
        ))
        // optional_auth is the outer layer (runs first, sets AuthenticatedUser in extensions)
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());

    // SPARQL query + Graph Store reads + management routes (optional auth for visibility scoping)
    // SetResponseHeaderLayer adds Vary: Accept to all responses (required for correct caching
    // M-6: SPARQL query/update routes — rate-limited per IP (separate from auth rate limit).
    let sparql_routes = Router::new()
        .merge(routes::sparql_routes())
        .merge(llm_sparql::llm_routes())
        .merge(routes::graph_store_read_routes())
        .merge(routes::management_routes())
        .route_layer(GovernorLayer {
            config: sparql_rate_conf.clone(),
        })
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("vary"),
            HeaderValue::from_static("Accept"),
        ))
        .with_state(state.clone());

    // Graph Store write routes (authentication required)
    // M-6: Generous 50 MB per-route body limit for RDF file uploads; rate-limited.
    let graph_store_write_routes = Router::new()
        .merge(routes::graph_store_write_routes())
        .route_layer(GovernorLayer {
            config: sparql_rate_conf.clone(),
        })
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
        .with_state(state.clone());

    // Bulk multi-file import (authentication required)
    // 200 MB body limit (multi-file upload); rate-limited like other write paths.
    let bulk_import_routes = Router::new()
        .merge(crate::imports::routes::bulk_import_routes())
        .route_layer(GovernorLayer {
            config: bulk_import_rate_conf,
        })
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .layer(DefaultBodyLimit::max(200 * 1024 * 1024))
        .with_state(state.clone());

    // Batch SPARQL UPDATE routes (authentication required)
    // M-6: 10 MB body limit for batch updates; rate-limited.
    let batch_update_routes = Router::new()
        .merge(routes::sparql_batch_routes())
        .route_layer(GovernorLayer {
            config: sparql_rate_conf.clone(),
        })
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
        .with_state(state.clone());

    // Linked data asset endpoint — public IRI, optional auth (public datasets accessible unauthenticated)
    let asset_ld_routes = Router::new()
        .route(
            "/datasets/:dataset_id/assets/:asset_id",
            axum::routing::get(routes::linked_data_asset),
        )
        // Typed-metadata JSON view — same optional-auth access control as the linked-data endpoint.
        .route(
            "/api/datasets/:dataset_id/assets/:asset_id/metadata",
            axum::routing::get(routes::asset_metadata),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());

    // IRI dereference + VoID dataset description (FAIR A + I)
    let dereference_routes = Router::new()
        .merge(linked_data::dereference_routes())
        .merge(linked_data::well_known_routes())
        .merge(linked_data::well_known_org_routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());

    // `frame-ancestors 'none'` (anti-clickjacking), `object-src 'none'` and
    // `base-uri 'self'` (anti-injection) added alongside the existing directives.
    let csp_value = HeaderValue::from_static(
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'self'; form-action 'self'"
    );

    // Reasoning routes (always compiled; feature gates are inside the handler)
    let reasoning_api_routes = Router::new()
        .merge(routes::reasoning_routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // Data-model registry — public read routes
    let data_model_read = Router::new()
        .merge(data_model_public_routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());

    // Data-model registry — write routes
    let data_model_write = Router::new()
        .merge(data_model_auth_routes())
        .route_layer(middleware::from_fn(require_publisher))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // Dataset versioning — public read routes (visibility scoped via optional_auth)
    let dataset_version_read = Router::new()
        .merge(dataset_version_public_routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());

    // Dataset versioning — write routes (per-dataset write checks inside handlers)
    let dataset_version_write = Router::new()
        .merge(dataset_version_auth_routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // Saved queries — read/run routes (optional_auth; public scopes reachable
    // anonymously, non-public require an API token, enforced in the handlers).
    let saved_query_read = Router::new()
        .merge(saved_query_public_routes())
        // Anonymous-reachable parameterised SPARQL (`…/run`, `…/openapi.json`) — throttle like the
        // other expensive SPARQL groups so a single IP can't saturate the blocking pool for free.
        .route_layer(GovernorLayer {
            config: sparql_rate_conf.clone(),
        })
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());

    // Saved queries — management routes (require_auth; owner-admin + write scope
    // checked inside the handlers).
    let saved_query_write = Router::new()
        .merge(saved_query_auth_routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // LDP routes (feature-gated). Mounted behind `require_auth`: the LDP handlers
    // read/write the shared store via raw SPARQL with no per-graph scoping, so an
    // unauthenticated mount allowed anonymous `PATCH /ldp/*` (arbitrary SPARQL
    // UPDATE — e.g. `DROP GRAPH`) and `Slug`/path SPARQL injection against ANY
    // tenant's graphs. Requiring auth closes the anonymous-access hole; full
    // per-graph ACL scoping for authenticated LDP writes is tracked as a follow-up.
    #[cfg(feature = "ldp")]
    let ldp_router = {
        use crate::ldp::ldp_routes;
        Router::new()
            .merge(ldp_routes())
            .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
            .with_state(state.clone())
    };

    // ── ACL management routes (admin required) ────────────────────────────
    let acl_admin_routes = Router::new()
        .route(
            "/api/admin/acl/endpoints",
            get(acl_handlers::list_endpoint_acl_rules).post(acl_handlers::create_endpoint_acl_rule),
        )
        .route(
            "/api/admin/acl/endpoints/:id",
            put(acl_handlers::update_endpoint_acl_rule)
                .delete(acl_handlers::delete_endpoint_acl_rule),
        )
        .route(
            "/api/admin/acl/graphs",
            get(acl_handlers::list_graph_acl_rules).post(acl_handlers::grant_graph_permission),
        )
        .route(
            "/api/admin/acl/graphs/:id",
            delete(acl_handlers::revoke_graph_permission),
        )
        .route(
            "/api/admin/acl/triples",
            get(acl_handlers::list_triple_security_labels)
                .post(acl_handlers::create_triple_security_label),
        )
        .route(
            "/api/admin/acl/triples/:id",
            delete(acl_handlers::delete_triple_security_label),
        )
        .route_layer(middleware::from_fn(require_admin))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // ── OAuth provider admin routes (admin required) ──────────────────────
    let oauth_admin_routes = Router::new()
        .route(
            "/api/admin/oauth/providers",
            get(oauth_handlers::admin_list_providers).post(oauth_handlers::admin_create_provider),
        )
        .route(
            "/api/admin/oauth/providers/:id",
            get(oauth_handlers::admin_get_provider)
                .put(oauth_handlers::admin_update_provider)
                .delete(oauth_handlers::admin_delete_provider),
        )
        .route_layer(middleware::from_fn(require_admin))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state.clone());

    // ── OAuth/SAML flow routes (public, rate-limited) ─────────────────────
    let oauth_flow_routes = Router::new()
        .route(
            "/api/auth/oauth/providers",
            get(oauth_handlers::list_active_providers),
        )
        .route(
            "/api/auth/oauth/:slug/authorize",
            get(oauth_handlers::oidc_authorize),
        )
        .route(
            "/api/auth/oauth/:slug/callback",
            get(oauth_handlers::oidc_callback),
        )
        .route(
            "/api/auth/saml/:slug/metadata",
            get(oauth_handlers::saml_metadata),
        )
        .route("/api/auth/saml/:slug/acs", post(oauth_handlers::saml_acs))
        // Inject the OAuth session store as a layer extension
        .layer(axum::Extension(state.oauth_sessions.clone()))
        // Brute-force / DoS limiter on the unauthenticated SSO surface: `authorize`
        // triggers outbound OIDC discovery + creates server-side PKCE session state,
        // and `acs` parses signed XML — both are cheap-to-spam without a throttle.
        .route_layer(GovernorLayer {
            config: auth_rate_conf.clone(),
        })
        .with_state(state.clone());

    // In-app documentation API (optional auth; admin-only docs filtered + admin
    // CRUD enforced in-handler).
    let docs_routes = crate::docs::routes()
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());

    let mut router = Router::new()
        .merge(docs_routes)
        .merge(auth_public_routes)
        .merge(auth_sensitive_routes)
        .merge(auth_protected_routes)
        .merge(admin_routes)
        .merge(acl_admin_routes)
        .merge(oauth_admin_routes)
        .merge(oauth_flow_routes)
        .merge(org_mixed_routes)
        .merge(org_routes)
        .merge(public_user_routes)
        .merge(user_routes)
        .merge(avatar_routes)
        .merge(avatar_get_routes)
        .merge(org_image_routes)
        .merge(dataset_image_routes)
        .merge(dataset_mixed_routes)
        .merge(dataset_protected_routes)
        .merge(asset_routes)
        .merge(dataset_sparql_routes)
        .merge(shacl_routes)
        .merge(shaclc_routes)
        .merge(studio_auth)
        .merge(studio_optional)
        .merge(rml_routes)
        .merge(rml_preview_routes)
        .merge(browse_routes)
        .merge(sparql_routes)
        .merge(llm_history_routes)
        .merge(graph_store_write_routes)
        .merge(bulk_import_routes)
        .merge(batch_update_routes)
        .merge(asset_ld_routes)
        .merge(dereference_routes)
        .merge(reasoning_api_routes)
        .merge(data_model_read)
        .merge(data_model_write)
        .merge(dataset_version_read)
        .merge(dataset_version_write)
        .merge(saved_query_read)
        .merge(saved_query_write)
        .merge(
            Router::new()
                .merge(catalog_routes())
                .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
                .with_state(state.clone()),
        );

    // OGC API – Features (Core, P4). Nested router so `optional_auth` populates
    // AuthenticatedUser for logged-in callers while public datasets stay
    // anonymously reachable (handlers gate on can_access_dataset).
    router = router.merge(
        Router::new()
            .merge(crate::ogcapi::ogcapi_routes())
            .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
            .with_state(state.clone()),
    );

    // 3D Tiles 1.1 (P5): tileset.json + content.glb, anonymous-capable.
    #[cfg(feature = "geometry3d")]
    {
        let tiles3d_routes = Router::new()
            .merge(crate::tiles3d::tiles3d_routes())
            .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
            .with_state(state.clone());
        router = router.merge(tiles3d_routes);
    }

    #[cfg(feature = "ldp")]
    {
        router = router.merge(ldp_router);
    }

    // ShEx validation routes (feature-gated, auth required)
    #[cfg(feature = "shex")]
    {
        let shex_auth_routes = Router::new()
            .merge(routes::shex_routes())
            .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
            .with_state(state.clone());
        router = router.merge(shex_auth_routes);
    }

    // SWRL rule execution routes (feature-gated, auth required)
    #[cfg(feature = "swrl")]
    {
        let swrl_auth_routes = Router::new()
            .merge(routes::swrl_routes())
            .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
            .with_state(state.clone());
        router = router.merge(swrl_auth_routes);
    }

    // NOTE: utoipa-swagger-ui 7.x is incompatible with axum 0.7 / matchit 0.7
    // because it internally registers "/api-docs/{_:.*}" which uses regex syntax
    // unsupported by matchit, causing a panic at startup.
    // Swagger UI is therefore disabled; the OpenAPI JSON spec is served below.
    // TODO: upgrade utoipa-swagger-ui to 8.x when migrating to axum 0.8.
    // optional_auth so the handler can tailor the spec to the caller: token-required
    // operations are hidden from anonymous callers, and Admin operations from non-admins.
    let openapi_doc_route = Router::new()
        .route("/api-docs/openapi.json", get(openapi::openapi_json_handler))
        .route_layer(middleware::from_fn_with_state(state.clone(), optional_auth))
        .with_state(state.clone());
    router = router.merge(openapi_doc_route);

    // Compile-time plugins (src/plugins.rs): `GET /api/plugins` lists what's
    // compiled in, and each registered plugin's routes are nested under
    // `/ext/<name>`. Both are unconditional — with zero `plugin-*` features
    // enabled the list is empty and no `/ext/*` routes exist, matching
    // upstream behavior exactly.
    let plugin_ctx = crate::plugins::plugin_context(&state);
    router = router.route("/api/plugins", get(crate::plugins::list_plugins));
    router = crate::plugins::mount_plugins(router, &plugin_ctx);

    let mut router = router
        // Innermost global layer (added first ⇒ closest to the route handlers): turn a
        // panic in any handler into a clean `500` instead of letting it unwind the
        // per-connection task. Without this a single malformed request that trips a
        // panic (a failed `unwrap`, an out-of-bounds slice, a debug-build arithmetic
        // overflow, or a panic raised deep inside a parsing library on adversarial
        // input) drops the connection with no status code. Placing it *inside* the
        // timeout / compression / CORS / security-header / trace / request-id layers
        // means the synthesised 500 still receives all of those (security headers,
        // gzip, an `x-request-id`, a trace span), exactly like a normal error response.
        .layer(tower_http::catch_panic::CatchPanicLayer::custom(
            handle_request_panic,
        ))
        // Generous global request timeout as a DoS backstop for stuck/slow handlers.
        // It bounds the time to PRODUCE a response, not body streaming: SPARQL
        // results stream after a fast first-byte response, so large exports are not
        // truncated. The per-query 30s timeout and the expensive-op semaphore are the
        // primary guards; this catches anything that slips past them. (Slow-header
        // Slowloris is handled at the reverse proxy.)
        // `with_status_code` replaces the deprecated `TimeoutLayer::new`; passing
        // REQUEST_TIMEOUT (408) keeps the exact response status `new` defaulted to.
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(300),
        ))
        // M-6: Default body limit for all other endpoints (JSON API, image uploads, etc.).
        // Graph store and batch-update routes have their own per-route limits applied above.
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024)) // 8 MB global default
        // Compress responses with gzip or brotli when the client sends Accept-Encoding.
        // This can reduce SPARQL JSON payloads by 70–90 % (e.g. 50 MB → 5 MB).
        .layer(CompressionLayer::new())
        .layer(build_cors_layer(cors_origins))
        // Re-attach the LDP capability headers that the CORS preflight short-circuit
        // would otherwise drop. Must sit just outside the CORS layer so it runs on the
        // preflight response CORS produced (see `ldp_options_capabilities`).
        .layer(middleware::from_fn(ldp_options_capabilities))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("content-security-policy"),
            csp_value,
        ))
        // Defense-in-depth security headers applied to every response:
        //  - X-Frame-Options: belt-and-braces clickjacking defense for agents that
        //    don't honour CSP `frame-ancestors`.
        //  - X-Content-Type-Options: stop MIME sniffing (e.g. an uploaded SVG/HTML
        //    asset being interpreted as active content — see asset handlers).
        //  - Referrer-Policy: don't leak full URLs (which can carry IRIs/ids) cross-origin.
        //  - Permissions-Policy: drop access to powerful features the app doesn't use.
        //  - Cross-Origin-Opener-Policy: isolate the browsing context.
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("geolocation=(), microphone=(), camera=(), payment=()"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(request_id_middleware));

    // HSTS only when TLS is in use (the `secure_cookies` flag is the operator's
    // signal that the service is fronted by HTTPS). Sending HSTS over plain HTTP
    // would be wrong and could brick local-dev access.
    if state.secure_cookies {
        router = router.layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("strict-transport-security"),
            HeaderValue::from_static("max-age=63072000; includeSubDomains"),
        ));
    }

    router
}

/// Per-request correlation ID. Inserted as a request extension and as the
/// `x-request-id` response header so audit events and traces can be linked
/// back to a specific HTTP request.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

async fn request_id_middleware(
    mut req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let span = tracing::Span::current();
    span.record("request_id", tracing::field::display(&id));
    req.extensions_mut().insert(RequestId(id.clone()));
    let mut resp = next.run(req).await;
    if let Ok(hv) = axum::http::HeaderValue::from_str(&id) {
        resp.headers_mut().insert("x-request-id", hv);
    }
    resp
}

/// Panic handler for the [`tower_http::catch_panic::CatchPanicLayer`] wrapped around
/// every route (see [`build_router`]).
///
/// A panic in a handler — a failed `unwrap`/`expect`, an out-of-bounds index, a
/// debug-build arithmetic overflow, or a panic raised deep inside a parsing library
/// on adversarial input — would otherwise unwind the per-connection task, leaving the
/// client with an abrupt connection reset (no status code) and the failure visible
/// only as a stack trace in the logs. Catching it turns every such panic into a
/// normal `500 Internal Server Error`, so one malformed request can no longer drop
/// its connection. The response is deliberately generic: the panic payload is logged
/// server-side but never sent to the client, matching [`error::AppError::Internal`].
fn handle_request_panic(err: Box<dyn std::any::Any + Send + 'static>) -> axum::response::Response {
    // The panic payload is the value passed to `panic!` — almost always a `&str`
    // (string literals) or a `String` (formatted messages, e.g. from `unwrap`).
    let detail = if let Some(s) = err.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = err.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    };
    // This layer runs inside TraceLayer + request_id_middleware, so the error line is
    // emitted within the per-request span and is correlated with its `x-request-id`.
    tracing::error!("caught panic in request handler: {detail}");

    axum::http::Response::builder()
        .status(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        .header(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )
        .body(axum::body::Body::from("Internal server error"))
        // Infallible: a static status + header + body can never be a builder error.
        .expect("static 500 response is always valid")
}

/// Path-aware framing policy, applied OUTSIDE the whole router (API routes,
/// their security-header layers, and the static SPA fallback):
///
///  - `/embed/*` pages exist to be iframed by third-party sites, so they get
///    `Content-Security-Policy: frame-ancestors <EMBED_FRAME_ANCESTORS>`
///    (default `*`) and no `X-Frame-Options`. Operators can restrict embedding
///    to specific hosts (`EMBED_FRAME_ANCESTORS="https://example.com"`) or
///    disable it (`"'none'"`).
///  - Every other response that carries NO CSP — which is exactly the static
///    SPA files, since they are served by the fallback service that the
///    security-header layers in [`build_router`] do not wrap — gets the
///    anti-clickjacking pair the API already has. This closes the gap where
///    the app shell itself was frameable while the API was not.
///
/// API responses already carry the strict CSP from [`build_router`]; those are
/// left untouched (except under `/embed`, a path no API route uses).
async fn frame_policy_headers(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    static EMBED_ANCESTORS: std::sync::OnceLock<Option<HeaderValue>> = std::sync::OnceLock::new();
    let ancestors = EMBED_ANCESTORS.get_or_init(|| {
        let v = std::env::var("EMBED_FRAME_ANCESTORS").unwrap_or_else(|_| "*".to_string());
        let v = v.trim().to_string();
        if v.is_empty() || v == "'none'" || v == "none" {
            None // embedding disabled — /embed falls through to the default policy
        } else {
            HeaderValue::from_str(&format!("frame-ancestors {v}")).ok()
        }
    });

    let is_embed = req.uri().path().starts_with("/embed");
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    // The SPA document can also leave through content-negotiating API routes
    // (`/` and `/sparql` serve index.html to browsers — see `spa_shell_response`).
    // Those picked up the strict API CSP (`connect-src 'self'`, …) from the
    // inner layers, which blocks the viewer's external basemap tiles and the
    // Cesium CDN for the whole SPA session — the "map never loads" failure in
    // production. The marker tells us to re-stamp them with the SPA policy.
    let spa_shell = headers.remove("x-ots-spa-shell").is_some();
    match ancestors {
        Some(allow) if is_embed => {
            headers.insert(
                HeaderName::from_static("content-security-policy"),
                allow.clone(),
            );
            headers.remove("x-frame-options");
        }
        _ => {
            if spa_shell || !headers.contains_key("content-security-policy") {
                headers.insert(
                    HeaderName::from_static("content-security-policy"),
                    HeaderValue::from_static("frame-ancestors 'self'"),
                );
                headers.insert(
                    HeaderName::from_static("x-frame-options"),
                    HeaderValue::from_static("SAMEORIGIN"),
                );
                headers.insert(
                    HeaderName::from_static("x-content-type-options"),
                    HeaderValue::from_static("nosniff"),
                );
            }
        }
    }
    resp
}

/// Start the HTTP server.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    store: TripleStore,
    prefix_registry: Arc<PrefixRegistry>,
    auth_db: Arc<AuthDb>,
    jwt_config: Arc<JwtConfig>,
    object_store: Arc<ObjectStore>,
    base_url: &str,
    addr: &str,
    cors_origins: &str,
    trusted_cidrs: Vec<IpNet>,
    query_timeout_secs: u64,
    write_timeout_secs: u64,
    secure_cookies: bool,
    serve_frontend: bool,
    seed_dir: Option<std::path::PathBuf>,
    // Opt-in fallback to any free port when `addr`'s port is already in use
    // (--port-fallback / PORT_FALLBACK, default off — see the listener bind
    // below). Upstream behavior when this is `false` is unchanged: a busy
    // port still refuses to start.
    port_fallback: bool,
    // Cross-app service-registry self-registration (--discovery /
    // LD_DISCOVERY), applied AFTER the listener is bound so a port_fallback
    // rewrite of the advertised URL is reflected in what gets registered.
    discovery: bool,
    registry_url: String,
    registry_token: String,
    #[cfg(feature = "text-search")] text_index: Option<Arc<TextIndex>>,
) -> anyhow::Result<()> {
    let audit = Arc::new(crate::auth::audit::AuditLogger::new(auth_db.pool()));

    // ── Backup subsystem (optional) ─────────────────────────────────────────
    let backup = {
        let dir = std::env::var("BACKUP_DIR").unwrap_or_else(|_| "data/backups".to_string());
        let sqlite =
            std::env::var("AUTH_DB_PATH").unwrap_or_else(|_| "data/auth.sqlite".to_string());
        let retention: usize = std::env::var("BACKUP_RETENTION_COUNT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(7);
        let encrypt = std::env::var("BACKUP_ENCRYPT")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        // Initialize backup encryption key (auto-generates if not present)
        let key_path = if encrypt {
            let default_path = std::path::PathBuf::from("data/backup_key.age");
            let key_file = std::env::var("BACKUP_ENCRYPT_KEY_PATH")
                .ok()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| default_path);
            match crate::backup::init_backup_encryption(&key_file) {
                Ok(path) => path,
                Err(e) => {
                    tracing::error!("Failed to initialize backup encryption: {}", e);
                    None
                }
            }
        } else {
            None
        };

        if !encrypt && std::path::Path::new(&dir).starts_with("data") {
            tracing::warn!(
                "Backup encryption is disabled. Recommended if data/ is not on an encrypted volume — set BACKUP_ENCRYPT=true"
            );
        }
        match crate::backup::BackupManager::new(
            std::path::PathBuf::from(&dir),
            std::path::PathBuf::from(&sqlite),
            store.clone(),
            audit.clone(),
            retention,
            encrypt,
            key_path,
        ) {
            Ok(m) => Some(Arc::new(m)),
            Err(e) => {
                tracing::warn!("backup: disabled — init failed: {}", e);
                None
            }
        }
    };

    // OIDC resource-server config from the environment (disabled unless OIDC_ISSUER set).
    let auth_ext = Arc::new(crate::auth::oidc_rs::AuthExt::from_env());
    if let Some(verifier) = auth_ext.oidc.as_ref() {
        match crate::auth::oidc_rs::ensure_env_provider(
            &auth_db,
            verifier.issuer(),
            &auth_ext.default_role,
        ) {
            Ok(_) => tracing::info!(
                "OIDC resource-server enabled (issuer={}, accept_legacy_tokens={})",
                verifier.issuer(),
                auth_ext.accept_legacy_tokens
            ),
            Err(e) => tracing::warn!("OIDC provider bootstrap failed: {e}"),
        }
    }

    // Transactional account email (verification, password reset). Logs-only
    // until SMTP_HOST is configured; links default to the public base URL.
    let mailer = Arc::new(crate::email::Mailer::from_env(
        base_url.trim_end_matches('/'),
    ));
    if mailer.smtp_configured() {
        tracing::info!("email: SMTP relay configured — account emails will be delivered");
    } else {
        tracing::info!(
            "email: SMTP not configured (set SMTP_HOST) — account emails are written to the log"
        );
    }

    // Hold a flush handle for graceful shutdown — `store` is moved into AppState below.
    let shutdown_store = store.clone();
    let state = AppState {
        store,
        prefix_registry,
        auth_db: auth_db.clone(),
        audit,
        backup: backup.clone(),
        jwt_config: jwt_config.clone(),
        object_store,
        mailer,
        base_url: Arc::new(base_url.trim_end_matches('/').to_string()),
        oauth_sessions: crate::auth::oauth::new_session_store(),
        passkey_sessions: crate::auth::passkey::new_session_store(),
        auth_ext,
        query_timeout_secs,
        write_timeout_secs,
        secure_cookies,
        browse_semaphore: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_BROWSE_QUERIES)),
        expensive_semaphore: Arc::new(tokio::sync::Semaphore::new(expensive_op_capacity())),
        #[cfg(feature = "text-search")]
        text_index,
        #[cfg(feature = "text-search")]
        text_dirty: Arc::new(AtomicBool::new(false)),
    };

    // Compile-time plugins (src/plugins.rs): on_boot + any background task,
    // once per process. A no-op with zero `plugin-*` features enabled.
    crate::plugins::boot_plugins(&crate::plugins::plugin_context(&state));

    // Spawn a background task to periodically prune expired PKCE OAuth sessions (L-7)
    {
        let sessions = state.oauth_sessions.clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(300); // 5 minutes
            loop {
                tokio::time::sleep(interval).await;
                crate::auth::oauth::prune_sessions(&sessions);
            }
        });
    }

    // Same periodic pruning for in-flight WebAuthn passkey challenges.
    {
        let sessions = state.passkey_sessions.clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(300); // 5 minutes
            loop {
                tokio::time::sleep(interval).await;
                crate::auth::passkey::prune_sessions(&sessions);
            }
        });
    }

    // Backup scheduler.
    if let Some(ref mgr) = backup {
        let hours: u64 = std::env::var("BACKUP_SCHEDULE_HOURS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(24);
        crate::backup::spawn_scheduler(mgr.clone(), hours, Some(state.object_store.clone()));
    }

    // Boot-time migration + seeding + the SHACL pipeline scheduler.
    {
        // One SEQUENTIAL blocking task, not two concurrent ones. The SHACL/standards
        // migration and the demo-data seed both write the same SQLite identity DB and
        // the same RDF store; running them concurrently produced "database is locked"
        // contention and a boot deadlock that left the public demo datasets
        // half-seeded — registered graphs with zero triples, which read as "no data"
        // to logged-out users and made the landing-page total count show zero.
        // Sequencing removes the contention and lets the dataset-metadata audit run
        // *after* the datasets it audits exist. Spawned (not awaited) so the server
        // still starts serving immediately.
        let seed_state = state.clone();
        let base = state.base_url.to_string();
        tokio::task::spawn_blocking(move || {
            let store = &seed_state.store;
            let auth = &seed_state.auth_db;
            // 1. SHACL Studio meta-shapes, legacy shape import, per-standard shapes.
            if let Err(e) = crate::shacl_studio::seed::seed_shacl_shacl(store, auth) {
                tracing::warn!("shacl_studio: SHACL-SHACL seed failed: {e}");
            }
            if let Err(e) = crate::shacl_studio::migrate::migrate_legacy(store, auth, &base) {
                tracing::warn!("shacl_studio: legacy migration failed: {e}");
            }
            // Self-healing: adopt every dataset's shapes graph(s) — configured
            // `shapes_graph_iri` or shapes-role dataset graphs — into the Studio
            // Library and bind them in the validation layer (idempotent).
            crate::shacl_studio::migrate::backfill_dataset_shapes(&seed_state);
            if let Err(e) = crate::shacl_studio::seed_standards::seed_standards(store, auth) {
                tracing::warn!("shacl_studio: standards seed failed: {e}");
            }
            // 2. Dataset-structure governance shapes (must exist before the audit).
            let _ = crate::auth::dataset_audit::seed_dataset_structure_shapes(store, auth);
            // 3. Bundled public demo org + datasets + graph data + saved queries.
            //    Idempotent and self-healing: back-fills any registered-but-empty
            //    public demo graph left behind by an earlier interrupted seed.
            crate::saved_queries::seed::seed_open_triplestore(&seed_state);
            // 3b. Operator-supplied seed bundles (--seed-dir / SEED_DIR), if any —
            //     same idempotent/fail-soft engine as the reference bundle above.
            //     Sequenced here (not a separate spawn) for the same reason the
            //     demo seed is: concurrent writers to the same SQLite identity DB
            //     and RDF store previously produced boot-time lock contention.
            if let Some(ref dir) = seed_dir {
                crate::seed_bundles::load_seed_dir(&seed_state, dir);
            }
            // 4. Standard RDF vocabularies into the model registry.
            crate::data_models::seed_vocab::seed_standard_vocabularies(&seed_state);
            // 5. Canonical dataset-metadata IRIs, then audit/repair — datasets exist now.
            crate::auth::dataset_graph::reconcile_all_dataset_metadata(
                store,
                &seed_state.base_url,
                auth,
            );
            // 5b. Model/Vocabulary/Instance reframe: reclassify stored property
            //     graphs (model→vocabulary) and rewrite legacy …/ontology/ IRIs to …/ns#.
            crate::auth::dataset_graph::migrate_model_vocabulary_reframe(
                store,
                &seed_state.base_url,
                auth,
            );
            if let Err(e) = crate::auth::dataset_audit::audit_dataset_metadata(store, auth, &base) {
                tracing::warn!("dataset metadata audit failed: {e}");
            }
            // 6. Built-in documentation pages (idempotent; preserves user edits).
            if let Err(e) = crate::docs::seed_builtin_docs(auth) {
                tracing::warn!("docs seed failed: {e}");
            }
        });
        crate::shacl_studio::scheduler::spawn_scheduler(
            state.store.clone(),
            state.auth_db.clone(),
            state.base_url.to_string(),
        );
    }

    // GDPR/AVG: pseudonymise old audit rows daily.
    {
        let days: u64 = std::env::var("AUDIT_PSEUDONYMISE_AFTER_DAYS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(365);
        crate::auth::audit::spawn_pseudonymisation_task(state.audit.clone(), days);
    }

    // Build router — auth endpoint rate limiting is applied inside build_router.
    let app = build_router(state, cors_origins, trusted_cidrs);

    // Serve frontend SPA from frontend/dist. Gated by --serve-frontend /
    // SERVE_FRONTEND (default on); disable for an API-only server. SPARQL, Graph
    // Store and REST endpoints are unaffected.
    //
    // Use `.fallback` (NOT `.not_found_service`) for the index.html catch-all:
    // svelte-routing uses history mode, so a deep link or hard refresh to a client
    // route such as /browse must return index.html with a 200. `.not_found_service`
    // serves the body but forces a 404 status, which breaks deep links and caching.
    let frontend_dir = std::path::Path::new("frontend/dist");
    let mut app = Router::new().merge(app);
    if serve_frontend && frontend_dir.exists() {
        app = app.fallback_service(
            ServeDir::new("frontend/dist").fallback(ServeFile::new("frontend/dist/index.html")),
        );
        info!("Web UI served at http://{}/", addr);
    } else if !serve_frontend {
        info!("Web UI disabled (SERVE_FRONTEND=false); serving API only");
    }
    // Outermost, path-aware framing policy: /embed/* is iframable (that is its
    // purpose), everything else that carries no CSP (the static SPA) gets the
    // anti-clickjacking headers. See `frame_policy_headers`.
    let app = app.layer(middleware::from_fn(frame_policy_headers));

    // Use into_make_service_with_connect_info so TCP peer IP is available to rate limiter.
    let requested_port = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
    let bind_host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        // Opt-in fallback (--port-fallback / PORT_FALLBACK, default off): bind any
        // free port instead of refusing to start. Checked ONLY on AddrInUse, so a
        // permission error or an invalid bind address still surfaces immediately
        // below rather than silently wandering off to an unrelated port.
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && port_fallback => {
            tracing::warn!(
                "PORT_FALLBACK: {addr} is already in use — falling back to any free port on {bind_host}"
            );
            crate::netutil::bind_free_port(bind_host)
                .await
                .map_err(|e2| {
                    anyhow::anyhow!(
                        "port fallback failed: could not bind any free port on {bind_host}: {e2}"
                    )
                })?
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // Fail LOUDLY instead of killing whatever holds the port. The previous behaviour
            // (lsof + kill -9, then re-bind) was a data-corruption footgun: if the occupant was
            // another open-triplestore on the same data dir, killing it and re-opening RocksDB
            // races the store into "SST ahead of WALs" (after which it refuses to reopen). Refuse
            // to start and tell the operator how to resolve it.
            let port_str = addr.rsplit(':').next().unwrap_or(addr);
            anyhow::bail!(
                "Refusing to start: {addr} is already in use (port {port_str}). Another process — \
                 most likely another open-triplestore — holds it. NOT killing it: doing so and \
                 reopening the same data dir can corrupt RocksDB (\"SST ahead of WALs\"). Stop the \
                 other instance first (find it with `lsof -ti :{port_str}`), start this one on a \
                 different --port and/or --data-dir, or pass --port-fallback / set PORT_FALLBACK=1 \
                 to bind any free port automatically."
            );
        }
        Err(e) => return Err(e.into()),
    };
    let bound_addr = listener.local_addr()?;
    // Only true when --port-fallback actually moved the bind: the common case
    // (flag off, or the requested port was free) leaves this false and every
    // downstream URL exactly as configured.
    let port_changed = requested_port.is_some_and(|p| p != bound_addr.port());
    if port_changed {
        tracing::warn!(
            "PORT_FALLBACK: bound port {} instead of the requested {} — the advertised base URL \
             is rewritten to match for service-registry self-registration, where it references \
             the requested port",
            bound_addr.port(),
            requested_port.unwrap_or(0)
        );
    }
    info!("Listening on {}", bound_addr);

    // Cross-app service discovery is opt-in (LD_DISCOVERY). Self-registration is done HERE
    // (after the real bind, not in main.rs before it) so the advertised self_url reflects any
    // --port-fallback rewrite rather than the originally requested port.
    if discovery {
        let self_url = if port_changed {
            crate::netutil::rewrite_url_port(
                base_url,
                requested_port.unwrap_or(0),
                bound_addr.port(),
            )
        } else {
            base_url.to_string()
        };
        crate::svc_registry::spawn_registrar(self_url, registry_url, registry_token);
    }

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    // The server has stopped accepting connections and drained in-flight requests. Flush RocksDB to
    // disk so the store closes in a consistent state: this triplestore has no other shutdown path,
    // and an abrupt SIGINT/SIGTERM mid-write can leave RocksDB as "SST ahead of WALs" — which then
    // refuses to reopen. flush() forces all column-family memtables out to synced SST files.
    info!("Shutdown signal received — flushing store to disk…");
    match shutdown_store.store().flush() {
        Ok(()) => info!("Store flushed cleanly; exiting."),
        Err(e) => tracing::error!("Store flush on shutdown failed: {e}"),
    }

    Ok(())
}

/// Resolves when the process receives SIGINT (Ctrl-C) or SIGTERM, so the server can shut down
/// gracefully (drain in-flight requests) and the store can be flushed before exit. Without an
/// explicit handler the default signal disposition kills the process immediately, skipping the
/// flush — the exact failure mode that corrupted the RocksDB store as "SST ahead of WALs".
async fn shutdown_signal() {
    use tokio::signal;
    let ctrl_c = async {
        if let Err(e) = signal::ctrl_c().await {
            tracing::error!("failed to install Ctrl-C handler: {e}");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::error!("failed to install SIGTERM handler: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod panic_safety_net_tests {
    //! Regression test for the panic safety net wired into [`build_router`]: a
    //! panic inside a handler must surface as a clean `500` produced by
    //! [`handle_request_panic`], never as an unwound (reset) connection.
    //!
    //! NB: the default panic hook still prints a "thread '…' panicked" line to
    //! stderr when the panic is caught — that output is expected here and is not
    //! a test failure.
    use super::handle_request_panic;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _; // for `oneshot`

    async fn always_panics() -> axum::response::Response {
        panic!("simulated handler panic");
    }

    #[tokio::test]
    async fn handler_panic_becomes_clean_500() {
        let app = Router::new().route("/boom", get(always_panics)).layer(
            tower_http::catch_panic::CatchPanicLayer::custom(handle_request_panic),
        );

        let resp = app
            .oneshot(Request::builder().uri("/boom").body(Body::empty()).unwrap())
            .await
            // The panic must be caught: the service resolves to a response rather
            // than propagating the unwind to the caller (the connection task).
            .expect("service resolved to a response");

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/plain; charset=utf-8"),
        );

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        // Generic message only — the panic payload is never leaked to the client.
        assert_eq!(body.as_ref(), b"Internal server error");
    }
}
