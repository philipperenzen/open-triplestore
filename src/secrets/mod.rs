//! Secret references: the store holds *pointers* to secrets, never secrets.
//!
//! Every credential the server needs — datasource passwords, OIDC client
//! secrets, the LLM gateway key, SMTP and S3 credentials, the JWT signing
//! secret — is configured as a [`SecretRef`] string and resolved at the moment
//! of use:
//!
//! | Form | Resolves to |
//! |---|---|
//! | `env:NAME` | the process environment variable `NAME` |
//! | `file:/run/secrets/name` | the file's contents, trailing whitespace trimmed |
//! | `vault:<mount>/data/<path>#<key>` | HashiCorp Vault KV **v2** — `GET /v1/<mount>/data/<path>`, field `<key>` |
//! | `vault:<mount>/<path>#<key>` | HashiCorp Vault KV **v1** — `GET /v1/<mount>/<path>`, field `<key>` |
//!
//! `aws-sm:`, `gcp-sm:` and `azure-kv:` are reserved for cloud providers and
//! refused with a clear message until a provider lands.
//!
//! Vault is reached at `VAULT_ADDR`, with the token from `VAULT_TOKEN_FILE`
//! (the Vault Agent sink — re-read on every resolution so rotation is picked
//! up) or `VAULT_TOKEN`, an optional `VAULT_NAMESPACE`, and an optional
//! `VAULT_CACERT` PEM bundle for a private CA. There is no "skip verify".
//!
//! Resolved values are cached for `OTS_SECRET_CACHE_TTL_SECS` (default 60,
//! positive results only), never persisted, never logged — [`Secret`] has no
//! `Display` and a redacting `Debug` — and [`redact`] strips them from any
//! error string that could reach a caller. Rotation happens in the secret
//! store; this server notices on the next resolution after the TTL.
//!
//! [`posture`] reports the deployment posture from `OTS_ENV`: in
//! `production` a raw secret value where a reference is expected is refused
//! (configuration → startup error, API body → 400); in development it is
//! accepted with a deprecation warning, once per setting.

pub mod providers;

use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;

/// Environment variable naming the deployment posture.
pub const POSTURE_ENV: &str = "OTS_ENV";
/// Positive-cache TTL for resolved secrets, in seconds.
pub const CACHE_TTL_ENV: &str = "OTS_SECRET_CACHE_TTL_SECS";

const SCHEMES: [&str; 6] = ["env:", "file:", "vault:", "aws-sm:", "gcp-sm:", "azure-kv:"];

/// A pointer to a secret in an external store. Safe to persist, log and
/// return from an API: it carries no secret material.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SecretRef {
    Env {
        name: String,
    },
    File {
        path: PathBuf,
    },
    Vault {
        mount: String,
        path: String,
        key: String,
        /// `2` for the `<mount>/data/<path>` form, `1` otherwise.
        kv_version: u8,
    },
}

