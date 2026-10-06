//! SAML 2.0 with this store as the service provider (SP).
//!
//! Enabled by the `saml` feature. Without it the module still compiles, the
//! login page offers no SAML provider and every entry point answers that SAML
//! is not compiled in.
//!
//! ## What a provider holds
//! - `entity_id`, `sso_url` — the IdP's entity ID and SSO endpoint
//!   (HTTP-Redirect binding);
//! - `idp_certificate` — one or more IdP signing certificates, so a rollover
//!   can trust the old and the new key at once. A provider without one never
//!   starts a sign-in: samael would skip signature verification;
//! - `saml_config` ([`SamlConfig`]) — our entity ID override, the IdP's SLO
//!   endpoint, NameID format, attribute names, IdP-initiated policy, clock
//!   skew, request signing, encryption policy, contact;
//! - its SP key pairs (`saml_sp_keys`), created on first use and stored like an
//!   OAuth client secret ([`crate::auth::secret`]). Every key is published in the
//!   metadata (signing and encryption) and tried for decryption; the current
//!   one signs. That is how a key rollover works.
//!
//! ## Endpoints (`/api/auth/saml/{slug}/…`)
//! - `metadata` — our SP metadata, whose URL is also our default entity ID;
//! - `login` — SP-initiated sign-in: an AuthnRequest (128-bit ID, NameID
//!   policy, optionally signed) over HTTP-Redirect, bound to the browser by a
//!   one-time RelayState key and the `saml_state` cookie;
//! - `acs` — the Assertion Consumer Service (HTTP-POST). It accepts a response
//!   only once, only from the browser that started the sign-in, answering that
//!   request, signed with SHA-256 or stronger, encrypted or not. IdP-initiated
//!   responses only when the provider allows them, and then each assertion once;
//! - `slo` — Single Logout in both directions (HTTP-Redirect or HTTP-POST).
//!
//! Signing out ends the refresh-token family the SAML sign-in issued.
//! Access tokens already issued stay valid until they expire
//! (`ACCESS_TOKEN_EXPIRY_MINUTES`): they are stateless JWTs.
//!
//! ## Transport
//! The IdP returns the browser to the ACS with a cross-site POST, which only a
//! `SameSite=None; Secure` cookie survives, so SAML needs an `https`
//! `BASE_URL`. Plain `http` works for a loopback `BASE_URL` only (local
//! development with an IdP on the same site).

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;

use super::db::AuthDb;
use super::jwt::JwtConfig;
use super::models::{OauthProvider, SamlConfig, SamlSession, SamlSpKey};

#[cfg(feature = "saml")]
mod sp;
#[cfg(feature = "saml")]
mod xml;

/// What a verified assertion says.
#[derive(Debug, Clone)]
pub struct SamlClaims {
    /// The account key: the NameID, or the configured subject attribute.
    pub subject: String,
    pub name_id: String,
    pub name_id_format: Option<String>,
    pub name_qualifier: Option<String>,
    pub sp_name_qualifier: Option<String>,
    pub session_index: Option<String>,
    pub assertion_id: String,
    pub not_on_or_after: Option<chrono::DateTime<chrono::Utc>>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub groups: Vec<String>,
}

/// The store's own endpoints towards one provider's IdP.
#[derive(Debug, Clone)]
pub struct SpUrls {
    pub entity_id: String,
    pub metadata: String,
    pub acs: String,
    pub slo: String,
}

/// What IdP metadata says, for the admin form.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IdpMetadata {
    pub entity_id: String,
    pub sso_url: String,
    pub slo_url: Option<String>,
    pub slo_response_url: Option<String>,
    /// Every signing certificate, as PEM, ready for `idp_certificate`.
    pub certificates_pem: String,
    /// Fingerprint, subject and expiry of each certificate.
    pub certificates: Vec<serde_json::Value>,
    pub want_authn_requests_signed: bool,
}

/// A verified logout message from the IdP.
#[derive(Debug, Clone)]
pub enum LogoutMessage {
    Request {
        id: String,
        name_id: String,
        session_indexes: Vec<String>,
        not_after: chrono::DateTime<chrono::Utc>,
    },
    Response {
        in_response_to: Option<String>,
        success: bool,
    },
}

