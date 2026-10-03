//! Configurable capability policy for the principals whose power used to be
//! implicit: self-registered **guests**, **OIDC access tokens** this server
//! issues to registered clients, and access tokens an **external IdP** issues
//! for this store's audience (resource-server mode).
//!
//! All three are deliberately env-driven with a conservative default, because the
//! right answer differs per deployment: a public demo wants guests who can only
//! look, an internal instance may want them to fill forms.
//!
//! * [`guest_capabilities`] — `OTS_GUEST_CAPABILITIES`, default read-only. The
//!   `Guest` role existed but nothing consulted it, so a self-registered guest
//!   could create datasets, write graph data and mint API tokens exactly like a
//!   full user.
//! * [`oidc_session_policy`] — `OTS_OIDC_SESSION_POLICY`, default `session`. An
//!   access token was accepted as a full-power credential and could be exchanged
//!   for a long-lived API token at `POST /api/auth/tokens`, turning a read-only
//!   delegation into permanent account access.
//! * [`idp_token_policy`] — `OTS_OIDC_IDP_TOKEN_POLICY`, default `session`. The same
//!   choice for IdP access tokens, which used to mint API tokens too. It is a
//!   separate setting because the two token sources are configured for
//!   different audiences: tightening this store's own provider for third-party
//!   clients must not silently make every IdP user read-only, and loosening it
//!   for one legacy client must not reopen minting for every IdP token.
//!
//! The parsing is pure so the decisions are unit-testable without a server.

/// What a `Guest` may do beyond reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GuestCapabilities {
    /// Write data into datasets the guest has an explicit grant on.
    pub write: bool,
    /// Create datasets of their own.
    pub create_datasets: bool,
    /// Mint long-lived API tokens.
    pub api_tokens: bool,
    /// Publish model/vocabulary versions (the `can_publish` flag).
    pub publish: bool,
}

impl GuestCapabilities {
    /// Everything a normal `user` may do — the escape hatch (`all`).
    pub fn all() -> Self {
        Self {
            write: true,
            create_datasets: true,
            api_tokens: true,
            publish: true,
        }
    }
}

/// Parse `OTS_GUEST_CAPABILITIES`: a comma/space separated list.
///
/// Reading is always permitted and needs no token, so it is accepted as a no-op
/// spelling of the default. Unknown words are ignored rather than fatal: a typo
/// must never silently *grant* something, and the default already denies.
pub fn parse_guest_capabilities(raw: &str) -> GuestCapabilities {
    let mut caps = GuestCapabilities::default();
    for word in raw
        .split([',', ' ', '\t'])
        .map(|w| w.trim().to_ascii_lowercase())
        .filter(|w| !w.is_empty())
    {
        match word.as_str() {
            "all" => return GuestCapabilities::all(),
            "write" => caps.write = true,
            "create_datasets" | "create-datasets" => caps.create_datasets = true,
            "api_tokens" | "api-tokens" => caps.api_tokens = true,
            "publish" => caps.publish = true,
            "read" | "none" => {}
            other => tracing::warn!(
                "OTS_GUEST_CAPABILITIES: ignoring unknown capability {other:?} \
                 (known: read, write, create_datasets, api_tokens, publish, all)"
            ),
        }
    }
    caps
}

/// Capabilities granted to the `Guest` role in this deployment (default: read only).
pub fn guest_capabilities() -> GuestCapabilities {
    match std::env::var("OTS_GUEST_CAPABILITIES") {
        Ok(raw) => parse_guest_capabilities(&raw),
        Err(_) => GuestCapabilities::default(),
    }
}

