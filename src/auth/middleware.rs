use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use super::acl::check_endpoint_acl;
use super::audit::{AuditEventBuilder, AuditEventType, AuditLogger, AuditOutcome};
use super::db::AuthDb;
use super::jwt::{hash_token, verify_token, JwtConfig};
use super::models::{AccessLevel, SystemRole};
use super::oidc_rs::AuthExt;

/// Authenticated user extracted from JWT token or API token.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user_id: String,
    pub role: SystemRole,
    /// True if the user has been explicitly granted publish permission, or is admin/super-admin.
    pub can_publish: bool,
    /// True if this principal has write scope. Always true for JWT sessions; for API tokens
    /// this is `true` only when the token was issued with `write` or `admin` scope (M-8).
    pub write_access: bool,
    /// True if this principal may exchange itself for a long-lived API token at
    /// `POST /api/auth/tokens`. False for OIDC access tokens under the default
    /// policy: a credential delegated to a client (possibly for read-only
    /// scopes) must not be upgradable into permanent account access. See
    /// [`crate::auth::policy::OidcSessionPolicy`].
    pub can_mint_api_tokens: bool,
}

impl AuthenticatedUser {
    /// Returns true if the user has admin-level or above privileges.
    pub fn is_admin(&self) -> bool {
        self.role.is_admin()
    }

    /// Returns true if the user can create/edit/upload/publish ontology versions.
    pub fn is_publisher(&self) -> bool {
        self.role.is_admin() || self.can_publish
    }

    /// Whether this principal may create datasets of their own.
    ///
    /// Everyone signed in may, except a `Guest` in a deployment that has not
    /// granted `create_datasets` — the default.
    pub fn can_create_datasets(&self) -> bool {
        self.role != SystemRole::Guest || crate::auth::policy::guest_capabilities().create_datasets
    }

    /// Clamp a freshly-resolved principal to the capabilities its role is
    /// configured for.
    ///
    /// Applied on every authentication path, so a guest is equally limited
    /// whether they arrive by session cookie, API token or OIDC token. Only
    /// ever removes authority — a role can never gain power here — and roles
    /// other than `Guest` are untouched.
    fn clamped_to_role_policy(mut self) -> Self {
        if self.role == SystemRole::Guest {
            let caps = crate::auth::policy::guest_capabilities();
            self.write_access &= caps.write;
            self.can_mint_api_tokens &= caps.api_tokens;
            self.can_publish &= caps.publish;
        }
        self
    }
}