fn api_base(base_url: &str, slug: &str) -> String {
    format!("{}/api/auth/saml/{slug}", base_url.trim_end_matches('/'))
}

/// The store's endpoints and entity ID towards `provider`'s IdP.
pub fn sp_urls(base_url: &str, provider: &OauthProvider) -> SpUrls {
    let base = api_base(base_url, &provider.slug);
    let metadata = format!("{base}/metadata");
    let entity_id = provider
        .saml_config
        .sp_entity_id
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| metadata.clone());
    SpUrls {
        entity_id,
        metadata,
        acs: format!("{base}/acs"),
        slo: format!("{base}/slo"),
    }
}

/// Whether `base_url` is https. The `saml_state` cookie is `SameSite=None;
/// Secure` exactly then.
pub fn base_url_is_https(base_url: &str) -> bool {
    url::Url::parse(base_url)
        .map(|u| u.scheme() == "https")
        .unwrap_or(false)
}

/// SAML needs HTTPS outside localhost: the IdP's cross-site POST to the ACS
/// carries the binding cookie only when it is `SameSite=None; Secure`.
pub fn check_transport(base_url: &str) -> anyhow::Result<()> {
    if base_url_is_https(base_url) || crate::auth::oidc_rs::is_secure_idp_url(base_url) {
        return Ok(());
    }
    anyhow::bail!(
        "SAML sign-in needs an https BASE_URL (plain http only on localhost); \
         BASE_URL is {base_url}"
    )
}

// ─── In-flight state ─────────────────────────────────────────────────────────

/// How long a sign-in may take between the redirect to the IdP and the ACS.
const PENDING_TTL: Duration = Duration::from_secs(600);
/// Hard cap on in-flight requests, so flooding the login route cannot grow
/// memory without bound (the route is also rate-limited).
const MAX_PENDING: usize = 10_000;
/// Hard cap on remembered assertion and logout-request IDs.
const MAX_REPLAY: usize = 100_000;

struct Pending {
    slug: String,
    request_id: String,
    created_at: Instant,
}

/// In-flight AuthnRequests keyed by RelayState, and LogoutRequests keyed by
/// their ID. Process-wide: the keys are random, and a request is only ever
/// answered at the instance that sent it.
static PENDING: LazyLock<DashMap<String, Pending>> = LazyLock::new(DashMap::new);
static PENDING_LOGOUT: LazyLock<DashMap<String, Pending>> = LazyLock::new(DashMap::new);

/// Assertion and LogoutRequest IDs already accepted, per provider, until they
/// expire, so neither can be presented twice.
static REPLAY: LazyLock<DashMap<(String, String), chrono::DateTime<chrono::Utc>>> =
    LazyLock::new(DashMap::new);

fn prune(map: &DashMap<String, Pending>) {
    map.retain(|_, p| p.created_at.elapsed() < PENDING_TTL);
    if map.len() >= MAX_PENDING {
        let mut entries: Vec<(String, Instant)> = map
            .iter()
            .map(|e| (e.key().clone(), e.value().created_at))
            .collect();
        entries.sort_by_key(|(_, t)| *t); // oldest first
        let to_remove = entries.len() + 1 - MAX_PENDING;
        for (k, _) in entries.into_iter().take(to_remove) {
            map.remove(&k);
        }
    }
}

/// Drop expired in-flight state. Called by the server's periodic pruning.
pub fn prune_state() {
    prune(&PENDING);
    prune(&PENDING_LOGOUT);
    let now = chrono::Utc::now();
    REPLAY.retain(|_, until| *until > now);
}

/// Consume the pending request filed under `relay_state` and return its ID,
/// if it was sent for `slug` and has not expired. Single use: a second call
/// with the same key returns `None`.
pub fn take_pending_request(relay_state: &str, slug: &str) -> Option<String> {
    let (_, pending) = PENDING.remove(relay_state)?;
    (pending.slug == slug && pending.created_at.elapsed() < PENDING_TTL)
        .then_some(pending.request_id)
}

