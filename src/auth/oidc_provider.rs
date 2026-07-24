//! This store as an OIDC **provider** (Unified Accounts).
//!
//! The suite's client apps (viewer, validation platform, form app) sign their
//! users in *against this store* with the standard authorization-code + PKCE
//! flow, so one account and one session cover every tool:
//!
//! * `GET /.well-known/openid-configuration` — discovery.
//! * `GET /oauth/jwks` — the ES256 public key set.
//! * `GET /oauth/authorize` — **served by the SPA** (the frontend route handles
//!   login + consent, then calls the endpoint below); advertised in discovery.
//! * `POST /api/oauth/authorize` (authenticated) — mints the single-use,
//!   PKCE-bound authorization code and returns the `redirect_to` URL.
//! * `POST /oauth/token` — code → tokens (with rotating refresh), refresh →
//!   tokens. Form-encoded per RFC 6749; public clients are PKCE-verified,
//!   confidential clients present their secret.
//! * `GET /oauth/userinfo` — standard claims for a provider access token.
//!
//! Access tokens are ES256 JWTs carrying the account's role and org/group
//! memberships, so resource servers can verify offline against `/oauth/jwks`
//! (see `docs/oidc-provider.md`). The auth middleware also accepts them
//! directly, which keeps `/api/auth/me` working for provider-issued tokens.
//!
//! Clients live in the `oauth_clients` table (admin CRUD) and can be seeded
//! declaratively from `OAUTH_CLIENTS_JSON` for infra-as-code deployments.
//! There is deliberately no implicit/hybrid flow, no plain PKCE method, and
//! no wildcard redirect matching.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Form, Json};
use base64::Engine;
use jsonwebtoken::{
    decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::db::AuthDb;
use super::jwt::hash_token;
use super::middleware::AuthenticatedUser;
use super::models::User;
use super::secret;
use crate::server::AppState;

/// Code lifetime (RFC 6749 §4.1.2 recommends ≤ 10 minutes).
const CODE_TTL_SECS: i64 = 600;
/// Provider access-token lifetime.
const ACCESS_TTL_SECS: i64 = 3600;
/// Rotating refresh-token lifetime.
const REFRESH_TTL_SECS: i64 = 30 * 86400;
/// The only scopes this provider knows. Anything else in a request is dropped.
const KNOWN_SCOPES: &[&str] = &["openid", "profile", "email"];

fn b64url(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

fn rand_token(prefix: &str) -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    format!("{prefix}{}", b64url(&bytes))
}

fn now_plus(secs: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339()
}

fn normalize_scope(requested: &str) -> String {
    let mut out: Vec<&str> = requested
        .split_whitespace()
        .filter(|s| KNOWN_SCOPES.contains(s))
        .collect();
    if !out.contains(&"openid") {
        out.insert(0, "openid");
    }
    out.join(" ")
}

// ─── Signing keys ──────────────────────────────────────────────────────────────

/// The provider's active ES256 keypair, loaded once at boot.
pub struct ProviderKeys {
    pub kid: String,
    encoding: EncodingKey,
    decoding: DecodingKey,
    /// The public JWK served at /oauth/jwks.
    pub public_jwk: serde_json::Value,
}

impl ProviderKeys {
    /// Load the persisted keypair, generating one on first boot. The private
    /// key is stored AES-GCM-encrypted with the HKDF key derived from the JWT
    /// secret (same scheme as stored OAuth client secrets).
    pub fn load_or_generate(db: &AuthDb, jwt_secret: &str) -> anyhow::Result<Self> {
        if let Some((kid, alg, pkcs8_enc, public_jwk)) = db.get_signing_key()? {
            anyhow::ensure!(alg == "ES256", "unsupported provider key alg {alg}");
            let pkcs8_b64 = secret::decrypt_secret(&pkcs8_enc, jwt_secret)?;
            let pkcs8 = base64::engine::general_purpose::STANDARD.decode(pkcs8_b64)?;
            let jwk: serde_json::Value = serde_json::from_str(&public_jwk)?;
            return Self::from_pkcs8(kid, &pkcs8, jwk);
        }
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| anyhow::anyhow!("ES256 keypair generation failed"))?;
        let kid = uuid::Uuid::new_v4().to_string();
        let jwk = Self::public_jwk_from_pkcs8(&kid, pkcs8.as_ref())?;
        let pkcs8_b64 = base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref());
        let pkcs8_enc = secret::encrypt_secret(&pkcs8_b64, jwt_secret)?;
        db.insert_signing_key(&kid, "ES256", &pkcs8_enc, &jwk.to_string())?;
        Self::from_pkcs8(kid, pkcs8.as_ref(), jwk)
    }

    fn from_pkcs8(kid: String, pkcs8: &[u8], jwk: serde_json::Value) -> anyhow::Result<Self> {
        let (x, y) = (
            jwk["x"].as_str().unwrap_or_default().to_string(),
            jwk["y"].as_str().unwrap_or_default().to_string(),
        );
        Ok(Self {
            kid,
            encoding: EncodingKey::from_ec_der(pkcs8),
            decoding: DecodingKey::from_ec_components(&x, &y)
                .map_err(|e| anyhow::anyhow!("invalid stored public JWK: {e}"))?,
            public_jwk: jwk,
        })
    }

    fn public_jwk_from_pkcs8(kid: &str, pkcs8: &[u8]) -> anyhow::Result<serde_json::Value> {
        let rng = SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8, &rng)
            .map_err(|_| anyhow::anyhow!("invalid generated pkcs8"))?;
        let public = pair.public_key().as_ref();
        // Uncompressed SEC1 point: 0x04 || X (32) || Y (32).
        anyhow::ensure!(
            public.len() == 65 && public[0] == 4,
            "unexpected EC point encoding"
        );
        Ok(serde_json::json!({
            "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig",
            "kid": kid,
            "x": b64url(&public[1..33]),
            "y": b64url(&public[33..65]),
        }))
    }

    /// Verify one of OUR access tokens: ES256, our issuer, unexpired. Returns
    /// the claims. (Audience is per-client, so it is checked by callers that
    /// know which client they are — the middleware accepts any registered aud.)
    pub fn verify(&self, issuer: &str, token: &str) -> Option<serde_json::Value> {
        let header = decode_header(token).ok()?;
        if header.alg != Algorithm::ES256 {
            return None;
        }
        let mut validation = Validation::new(Algorithm::ES256);
        validation.set_issuer(&[issuer]);
        validation.validate_aud = false; // aud = client_id, verified by resource servers
        decode::<serde_json::Value>(token, &self.decoding, &validation)
            .ok()
            .map(|d| d.claims)
    }
}