/// Extract a bearer token from the request:
/// 1. `Authorization: Bearer <token>` header (API tokens and backward-compat clients)
/// 2. `access_token` cookie (browser sessions using HttpOnly cookies, M-2)
fn extract_token(req: &Request) -> Option<String> {
    // 1. Authorization header
    if let Some(v) = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        return Some(v.to_string());
    }
    // 2. HttpOnly cookie fallback
    if let Some(cookie_hdr) = req.headers().get("cookie").and_then(|v| v.to_str().ok()) {
        for part in cookie_hdr.split(';') {
            let part = part.trim();
            if let Some(val) = part.strip_prefix("access_token=") {
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

/// The 401 for a deactivated account. Guests disabled by the admin's
/// guest-self-registration toggle get the specific message client apps
/// surface verbatim; everyone else keeps the generic line. The token already
/// proved the identity, so the specific message is not an enumeration oracle.
fn deactivated_response(auth_db: &AuthDb, user_id: &str) -> Response {
    use super::handlers::{GUEST_DISABLED_MESSAGE, GUEST_DISABLED_REASON};
    if matches!(auth_db.deactivation_reason(user_id), Ok(Some(ref r)) if r == GUEST_DISABLED_REASON)
    {
        return (StatusCode::UNAUTHORIZED, GUEST_DISABLED_MESSAGE).into_response();
    }
    (StatusCode::UNAUTHORIZED, "User account is deactivated").into_response()
}

/// Resolve a bearer token to an AuthenticatedUser.
/// Supports both JWT tokens and API tokens (prefixed with `ots_`).
#[allow(clippy::result_large_err)]
fn resolve_token(
    jwt_config: &JwtConfig,
    auth_db: &AuthDb,
    token: &str,
) -> Result<AuthenticatedUser, Response> {
    if token.starts_with("ots_") {
        // API token path
        let token_hash = hash_token(token);
        let api_token = auth_db
            .get_api_token_by_hash(&token_hash)
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response())?
            .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Invalid API token").into_response())?;

        if api_token.revoked {
            return Err((StatusCode::UNAUTHORIZED, "API token has been revoked").into_response());
        }

        // Check expiry
        if let Some(ref expires_at) = api_token.expires_at {
            let now = chrono::Utc::now().to_rfc3339();
            if now > *expires_at {
                return Err((StatusCode::UNAUTHORIZED, "API token has expired").into_response());
            }
        }

        // Load the user
        let user = auth_db
            .get_user_by_id(&api_token.user_id)
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response())?
            .ok_or_else(|| (StatusCode::UNAUTHORIZED, "User not found").into_response())?;

        if !user.is_active {
            return Err(deactivated_response(auth_db, &user.id));
        }

        // Update last_used_at (best effort, don't fail on this)
        let _ = auth_db.update_api_token_last_used(&api_token.id);

        // M-8: honour API token scopes — only tokens whose scope grants write
        // capability (write or admin) may do updates.
        let write_access = api_token
            .scopes
            .iter()
            .any(|s| AccessLevel::from(*s).can_write());

        Ok(AuthenticatedUser {
            user_id: user.id,
            role: user.role,
            can_publish: user.can_publish,
            write_access,
            // An API token is already a long-lived credential; it does not get to
            // mint more of them.
            can_mint_api_tokens: false,
        }
        .clamped_to_role_policy())
    } else {
        // JWT token path
        let claims = verify_token(jwt_config, token)
            .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid or expired token").into_response())?;

        if claims.token_type != "access" {
            return Err((StatusCode::UNAUTHORIZED, "Expected access token").into_response());
        }

        // Check user is still active
        let user = auth_db
            .get_user_by_id(&claims.sub)
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response())?
            .ok_or_else(|| (StatusCode::UNAUTHORIZED, "User not found").into_response())?;

        if !user.is_active {
            return Err(deactivated_response(auth_db, &claims.sub));
        }

        Ok(AuthenticatedUser {
            user_id: claims.sub,
            role: user.role, // Use DB role, not token role (in case it changed)
            can_publish: user.can_publish,
            write_access: true,        // JWT sessions always have write access
            can_mint_api_tokens: true, // a first-party interactive session
        }
        .clamped_to_role_policy())
    }
}

/// Verify an OIDC access token (issued directly by the IdP) and JIT-provision
/// the matching local user. Only reached when the legacy paths reject the token.
#[allow(clippy::result_large_err)]
async fn resolve_oidc_token(
    auth_ext: &AuthExt,
    auth_db: &Arc<AuthDb>,
    token: &str,
) -> Result<AuthenticatedUser, Response> {
    let verifier = auth_ext
        .oidc
        .as_ref()
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Invalid or expired token").into_response())?;
    let claims = verifier
        .verify(token)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid or expired token").into_response())?;

    let provider =
        super::oidc_rs::ensure_env_provider(auth_db, verifier.issuer(), &auth_ext.default_role)
            .map_err(|_| {
                (StatusCode::INTERNAL_SERVER_ERROR, "Auth provisioning error").into_response()
            })?;
    let user = super::oidc_rs::provision_from_claims(auth_db, &provider, auth_ext, &claims)
        .map_err(|_| (StatusCode::UNAUTHORIZED, "User provisioning failed").into_response())?;

    if !user.is_active {
        return Err((StatusCode::UNAUTHORIZED, "User account is deactivated").into_response());
    }

    Ok(AuthenticatedUser {
        user_id: user.id,
        role: user.role,
        can_publish: user.can_publish,
        write_access: true, // interactive (OIDC) sessions always have write access
        can_mint_api_tokens: true,
    }
    .clamped_to_role_policy())
}

