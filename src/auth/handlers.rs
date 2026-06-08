use axum::{
    extract::{Extension, Multipart, Path, Query, State},
    http::{
        header::{CONTENT_TYPE, SET_COOKIE},
        HeaderMap, StatusCode,
    },
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use super::audit::{self, AuditEventBuilder, AuditEventType, AuditLogger, AuditOutcome};
use super::authz;
use super::db::AuthDb;
use super::jwt::{self, hash_token, JwtConfig};
use super::middleware::AuthenticatedUser;
use super::models::*;
use super::password;
use super::{dataset_graph, user_graph};
use crate::server::{AppState, CookieConfig};

// ─── Request / Response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub user: UserResponse,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UserResponse {
    pub id: String,
    pub username: String,
    pub email: String,
    pub role: SystemRole,
    pub is_active: bool,
    pub is_public: bool,
    pub can_publish: bool,
    pub avatar_key: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    // FOAF / VCARD profile fields
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub website: Option<String>,
    pub phone: Option<String>,
    pub organization: Option<String>,
}

impl From<User> for UserResponse {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            username: u.username,
            email: u.email,
            role: u.role,
            is_active: u.is_active,
            is_public: u.is_public,
            can_publish: u.can_publish,
            avatar_key: u.avatar_key,
            created_at: u.created_at,
            updated_at: u.updated_at,
            display_name: u.display_name,
            bio: u.bio,
            website: u.website,
            phone: u.phone,
            organization: u.organization,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateProfileRequest {
    pub username: Option<String>,
    pub email: Option<String>,
    // FOAF / VCARD profile fields
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub website: Option<String>,
    pub phone: Option<String>,
    pub organization: Option<String>,
    pub is_public: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LogoutRequest {
    pub refresh_token: String,
}

// ─── API Token request/response types ────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateApiTokenRequest {
    pub name: String,
    pub scopes: Vec<String>,
    pub expires_in_days: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiTokenResponse {
    pub id: String,
    pub name: String,
    pub token_prefix: String,
    pub scopes: Vec<ApiScope>,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
    pub created_at: String,
    pub revoked: bool,
}

impl From<ApiToken> for ApiTokenResponse {
    fn from(t: ApiToken) -> Self {
        Self {
            id: t.id,
            name: t.name,
            token_prefix: t.token_prefix,
            scopes: t.scopes,
            expires_at: t.expires_at,
            last_used_at: t.last_used_at,
            created_at: t.created_at,
            revoked: t.revoked,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiTokenCreatedResponse {
    pub token: String,
    pub id: String,
    pub name: String,
    pub token_prefix: String,
    pub scopes: Vec<ApiScope>,
    pub expires_at: Option<String>,
}

// ─── Admin user management request types ─────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct AdminCreateUserRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    pub role: Option<String>,
    pub can_publish: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AdminUpdateUserRequest {
    pub email: Option<String>,
    pub role: Option<String>,
    pub is_active: Option<bool>,
    pub can_publish: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AdminResetPasswordRequest {
    pub new_password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PaginationParams {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub search: Option<String>,
}

// ─── Organisation request types ───────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateOrgRequest {
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    /// Optional parent organisation (`org:subOrganizationOf`).
    pub parent_org_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateOrgRequest {
    pub name: String,
    pub description: Option<String>,
    /// `foaf:homepage` — primary web page IRI/URL.
    pub homepage: Option<String>,
    /// `dct:identifier` — official identifier (KVK, LEI, company registration…).
    pub identifier: Option<String>,
    /// Contact name → `vcard:fn`.
    pub contact_name: Option<String>,
    /// Contact e-mail → `vcard:hasEmail`.
    pub contact_email: Option<String>,
    /// Contact URL → `vcard:hasURL`.
    pub contact_url: Option<String>,
    /// RDF type suffix: `"Organization"` | `"FormalOrganization"` | `"OrganizationalUnit"`.
    pub org_type: Option<String>,
    /// Parent organisation (`org:subOrganizationOf`). Empty string or null clears it.
    pub parent_org_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AddMemberRequest {
    pub user_id: String,
    pub role: String,
}

// ─── Group request types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateGroupRequest {
    pub name: String,
    pub parent_group_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateGroupRequest {
    pub name: String,
    pub parent_group_id: Option<String>,
}

// ─── Dataset response type ───────────────────────────────────────────────────

/// Dataset with caller's write permission flag, used in GET responses.
#[derive(Debug, serde::Serialize)]
pub struct DatasetView {
    #[serde(flatten)]
    pub dataset: crate::auth::models::Dataset,
    /// Canonical dataset IRI (`{base_url}/dataset/{id}`). Clients mint graph IRIs
    /// for this dataset under `{dataset_iri}/...`; the bulk-import write boundary
    /// only admits targets registered to the dataset or under this namespace.
    pub dataset_iri: String,
    pub can_write: bool,
    /// Whether the caller can manage this dataset's settings and access grants.
    pub can_manage: bool,
    /// The caller's effective role on this dataset (viewer | editor | admin),
    /// or null for an anonymous/no-access caller.
    pub effective_role: Option<crate::auth::models::ResourceRole>,
    /// Distinct roles across this dataset's registered graphs (e.g. a dataset
    /// holding a model + vocabulary + instances reports all three). Empty when
    /// no graph has a role tag yet.
    #[serde(default)]
    pub roles: Vec<crate::auth::models::GraphKind>,
}

// ─── Dataset request types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateDatasetRequest {
    pub name: String,
    pub description: Option<String>,
    pub owner_type: String,
    pub owner_id: String,
    pub visibility: Option<String>,
    pub conforms_to_ontology: Option<String>,
    pub conforms_to_version: Option<String>,
    /// Optional box classification: "abox" | "tbox" | "shapes" | "entailment" | "system"
    pub graph_role: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateDatasetRequest {
    pub name: String,
    pub description: Option<String>,
    pub visibility: String,
    pub conforms_to_ontology: Option<String>,
    pub conforms_to_version: Option<String>,
    // DCAT / ADMS / VoID metadata
    pub license: Option<String>,
    /// IRI array serialised by the client as a JSON array (e.g. `["https://…"]`)
    pub themes: Option<Vec<String>>,
    pub keywords: Option<Vec<String>>,
    pub contact_name: Option<String>,
    pub contact_email: Option<String>,
    pub contact_url: Option<String>,
    pub adms_status: Option<String>,
    pub version_notes: Option<String>,
    pub spatial: Option<String>,
    pub landing_page: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DatasetShaclRequest {
    pub shacl_on_write: bool,
    pub shapes_graph_iri: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GraphIriRequest {
    pub graph_iri: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PatchDatasetGraphRoleRequest {
    pub graph_iri: String,
    pub graph_role: Option<String>,
    /// When present, sets the graph's privacy flag. Omitted leaves it unchanged.
    pub private: Option<bool>,
}

// ─── Service request types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateServiceRequest {
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateServiceRequest {
    pub name: String,
    pub description: Option<String>,
    pub is_active: Option<bool>,
}

// ─── Token helpers ───────────────────────────────────────────────────────────

/// Issue an access + refresh token pair, storing the refresh token hash in DB.
/// Build `Set-Cookie` headers for access and refresh tokens (M-2: HttpOnly cookies).
fn auth_cookie_headers(
    access_token: &str,
    refresh_token: &str,
    access_expiry_secs: u64,
    refresh_expiry_secs: u64,
    secure: bool,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let secure_attr = if secure { "; Secure" } else { "" };
    let access_cookie = format!(
        "access_token={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{}",
        access_token, access_expiry_secs, secure_attr
    );
    let refresh_cookie = format!(
        "refresh_token={}; HttpOnly; SameSite=Strict; Path=/api/auth; Max-Age={}{}",
        refresh_token, refresh_expiry_secs, secure_attr
    );
    if let (Ok(a), Ok(r)) = (
        axum::http::HeaderValue::from_str(&access_cookie),
        axum::http::HeaderValue::from_str(&refresh_cookie),
    ) {
        headers.append(SET_COOKIE, a);
        headers.append(SET_COOKIE, r);
    }
    headers
}

/// Build `Set-Cookie` headers that clear access and refresh tokens on logout.
fn clear_auth_cookie_headers(secure: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let secure_attr = if secure { "; Secure" } else { "" };
    for (name, path) in &[("access_token", "/"), ("refresh_token", "/api/auth")] {
        let val = format!(
            "{}=; HttpOnly; SameSite=Strict; Path={}; Max-Age=0{}",
            name, path, secure_attr
        );
        if let Ok(v) = axum::http::HeaderValue::from_str(&val) {
            headers.append(SET_COOKIE, v);
        }
    }
    headers
}

fn issue_tokens(
    jwt_config: &JwtConfig,
    auth_db: &AuthDb,
    user: &User,
) -> Result<(String, String, u64), (StatusCode, String)> {
    let access_token =
        jwt::issue_access_token(jwt_config, &user.id, &user.username, user.role.as_str())
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let refresh_token =
        jwt::issue_refresh_token(jwt_config, &user.id, &user.username, user.role.as_str())
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Store refresh token hash in DB
    let refresh_hash = hash_token(&refresh_token);
    let refresh_id = uuid::Uuid::new_v4().to_string();
    let expires_at =
        chrono::Utc::now() + chrono::Duration::days(jwt_config.refresh_expiry_days as i64);
    auth_db
        .create_refresh_token(
            &refresh_id,
            &user.id,
            &refresh_hash,
            &expires_at.to_rfc3339(),
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let expires_in = jwt_config.access_expiry_minutes * 60;
    Ok((access_token, refresh_token, expires_in))
}

// ─── Linked-data graph helpers ────────────────────────────────────────────────

/// Best-effort: (re)write the DCAT metadata named graph for `dataset_id`.
/// Silently ignores any error so callers never abort on a metadata failure.
fn refresh_dataset_metadata(state: &AppState, dataset_id: &str) {
    if let Ok(Some(ds)) = state.auth_db.get_dataset(dataset_id) {
        let entries = state
            .auth_db
            .list_dataset_graph_entries(dataset_id)
            .unwrap_or_default();
        dataset_graph::write_dataset_metadata_graph(&state.store, &state.base_url, &ds, &entries);
    }
}

/// Best-effort: (re)write the organisation knowledge graph for `org_id`,
/// including its `org:hasSubOrganization` links to direct children.
fn refresh_org_metadata(state: &AppState, org_id: &str) {
    if let Ok(Some(org)) = state.auth_db.get_organisation(org_id) {
        let children = state
            .auth_db
            .list_child_organisations(org_id)
            .unwrap_or_default();
        crate::auth::org_graph::write_org_metadata_graph(
            &state.store,
            &state.base_url,
            &org,
            &children,
        );
    }
}

// ─── Auth handlers ────────────────────────────────────────────────────────────

/// POST /api/auth/register
pub async fn register(
    State(db): State<Arc<AuthDb>>,
    State(jwt_config): State<Arc<JwtConfig>>,
    State(state): State<AppState>,
    State(cookie_config): State<CookieConfig>,
    Json(req): Json<RegisterRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if req.username.len() < 3 || req.username.len() > 50 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Username must be 3-50 characters".to_string(),
        ));
    }
    if req.password.len() < 8 || req.password.len() > 1024 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Password must be between 8 and 1024 characters".to_string(),
        ));
    }

    // Operators can disable open self-registration with OTS_DISABLE_REGISTRATION=1
    // (provision users out-of-band instead). The very FIRST account is still allowed
    // so a fresh instance can be bootstrapped through the UI; thereafter registration
    // is closed. This prevents an attacker on an exposed fresh deployment from
    // racing to claim the first-user super_admin once the operator has locked it.
    let registration_disabled = std::env::var("OTS_DISABLE_REGISTRATION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if registration_disabled
        && db
            .count_users()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            > 0
    {
        return Err((
            StatusCode::FORBIDDEN,
            "Self-registration is disabled".to_string(),
        ));
    }

    // Check if username already exists
    if db
        .get_user_by_username(&req.username)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_some()
    {
        return Err((StatusCode::CONFLICT, "Username already taken".to_string()));
    }

    let hash = password::hash_password(&req.password)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // First user gets super_admin, all others get user
    let role = if db.count_users().unwrap_or(0) == 0 {
        SystemRole::SuperAdmin
    } else {
        SystemRole::User
    };

    let id = uuid::Uuid::new_v4().to_string();
    let user = db
        .create_user(&id, &req.username, &req.email, &hash, role)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Write the FOAF/VCARD profile named graph for the new user.
    user_graph::write_user_profile_graph(&state.store, &state.base_url, &user);

    // First-admin bootstrap: on a brand-new install the startup seed runs before
    // any admin exists and therefore skips. This is the moment the first admin
    // appears, so kick off the (idempotent) demo seed now instead of waiting for
    // the next restart.
    if matches!(role, SystemRole::SuperAdmin) {
        let seed_state = state.clone();
        tokio::task::spawn_blocking(move || {
            crate::saved_queries::seed::seed_open_triplestore(&seed_state)
        });
    }

    let (access_token, refresh_token, expires_in) = issue_tokens(&jwt_config, &db, &user)?;

    let cookie_headers = auth_cookie_headers(
        &access_token,
        &refresh_token,
        expires_in,
        jwt_config.refresh_expiry_days * 86400,
        cookie_config.secure,
    );

    Ok((
        StatusCode::CREATED,
        cookie_headers,
        Json(AuthResponse {
            access_token,
            refresh_token,
            expires_in,
            user: user.into(),
        }),
    ))
}

/// A fixed, valid Argon2id hash used to equalize login timing for unknown
/// usernames. Verifying a submitted password against this (always-failing) hash
/// costs the same as verifying against a real user's hash, so an attacker cannot
/// distinguish "no such user" from "wrong password" by response latency.
fn dummy_password_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY.get_or_init(|| {
        password::hash_password("argon2-timing-equalization-placeholder").unwrap_or_default()
    })
}

/// POST /api/auth/login
pub async fn login(
    State(db): State<Arc<AuthDb>>,
    State(jwt_config): State<Arc<JwtConfig>>,
    State(audit_log): State<Arc<AuditLogger>>,
    State(cookie_config): State<CookieConfig>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ip = audit::client_ip(&headers, None);
    let ua = audit::user_agent(&headers);
    let req_id = audit::request_id_from_headers(&headers);

    let log_failure = |username: &str, reason: &str, actor_id: Option<String>| {
        let mut b = AuditEventBuilder::new(AuditEventType::LoginFailure, AuditOutcome::Failure)
            .actor_username(username.to_string())
            .details(serde_json::json!({ "reason": reason }));
        b.actor_id = actor_id;
        b.ip_address = ip.clone();
        b.user_agent = ua.clone();
        b.request_id = req_id.clone();
        audit_log.log(b);
    };

    // Per-account lockout (independent of the per-IP rate limiter) — blocks
    // distributed credential-stuffing against one username. Keyed by the submitted
    // username, checked before any work; returns the same generic message so lock
    // state isn't revealed to a third party.
    if db.is_login_locked(&req.username).unwrap_or(false) {
        log_failure(&req.username, "account_locked", None);
        return Err((StatusCode::UNAUTHORIZED, "Invalid credentials".to_string()));
    }

    let user = match db
        .get_user_by_username(&req.username)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        Some(u) => u,
        None => {
            // Run a real Argon2 verify against a dummy hash so the unknown-user
            // path takes the same time as the wrong-password path (no timing
            // oracle for user enumeration), and return the same generic message.
            let _ = password::verify_password(&req.password, dummy_password_hash());
            let _ = db.record_login_failure(&req.username);
            log_failure(&req.username, "user_not_found", None);
            return Err((StatusCode::UNAUTHORIZED, "Invalid credentials".to_string()));
        }
    };

    // Verify the password before branching on account state, and return ONE
    // generic message for every failure (unknown user / bad password / disabled
    // account). The specific reason is recorded server-side in the audit log only,
    // so the response can't be used to enumerate accounts or their status.
    let valid = password::verify_password(&req.password, &user.password_hash)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !valid {
        let _ = db.record_login_failure(&req.username);
        log_failure(&user.username, "bad_password", Some(user.id.clone()));
        return Err((StatusCode::UNAUTHORIZED, "Invalid credentials".to_string()));
    }

    if !user.is_active {
        let _ = db.record_login_failure(&req.username);
        log_failure(&user.username, "account_deactivated", Some(user.id.clone()));
        return Err((StatusCode::UNAUTHORIZED, "Invalid credentials".to_string()));
    }

    // Successful authentication — reset the failure counter.
    let _ = db.clear_login_attempts(&req.username);

    let (access_token, refresh_token, expires_in) = issue_tokens(&jwt_config, &db, &user)?;

    {
        let mut b = AuditEventBuilder::new(AuditEventType::LoginSuccess, AuditOutcome::Success)
            .actor(&user.id, &user.username, user.role.as_str());
        b.ip_address = ip;
        b.user_agent = ua;
        b.request_id = req_id;
        audit_log.log(b);
    }

    let cookie_headers = auth_cookie_headers(
        &access_token,
        &refresh_token,
        expires_in,
        jwt_config.refresh_expiry_days * 86400,
        cookie_config.secure,
    );

    Ok((
        cookie_headers,
        Json(AuthResponse {
            access_token,
            refresh_token,
            expires_in,
            user: user.into(),
        }),
    ))
}

/// POST /api/auth/refresh
pub async fn refresh(
    State(db): State<Arc<AuthDb>>,
    State(jwt_config): State<Arc<JwtConfig>>,
    State(cookie_config): State<CookieConfig>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Accept refresh token from JSON body OR from HttpOnly cookie (M-2)
    let refresh_token_str: String = {
        // Try JSON body first
        let body_token = if !body.is_empty() {
            serde_json::from_slice::<RefreshRequest>(&body)
                .ok()
                .map(|r| r.refresh_token)
        } else {
            None
        };
        // Fall back to cookie
        if let Some(t) = body_token {
            t
        } else {
            headers
                .get("cookie")
                .and_then(|v| v.to_str().ok())
                .and_then(|c| {
                    c.split(';')
                        .find_map(|p| p.trim().strip_prefix("refresh_token=").map(str::to_string))
                })
                .ok_or_else(|| {
                    (
                        StatusCode::UNAUTHORIZED,
                        "Missing refresh token".to_string(),
                    )
                })?
        }
    };

    // Verify the refresh JWT
    let claims = jwt::verify_token(&jwt_config, &refresh_token_str).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            "Invalid or expired refresh token".to_string(),
        )
    })?;

    if claims.token_type != "refresh" {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Expected refresh token".to_string(),
        ));
    }