/// Seed clients from `OAUTH_CLIENTS_JSON` (infra-as-code): a JSON array of
/// `{client_id, name, redirect_uris: [..], public?, secret?}`. Secrets are
/// stored encrypted; entries upsert (existing secrets survive omission).
pub fn seed_clients_from_env(db: &AuthDb, jwt_secret: &str) {
    let raw = match std::env::var("OAUTH_CLIENTS_JSON") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return,
    };
    #[derive(Deserialize)]
    struct SeedClient {
        client_id: String,
        name: String,
        redirect_uris: Vec<String>,
        #[serde(default = "default_true")]
        public: bool,
        secret: Option<String>,
    }
    fn default_true() -> bool {
        true
    }
    let parsed: Vec<SeedClient> = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("OAUTH_CLIENTS_JSON is not valid JSON ({e}); skipping client seed");
            return;
        }
    };
    for c in parsed {
        let secret_enc = c.secret.as_deref().and_then(|s| {
            secret::encrypt_secret(s, jwt_secret)
                .map_err(|e| tracing::warn!("client {} secret not stored: {e}", c.client_id))
                .ok()
        });
        match db.upsert_oauth_client(
            &c.client_id,
            &c.name,
            &c.redirect_uris,
            c.public,
            secret_enc.as_deref(),
        ) {
            Ok(()) => tracing::info!("OIDC client '{}' seeded from env", c.client_id),
            Err(e) => tracing::warn!("OIDC client '{}' seed failed: {e}", c.client_id),
        }
    }
}

// ─── Admin client CRUD ────────────────────────────────────────────────────────

/// GET /api/admin/oauth-clients — every registered relying-party client
/// (secrets never leave the server; only `has_secret` is reported).
pub async fn admin_list_clients(
    State(state): State<AppState>,
) -> Result<Json<Vec<super::models::OidcClient>>, (StatusCode, String)> {
    state
        .auth_db
        .list_oauth_clients()
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[derive(Deserialize)]
pub struct UpsertClientRequest {
    pub client_id: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
    #[serde(default = "default_public")]
    pub public: bool,
    /// Confidential clients only. Omitted = keep the existing secret.
    pub secret: Option<String>,
}

fn default_public() -> bool {
    true
}

/// POST /api/admin/oauth-clients — create or update a client.
pub async fn admin_upsert_client(
    State(state): State<AppState>,
    Json(req): Json<UpsertClientRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if req.client_id.trim().is_empty() || req.redirect_uris.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "client_id and at least one redirect_uri are required".to_string(),
        ));
    }
    for uri in &req.redirect_uris {
        let ok = uri.starts_with("http://") || uri.starts_with("https://");
        if !ok || uri.contains('*') {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("redirect_uri {uri:?} must be an absolute http(s) URL without wildcards"),
            ));
        }
    }
    if req.public && req.secret.is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            "public clients cannot have a secret (use PKCE)".to_string(),
        ));
    }
    let secret_enc = match req.secret.as_deref() {
        Some(s) => Some(
            secret::encrypt_secret(s, &state.jwt_config.secret)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        ),
        None => None,
    };
    state
        .auth_db
        .upsert_oauth_client(
            &req.client_id,
            &req.name,
            &req.redirect_uris,
            req.public,
            secret_enc.as_deref(),
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let client = state
        .auth_db
        .get_oauth_client(&req.client_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "client missing after upsert".to_string(),
        ))?;
    Ok(Json(client))
}