/// How much authority a delegated OIDC access token carries: one issued by
/// this server ([`oidc_session_policy`]) or by the external IdP
/// ([`idp_token_policy`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OidcSessionPolicy {
    /// Interactive-session semantics: may read and write, but may NOT mint API
    /// tokens. Default — it keeps existing sign-in flows working (first-party
    /// apps commonly request only `openid profile email`, so scope-based writing
    /// would break them) while cutting the escalation from a delegated token to
    /// a permanent credential.
    Session,
    /// Least privilege: write only when the token's `scope` actually grants it,
    /// and never mint API tokens. The right choice once third-party clients can
    /// register — with it, consenting to `openid profile email` grants reading
    /// and nothing else.
    Scoped,
    /// Legacy behaviour: full write plus API-token minting. Escape hatch for a
    /// deployment that depends on it.
    Full,
}

impl OidcSessionPolicy {
    /// Whether a provider token carrying `scope` may write. Extra write
    /// scopes come from `OTS_OIDC_WRITE_SCOPES`.
    pub fn allows_write(self, scope: &str) -> bool {
        self.allows_write_with(scope, || env_or_empty("OTS_OIDC_WRITE_SCOPES"))
    }

    /// Whether an IdP token carrying `scope` may write. Extra write scopes
    /// come from `OTS_OIDC_IDP_WRITE_SCOPES`.
    pub fn allows_idp_write(self, scope: &str) -> bool {
        self.allows_write_with(scope, || env_or_empty("OTS_OIDC_IDP_WRITE_SCOPES"))
    }

    fn allows_write_with(self, scope: &str, extra_scopes: impl FnOnce() -> String) -> bool {
        match self {
            OidcSessionPolicy::Session | OidcSessionPolicy::Full => true,
            OidcSessionPolicy::Scoped => scope_grants_write_with(scope, &extra_scopes()),
        }
    }

    /// Whether a token may exchange itself for a long-lived API token.
    pub fn allows_api_token_minting(self) -> bool {
        matches!(self, OidcSessionPolicy::Full)
    }
}

fn env_or_empty(var: &str) -> String {
    std::env::var(var).unwrap_or_default()
}

/// Scope values understood as granting write. `admin` implies write.
///
/// Issuers that namespace their scopes (`myapp:write`) name the extra
/// spellings in `extra_scopes` (`OTS_OIDC_WRITE_SCOPES` / `OTS_OIDC_IDP_WRITE_SCOPES`)
/// rather than teaching this server about them.
fn scope_grants_write_with(scope: &str, extra_scopes: &str) -> bool {
    let extra: Vec<String> = scope_words(extra_scopes).collect();
    scope_words(scope).any(|s| matches!(s.as_str(), "write" | "admin") || extra.contains(&s))
}

fn scope_words(raw: &str) -> impl Iterator<Item = String> + '_ {
    raw.split([' ', ',', '\t'])
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

/// Parse `OTS_OIDC_SESSION_POLICY`. Anything unrecognised falls back to the
/// default rather than to the permissive setting.
pub fn parse_oidc_session_policy(raw: &str) -> OidcSessionPolicy {
    parse_policy("OTS_OIDC_SESSION_POLICY", raw)
}

/// Parse `OTS_OIDC_IDP_TOKEN_POLICY`; same values and fallback as
/// [`parse_oidc_session_policy`].
pub fn parse_idp_token_policy(raw: &str) -> OidcSessionPolicy {
    parse_policy("OTS_OIDC_IDP_TOKEN_POLICY", raw)
}

fn parse_policy(var: &str, raw: &str) -> OidcSessionPolicy {
    match raw.trim().to_ascii_lowercase().as_str() {
        "scoped" => OidcSessionPolicy::Scoped,
        "full" | "legacy" => OidcSessionPolicy::Full,
        "session" | "" => OidcSessionPolicy::Session,
        other => {
            tracing::warn!(
                "{var}: unknown value {other:?}, using 'session' (known: session, scoped, full)"
            );
            OidcSessionPolicy::Session
        }
    }
}

/// The policy for access tokens this server issues as an OIDC provider
/// (default: `session`).
pub fn oidc_session_policy() -> OidcSessionPolicy {
    match std::env::var("OTS_OIDC_SESSION_POLICY") {
        Ok(raw) => parse_oidc_session_policy(&raw),
        Err(_) => OidcSessionPolicy::Session,
    }
}

