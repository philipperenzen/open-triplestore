//! SAML 2.0 Service Provider implementation.
//!
//! Enabled by the `saml` feature flag.  When the feature is disabled, the
//! module still compiles but all public functions return an "unsupported"
//! error at runtime so that the binary can be built without OpenSSL.
//!
//! ## Configuration per provider
//! - `entity_id`       — IdP entity ID (from IdP metadata)
//! - `sso_url`         — IdP SSO redirect URL
//! - `idp_certificate` — IdP signing certificate (PEM)
//!
//! ## SP metadata
//! The store's own entity ID for a provider is its metadata URL,
//! `GET /api/auth/saml/{slug}/metadata`; register that and the ACS URL
//! `POST /api/auth/saml/{slug}/acs` with the IdP.
//!
//! ## Flow (SP-initiated only)
//! [`begin_saml_flow`] sends the browser to the IdP with an AuthnRequest
//! (HTTP-Redirect binding, unsigned) and remembers its ID under a one-time
//! RelayState key. The ACS takes that key back ([`take_pending_request`]) and
//! accepts only a signed response whose `InResponseTo` is that request's ID, so
//! an IdP-initiated or replayed response is refused.

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;

use super::db::AuthDb;
use super::jwt::JwtConfig;
use super::models::OauthProvider;

/// Claims extracted from a verified SAML assertion.
#[derive(Debug)]
pub struct SamlClaims {
    pub name_id: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub groups: Vec<String>,
}

/// The store's entity ID towards a provider's IdP: its SP metadata URL.
pub fn sp_entity_id(base_url: &str, slug: &str) -> String {
    format!(
        "{}/api/auth/saml/{slug}/metadata",
        base_url.trim_end_matches('/')
    )
}

/// The Assertion Consumer Service URL registered with a provider's IdP.
pub fn acs_url(base_url: &str, slug: &str) -> String {
    format!(
        "{}/api/auth/saml/{slug}/acs",
        base_url.trim_end_matches('/')
    )
}

// ─── Pending AuthnRequests ────────────────────────────────────────────────────

/// How long a sign-in may take between the redirect to the IdP and the ACS.
const PENDING_TTL: Duration = Duration::from_secs(600);
/// Hard cap on in-flight requests, so flooding the login route cannot grow
/// memory without bound (the route is also rate-limited).
const MAX_PENDING: usize = 10_000;

struct PendingRequest {
    slug: String,
    request_id: String,
    created_at: Instant,
}

/// In-flight AuthnRequests keyed by RelayState. Process-wide: the keys are
/// random, and a request is only ever answered at the instance that sent it.
static PENDING: LazyLock<DashMap<String, PendingRequest>> = LazyLock::new(DashMap::new);

fn prune_pending() {
    PENDING.retain(|_, p| p.created_at.elapsed() < PENDING_TTL);
    if PENDING.len() >= MAX_PENDING {
        let mut entries: Vec<(String, Instant)> = PENDING
            .iter()
            .map(|e| (e.key().clone(), e.value().created_at))
            .collect();
        entries.sort_by_key(|(_, t)| *t); // oldest first
        let to_remove = entries.len() + 1 - MAX_PENDING;
        for (k, _) in entries.into_iter().take(to_remove) {
            PENDING.remove(&k);
        }
    }
}

/// Consume the pending request filed under `relay_state` and return its ID,
/// if it was sent for `slug` and has not expired. Single use: a second call
/// with the same key returns `None`.
pub fn take_pending_request(relay_state: &str, slug: &str) -> Option<String> {
    let (_, pending) = PENDING.remove(relay_state)?;
    (pending.slug == slug && pending.created_at.elapsed() < PENDING_TTL)
        .then_some(pending.request_id)
}

// ─── Feature-gated implementation ────────────────────────────────────────────

#[cfg(feature = "saml")]
mod inner {
    use super::*;
    use samael::metadata::EntityDescriptor;
    use samael::service_provider::{ServiceProvider, ServiceProviderBuilder};
    use samael::traits::ToXml;

    /// Strip PEM armor and whitespace from a certificate, yielding the bare
    /// base64 DER body that goes inside an XML `<ds:X509Certificate>` element.
    /// Input that is already a bare base64 blob passes through unchanged.
    fn pem_to_base64_der(pem: &str) -> String {
        pem.lines()
            .filter(|l| !l.trim_start().starts_with("-----"))
            .flat_map(|l| l.split_whitespace())
            .collect::<String>()
    }

