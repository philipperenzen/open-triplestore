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
        })
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
            write_access: true, // JWT sessions always have write access
        })
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
    })
}

/// Resolve a bearer token to an authenticated user, honoring the legacy-token
/// flag and falling through to OIDC verification for IdP-issued JWTs.
#[allow(clippy::result_large_err)]
async fn authenticate(
    jwt_config: &JwtConfig,
    auth_db: &Arc<AuthDb>,
    auth_ext: &AuthExt,
    token: &str,
) -> Result<AuthenticatedUser, Response> {
    let is_legacy_api_token = token.starts_with("ots_");

    if auth_ext.accept_legacy_tokens {
        match resolve_token(jwt_config, auth_db, token) {
            Ok(user) => return Ok(user),
            // `ots_` tokens are never OIDC; if no OIDC verifier, surface the
            // original error. Otherwise fall through and try OIDC.
            Err(resp) if is_legacy_api_token || auth_ext.oidc.is_none() => return Err(resp),
            Err(_) => {}
        }
    } else if is_legacy_api_token {
        return Err((StatusCode::UNAUTHORIZED, "Legacy tokens are disabled").into_response());
    }

    if auth_ext.oidc.is_some() && !is_legacy_api_token {
        return resolve_oidc_token(auth_ext, auth_db, token).await;
    }
    Err((StatusCode::UNAUTHORIZED, "Invalid or expired token").into_response())
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
pub async fn require_auth(
    State(jwt_config): State<Arc<JwtConfig>>,
    State(auth_db): State<Arc<AuthDb>>,
    State(auth_ext): State<Arc<AuthExt>>,
    State(audit): State<Arc<AuditLogger>>,
    mut req: Request,
    next: Next,
) -> Result<Response, Response> {
    let token = extract_token(&req)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing authorization token").into_response())?;

    let user = authenticate(&jwt_config, &auth_db, &auth_ext, &token).await?;
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
pub async fn optional_auth(
    State(jwt_config): State<Arc<JwtConfig>>,
    State(auth_db): State<Arc<AuthDb>>,
    State(auth_ext): State<Arc<AuthExt>>,
    State(audit): State<Arc<AuditLogger>>,
    mut req: Request,
    next: Next,
) -> Response {
    if let Some(token) = extract_token(&req) {
        if let Ok(user) = authenticate(&jwt_config, &auth_db, &auth_ext, &token).await {
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
    if mutating && !user.write_access && !user.is_admin() {
        return Err((
            StatusCode::FORBIDDEN,
            "This API token does not have write scope",
        )
            .into_response());
    }
    Ok(())
}

/// Middleware that requires publisher privileges (publisher, admin, or super-admin).
/// Must be used after `require_auth`.
pub async fn require_publisher(req: Request, next: Next) -> Result<Response, Response> {
    let user = req
        .extensions()
        .get::<AuthenticatedUser>()
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Authentication required").into_response())?;

    if !user.is_publisher() {
        return Err((StatusCode::FORBIDDEN, "Publisher access required").into_response());
    }

    Ok(next.run(req).await)
}

/// Middleware that checks endpoint-level ACL rules from the `endpoint_acl` table.
///
/// Must be placed **after** `optional_auth` or `require_auth` so that the
/// `AuthenticatedUser` extension is populated.  If the DB contains no rules
/// that match the current request, access is allowed (fail-open, with role
/// middleware still applying separately).
pub async fn endpoint_acl_guard(
    State(auth_db): State<Arc<AuthDb>>,
    State(audit_log): State<Arc<crate::auth::audit::AuditLogger>>,
    req: Request,
    next: Next,
) -> Result<Response, Response> {
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