/// A peer's identity assertion: verified against the peer's JWKS with this
/// instance as audience, provisioned as a read-only federated user whose
/// organisation memberships follow the assertion's `org:` groups.
async fn resolve_federated_token(
    auth_ext: &AuthExt,
    auth_db: &Arc<AuthDb>,
    verifier: &super::oidc_rs::OidcVerifier,
    token: &str,
) -> Result<AuthenticatedUser, Response> {
    let mut claims = verifier.verify(token).await.map_err(|e| {
        tracing::debug!(
            "federation: assertion from {} rejected: {e}",
            verifier.issuer()
        );
        (StatusCode::UNAUTHORIZED, "Invalid or expired token").into_response()
    })?;
    // Assertions carry no e-mail; provisioning wants one. A synthetic address
    // under the reserved `.invalid` TLD, keyed on issuer + subject, is stable
    // and can never match a real account (no linking by e-mail).
    if claims
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .is_none()
    {
        let host = crate::federation::origin_of(verifier.issuer())
            .and_then(|o| o.split("://").nth(1).map(|h| h.replace(':', "-")))
            .unwrap_or_else(|| "peer".to_string());
        claims.email = Some(format!("{}@{host}.federated.invalid", claims.sub));
    }
    let provider =
        super::oidc_rs::ensure_env_provider(auth_db, verifier.issuer(), &auth_ext.default_role)
            .map_err(|_| {
                (StatusCode::INTERNAL_SERVER_ERROR, "Auth provisioning error").into_response()
            })?;
    let user = super::oidc_rs::provision_from_claims(auth_db, &provider, auth_ext, &claims)
        .map_err(|_| (StatusCode::UNAUTHORIZED, "User provisioning failed").into_response())?;
    if !user.is_active {
        return Err((StatusCode::UNAUTHORIZED, "User account is deactivated").into_response());
    }
    Ok(AuthenticatedUser {
        user_id: user.id,
        role: user.role,
        can_publish: false,
        // Federated principals read; writes need a local credential.
        write_access: false,
        can_mint_api_tokens: false,
    })
}

/// Resolve a bearer token to an authenticated user, honoring the legacy-token
/// flag and falling through to OIDC verification for IdP-issued JWTs.
#[allow(clippy::result_large_err)]
async fn authenticate(
    jwt_config: &JwtConfig,
    auth_db: &Arc<AuthDb>,
    auth_ext: &AuthExt,
    provider: &crate::server::OidcProviderState,
    issuer: &str,
    token: &str,
) -> Result<AuthenticatedUser, Response> {
    let is_legacy_api_token = token.starts_with("ots_");

    let has_fallback =
        auth_ext.oidc.is_some() || provider.0.is_some() || !auth_ext.trusted_issuers.is_empty();
    // The legacy path's error is the most specific one we have (e.g. the
    // guest-disabled message for a deactivated account's still-valid session
    // token) — keep it and only surface the generic error when NO path could
    // say anything better.
    let mut legacy_err: Option<Response> = None;
    if auth_ext.accept_legacy_tokens {
        match resolve_token(jwt_config, auth_db, token) {
            Ok(user) => return Ok(user),
            // `ots_` tokens are never OIDC; with no other verifier, surface the
            // original error. Otherwise remember it and try the OIDC paths.
            Err(resp) if is_legacy_api_token || !has_fallback => return Err(resp),
            Err(resp) => legacy_err = Some(resp),
        }
    } else if is_legacy_api_token {
        return Err((StatusCode::UNAUTHORIZED, "Legacy tokens are disabled").into_response());
    }

    // Our own OIDC-provider access tokens (ES256, issued at /oauth/token).
    if let Some(keys) = provider.0.as_deref() {
        if let Some((sub, scope)) = crate::auth::oidc_provider::provider_token_identity(
            keys,
            issuer.trim_end_matches('/'),
            token,
        ) {
            let user = auth_db
                .get_user_by_id(&sub)
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response())?
                .ok_or_else(|| (StatusCode::UNAUTHORIZED, "User not found").into_response())?;
            if !user.is_active {
                return Err(deactivated_response(auth_db, &user.id));
            }
            // How far a delegated access token reaches is deployment policy: by
            // default it behaves like an interactive session (may write) but may
            // not mint a long-lived API token, which would turn a delegation the
            // user granted to a client into permanent account access.
            let policy = crate::auth::policy::oidc_session_policy();
            return Ok(AuthenticatedUser {
                user_id: user.id,
                role: user.role,
                can_publish: user.can_publish,
                write_access: policy.allows_write(&scope),
                can_mint_api_tokens: policy.allows_api_token_minting(),
            }
            .clamped_to_role_policy());
        }
    }

    // Federated identity assertions from trusted peer instances (crate::federation).
    if !auth_ext.trusted_issuers.is_empty() && !is_legacy_api_token {
        if let Some(iss) = crate::federation::unverified_issuer(token) {
            if let Some(verifier) = auth_ext.trusted_issuers.iter().find(|v| v.issuer() == iss) {
                return resolve_federated_token(auth_ext, auth_db, verifier, token).await;
            }
        }
    }
    if auth_ext.oidc.is_some() && !is_legacy_api_token {
        return resolve_oidc_token(auth_ext, auth_db, token).await;
    }
    Err(legacy_err
        .unwrap_or_else(|| (StatusCode::UNAUTHORIZED, "Invalid or expired token").into_response()))
}