#[cfg_attr(not(feature = "saml"), allow(dead_code))]
/// Record `id` as used at `provider_id` until `until`. Returns false when it
/// was already used. Refuses when the cache is full rather than forgetting.
fn first_use(provider_id: &str, id: &str, until: chrono::DateTime<chrono::Utc>) -> bool {
    let now = chrono::Utc::now();
    if REPLAY.len() >= MAX_REPLAY {
        REPLAY.retain(|_, u| *u > now);
        if REPLAY.len() >= MAX_REPLAY {
            return false;
        }
    }
    // Keep an entry at least a few minutes, at most a day.
    let until = until.clamp(
        now + chrono::Duration::minutes(5),
        now + chrono::Duration::hours(24),
    );
    use dashmap::mapref::entry::Entry;
    match REPLAY.entry((provider_id.to_string(), id.to_string())) {
        Entry::Occupied(mut e) if *e.get() <= now => {
            e.insert(until);
            true
        }
        Entry::Occupied(_) => false,
        Entry::Vacant(e) => {
            e.insert(until);
            true
        }
    }
}

// ─── SP keys ─────────────────────────────────────────────────────────────────

/// The common name on a generated SP certificate: the store's host.
fn key_common_name(base_url: &str, provider: &OauthProvider) -> String {
    let host = url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "open-triplestore".to_string());
    format!("{host} SAML SP {}", provider.slug)
}

/// Create a new SP key for `provider` (current if it has none yet).
pub fn generate_sp_key(
    auth_db: &AuthDb,
    provider: &OauthProvider,
    base_url: &str,
    jwt_secret: &str,
) -> anyhow::Result<SamlSpKey> {
    #[cfg(feature = "saml")]
    {
        let (pem, cert_der) = sp::generate_key(&key_common_name(base_url, provider))?;
        // A generated key has no outside home to reference, so it is stored
        // encrypted, like the store's own OIDC signing keys.
        let stored = crate::auth::secret::encrypt_secret(&pem, jwt_secret)?;
        use base64::Engine as _;
        let cert = base64::engine::general_purpose::STANDARD.encode(&cert_der);
        auth_db.create_saml_sp_key(&provider.id, &new_kid(), &stored, &cert)
    }
    #[cfg(not(feature = "saml"))]
    {
        let _ = (auth_db, provider, base_url, jwt_secret, key_common_name);
        anyhow::bail!(NOT_COMPILED)
    }
}

/// Add an SP key an admin supplies: `private_key` is a secret reference
/// (`env:`, `file:`, `vault:`) or, outside the production posture, a PEM key;
/// `certificate` is its PEM certificate.
pub fn import_sp_key(
    auth_db: &AuthDb,
    provider: &OauthProvider,
    private_key: &str,
    certificate: &str,
    jwt_secret: &str,
) -> anyhow::Result<SamlSpKey> {
    #[cfg(feature = "saml")]
    {
        let cert_der = sp::parse_one_certificate(certificate)?;
        let stored = crate::auth::secret::store_configured_secret(
            "saml_sp_private_key",
            private_key,
            jwt_secret,
        )?;
        let pem = crate::auth::secret::read_stored_secret(&stored, jwt_secret)?;
        sp::load_key(&pem, &cert_der)?;
        use base64::Engine as _;
        let cert = base64::engine::general_purpose::STANDARD.encode(&cert_der);
        auth_db.create_saml_sp_key(&provider.id, &new_kid(), &stored, &cert)
    }
    #[cfg(not(feature = "saml"))]
    {
        let _ = (auth_db, provider, private_key, certificate, jwt_secret);
        anyhow::bail!(NOT_COMPILED)
    }
}