impl SecretRef {
    /// Parse a reference string. Only the syntax is checked here; use
    /// [`validate`] to also require that it resolves.
    pub fn parse(s: &str) -> Result<Self, SecretError> {
        let s = s.trim();
        let malformed = |reason: &str| SecretError::Malformed {
            reference: s.to_string(),
            reason: reason.to_string(),
        };
        if let Some(name) = s.strip_prefix("env:") {
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(malformed(
                    "an env: reference names one environment variable (letters, digits, '_')",
                ));
            }
            return Ok(SecretRef::Env {
                name: name.to_string(),
            });
        }
        if let Some(path) = s.strip_prefix("file:") {
            if path.is_empty() {
                return Err(malformed("a file: reference needs a path"));
            }
            return Ok(SecretRef::File {
                path: PathBuf::from(path),
            });
        }
        if let Some(rest) = s.strip_prefix("vault:") {
            let (path, key) = rest
                .rsplit_once('#')
                .ok_or_else(|| malformed("a vault: reference ends in '#<key>'"))?;
            let path = path.trim_matches('/');
            if key.is_empty() || path.is_empty() {
                return Err(malformed(
                    "a vault: reference is '<mount>/data/<path>#<key>' (KV v2) or '<mount>/<path>#<key>' (KV v1)",
                ));
            }
            let mut segments = path.split('/');
            let mount = segments.next().unwrap_or("").to_string();
            let remaining: Vec<&str> = segments.collect();
            if mount.is_empty() || remaining.is_empty() {
                return Err(malformed(
                    "a vault: reference needs a mount and a secret path",
                ));
            }
            let (kv_version, secret_path) = if remaining[0] == "data" && remaining.len() > 1 {
                (2, remaining[1..].join("/"))
            } else {
                (1, remaining.join("/"))
            };
            return Ok(SecretRef::Vault {
                mount,
                path: secret_path,
                key: key.to_string(),
                kv_version,
            });
        }
        for scheme in ["aws-sm:", "gcp-sm:", "azure-kv:"] {
            if s.starts_with(scheme) {
                return Err(SecretError::Unsupported {
                    scheme: scheme.trim_end_matches(':').to_string(),
                });
            }
        }
        Err(malformed(
            "expected env:NAME, file:/path or vault:<mount>/data/<path>#<key>",
        ))
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretRef::Env { name } => write!(f, "env:{name}"),
            SecretRef::File { path } => write!(f, "file:{}", path.display()),
            SecretRef::Vault {
                mount,
                path,
                key,
                kv_version,
            } => {
                if *kv_version == 2 {
                    write!(f, "vault:{mount}/data/{path}#{key}")
                } else {
                    write!(f, "vault:{mount}/{path}#{key}")
                }
            }
        }
    }
}

/// Whether a configured value is *meant* as a reference (one of the known
/// schemes), as opposed to a raw secret value.
pub fn looks_like_ref(value: &str) -> bool {
    let v = value.trim();
    SCHEMES.iter().any(|s| v.starts_with(s))
}

/// A resolved secret value. No `Display`; `Debug` redacts.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("secret reference '{reference}' is malformed: {reason}")]
    Malformed { reference: String, reason: String },
    #[error("secret reference scheme '{scheme}:' is reserved for a provider that is not available in this build")]
    Unsupported { scheme: String },
    #[error("secret reference '{reference}' could not be resolved: {reason}")]
    Unresolvable { reference: String, reason: String },
    #[error("{setting} holds a raw secret value; in the production posture ({POSTURE_ENV}=production) only a secret reference (env:NAME, file:/path, vault:…) is accepted")]
    RawSecretRefused { setting: String },
}

/// Resolve a reference through its provider, with a positive cache.
pub fn resolve(reference: &SecretRef) -> Result<Secret, SecretError> {
    let key = reference.to_string();
    let ttl = cache_ttl();
    if !ttl.is_zero() {
        if let Some(hit) = cache().get(&key) {
            let (secret, at) = hit.value();
            if at.elapsed() < ttl {
                return Ok(secret.clone());
            }
        }
    }
    let secret = providers::resolve(reference)?;
    if !ttl.is_zero() {
        cache().insert(key, (secret.clone(), Instant::now()));
    }
    Ok(secret)
}

/// Require that a reference is well-formed **and** resolvable right now.
/// Used at registration time so a broken pointer is an error the admin sees,
/// not a failure at the first run.
pub fn validate(reference: &SecretRef) -> Result<(), SecretError> {
    resolve(reference).map(|_| ())
}

/// Drop every cached value (rotation on demand, and tests).
pub fn clear_cache() {
    cache().clear();
}

fn cache() -> &'static DashMap<String, (Secret, Instant)> {
    static CACHE: OnceLock<DashMap<String, (Secret, Instant)>> = OnceLock::new();
    CACHE.get_or_init(DashMap::new)
}

fn cache_ttl() -> Duration {
    let secs = std::env::var(CACHE_TTL_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(60);
    Duration::from_secs(secs)
}

/// Replace every occurrence of each secret's value in `text` with
/// `[redacted]`. Values shorter than four characters are not searched for
/// (they would blank out ordinary words), but such a value is not a secret
/// any password policy would accept either.
pub fn redact(text: &str, secrets: &[&Secret]) -> String {
    let mut out = text.to_string();
    for s in secrets {
        let v = s.expose();
        if v.len() >= 4 && out.contains(v) {
            out = out.replace(v, "[redacted]");
        }
    }
    out
}

/// The deployment posture, from `OTS_ENV`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posture {
    Development,
    Production,
}

