//! WebAuthn / FIDO2 passkey support.
//!
//! Built on [`webauthn_rs`]. Registration is an authenticated two-step flow
//! (`register/start` → browser `navigator.credentials.create()` →
//! `register/finish`); login is a public discoverable-credential flow
//! (`login/start` → `navigator.credentials.get()` → `login/finish`) that
//! issues the same session tokens and cookies as a password login.
//!
//! In-progress challenge state lives in an in-memory [`DashMap`] (mirroring
//! the OAuth PKCE session store) keyed by an opaque `challenge_id`, consumed
//! exactly once by the matching finish call. Only finished credentials are
//! persisted, in the `webauthn_credentials` table.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::Engine as _;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, CredentialID, DiscoverableAuthentication, DiscoverableKey, Passkey,
    PasskeyRegistration, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse, Webauthn, WebauthnBuilder,
};

use super::audit::{self, AuditEventBuilder, AuditEventType, AuditLogger, AuditOutcome};
use super::db::AuthDb;
use super::handlers::{
    auth_cookie_headers, issue_tokens, require_verified_email, AuthResponse, MfaRequiredResponse,
    UserResponse,
};
use super::jwt::{self, JwtConfig};
use super::middleware::AuthenticatedUser;
use super::password;
use crate::server::{AppState, CookieConfig};

/// Relying-party display name shown by authenticator UIs.
const RP_NAME: &str = "Open Triplestore";

/// How long a started registration/login may take before the challenge expires.
const CHALLENGE_TTL_SECS: u64 = 300;

/// Hard cap on concurrent in-flight challenges, bounding memory if an attacker
/// floods `login/start` within the TTL window (the route is also rate-limited).
const MAX_PASSKEY_SESSIONS: usize = 10_000;

/// Per-account passkey cap — purely a sanity bound for the settings UI.
const MAX_PASSKEYS_PER_USER: usize = 10;

const GENERIC_LOGIN_ERROR: &str = "Invalid credentials";
const GENERIC_CHALLENGE_ERROR: &str = "Unknown or expired passkey challenge";

// ─── In-memory challenge store ────────────────────────────────────────────────

/// One in-flight WebAuthn ceremony. The server-side state types are kept in
/// memory on purpose: they must never travel to the client, and serialising
/// them would require webauthn-rs' danger-allow-state-serialisation feature.
pub enum PasskeyChallenge {
    Registration {
        user_id: String,
        reg_state: Box<PasskeyRegistration>,
        created_at: Instant,
    },
    Authentication {
        auth_state: Box<DiscoverableAuthentication>,
        created_at: Instant,
    },
}

impl PasskeyChallenge {
    fn created_at(&self) -> Instant {
        match self {
            Self::Registration { created_at, .. } | Self::Authentication { created_at, .. } => {
                *created_at
            }
        }
    }
}

/// Thread-safe in-memory store for in-flight passkey ceremonies, keyed by the
/// opaque `challenge_id` handed to the browser.
pub type PasskeySessions = Arc<DashMap<String, PasskeyChallenge>>;

pub fn new_session_store() -> PasskeySessions {
    Arc::new(DashMap::new())
}

/// Remove expired challenges, then enforce the hard size cap by evicting the
/// oldest entries beyond [`MAX_PASSKEY_SESSIONS`].
pub fn prune_sessions(sessions: &PasskeySessions) {
    let ttl = Duration::from_secs(CHALLENGE_TTL_SECS);
    sessions.retain(|_, v| v.created_at().elapsed() < ttl);

    if sessions.len() > MAX_PASSKEY_SESSIONS {
        let mut entries: Vec<(String, Instant)> = sessions
            .iter()
            .map(|e| (e.key().clone(), e.value().created_at()))
            .collect();
        entries.sort_by_key(|(_, t)| *t); // oldest first
        let to_remove = entries.len().saturating_sub(MAX_PASSKEY_SESSIONS);
        for (k, _) in entries.into_iter().take(to_remove) {
            sessions.remove(&k);
        }
    }
}