    // Check refresh token hash exists in DB and is not revoked
    let token_hash = hash_token(&refresh_token_str);
    let stored = db
        .get_refresh_token_by_hash(&token_hash)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "Refresh token not found".to_string(),
            )
        })?;

    if stored.revoked {
        // Refresh-token REUSE: a token that was already rotated (or explicitly
        // revoked) is being replayed. Per RFC 6819 / refresh-token-rotation
        // guidance this signals theft of the token chain, so we revoke EVERY
        // refresh token for the user, forcing full re-authentication of both the
        // legitimate user and the attacker.
        tracing::warn!(
            user_id = %claims.sub,
            "refresh-token reuse detected; revoking all refresh tokens for user"
        );
        let _ = db.revoke_all_user_refresh_tokens(&claims.sub);
        return Err((
            StatusCode::UNAUTHORIZED,
            "Refresh token has been revoked".to_string(),
        ));
    }

    // Revoke the old refresh token (rotation)
    db.revoke_refresh_token(&stored.id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Load user and check active
    let user = db
        .get_user_by_id(&claims.sub)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "User not found".to_string()))?;

    if !user.is_active {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Account is deactivated".to_string(),
        ));
    }

    // Issue new token pair
    let (access_token, refresh_token, expires_in) = issue_tokens(&jwt_config, &db, &user)?;

    let cookie_headers = auth_cookie_headers(
        &access_token,
        &refresh_token,
        expires_in,
        jwt_config.refresh_expiry_days * 86400,
        cookie_config.secure,
    );

    Ok((
        cookie_headers,
        Json(AuthResponse {
            access_token,
            refresh_token,
            expires_in,
            user: user.into(),
        }),
    ))
}

/// POST /api/auth/logout
pub async fn logout(
    State(db): State<Arc<AuthDb>>,
    State(jwt_config): State<Arc<JwtConfig>>,
    State(audit_log): State<Arc<AuditLogger>>,
    State(cookie_config): State<CookieConfig>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mut logged_actor: Option<String> = None;
    // Accept refresh token from JSON body OR from HttpOnly cookie (M-2)
    let refresh_token_str: Option<String> = {
        if !body.is_empty() {
            serde_json::from_slice::<LogoutRequest>(&body)
                .ok()
                .map(|r| r.refresh_token)
        } else {
            headers
                .get("cookie")
                .and_then(|v| v.to_str().ok())
                .and_then(|c| {
                    c.split(';')
                        .find_map(|p| p.trim().strip_prefix("refresh_token=").map(str::to_string))
                })
        }
    };

    // Best effort: revoke the refresh token if valid
    if let Some(ref tok) = refresh_token_str {
        if let Ok(claims) = jwt::verify_token(&jwt_config, tok) {
            if claims.token_type == "refresh" {
                logged_actor = Some(claims.sub.clone());
                let token_hash = hash_token(tok);
                if let Ok(Some(stored)) = db.get_refresh_token_by_hash(&token_hash) {
                    let _ = db.revoke_refresh_token(&stored.id);
                }
            }
        }
    }

    {
        let mut b = AuditEventBuilder::new(AuditEventType::Logout, AuditOutcome::Success);
        b.actor_id = logged_actor;
        b.ip_address = audit::client_ip(&headers, None);
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        audit_log.log(b);
    }

    Ok((
        StatusCode::NO_CONTENT,
        clear_auth_cookie_headers(cookie_config.secure),
    ))
}

/// GET /api/auth/me
pub async fn me(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let user = db
        .get_user_by_id(&current_user.user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    Ok(Json(UserResponse::from(user)))
}

/// PUT /api/auth/me
pub async fn update_me(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(state): State<AppState>,
    Json(req): Json<UpdateProfileRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let user = db
        .get_user_by_id(&current_user.user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    // Update username / email (fall back to current values when not provided).
    let username = req.username.as_deref().unwrap_or(&user.username);
    let email = req.email.as_deref().unwrap_or(&user.email);

    // Pre-check uniqueness so a collision returns a clean 409 rather than a 500 that
    // would leak the SQLite constraint text (and confirm the value is taken).
    if username != user.username {
        if let Some(other) = db
            .get_user_by_username(username)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            if other.id != current_user.user_id {
                return Err((StatusCode::CONFLICT, "Username already taken".to_string()));
            }
        }
    }
    if email != user.email {
        if let Some(other) = db
            .get_user_by_email(email)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            if other.id != current_user.user_id {
                return Err((StatusCode::CONFLICT, "Email already in use".to_string()));
            }
        }
    }

    db.update_user(&current_user.user_id, username, email)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Update FOAF profile fields when this looks like a full profile update
    // (i.e. username or a FOAF field was explicitly included in the request).
    // The privacy-toggle path only sends `is_public` and no name/FOAF keys.
    let is_profile_update = req.username.is_some()
        || req.email.is_some()
        || req.display_name.is_some()
        || req.bio.is_some()
        || req.website.is_some()
        || req.phone.is_some()
        || req.organization.is_some();

    if is_profile_update {
        // `None` in the request means "clear the field" (the frontend sends
        // explicit nulls when the user empties an input).
        db.update_user_profile(
            &current_user.user_id,
            req.display_name.as_deref(),
            req.bio.as_deref(),
            req.website.as_deref(),
            req.phone.as_deref(),
            req.organization.as_deref(),
            req.is_public.unwrap_or(user.is_public),
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    } else if let Some(is_public) = req.is_public {
        // Privacy-toggle-only path.
        db.update_user_public(&current_user.user_id, is_public)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let updated = db
        .get_user_by_id(&current_user.user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    // Rewrite the FOAF/VCARD profile named graph.
    user_graph::write_user_profile_graph(&state.store, &state.base_url, &updated);

    Ok(Json(UserResponse::from(updated)))
}

/// POST /api/auth/change-password
pub async fn change_password(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(audit_log): State<Arc<AuditLogger>>,
    headers: HeaderMap,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let user = db
        .get_user_by_id(&current_user.user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    let valid = password::verify_password(&req.current_password, &user.password_hash)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !valid {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Current password is incorrect".to_string(),
        ));
    }

    if req.new_password.len() < 8 || req.new_password.len() > 1024 {
        return Err((
            StatusCode::BAD_REQUEST,
            "New password must be between 8 and 1024 characters".to_string(),
        ));
    }

    let new_hash = password::hash_password(&req.new_password)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    db.update_password(&current_user.user_id, &new_hash)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Invalidate all existing sessions on password change: a changed password must
    // cut off any other (possibly attacker-held) session. Refresh tokens are
    // revoked so they can no longer mint access tokens; the short-lived access JWT
    // for the current caller expires on its own. API tokens are intentionally left
    // intact (they are managed separately under /api/auth/tokens).
    let _ = db.revoke_all_user_refresh_tokens(&current_user.user_id);

    {
        let mut b = AuditEventBuilder::new(AuditEventType::PasswordChanged, AuditOutcome::Success)
            .actor(&current_user.user_id, &user.username, user.role.as_str());
        b.ip_address = audit::client_ip(&headers, None);
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        audit_log.log(b);
    }

    Ok(StatusCode::NO_CONTENT)
}

// ─── API Token handlers ──────────────────────────────────────────────────────

/// GET /api/auth/tokens
pub async fn list_api_tokens(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let tokens = db
        .list_api_tokens(&current_user.user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<ApiTokenResponse> = tokens.into_iter().map(ApiTokenResponse::from).collect();
    Ok(Json(response))
}

/// POST /api/auth/tokens
pub async fn create_api_token(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(audit_log): State<Arc<AuditLogger>>,
    headers: HeaderMap,
    Json(req): Json<CreateApiTokenRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if req.name.is_empty() || req.name.len() > 100 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Token name must be 1-100 characters".to_string(),
        ));
    }

    let scopes: Vec<ApiScope> = req
        .scopes
        .iter()
        .filter_map(|s| ApiScope::from_str(s))
        .collect();
    if scopes.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "At least one valid scope required (read, write, admin)".to_string(),
        ));
    }

    // Non-admins cannot request admin scope
    if scopes.contains(&ApiScope::Admin) && !current_user.is_admin() {
        return Err((
            StatusCode::FORBIDDEN,
            "Admin scope requires admin privileges".to_string(),
        ));
    }

    let raw_token = jwt::generate_api_token();
    let token_hash = hash_token(&raw_token);
    let token_prefix = if raw_token.len() > 11 {
        format!("{}...", &raw_token[..11])
    } else {
        raw_token.clone()
    };

    let expires_at = req.expires_in_days.map(|days| {
        let exp = chrono::Utc::now() + chrono::Duration::days(days as i64);
        exp.to_rfc3339()
    });

    let id = uuid::Uuid::new_v4().to_string();
    let token = db
        .create_api_token(
            &id,
            &current_user.user_id,
            &req.name,
            &token_hash,
            &token_prefix,
            &scopes,
            expires_at.as_deref(),
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    {
        let mut b = AuditEventBuilder::new(AuditEventType::TokenCreated, AuditOutcome::Success)
            .actor_id(&current_user.user_id)
            .resource("api_token", &token.id)
            .details(serde_json::json!({ "name": token.name, "scopes": token.scopes }));
        b.ip_address = audit::client_ip(&headers, None);
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        audit_log.log(b);
    }

    Ok((
        StatusCode::CREATED,
        Json(ApiTokenCreatedResponse {
            token: raw_token,
            id: token.id,
            name: token.name,
            token_prefix: token.token_prefix,
            scopes: token.scopes,
            expires_at: token.expires_at,
        }),
    ))
}