impl Posture {
    pub fn is_production(self) -> bool {
        matches!(self, Posture::Production)
    }
}

pub fn posture() -> Posture {
    match std::env::var(POSTURE_ENV)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "production" | "prod" => Posture::Production,
        _ => Posture::Development,
    }
}

/// How a configured secret setting was supplied.
#[derive(Debug, Clone)]
pub enum ConfiguredSecret {
    Reference(SecretRef),
    /// A raw value — only ever produced in the development posture.
    Raw(Secret),
}

/// Interpret a configured secret setting: a reference is parsed (a malformed
/// or unsupported reference is an error in every posture); anything else is
/// a raw secret, refused in production and accepted with a one-time
/// deprecation warning in development. `setting` names the input in messages
/// (`LLM_API_KEY`, `client_secret`, …) and is never the value.
pub fn configured(setting: &str, value: &str) -> Result<ConfiguredSecret, SecretError> {
    if looks_like_ref(value) {
        return SecretRef::parse(value).map(ConfiguredSecret::Reference);
    }
    if posture().is_production() {
        return Err(SecretError::RawSecretRefused {
            setting: setting.to_string(),
        });
    }
    warn_raw_once(setting);
    Ok(ConfiguredSecret::Raw(Secret::new(value)))
}

/// [`configured`] followed by resolution.
pub fn resolve_configured(setting: &str, value: &str) -> Result<Secret, SecretError> {
    match configured(setting, value)? {
        ConfiguredSecret::Reference(r) => resolve(&r),
        ConfiguredSecret::Raw(s) => Ok(s),
    }
}

/// Read an environment-configured secret (`None` when unset or empty),
/// accepting a reference or — in development — a raw value.
pub fn env_secret(setting: &str) -> Result<Option<Secret>, SecretError> {
    match std::env::var(setting) {
        Ok(v) if !v.trim().is_empty() => resolve_configured(setting, v.trim()).map(Some),
        _ => Ok(None),
    }
}

/// [`env_secret`], with a failure logged and treated as "not configured".
///
/// The call sites that want this are the ones where a missing credential is a
/// supported configuration (no SMTP auth, no gateway key): a reference that
/// will not resolve must not be sent as if it were the secret, and must not
/// take the server down either.
pub fn env_secret_opt(setting: &str) -> Option<String> {
    match env_secret(setting) {
        Ok(secret) => secret.map(|s| s.expose().to_string()),
        Err(e) => {
            tracing::warn!("{setting} could not be resolved, continuing without it: {e}");
            None
        }
    }
}