#[cfg(feature = "saml")]
fn new_kid() -> String {
    format!("sp-{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

#[cfg(not(feature = "saml"))]
const NOT_COMPILED: &str = "SAML support is not compiled in (enable the 'saml' feature)";

/// The provider's keys, decrypted; creates the first one when it has none.
#[cfg(feature = "saml")]
fn load_keys(
    auth_db: &AuthDb,
    provider: &OauthProvider,
    base_url: &str,
    jwt_secret: &str,
) -> anyhow::Result<Vec<sp::SpKey>> {
    let mut rows = auth_db.list_saml_sp_keys(&provider.id)?;
    if rows.is_empty() {
        generate_sp_key(auth_db, provider, base_url, jwt_secret)?;
        rows = auth_db.list_saml_sp_keys(&provider.id)?;
    }
    use base64::Engine as _;
    rows.into_iter()
        .map(|row| {
            let cert_der = base64::engine::general_purpose::STANDARD.decode(&row.certificate)?;
            let pem = crate::auth::secret::read_stored_secret(&row.private_key_enc, jwt_secret)?;
            Ok(sp::SpKey {
                kid: row.kid,
                pkey: sp::load_key(&pem, &cert_der)?,
                cert_der,
            })
        })
        .collect()
}

#[cfg(feature = "saml")]
fn engine<'a>(
    auth_db: &AuthDb,
    provider: &'a OauthProvider,
    base_url: &str,
    jwt_secret: &str,
) -> anyhow::Result<sp::Sp<'a>> {
    Ok(sp::Sp {
        provider,
        urls: sp_urls(base_url, provider),
        keys: load_keys(auth_db, provider, base_url, jwt_secret)?,
    })
}

// ─── Admin helpers ───────────────────────────────────────────────────────────

/// Check the SAML fields of a provider an admin is saving: the settings, and
/// (in a build with SAML) that every IdP certificate parses.
pub fn validate_provider_fields(
    idp_certificate: Option<&str>,
    config: Option<&SamlConfig>,
) -> Result<(), String> {
    if let Some(config) = config {
        config.validate()?;
    }
    #[cfg(feature = "saml")]
    if let Some(certs) = idp_certificate.filter(|c| !c.trim().is_empty()) {
        sp::parse_certificates(certs).map_err(|e| e.to_string())?;
    }
    #[cfg(not(feature = "saml"))]
    let _ = idp_certificate;
    Ok(())
}

/// Read IdP metadata (pasted XML) for the admin form.
pub fn read_idp_metadata(xml_text: &str) -> anyhow::Result<IdpMetadata> {
    #[cfg(feature = "saml")]
    {
        sp::parse_idp_metadata(xml_text)
    }
    #[cfg(not(feature = "saml"))]
    {
        let _ = xml_text;
        anyhow::bail!(NOT_COMPILED)
    }
}

/// Fetch IdP metadata from an https URL (no redirects, 1 MiB, 15 s) and read it.
pub async fn fetch_idp_metadata(url: &str) -> anyhow::Result<IdpMetadata> {
    if !crate::auth::oidc_rs::is_secure_idp_url(url) {
        anyhow::bail!("the metadata URL must use https (http only for localhost)");
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()?;
    let mut resp = client
        .get(url)
        .header(
            reqwest::header::ACCEPT,
            "application/samlmetadata+xml, application/xml, text/xml",
        )
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("could not fetch the metadata: {e}"))?;
    if !resp.status().is_success() {
        anyhow::bail!(
            "the metadata URL answered {} (redirects are not followed)",
            resp.status()
        );
    }
    let limit = 1usize << 20;
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > limit {
            anyhow::bail!("the metadata is larger than 1 MiB");
        }
    }
    let text = String::from_utf8(body).map_err(|_| anyhow::anyhow!("the metadata is not UTF-8"))?;
    read_idp_metadata(&text)
}

