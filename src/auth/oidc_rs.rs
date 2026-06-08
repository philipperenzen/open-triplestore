//! OIDC **resource-server** token verification (Unified Accounts plan, Phase 1,
//! Workstream C).
//!
//! This is distinct from [`super::oauth`], which implements the *login* (RP /
//! authorization-code) flow. Here OTS acts as a resource server: it accepts an
//! access token that a client (e.g. a single-page frontend app) already obtained directly from
//! the corporate IdP, verifies its signature against the IdP's JWKS, checks
//! `iss`/`aud`/`exp`, and JIT-provisions a local user keyed on the token's
//! subject — reusing [`super::oauth::provision_or_link_user`].
//!
//! Everything is gated by env config and disabled by default, so existing
//! password/PAT auth is unaffected until an IdP is wired up:
//! - `OIDC_ISSUER` — issuer URL, e.g. `https://idp/realms/example` (enables this)
//! - `OIDC_AUDIENCE` — expected `aud` (REQUIRED once `OIDC_ISSUER` is set; tokens are rejected until it is configured)
//! - `OIDC_DEFAULT_ROLE` — role for newly provisioned users (default `user`)
//! - `ACCEPT_LEGACY_TOKENS` — keep accepting password-session JWTs + `ots_` PATs (default true)

use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;

use super::db::AuthDb;
use super::models::{
    map_claims_to_role, OauthProvider, OauthProviderCreate, Role, SystemRole, User,
};
use super::oauth::provision_or_link_user;

/// Slug of the synthetic OAuth provider row used to anchor env-driven OIDC
/// identities (the `oauth_identities` table FK-references a provider row).
pub const ENV_OIDC_PROVIDER_SLUG: &str = "env-oidc";

/// JWKS cache TTL. Keys are also force-refreshed on an unknown `kid` (rotation).
const JWKS_TTL: Duration = Duration::from_secs(3600);

/// Auth configuration resolved from the environment and shared via `AppState`.
pub struct AuthExt {
    /// Resource-server verifier; `None` disables OIDC (PAT/password only).
    pub oidc: Option<OidcVerifier>,
    /// Keep accepting legacy password-session JWTs and `ots_` API tokens (C4).
    pub accept_legacy_tokens: bool,
    /// Role assigned to JIT-provisioned users when no role claim maps (Phase 1).
    pub default_role: String,
    /// Token claim names (dotted paths supported) inspected for role mapping (P2-1).
    pub role_claims: Vec<String>,
    /// Token claim holding org group memberships (P2-2).
    pub groups_claim: String,
    /// Prefix marking an org-membership group value, e.g. `org:example-gis` (P2-2).
    pub org_group_prefix: String,
    /// JSON map `{ "<claim value>": "super_admin"|"admin"|"user" }` (P2-1).
    pub role_claim_map: Option<String>,
}

impl AuthExt {
    /// Default disabled config (OIDC off, legacy on) — used by tests/constructors.
    pub fn disabled() -> Self {
        Self {
            oidc: None,
            accept_legacy_tokens: true,
            default_role: "user".to_string(),
            role_claims: default_role_claims(),
            groups_claim: "groups".to_string(),
            org_group_prefix: "org:".to_string(),
            role_claim_map: None,
        }
    }