/// DELETE /api/admin/oauth-clients/:client_id — remove a client (and its
/// outstanding refresh tokens).
pub async fn admin_delete_client(
    State(state): State<AppState>,
    axum::extract::Path(client_id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let deleted = state
        .auth_db
        .delete_oauth_client(&client_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !deleted {
        return Err((StatusCode::NOT_FOUND, "unknown client".to_string()));
    }
    Ok(Json(serde_json::json!({ "deleted": client_id })))
}

// ─── Discovery + JWKS ─────────────────────────────────────────────────────────

/// GET /.well-known/openid-configuration
pub async fn discovery(State(state): State<AppState>) -> Json<serde_json::Value> {
    let base = state.base_url.trim_end_matches('/').to_string();
    Json(serde_json::json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "jwks_uri": format!("{base}/oauth/jwks"),
        "userinfo_endpoint": format!("{base}/oauth/userinfo"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "scopes_supported": KNOWN_SCOPES,
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["ES256"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_post"],
    }))
}

/// GET /oauth/jwks
pub async fn jwks(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let keys = provider(&state)?;
    Ok(Json(serde_json::json!({ "keys": [keys.public_jwk] })))
}

fn provider(state: &AppState) -> Result<&Arc<ProviderKeys>, (StatusCode, String)> {
    state.oidc_provider.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "OIDC provider keys unavailable".to_string(),
    ))
}

// ─── Authorize (SPA-driven) ───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AuthorizeRequest {
    pub client_id: String,
    pub redirect_uri: String,
    #[serde(default)]
    pub scope: String,
    pub state: Option<String>,
    pub nonce: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    /// "check" (default) returns client info + whether consent is still needed;
    /// "approve" mints the code.
    #[serde(default)]
    pub decision: String,
}

/// POST /api/oauth/authorize (authenticated; the /oauth/authorize SPA route
/// drives it). Validates client + redirect URI + PKCE, then either reports
/// consent state ("check") or mints the code ("approve").
pub async fn authorize(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(req): Json<AuthorizeRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let db = &state.auth_db;
    let client = db
        .get_oauth_client(&req.client_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::BAD_REQUEST, "unknown client_id".to_string()))?;
    if !client.redirect_uris.iter().any(|u| u == &req.redirect_uri) {
        return Err((
            StatusCode::BAD_REQUEST,
            "redirect_uri not allowed for this client".to_string(),
        ));
    }
    let challenge = req.code_challenge.as_deref().unwrap_or("");
    if client.public {
        if challenge.is_empty() || req.code_challenge_method.as_deref() != Some("S256") {
            return Err((
                StatusCode::BAD_REQUEST,
                "public clients must use PKCE with S256".to_string(),
            ));
        }
    } else if !challenge.is_empty() && req.code_challenge_method.as_deref() != Some("S256") {
        return Err((
            StatusCode::BAD_REQUEST,
            "only the S256 code_challenge_method is supported".to_string(),
        ));
    }
    let scope = normalize_scope(&req.scope);
    let requires_consent = !db.has_oauth_consent(&current_user.user_id, &client.client_id, &scope);

    if req.decision != "approve" {
        return Ok(Json(serde_json::json!({
            "client_id": client.client_id,
            "client_name": client.name,
            "scope": scope,
            "requires_consent": requires_consent,
        })));
    }

    let code = rand_token("otc_");
    db.insert_oauth_code(
        &hash_token(&code),
        &client.client_id,
        &current_user.user_id,
        &req.redirect_uri,
        &scope,
        req.nonce.as_deref(),
        challenge,
        &now_plus(CODE_TTL_SECS),
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    db.record_oauth_consent(&current_user.user_id, &client.client_id, &scope)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut redirect_to = format!(
        "{}{}code={}",
        req.redirect_uri,
        if req.redirect_uri.contains('?') {
            "&"
        } else {
            "?"
        },
        urlencoding_encode(&code)
    );
    if let Some(s) = &req.state {
        redirect_to.push_str(&format!("&state={}", urlencoding_encode(s)));
    }
    Ok(Json(serde_json::json!({ "redirect_to": redirect_to })))
}