/// What the admin UI shows about a provider's SAML side: our URLs, our keys
/// and the IdP certificates.
pub fn sp_overview(
    auth_db: &AuthDb,
    provider: &OauthProvider,
    base_url: &str,
) -> anyhow::Result<serde_json::Value> {
    let urls = sp_urls(base_url, provider);
    let keys = auth_db.list_saml_sp_keys(&provider.id)?;
    #[cfg(feature = "saml")]
    let (key_info, idp_certs): (Vec<serde_json::Value>, serde_json::Value) = {
        use base64::Engine as _;
        let key_info = keys
            .iter()
            .map(|k| {
                let der = base64::engine::general_purpose::STANDARD
                    .decode(&k.certificate)
                    .unwrap_or_default();
                serde_json::json!({
                    "kid": k.kid,
                    "is_current": k.is_current,
                    "created_at": k.created_at,
                    "certificate": sp::certificate_info(&der),
                })
            })
            .collect();
        let idp_certs =
            match sp::parse_certificates(provider.idp_certificate.as_deref().unwrap_or_default()) {
                Ok(certs) => serde_json::json!(certs
                    .iter()
                    .map(|c| sp::certificate_info(c))
                    .collect::<Vec<_>>()),
                Err(e) => serde_json::json!({ "error": e.to_string() }),
            };
        (key_info, idp_certs)
    };
    #[cfg(not(feature = "saml"))]
    let (key_info, idp_certs): (Vec<serde_json::Value>, serde_json::Value) = (
        keys.iter()
            .map(|k| serde_json::json!({"kid": k.kid, "is_current": k.is_current, "created_at": k.created_at}))
            .collect(),
        serde_json::Value::Null,
    );
    Ok(serde_json::json!({
        "compiled": cfg!(feature = "saml"),
        "entity_id": urls.entity_id,
        "metadata_url": urls.metadata,
        "acs_url": urls.acs,
        "slo_url": urls.slo,
        "transport_ok": check_transport(base_url).is_ok(),
        "sp_keys": key_info,
        "idp_certificates": idp_certs,
    }))
}

// ─── Flows ───────────────────────────────────────────────────────────────────

/// The SP metadata XML for `provider`.
pub fn generate_sp_metadata(
    auth_db: &AuthDb,
    provider: &OauthProvider,
    base_url: &str,
    jwt_secret: &str,
) -> anyhow::Result<String> {
    #[cfg(feature = "saml")]
    {
        Ok(sp::metadata_xml(&engine(
            auth_db, provider, base_url, jwt_secret,
        )?))
    }
    #[cfg(not(feature = "saml"))]
    {
        let _ = (auth_db, provider, base_url, jwt_secret);
        anyhow::bail!(NOT_COMPILED)
    }
}

/// Start an SP-initiated sign-in: build the redirect to the IdP and file the
/// request under a fresh RelayState key. Returns `(redirect_url, relay_state)`;
/// the caller binds `relay_state` to the browser.
pub fn begin_saml_flow(
    auth_db: &AuthDb,
    provider: &OauthProvider,
    base_url: &str,
    jwt_secret: &str,
) -> anyhow::Result<(String, String)> {
    check_transport(base_url)?;
    #[cfg(feature = "saml")]
    {
        let relay_state = uuid::Uuid::new_v4().simple().to_string();
        let engine = engine(auth_db, provider, base_url, jwt_secret)?;
        let (url, request_id) = sp::authn_request_url(&engine, &relay_state)?;
        prune(&PENDING);
        PENDING.insert(
            relay_state.clone(),
            Pending {
                slug: provider.slug.clone(),
                request_id,
                created_at: Instant::now(),
            },
        );
        Ok((url, relay_state))
    }
    #[cfg(not(feature = "saml"))]
    {
        let _ = (auth_db, provider, jwt_secret);
        anyhow::bail!(NOT_COMPILED)
    }
}

/// Verify a SAML response at the ACS. `request_id` is the AuthnRequest it
/// must answer, or `None` for an IdP-initiated response (the caller checks the
/// provider allows those). Each assertion is accepted once.
pub fn verify_saml_response(
    auth_db: &AuthDb,
    provider: &OauthProvider,
    base_url: &str,
    jwt_secret: &str,
    saml_response_b64: &str,
    request_id: Option<&str>,
) -> anyhow::Result<SamlClaims> {
    #[cfg(feature = "saml")]
    {
        let engine = engine(auth_db, provider, base_url, jwt_secret)?;
        let claims = sp::parse_response(&engine, saml_response_b64, request_id)?;
        let until = claims
            .not_on_or_after
            .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::minutes(10))
            + chrono::Duration::seconds(i64::from(provider.saml_config.clock_skew()));
        if !first_use(&provider.id, &claims.assertion_id, until) {
            anyhow::bail!("this assertion has already been used");
        }
        Ok(claims)
    }
    #[cfg(not(feature = "saml"))]
    {
        let _ = (
            auth_db,
            provider,
            base_url,
            jwt_secret,
            saml_response_b64,
            request_id,
        );
        anyhow::bail!(NOT_COMPILED)
    }
}