/// Marker inserted into a `403` response's extensions by a guard that has
/// already emitted its own `permission_denied` audit event (e.g.
/// [`endpoint_acl_guard`]). The `require_auth`/`optional_auth` denial-audit pass
/// skips any response carrying it, so a single denial is never logged twice.
#[derive(Clone, Copy)]
struct DenialAudited;

/// Identity + endpoint context captured *before* the request is consumed by the
/// inner service, so a `403` produced downstream can be attributed in the audit
/// log (who, from where, which endpoint).
struct DenialContext {
    method: String,
    path: String,
    actor_id: Option<String>,
    actor_role: Option<String>,
    ip: Option<String>,
    request_id: Option<String>,
}

impl DenialContext {
    fn capture(req: &Request) -> Self {
        let user = req.extensions().get::<AuthenticatedUser>();
        Self {
            method: req.method().as_str().to_string(),
            path: req.uri().path().to_string(),
            actor_id: user.map(|u| u.user_id.clone()),
            actor_role: user.map(|u| u.role.as_str().to_string()),
            ip: super::audit::client_ip(req.headers(), None),
            request_id: req
                .extensions()
                .get::<crate::server::RequestId>()
                .map(|r| r.0.clone()),
        }
    }
}

/// Emit a `permission_denied` audit event when the downstream service answered
/// with `403 Forbidden`. This is the broad net that captures the per-dataset /
/// per-graph authorization denials individual handlers return inline
/// (`can_*_dataset(..) -> 403`), which would otherwise leave cross-tenant probe
/// attempts with no audit trail. Anonymous (unauthenticated) denials are logged
/// too — they are exactly the probe attempts worth recording.
fn audit_forbidden(audit: &AuditLogger, ctx: &DenialContext, resp: &Response) {
    if resp.status() != StatusCode::FORBIDDEN {
        return;
    }
    // A guard that already logged its own denial marks the response; skip it
    // here so the event isn't recorded twice.
    if resp.extensions().get::<DenialAudited>().is_some() {
        return;
    }
    let mut b = AuditEventBuilder::new(AuditEventType::PermissionDenied, AuditOutcome::Denied)
        .resource("endpoint", &ctx.path)
        .action(&ctx.method);
    b.actor_id = ctx.actor_id.clone();
    b.actor_role = ctx.actor_role.clone();
    b.ip_address = ctx.ip.clone();
    b.request_id = ctx.request_id.clone();
    audit.log(b);
}

/// Middleware that requires a valid JWT or API token. Returns 401 if missing or invalid.
#[allow(clippy::too_many_arguments)]
// axum substate extractors, one per capability
// Axum middleware: the error type must itself be a `Response`, so the
// `Err` variant is inherently response-sized. Boxing it would only move
// the allocation without changing the signature the framework requires.
#[allow(clippy::result_large_err)]
pub async fn require_auth(
    State(jwt_config): State<Arc<JwtConfig>>,
    State(auth_db): State<Arc<AuthDb>>,
    State(auth_ext): State<Arc<AuthExt>>,
    State(provider): State<crate::server::OidcProviderState>,
    State(base_url): State<crate::server::BaseUrl>,
    State(audit): State<Arc<AuditLogger>>,
    mut req: Request,
    next: Next,
) -> Result<Response, Response> {
    let token = extract_token(&req)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing authorization token").into_response())?;

    let user = authenticate(
        &jwt_config,
        &auth_db,
        &auth_ext,
        &provider,
        &base_url.0,
        &token,
    )
    .await?;
    // M-8 (generalized): a read-scoped API token may not perform mutating requests.
    enforce_write_scope_for_mutation(&req, &user)?;
    req.extensions_mut().insert(user);

    // Capture identity/endpoint context, then audit if the handler (or an inner
    // guard) denies with 403 (see `audit_forbidden`).
    let ctx = DenialContext::capture(&req);
    let resp = next.run(req).await;
    audit_forbidden(&audit, &ctx, &resp);
    Ok(resp)
}