    fn build_sp(
        provider: &OauthProvider,
        sp_entity_id: &str,
        acs_url: &str,
    ) -> anyhow::Result<ServiceProvider> {
        let cert_pem = provider.idp_certificate.as_deref().ok_or_else(|| {
            anyhow::anyhow!("Provider '{}' has no idp_certificate", provider.slug)
        })?;

        let entity_id = provider
            .entity_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' has no entity_id", provider.slug))?;

        let sso_url = provider.sso_url.as_deref().unwrap_or("");

        // samael 0.0.18 reads the IdP signing certificate from `idp_metadata`
        // (an EntityDescriptor) rather than via a raw-PEM builder setter. Build
        // minimal IdP metadata embedding the base64 DER cert and parse it.
        let cert_b64 = pem_to_base64_der(cert_pem);
        let idp_metadata_xml = format!(
            "<md:EntityDescriptor xmlns:md=\"urn:oasis:names:tc:SAML:2.0:metadata\" entityID=\"{entity_id}\">\
               <md:IDPSSODescriptor protocolSupportEnumeration=\"urn:oasis:names:tc:SAML:2.0:protocol\">\
                 <md:KeyDescriptor use=\"signing\">\
                   <ds:KeyInfo xmlns:ds=\"http://www.w3.org/2000/09/xmldsig#\">\
                     <ds:X509Data><ds:X509Certificate>{cert_b64}</ds:X509Certificate></ds:X509Data>\
                   </ds:KeyInfo>\
                 </md:KeyDescriptor>\
                 <md:SingleSignOnService Binding=\"urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect\" Location=\"{sso_url}\"/>\
               </md:IDPSSODescriptor>\
             </md:EntityDescriptor>"
        );
        let idp_metadata: EntityDescriptor = idp_metadata_xml.parse().map_err(|e| {
            anyhow::anyhow!("Failed to build IdP metadata for '{}': {e}", provider.slug)
        })?;

        // The SP's own entity ID (the audience the IdP must assert), not the
        // IdP's: that one is the expected `Issuer`, carried in `idp_metadata`.
        let sp = ServiceProviderBuilder::default()
            .entity_id(sp_entity_id.to_string())
            .acs_url(acs_url.to_string())
            .idp_metadata(idp_metadata)
            .build()
            .map_err(|e| anyhow::anyhow!("SAML SP build error: {e}"))?;

        Ok(sp)
    }

    /// Generate SP metadata XML for registration with the IdP.
    pub fn generate_sp_metadata(
        provider: &OauthProvider,
        sp_entity_id: &str,
        acs_url: &str,
    ) -> anyhow::Result<String> {
        let sp = build_sp(provider, sp_entity_id, acs_url)?;
        let metadata = sp
            .metadata()
            .map_err(|e| anyhow::anyhow!("metadata error: {e}"))?;
        metadata
            .to_string()
            .map_err(|e| anyhow::anyhow!("metadata serialization error: {e}"))
    }