/// DELETE /api/auth/tokens/:token_id
pub async fn revoke_api_token(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(audit_log): State<Arc<AuditLogger>>,
    headers: HeaderMap,
    Path(token_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let token = db
        .get_api_token_by_id(&token_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Token not found".to_string()))?;

    if token.user_id != current_user.user_id && !current_user.is_admin() {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    db.revoke_api_token(&token_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    {
        let mut b = AuditEventBuilder::new(AuditEventType::TokenRevoked, AuditOutcome::Success)
            .actor_id(&current_user.user_id)
            .resource("api_token", &token_id);
        b.ip_address = audit::client_ip(&headers, None);
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        audit_log.log(b);
    }

    Ok(StatusCode::NO_CONTENT)
}

// ─── Admin user management handlers ──────────────────────────────────────────

/// GET /api/admin/users
pub async fn admin_list_users(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Query(params): Query<PaginationParams>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authz::require_admin(&current_user)?;

    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let search = params.search.as_deref();

    let (users, total) = db
        .list_users_paginated(page, limit, search)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<UserResponse> = users.into_iter().map(UserResponse::from).collect();
    Ok(Json(serde_json::json!({
        "users": response,
        "total": total,
        "page": page,
        "limit": limit,
    })))
}

/// POST /api/admin/users
pub async fn admin_create_user(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(audit_log): State<Arc<AuditLogger>>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AdminCreateUserRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authz::require_admin(&current_user)?;

    if req.username.len() < 3 || req.username.len() > 50 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Username must be 3-50 characters".to_string(),
        ));
    }
    if req.password.len() < 8 || req.password.len() > 1024 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Password must be between 8 and 1024 characters".to_string(),
        ));
    }

    if db
        .get_user_by_username(&req.username)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_some()
    {
        return Err((StatusCode::CONFLICT, "Username already taken".to_string()));
    }

    let role = req
        .role
        .as_deref()
        .map(|r| {
            SystemRole::from_str(r)
                .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid role".to_string()))
        })
        .transpose()?
        .unwrap_or(SystemRole::User);

    // Enforce role hierarchy: can't create users above your own level
    if !authz::can_grant_role(&current_user, role) {
        return Err((
            StatusCode::FORBIDDEN,
            "Cannot create user with higher role than your own".to_string(),
        ));
    }

    let hash = password::hash_password(&req.password)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let id = uuid::Uuid::new_v4().to_string();
    let user = db
        .create_user(&id, &req.username, &req.email, &hash, role)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if req.can_publish == Some(true) {
        let _ = db.update_user_can_publish(&id, true);
    }

    {
        let mut b = AuditEventBuilder::new(AuditEventType::UserCreated, AuditOutcome::Success)
            .actor_id(&current_user.user_id)
            .resource("user", &user.id)
            .details(serde_json::json!({ "username": user.username, "role": role.as_str() }));
        b.ip_address = audit::client_ip(&headers, None);
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        audit_log.log(b);
    }

    // Write the FOAF/VCARD profile named graph for the new user.
    user_graph::write_user_profile_graph(&state.store, &state.base_url, &user);

    Ok((StatusCode::CREATED, Json(UserResponse::from(user))))
}