/// Sign in the subject of verified `claims` and return `(access, refresh)`.
pub fn complete_saml_flow(
    claims: &SamlClaims,
    provider: &OauthProvider,
    auth_db: &Arc<AuthDb>,
    jwt_config: &JwtConfig,
) -> anyhow::Result<(String, String)> {
    use super::jwt::{hash_token, issue_access_token, issue_refresh_token};
    use super::models::{map_claims_to_role, SystemRole};
    use super::oauth::provision_or_link_user;
    use uuid::Uuid;

    // Map SAML group attributes → role + capabilities. The strongest matched
    // role wins; absent that, fall back to the provider default. As with OIDC,
    // this sets the role of a new account only: a returning user keeps theirs.
    let mapped = map_claims_to_role(&claims.groups, provider.role_claim_map.as_deref());
    let default_role = SystemRole::from_str(&provider.default_role).unwrap_or(SystemRole::User);
    let best_role = match mapped.role {
        Some(role) if role.level() > default_role.level() => role,
        _ => default_role,
    };
    // SSO must never grant super_admin (see oauth::derive_grants_from_claims).
    let best_role = if best_role == SystemRole::SuperAdmin {
        SystemRole::Admin
    } else {
        best_role
    };

    let display = claims
        .display_name
        .as_deref()
        .or(claims.email.as_deref())
        .unwrap_or(&claims.subject);

    // SAML has no standard `email_verified` assertion, and an IdP that does not
    // verify email ownership would otherwise enable account takeover by email. So
    // SAML logins are NOT auto-linked to an existing local account by email; users
    // link SAML from their authenticated settings instead (or match on subject).
    let mut user = provision_or_link_user(
        &claims.subject,
        claims.email.as_deref(),
        false, // email_verified
        display,
        best_role,
        provider,
        auth_db,
    )?;
    if !user.is_active {
        anyhow::bail!("User account is deactivated");
    }

    // Grant the publish capability when a SAML group maps to "publisher".
    // Non-destructive: only ever sets the flag, never revokes it.
    if mapped.grant_publish && !user.can_publish {
        auth_db.update_user_can_publish(&user.id, true)?;
        user.can_publish = true;
    }

    let access = issue_access_token(jwt_config, &user.id, &user.username, user.role.as_str())?;
    let refresh = issue_refresh_token(jwt_config, &user.id, &user.username, user.role.as_str())?;
    let refresh_hash = hash_token(&refresh);
    let refresh_id = Uuid::new_v4().to_string();
    // A fresh SAML login starts its own session family, which Single Logout
    // revokes as a whole.
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
    auth_db.create_saml_session(&SamlSession {
        family_id,
        provider_id: provider.id.clone(),
        user_id: user.id.clone(),
        name_id: claims.name_id.clone(),
        name_id_format: claims.name_id_format.clone(),
        name_qualifier: claims.name_qualifier.clone(),
        sp_name_qualifier: claims.sp_name_qualifier.clone(),
        session_index: claims.session_index.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    })?;

    Ok((access, refresh))
}

/// Sign out a refresh-token family that a SAML sign-in issued: revoke the
/// whole family and, when the IdP has an SLO endpoint, return the redirect
/// that carries our LogoutRequest to it. `Ok(None)` for a family that did not
/// come from SAML (nothing is revoked then) or an IdP without SLO.
pub fn begin_saml_logout(
    auth_db: &AuthDb,
    family_id: &str,
    base_url: &str,
    jwt_secret: &str,
) -> anyhow::Result<Option<String>> {
    let Some(session) = auth_db.get_saml_session(family_id)? else {
        return Ok(None);
    };
    auth_db.revoke_refresh_token_family(family_id)?;
    auth_db.delete_saml_session(family_id)?;
    let Some(provider) = auth_db.get_oauth_provider_by_id(&session.provider_id)? else {
        return Ok(None);
    };
    if !provider.is_active || provider.provider_type != "saml" {
        return Ok(None);
    }
    #[cfg(feature = "saml")]
    {
        let engine = engine(auth_db, &provider, base_url, jwt_secret)?;
        let Some((url, id)) = sp::logout_request_url(&engine, &session)? else {
            return Ok(None);
        };
        prune(&PENDING_LOGOUT);
        PENDING_LOGOUT.insert(
            id.clone(),
            Pending {
                slug: provider.slug.clone(),
                request_id: id,
                created_at: Instant::now(),
            },
        );
        Ok(Some(url))
    }
    #[cfg(not(feature = "saml"))]
    {
        let _ = (base_url, jwt_secret);
        Ok(None)
    }
}