/// Middleware that optionally extracts auth. If present and valid, sets the
/// authenticated user. If missing or invalid, continues without authentication.
#[allow(clippy::too_many_arguments)] // axum substate extractors, one per capability
pub async fn optional_auth(
    State(jwt_config): State<Arc<JwtConfig>>,
    State(auth_db): State<Arc<AuthDb>>,
    State(auth_ext): State<Arc<AuthExt>>,
    State(provider): State<crate::server::OidcProviderState>,
    State(base_url): State<crate::server::BaseUrl>,
    State(audit): State<Arc<AuditLogger>>,
    mut req: Request,
    next: Next,
) -> Response {
    if let Some(token) = extract_token(&req) {
        if let Ok(user) = authenticate(
            &jwt_config,
            &auth_db,
            &auth_ext,
            &provider,
            &base_url.0,
            &token,
        )
        .await
        {
            // M-8 (generalized): a read-scoped API token may not mutate, even on
            // optional-auth routes whose handlers self-gate on resource role.
            if let Err(resp) = enforce_write_scope_for_mutation(&req, &user) {
                return resp;
            }
            req.extensions_mut().insert(user);
        }
    }

    // Audit any downstream 403 — including anonymous cross-tenant read probes on
    // visibility-scoped routes that this middleware lets through unauthenticated.
    let ctx = DenialContext::capture(&req);
    let resp = next.run(req).await;
    audit_forbidden(&audit, &ctx, &resp);
    resp
}

/// Middleware that requires admin privileges. Must be used after `require_auth`.
// Axum middleware: the error type must itself be a `Response`, so the
// `Err` variant is inherently response-sized. Boxing it would only move
// the allocation without changing the signature the framework requires.
#[allow(clippy::result_large_err)]
pub async fn require_admin(req: Request, next: Next) -> Result<Response, Response> {
    let user = req
        .extensions()
        .get::<AuthenticatedUser>()
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Authentication required").into_response())?;

    if !user.is_admin() {
        return Err((StatusCode::FORBIDDEN, "Admin access required").into_response());
    }

    Ok(next.run(req).await)
}

/// Enforce write scope on **mutating** requests (POST/PUT/PATCH/DELETE) for a
/// resolved principal. Read methods pass untouched so a read-scoped token can
/// still read. Called from `require_auth`/`optional_auth` so the M-8 token-scope
/// check (previously only on SPARQL UPDATE + Graph-Store writes) applies uniformly
/// to every authenticated mutating endpoint.
#[allow(clippy::result_large_err)] // Err is an axum Response, returned on the cold deny path
fn enforce_write_scope_for_mutation(
    req: &Request,
    user: &AuthenticatedUser,
) -> Result<(), Response> {
    let mutating = matches!(req.method().as_str(), "POST" | "PUT" | "PATCH" | "DELETE");
    // A SPARQL *query* sent by POST (SPARQL 1.1 Protocol §2.1.2/2.1.3) is a read;
    // the update path enforces write scope itself (execute_update). Treating
    // every POST as a mutation locked read-only principals — read-scoped API
    // tokens and federated identities — out of the protocol's POST query form.
    let sparql_post = req.method() == axum::http::Method::POST
        && req.uri().path().trim_end_matches('/') == "/sparql"
        && !req
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .starts_with("application/sparql-update");
    if mutating && !sparql_post && !user.write_access && !user.is_admin() {
        return Err((
            StatusCode::FORBIDDEN,
            "This API token does not have write scope",
        )
            .into_response());
    }
    Ok(())
}

/// Whether the endpoint ACL is enforced. `ENDPOINT_ACL_ENFORCE=false` (or `0`)
/// turns it off. Read once — this sits on every request.
fn endpoint_acl_enforced() -> bool {
    static ENFORCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENFORCED.get_or_init(|| {
        !matches!(
            std::env::var("ENDPOINT_ACL_ENFORCE")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "false" | "0" | "no" | "off"
        )
    })
}