/// GET /api/admin/users/:user_id
pub async fn admin_get_user(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(user_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authz::require_admin(&current_user)?;

    let user = db
        .get_user_by_id(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    Ok(Json(UserResponse::from(user)))
}

/// GET /api/admin/users/:user_id/identities — list OAuth/SAML identities linked to a user.
pub async fn admin_list_user_identities(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(user_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authz::require_admin(&current_user)?;

    let identities = db
        .list_oauth_identities_for_user(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(identities))
}

/// PUT /api/admin/users/:user_id
pub async fn admin_update_user(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(audit_log): State<Arc<AuditLogger>>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Json(req): Json<AdminUpdateUserRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authz::require_admin(&current_user)?;

    let target = db
        .get_user_by_id(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    // Admin cannot modify users at or above their own level (except themselves)
    if !authz::can_administer_user(&current_user, &user_id, target.role) {
        return Err((
            StatusCode::FORBIDDEN,
            "Cannot modify user with equal or higher role".to_string(),
        ));
    }

    // Cannot self-promote
    if user_id == current_user.user_id {
        if let Some(ref role_str) = req.role {
            if let Some(new_role) = SystemRole::from_str(role_str) {
                if !authz::can_grant_role(&current_user, new_role) {
                    return Err((StatusCode::FORBIDDEN, "Cannot self-promote".to_string()));
                }
            }
        }
    }

    if let Some(ref email) = req.email {
        db.update_user(&user_id, &target.username, email)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    if let Some(ref role_str) = req.role {
        let new_role = SystemRole::from_str(role_str)
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid role".to_string()))?;

        // Can't assign role above your own level
        if !authz::can_grant_role(&current_user, new_role) {
            return Err((
                StatusCode::FORBIDDEN,
                "Cannot assign role higher than your own".to_string(),
            ));
        }

        // Don't let demotion of the last active super_admin lock the system out.
        if target.role == SystemRole::SuperAdmin
            && new_role != SystemRole::SuperAdmin
            && target.is_active
            && db.count_active_super_admins().unwrap_or(0) <= 1
        {
            return Err((
                StatusCode::CONFLICT,
                "Cannot demote the last active super admin".to_string(),
            ));
        }

        db.update_user_role(&user_id, new_role)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let mut b = AuditEventBuilder::new(AuditEventType::RoleChanged, AuditOutcome::Success)
            .actor_id(&current_user.user_id)
            .resource("user", &user_id)
            .details(serde_json::json!({
                "from": target.role.as_str(),
                "to": new_role.as_str(),
            }));
        b.ip_address = audit::client_ip(&headers, None);
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        audit_log.log(b);
    }

    if let Some(is_active) = req.is_active {
        // Don't let deactivation of the last active super_admin lock the system out.
        if !is_active
            && target.role == SystemRole::SuperAdmin
            && target.is_active
            && db.count_active_super_admins().unwrap_or(0) <= 1
        {
            return Err((
                StatusCode::CONFLICT,
                "Cannot deactivate the last active super admin".to_string(),
            ));
        }

        db.set_user_active(&user_id, is_active)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // If deactivating, revoke all tokens and apply side-effects
        if !is_active {
            let _ = db.revoke_all_user_refresh_tokens(&user_id);
            let _ = db.revoke_all_user_api_tokens(&user_id);
            apply_deactivation_effects(&db, &user_id);
        }

        let evt = if is_active {
            AuditEventType::UserActivated
        } else {
            AuditEventType::UserDeactivated
        };
        let mut b = AuditEventBuilder::new(evt, AuditOutcome::Success)
            .actor_id(&current_user.user_id)
            .resource("user", &user_id);
        b.ip_address = audit::client_ip(&headers, None);
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        audit_log.log(b);
    }

    if let Some(can_publish) = req.can_publish {
        db.update_user_can_publish(&user_id, can_publish)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let updated = db
        .get_user_by_id(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    // Rewrite the FOAF/VCARD profile named graph to reflect any changes.
    user_graph::write_user_profile_graph(&state.store, &state.base_url, &updated);

    Ok(Json(UserResponse::from(updated)))
}

/// DELETE /api/admin/users/:user_id — deactivates user and revokes tokens
pub async fn admin_delete_user(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(audit_log): State<Arc<AuditLogger>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authz::require_admin(&current_user)?;

    if user_id == current_user.user_id {
        return Err((
            StatusCode::BAD_REQUEST,
            "Cannot deactivate yourself".to_string(),
        ));
    }

    let target = db
        .get_user_by_id(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    if !authz::can_administer_user(&current_user, &user_id, target.role) {
        return Err((
            StatusCode::FORBIDDEN,
            "Cannot deactivate user with equal or higher role".to_string(),
        ));
    }

    db.set_user_active(&user_id, false)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let _ = db.revoke_all_user_refresh_tokens(&user_id);
    let _ = db.revoke_all_user_api_tokens(&user_id);

    apply_deactivation_effects(&db, &user_id);

    {
        let mut b = AuditEventBuilder::new(AuditEventType::UserDeactivated, AuditOutcome::Success)
            .actor_id(&current_user.user_id)
            .resource("user", &user_id)
            .details(serde_json::json!({ "via": "admin_delete_user" }));
        b.ip_address = audit::client_ip(&headers, None);
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        audit_log.log(b);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/admin/users/:user_id/reset-password
pub async fn admin_reset_password(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(audit_log): State<Arc<AuditLogger>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Json(req): Json<AdminResetPasswordRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authz::require_admin(&current_user)?;

    let target = db
        .get_user_by_id(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    if !authz::can_administer_user(&current_user, &user_id, target.role) {
        return Err((
            StatusCode::FORBIDDEN,
            "Cannot reset password for user with equal or higher role".to_string(),
        ));
    }

    if req.new_password.len() < 8 || req.new_password.len() > 1024 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Password must be between 8 and 1024 characters".to_string(),
        ));
    }

    let new_hash = password::hash_password(&req.new_password)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    db.update_password(&user_id, &new_hash)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Revoke all refresh tokens to force re-login
    let _ = db.revoke_all_user_refresh_tokens(&user_id);

    {
        let mut b =
            AuditEventBuilder::new(AuditEventType::PasswordResetForced, AuditOutcome::Success)
                .actor_id(&current_user.user_id)
                .resource("user", &user_id);
        b.ip_address = audit::client_ip(&headers, None);
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        audit_log.log(b);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/admin/users/:user_id/purge — permanently delete a deactivated user
pub async fn admin_purge_user(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(audit_log): State<Arc<AuditLogger>>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authz::require_admin(&current_user)?;
    if user_id == current_user.user_id {
        return Err((StatusCode::BAD_REQUEST, "Cannot purge yourself".to_string()));
    }
    let target = db
        .get_user_by_id(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;
    if target.is_active {
        return Err((
            StatusCode::CONFLICT,
            "User must be deactivated before permanent deletion".to_string(),
        ));
    }
    if !authz::can_administer_user(&current_user, &user_id, target.role) {
        return Err((
            StatusCode::FORBIDDEN,
            "Cannot purge user with equal or higher role".to_string(),
        ));
    }
    // Delete the user's FOAF/VCARD profile named graph before removing the row.
    let profile_graph = user_graph::user_profile_graph_iri(&user_id);
    let _ = state.store.graph_store_delete(Some(&profile_graph));

    db.delete_user(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    {
        let mut b = AuditEventBuilder::new(AuditEventType::UserDeleted, AuditOutcome::Success)
            .actor_id(&current_user.user_id)
            .resource("user", &user_id)
            .details(serde_json::json!({ "username": target.username }));
        b.ip_address = audit::client_ip(&headers, None);
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        audit_log.log(b);
    }

    Ok(StatusCode::NO_CONTENT)
}

// ─── Deactivation side-effects helper ────────────────────────────────────────

/// Called whenever a user account is deactivated (by admin or by the user themselves).
///
/// - Makes all personal datasets private so their data is no longer publicly visible.
/// - For each org the user belongs to: if no other active member remains, makes all
///   org-owned datasets private as well (the org effectively "goes dark").
/// - Removes the user from every group and organisation.
fn apply_deactivation_effects(db: &AuthDb, user_id: &str) {
    let _ = db.make_user_datasets_private(user_id);

    let org_ids = db.get_user_org_ids(user_id).unwrap_or_default();
    for org_id in &org_ids {
        if let Ok(0) = db.count_org_other_active_members(org_id, user_id) {
            let _ = db.make_org_datasets_private(org_id);
        }
    }

    let _ = db.remove_user_from_all_orgs_and_groups(user_id);
}

// ─── Self-account management ──────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct AccountActionRequest {
    pub password: String,
}

/// DELETE /api/auth/account — the authenticated user deactivates their own account.
///
/// Requires password confirmation. Super admins are blocked (use admin panel).
/// After deactivation: all tokens revoked, datasets made private, org memberships removed.
pub async fn self_deactivate(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Json(req): Json<AccountActionRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if current_user.role == SystemRole::SuperAdmin {
        return Err((
            StatusCode::FORBIDDEN,
            "Super admins cannot deactivate their own account via self-service. Contact another super admin.".to_string(),
        ));
    }

    let user = db
        .get_user_by_id(&current_user.user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    let valid = password::verify_password(&req.password, &user.password_hash)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !valid {
        return Err((StatusCode::UNAUTHORIZED, "Incorrect password".to_string()));
    }

    db.set_user_active(&current_user.user_id, false)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let _ = db.revoke_all_user_refresh_tokens(&current_user.user_id);
    let _ = db.revoke_all_user_api_tokens(&current_user.user_id);

    apply_deactivation_effects(&db, &current_user.user_id);

    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/auth/account/purge — the authenticated user permanently deletes their own account.
///
/// Requires password confirmation. Account must not be super_admin.
/// All personal data, org memberships, and asset metadata are removed permanently.
pub async fn self_purge(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(state): State<AppState>,
    Json(req): Json<AccountActionRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if current_user.role == SystemRole::SuperAdmin {
        return Err((
            StatusCode::FORBIDDEN,
            "Super admins cannot delete their own account via self-service.".to_string(),
        ));
    }

    let user = db
        .get_user_by_id(&current_user.user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    let valid = password::verify_password(&req.password, &user.password_hash)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !valid {
        return Err((StatusCode::UNAUTHORIZED, "Incorrect password".to_string()));
    }

    // Apply side-effects before deleting (org cleanup, make datasets private)
    apply_deactivation_effects(&db, &current_user.user_id);
    let _ = db.revoke_all_user_refresh_tokens(&current_user.user_id);
    let _ = db.revoke_all_user_api_tokens(&current_user.user_id);

    // Delete the FOAF/VCARD profile named graph.
    let profile_graph = user_graph::user_profile_graph_iri(&current_user.user_id);
    let _ = state.store.graph_store_delete(Some(&profile_graph));

    db.delete_user(&current_user.user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Auth helper ─────────────────────────────────────────────────────────────

/// Extract the authenticated user from an optional extension, returning 401
/// when no valid token was provided.  Used by handlers that sit behind
/// `optional_auth` middleware but still require authentication for their
/// specific HTTP method (e.g. POST on a path whose GET is public).
fn require_user(
    user: Option<Extension<AuthenticatedUser>>,
) -> Result<AuthenticatedUser, (StatusCode, String)> {
    user.map(|Extension(u)| u).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            "Authentication required".to_string(),
        )
    })
}

// ─── Organisation handlers ────────────────────────────────────────────────────

/// POST /api/organisations
pub async fn create_organisation(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    State(state): State<AppState>,
    Json(req): Json<CreateOrgRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    // Creating an organisation grants the caller org-admin powers (line below
    // adds them as Admin) — which then lets them invite arbitrary users and
    // own datasets/ontologies under that org. Restrict to platform admins so
    // org provisioning stays an explicit operator action.
    if !current_user.is_admin() {
        return Err((
            StatusCode::FORBIDDEN,
            "Admin access required to create an organisation".to_string(),
        ));
    }
    // A parent, if given, must reference an existing organisation.
    let parent = req.parent_org_id.as_deref().filter(|s| !s.is_empty());
    if let Some(pid) = parent {
        if db
            .get_organisation(pid)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .is_none()
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "Parent organisation not found".to_string(),
            ));
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let org = db
        .create_organisation(
            &id,
            &req.name,
            &req.slug,
            req.description.as_deref(),
            parent,
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Creator becomes admin
    db.add_org_member(&current_user.user_id, &id, Role::Admin)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Write the organisation knowledge graph; refresh the parent so its
    // hasSubOrganization list picks up the new child.
    refresh_org_metadata(&state, &id);
    if let Some(pid) = parent {
        refresh_org_metadata(&state, pid);
    }

    Ok((StatusCode::CREATED, Json(org)))
}

/// GET /api/organisations
pub async fn list_organisations(
    user: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Admins see all organisations.
    // Regular authenticated users see the organisations they are a member of
    // (so they can't accidentally import into orgs they don't belong to).
    // Everyone — including unauthenticated callers — additionally sees any
    // organisation that owns at least one public dataset, so public reference
    // orgs (e.g. the bundled "Open Triplestore" demo) are discoverable.
    let mut orgs = match user {
        Some(Extension(ref u)) if u.is_admin() => db.list_organisations(),
        Some(Extension(ref u)) => db.list_user_organisations(&u.user_id),
        None => Ok(vec![]),
    }
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let is_admin = matches!(&user, Some(Extension(u)) if u.is_admin());
    if !is_admin {
        let seen: std::collections::HashSet<String> = orgs.iter().map(|o| o.id.clone()).collect();
        let public = public_dataset_orgs(&db)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        for org in public {
            if !seen.contains(&org.id) {
                orgs.push(org);
            }
        }
    }

    Ok(Json(orgs))
}

/// Organisations that own at least one public dataset (anonymously visible).
fn public_dataset_orgs(db: &AuthDb) -> anyhow::Result<Vec<crate::auth::models::Organisation>> {
    let mut ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for d in db.list_datasets()? {
        if matches!(d.owner_type, OwnerType::Organisation)
            && matches!(d.visibility, Visibility::Public)
        {
            ids.insert(d.owner_id);
        }
    }
    let mut orgs = Vec::new();
    for id in ids {
        if let Some(org) = db.get_organisation(&id)? {
            orgs.push(org);
        }
    }
    Ok(orgs)
}

/// True when `org_id` owns at least one public dataset.
fn org_has_public_dataset(db: &AuthDb, org_id: &str) -> bool {
    db.list_datasets()
        .map(|datasets| {
            datasets.iter().any(|d| {
                d.owner_id == org_id
                    && matches!(d.owner_type, OwnerType::Organisation)
                    && matches!(d.visibility, Visibility::Public)
            })
        })
        .unwrap_or(false)
}

/// GET /api/organisations/:org_id
pub async fn get_organisation(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    Path(org_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let org = db
        .get_organisation(&org_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Organisation not found".to_string()))?;

    // Readable by: admins; members; or anyone when the org owns a public dataset
    // (public reference orgs like the demo are browsable without an account).
    let is_admin = user_opt
        .as_ref()
        .map(|Extension(u)| u.is_admin())
        .unwrap_or(false);
    let is_member = match &user_opt {
        Some(Extension(u)) => db
            .get_org_membership(&u.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .is_some(),
        None => false,
    };
    if !is_admin && !is_member && !org_has_public_dataset(&db, &org_id) {
        // Preserve the prior contract: anonymous → 401, signed-in non-member → 403.
        return Err(match user_opt {
            Some(_) => (StatusCode::FORBIDDEN, "Not a member".to_string()),
            None => (
                StatusCode::UNAUTHORIZED,
                "Authentication required".to_string(),
            ),
        });
    }

    Ok(Json(org))
}

/// Schemes permitted in stored DCAT / vCard URL metadata. Mirrors the frontend
/// `safeExternalUrl` allowlist (frontend/src/lib/safeUrl.ts): `mailto:` is kept
/// for contact URLs, everything dangerous (`javascript:`, `data:`, `file:`, …)
/// is rejected.
const SAFE_METADATA_URL_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// Reject DCAT / vCard URL metadata that does not use a safe web scheme.
///
/// These fields are later rendered as `<a href>` on public (and anonymously
/// viewable) pages, so a `javascript:`/`data:` value would be stored XSS in
/// waiting. The frontend gates the href as defence-in-depth; this is the
/// root-cause fix that keeps such values out of storage entirely. An empty /
/// `None` value clears the field and is always allowed.
fn validate_metadata_url(field: &str, value: Option<&str>) -> Result<(), (StatusCode, String)> {
    if let Some(raw) = value {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        let scheme_ok = url::Url::parse(trimmed)
            .map(|u| SAFE_METADATA_URL_SCHEMES.contains(&u.scheme()))
            .unwrap_or(false);
        if !scheme_ok {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("{field} must be an http(s) or mailto URL"),
            ));
        }
    }
    Ok(())
}

/// PUT /api/organisations/:org_id
pub async fn update_organisation(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    Json(req): Json<UpdateOrgRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    if !current_user.is_admin() {
        match db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }

    // Reject unsafe URL schemes before they reach storage (these surface as
    // <a href> on the org's public page).
    validate_metadata_url("homepage", req.homepage.as_deref())?;
    validate_metadata_url("contact_url", req.contact_url.as_deref())?;

    // Resolve and validate the requested parent (empty string clears it).
    let existing = db
        .get_organisation(&org_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Organisation not found".to_string()))?;
    let new_parent = req
        .parent_org_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(pid) = new_parent {
        if pid == org_id {
            return Err((
                StatusCode::BAD_REQUEST,
                "An organisation cannot be its own parent".to_string(),
            ));
        }
        if db
            .get_organisation(pid)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .is_none()
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "Parent organisation not found".to_string(),
            ));
        }
        // Reject cycles: the chosen parent must not be a descendant of this org.
        if db
            .is_org_ancestor(&org_id, pid)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "Cannot set a descendant organisation as the parent (would create a cycle)"
                    .to_string(),
            ));
        }
    }

    db.update_organisation(
        &org_id,
        &req.name,
        req.description.as_deref(),
        req.homepage.as_deref(),
        req.identifier.as_deref(),
        req.contact_name.as_deref(),
        req.contact_email.as_deref(),
        req.contact_url.as_deref(),
        // org_type is NOT NULL; preserve the existing value when the request
        // omits it rather than writing NULL (which violates the constraint).
        req.org_type.as_deref().or(existing.org_type.as_deref()),
        new_parent,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let org = db
        .get_organisation(&org_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Organisation not found".to_string()))?;

    // Rewrite this org's knowledge graph, plus the old and new parent graphs so
    // their hasSubOrganization links stay in sync after a re-parenting.
    refresh_org_metadata(&state, &org_id);
    let old_parent = existing.parent_org_id.as_deref().filter(|s| !s.is_empty());
    if let Some(p) = old_parent {
        refresh_org_metadata(&state, p);
    }
    if let Some(p) = new_parent {
        if Some(p) != old_parent {
            refresh_org_metadata(&state, p);
        }
    }

    Ok(Json(org))
}

/// DELETE /api/organisations/:org_id
pub async fn delete_organisation(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(org_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    if !current_user.is_admin() {
        match state
            .auth_db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }

    // Delete all datasets owned by this organisation (and their named graphs).
    let dataset_ids = state
        .auth_db
        .list_dataset_ids_by_owner(&org_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    for dataset_id in &dataset_ids {
        let dataset = state
            .auth_db
            .get_dataset(dataset_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let graph_iris = state
            .auth_db
            .list_dataset_graphs(dataset_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        for iri in &graph_iris {
            let shared = state
                .auth_db
                .graph_has_other_dataset_refs(iri.as_str(), dataset_id)
                .unwrap_or(false);
            if !shared {
                let _ = state.store.graph_store_delete(Some(iri.as_str()));
            }
        }
        // Also delete shapes graph when present.
        if let Some(ref d) = dataset {
            if let Some(ref shapes_iri) = d.shapes_graph_iri {
                let _ = state.store.graph_store_delete(Some(shapes_iri.as_str()));
            }
        }
        state
            .auth_db
            .delete_dataset(dataset_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    // Capture hierarchy before deletion so we can keep graphs consistent and
    // avoid the parent_org_id foreign key blocking the delete.
    let former_parent = state
        .auth_db
        .get_organisation(&org_id)
        .ok()
        .flatten()
        .and_then(|o| o.parent_org_id)
        .filter(|p| !p.is_empty());
    let children = state
        .auth_db
        .list_child_organisations(&org_id)
        .unwrap_or_default();
    // Orphan direct children (promote to top-level) so the FK reference clears.
    for child in &children {
        let _ = state.auth_db.update_organisation(
            &child.id,
            &child.name,
            child.description.as_deref(),
            child.homepage.as_deref(),
            child.identifier.as_deref(),
            child.contact_name.as_deref(),
            child.contact_email.as_deref(),
            child.contact_url.as_deref(),
            child.org_type.as_deref(),
            None,
        );
    }

    state
        .auth_db
        .delete_organisation(&org_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Tear down the org knowledge graph and refresh affected neighbours.
    crate::auth::org_graph::delete_org_metadata_graph(&state.store, &org_id);
    if let Some(p) = former_parent {
        refresh_org_metadata(&state, &p);
    }
    for child in &children {
        refresh_org_metadata(&state, &child.id);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/organisations/:org_id/members
pub async fn list_org_members(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(org_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !current_user.is_admin() {
        db.get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .ok_or_else(|| (StatusCode::FORBIDDEN, "Not a member".to_string()))?;
    }

    let members = db
        .list_org_members(&org_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Flat shape consumed by both frontends (user_id / username / role).
    let response: Vec<serde_json::Value> = members
        .into_iter()
        .map(|(user, role)| {
            serde_json::json!({
                "user_id": user.id,
                "username": user.username,
                "email": user.email,
                "display_name": user.display_name,
                "role": role.as_str(),
            })
        })
        .collect();

    Ok(Json(response))
}

/// POST /api/organisations/:org_id/members
pub async fn add_org_member(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(org_id): Path<String>,
    Json(req): Json<AddMemberRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !current_user.is_admin() {
        match db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }

    let role = Role::from_str(&req.role)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid role".to_string()))?;

    // Validate the target user exists before writing a membership row (the DB uses
    // INSERT OR REPLACE with no FK check, so a bogus user_id would otherwise create
    // a phantom membership).
    if db
        .get_user_by_id(&req.user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        return Err((StatusCode::BAD_REQUEST, "User not found".to_string()));
    }

    db.add_org_member(&req.user_id, &org_id, role)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::CREATED)
}

/// DELETE /api/organisations/:org_id/members/:user_id
pub async fn remove_org_member(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path((org_id, user_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !current_user.is_admin() {
        match db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }

    // Super admins cannot be removed from any organisation.
    let target = db
        .get_user_by_id(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Some(u) = target {
        if u.role == SystemRole::SuperAdmin {
            return Err((
                StatusCode::FORBIDDEN,
                "Super admins cannot be removed from an organisation".to_string(),
            ));
        }
    }

    db.remove_org_member(&user_id, &org_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// PUT /api/organisations/:org_id/members/:user_id
/// Update the role of an existing member.
pub async fn update_org_member_role(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path((org_id, user_id)): Path<(String, String)>,
    Json(req): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !current_user.is_admin() {
        match db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }

    let role = req["role"]
        .as_str()
        .and_then(|r| Role::from_str(r))
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "Invalid or missing role".to_string(),
            )
        })?;

    // add_org_member uses INSERT OR REPLACE so it also handles updates
    db.add_org_member(&user_id, &org_id, role)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Group handlers ───────────────────────────────────────────────────────────

/// POST /api/organisations/:org_id/groups
pub async fn create_group(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(org_id): Path<String>,
    Json(req): Json<CreateGroupRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !current_user.is_admin() {
        match db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    let group = db
        .create_group(&id, &org_id, &req.name, req.parent_group_id.as_deref())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok((StatusCode::CREATED, Json(group)))
}

/// GET /api/organisations/:org_id/groups
pub async fn list_groups(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(org_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !current_user.is_admin() {
        db.get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .ok_or_else(|| (StatusCode::FORBIDDEN, "Not a member".to_string()))?;
    }

    let groups = db
        .list_org_groups(&org_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(groups))
}

/// GET /api/organisations/:org_id/groups/:group_id
pub async fn get_group(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path((org_id, group_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !current_user.is_admin() {
        db.get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .ok_or_else(|| (StatusCode::FORBIDDEN, "Not a member".to_string()))?;
    }

    let group = db
        .get_group(&group_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Group not found".to_string()))?;

    if group.org_id != org_id {
        return Err((
            StatusCode::NOT_FOUND,
            "Group not found in this organisation".to_string(),
        ));
    }

    Ok(Json(group))
}

/// PUT /api/organisations/:org_id/groups/:group_id
pub async fn update_group(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path((org_id, group_id)): Path<(String, String)>,
    Json(req): Json<UpdateGroupRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !current_user.is_admin() {
        match db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }

    db.update_group(&group_id, &req.name, req.parent_group_id.as_deref())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let group = db
        .get_group(&group_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Group not found".to_string()))?;

    Ok(Json(group))
}

/// DELETE /api/organisations/:org_id/groups/:group_id
pub async fn delete_group(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path((org_id, group_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !current_user.is_admin() {
        match db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }

    db.delete_group(&group_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/organisations/:org_id/groups/:group_id/members
pub async fn list_group_members(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path((org_id, group_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !current_user.is_admin() {
        db.get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .ok_or_else(|| (StatusCode::FORBIDDEN, "Not a member".to_string()))?;
    }

    let members = db
        .list_group_members(&group_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Flat shape consumed by both frontends (user_id / username / role).
    let response: Vec<serde_json::Value> = members
        .into_iter()
        .map(|(user, role)| {
            serde_json::json!({
                "user_id": user.id,
                "username": user.username,
                "email": user.email,
                "display_name": user.display_name,
                "role": role.as_str(),
            })
        })
        .collect();

    Ok(Json(response))
}

/// POST /api/organisations/:org_id/groups/:group_id/members
pub async fn add_group_member(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path((org_id, group_id)): Path<(String, String)>,
    Json(req): Json<AddMemberRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !current_user.is_admin() {
        match db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }

    let role = Role::from_str(&req.role)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid role".to_string()))?;

    // Validate the target user exists (INSERT OR REPLACE has no FK check).
    if db
        .get_user_by_id(&req.user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        return Err((StatusCode::BAD_REQUEST, "User not found".to_string()));
    }

    db.add_group_member(&req.user_id, &group_id, role)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::CREATED)
}

/// DELETE /api/organisations/:org_id/groups/:group_id/members/:user_id
pub async fn remove_group_member(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path((org_id, group_id, user_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !current_user.is_admin() {
        match db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }

    // Super admins cannot be removed from any group.
    let target = db
        .get_user_by_id(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Some(u) = target {
        if u.role == SystemRole::SuperAdmin {
            return Err((
                StatusCode::FORBIDDEN,
                "Super admins cannot be removed from a group".to_string(),
            ));
        }
    }

    db.remove_group_member(&user_id, &group_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Dataset handlers ─────────────────────────────────────────────────────────

/// POST /api/datasets
pub async fn create_dataset(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    State(state): State<AppState>,
    Json(req): Json<CreateDatasetRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let owner_type = OwnerType::from_str(&req.owner_type)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid owner_type".to_string()))?;

    let visibility = req
        .visibility
        .as_deref()
        .map(Visibility::from_str)
        .unwrap_or(Some(Visibility::Private))
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid visibility".to_string()))?;

    // Authorization: a non-admin may only create datasets owned by themselves or
    // by an organisation/group they belong to — otherwise `owner_id` could be
    // forged to impersonate another principal or attribute data to a foreign
    // catalogue. Publishing (visibility=public) additionally requires publisher
    // rights, mirroring the visibility gate in `update_dataset`.
    if !current_user.is_admin() {
        if !db
            .can_act_as_owner(&current_user.user_id, owner_type, &req.owner_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            return Err((
                StatusCode::FORBIDDEN,
                "You may only create datasets owned by yourself or an organisation/group you belong to"
                    .to_string(),
            ));
        }
        if visibility == Visibility::Public && !current_user.is_publisher() {
            return Err((
                StatusCode::FORBIDDEN,
                "Publisher access is required to create a public dataset".to_string(),
            ));
        }
    }

    // Human-readable, unique slug id → IRI `{base}/dataset/{id}` reads semantically
    // (e.g. `…/dataset/bridge-inventory`) instead of exposing a raw UUID.
    let id = unique_dataset_slug(&db, &req.name);
    let graph_role = req.graph_role.as_deref().and_then(GraphKind::from_str);
    let dataset = db
        .create_dataset(
            &id,
            &req.name,
            req.description.as_deref(),
            owner_type,
            &req.owner_id,
            visibility,
            graph_role,
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Set ontology conformance if provided
    if req.conforms_to_ontology.is_some() || req.conforms_to_version.is_some() {
        db.update_dataset_conformance(
            &id,
            req.conforms_to_ontology.as_deref(),
            req.conforms_to_version.as_deref(),
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let dataset = db
        .get_dataset(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap_or(dataset);

    // Auto-provision a default SPARQL service so the dataset is immediately
    // queryable via /api/datasets/:id/services/sparql/sparql without any
    // manual setup step.
    let svc_id = uuid::Uuid::new_v4().to_string();
    let _ = db.create_sparql_service(&svc_id, &id, "SPARQL Endpoint", "sparql", None);

    // Write the DCAT metadata named graph for the new dataset, enforcing the
    // built-in dataset-structure SHACL model. A well-formed dataset always
    // carries title/identifier/visibility (emitted by the writer) so this only
    // trips on genuinely malformed metadata — rejected as 422 + report.
    if let Err(report) = dataset_graph::write_dataset_metadata_graph_checked(
        &state.store,
        &state.base_url,
        &dataset,
        &[],
    ) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::to_string(&report)
                .unwrap_or_else(|_| "dataset metadata failed validation".to_string()),
        ));
    }

    // Surface the canonical dataset IRI alongside the raw fields so clients can
    // immediately mint graph IRIs under `{dataset_iri}/...` (which the bulk-import
    // write boundary admits) — important for the import wizard's lazy
    // create-then-import flow, where the new id isn't known until now.
    let mut body = serde_json::to_value(&dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "dataset_iri".to_string(),
            serde_json::Value::String(dataset_graph::dataset_iri(&state.base_url, &dataset.id)),
        );
    }
    Ok((StatusCode::CREATED, Json(body)))
}

/// GET /api/datasets
pub async fn list_datasets(
    user: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    State(base_url): State<crate::server::BaseUrl>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let user_id = user.as_ref().map(|u| u.user_id.as_str());
    let datasets = db
        .list_accessible_datasets(user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // Distinct roles per dataset, fetched once for the whole list.
    let mut roles_by_dataset = db.all_dataset_roles().unwrap_or_default();

    let views: Vec<DatasetView> = datasets
        .into_iter()
        .map(|ds| {
            let effective_role = db.effective_dataset_role(user_id, &ds).ok().flatten();
            let can_write = effective_role.map(|r| r.can_write()).unwrap_or(false);
            let can_manage = effective_role.map(|r| r.can_manage()).unwrap_or(false);
            let mut roles = roles_by_dataset.remove(&ds.id).unwrap_or_default();
            // Fall back to the dataset-level role tag if no per-graph roles exist.
            if roles.is_empty() {
                if let Some(r) = ds.graph_role {
                    roles.push(r);
                }
            }
            let dataset_iri = dataset_graph::dataset_iri(&base_url.0, &ds.id);
            DatasetView {
                dataset: ds,
                dataset_iri,
                can_write,
                can_manage,
                effective_role,
                roles,
            }
        })
        .collect();

    Ok(Json(views))
}

/// GET /api/datasets/:dataset_id
pub async fn get_dataset(
    user: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    State(base_url): State<crate::server::BaseUrl>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    let user_id = user.as_ref().map(|u| u.user_id.as_str());
    if !db
        .can_access_dataset(user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    // Best-effort private usage telemetry (powers the "recently used" ranking).
    let _ = db.record_dataset_usage(&dataset_id, user_id, "view");

    let effective_role = db.effective_dataset_role(user_id, &dataset).ok().flatten();
    let can_write = effective_role.map(|r| r.can_write()).unwrap_or(false);
    let can_manage = effective_role.map(|r| r.can_manage()).unwrap_or(false);
    let mut roles: Vec<crate::auth::models::GraphKind> = db
        .list_dataset_graph_entries(&dataset.id)
        .map(|entries| {
            let mut rs: Vec<_> = entries.into_iter().filter_map(|e| e.graph_role).collect();
            rs.sort_by_key(|r| r.as_str());
            rs.dedup();
            rs
        })
        .unwrap_or_default();
    if roles.is_empty() {
        if let Some(r) = dataset.graph_role {
            roles.push(r);
        }
    }
    let dataset_iri = dataset_graph::dataset_iri(&base_url.0, &dataset.id);
    Ok(Json(DatasetView {
        dataset,
        dataset_iri,
        can_write,
        can_manage,
        effective_role,
        roles,
    }))
}

/// GET /api/me/dataset-usage — the calling user's OWN dataset usage, aggregated
/// per dataset (count + last used), most-used first. This is the caller reading
/// their own footprint (used to rank "recently used / use a lot"); it never
/// exposes anyone else's activity.
pub async fn my_dataset_usage(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
) -> Result<Json<Vec<DatasetUsageStat>>, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let stats = db
        .dataset_usage_for_user(&current_user.user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(stats))
}

#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    pub since: Option<String>,
    pub limit: Option<i64>,
}

/// GET /api/admin/dataset-usage — cross-user dataset usage aggregate. This is
/// private activity data, so it is super_admin only (the route layer requires
/// admin; the explicit check below narrows it to super_admin).
pub async fn admin_dataset_usage(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Query(q): Query<UsageQuery>,
) -> Result<Json<Vec<DatasetUsageStat>>, (StatusCode, String)> {
    if current_user.role != SystemRole::SuperAdmin {
        return Err((StatusCode::FORBIDDEN, "super_admin only".into()));
    }
    let limit = q.limit.unwrap_or(1000).clamp(1, 100_000);
    let stats = db
        .dataset_usage_all(q.since.as_deref(), limit)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(stats))
}

/// PUT /api/datasets/:dataset_id
pub async fn update_dataset(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    Json(req): Json<UpdateDatasetRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    let visibility = Visibility::from_str(&req.visibility)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid visibility".to_string()))?;

    // Changing visibility (especially widening to public) is an owner/manager
    // action, not a plain write: a mere Editor (e.g. a regular org member on a
    // `members`-visibility org dataset) must not be able to publish it.
    if visibility != dataset.visibility
        && !db
            .can_manage_dataset(&current_user.user_id, &dataset)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((
            StatusCode::FORBIDDEN,
            "Manage access required to change visibility".to_string(),
        ));
    }

    // Reject unsafe URL schemes before they reach storage (these surface as
    // <a href> on the dataset's public DCAT metadata page).
    validate_metadata_url("license", req.license.as_deref())?;
    validate_metadata_url("spatial", req.spatial.as_deref())?;
    validate_metadata_url("landing_page", req.landing_page.as_deref())?;
    validate_metadata_url("contact_url", req.contact_url.as_deref())?;

    db.update_dataset(
        &dataset_id,
        &req.name,
        req.description.as_deref(),
        visibility,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Update ontology conformance
    db.update_dataset_conformance(
        &dataset_id,
        req.conforms_to_ontology.as_deref(),
        req.conforms_to_version.as_deref(),
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Update DCAT / ADMS / VoID metadata fields.
    // Themes and keywords arrive as Vec<String> and are stored as JSON arrays.
    let themes_json = req
        .themes
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default());
    let keywords_json = req
        .keywords
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default());
    db.update_dataset_metadata(
        &dataset_id,
        req.license.as_deref(),
        themes_json.as_deref(),
        keywords_json.as_deref(),
        req.contact_name.as_deref(),
        req.contact_email.as_deref(),
        req.contact_url.as_deref(),
        req.adms_status.as_deref(),
        req.version_notes.as_deref(),
        req.spatial.as_deref(),
        req.landing_page.as_deref(),
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let updated = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    // Rewrite the DCAT metadata named graph.
    refresh_dataset_metadata(&state, &dataset_id);

    Ok(Json(updated))
}

/// DELETE /api/datasets/:dataset_id
pub async fn delete_dataset(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    // Delete all named graphs associated with this dataset from Oxigraph,
    // but only when no other dataset still references the same graph IRI.
    let graph_iris = state
        .auth_db
        .list_dataset_graphs(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    for iri in &graph_iris {
        let shared = state
            .auth_db
            .graph_has_other_dataset_refs(iri.as_str(), &dataset_id)
            .unwrap_or(false);
        if !shared {
            // Tolerate errors (graph may already be absent); never abort the delete.
            let _ = state.store.graph_store_delete(Some(iri.as_str()));
        }
    }

    // Also delete the shapes graph, which is stored only in the datasets table
    // and is therefore not listed in dataset_graphs — it would otherwise be
    // left orphaned in Oxigraph.
    if let Some(ref shapes_iri) = dataset.shapes_graph_iri {
        let _ = state.store.graph_store_delete(Some(shapes_iri.as_str()));
    }

    // Delete the DCAT metadata named graph for this dataset.
    let meta_graph = dataset_graph::dataset_metadata_graph_iri(&dataset_id);
    let _ = state.store.graph_store_delete(Some(&meta_graph));

    state
        .auth_db
        .delete_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Dataset Graphs handlers ──────────────────────────────────────────────────

/// GET /api/datasets/:dataset_id/graphs
pub async fn list_dataset_graphs(
    user: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    let user_id = user.as_ref().map(|u| u.user_id.as_str());
    let effective_role = db
        .effective_dataset_role(user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let role = match effective_role {
        Some(r) => r,
        None => return Err((StatusCode::FORBIDDEN, "Access denied".to_string())),
    };
    // Private graphs are only visible to writers (owner / maintainer / admin).
    let can_see_private = role.can_write();

    // Return rich entries { graph_iri, graph_role, private, triple_count } so the UI can
    // show names, role badges, privacy state, and sizes (the bare string list was
    // unusable client-side).
    let entries = db
        .list_dataset_graph_entries(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let graphs: Vec<serde_json::Value> = entries
        .into_iter()
        .filter(|e| can_see_private || !e.private)
        .map(|e| {
            let triple_count = state.store.graph_count_cached(Some(&e.graph_iri));
            serde_json::json!({
                "graph_iri": e.graph_iri,
                "graph_role": e.graph_role.map(|r| r.as_str()),
                "private": e.private,
                "triple_count": triple_count,
            })
        })
        .collect();

    Ok(Json(graphs))
}

/// GET /api/datasets/:dataset_id/commits — provenance trail for writes touching
/// any graph registered to this dataset. Readable by anyone with dataset access.
pub async fn list_dataset_commits(
    user: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    Query(params): Query<crate::commit_log::CommitsParams>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    let user_id = user.as_ref().map(|u| u.user_id.as_str());
    if !db
        .can_access_dataset(user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    let graphs = db
        .list_dataset_graphs(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let scope = crate::commit_log::CommitScope::Graphs(graphs);
    let mut commits = crate::commit_log::list_commits(&state.store, &scope, &params.to_query());
    crate::commit_log::resolve_actors(db.as_ref(), &mut commits);
    Ok(Json(commits))
}

/// POST /api/datasets/:dataset_id/graphs
pub async fn add_dataset_graph(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    Json(req): Json<GraphIriRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    // Per-graph registration boundary. `can_write_dataset` only proves the caller
    // may write *into this dataset*; it does not constrain which graph IRI they
    // attach to it. Without this gate a writer could register another tenant's
    // private graph IRI to their own dataset and then read it, since
    // `get_accessible_graph_iris` makes any graph registered to an accessible
    // dataset readable (cross-tenant read — the IDOR the bulk-import path already
    // defends against). Admins are unrestricted.
    if !current_user.is_admin() {
        if let Err(msg) = crate::auth::dataset_graph::authorize_dataset_graph_target(
            &db,
            &state.base_url,
            &dataset_id,
            &req.graph_iri,
        ) {
            state.audit.log_denied(
                Some(current_user.user_id.clone()),
                None,
                "dataset_graph",
                &dataset_id,
                "register_graph",
                None,
            );
            return Err((StatusCode::FORBIDDEN, msg));
        }
    }

    db.add_dataset_graph(&dataset_id, &req.graph_iri)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Rewrite DCAT metadata graph to include the new void:subset entry.
    refresh_dataset_metadata(&state, &dataset_id);

    Ok(StatusCode::CREATED)
}

/// DELETE /api/datasets/:dataset_id/graphs
pub async fn remove_dataset_graph(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    Json(req): Json<GraphIriRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    state
        .auth_db
        .remove_dataset_graph(&dataset_id, &req.graph_iri)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // If no other dataset references this graph IRI, remove the Oxigraph graph too.
    // The DB row is already gone, so exclude_dataset_id="" — any hit means another dataset.
    let shared = state
        .auth_db
        .graph_has_other_dataset_refs(&req.graph_iri, "")
        .unwrap_or(true); // default to keeping the graph on error
    if !shared {
        let _ = state.store.graph_store_delete(Some(req.graph_iri.as_str()));
    }

    // Rewrite DCAT metadata graph to remove the void:subset entry.
    refresh_dataset_metadata(&state, &dataset_id);

    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /api/datasets/:dataset_id/graphs
/// Update the `graph_role` of an already-registered dataset graph.
pub async fn patch_dataset_graph_role(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    Json(req): Json<PatchDatasetGraphRoleRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    // A request carrying `private` is a privacy toggle and leaves the role
    // untouched; otherwise it is a role update (where a null role clears it).
    if let Some(private) = req.private {
        db.set_dataset_graph_private(&dataset_id, &req.graph_iri, private)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    } else {
        let graph_role = req.graph_role.as_deref().and_then(GraphKind::from_str);
        db.set_dataset_graph_role(&dataset_id, &req.graph_iri, graph_role)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // Rewrite DCAT metadata graph to reflect updated ots:graphRole triples.
        refresh_dataset_metadata(&state, &dataset_id);
    }

    Ok(StatusCode::NO_CONTENT)
}

// ─── SPARQL Service handlers ──────────────────────────────────────────────────

/// POST /api/datasets/:dataset_id/services
pub async fn create_service(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    Path(dataset_id): Path<String>,
    Json(req): Json<CreateServiceRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    // Idempotent: the default `sparql` service is auto-created with the dataset,
    // and clients may also request it — return the existing one instead of a
    // UNIQUE-constraint 500.
    if let Ok(Some(existing)) = db.get_sparql_service_by_slug(&dataset_id, &req.slug) {
        return Ok((StatusCode::OK, Json(existing)));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let service = db
        .create_sparql_service(
            &id,
            &dataset_id,
            &req.name,
            &req.slug,
            req.description.as_deref(),
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok((StatusCode::CREATED, Json(service)))
}

/// GET /api/datasets/:dataset_id/services
pub async fn list_services(
    user: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    let user_id = user.as_ref().map(|u| u.user_id.as_str());
    if !db
        .can_access_dataset(user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    let services = db
        .list_dataset_services(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(services))
}

/// GET /api/datasets/:dataset_id/services/:service_id
pub async fn get_service(
    user: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    Path((dataset_id, service_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    let user_id = user.as_ref().map(|u| u.user_id.as_str());
    if !db
        .can_access_dataset(user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    let service = db
        .get_sparql_service(&service_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Service not found".to_string()))?;

    Ok(Json(service))
}

/// PUT /api/datasets/:dataset_id/services/:service_id
pub async fn update_service(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    Path((dataset_id, service_id)): Path<(String, String)>,
    Json(req): Json<UpdateServiceRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    db.update_sparql_service(
        &service_id,
        &req.name,
        req.description.as_deref(),
        req.is_active,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let service = db
        .get_sparql_service(&service_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Service not found".to_string()))?;

    Ok(Json(service))
}

/// DELETE /api/datasets/:dataset_id/services/:service_id
pub async fn delete_service(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    Path((dataset_id, service_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    db.delete_sparql_service(&service_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/datasets/:dataset_id/services/:service_id/graphs
pub async fn add_service_graph(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    Path((dataset_id, service_id)): Path<(String, String)>,
    Json(req): Json<GraphIriRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    db.add_service_graph(&service_id, &req.graph_iri)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::CREATED)
}

/// DELETE /api/datasets/:dataset_id/services/:service_id/graphs
pub async fn remove_service_graph(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    Path((dataset_id, service_id)): Path<(String, String)>,
    Json(req): Json<GraphIriRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    db.remove_service_graph(&service_id, &req.graph_iri)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/datasets/:dataset_id/services/:service_id/graphs
pub async fn list_service_graphs(
    user: Option<Extension<AuthenticatedUser>>,
    State(db): State<Arc<AuthDb>>,
    Path((dataset_id, service_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    let user_id = user.as_ref().map(|u| u.user_id.as_str());
    if !db
        .can_access_dataset(user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    let graphs = db
        .list_service_graphs(&service_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(graphs))
}

// ─── User admin handlers (legacy, kept for backward compat) ──────────────────

/// GET /api/users/public — returns minimal public info (id, username, avatar_key).
/// No authentication required.
pub async fn list_public_users(
    State(db): State<Arc<AuthDb>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let users = db
        .list_users()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    #[derive(Serialize)]
    struct PublicUser {
        id: String,
        username: String,
        avatar_key: Option<String>,
    }

    let response: Vec<PublicUser> = users
        .into_iter()
        .filter(|u| u.is_active)
        .map(|u| PublicUser {
            id: u.id,
            username: u.username,
            avatar_key: u.avatar_key,
        })
        .collect();

    Ok(Json(response))
}

/// GET /api/users (admin only)
pub async fn list_users(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authz::require_admin(&current_user)?;

    let users = db
        .list_users()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<UserResponse> = users.into_iter().map(UserResponse::from).collect();
    Ok(Json(response))
}

/// GET /api/users/:user_id
pub async fn get_user(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(user_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if user_id != current_user.user_id && !current_user.is_admin() {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    let user = db
        .get_user_by_id(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    Ok(Json(UserResponse::from(user)))
}

/// DELETE /api/users/:user_id (admin only)
///
/// Hard-deletes an account. Previously this only checked `require_admin` and then
/// deleted unconditionally, which let an admin delete a super_admin (privilege-
/// hierarchy bypass), delete themselves, or remove the last super_admin and leave
/// the install unadministrable. It now enforces the same guards as the maintained
/// purge path plus a last-super-admin lockout check.
pub async fn delete_user(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(user_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authz::require_admin(&current_user)?;

    if user_id == current_user.user_id {
        return Err((
            StatusCode::BAD_REQUEST,
            "Cannot delete your own account here".to_string(),
        ));
    }

    let target = db
        .get_user_by_id(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    // Hierarchy: may only act on accounts strictly below your own tier.
    if !authz::can_administer_user(&current_user, &user_id, target.role) {
        return Err((
            StatusCode::FORBIDDEN,
            "Cannot delete user with equal or higher role".to_string(),
        ));
    }

    // Never allow the system to be left with zero super admins.
    if target.role == SystemRole::SuperAdmin
        && target.is_active
        && db.count_active_super_admins().unwrap_or(0) <= 1
    {
        return Err((
            StatusCode::CONFLICT,
            "Cannot delete the last active super admin".to_string(),
        ));
    }

    // Revoke tokens before removing the row so no session outlives the account.
    let _ = db.revoke_all_user_refresh_tokens(&user_id);
    let _ = db.revoke_all_user_api_tokens(&user_id);

    db.delete_user(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Dataset SHACL config handler ─────────────────────────────────────────────

/// PUT /api/datasets/:dataset_id/shacl
pub async fn update_dataset_shacl(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(dataset_id): Path<String>,
    Json(req): Json<DatasetShaclRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    db.update_dataset_shacl(
        &dataset_id,
        req.shacl_on_write,
        req.shapes_graph_iri.as_deref(),
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Dataset private access handlers ──────────────────────────────────────────

/// PUT /api/datasets/:dataset_id/role
///
/// Retags both the dataset row and every one of its `dataset_graphs` rows with
/// the new role. When the role is `model` or `vocabulary`, also creates the
/// matching record in the model / vocabulary registry (if one does not already
/// exist for that dataset id) so the dataset surfaces in those listings.
pub async fn update_dataset_role(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let db = &state.auth_db;
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    let graph_role = req["graph_role"].as_str().and_then(GraphKind::from_str);

    db.update_dataset_role(&dataset_id, graph_role)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    db.update_dataset_graphs_role(&dataset_id, graph_role)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Auto-register the dataset in the model / vocabulary registry so it shows
    // up in those listings. Idempotent: skip if a record with this id already exists.
    if let Some(role) = graph_role {
        let role_str = role.as_str();
        if role_str == "model" || role_str == "vocabulary" {
            let registry_id = slugify(&dataset.name);
            if !registry_id.is_empty() {
                let now = chrono::Utc::now().to_rfc3339();
                let owner_type = match dataset.owner_type {
                    OwnerType::User => "user",
                    OwnerType::Organisation => "organisation",
                    OwnerType::Group => "group",
                };
                let owner_id = dataset.owner_id.as_str();
                let creator = format!("{}/users/{}", state.base_url, current_user.user_id);
                let title = if dataset.name.is_empty() {
                    dataset_id.as_str()
                } else {
                    dataset.name.as_str()
                };
                let description = dataset.description.as_deref();

                if role_str == "model" {
                    if !crate::data_models::registry::data_model_exists(
                        &state.store,
                        &state.base_url,
                        &registry_id,
                    ) {
                        if let Err(e) = crate::data_models::registry::insert_data_model(
                            &state.store,
                            &state.base_url,
                            &registry_id,
                            title,
                            "", // namespace unknown — user can edit later
                            description,
                            false, // is_public — keep private until user opts in
                            Some(owner_type),
                            Some(owner_id),
                            Some(&creator),
                            &now,
                        ) {
                            tracing::warn!(
                                "failed to auto-register data model for dataset {dataset_id}: {e}"
                            );
                        }
                    }
                } else {
                    // role_str == "vocabulary" — same unified registry, kind=vocabulary.
                    if !crate::data_models::registry::data_model_exists(
                        &state.store,
                        &state.base_url,
                        &registry_id,
                    ) {
                        if let Err(e) = crate::data_models::registry::insert_data_model(
                            &state.store,
                            &state.base_url,
                            &registry_id,
                            title,
                            "",
                            description,
                            false,
                            Some(owner_type),
                            Some(owner_id),
                            Some(&creator),
                            &now,
                        ) {
                            tracing::warn!(
                                "failed to auto-register vocabulary for dataset {dataset_id}: {e}"
                            );
                        } else if let Err(e) = crate::data_models::registry::set_data_model_kind(
                            &state.store,
                            &state.base_url,
                            &registry_id,
                            crate::kind_detector::RegistryKind::Vocabulary,
                        ) {
                            tracing::warn!(
                                "failed to set vocabulary kind for dataset {dataset_id}: {e}"
                            );
                        }
                    }
                }
            }
        }
    }

    // Rewrite DCAT metadata graph to reflect the new ots:graphRole classification.
    refresh_dataset_metadata(&state, &dataset_id);

    Ok(StatusCode::NO_CONTENT)
}

/// URL-safe slug derived from a free-form title — same shape that
/// `create_data_model` mints when the user creates an entry from scratch, so
/// the auto-registered id collides with an existing one
/// rather than producing a duplicate when the user already created it manually.
fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Mint a human-readable, **globally unique** id for a new dataset from its name.
///
/// The dataset id is both the SQLite primary key and the local name of the
/// dataset's IRI (`{base}/dataset/{id}`, see the IRI scheme in
/// `docs/linked-data-modelling-styleguide.md` §3.3). Using a slug instead of a
/// UUID makes that IRI semantically understandable — the same slug scheme the
/// data-model and vocabulary registries already use. Empty slugs (a name with
/// no alphanumerics) fall back to `"dataset"`, and `-2`, `-3`, … suffixes are
/// appended on collision so the id stays unique without leaking owner/year/etc.
/// into the local name.
pub(crate) fn unique_dataset_slug(db: &AuthDb, name: &str) -> String {
    let base = {
        let s = slugify(name);
        if s.is_empty() {
            "dataset".to_string()
        } else {
            s
        }
    };
    if db.get_dataset(&base).ok().flatten().is_none() {
        return base;
    }
    for n in 2..10_000 {
        let candidate = format!("{base}-{n}");
        if db.get_dataset(&candidate).ok().flatten().is_none() {
            return candidate;
        }
    }
    // Pathological fallback (10k same-named datasets): keep it unique and human-ish.
    format!("{base}-{}", uuid::Uuid::new_v4())
}

/// POST /api/datasets/:dataset_id/access
pub async fn grant_access(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(dataset_id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    // Managing who may access a dataset is an owner/admin capability, not an
    // editor one — a mere editor (write grant) must not be able to grant access
    // to arbitrary users. Mirrors the role-based grant endpoints below.
    if !db
        .can_manage_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Manage access required".to_string()));
    }

    let user_id = req["user_id"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing user_id".to_string()))?;

    db.grant_dataset_access(&dataset_id, user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::CREATED)
}

/// DELETE /api/datasets/:dataset_id/access/:user_id
pub async fn revoke_access(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path((dataset_id, user_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    // Revoking access is an owner/admin (manage) capability, not an editor one.
    if !db
        .can_manage_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Manage access required".to_string()));
    }

    db.revoke_dataset_access(&dataset_id, &user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/datasets/:dataset_id/access
pub async fn list_access(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    // Enumerating who has access is an owner/admin (manage) capability — an
    // editor must not be able to read the access list.
    if !db
        .can_manage_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Manage access required".to_string()));
    }

    let users = db
        .list_dataset_access_users(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<UserResponse> = users.into_iter().map(UserResponse::from).collect();
    Ok(Json(response))
}

// ─── Role-based per-resource grants ───────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetResourceGrantRequest {
    /// "user" | "group" | "organisation"
    pub principal_type: String,
    pub principal_id: String,
    /// "viewer" | "editor" | "admin"
    pub role: String,
}

/// Validate the principal type / role and confirm the principal exists.
fn validate_grant_principal(
    db: &AuthDb,
    principal_type: &str,
    principal_id: &str,
    role: &str,
) -> Result<ResourceRole, (StatusCode, String)> {
    let role = ResourceRole::from_str(role).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Invalid role (expected viewer | editor | admin)".to_string(),
        )
    })?;
    match principal_type {
        "user" => {
            let exists = db
                .get_user_by_id(principal_id)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .is_some();
            if !exists {
                return Err((StatusCode::BAD_REQUEST, "Unknown user".to_string()));
            }
        }
        "group" => {
            let exists = db
                .get_group(principal_id)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .is_some();
            if !exists {
                return Err((StatusCode::BAD_REQUEST, "Unknown group".to_string()));
            }
        }
        "organisation" => {
            let exists = db
                .get_organisation(principal_id)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .is_some();
            if !exists {
                return Err((StatusCode::BAD_REQUEST, "Unknown organisation".to_string()));
            }
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "principal_type must be 'user', 'group', or 'organisation'".to_string(),
            ))
        }
    }
    Ok(role)
}

/// GET /api/datasets/:dataset_id/grants — list role grants (manage required)
pub async fn list_dataset_grants(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !db
        .can_manage_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Manage access required".to_string()));
    }
    let grants = db
        .list_resource_grants("dataset", &dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(grants))
}

/// PUT /api/datasets/:dataset_id/grants — create or update a role grant
pub async fn set_dataset_grant(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path(dataset_id): Path<String>,
    Json(req): Json<SetResourceGrantRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !db
        .can_manage_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Manage access required".to_string()));
    }
    let role = validate_grant_principal(&db, &req.principal_type, &req.principal_id, &req.role)?;
    let grant = db
        .set_resource_grant(
            "dataset",
            &dataset_id,
            &req.principal_type,
            &req.principal_id,
            role,
            &current_user.user_id,
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok((StatusCode::OK, Json(grant)))
}

/// DELETE /api/datasets/:dataset_id/grants/:principal_type/:principal_id
pub async fn revoke_dataset_grant(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    Path((dataset_id, principal_type, principal_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !db
        .can_manage_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Manage access required".to_string()));
    }
    db.revoke_resource_grant("dataset", &dataset_id, &principal_type, &principal_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── Image upload/download handlers ──────────────────────────────────────────

/// PUT /api/users/me/avatar — upload avatar for the authenticated user
/// Allowed image MIME types for avatars / profile / banner images. Deliberately a
/// raster allow-list: `image/svg+xml` (and text/html, *+xml) are rejected because
/// SVG can carry active `<script>` and these images are served back — some
/// publicly (e.g. `/api/users/:id/avatar`) — so an inline-rendered SVG would be
/// stored XSS in the viewer's origin. With the global `X-Content-Type-Options:
/// nosniff` header this closes the vector.
fn is_allowed_image_type(content_type: &str) -> bool {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        ct.as_str(),
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp"
    )
}

pub async fn upload_user_avatar(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    {
        let content_type = field.content_type().unwrap_or("image/jpeg").to_string();
        if !is_allowed_image_type(&content_type) {
            return Err((
                StatusCode::BAD_REQUEST,
                "Only PNG, JPEG, GIF, or WebP images are allowed".to_string(),
            ));
        }
        let data = field
            .bytes()
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        if data.len() > 5 * 1024 * 1024 {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                "Image must be under 5 MB".to_string(),
            ));
        }
        let ext = content_type.split('/').nth(1).unwrap_or("jpg");
        let key = format!("avatars/{}.{}", current_user.user_id, ext);
        state
            .object_store
            .upload(&key, data, &content_type)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        state
            .auth_db
            .update_user_avatar(&current_user.user_id, Some(&key))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(Json(serde_json::json!({ "avatar_key": key })));
    }
    Err((StatusCode::BAD_REQUEST, "No file provided".to_string()))
}

/// GET /api/users/:user_id/avatar — download user avatar
pub async fn get_user_avatar(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let user = state
        .auth_db
        .get_user_by_id(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;
    let key = user
        .avatar_key
        .ok_or_else(|| (StatusCode::NOT_FOUND, "No avatar set".to_string()))?;
    let (data, content_type) = state
        .object_store
        .download(&key)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Avatar not found".to_string()))?;
    Ok((StatusCode::OK, [(CONTENT_TYPE, content_type)], data).into_response())
}

/// PUT /api/organisations/:org_id/image — upload org image
pub async fn upload_org_image(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    // Must be system admin or org admin
    if !current_user.is_admin() {
        match state
            .auth_db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }
    if let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    {
        let content_type = field.content_type().unwrap_or("image/jpeg").to_string();
        if !is_allowed_image_type(&content_type) {
            return Err((
                StatusCode::BAD_REQUEST,
                "Only PNG, JPEG, GIF, or WebP images are allowed".to_string(),
            ));
        }
        let data = field
            .bytes()
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        if data.len() > 5 * 1024 * 1024 {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                "Image must be under 5 MB".to_string(),
            ));
        }
        let ext = content_type.split('/').nth(1).unwrap_or("jpg");
        let key = format!("org-images/{}.{}", org_id, ext);
        state
            .object_store
            .upload(&key, data, &content_type)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        state
            .auth_db
            .update_org_image(&org_id, Some(&key))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(Json(serde_json::json!({ "image_key": key })));
    }
    Err((StatusCode::BAD_REQUEST, "No file provided".to_string()))
}

/// GET /api/organisations/:org_id/image — download org image
pub async fn get_org_image(
    State(state): State<AppState>,
    Path(org_id): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let org = state
        .auth_db
        .get_organisation(&org_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Organisation not found".to_string()))?;
    let key = org
        .image_key
        .ok_or_else(|| (StatusCode::NOT_FOUND, "No image set".to_string()))?;
    let (data, content_type) = state
        .object_store
        .download(&key)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Image not found".to_string()))?;
    Ok((StatusCode::OK, [(CONTENT_TYPE, content_type)], data).into_response())
}

/// PUT /api/datasets/:dataset_id/image — upload dataset cover image
pub async fn upload_dataset_image(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    if let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    {
        let content_type = field.content_type().unwrap_or("image/jpeg").to_string();
        if !is_allowed_image_type(&content_type) {
            return Err((
                StatusCode::BAD_REQUEST,
                "Only PNG, JPEG, GIF, or WebP images are allowed".to_string(),
            ));
        }
        let data = field
            .bytes()
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        if data.len() > 5 * 1024 * 1024 {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                "Image must be under 5 MB".to_string(),
            ));
        }
        let ext = content_type.split('/').nth(1).unwrap_or("jpg");
        let key = format!("dataset-images/{}.{}", dataset_id, ext);
        state
            .object_store
            .upload(&key, data, &content_type)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        state
            .auth_db
            .update_dataset_image(&dataset_id, Some(&key))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(Json(serde_json::json!({ "image_key": key })));
    }
    Err((StatusCode::BAD_REQUEST, "No file provided".to_string()))
}

/// GET /api/datasets/:dataset_id/image — download dataset cover image
pub async fn get_dataset_image(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    let user_id = user.as_ref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_dataset(user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }
    let key = dataset
        .image_key
        .ok_or_else(|| (StatusCode::NOT_FOUND, "No image set".to_string()))?;
    let (data, content_type) = state
        .object_store
        .download(&key)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Image not found".to_string()))?;
    Ok((StatusCode::OK, [(CONTENT_TYPE, content_type)], data).into_response())
}

/// PUT /api/organisations/:org_id/banner — upload org banner image
pub async fn upload_org_banner(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    // Must be system admin or org admin
    if !current_user.is_admin() {
        match state
            .auth_db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }
    if let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    {
        let content_type = field.content_type().unwrap_or("image/jpeg").to_string();
        if !is_allowed_image_type(&content_type) {
            return Err((
                StatusCode::BAD_REQUEST,
                "Only PNG, JPEG, GIF, or WebP images are allowed".to_string(),
            ));
        }
        let data = field
            .bytes()
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        if data.len() > 5 * 1024 * 1024 {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                "Image must be under 5 MB".to_string(),
            ));
        }
        let ext = content_type.split('/').nth(1).unwrap_or("jpg");
        let key = format!("org-banners/{}.{}", org_id, ext);
        state
            .object_store
            .upload(&key, data, &content_type)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        state
            .auth_db
            .update_org_banner(&org_id, Some(&key))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(Json(serde_json::json!({ "banner_key": key })));
    }
    Err((StatusCode::BAD_REQUEST, "No file provided".to_string()))
}

/// GET /api/organisations/:org_id/banner — download org banner image
pub async fn get_org_banner(
    State(state): State<AppState>,
    Path(org_id): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let org = state
        .auth_db
        .get_organisation(&org_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Organisation not found".to_string()))?;
    let key = org
        .banner_key
        .ok_or_else(|| (StatusCode::NOT_FOUND, "No banner set".to_string()))?;
    let (data, content_type) = state
        .object_store
        .download(&key)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Banner not found".to_string()))?;
    Ok((StatusCode::OK, [(CONTENT_TYPE, content_type)], data).into_response())
}

/// PUT /api/datasets/:dataset_id/banner — upload dataset banner image
pub async fn upload_dataset_banner(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    if let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    {
        let content_type = field.content_type().unwrap_or("image/jpeg").to_string();
        if !is_allowed_image_type(&content_type) {
            return Err((
                StatusCode::BAD_REQUEST,
                "Only PNG, JPEG, GIF, or WebP images are allowed".to_string(),
            ));
        }
        let data = field
            .bytes()
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        if data.len() > 5 * 1024 * 1024 {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                "Image must be under 5 MB".to_string(),
            ));
        }
        let ext = content_type.split('/').nth(1).unwrap_or("jpg");
        let key = format!("dataset-banners/{}.{}", dataset_id, ext);
        state
            .object_store
            .upload(&key, data, &content_type)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        state
            .auth_db
            .update_dataset_banner(&dataset_id, Some(&key))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(Json(serde_json::json!({ "banner_key": key })));
    }
    Err((StatusCode::BAD_REQUEST, "No file provided".to_string()))
}

/// GET /api/datasets/:dataset_id/banner — download dataset banner image
pub async fn get_dataset_banner(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    let user_id = user.as_ref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_dataset(user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }
    let key = dataset
        .banner_key
        .ok_or_else(|| (StatusCode::NOT_FOUND, "No banner set".to_string()))?;
    let (data, content_type) = state
        .object_store
        .download(&key)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Banner not found".to_string()))?;
    Ok((StatusCode::OK, [(CONTENT_TYPE, content_type)], data).into_response())
}

/// Request body for the banner-preset endpoints: a preset id to apply, or
/// null/empty to clear the banner.
#[derive(Deserialize)]
pub struct BannerPresetBody {
    #[serde(default)]
    pub preset: Option<String>,
}

/// Banner preset ids are short url-safe slugs (`[a-z0-9-]`). The frontend
/// `banners.ts` registry is the source of truth for which ids actually render;
/// here we only guard the shape so the stored `preset:<id>` sentinel can never
/// carry arbitrary text.
fn is_valid_banner_preset_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Resolve a request body into the `banner_key` to store: `Some("preset:<id>")`
/// for a valid id, or `None` to clear. Rejects malformed ids.
fn resolve_banner_preset_key(
    body: &BannerPresetBody,
) -> Result<Option<String>, (StatusCode, String)> {
    match body
        .preset
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(id) => {
            if !is_valid_banner_preset_id(id) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Invalid banner preset id".to_string(),
                ));
            }
            Ok(Some(format!("preset:{id}")))
        }
        None => Ok(None),
    }
}

/// PUT /api/datasets/:dataset_id/banner-preset — select a built-in animated
/// banner preset (stored as `banner_key = "preset:<id>"`) or clear it. Mirrors
/// the write-access check of `upload_dataset_banner`.
pub async fn set_dataset_banner_preset(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    Json(body): Json<BannerPresetBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    let new_key = resolve_banner_preset_key(&body)?;
    state
        .auth_db
        .update_dataset_banner(&dataset_id, new_key.as_deref())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "banner_key": new_key })))
}

/// PUT /api/organisations/:org_id/banner-preset — same as above for an
/// organisation. Mirrors the admin/org-admin check of `upload_org_banner`.
pub async fn set_org_banner_preset(
    user_opt: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    Json(body): Json<BannerPresetBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let current_user = require_user(user_opt)?;
    if !current_user.is_admin() {
        match state
            .auth_db
            .get_org_membership(&current_user.user_id, &org_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            Some(Role::Admin) => {}
            _ => return Err((StatusCode::FORBIDDEN, "Admin access required".to_string())),
        }
    }
    let new_key = resolve_banner_preset_key(&body)?;
    state
        .auth_db
        .update_org_banner(&org_id, new_key.as_deref())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "banner_key": new_key })))
}

#[cfg(test)]
mod banner_preset_tests {
    use super::{is_valid_banner_preset_id, resolve_banner_preset_key, BannerPresetBody};

    #[test]
    fn accepts_slug_ids_and_rejects_junk() {
        for ok in ["aurora-teal", "gradient-dusk", "a", "x0123456789"] {
            assert!(is_valid_banner_preset_id(ok), "{ok} should be valid");
        }
        let too_long = "z".repeat(41);
        for bad in [
            "",
            "Aurora",
            "has space",
            "semi;colon",
            "preset:teal",
            "../x",
            too_long.as_str(),
        ] {
            assert!(!is_valid_banner_preset_id(bad), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn resolve_maps_to_sentinel_or_clears() {
        let apply = BannerPresetBody {
            preset: Some("aurora-rose".into()),
        };
        assert_eq!(
            resolve_banner_preset_key(&apply).unwrap(),
            Some("preset:aurora-rose".to_string())
        );
        // trims whitespace
        let padded = BannerPresetBody {
            preset: Some("  aurora-teal  ".into()),
        };
        assert_eq!(
            resolve_banner_preset_key(&padded).unwrap(),
            Some("preset:aurora-teal".to_string())
        );
        // null / empty clears
        for clear in [None, Some(String::new()), Some("   ".into())] {
            let body = BannerPresetBody { preset: clear };
            assert_eq!(resolve_banner_preset_key(&body).unwrap(), None);
        }
        // malformed → 400
        let bad = BannerPresetBody {
            preset: Some("Bad Id".into()),
        };
        assert!(resolve_banner_preset_key(&bad).is_err());
    }
}

#[cfg(test)]
mod metadata_url_security_tests {
    use super::{validate_metadata_url, StatusCode};

    #[test]
    fn allows_safe_or_empty_urls() {
        for v in [
            None,
            Some(""),
            Some("   "),
            Some("https://example.org/landing"),
            Some("http://example.org"),
            Some("mailto:admin@example.org"),
        ] {
            assert!(
                validate_metadata_url("field", v).is_ok(),
                "should allow {v:?}"
            );
        }
    }

    #[test]
    fn rejects_dangerous_schemes_stored_xss() {
        // Each of these would otherwise round-trip into an <a href> on a public page.
        for v in [
            "javascript:alert(document.cookie)",
            "JavaScript:alert(1)", // scheme is case-insensitive
            "data:text/html,<script>alert(1)</script>",
            "file:///etc/passwd",
            "vbscript:msgbox(1)",
        ] {
            let err = validate_metadata_url("homepage", Some(v))
                .expect_err(&format!("should reject {v}"));
            assert_eq!(err.0, StatusCode::BAD_REQUEST);
        }
    }
}