// ─── WebAuthn relying-party construction ──────────────────────────────────────

/// Build the [`Webauthn`] relying party from the deployment's public base URL.
/// The RP id is the bare host; the origin must match what the browser shows.
/// For localhost the port check is relaxed so the Vite dev server (different
/// port, same host) can complete ceremonies against the API origin.
fn build_webauthn(base_url: &str) -> Result<Webauthn, (StatusCode, String)> {
    let origin = url::Url::parse(base_url).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Invalid PUBLIC_BASE_URL for WebAuthn: {e}"),
        )
    })?;
    let rp_id = origin.host_str().map(str::to_string).ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "PUBLIC_BASE_URL has no host — cannot derive WebAuthn RP id".to_string(),
        )
    })?;
    let mut builder = WebauthnBuilder::new(&rp_id, &origin)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("WebAuthn configuration error: {e}"),
            )
        })?
        .rp_name(RP_NAME);
    if rp_id == "localhost" || rp_id == "127.0.0.1" {
        builder = builder.allow_any_port(true);
    }
    builder.build().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("WebAuthn configuration error: {e}"),
        )
    })
}

/// Base64url (no padding) — the canonical text form of a credential ID.
fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// webauthn-rs wants a UUID per user; account ids are free-form strings, so
/// derive a stable v5 UUID from the id. One-way: never mapped back.
fn stable_user_uuid(user_id: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, user_id.as_bytes())
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