    pub fn from_env() -> Self {
        let issuer = std::env::var("OIDC_ISSUER")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty());
        let audience = std::env::var("OIDC_AUDIENCE")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let default_role = std::env::var("OIDC_DEFAULT_ROLE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "user".to_string());
        // Default ON; only "false"/"0" disables (transition safety).
        let accept_legacy_tokens = std::env::var("ACCEPT_LEGACY_TOKENS")
            .map(|v| !matches!(v.trim(), "false" | "0"))
            .unwrap_or(true);
        let role_claims = std::env::var("OIDC_ROLE_CLAIMS")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_else(default_role_claims);
        let groups_claim = std::env::var("OIDC_GROUPS_CLAIM")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "groups".to_string());
        let org_group_prefix = std::env::var("OIDC_ORG_GROUP_PREFIX")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "org:".to_string());
        let role_claim_map = std::env::var("OIDC_ROLE_CLAIM_MAP")
            .ok()
            .filter(|s| !s.trim().is_empty());

        if issuer.is_some() && audience.is_none() {
            tracing::warn!(
                "OIDC_ISSUER is set but OIDC_AUDIENCE is not — OIDC bearer-token \
                 verification will reject all tokens until OIDC_AUDIENCE is configured \
                 (audience validation is mandatory)."
            );
        }
        // Refuse to enable OIDC over a cleartext issuer: discovery/JWKS would be
        // fetched over `http`, letting a MITM serve forged keys (→ token forgery).
        // Fail closed (disable OIDC) rather than silently trusting the network.
        let oidc = match issuer {
            Some(iss) if !is_secure_idp_url(&iss) => {
                tracing::error!(
                    "OIDC_ISSUER='{iss}' is not https (and not a loopback dev URL) — refusing \
                     to enable OIDC resource-server verification, since IdP key material would \
                     be fetched over cleartext (MITM → token forgery). Configure an https issuer."
                );
                None
            }
            Some(iss) => Some(OidcVerifier::new(iss, audience)),
            None => None,
        };
        Self {
            oidc,
            accept_legacy_tokens,
            default_role,
            role_claims,
            groups_claim,
            org_group_prefix,
            role_claim_map,
        }
    }
}

fn default_role_claims() -> Vec<String> {
    // Cover the common shapes: flat `roles`, Keycloak `realm_access.roles`, `groups`.
    vec![
        "roles".to_string(),
        "realm_access.roles".to_string(),
        "groups".to_string(),
    ]
}

/// Whether `raw` is safe to fetch IdP key material / discovery metadata from.
///
/// OIDC discovery and JWKS documents are trust anchors: a token's signature is
/// only as trustworthy as the keys we fetched. Fetching them over cleartext
/// `http` lets a network attacker (MITM) serve their own JWKS and forge tokens
/// we would accept. We therefore require `https`, with a loopback-only `http`
/// exception so local development against a dev IdP still works.
pub(crate) fn is_secure_idp_url(raw: &str) -> bool {
    match url::Url::parse(raw) {
        Ok(u) => match u.scheme() {
            "https" => true,
            "http" => u.host_str().map(is_loopback_host).unwrap_or(false),
            _ => false,
        },
        Err(_) => false,
    }
}

