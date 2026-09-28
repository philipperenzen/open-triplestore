//! Federated access control: identity carried across instances, authorisation
//! kept local (the DSGO pattern).
//!
//! * **Outbound** (`OTS_REMOTE_AUTH=assert`): when this instance calls an
//!   allowlisted peer on a user's behalf — a `SERVICE` clause, an LDES sync —
//!   it mints a short-lived ES256 *identity assertion* with its OIDC-provider
//!   key: `iss` = this instance, `sub` = the user, `aud` = the peer's origin,
//!   `preferred_username`, and `groups` = `org:<slug>` for every organisation
//!   the user belongs to here. The user's own session token never leaves
//!   this instance.
//! * **Inbound** (`OTS_TRUSTED_ISSUERS=https://peer-a,https://peer-b`): a
//!   bearer whose `iss` is a trusted peer is verified against that peer's
//!   JWKS (discovery at `<iss>/.well-known/openid-configuration`), must name
//!   this instance (`BASE_URL`) as its audience, and is provisioned as a
//!   local federated user — read-only, with organisation memberships synced
//!   from the `org:` groups when an organisation of that slug exists here.
//!   Every local rule then applies unchanged: dataset visibility, grants,
//!   graph and endpoint ACLs.
//!
//! The signer is process-wide (one instance per process); the identity of the
//! current request travels to the store thread through a thread-local, which
//! is where `SERVICE` evaluation runs.

use std::cell::RefCell;
use std::sync::{Arc, OnceLock};

use crate::auth::oidc_provider::ProviderKeys;
use crate::server::AppState;

/// How long an identity assertion lives.
const ASSERTION_TTL_SECS: i64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundAuth {
    None,
    Assert,
}

pub fn outbound_auth() -> OutboundAuth {
    match std::env::var("OTS_REMOTE_AUTH")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "assert" | "forward" | "identity" => OutboundAuth::Assert,
        _ => OutboundAuth::None,
    }
}

/// Issuers whose assertions this instance accepts.
pub fn trusted_issuers() -> Vec<String> {
    std::env::var("OTS_TRUSTED_ISSUERS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The identity a request acts as, as far as peers need to know.
#[derive(Debug, Clone)]
pub struct Identity {
    pub user_id: String,
    pub username: String,
    /// Organisation slugs.
    pub orgs: Vec<String>,
}

struct Signer {
    keys: Arc<ProviderKeys>,
    issuer: String,
}

static SIGNER: OnceLock<Signer> = OnceLock::new();

/// Install the signing key and issuer (this instance's base URL). Called at
/// server start; a second call is ignored.
pub fn init(keys: Arc<ProviderKeys>, base_url: &str) {
    let _ = SIGNER.set(Signer {
        keys,
        issuer: base_url.trim_end_matches('/').to_string(),
    });
}

thread_local! {
    static CURRENT: RefCell<Option<Arc<Identity>>> = const { RefCell::new(None) };
}

/// Sets the current thread's identity for as long as it lives.
pub struct IdentityGuard(Option<Arc<Identity>>);

impl IdentityGuard {
    pub fn set(identity: Option<Arc<Identity>>) -> Self {
        let previous = CURRENT.with(|c| c.replace(identity));
        IdentityGuard(previous)
    }
}

impl Drop for IdentityGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        CURRENT.with(|c| *c.borrow_mut() = previous);
    }
}

pub fn current_identity() -> Option<Arc<Identity>> {
    CURRENT.with(|c| c.borrow().clone())
}

/// The identity of `user` for outbound assertions (None when assertions are
/// off, so the common path costs nothing).
pub fn identity_for(state: &AppState, user_id: &str) -> Option<Arc<Identity>> {
    if outbound_auth() == OutboundAuth::None {
        return None;
    }
    let username = state
        .auth_db
        .get_user_by_id(user_id)
        .ok()
        .flatten()
        .map(|u| u.username)
        .unwrap_or_else(|| user_id.to_string());
    let orgs = state
        .auth_db
        .list_user_organisations(user_id)
        .unwrap_or_default()
        .into_iter()
        .map(|o| o.slug)
        .collect();
    Some(Arc::new(Identity {
        user_id: user_id.to_string(),
        username,
        orgs,
    }))
}

/// `scheme://host[:port]` of a URL.
pub fn origin_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let host = u.host_str()?;
    Some(match u.port() {
        Some(p) => format!("{}://{host}:{p}", u.scheme()),
        None => format!("{}://{host}", u.scheme()),
    })
}

/// An identity assertion for the peer at `url`, when assertions are on, a
/// signer is installed and the current thread acts for a user.
pub fn assertion_for(url: &str) -> Option<String> {
    assertion_for_identity(url, current_identity().as_ref())
}

/// As [`assertion_for`], for an explicit identity (captured on the request
/// thread and carried to wherever the call is made — query evaluation may
/// run on another thread).
pub fn assertion_for_identity(url: &str, identity: Option<&Arc<Identity>>) -> Option<String> {
    if outbound_auth() == OutboundAuth::None {
        return None;
    }
    let signer = SIGNER.get()?;
    let identity = identity?;
    let aud = origin_of(url)?;
    let now = chrono::Utc::now().timestamp();
    let claims = serde_json::json!({
        "iss": signer.issuer,
        "sub": identity.user_id,
        "aud": aud,
        "iat": now,
        "nbf": now - 5,
        "exp": now + ASSERTION_TTL_SECS,
        "preferred_username": identity.username,
        "groups": identity.orgs.iter().map(|s| format!("org:{s}")).collect::<Vec<_>>(),
        "ots_federated": true,
    });
    match signer.keys.sign_claims(&claims) {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::warn!("federation: could not sign assertion for {aud}: {e}");
            None
        }
    }
}

/// The unverified `iss` of a JWT (to pick the verifier; verification follows).
pub fn unverified_issuer(token: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("iss")?
        .as_str()
        .map(|s| s.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    #[test]
    fn origins_and_issuers() {
        assert_eq!(
            origin_of("http://127.0.0.1:8080/sparql?x=1").as_deref(),
            Some("http://127.0.0.1:8080")
        );
        assert_eq!(
            origin_of("https://peer.example.org/api/datasets/x/ldes").as_deref(),
            Some("https://peer.example.org")
        );
        assert!(origin_of("not a url").is_none());
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"iss":"https://a.example/","sub":"u"}"#);
        assert_eq!(
            unverified_issuer(&format!("h.{payload}.s")).as_deref(),
            Some("https://a.example")
        );
        assert!(unverified_issuer("garbage").is_none());
    }

    #[test]
    fn identity_guard_scopes_the_thread_local() {
        assert!(current_identity().is_none());
        {
            let _g = IdentityGuard::set(Some(Arc::new(Identity {
                user_id: "u".into(),
                username: "u".into(),
                orgs: vec![],
            })));
            assert_eq!(current_identity().unwrap().user_id, "u");
        }
        assert!(current_identity().is_none());
    }
}
