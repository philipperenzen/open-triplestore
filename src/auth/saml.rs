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
//! Expose `GET /api/auth/saml/{slug}/metadata` and register the ACS URL
//! `POST /api/auth/saml/{slug}/acs` with the IdP.

use std::sync::Arc;

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

    fn build_sp(provider: &OauthProvider, acs_url: &str) -> anyhow::Result<ServiceProvider> {
        let cert_pem = provider
            .idp_certificate
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' has no idp_certificate", provider.slug))?;

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
        let idp_metadata: EntityDescriptor = idp_metadata_xml
            .parse()
            .map_err(|e| anyhow::anyhow!("Failed to build IdP metadata for '{}': {e}", provider.slug))?;

        let sp = ServiceProviderBuilder::default()
            .entity_id(entity_id.to_string())
            .acs_url(acs_url.to_string())
            .idp_metadata(idp_metadata)
            .build()
            .map_err(|e| anyhow::anyhow!("SAML SP build error: {e}"))?;

        Ok(sp)
    }

    /// Generate SP metadata XML for registration with the IdP.
    pub fn generate_sp_metadata(provider: &OauthProvider, acs_url: &str) -> anyhow::Result<String> {
        let sp = build_sp(provider, acs_url)?;
        let metadata = sp
            .metadata()
            .map_err(|e| anyhow::anyhow!("metadata error: {e}"))?;
        metadata
            .to_string()
            .map_err(|e| anyhow::anyhow!("metadata serialization error: {e}"))
    }

    /// Verify and parse a base64-encoded SAML response from the IdP.
    pub fn parse_saml_response(
        saml_response_b64: &str,
        provider: &OauthProvider,
        acs_url: &str,
    ) -> anyhow::Result<SamlClaims> {
        let sp = build_sp(provider, acs_url)?;

        let assertion = sp
            .parse_base64_response(saml_response_b64, Some(&["urn:oasis:names:tc:SAML:2.0:cm:bearer"]))
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
                let values: Vec<String> = attr
                    .values
                    .into_iter()
                    .filter_map(|v| v.value)
                    .collect();
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

    pub fn generate_sp_metadata(_provider: &OauthProvider, _acs_url: &str) -> anyhow::Result<String> {
        anyhow::bail!("SAML support is not compiled in (enable the 'saml' feature)")
    }

    pub fn parse_saml_response(
        _saml_response_b64: &str,
        _provider: &OauthProvider,
        _acs_url: &str,
    ) -> anyhow::Result<SamlClaims> {
        anyhow::bail!("SAML support is not compiled in (enable the 'saml' feature)")
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

pub use inner::{generate_sp_metadata, parse_saml_response};

/// Process a SAML ACS POST and return `(access_token, refresh_token)`.
pub async fn complete_saml_flow(
    saml_response_b64: &str,
    provider: &OauthProvider,
    acs_url: &str,
    auth_db: &Arc<AuthDb>,
    jwt_config: &JwtConfig,
) -> anyhow::Result<(String, String)> {
    let claims = parse_saml_response(saml_response_b64, provider, acs_url)?;

    use super::oauth::provision_or_link_user;
    use super::models::{map_claims_to_role, SystemRole};
    use super::jwt::{issue_access_token, issue_refresh_token, hash_token};
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
    let best_role = if best_role == SystemRole::SuperAdmin { SystemRole::Admin } else { best_role };

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
    let expires =
        chrono::Utc::now() + chrono::Duration::days(jwt_config.refresh_expiry_days as i64);
    auth_db.create_refresh_token(&refresh_id, &user.id, &refresh_hash, &expires.to_rfc3339())?;

    Ok((access, refresh))
}