/// Loopback host check for the dev `http` exception: `localhost` or any
/// loopback IP literal (`127.0.0.0/8`, `::1`, optionally bracketed).
fn is_loopback_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost")
        || h.parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Claims we read off a verified access token. Registered claims (`exp`, `iss`,
/// `aud`) are validated by `jsonwebtoken` independently of this struct.
#[derive(Debug, Deserialize)]
pub struct ExternalClaims {
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    /// All other claims (roles, groups, realm_access, …) for claim-driven mapping.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl ExternalClaims {
    /// Best available human-readable name, falling back to the subject.
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.preferred_username.clone())
            .or_else(|| self.email.clone())
            .unwrap_or_else(|| self.sub.clone())
    }

    /// Collect string values from the named claims. Each name may be a dotted
    /// path (e.g. `realm_access.roles`); values may be a string or string array.
    pub fn claim_strings(&self, names: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for name in names {
            let mut segs = name.split('.');
            let Some(first) = segs.next() else { continue };
            let Some(mut cur) = self.extra.get(first) else {
                continue;
            };
            let mut ok = true;
            for seg in segs {
                match cur.get(seg) {
                    Some(next) => cur = next,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            match cur {
                Value::String(s) => out.push(s.clone()),
                Value::Array(arr) => {
                    out.extend(arr.iter().filter_map(|v| v.as_str().map(str::to_string)))
                }
                _ => {}
            }
        }
        out
    }
}

/// Additively sync org memberships from `org:<slug>` group claims (P2-2).
/// Non-destructive: never removes memberships, and only adds a default `member`
/// role when the user is not already a member (preserves manual elevation).
fn sync_org_memberships(
    auth_db: &Arc<AuthDb>,
    user_id: &str,
    claims: &ExternalClaims,
    groups_claim: &str,
    org_group_prefix: &str,
) -> anyhow::Result<()> {
    let groups = claims.claim_strings(std::slice::from_ref(&groups_claim.to_string()));
    for group in groups {
        let Some(slug) = group.strip_prefix(org_group_prefix) else {
            continue;
        };
        if slug.is_empty() {
            continue;
        }
        if let Some(org) = auth_db.get_organisation_by_slug(slug)? {
            if auth_db.get_org_membership(user_id, &org.id)?.is_none() {
                auth_db.add_org_member(user_id, &org.id, Role::Member)?;
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Discovery {
    jwks_uri: String,
}

struct CachedJwks {
    jwks: JwkSet,
    fetched_at: Instant,
}

/// Verifies IdP-issued access tokens against the issuer's JWKS.
pub struct OidcVerifier {
    issuer: String,
    audience: Option<String>,
    http: reqwest::Client,
    cache: RwLock<Option<CachedJwks>>,
}

impl OidcVerifier {
    pub fn new(issuer: String, audience: Option<String>) -> Self {
        Self {
            issuer,
            audience,
            http: reqwest::Client::new(),
            cache: RwLock::new(None),
        }
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Fetch (and cache) the issuer's JWKS via OIDC discovery.
    async fn jwks(&self, force: bool) -> anyhow::Result<JwkSet> {
        if !force {
            if let Some(c) = self.cache.read().await.as_ref() {
                if c.fetched_at.elapsed() < JWKS_TTL {
                    return Ok(c.jwks.clone());
                }
            }
        }
        // Defense in depth (the issuer is also validated at startup in `from_env`):
        // never fetch discovery/JWKS over cleartext, even if an insecure issuer
        // slipped through (e.g. a directly-constructed verifier).
        if !is_secure_idp_url(&self.issuer) {
            anyhow::bail!(
                "refusing to fetch OIDC discovery/JWKS for insecure issuer '{}': \
                 the issuer must use https (loopback http is allowed for local dev)",
                self.issuer
            );
        }
        let disco_url = format!(
            "{}/.well-known/openid-configuration",
            self.issuer.trim_end_matches('/')
        );
        let disco: Discovery = self
            .http
            .get(&disco_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        // The discovery document chooses `jwks_uri`; require https there too so a
        // (hypothetically) tampered or misconfigured document can't redirect the
        // key fetch to cleartext.
        if !is_secure_idp_url(&disco.jwks_uri) {
            anyhow::bail!(
                "refusing to fetch JWKS over insecure jwks_uri '{}': it must use https",
                disco.jwks_uri
            );
        }
        let jwks: JwkSet = self
            .http
            .get(&disco.jwks_uri)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        *self.cache.write().await = Some(CachedJwks {
            jwks: jwks.clone(),
            fetched_at: Instant::now(),
        });
        Ok(jwks)
    }

    /// Verify a bearer token: signature (via JWKS), `iss`, optional `aud`, `exp`.
    pub async fn verify(&self, token: &str) -> anyhow::Result<ExternalClaims> {
        let header = decode_header(token)?;
        // Reject symmetric algorithms: an OIDC access token must be asymmetric,
        // and allowing HS* here would invite alg-confusion against our own JWKS.
        if matches!(
            header.alg,
            Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
        ) {
            anyhow::bail!("symmetric JWT algorithm not allowed for OIDC tokens");
        }
        let kid = header
            .kid
            .ok_or_else(|| anyhow::anyhow!("token header has no kid"))?;

        let mut jwks = self.jwks(false).await?;
        let jwk = match jwks.find(&kid) {
            Some(k) => k.clone(),
            None => {
                // Unknown kid → keys may have rotated; refetch once.
                jwks = self.jwks(true).await?;
                jwks.find(&kid)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("no JWKS key for kid {kid}"))?
            }
        };

        let key = DecodingKey::from_jwk(&jwk)?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.issuer.as_str()]);
        // Audience validation is MANDATORY. With no configured `aud` we would
        // accept any token the IdP minted for *any* client of the same issuer
        // (audience-confusion / token-redirection). Fail closed instead of
        // disabling the check.
        let aud = self.audience.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "OIDC_AUDIENCE is not configured; refusing to verify IdP-issued token \
                 (set OIDC_AUDIENCE to this service's expected `aud`)"
            )
        })?;
        validation.set_audience(&[aud]);
        // jsonwebtoken validates `exp` by default but not `nbf`; reject
        // not-yet-valid tokens too.
        validation.validate_nbf = true;
        let data = decode::<ExternalClaims>(token, &key, &validation)?;
        Ok(data.claims)
    }
}

/// Ensure the synthetic env-OIDC provider row exists (idempotent) and return it.
/// Required because `oauth_identities` FK-references `oauth_providers(id)`.
pub fn ensure_env_provider(
    auth_db: &Arc<AuthDb>,
    issuer: &str,
    default_role: &str,
) -> anyhow::Result<OauthProvider> {
    if let Some(p) = auth_db.get_oauth_provider_by_slug(ENV_OIDC_PROVIDER_SLUG)? {
        return Ok(p);
    }
    let create = OauthProviderCreate {
        name: "Environment OIDC".to_string(),
        slug: ENV_OIDC_PROVIDER_SLUG.to_string(),
        provider_type: "oidc".to_string(),
        client_id: None,
        client_secret: None,
        client_secret_enc: None,
        discovery_url: Some(format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        )),
        tenant_id: None,
        entity_id: None,
        sso_url: None,
        idp_certificate: None,
        scopes: Some("openid email profile".to_string()),
        role_claim_map: None,
        auto_provision: true,
        default_role: Some(default_role.to_string()),
        is_active: true,
    };
    auth_db.create_oauth_provider(&create)
}

