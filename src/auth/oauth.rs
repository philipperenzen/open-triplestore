//! OIDC / OAuth 2.0 flow implementation.
//!
//! Supports any OpenID Connect provider including Azure AD / Entra ID.
//!
//! ## Azure AD configuration
//! Set `discovery_url` to:
//! - Single tenant: `https://login.microsoftonline.com/{tenant_id}/v2.0/.well-known/openid-configuration`
//! - Multi-tenant:  `https://login.microsoftonline.com/common/v2.0/.well-known/openid-configuration`
//!
//! Set `tenant_id` in the DB row to the AAD tenant GUID (or "common" for
//! multi-tenant).  Leave blank for non-Azure providers.
//!
//! ## Role mapping
//! `role_claim_map` is a JSON string, e.g.:
//! ```json
//! {
//!   "00000000-0000-0000-0000-000000000001": "admin",
//!   "reader-group-guid": "user"
//! }
//! ```
//! The values are checked against the `groups` array claim in the ID token.
//! The highest-privilege matched role is used; falls back to `default_role`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    reqwest::async_http_client,
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
use tracing::debug;
use uuid::Uuid;

use super::db::AuthDb;
use super::jwt::{issue_access_token, issue_refresh_token};
use super::models::{map_claims_to_role, OauthProvider, SystemRole};
use super::secret::decrypt_secret;

// ─── In-memory PKCE session store ─────────────────────────────────────────────

const SESSION_TTL_SECS: u64 = 600; // 10 minutes

#[derive(Debug)]
pub struct OAuthSession {
    pub provider_id: String,
    pub pkce_verifier: PkceCodeVerifier,
    pub nonce: Nonce,
    pub created_at: Instant,
}

/// Thread-safe in-memory store for ongoing OAuth PKCE sessions.
/// Keyed by the CSRF `state` token returned to the browser.
pub type OAuthSessions = Arc<DashMap<String, OAuthSession>>;

pub fn new_session_store() -> OAuthSessions {
    Arc::new(DashMap::new())
}

/// Hard cap on concurrent in-flight OAuth/PKCE sessions, bounding memory if an
/// attacker floods `authorize` within the TTL window (the SSO routes are also
/// rate-limited, so this is defense-in-depth).
const MAX_OAUTH_SESSIONS: usize = 10_000;

/// Remove expired sessions (call periodically or on each request), then enforce a
/// hard size cap by evicting the oldest sessions beyond `MAX_OAUTH_SESSIONS`.
pub fn prune_sessions(sessions: &OAuthSessions) {
    let ttl = Duration::from_secs(SESSION_TTL_SECS);
    sessions.retain(|_, v| v.created_at.elapsed() < ttl);

    if sessions.len() > MAX_OAUTH_SESSIONS {
        let mut entries: Vec<(String, Instant)> = sessions
            .iter()
            .map(|e| (e.key().clone(), e.value().created_at))
            .collect();
        entries.sort_by_key(|(_, t)| *t); // oldest first
        let to_remove = entries.len().saturating_sub(MAX_OAUTH_SESSIONS);
        for (k, _) in entries.into_iter().take(to_remove) {
            sessions.remove(&k);
        }
    }
}

// ─── OIDC flow ────────────────────────────────────────────────────────────────

/// Build the OIDC redirect URL for a provider.
/// Returns `(redirect_url, state_key)`.  The state key is stored in
/// `sessions` and must be passed back in the callback.
pub async fn begin_oidc_flow(
    provider: &OauthProvider,
    sessions: &OAuthSessions,
    redirect_uri: &str,
    jwt_secret: &str,
) -> anyhow::Result<(String, String)> {
    let discovery_url = provider
        .discovery_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Provider '{}' has no discovery_url", provider.slug))?;
    // Never fetch IdP metadata over cleartext (MITM → forged identity/tokens):
    // require https, with a loopback http exception for local dev.
    if !super::oidc_rs::is_secure_idp_url(discovery_url) {
        anyhow::bail!(
            "OIDC provider '{}' discovery_url must use https (refusing to fetch IdP \
             metadata over cleartext): {discovery_url}",
            provider.slug
        );
    }

    let client_id = provider
        .client_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Provider '{}' has no client_id", provider.slug))?;

    let client_secret = match &provider.client_secret_enc {
        Some(enc) => Some(decrypt_secret(enc, jwt_secret)?),
        None => None,
    };

    let issuer = IssuerUrl::new(
        // For Azure AD, strip the well-known path to get the issuer base
        discovery_url
            .trim_end_matches("/.well-known/openid-configuration")
            .to_string(),
    )?;

    let provider_metadata = CoreProviderMetadata::discover_async(issuer, async_http_client).await?;

    let oidc_client = CoreClient::from_provider_metadata(
        provider_metadata,
        ClientId::new(client_id.to_string()),
        client_secret.map(ClientSecret::new),
    )
    .set_redirect_uri(RedirectUrl::new(redirect_uri.to_string())?);

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let nonce = Nonce::new_random();

    let scopes: Vec<Scope> = provider
        .scopes
        .split_whitespace()
        .filter(|s| *s != "openid") // openid is added automatically
        .map(|s| Scope::new(s.to_string()))
        .collect();

    let nonce_for_auth = nonce.clone();
    let (auth_url, csrf_token, _nonce) = {
        let mut req = oidc_client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                move || nonce_for_auth,
            )
            .set_pkce_challenge(pkce_challenge);
        for scope in scopes {
            req = req.add_scope(scope);
        }
        req.url()
    };

    let state_key = csrf_token.secret().clone();
    prune_sessions(sessions);
    sessions.insert(
        state_key.clone(),
        OAuthSession {
            provider_id: provider.id.clone(),
            pkce_verifier,
            nonce,
            created_at: Instant::now(),
        },
    );

    debug!(
        "OIDC flow started for provider '{}', state={}",
        provider.slug,
        &state_key[..8]
    );
    Ok((auth_url.to_string(), state_key))
}