    /// Build the HTTP-Redirect URL that carries a fresh AuthnRequest to the
    /// IdP's SSO URL. Returns `(url, request_id)`.
    pub fn authn_request_url(
        provider: &OauthProvider,
        sp_entity_id: &str,
        acs_url: &str,
        relay_state: &str,
    ) -> anyhow::Result<(String, String)> {
        let sso_url = provider
            .sso_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' has no sso_url", provider.slug))?;
        if !crate::auth::oidc_rs::is_secure_idp_url(sso_url) {
            anyhow::bail!(
                "SAML provider '{}' sso_url must use https: {sso_url}",
                provider.slug
            );
        }
        let sp = build_sp(provider, sp_entity_id, acs_url)?;
        let mut request = sp
            .make_authentication_request(sso_url)
            .map_err(|e| anyhow::anyhow!("AuthnRequest error: {e}"))?;
        // samael's default ID carries 32 random bits; the ACS binds the response
        // to this ID, so make it unguessable. An xs:ID must not start with a digit.
        request.id = format!("_{}", uuid::Uuid::new_v4().simple());
        let url = request
            .redirect(relay_state)
            .map_err(|e| anyhow::anyhow!("AuthnRequest encoding error: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("AuthnRequest has no destination"))?;
        Ok((url.to_string(), request.id))
    }

    /// Verify and parse a base64-encoded SAML response from the IdP. The
    /// response must answer `request_id` (`InResponseTo`); IdP-initiated
    /// responses are refused.
    pub fn parse_saml_response(
        saml_response_b64: &str,
        provider: &OauthProvider,
        sp_entity_id: &str,
        acs_url: &str,
        request_id: &str,
    ) -> anyhow::Result<SamlClaims> {
        let sp = build_sp(provider, sp_entity_id, acs_url)?;

        let assertion = sp
            .parse_base64_response(saml_response_b64, Some(&[request_id]))
            .map_err(|e| anyhow::anyhow!("SAML parse error: {e}"))?;

        let name_id = assertion
            .subject
            .as_ref()
            .and_then(|s| s.name_id.as_ref())
            .map(|n| n.value.clone())
            .ok_or_else(|| anyhow::anyhow!("SAML assertion missing NameID"))?;

        let mut email = None;
        let mut display_name = None;
        let mut groups = Vec::new();

        for attr_stmt in assertion.attribute_statements.unwrap_or_default() {
            for attr in attr_stmt.attributes {
                let name = attr.name.as_deref().unwrap_or("");
                let values: Vec<String> = attr.values.into_iter().filter_map(|v| v.value).collect();
                match name {
                    "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress"
                    | "email"
                    | "mail" => {
                        email = values.into_iter().next();
                    }
                    "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name"
                    | "displayName"
                    | "cn" => {
                        display_name = values.into_iter().next();
                    }
                    "http://schemas.microsoft.com/ws/2008/06/identity/claims/groups"
                    | "memberOf"
                    | "groups" => {
                        groups.extend(values);
                    }
                    _ => {}
                }
            }
        }

        Ok(SamlClaims {
            name_id,
            email,
            display_name,
            groups,
        })
    }
}

#[cfg(not(feature = "saml"))]
mod inner {
    use super::*;

    pub fn generate_sp_metadata(
        _provider: &OauthProvider,
        _sp_entity_id: &str,
        _acs_url: &str,
    ) -> anyhow::Result<String> {
        anyhow::bail!("SAML support is not compiled in (enable the 'saml' feature)")
    }

    pub fn authn_request_url(
        _provider: &OauthProvider,
        _sp_entity_id: &str,
        _acs_url: &str,
        _relay_state: &str,
    ) -> anyhow::Result<(String, String)> {
        anyhow::bail!("SAML support is not compiled in (enable the 'saml' feature)")
    }

    pub fn parse_saml_response(
        _saml_response_b64: &str,
        _provider: &OauthProvider,
        _sp_entity_id: &str,
        _acs_url: &str,
        _request_id: &str,
    ) -> anyhow::Result<SamlClaims> {
        anyhow::bail!("SAML support is not compiled in (enable the 'saml' feature)")
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

pub use inner::{authn_request_url, generate_sp_metadata, parse_saml_response};

/// Start an SP-initiated sign-in: build the redirect to the IdP and file the
/// request under a fresh RelayState key. Returns `(redirect_url, relay_state)`;
/// the caller binds `relay_state` to the browser.
pub fn begin_saml_flow(
    provider: &OauthProvider,
    base_url: &str,
) -> anyhow::Result<(String, String)> {
    let relay_state = uuid::Uuid::new_v4().simple().to_string();
    let (url, request_id) = authn_request_url(
        provider,
        &sp_entity_id(base_url, &provider.slug),
        &acs_url(base_url, &provider.slug),
        &relay_state,
    )?;
    prune_pending();
    PENDING.insert(
        relay_state.clone(),
        PendingRequest {
            slug: provider.slug.clone(),
            request_id,
            created_at: Instant::now(),
        },
    );
    Ok((url, relay_state))
}

/// Process a SAML ACS POST answering `request_id` and return
/// `(access_token, refresh_token)`.
pub async fn complete_saml_flow(
    saml_response_b64: &str,
    request_id: &str,
    provider: &OauthProvider,
    base_url: &str,
    auth_db: &Arc<AuthDb>,
    jwt_config: &JwtConfig,
) -> anyhow::Result<(String, String)> {
    let claims = parse_saml_response(
        saml_response_b64,
        provider,
        &sp_entity_id(base_url, &provider.slug),
        &acs_url(base_url, &provider.slug),
        request_id,
    )?;

    use super::jwt::{hash_token, issue_access_token, issue_refresh_token};
    use super::models::{map_claims_to_role, SystemRole};
    use super::oauth::provision_or_link_user;
    use uuid::Uuid;

    // Map SAML group attributes → role + capabilities. The strongest matched
    // role wins; absent that, fall back to the provider default.
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
        .unwrap_or(&claims.name_id);

    // SAML has no standard `email_verified` assertion, and an IdP that does not
    // verify email ownership would otherwise enable account takeover by email. So
    // SAML logins are NOT auto-linked to an existing local account by email; users
    // link SAML from their authenticated settings instead (or match on name_id).
    let mut user = provision_or_link_user(
        &claims.name_id,
        claims.email.as_deref(),
        false, // email_verified
        display,
        best_role,
        provider,
        auth_db,
    )?;

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
    // A fresh SAML login starts its own session family.
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