/// The policy for access tokens the external IdP issues for this store's
/// audience (default: `session`). Independent of [`oidc_session_policy`].
pub fn idp_token_policy() -> OidcSessionPolicy {
    match std::env::var("OTS_OIDC_IDP_TOKEN_POLICY") {
        Ok(raw) => parse_idp_token_policy(&raw),
        Err(_) => OidcSessionPolicy::Session,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guests_are_read_only_by_default() {
        let caps = parse_guest_capabilities("");
        assert!(!caps.write && !caps.create_datasets && !caps.api_tokens && !caps.publish);
        // "read" is the explicit spelling of the same thing.
        assert_eq!(parse_guest_capabilities("read"), caps);
    }

    #[test]
    fn guest_capabilities_are_opt_in_individually() {
        let caps = parse_guest_capabilities("write, create_datasets");
        assert!(caps.write && caps.create_datasets);
        assert!(!caps.api_tokens, "never granted unless named");

        assert_eq!(parse_guest_capabilities("all"), GuestCapabilities::all());
        // Separator and case tolerance, plus the hyphenated spellings.
        assert!(parse_guest_capabilities("API-TOKENS").api_tokens);
    }

    #[test]
    fn unknown_capability_words_never_grant() {
        let caps = parse_guest_capabilities("admin, superuser, *, write");
        assert!(caps.write, "the recognised word still applies");
        assert!(
            !caps.create_datasets && !caps.api_tokens,
            "a typo must not widen the grant"
        );
    }

    #[test]
    fn default_oidc_policy_keeps_write_but_stops_token_minting() {
        let p = parse_oidc_session_policy("");
        assert_eq!(p, OidcSessionPolicy::Session);
        // First-party apps commonly request only `openid profile email`.
        assert!(p.allows_write("openid profile email"));
        // ...but a delegated token must not become a permanent credential.
        assert!(!p.allows_api_token_minting());
    }

    #[test]
    fn scoped_policy_honours_the_scope_claim() {
        let p = parse_oidc_session_policy("scoped");
        assert!(!p.allows_write("openid profile email"));
        assert!(p.allows_write("openid write"));
        assert!(p.allows_write("admin"));
        assert!(!p.allows_api_token_minting());
    }

    #[test]
    fn namespaced_write_scopes_are_configurable() {
        // A deployment whose issuer namespaces its scopes names them itself;
        // the server knows only `write`/`admin` out of the box.
        assert!(!scope_grants_write_with("openid myapp:write", ""));
        assert!(scope_grants_write_with(
            "openid myapp:write",
            "myapp:write, myapp:admin"
        ));
        // The built-in spellings keep working alongside the configured ones.
        assert!(scope_grants_write_with("write", "myapp:write"));
        // An empty setting must not turn every scope into a write grant.
        assert!(!scope_grants_write_with("openid", " , ,"));
    }

    #[test]
    fn full_policy_is_the_legacy_escape_hatch() {
        let p = parse_oidc_session_policy("full");
        assert!(p.allows_write("openid"));
        assert!(p.allows_api_token_minting());
    }

    #[test]
    fn unknown_policy_falls_back_to_the_default_not_the_permissive_one() {
        let p = parse_oidc_session_policy("banana");
        assert_eq!(p, OidcSessionPolicy::Session);
        assert!(!p.allows_api_token_minting());
    }

    #[test]
    fn idp_token_policy_reads_the_same_values_and_defaults_to_session() {
        assert_eq!(parse_idp_token_policy(""), OidcSessionPolicy::Session);
        assert!(!parse_idp_token_policy("").allows_api_token_minting());
        assert_eq!(parse_idp_token_policy("Scoped"), OidcSessionPolicy::Scoped);
        assert_eq!(parse_idp_token_policy("legacy"), OidcSessionPolicy::Full);
        assert_eq!(parse_idp_token_policy("banana"), OidcSessionPolicy::Session);
    }
}