/// Exchange the authorization code for tokens and provision/look-up the local user.
/// Returns `(access_token, refresh_token)` ready to send to the browser.
pub async fn complete_oidc_flow(
    code: &str,
    state_key: &str,
    sessions: &OAuthSessions,
    auth_db: &Arc<AuthDb>,
    jwt_config: &super::jwt::JwtConfig,
    redirect_uri: &str,
    _base_url: &str,
) -> anyhow::Result<(String, String)> {
    // Look up + remove session (one-time use)
    let session = sessions
        .remove(state_key)
        .map(|(_, v)| v)
        .ok_or_else(|| anyhow::anyhow!("Unknown or expired OAuth state"))?;

    if session.created_at.elapsed() > Duration::from_secs(SESSION_TTL_SECS) {
        anyhow::bail!("OAuth session has expired");
    }

    let provider = auth_db
        .get_oauth_provider_by_id(&session.provider_id)?
        .ok_or_else(|| anyhow::anyhow!("OAuth provider not found"))?;

    let client_secret = match &provider.client_secret_enc {
        Some(enc) => Some(decrypt_secret(enc, &jwt_config.secret)?),
        None => None,
    };

    let discovery_url = provider
        .discovery_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Provider has no discovery_url"))?;
    // Never fetch IdP metadata / exchange the code against an issuer reached over
    // cleartext (MITM → forged identity/tokens): require https (loopback http ok for dev).
    if !super::oidc_rs::is_secure_idp_url(discovery_url) {
        anyhow::bail!(
            "OIDC provider '{}' discovery_url must use https (refusing to fetch IdP \
             metadata over cleartext): {discovery_url}",
            provider.slug
        );
    }

    let issuer = IssuerUrl::new(
        discovery_url
            .trim_end_matches("/.well-known/openid-configuration")
            .to_string(),
    )?;
    let provider_metadata = CoreProviderMetadata::discover_async(issuer, async_http_client).await?;

    let client_id = provider
        .client_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Provider has no client_id"))?;

    let oidc_client = CoreClient::from_provider_metadata(
        provider_metadata,
        ClientId::new(client_id.to_string()),
        client_secret.map(ClientSecret::new),
    )
    .set_redirect_uri(RedirectUrl::new(redirect_uri.to_string())?);

    let token_response = oidc_client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .set_pkce_verifier(session.pkce_verifier)
        .request_async(async_http_client)
        .await?;

    let id_token = token_response
        .id_token()
        .ok_or_else(|| anyhow::anyhow!("No id_token in response"))?;

    // Verify the ID token
    let id_token_verifier = oidc_client.id_token_verifier();
    let claims = id_token.claims(&id_token_verifier, &session.nonce)?;

    let sub = claims.subject().as_str().to_string();
    let email = claims.email().map(|e| e.as_str().to_string());
    // Only an IdP-asserted verified email may auto-link to an existing local
    // account (account-takeover defense — see `provision_or_link_user`).
    let email_verified = claims.email_verified().unwrap_or(false);
    let name = claims
        .name()
        .and_then(|n| n.get(None))
        .map(|n| n.as_str().to_string())
        .unwrap_or_else(|| email.clone().unwrap_or_else(|| sub.clone()));

    // Map IdP group/role claims to a role + capabilities. The typed OIDC claims
    // view uses `EmptyAdditionalClaims` and can't expose arbitrary claims, so we
    // read the verified ID token's payload directly (its signature was validated
    // by `id_token.claims(..)` above, so reading the payload is safe).
    let groups = groups_from_verified_id_token(id_token);
    let (derived_role, grant_publish) = derive_grants_from_claims(&groups, &provider);

    let mut user = provision_or_link_user(
        &sub,
        email.as_deref(),
        email_verified,
        &name,
        derived_role,
        &provider,
        auth_db,
    )?;

    // Grant the publish capability when a claim maps to "publisher".
    // Non-destructive: only ever sets the flag, never revokes it.
    if grant_publish && !user.can_publish {
        auth_db.update_user_can_publish(&user.id, true)?;
        user.can_publish = true;
    }

    // Issue local JWT pair
    let access = issue_access_token(jwt_config, &user.id, &user.username, user.role.as_str())?;
    let refresh_id = Uuid::new_v4().to_string();
    let refresh = issue_refresh_token(jwt_config, &user.id, &user.username, user.role.as_str())?;

    // Store refresh token hash — a fresh SSO login starts its own session family.
    let refresh_hash = super::jwt::hash_token(&refresh);
    let family_id = Uuid::new_v4().to_string();
    let expires =
        chrono::Utc::now() + chrono::Duration::days(jwt_config.refresh_expiry_days as i64);
    auth_db.create_refresh_token(
        &refresh_id,
        &user.id,
        &refresh_hash,
        &expires.to_rfc3339(),
        &family_id,
    )?;

    Ok((access, refresh))
}