/// How the IdP delivered a logout message.
pub enum SloBinding<'a> {
    /// HTTP-Redirect: the raw query string, exactly as received.
    Redirect(&'a str),
    /// HTTP-POST: the form fields.
    Post {
        message_b64: &'a str,
        relay_state: Option<&'a str>,
    },
}

/// The outcome of a message at the SLO endpoint.
pub enum SloOutcome {
    /// The IdP asked us to end sessions: `revoked` families were revoked;
    /// send the browser to `response_url` with our LogoutResponse (if the IdP
    /// has an SLO endpoint to send it to).
    LoggedOut {
        revoked: usize,
        user_ids: Vec<String>,
        response_url: Option<String>,
    },
    /// The IdP answered our LogoutRequest.
    Answered { success: bool },
}

/// Handle a LogoutRequest from the IdP or its LogoutResponse to ours. Both
/// must be signed by the IdP. A request revokes the refresh-token families of
/// the named subject (only those with a listed SessionIndex, when it lists any).
pub fn handle_slo(
    auth_db: &AuthDb,
    provider: &OauthProvider,
    base_url: &str,
    jwt_secret: &str,
    binding: SloBinding<'_>,
) -> anyhow::Result<SloOutcome> {
    #[cfg(feature = "saml")]
    {
        let engine = engine(auth_db, provider, base_url, jwt_secret)?;
        let (signed_xml, relay_state) = match binding {
            SloBinding::Redirect(raw_query) => {
                let param = if raw_query.split('&').any(|p| p.starts_with("SAMLRequest=")) {
                    "SAMLRequest"
                } else {
                    "SAMLResponse"
                };
                sp::verify_redirect(provider, raw_query, param)?
            }
            SloBinding::Post {
                message_b64,
                relay_state,
            } => (
                sp::verify_post(provider, message_b64)?,
                relay_state.map(str::to_string),
            ),
        };
        match sp::read_logout_message(&engine, &signed_xml)? {
            LogoutMessage::Request {
                id,
                name_id,
                session_indexes,
                not_after,
            } => {
                if !first_use(&provider.id, &id, not_after) {
                    anyhow::bail!("this LogoutRequest has already been processed");
                }
                let mut revoked = 0;
                let mut user_ids = Vec::new();
                for s in auth_db.saml_sessions_for_subject(&provider.id, &name_id)? {
                    let listed = session_indexes.is_empty()
                        || s.session_index
                            .as_ref()
                            .is_some_and(|i| session_indexes.contains(i));
                    if listed {
                        auth_db.revoke_refresh_token_family(&s.family_id)?;
                        auth_db.delete_saml_session(&s.family_id)?;
                        revoked += 1;
                        if !user_ids.contains(&s.user_id) {
                            user_ids.push(s.user_id);
                        }
                    }
                }
                let response_url = sp::logout_response_url(&engine, &id, relay_state.as_deref())?;
                Ok(SloOutcome::LoggedOut {
                    revoked,
                    user_ids,
                    response_url,
                })
            }
            LogoutMessage::Response {
                in_response_to,
                success,
            } => {
                let pending = in_response_to
                    .as_deref()
                    .and_then(|id| PENDING_LOGOUT.remove(id))
                    .map(|(_, p)| p);
                match pending {
                    Some(p) if p.slug == provider.slug && p.created_at.elapsed() < PENDING_TTL => {
                        Ok(SloOutcome::Answered { success })
                    }
                    _ => anyhow::bail!("the LogoutResponse answers no logout this store sent"),
                }
            }
        }
    }
    #[cfg(not(feature = "saml"))]
    {
        let _ = (auth_db, provider, base_url, jwt_secret, binding);
        anyhow::bail!(NOT_COMPILED)
    }
}