/// Middleware that checks endpoint-level ACL rules from the `endpoint_acl` table.
///
/// Must be placed **after** `optional_auth` or `require_auth` so that the
/// `AuthenticatedUser` extension is populated.  If the DB contains no rules
/// that match the current request, access is allowed (fail-open, with role
/// middleware still applying separately).
// Axum middleware: the error type must itself be a `Response`, so the
// `Err` variant is inherently response-sized. Boxing it would only move
// the allocation without changing the signature the framework requires.
#[allow(clippy::result_large_err)]
pub async fn endpoint_acl_guard(
    State(auth_db): State<Arc<AuthDb>>,
    State(audit_log): State<Arc<crate::auth::audit::AuditLogger>>,
    req: Request,
    next: Next,
) -> Result<Response, Response> {
    // Escape hatch for an operator whose rule misfires. Enforcement is ON by
    // default (secure by default); this exists because the guard now covers
    // every authenticated route rather than the six `/api/browse/*` ones it was
    // mounted on before, so a bad rule has a much larger blast radius.
    if !endpoint_acl_enforced() {
        return Ok(next.run(req).await);
    }

    let user = req.extensions().get::<AuthenticatedUser>().cloned();
    let method = req.method().as_str().to_uppercase();
    let path = req.uri().path().to_string();
    let request_id = req
        .extensions()
        .get::<crate::server::RequestId>()
        .map(|r| r.0.clone());

    if !check_endpoint_acl(user.as_ref(), &method, &path, &auth_db) {
        let mut b = AuditEventBuilder::new(AuditEventType::PermissionDenied, AuditOutcome::Denied)
            .resource("endpoint", &path)
            .action(&method);
        if let Some(u) = &user {
            b.actor_id = Some(u.user_id.clone());
            b.actor_role = Some(u.role.as_str().to_string());
        }
        b.request_id = request_id;
        audit_log.log(b);
        // Mark the response so the outer auth-middleware denial-audit pass does
        // not record this same 403 a second time.
        let mut resp = (
            StatusCode::FORBIDDEN,
            "Access denied by endpoint ACL policy",
        )
            .into_response();
        resp.extensions_mut().insert(DenialAudited);
        return Err(resp);
    }

    Ok(next.run(req).await)
}

#[cfg(test)]
mod role_policy_tests {
    use super::*;

    fn principal(role: SystemRole) -> AuthenticatedUser {
        // Everything on, as an interactive session would arrive.
        AuthenticatedUser {
            user_id: "u1".to_string(),
            role,
            can_publish: true,
            write_access: true,
            can_mint_api_tokens: true,
        }
    }

    /// The env var is process-global, so the guest cases share one test to keep
    /// them ordered rather than racing other tests in the same binary.
    #[test]
    fn guest_capabilities_clamp_the_resolved_principal() {
        // Default: a guest reads and nothing else, even arriving on a session
        // that would otherwise carry full authority.
        std::env::remove_var("OTS_GUEST_CAPABILITIES");
        let g = principal(SystemRole::Guest).clamped_to_role_policy();
        assert!(!g.write_access, "guests must not write by default");
        assert!(!g.can_mint_api_tokens, "guests must not mint API tokens");
        assert!(!g.can_publish && !g.is_publisher());
        assert!(!g.can_create_datasets());

        // Opt in to exactly one capability: the others stay denied.
        std::env::set_var("OTS_GUEST_CAPABILITIES", "write");
        let g = principal(SystemRole::Guest).clamped_to_role_policy();
        assert!(g.write_access);
        assert!(!g.can_mint_api_tokens && !g.can_publish);
        assert!(!g.can_create_datasets());

        // The escape hatch restores user-equivalent power.
        std::env::set_var("OTS_GUEST_CAPABILITIES", "all");
        let g = principal(SystemRole::Guest).clamped_to_role_policy();
        assert!(g.write_access && g.can_mint_api_tokens && g.can_publish);
        assert!(g.can_create_datasets());

        // Other roles are never touched by the guest policy.
        std::env::set_var("OTS_GUEST_CAPABILITIES", "read");
        for role in [SystemRole::User, SystemRole::Admin, SystemRole::SuperAdmin] {
            let u = principal(role).clamped_to_role_policy();
            assert!(u.write_access, "{role:?} must keep write");
            assert!(u.can_mint_api_tokens, "{role:?} must keep token minting");
            assert!(
                u.can_create_datasets(),
                "{role:?} must keep dataset creation"
            );
        }
        std::env::remove_var("OTS_GUEST_CAPABILITIES");
    }

    /// The clamp only ever removes authority — it cannot hand a guest something
    /// the authentication path did not already grant.
    #[test]
    fn clamping_never_adds_authority() {
        std::env::set_var("OTS_GUEST_CAPABILITIES", "all");
        let mut p = principal(SystemRole::Guest);
        p.write_access = false;
        p.can_mint_api_tokens = false;
        let g = p.clamped_to_role_policy();
        assert!(!g.write_access && !g.can_mint_api_tokens);
        std::env::remove_var("OTS_GUEST_CAPABILITIES");
    }
}