// ─── User provisioning ────────────────────────────────────────────────────────

/// Extract group/role claim values from an already-verified ID token.
///
/// The OIDC client uses `EmptyAdditionalClaims`, so the typed claims view can't
/// see arbitrary claims like `groups`/`roles`. We instead decode the token's
/// payload segment directly. This is safe: the caller verifies the token's
/// signature and standard claims before calling this. Reads the common
/// `groups` and `roles` claims (each a string array or a single string).
fn groups_from_verified_id_token(id_token: &openidconnect::core::CoreIdToken) -> Vec<String> {
    // openidconnect serializes an IdToken to its compact JWT string form.
    match serde_json::to_value(id_token) {
        Ok(serde_json::Value::String(jwt)) => group_claims_from_jwt(&jwt),
        _ => Vec::new(),
    }
}

/// Decode a compact JWT and return the string values of its `groups` and
/// `roles` claims (each may be a string array or a single string). Pure and
/// signature-agnostic — only call on tokens whose signature is already verified.
fn group_claims_from_jwt(jwt: &str) -> Vec<String> {
    use base64::Engine as _;

    let Some(payload_b64) = jwt.split('.').nth(1) else {
        return Vec::new();
    };
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload_b64) else {
        return Vec::new();
    };
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for key in ["groups", "roles"] {
        match payload.get(key) {
            Some(serde_json::Value::Array(arr)) => {
                out.extend(arr.iter().filter_map(|v| v.as_str().map(str::to_string)));
            }
            Some(serde_json::Value::String(s)) => out.push(s.clone()),
            _ => {}
        }
    }
    out
}

/// Map group claim values to a `(role, grant_publish)` pair using the provider's
/// `role_claim_map`, folding in `default_role` when no role claim matched.
fn derive_grants_from_claims(groups: &[String], provider: &OauthProvider) -> (SystemRole, bool) {
    let mapped = map_claims_to_role(groups, provider.role_claim_map.as_deref());
    let default_role = SystemRole::from_str(&provider.default_role).unwrap_or(SystemRole::User);
    let mut role = match mapped.role {
        Some(role) if role.level() > default_role.level() => role,
        _ => default_role,
    };
    // SSO must never grant super_admin: that tier governs the whole instance and
    // is provisioned out-of-band (CLI `--promote-super-admin`). A misconfigured or
    // attacker-influenced group/role claim mapping to "super_admin" is capped at
    // admin so identity-provider membership can never confer instance ownership.
    if role == SystemRole::SuperAdmin {
        role = SystemRole::Admin;
    }
    (role, mapped.grant_publish)
}