fn urlencoding_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

// ─── Token endpoint ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub code_verifier: Option<String>,
    pub refresh_token: Option<String>,
}

#[derive(Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
    pub refresh_token: String,
    pub id_token: String,
    pub scope: String,
}

fn oauth_err(status: StatusCode, code: &str, desc: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({ "error": code, "error_description": desc })),
    )
}

/// POST /oauth/token (public; form-encoded per RFC 6749).
pub async fn token(
    State(state): State<AppState>,
    Form(req): Form<TokenRequest>,
) -> Result<Json<TokenResponse>, (StatusCode, Json<serde_json::Value>)> {
    let db = state.auth_db.clone();
    let client_id = req.client_id.clone().ok_or_else(|| {
        oauth_err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "client_id required",
        )
    })?;
    let client = db
        .get_oauth_client(&client_id)
        .map_err(|_| oauth_err(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "storage"))?
        .ok_or_else(|| oauth_err(StatusCode::BAD_REQUEST, "invalid_client", "unknown client"))?;

    // Confidential clients must authenticate; public clients must NOT have a secret.
    if !client.public {
        let presented = req.client_secret.as_deref().unwrap_or("");
        let stored = client
            .secret_enc
            .as_deref()
            .and_then(|enc| secret::decrypt_secret(enc, &state.jwt_config.secret).ok())
            .unwrap_or_default();
        let ok = !presented.is_empty()
            && !stored.is_empty()
            && super::totp::constant_time_eq(presented.as_bytes(), stored.as_bytes());
        if !ok {
            return Err(oauth_err(
                StatusCode::UNAUTHORIZED,
                "invalid_client",
                "client authentication failed",
            ));
        }
    }

    match req.grant_type.as_str() {
        "authorization_code" => {
            let code = req.code.as_deref().ok_or_else(|| {
                oauth_err(StatusCode::BAD_REQUEST, "invalid_request", "code required")
            })?;
            let row = db
                .consume_oauth_code(&hash_token(code))
                .map_err(|_| {
                    oauth_err(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "storage")
                })?
                .ok_or_else(|| {
                    oauth_err(
                        StatusCode::BAD_REQUEST,
                        "invalid_grant",
                        "code invalid, expired or already used",
                    )
                })?;
            if row.client_id != client_id {
                return Err(oauth_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_grant",
                    "code was issued to another client",
                ));
            }
            if req.redirect_uri.as_deref() != Some(row.redirect_uri.as_str()) {
                return Err(oauth_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_grant",
                    "redirect_uri mismatch",
                ));
            }
            if !row.code_challenge.is_empty() {
                let verifier = req.code_verifier.as_deref().unwrap_or("");
                let hashed = b64url(&Sha256::digest(verifier.as_bytes()));
                if verifier.is_empty() || hashed != row.code_challenge {
                    return Err(oauth_err(
                        StatusCode::BAD_REQUEST,
                        "invalid_grant",
                        "PKCE verification failed",
                    ));
                }
            }
            issue_tokens(
                &state,
                &client_id,
                &row.user_id,
                &row.scope,
                row.nonce.as_deref(),
            )
        }
        "refresh_token" => {
            let presented = req.refresh_token.as_deref().unwrap_or("");
            if presented.is_empty() {
                return Err(oauth_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "refresh_token required",
                ));
            }
            let (rt_client, user_id, scope, expires_at) = db
                .take_client_refresh_token(&hash_token(presented))
                .map_err(|_| {
                    oauth_err(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "storage")
                })?
                .ok_or_else(|| {
                    oauth_err(
                        StatusCode::BAD_REQUEST,
                        "invalid_grant",
                        "refresh token invalid or already rotated",
                    )
                })?;
            if rt_client != client_id {
                return Err(oauth_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_grant",
                    "refresh token was issued to another client",
                ));
            }
            if expires_at < chrono::Utc::now().to_rfc3339() {
                return Err(oauth_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_grant",
                    "refresh token expired",
                ));
            }
            issue_tokens(&state, &client_id, &user_id, &scope, None)
        }
        other => Err(oauth_err(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            &format!("unsupported grant_type {other:?}"),
        )),
    }
}