// ─── Request / response types ─────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct RegisterStartResponse {
    /// Opaque handle pairing this challenge with the finish call.
    pub challenge_id: String,
    /// WebAuthn creation options: pass the nested `publicKey` member to
    /// `navigator.credentials.create()` (binary fields are base64url strings).
    #[schema(value_type = Object)]
    pub options: CreationChallengeResponse,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterFinishRequest {
    pub challenge_id: String,
    /// Label for the new passkey (defaults to "Passkey").
    pub name: Option<String>,
    /// JSON-encoded result of `navigator.credentials.create()`.
    #[schema(value_type = Object)]
    pub credential: RegisterPublicKeyCredential,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoginStartResponse {
    pub challenge_id: String,
    /// WebAuthn request options: pass the nested `publicKey` member to
    /// `navigator.credentials.get()`.
    #[schema(value_type = Object)]
    pub options: RequestChallengeResponse,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginFinishRequest {
    pub challenge_id: String,
    /// JSON-encoded result of `navigator.credentials.get()`.
    #[schema(value_type = Object)]
    pub credential: PublicKeyCredential,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DeletePasskeyRequest {
    /// Current account password — a hijacked session must not be able to
    /// remove credentials silently.
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PasskeySummary {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    /// Authenticator transports reported at registration (`usb`, `internal`…).
    pub transports: Vec<String>,
}

impl From<crate::auth::models::WebauthnCredential> for PasskeySummary {
    fn from(c: crate::auth::models::WebauthnCredential) -> Self {
        let transports = c
            .transports
            .as_deref()
            .and_then(|t| serde_json::from_str::<Vec<String>>(t).ok())
            .unwrap_or_default();
        PasskeySummary {
            id: c.id,
            name: c.name,
            created_at: c.created_at,
            last_used_at: c.last_used_at,
            transports,
        }
    }
}

// ─── Registration (authenticated) ─────────────────────────────────────────────

/// POST /api/auth/passkeys/register/start — mint a WebAuthn creation challenge
/// for the signed-in user. Existing credentials are excluded so an
/// authenticator can't enroll twice.
pub async fn register_start(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let user = db
        .get_user_by_id(&current_user.user_id)
        .map_err(internal)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    let existing = db.list_webauthn_credentials(&user.id).map_err(internal)?;
    if existing.len() >= MAX_PASSKEYS_PER_USER {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("At most {MAX_PASSKEYS_PER_USER} passkeys per account — remove one first"),
        ));
    }
    let exclude: Vec<CredentialID> = existing
        .iter()
        .filter_map(|c| serde_json::from_str::<Passkey>(&c.public_key).ok())
        .map(|pk| pk.cred_id().clone())
        .collect();

    let webauthn = build_webauthn(&state.base_url)?;
    let display_name = user
        .display_name
        .clone()
        .unwrap_or_else(|| user.username.clone());
    let (options, reg_state) = webauthn
        .start_passkey_registration(
            stable_user_uuid(&user.id),
            &user.username,
            &display_name,
            Some(exclude),
        )
        .map_err(internal)?;

    prune_sessions(&state.passkey_sessions);
    let challenge_id = Uuid::new_v4().to_string();
    state.passkey_sessions.insert(
        challenge_id.clone(),
        PasskeyChallenge::Registration {
            user_id: user.id.clone(),
            reg_state: Box::new(reg_state),
            created_at: Instant::now(),
        },
    );

    Ok(Json(RegisterStartResponse {
        challenge_id,
        options,
    }))
}

/// POST /api/auth/passkeys/register/finish — verify the authenticator's
/// attestation and store the new credential.
pub async fn register_finish(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(audit_log): State<Arc<AuditLogger>>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterFinishRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let invalid = || (StatusCode::BAD_REQUEST, GENERIC_CHALLENGE_ERROR.to_string());

    // Consume the challenge — it must never verify twice.
    let (_, challenge) = state
        .passkey_sessions
        .remove(&req.challenge_id)
        .ok_or_else(invalid)?;
    let PasskeyChallenge::Registration {
        user_id,
        reg_state,
        created_at,
    } = challenge
    else {
        return Err(invalid());
    };
    if created_at.elapsed() > Duration::from_secs(CHALLENGE_TTL_SECS)
        || user_id != current_user.user_id
    {
        return Err(invalid());
    }

    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Passkey");
    if name.len() > 100 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Passkey name must be at most 100 characters".to_string(),
        ));
    }

    let user = db
        .get_user_by_id(&user_id)
        .map_err(internal)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    let webauthn = build_webauthn(&state.base_url)?;
    let passkey = webauthn
        .finish_passkey_registration(&req.credential, &reg_state)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Passkey registration failed: {e}"),
            )
        })?;

    // A credential ID is globally unique to one authenticator key pair: if it
    // is already bound to an account, a second registration must not succeed.
    let cred_id_b64 = b64url(passkey.cred_id().as_ref());
    if db
        .get_webauthn_credential_by_cred_id(&cred_id_b64)
        .map_err(internal)?
        .is_some()
    {
        return Err((
            StatusCode::CONFLICT,
            "This passkey is already registered".to_string(),
        ));
    }

    let public_key = serde_json::to_string(&passkey).map_err(internal)?;
    let transports = req
        .credential
        .response
        .transports
        .as_ref()
        .and_then(|t| serde_json::to_string(t).ok());
    let id = Uuid::new_v4().to_string();
    db.create_webauthn_credential(
        &id,
        &user.id,
        &cred_id_b64,
        &public_key,
        0,
        transports.as_deref(),
        name,
    )
    .map_err(internal)?;

    {
        let mut b =
            AuditEventBuilder::new(AuditEventType::PasskeyRegistered, AuditOutcome::Success)
                .actor(&user.id, &user.username, user.role.as_str())
                .resource("passkey", &id)
                .details(serde_json::json!({ "name": name }));
        b.ip_address = audit::client_ip(&headers, None);
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        audit_log.log(b);
    }

    let row = db
        .get_webauthn_credential_by_cred_id(&cred_id_b64)
        .map_err(internal)?
        .ok_or_else(|| internal("passkey row vanished after insert"))?;
    Ok((StatusCode::CREATED, Json(PasskeySummary::from(row))))
}

// ─── Listing / removal (authenticated) ────────────────────────────────────────