/// Find an existing user via OAuth identity, or auto-provision a new one,
/// or (if `auto_provision=false`) require a pre-existing account.
pub fn provision_or_link_user(
    external_subject: &str,
    email: Option<&str>,
    email_verified: bool,
    display_name: &str,
    derived_role: SystemRole,
    provider: &OauthProvider,
    auth_db: &Arc<AuthDb>,
) -> anyhow::Result<super::models::User> {
    // 1. Check if we already have an identity link on the stable (provider, subject)
    //    pair. This is the authoritative match — never email.
    if let Some(user) = auth_db.find_user_by_oauth_identity(&provider.id, external_subject)? {
        // Refresh the identity record's last_login_at
        let _ = auth_db.upsert_oauth_identity(
            &Uuid::new_v4().to_string(),
            &user.id,
            &provider.id,
            external_subject,
            email,
        );
        return Ok(user);
    }

    // 2. Match an existing local user by email — ONLY when the IdP asserts the
    //    email is verified. Account-takeover defense: without this gate, an
    //    attacker could register an IdP account using a victim's email (which many
    //    IdPs never verify) and, on first SSO login, get the attacker's subject
    //    linked to the victim's existing local account — including an admin. When
    //    the email is unverified but already belongs to a local account, we refuse
    //    rather than link or create a colliding account.
    if let Some(email_str) = email {
        if let Some(user) = auth_db.get_user_by_email(email_str)? {
            if !email_verified {
                anyhow::bail!(
                    "An account already exists for this email. For security, sign in to that \
                     account and link this identity provider from your settings — the provider \
                     did not assert a verified email, so automatic linking is refused."
                );
            }
            if !user.is_active {
                anyhow::bail!("User account is deactivated");
            }
            // Link the external identity to the existing account (verified email).
            auth_db.upsert_oauth_identity(
                &Uuid::new_v4().to_string(),
                &user.id,
                &provider.id,
                external_subject,
                Some(email_str),
            )?;
            return Ok(user);
        }
    }

    // 3. Auto-provision or reject
    if !provider.auto_provision {
        anyhow::bail!(
            "No matching local account found and auto-provisioning is disabled for provider '{}'",
            provider.slug
        );
    }

    // Create a new local user
    let new_user_id = Uuid::new_v4().to_string();
    // Derive a unique username from display_name
    let base_username = display_name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>()
        .to_lowercase();
    let username = if base_username.is_empty() {
        format!("user_{}", &new_user_id[..8])
    } else {
        base_username
    };

    // Ensure username is unique
    let unique_username = ensure_unique_username(auth_db, &username)?;
    let fallback_email = format!("{}@oauth.local", &new_user_id[..8]);
    let email_for_user = email.unwrap_or(&fallback_email);

    // Use a non-usable password hash (account uses SSO only)
    let password_hash = format!("oauth:{}:{}", provider.slug, external_subject);

    let user = auth_db.create_user(
        &new_user_id,
        &unique_username,
        email_for_user,
        &password_hash,
        derived_role,
    )?;

    // Link identity
    auth_db.upsert_oauth_identity(
        &Uuid::new_v4().to_string(),
        &user.id,
        &provider.id,
        external_subject,
        email,
    )?;

    Ok(user)
}

fn ensure_unique_username(auth_db: &Arc<AuthDb>, base: &str) -> anyhow::Result<String> {
    if auth_db.get_user_by_username(base)?.is_none() {
        return Ok(base.to_string());
    }
    for i in 2..=9999u32 {
        let candidate = format!("{}_{}", base, i);
        if auth_db.get_user_by_username(&candidate)?.is_none() {
            return Ok(candidate);
        }
    }
    Ok(format!("{}__{}", base, Uuid::new_v4().simple()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    /// Build a compact JWT (`header.payload.sig`) with the given JSON payload.
    fn fake_jwt(payload: serde_json::Value) -> String {
        let b64 = |v: &serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap())
        };
        format!(
            "{}.{}.{}",
            b64(&serde_json::json!({"alg": "RS256", "typ": "JWT"})),
            b64(&payload),
            "sig"
        )
    }

    #[test]
    fn group_claims_extracts_arrays_and_strings() {
        let jwt = fake_jwt(serde_json::json!({
            "sub": "u1",
            "groups": ["team-admins", "team-pub"],
            "roles": "staff",
        }));
        let mut got = group_claims_from_jwt(&jwt);
        got.sort();
        assert_eq!(got, vec!["staff", "team-admins", "team-pub"]);
    }

    #[test]
    fn group_claims_absent_or_malformed_yields_empty() {
        // No group/role claims.
        let jwt = fake_jwt(serde_json::json!({ "sub": "u1", "email": "a@b.c" }));
        assert!(group_claims_from_jwt(&jwt).is_empty());
        // Not a JWT at all.
        assert!(group_claims_from_jwt("not-a-jwt").is_empty());
        // Payload isn't valid base64/JSON.
        assert!(group_claims_from_jwt("aaa.!!!.bbb").is_empty());
    }
}