fn warn_raw_once(setting: &str) {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut seen = WARNED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if seen.insert(setting.to_string()) {
        tracing::warn!(
            setting,
            "{setting} is configured as a raw secret value. This is deprecated: point it at a \
             secret store instead (env:NAME, file:/path, vault:<mount>/data/<path>#<key>). \
             The production posture ({POSTURE_ENV}=production) refuses raw values."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_supported_form() {
        assert_eq!(
            SecretRef::parse("env:DB_PASSWORD").unwrap(),
            SecretRef::Env {
                name: "DB_PASSWORD".into()
            }
        );
        assert_eq!(
            SecretRef::parse("file:/run/secrets/db").unwrap(),
            SecretRef::File {
                path: PathBuf::from("/run/secrets/db")
            }
        );
        let v2 = SecretRef::parse("vault:secret/data/sources/legacy#password").unwrap();
        assert_eq!(
            v2,
            SecretRef::Vault {
                mount: "secret".into(),
                path: "sources/legacy".into(),
                key: "password".into(),
                kv_version: 2
            }
        );
        assert_eq!(v2.to_string(), "vault:secret/data/sources/legacy#password");
        let v1 = SecretRef::parse("vault:kv/sources/legacy#password").unwrap();
        assert_eq!(
            v1,
            SecretRef::Vault {
                mount: "kv".into(),
                path: "sources/legacy".into(),
                key: "password".into(),
                kv_version: 1
            }
        );
        assert_eq!(v1.to_string(), "vault:kv/sources/legacy#password");
    }

    #[test]
    fn rejects_malformed_and_reserved_forms() {
        for bad in [
            "",
            "hunter2",
            "env:",
            "env:has space",
            "file:",
            "vault:secret",
            "vault:#k",
            "vault:/#k",
        ] {
            assert!(
                matches!(SecretRef::parse(bad), Err(SecretError::Malformed { .. })),
                "{bad:?} should be malformed"
            );
        }
        assert!(matches!(
            SecretRef::parse("aws-sm:prod/db#password"),
            Err(SecretError::Unsupported { scheme }) if scheme == "aws-sm"
        ));
        assert!(looks_like_ref("azure-kv:x"));
        assert!(!looks_like_ref("s3cr3t"));
    }

    #[test]
    fn env_and_file_resolve_and_redact() {
        std::env::set_var("OTS_SECRETS_TEST_ENV", "topsecretvalue");
        let s = resolve(&SecretRef::parse("env:OTS_SECRETS_TEST_ENV").unwrap()).unwrap();
        assert_eq!(s.expose(), "topsecretvalue");
        assert_eq!(format!("{s:?}"), "Secret([redacted])");
        let dir = std::env::temp_dir().join(format!("ots-secrets-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("pw");
        std::fs::write(&f, "filesecret\n").unwrap();
        let s2 = resolve(&SecretRef::parse(&format!("file:{}", f.display())).unwrap()).unwrap();
        assert_eq!(s2.expose(), "filesecret", "trailing newline trimmed");
        assert_eq!(
            redact(
                "login failed for topsecretvalue at db (filesecret)",
                &[&s, &s2]
            ),
            "login failed for [redacted] at db ([redacted])"
        );
        assert!(matches!(
            resolve(&SecretRef::parse("env:OTS_SECRETS_TEST_MISSING").unwrap()),
            Err(SecretError::Unresolvable { .. })
        ));
        assert!(matches!(
            resolve(&SecretRef::parse("file:/nonexistent/ots/secret").unwrap()),
            Err(SecretError::Unresolvable { .. })
        ));
        // Error strings never carry the value.
        let err = resolve(&SecretRef::parse("env:OTS_SECRETS_TEST_MISSING").unwrap()).unwrap_err();
        assert!(!err.to_string().contains("topsecretvalue"));
    }

    #[test]
    fn cache_serves_within_ttl_and_can_be_cleared() {
        std::env::set_var("OTS_SECRETS_TEST_CACHE", "v1");
        let r = SecretRef::parse("env:OTS_SECRETS_TEST_CACHE").unwrap();
        assert_eq!(resolve(&r).unwrap().expose(), "v1");
        std::env::set_var("OTS_SECRETS_TEST_CACHE", "v2");
        assert_eq!(resolve(&r).unwrap().expose(), "v1", "cached");
        clear_cache();
        assert_eq!(
            resolve(&r).unwrap().expose(),
            "v2",
            "rotation seen after the cache is cleared"
        );
    }

    #[test]
    fn configured_accepts_refs_and_raw_only_in_development() {
        // This process runs in the development posture (OTS_ENV unset).
        assert_eq!(posture(), Posture::Development);
        match configured("LLM_API_KEY", "sk-raw").unwrap() {
            ConfiguredSecret::Raw(s) => assert_eq!(s.expose(), "sk-raw"),
            other => panic!("{other:?}"),
        }
        match configured("LLM_API_KEY", "env:X").unwrap() {
            ConfiguredSecret::Reference(r) => assert_eq!(r.to_string(), "env:X"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            configured("LLM_API_KEY", "vault:broken"),
            Err(SecretError::Malformed { .. })
        ));
        let msg = SecretError::RawSecretRefused {
            setting: "client_secret".into(),
        }
        .to_string();
        assert!(msg.contains("client_secret") && msg.contains("production"));
    }
}