fn issue_tokens(
    state: &AppState,
    client_id: &str,
    user_id: &str,
    scope: &str,
    nonce: Option<&str>,
) -> Result<Json<TokenResponse>, (StatusCode, Json<serde_json::Value>)> {
    let db = &state.auth_db;
    let keys = state.oidc_provider.as_ref().ok_or_else(|| {
        oauth_err(
            StatusCode::SERVICE_UNAVAILABLE,
            "server_error",
            "provider keys unavailable",
        )
    })?;
    let user = db
        .get_user_by_id(user_id)
        .ok()
        .flatten()
        .filter(|u| u.is_active)
        .ok_or_else(|| {
            oauth_err(
                StatusCode::BAD_REQUEST,
                "invalid_grant",
                "account unavailable",
            )
        })?;

    let issuer = state.base_url.trim_end_matches('/').to_string();
    let now = chrono::Utc::now().timestamp();
    let orgs: Vec<serde_json::Value> = db
        .list_user_membership_summaries(user_id)
        .unwrap_or_default()
        .into_iter()
        .map(|(slug, _name, role)| serde_json::json!({ "slug": slug, "role": role }))
        .collect();
    let groups: Vec<serde_json::Value> = db
        .list_user_group_summaries(user_id)
        .unwrap_or_default()
        .into_iter()
        .map(|(org_slug, id, name)| serde_json::json!({ "org_slug": org_slug, "id": id, "name": name }))
        .collect();

    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(keys.kid.clone());

    let access_claims = serde_json::json!({
        "iss": issuer, "sub": user.id, "aud": client_id,
        "iat": now, "exp": now + ACCESS_TTL_SECS,
        "jti": uuid::Uuid::new_v4().to_string(),
        "token_use": "access", "scope": scope,
        "username": user.username, "email": user.email,
        "role": user.role.as_str(),
        "organisations": orgs, "groups": groups,
    });
    let mut id_claims = serde_json::json!({
        "iss": issuer, "sub": user.id, "aud": client_id,
        "iat": now, "exp": now + ACCESS_TTL_SECS,
        "email": user.email, "preferred_username": user.username,
        "name": user.display_name.clone().unwrap_or_else(|| user.username.clone()),
    });
    if let Some(n) = nonce {
        id_claims["nonce"] = serde_json::Value::String(n.to_string());
    }

    let sign = |claims: &serde_json::Value| {
        encode(&header, claims, &keys.encoding).map_err(|_| {
            oauth_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "signing failed",
            )
        })
    };
    let access_token = sign(&access_claims)?;
    let id_token = sign(&id_claims)?;

    let refresh = rand_token("otr_");
    db.insert_client_refresh_token(
        &hash_token(&refresh),
        client_id,
        user_id,
        scope,
        &now_plus(REFRESH_TTL_SECS),
    )
    .map_err(|_| oauth_err(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "storage"))?;

    Ok(Json(TokenResponse {
        access_token,
        token_type: "Bearer",
        expires_in: ACCESS_TTL_SECS,
        refresh_token: refresh,
        id_token,
        scope: scope.to_string(),
    }))
}

// ─── Userinfo + middleware acceptance ─────────────────────────────────────────

/// GET /oauth/userinfo — standard claims for a provider-issued access token.
pub async fn userinfo(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let keys = provider(&state)?;
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .unwrap_or("");
    let issuer = state.base_url.trim_end_matches('/');
    let claims = keys
        .verify(issuer, token)
        .ok_or((StatusCode::UNAUTHORIZED, "invalid access token".to_string()))?;
    let sub = claims["sub"].as_str().unwrap_or_default().to_string();
    let user: Option<User> = state.auth_db.get_user_by_id(&sub).ok().flatten();
    let Some(user) = user.filter(|u| u.is_active) else {
        return Err((StatusCode::UNAUTHORIZED, "account unavailable".to_string()));
    };
    Ok(Json(serde_json::json!({
        "sub": user.id,
        "preferred_username": user.username,
        "email": user.email,
        "email_verified": user.email_verified,
        "name": user.display_name.unwrap_or(user.username),
        "role": user.role.as_str(),
    })))
}

/// Middleware hook: the `sub` of a provider-issued ES256 **access** token, or
/// None when the token is not ours / not an access token. The middleware owns
/// the user load + is_active handling (so deactivation messages — e.g. the
/// guest-disabled one — stay consistent across every token kind).
pub fn provider_token_subject(
    state_keys: &ProviderKeys,
    issuer: &str,
    token: &str,
) -> Option<String> {
    let claims = state_keys.verify(issuer, token)?;
    if claims["token_use"].as_str() != Some("access") {
        return None;
    }
    claims["sub"].as_str().map(str::to_string)
}