/// GET /api/auth/passkeys — the signed-in user's registered passkeys.
pub async fn list_passkeys(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let creds = db
        .list_webauthn_credentials(&current_user.user_id)
        .map_err(internal)?;
    let out: Vec<PasskeySummary> = creds.into_iter().map(PasskeySummary::from).collect();
    Ok(Json(out))
}

/// DELETE /api/auth/passkeys/:credential_id — remove one passkey. Requires the
/// current password so a hijacked session can't strip credentials.
pub async fn delete_passkey(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(db): State<Arc<AuthDb>>,
    State(audit_log): State<Arc<AuditLogger>>,
    headers: HeaderMap,
    Path(credential_id): Path<String>,
    Json(req): Json<DeletePasskeyRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let user = db
        .get_user_by_id(&current_user.user_id)
        .map_err(internal)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    let valid = password::verify_password(&req.password, &user.password_hash).map_err(internal)?;
    if !valid {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Current password is incorrect".to_string(),
        ));
    }

    let removed = db
        .delete_webauthn_credential(&credential_id, &user.id)
        .map_err(internal)?;
    if !removed {
        return Err((StatusCode::NOT_FOUND, "Passkey not found".to_string()));
    }

    {
        let mut b = AuditEventBuilder::new(AuditEventType::PasskeyRemoved, AuditOutcome::Success)
            .actor(&user.id, &user.username, user.role.as_str())
            .resource("passkey", &credential_id);
        b.ip_address = audit::client_ip(&headers, None);
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        audit_log.log(b);
    }

    Ok(StatusCode::NO_CONTENT)
}

// ─── Login (public, rate-limited) ─────────────────────────────────────────────

/// POST /api/auth/passkeys/login/start — mint a discoverable-credential
/// challenge. No username required: the authenticator picks the credential.
pub async fn login_start(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let webauthn = build_webauthn(&state.base_url)?;
    let (options, auth_state) = webauthn
        .start_discoverable_authentication()
        .map_err(internal)?;

    prune_sessions(&state.passkey_sessions);
    let challenge_id = Uuid::new_v4().to_string();
    state.passkey_sessions.insert(
        challenge_id.clone(),
        PasskeyChallenge::Authentication {
            auth_state: Box::new(auth_state),
            created_at: Instant::now(),
        },
    );

    Ok(Json(LoginStartResponse {
        challenge_id,
        options,
    }))
}