/// Find-or-create the local user for a set of verified external claims, then
/// apply claim-driven role (P2-1) and org-membership (P2-2) mapping.
pub fn provision_from_claims(
    auth_db: &Arc<AuthDb>,
    provider: &OauthProvider,
    auth_ext: &AuthExt,
    claims: &ExternalClaims,
) -> anyhow::Result<User> {
    // Inspect both the configured role claims and the groups claim for role mapping.
    let mut claim_values = claims.claim_strings(&auth_ext.role_claims);
    claim_values.extend(claims.claim_strings(std::slice::from_ref(&auth_ext.groups_claim)));
    let mapped = map_claims_to_role(&claim_values, auth_ext.role_claim_map.as_deref());

    // New users get the mapped role, else the configured default. SSO must never
    // confer super_admin (instance ownership is provisioned out-of-band), so any
    // claim that maps to super_admin is capped at admin.
    let cap_role = |r: SystemRole| {
        if r == SystemRole::SuperAdmin {
            SystemRole::Admin
        } else {
            r
        }
    };
    let default_role = SystemRole::from_str(&provider.default_role).unwrap_or(SystemRole::User);
    let mapped_role = mapped.role.map(cap_role);

    // Honour the IdP's `email_verified` claim (bool, or "true"/"false" string).
    // Only a verified email may auto-link to an existing local account.
    let email_verified = claims
        .extra
        .get("email_verified")
        .and_then(|v| {
            v.as_bool()
                .or_else(|| v.as_str().map(|s| s.eq_ignore_ascii_case("true")))
        })
        .unwrap_or(false);

    let mut user = provision_or_link_user(
        &claims.sub,
        claims.email.as_deref(),
        email_verified,
        &claims.display_name(),
        mapped_role.unwrap_or(default_role),
        provider,
        auth_db,
    )?;

    // Existing users: re-apply the IdP role, but only when a claim actually
    // mapped — never downgrade just because role claims were absent/unconfigured.
    if let Some(role) = mapped_role {
        if user.role != role {
            auth_db.update_user_role(&user.id, role)?;
            user.role = role;
        }
    }

    // Grant the publish capability when a claim maps to "publisher". Applied
    // non-destructively: SSO only ever sets it, never revokes on absent claims.
    if mapped.grant_publish && !user.can_publish {
        auth_db.update_user_can_publish(&user.id, true)?;
        user.can_publish = true;
    }

    // Additive, non-destructive org-membership sync from group claims.
    sync_org_memberships(
        auth_db,
        &user.id,
        claims,
        &auth_ext.groups_claim,
        &auth_ext.org_group_prefix,
    )?;

    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_disabled_by_default() {
        // No OIDC_* env in the test process → disabled, legacy on.
        let ext = AuthExt::from_env();
        assert!(ext.oidc.is_none());
        assert!(ext.accept_legacy_tokens);
        assert_eq!(ext.default_role, "user");
    }

    fn claims(sub: &str, email: &str, extra: serde_json::Value) -> ExternalClaims {
        ExternalClaims {
            sub: sub.to_string(),
            email: Some(email.to_string()),
            name: Some("Test User".to_string()),
            preferred_username: None,
            extra: extra.as_object().cloned().unwrap_or_default(),
        }
    }

    #[test]
    fn jit_provisioning_is_idempotent() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        let provider =
            ensure_env_provider(&db, "https://idp.example/realms/example", "user").unwrap();
        let ext = AuthExt::disabled();

        let user = provision_from_claims(
            &db,
            &provider,
            &ext,
            &claims(
                "idp-subject-abc",
                "alice@example.com",
                serde_json::json!({}),
            ),
        )
        .unwrap();
        assert_eq!(user.email, "alice@example.com");
        assert_eq!(user.role, SystemRole::User);

        // Second sighting of the same subject must return the same user, not a dup.
        let user2 = provision_from_claims(
            &db,
            &provider,
            &ext,
            &claims(
                "idp-subject-abc",
                "alice@example.com",
                serde_json::json!({}),
            ),
        )
        .unwrap();
        assert_eq!(user.id, user2.id);
    }

    #[test]
    fn role_claim_maps_to_system_role() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        let provider =
            ensure_env_provider(&db, "https://idp.example/realms/example", "user").unwrap();
        let mut ext = AuthExt::disabled();
        ext.role_claim_map = Some(r#"{"app-admins":"admin"}"#.to_string());

        // New user whose token carries the admin group → provisioned as admin.
        let user = provision_from_claims(
            &db,
            &provider,
            &ext,
            &claims(
                "sub-admin",
                "boss@example.com",
                serde_json::json!({"groups": ["app-admins"]}),
            ),
        )
        .unwrap();
        assert_eq!(user.role, SystemRole::Admin);

        // Same subject later WITHOUT the role claim must NOT downgrade them.
        let again = provision_from_claims(
            &db,
            &provider,
            &ext,
            &claims("sub-admin", "boss@example.com", serde_json::json!({})),
        )
        .unwrap();
        assert_eq!(again.role, SystemRole::Admin);
    }

    #[test]
    fn publisher_claim_grants_can_publish_capability() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        let provider =
            ensure_env_provider(&db, "https://idp.example/realms/example", "user").unwrap();
        let mut ext = AuthExt::disabled();
        ext.role_claim_map = Some(r#"{"team-pub":"publisher"}"#.to_string());

        // "publisher" grants the can_publish capability but does NOT elevate the role.
        let user = provision_from_claims(
            &db,
            &provider,
            &ext,
            &claims(
                "sub-pub",
                "pub@example.com",
                serde_json::json!({"groups": ["team-pub"]}),
            ),
        )
        .unwrap();
        assert_eq!(user.role, SystemRole::User);
        assert!(user.can_publish, "publisher claim must set can_publish");

        // Absent the claim later, the capability is retained (non-destructive).
        let again = provision_from_claims(
            &db,
            &provider,
            &ext,
            &claims("sub-pub", "pub@example.com", serde_json::json!({})),
        )
        .unwrap();
        assert!(
            again.can_publish,
            "can_publish must not be revoked on absent claims"
        );
    }

    #[test]
    fn org_group_claim_adds_membership() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        let provider =
            ensure_env_provider(&db, "https://idp.example/realms/example", "user").unwrap();
        let ext = AuthExt::disabled();

        // Pre-create the org the claim refers to (id, name, slug, …).
        let org = db
            .create_organisation("org-1", "Example GIS", "example-gis", None, None)
            .unwrap();

        let user = provision_from_claims(
            &db,
            &provider,
            &ext,
            &claims(
                "sub-orgmember",
                "gis@example.com",
                serde_json::json!({"groups": ["org:example-gis", "other"]}),
            ),
        )
        .unwrap();

        assert_eq!(
            db.get_org_membership(&user.id, &org.id).unwrap(),
            Some(Role::Member)
        );
    }

    #[test]
    fn ensure_env_provider_is_idempotent() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        let p1 = ensure_env_provider(&db, "https://idp.example/realms/example", "user").unwrap();
        let p2 = ensure_env_provider(&db, "https://idp.example/realms/example", "user").unwrap();
        assert_eq!(p1.id, p2.id);
        assert_eq!(p1.slug, ENV_OIDC_PROVIDER_SLUG);
    }

    // ─── LOW: IdP metadata must not be fetched over cleartext ─────────────────
    // OIDC discovery + JWKS are trust anchors; fetching them over http lets a
    // MITM serve forged keys and mint tokens we'd accept. https is required,
    // with a loopback-only http exception for local development.

    #[test]
    fn is_secure_idp_url_security() {
        // https to any host is accepted.
        assert!(is_secure_idp_url("https://idp.example/realms/x"));
        assert!(is_secure_idp_url("https://idp.example:8443/realms/x"));
        // Plain http to a routable host is rejected (cleartext key material).
        assert!(!is_secure_idp_url("http://idp.example/realms/x"));
        assert!(!is_secure_idp_url("http://10.0.0.5:8080/realms/x"));
        // Loopback http is allowed (dev IdP on localhost).
        assert!(is_secure_idp_url("http://localhost:8080/realms/x"));
        assert!(is_secure_idp_url("http://127.0.0.1:9000/realms/x"));
        assert!(is_secure_idp_url("http://[::1]:9000/realms/x"));
        // Non-http(s) schemes and garbage are rejected.
        assert!(!is_secure_idp_url("ftp://idp.example"));
        assert!(!is_secure_idp_url("file:///etc/passwd"));
        assert!(!is_secure_idp_url("not-a-url"));
    }

    #[tokio::test]
    async fn oidc_verifier_refuses_insecure_issuer_security() {
        // A verifier with an http (non-loopback) issuer must refuse to fetch
        // discovery/JWKS — failing closed *before* any network I/O, so a cleartext
        // issuer can never serve forged keys. `force=true` skips the cache.
        let verifier = OidcVerifier::new("http://idp.example".to_string(), Some("aud".to_string()));
        let err = verifier
            .jwks(true)
            .await
            .expect_err("an http issuer must be refused before fetching keys");
        let msg = err.to_string();
        assert!(
            msg.contains("https") || msg.contains("insecure"),
            "the error must explain the https requirement, got: {msg}"
        );
    }
}