/// POST /api/auth/passkeys/login/finish — verify the assertion and issue the
/// same session tokens/cookies as a password login.
///
/// Every failure path returns one generic message (mirroring `login`) so the
/// endpoint can't be used to probe which credentials or accounts exist.
pub async fn login_finish(
    State(db): State<Arc<AuthDb>>,
    State(jwt_config): State<Arc<JwtConfig>>,
    State(audit_log): State<Arc<AuditLogger>>,
    State(cookie_config): State<CookieConfig>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginFinishRequest>,
) -> Result<Response, (StatusCode, String)> {
    let ip = audit::client_ip(&headers, None);
    let ua = audit::user_agent(&headers);
    let req_id = audit::request_id_from_headers(&headers);
    let unauthorized = || (StatusCode::UNAUTHORIZED, GENERIC_LOGIN_ERROR.to_string());

    let log_failure = |reason: &str, user: Option<(&str, &str)>| {
        let mut b = AuditEventBuilder::new(AuditEventType::LoginFailure, AuditOutcome::Failure)
            .details(serde_json::json!({ "reason": reason, "method": "passkey" }));
        if let Some((id, username)) = user {
            b.actor_id = Some(id.to_string());
            b.actor_username = Some(username.to_string());
        }
        b.ip_address = ip.clone();
        b.user_agent = ua.clone();
        b.request_id = req_id.clone();
        audit_log.log(b);
    };

    // Consume the challenge — an assertion must never verify twice.
    let Some((_, challenge)) = state.passkey_sessions.remove(&req.challenge_id) else {
        log_failure("unknown_challenge", None);
        return Err(unauthorized());
    };
    let PasskeyChallenge::Authentication {
        auth_state,
        created_at,
    } = challenge
    else {
        log_failure("challenge_type_mismatch", None);
        return Err(unauthorized());
    };
    if created_at.elapsed() > Duration::from_secs(CHALLENGE_TTL_SECS) {
        log_failure("challenge_expired", None);
        return Err(unauthorized());
    }

    // Identify the credential by its ID from the assertion (not by user
    // handle: some authenticators omit it, and the credential ID is unique).
    let cred_id_b64 = b64url(req.credential.raw_id.as_ref());
    let Some(row) = db
        .get_webauthn_credential_by_cred_id(&cred_id_b64)
        .map_err(internal)?
    else {
        log_failure("unknown_credential", None);
        return Err(unauthorized());
    };
    let user = db
        .get_user_by_id(&row.user_id)
        .map_err(internal)?
        .ok_or_else(unauthorized)?;

    let mut passkey: Passkey = serde_json::from_str(&row.public_key)
        .map_err(|e| internal(format!("stored passkey is corrupt: {e}")))?;

    let webauthn = build_webauthn(&state.base_url)?;
    let result = match webauthn.finish_discoverable_authentication(
        &req.credential,
        *auth_state,
        &[DiscoverableKey::from(&passkey)],
    ) {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("passkey assertion rejected for user {}: {e}", user.username);
            log_failure("bad_assertion", Some((&user.id, &user.username)));
            return Err(unauthorized());
        }
    };

    // Verify the signature before branching on account state (same
    // anti-enumeration order as the password login).
    if !user.is_active {
        log_failure("account_deactivated", Some((&user.id, &user.username)));
        return Err(unauthorized());
    }

    if require_verified_email() && !user.email_verified {
        log_failure("email_not_verified", Some((&user.id, &user.username)));
        return Err((
            StatusCode::FORBIDDEN,
            "Email address not verified. Sign in with your password to resend the confirmation link."
                .to_string(),
        ));
    }

    // Persist the post-authentication state: sign counter (clone detection),
    // backup-eligibility flags, and last-used timestamp.
    passkey.update_credential(&result);
    if let Ok(public_key) = serde_json::to_string(&passkey) {
        let _ = db.update_webauthn_credential_usage(&row.id, &public_key, result.counter() as i64);
    }

    // A user-verified passkey (PIN/biometric) is itself two factors. webauthn-rs
    // enforces user verification for discoverable logins, so this fallback to
    // the TOTP challenge is defense-in-depth only.
    if user.totp_enabled && !result.user_verified() {
        let mfa_token =
            jwt::issue_mfa_token(&jwt_config, &user.id, &user.username, user.role.as_str())
                .map_err(internal)?;
        return Ok(Json(MfaRequiredResponse {
            mfa_required: true,
            mfa_token,
            expires_in: jwt::MFA_TOKEN_EXPIRY_MINUTES * 60,
        })
        .into_response());
    }

    let _ = db.clear_login_attempts(&user.username);

    // A fresh login starts a new session family (see `login`).
    let family_id = Uuid::new_v4().to_string();
    let (access_token, refresh_token, expires_in) =
        issue_tokens(&jwt_config, &db, &user, &family_id)?;

    {
        let mut b = AuditEventBuilder::new(AuditEventType::LoginSuccess, AuditOutcome::Success)
            .actor(&user.id, &user.username, user.role.as_str())
            .details(serde_json::json!({
                "method": "passkey",
                "passkey_id": row.id,
                "user_verified": result.user_verified(),
            }));
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
            user: UserResponse::from(user),
        }),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_user_uuid_is_deterministic_and_distinct() {
        assert_eq!(stable_user_uuid("u1"), stable_user_uuid("u1"));
        assert_ne!(stable_user_uuid("u1"), stable_user_uuid("u2"));
    }

    #[test]
    fn b64url_has_no_padding() {
        assert_eq!(b64url(&[0xff, 0xfe, 0xfd]), "__79");
        assert_eq!(b64url(b"a"), "YQ");
    }
}
