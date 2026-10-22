//! The settings, one declaration each. Grouped as the admin view shows them.
//!
//! Adding a setting: declare it here with its kind, default and a line of
//! documentation, read it through its key, and name it in the docs (the
//! registry's tests check that every setting is documented somewhere under
//! `docs/` or in `.env.example`).

use super::{ConfigError, Ctx, Key, Kind, Spec};
use std::path::PathBuf;
use std::time::Duration;

/// `Kind::Int` from `min`, unbounded above.
const fn at_least(min: u64) -> Kind {
    Kind::Int { min, max: u64::MAX }
}

macro_rules! settings {
    ($( $(#[$m:meta])* $id:ident : $t:ty = $spec:expr; )*) => {
        $( $(#[$m])* pub const $id: Key<$t> = Key::new($spec); )*

        /// Every declared setting, in declaration order.
        pub static SPECS: &[Spec] = &[ $( $id.spec ),* ];

        /// `(name, whether its key's type can read its kind)` for every
        /// setting: the registry's tests insist on `true` throughout.
        #[cfg(test)]
        pub(crate) fn type_checks() -> Vec<(&'static str, bool)> {
            vec![ $( ($id.spec.name, <$t as super::FromSetting>::accepts(&$id.spec.kind)) ),* ]
        }
    };
}

// ── validators ─────────────────────────────────────────────────────────────

fn cidr(v: &str) -> Result<(), String> {
    v.parse::<ipnet::IpNet>()
        .map(|_| ())
        .map_err(|_| format!("`{v}` is not a CIDR block (e.g. 10.0.0.0/8)"))
}

fn cidr_list(v: &str) -> Result<(), String> {
    super::parse::list(v).iter().try_for_each(|e| cidr(e))
}

fn origin(v: &str) -> Result<(), String> {
    if v == "*" {
        return Ok(());
    }
    super::parse::http_url(v)?;
    axum::http::HeaderValue::from_str(v)
        .map(|_| ())
        .map_err(|_| format!("`{v}` cannot be sent as an origin"))
}

fn origin_list(v: &str) -> Result<(), String> {
    super::parse::list(v).iter().try_for_each(|e| origin(e))
}

/// An outbound allowlist entry: an absolute http(s) URL without credentials.
fn allow_entry(v: &str) -> Result<(), String> {
    super::parse::http_url(v)?;
    let u = url::Url::parse(v).map_err(|e| e.to_string())?;
    if !u.username().is_empty() || u.password().is_some() {
        return Err(format!("`{v}` carries credentials"));
    }
    Ok(())
}

fn allow_list(v: &str) -> Result<(), String> {
    super::parse::list(v)
        .iter()
        .try_for_each(|e| allow_entry(e))
}

fn host_port(v: &str) -> Result<(), String> {
    let (host, port) = v
        .rsplit_once(':')
        .ok_or_else(|| format!("`{v}` is not host:port"))?;
    if host.is_empty() || port.parse::<u16>().is_err() {
        return Err(format!("`{v}` is not host:port"));
    }
    Ok(())
}

fn bind_address(v: &str) -> Result<(), String> {
    let ok = v.parse::<std::net::IpAddr>().is_ok()
        || (!v.is_empty()
            && v.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'));
    if ok {
        Ok(())
    } else {
        Err(format!("`{v}` is not an IP address or host name"))
    }
}

fn frame_ancestors(v: &str) -> Result<(), String> {
    if v.eq_ignore_ascii_case("none") {
        return Err("write the CSP keyword with its quotes: 'none'".to_string());
    }
    axum::http::HeaderValue::from_str(&format!("frame-ancestors {v}"))
        .map(|_| ())
        .map_err(|_| format!("`{v}` cannot be sent in a CSP header"))
}

/// An OIDC issuer: https, or http on a loopback host (development).
fn idp_url(v: &str) -> Result<(), String> {
    if crate::auth::oidc_rs::is_secure_idp_url(v) {
        Ok(())
    } else {
        Err(format!(
            "`{v}` is not https (or a loopback http URL): the IdP's keys would be fetched \
             in cleartext"
        ))
    }
}

fn idp_url_list(v: &str) -> Result<(), String> {
    super::parse::list(v).iter().try_for_each(|e| idp_url(e))
}

fn json_object(v: &str) -> Result<(), String> {
    match serde_json::from_str::<serde_json::Value>(v) {
        Ok(serde_json::Value::Object(_)) => Ok(()),
        Ok(_) => Err("is not a JSON object".to_string()),
        Err(e) => Err(format!("is not JSON: {e}")),
    }
}

fn json_array(v: &str) -> Result<(), String> {
    match serde_json::from_str::<serde_json::Value>(v) {
        Ok(serde_json::Value::Array(_)) => Ok(()),
        Ok(_) => Err("is not a JSON array".to_string()),
        Err(e) => Err(format!("is not JSON: {e}")),
    }
}

fn guest_capabilities(v: &str) -> Result<(), String> {
    crate::auth::policy::check_guest_capabilities(v)
}

fn scope_words(_v: &str) -> Result<(), String> {
    Ok(())
}

fn iri(v: &str) -> Result<(), String> {
    oxigraph::model::NamedNode::new(v)
        .map(|_| ())
        .map_err(|_| format!("`{v}` is not an absolute IRI"))
}

fn publisher_type(v: &str) -> Result<(), String> {
    crate::dcat::authority::publisher_type_iri(v)
        .and_then(|i| oxigraph::model::NamedNode::new(i).ok())
        .map(|_| ())
        .ok_or_else(|| {
            format!(
                "`{v}` is not an ADMS publisher type (e.g. LocalAuthority, NationalAuthority, \
                 Company) or an IRI"
            )
        })
}

fn iso639_3(v: &str) -> Result<(), String> {
    if v.len() == 3 && v.chars().all(|c| c.is_ascii_alphabetic()) {
        Ok(())
    } else {
        Err(format!("`{v}` is not an ISO 639-3 code (ENG, NLD, …)"))
    }
}

fn email(v: &str) -> Result<(), String> {
    if v.contains('@') && oxigraph::model::NamedNode::new(format!("mailto:{v}")).is_ok() {
        Ok(())
    } else {
        Err(format!("`{v}` is not an e-mail address"))
    }
}

fn sha256_hex(v: &str) -> Result<(), String> {
    if v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(format!("`{v}` is not a SHA-256 hex digest"))
    }
}

/// An LLM request budget: requests per minute, `0` (AI refused), or
/// `unlimited`.
fn llm_budget(v: &str) -> Result<(), String> {
    if v.eq_ignore_ascii_case("unlimited") || v.parse::<u32>().is_ok() {
        Ok(())
    } else {
        Err(format!(
            "`{v}` is not a budget (a number of requests per minute, 0 to refuse, or unlimited)"
        ))
    }
}

fn function_graphs(v: &str) -> Result<(), String> {
    super::parse::list(v).iter().try_for_each(|g| {
        if crate::shacl::sparql_functions::is_designatable_graph(g) {
            Ok(())
        } else {
            Err(format!(
                "`{g}` may not hold SPARQL functions (use urn:system:functions or \
                 urn:system:functions:<name>)"
            ))
        }
    })
}

fn sync_required(v: &str) -> Result<(), String> {
    if v.eq_ignore_ascii_case("all") || v.parse::<u64>().is_ok_and(|n| n >= 1) {
        Ok(())
    } else {
        Err(format!("`{v}` is neither `all` nor a positive count"))
    }
}

fn cluster_members(v: &str) -> Result<(), String> {
    for m in super::parse::list(v) {
        let (id, url) = m
            .split_once('=')
            .ok_or_else(|| format!("`{m}` is not id=url"))?;
        id.trim()
            .parse::<u64>()
            .map_err(|_| format!("`{id}` is not a node number"))?;
        super::parse::http_url(url.trim())?;
    }
    Ok(())
}

// ── derived defaults ───────────────────────────────────────────────────────

/// Secure cookies whenever the browser reaches the store over https, and
/// always in production.
fn secure_cookies(ctx: &Ctx) -> Option<String> {
    let https = ctx
        .value(BASE_URL.name())
        .is_some_and(|u| u.to_ascii_lowercase().starts_with("https://"));
    Some((https || ctx.production()).to_string())
}

fn query_timeout_secs(ctx: &Ctx) -> Option<String> {
    ctx.value(SPARQL_QUERY_TIMEOUT.name())
}

fn parallel_max_triples(_: &Ctx) -> Option<String> {
    Some(crate::store::parallel_mirror::ram_aware_default_max_triples().to_string())
}

fn parallel_shards(_: &Ctx) -> Option<String> {
    Some(crate::store::parallel_mirror::default_shards().to_string())
}

fn shacl_run_memory_bytes(_: &Ctx) -> Option<String> {
    Some(crate::shacl::budget::default_run_memory_bytes().to_string())
}

fn repair_max_quads(_: &Ctx) -> Option<String> {
    Some(crate::repair::sandbox::default_max_quads().to_string())
}

// ── the settings ───────────────────────────────────────────────────────────

settings! {
    // ── server ──
    DATA_DIR: PathBuf = Spec::new("OTS_DATA_DIR", Kind::Path, "server")
        .default("./data")
        .cli("data-dir")
        .short('d')
        .doc("Storage directory: the store, the identity database, keys and caches.");
    PORT: u16 = Spec::new("OTS_PORT", Kind::Int { min: 1, max: 65535 }, "server")
        .default("7878")
        .cli("port")
        .short('p')
        .doc("HTTP port to listen on.");
    BIND: String = Spec::new("OTS_BIND", Kind::Custom { what: "address", check: bind_address }, "server")
        .default("0.0.0.0")
        .cli("bind")
        .short('b')
        .doc("Address to listen on (an IP address or a host name).");
    /// The deployment profile.
    ENV: String = Spec::new("OTS_ENV", Kind::Enum(&["development", "production"]), "server")
        .default("development")
        .doc("Deployment profile. `production` refuses raw secret values and applies the production defaults.");
    BASE_URL: String = Spec::new("OTS_BASE_URL", Kind::Url, "server")
        .default("http://localhost:7878")
        .cli("base-url")
        .doc("Public base URL: linked-data IRIs, the passkey relying party, emailed links.");
    PUBLIC_BASE_URL: Option<String> = Spec::new("OTS_PUBLIC_BASE_URL", Kind::Url, "server")
        .doc("Base URL for emailed action links only, when the UI is on another origin.");
    FRONTEND_ENABLED: bool = Spec::new("OTS_FRONTEND_ENABLED", Kind::Bool, "server")
        .default("true")
        .cli("serve-frontend")
        .doc("Serve the bundled web UI at `/`; off for an API-only server.");
    PORT_FALLBACK_ENABLED: bool = Spec::new("OTS_PORT_FALLBACK_ENABLED", Kind::Bool, "server")
        .default("false")
        .cli("port-fallback")
        .doc("Bind the next free port when the configured one is in use, instead of refusing to start.");
    DISCOVERY_ENABLED: bool = Spec::new("OTS_DISCOVERY_ENABLED", Kind::Bool, "server")
        .default("false")
        .cli("discovery")
        .doc("Self-register with the service registry at `OTS_REGISTRY_URL`.");
    REGISTRY_URL: String = Spec::new("OTS_REGISTRY_URL", Kind::Url, "server")
        .default("http://localhost:8500")
        .cli("registry-url")
        .doc("Service registry base URL, used only with `OTS_DISCOVERY_ENABLED`.");
    REGISTRY_TOKEN: Option<String> = Spec::new("OTS_REGISTRY_TOKEN", Kind::Secret, "server")
        .cli("registry-token")
        .doc("Bearer token for the service registry (a secret reference).");
    TEXT_SEARCH_DIR: Option<PathBuf> = Spec::new("OTS_TEXT_SEARCH_DIR", Kind::Path, "server")
        .cli("text-search-dir")
        .doc("Full-text index directory (default `<data dir>/tantivy`).");
    SEED_DIR: Option<PathBuf> = Spec::new("OTS_SEED_DIR", Kind::Path, "server")
        .cli("seed-dir")
        .doc("Directory of seed bundles loaded at boot.");
    PREFIX_CC_FALLBACK_ENABLED: bool = Spec::new("OTS_PREFIX_CC_FALLBACK_ENABLED", Kind::Bool, "server")
        .default("false")
        .doc("Ask prefix.cc for prefixes the bundled snapshot does not know.");

    // ── http ──
    CORS_ORIGINS: Vec<String> = Spec::new("OTS_CORS_ORIGINS", Kind::Custom { what: "origin list", check: origin_list }, "http")
        .cli("cors-origins")
        .doc("Origins allowed to call the API with credentials, comma-separated; `*` mirrors any origin.");
    TRUSTED_PROXY_CIDRS: Vec<String> = Spec::new("OTS_TRUSTED_PROXY_CIDRS", Kind::Custom { what: "CIDR list", check: cidr_list }, "http")
        .cli("trusted-proxy-cidrs")
        .doc("Reverse proxies whose X-Forwarded-For is believed, as CIDR blocks.");
    SECURE_COOKIES_ENABLED: bool = Spec::new("OTS_SECURE_COOKIES_ENABLED", Kind::Bool, "http")
        .derived("true when OTS_BASE_URL is https or OTS_ENV=production", secure_cookies)
        .cli("secure-cookies")
        .doc("Issue auth cookies with the `Secure` attribute (HTTPS only); production refuses `false`.");
    RATE_LIMIT_ENABLED: bool = Spec::new("OTS_RATE_LIMIT_ENABLED", Kind::Bool, "http")
        .default("true")
        .doc("Per-IP rate limiting; `false` is for trusted or test deployments only, and production refuses it.");
    EMBED_FRAME_ANCESTORS: String = Spec::new("OTS_EMBED_FRAME_ANCESTORS", Kind::Custom { what: "CSP sources", check: frame_ancestors }, "http")
        .profile(Some("*"), Some("'none'"))
        .doc("Who may frame the `/embed/*` pages (CSP frame-ancestors); `'none'` disables embedding.")
        .runtime();
    MAX_UPLOAD_BYTES: Option<u64> = Spec::new("OTS_MAX_UPLOAD_BYTES", Kind::Bytes { min: 1 }, "http")
        .doc("Request-body limit for RDF uploads (default 512 MiB for the Graph Store, 1 GiB for bulk import).");
    ENDPOINT_ACL_ENABLED: bool = Spec::new("OTS_ENDPOINT_ACL_ENABLED", Kind::Bool, "http")
        .default("true")
        .doc("Enforce the endpoint ACL rules.");
    SPARQL_QUERY_TIMEOUT: Duration = Spec::new("OTS_SPARQL_QUERY_TIMEOUT_SECS", at_least(1), "http")
        .default("30")
        .cli("query-timeout-secs")
        .doc("SPARQL query and update timeout.");
    WRITE_TIMEOUT: Duration = Spec::new("OTS_WRITE_TIMEOUT_SECS", at_least(1), "http")
        .default("120")
        .cli("write-timeout-secs")
        .doc("Timeout for Graph Store and dataset writes (bulk import is exempt).");

    // ── auth ──
    JWT_SECRET: Option<String> = Spec::new("OTS_JWT_SECRET", Kind::Secret, "auth")
        .cli("jwt-secret")
        .doc("JWT signing secret (a secret reference); generated and kept under the data dir when unset.");
    AUTH_DB_PATH: Option<PathBuf> = Spec::new("OTS_AUTH_DB_PATH", Kind::Path, "auth")
        .cli("db-path")
        .doc("The identity database (default `<data dir>/auth.db`).");
    ACCESS_TOKEN_EXPIRY: Duration = Spec::new("OTS_ACCESS_TOKEN_EXPIRY_SECS", at_least(60), "auth")
        .default("1800")
        .cli("access-token-expiry-secs")
        .doc("Lifetime of a session access token.");
    REFRESH_TOKEN_EXPIRY: Duration = Spec::new("OTS_REFRESH_TOKEN_EXPIRY_DAYS", at_least(1), "auth")
        .default("30")
        .cli("refresh-token-expiry-days")
        .doc("Lifetime of a refresh token.");
    REGISTRATION_ENABLED: bool = Spec::new("OTS_REGISTRATION_ENABLED", Kind::Bool, "auth")
        .profile(Some("true"), Some("false"))
        .doc("Open public self-registration (the first account can always register, to bootstrap).")
        .runtime();
    LOGIN_EMAIL_VERIFICATION_ENABLED: bool = Spec::new("OTS_LOGIN_EMAIL_VERIFICATION_ENABLED", Kind::Bool, "auth")
        .default("false")
        .doc("Password login requires a verified e-mail address.");
    GUEST_CAPABILITIES: String = Spec::new("OTS_GUEST_CAPABILITIES", Kind::Custom { what: "capabilities", check: guest_capabilities }, "auth")
        .default("read")
        .doc("What the guest role may do beyond reading: write, create_datasets, api_tokens, publish, all.")
        .runtime();
    OIDC_PROVIDER_TOKEN_POLICY: String = Spec::new("OTS_OIDC_PROVIDER_TOKEN_POLICY", Kind::Enum(&["session", "scoped", "full"]), "auth")
        .profile(Some("session"), Some("scoped"))
        .doc("What an access token this store issues as an OIDC provider may do.")
        .runtime();
    OIDC_PROVIDER_WRITE_SCOPES: Option<String> = Spec::new("OTS_OIDC_PROVIDER_WRITE_SCOPES", Kind::Custom { what: "scopes", check: scope_words }, "auth")
        .doc("Extra scope values that grant write to a provider token under the `scoped` policy.")
        .runtime();
    OIDC_IDP_TOKEN_POLICY: String = Spec::new("OTS_OIDC_IDP_TOKEN_POLICY", Kind::Enum(&["session", "scoped", "full"]), "auth")
        .default("session")
        .doc("What an access token the external IdP issues may do.")
        .runtime();
    OIDC_IDP_WRITE_SCOPES: Option<String> = Spec::new("OTS_OIDC_IDP_WRITE_SCOPES", Kind::Custom { what: "scopes", check: scope_words }, "auth")
        .doc("Extra scope values that grant write to an IdP token under the `scoped` policy.")
        .runtime();
    OIDC_ISSUER: Option<String> = Spec::new("OTS_OIDC_ISSUER", Kind::Custom { what: "issuer URL", check: idp_url }, "auth")
        .doc("External IdP whose access tokens this store accepts (https).");
    OIDC_AUDIENCE: Option<String> = Spec::new("OTS_OIDC_AUDIENCE", Kind::Text, "auth")
        .doc("The audience IdP tokens must carry (required with `OTS_OIDC_ISSUER`).");
    OIDC_DEFAULT_ROLE: String = Spec::new("OTS_OIDC_DEFAULT_ROLE", Kind::Enum(&["user", "guest"]), "auth")
        .default("user")
        .doc("Role of an account first seen through an IdP token (admin comes only from `OTS_OIDC_ROLE_CLAIM_MAP`).");
    SESSION_AND_API_TOKENS_ENABLED: bool = Spec::new("OTS_SESSION_AND_API_TOKENS_ENABLED", Kind::Bool, "auth")
        .default("true")
        .doc("With an IdP configured, keep accepting this store's own session and API tokens.");
    OIDC_ROLE_CLAIMS: Vec<String> = Spec::new("OTS_OIDC_ROLE_CLAIMS", Kind::List, "auth")
        .default("roles,realm_access.roles,groups")
        .doc("Claims a role is read from, in order.");
    OIDC_GROUPS_CLAIM: String = Spec::new("OTS_OIDC_GROUPS_CLAIM", Kind::Text, "auth")
        .default("groups")
        .doc("Claim holding organisation groups.");
    OIDC_ORG_GROUP_PREFIX: String = Spec::new("OTS_OIDC_ORG_GROUP_PREFIX", Kind::Text, "auth")
        .default("org:")
        .doc("Prefix marking an organisation-membership group.");
    OIDC_ROLE_CLAIM_MAP: Option<String> = Spec::new("OTS_OIDC_ROLE_CLAIM_MAP", Kind::Custom { what: "JSON object", check: json_object }, "auth")
        .doc("JSON map from a claim value to a role (admin, user, guest, publisher).");
    OAUTH_CLIENTS_JSON: Option<String> = Spec::new("OTS_OAUTH_CLIENTS_JSON", Kind::Custom { what: "JSON array", check: json_array }, "auth")
        .doc("OIDC clients seeded at boot (JSON array; secrets as references).");
    TRUSTED_ISSUERS: Vec<String> = Spec::new("OTS_TRUSTED_ISSUERS", Kind::Custom { what: "issuer URLs", check: idp_url_list }, "auth")
        .doc("Peer instances whose identity assertions this store accepts.");
    LDP_ROOT_ACL: String = Spec::new("OTS_LDP_ROOT_ACL", Kind::Enum(&["open", "owners"]), "auth")
        .profile(Some("open"), Some("owners"))
        .doc("What the LDP root ACL is seeded with on first start: `open` (every user reads and writes) or `owners` (admins only).");
    REMOTE_AUTH: String = Spec::new("OTS_REMOTE_AUTH", Kind::Enum(&["none", "assert"]), "auth")
        .default("none")
        .doc("`assert`: send an identity assertion with SERVICE and LDES requests to peers.");

    // ── storage ──
    S3_ENDPOINT: Option<String> = Spec::new("OTS_S3_ENDPOINT", Kind::Url, "storage")
        .cli("s3-endpoint")
        .doc("S3-compatible endpoint for assets; local files when unset.");
    S3_BUCKET: String = Spec::new("OTS_S3_BUCKET", Kind::Text, "storage")
        .default("triplestore-assets")
        .cli("s3-bucket")
        .doc("S3 bucket for assets.");
    S3_ACCESS_KEY: Option<String> = Spec::new("OTS_S3_ACCESS_KEY", Kind::Text, "storage")
        .cli("s3-access-key")
        .doc("S3 access key id.");
    S3_SECRET_KEY: Option<String> = Spec::new("OTS_S3_SECRET_KEY", Kind::Secret, "storage")
        .cli("s3-secret-key")
        .doc("S3 secret key (a secret reference).");
    S3_REGION: String = Spec::new("OTS_S3_REGION", Kind::Text, "storage")
        .default("us-east-1")
        .cli("s3-region")
        .doc("S3 region.");
    STORE_AUTO_RECOVER_ENABLED: bool = Spec::new("OTS_STORE_AUTO_RECOVER_ENABLED", Kind::Bool, "storage")
        .profile(Some("true"), Some("false"))
        .doc("On a corrupt store at startup, quarantine it and restore the newest unencrypted backup; off, the start fails with a remediation message (the production default).");
    CLAMAV_ADDR: Option<String> = Spec::new("OTS_CLAMAV_ADDR", Kind::Custom { what: "host:port", check: host_port }, "storage")
        .doc("ClamAV daemon that scans uploads; unset disables scanning.");

    // ── backup ──
    BACKUP_DIR: Option<PathBuf> = Spec::new("OTS_BACKUP_DIR", Kind::Path, "backup")
        .doc("Backup directory (default `<data dir>/backups`).");
    BACKUP_RETENTION_COUNT: usize = Spec::new("OTS_BACKUP_RETENTION_COUNT", at_least(1), "backup")
        .default("7")
        .doc("Backups kept.");
    BACKUP_SCHEDULE: Duration = Spec::new("OTS_BACKUP_SCHEDULE_SECS", at_least(60), "backup")
        .default("86400")
        .doc("Time between scheduled backups.");
    BACKUP_ENCRYPTION_ENABLED: bool = Spec::new("OTS_BACKUP_ENCRYPTION_ENABLED", Kind::Bool, "backup")
        .default("false")
        .doc("Encrypt backups with age.");
    BACKUP_ENCRYPTION_KEY_PATH: Option<PathBuf> = Spec::new("OTS_BACKUP_ENCRYPTION_KEY_PATH", Kind::Path, "backup")
        .doc("The age recipient file (default `<data dir>/backup_key.age`).");
    BACKUP_DECRYPTION_IDENTITY_PATH: Option<PathBuf> = Spec::new("OTS_BACKUP_DECRYPTION_IDENTITY_PATH", Kind::Path, "backup")
        .doc("The age identity `--restore` decrypts an encrypted backup with.");
    BACKUP_S3_ENABLED: bool = Spec::new("OTS_BACKUP_S3_ENABLED", Kind::Bool, "backup")
        .default("false")
        .doc("Copy every backup to the S3 bucket.");
    AUDIT_PSEUDONYMISE_AFTER_DAYS: u64 = Spec::new("OTS_AUDIT_PSEUDONYMISE_AFTER_DAYS", at_least(1), "backup")
        .default("365")
        .doc("Audit rows older than this are pseudonymised.");

    // ── query engine ──
    PARALLEL_QUERY_ENABLED: bool = Spec::new("OTS_PARALLEL_QUERY_ENABLED", Kind::Bool, "query")
        .default("true")
        .doc("The in-memory query accelerator.");
    PARALLEL_QUERY_MAX_TRIPLES: usize = Spec::new("OTS_PARALLEL_QUERY_MAX_TRIPLES", at_least(1), "query")
        .derived("from the memory limit", parallel_max_triples)
        .doc("Above this many triples the accelerator stays off.");
    PARALLEL_QUERY_SHARDS: usize = Spec::new("OTS_PARALLEL_QUERY_SHARDS", Kind::Int { min: 1, max: 16 }, "query")
        .derived("the core count, at most 16", parallel_shards)
        .doc("Accelerator shard count.");
    PARALLEL_QUERY_REBUILD_QUIET: Duration = Spec::new("OTS_PARALLEL_QUERY_REBUILD_QUIET_MS", at_least(0), "query")
        .default("500")
        .doc("Quiet period after a write before the accelerator rebuilds; 0 rebuilds eagerly.");
    PARALLEL_QUERY_BACKGROUND_QUIET: Duration = Spec::new("OTS_PARALLEL_QUERY_BACKGROUND_QUIET_MS", at_least(0), "query")
        .default("5000")
        .doc("Quiet period after a write before a large store's accelerator rebuilds in the background.");
    APPLY_DELTA_TX_MAX_QUADS: usize = Spec::new("OTS_APPLY_DELTA_TX_MAX_QUADS", at_least(1), "query")
        .default("200000")
        .doc("Quads a version restore applies in one transaction (tier 1), and one transaction of a delta build writes; keep it at or below OTS_CHANGE_CAPTURE_MAX_PAYLOAD.");
    CHECKPOINT_EVERY: usize = Spec::new("OTS_CHECKPOINT_EVERY", at_least(1), "query")
        .default("100")
        .doc("A dataset version is cut as a full checkpoint after this many delta versions.");
    CHECKPOINT_FRACTION: f64 = Spec::new("OTS_CHECKPOINT_FRACTION", Kind::Ratio { min: 0.0, max: 1.0 }, "query")
        .default("0.25")
        .doc("A version whose changes exceed this fraction of its base is cut as a checkpoint.");
    MATERIALISATION_MAX_QUADS: u64 = Spec::new("OTS_MATERIALISATION_MAX_QUADS", at_least(1), "query")
        .default("2000000")
        .doc("Largest version materialisation built on request.");
    MATERIALISATION_TTL: Duration = Spec::new("OTS_MATERIALISATION_TTL_SECS", at_least(0), "query")
        .default("3600")
        .doc("How long a materialised version is kept after its last use.");
    STUDIO_INCREMENTAL_MAX: usize = Spec::new("OTS_STUDIO_INCREMENTAL_MAX", at_least(0), "shacl")
        .default("10000")
        .doc("Changed focus nodes a write-triggered pipeline validates incrementally; above it the run is full.");
    COLUMNAR_QUERY_ENABLED: bool = Spec::new("OTS_COLUMNAR_QUERY_ENABLED", Kind::Bool, "query")
        .default("true")
        .doc("The columnar aggregate engine.");
    QUERY_CACHE_ENABLED: bool = Spec::new("OTS_QUERY_CACHE_ENABLED", Kind::Bool, "query")
        .default("true")
        .doc("The query result cache.");
    QUERY_CACHE_ENTRIES: usize = Spec::new("OTS_QUERY_CACHE_ENTRIES", at_least(1), "query")
        .default("1024")
        .doc("Queries the cache holds.");
    QUERY_CACHE_MAX_ROWS: usize = Spec::new("OTS_QUERY_CACHE_MAX_ROWS", at_least(0), "query")
        .default("10000")
        .doc("Rows a cached result may have.");
    TELEMETRY_QUERY_RING: usize = Spec::new("OTS_TELEMETRY_QUERY_RING", at_least(1), "query")
        .default("8192")
        .doc("Recent queries kept for telemetry.");
    TELEMETRY_VALIDATION_RING: usize = Spec::new("OTS_TELEMETRY_VALIDATION_RING", at_least(1), "query")
        .default("1024")
        .doc("Recent validation runs kept for telemetry.");
    TELEMETRY_TIMING_STRIDE: usize = Spec::new("OTS_TELEMETRY_TIMING_STRIDE", at_least(1), "query")
        .default("8")
        .doc("Time one query in this many.");
    CHANGE_CAPTURE_ENABLED: bool = Spec::new("OTS_CHANGE_CAPTURE_ENABLED", Kind::Bool, "query")
        .default("false")
        .doc("Keep the change log (always on for a replication leader).");
    CHANGE_CAPTURE_MAX_SCAN: usize = Spec::new("OTS_CHANGE_CAPTURE_MAX_SCAN", at_least(0), "query")
        .default("250000")
        .doc("Quads a WHERE update's graphs may hold for a before-image to be taken (about 5 µs and 0.8 KiB each).");
    CHANGE_CAPTURE_MAX_PAYLOAD: usize = Spec::new("OTS_CHANGE_CAPTURE_MAX_PAYLOAD", at_least(0), "query")
        .default("250000")
        .doc("Quads a full change-log row may carry.");
    CHANGE_RETENTION_DAYS: u64 = Spec::new("OTS_CHANGE_RETENTION_DAYS", at_least(0), "query")
        .default("90")
        .doc("Change-log rows kept this long (below the lowest live cursor; 0 sweeps them at once).");
    CURSOR_TTL_DAYS: u64 = Spec::new("OTS_CURSOR_TTL_DAYS", at_least(1), "query")
        .default("30")
        .doc("A change-log cursor idle this long is dropped.");

    // ── outbound ──
    SERVICE_ALLOWLIST: Vec<String> = Spec::new("OTS_SERVICE_ALLOWLIST", Kind::Custom { what: "URL list", check: allow_list }, "outbound")
        .doc("URLs a SPARQL `SERVICE` clause may contact (any /sparql caller); empty: none.");
    LDES_ALLOWLIST: Vec<String> = Spec::new("OTS_LDES_ALLOWLIST", Kind::Custom { what: "URL list", check: allow_list }, "outbound")
        .doc("URLs an LDES client sync may read (admin); empty: none.");
    SOURCES_ALLOWLIST: Vec<String> = Spec::new("OTS_SOURCES_ALLOWLIST", Kind::Custom { what: "URL list", check: allow_list }, "outbound")
        .doc("Endpoints SQL and SPARQL datasources may reach (admin); empty: none.");
    JSONLD_CONTEXT_ALLOWLIST: Vec<String> = Spec::new("OTS_JSONLD_CONTEXT_ALLOWLIST", Kind::Custom { what: "URL list", check: allow_list }, "outbound")
        .doc("Remote JSON-LD `@context` URLs an upload may load (the bundled W3C contexts need none); empty: none.");
    OFFLINE: bool = Spec::new("OTS_OFFLINE", Kind::Bool, "outbound")
        .profile(Some("false"), Some("true"))
        .doc("Make no download at startup (the LOV corpus, the demo IFC and STL models).");
    REMOTE_TIMEOUT: Duration = Spec::new("OTS_REMOTE_TIMEOUT_SECS", at_least(1), "outbound")
        .default("10")
        .doc("Timeout of one outbound request.");
    REMOTE_MAX_BYTES: u64 = Spec::new("OTS_REMOTE_MAX_BYTES", Kind::Bytes { min: 1 }, "outbound")
        .default("67108864")
        .doc("Largest outbound response body.");
    SERVICE_MAX_ROWS: usize = Spec::new("OTS_SERVICE_MAX_ROWS", at_least(1), "outbound")
        .default("10000")
        .doc("Rows one SERVICE call may return.");
    SERVICE_MAX_ENDPOINTS: usize = Spec::new("OTS_SERVICE_MAX_ENDPOINTS", at_least(1), "outbound")
        .default("16")
        .doc("Distinct SERVICE endpoints one query may contact.");
    SERVICE_MAX_CALLS: usize = Spec::new("OTS_SERVICE_MAX_CALLS", at_least(1), "outbound")
        .default("64")
        .doc("SERVICE requests one query may make.");
    SERVICE_DEADLINE: Duration = Spec::new("OTS_SERVICE_DEADLINE_SECS", at_least(1), "outbound")
        .derived("OTS_SPARQL_QUERY_TIMEOUT_SECS", query_timeout_secs)
        .doc("How long after it starts a query may still make SERVICE requests.");
    LDES_RETRIES: u32 = Spec::new("OTS_LDES_RETRIES", Kind::Int { min: 0, max: 16 }, "outbound")
        .default("4")
        .doc("Retries of an LDES fetch after a retryable status.");
    LDES_MAX_RETRY_WAIT: Duration = Spec::new("OTS_LDES_MAX_RETRY_WAIT_SECS", at_least(0), "outbound")
        .default("60")
        .doc("Longest wait before an LDES retry.");
    VALIDATION_API_URL: Option<String> = Spec::new("OTS_VALIDATION_API_URL", Kind::Url, "outbound")
        .doc("External validation service for validate-and-commit.");
    JSONLD_CONTEXT_MAX_BYTES: u64 = Spec::new("OTS_JSONLD_CONTEXT_MAX_BYTES", Kind::Bytes { min: 1 }, "outbound")
        .default("1048576")
        .doc("Largest remote JSON-LD context fetched.");

    // ── LDES ──
    LDES_SWEEP_INTERVAL: Duration = Spec::new("OTS_LDES_SWEEP_INTERVAL_SECS", at_least(0), "ldes")
        .default("60")
        .doc("A dataset's member log is pruned at most this often from the write path.");
    LDES_BNODE_DEPTH: usize = Spec::new("OTS_LDES_BNODE_DEPTH", at_least(0), "ldes")
        .default("8")
        .doc("How far up a blank-node tree the owner of a changed blank node is looked for.");
    LDES_BNODE_SUBJECTS_MAX: usize = Spec::new("OTS_LDES_BNODE_SUBJECTS_MAX", at_least(0), "ldes")
        .default("10000")
        .doc("Above this many changed blank-node subjects in one graph, the graph is republished whole.");
    LDES_MAX_IN_FLIGHT: usize = Spec::new("OTS_LDES_MAX_IN_FLIGHT", at_least(1), "ldes")
        .default("64")
        .doc("LDES page builds running at once.");
    LDES_MAX_IN_FLIGHT_PER_STREAM: usize = Spec::new("OTS_LDES_MAX_IN_FLIGHT_PER_STREAM", at_least(1), "ldes")
        .default("16")
        .doc("LDES page builds running at once for one stream.");

    // ── sources ──
    SOURCES_DIR: Option<PathBuf> = Spec::new("OTS_SOURCES_DIR", Kind::Path, "sources")
        .doc("Directory file-backed datasources must live under.");
    SOURCES_JOIN_MAX_ROWS: usize = Spec::new("OTS_SOURCES_JOIN_MAX_ROWS", at_least(1), "sources")
        .default("1000000")
        .doc("Rows one side of an RML join may hold.");
    DRYRUN_TTL: Duration = Spec::new("OTS_DRYRUN_TTL_SECS", at_least(1), "sources")
        .default("900")
        .doc("How long a dry run's scratch graph is kept.");
    REVIEW_MAX_ITEMS: usize = Spec::new("OTS_REVIEW_MAX_ITEMS", at_least(1), "sources")
        .default("500")
        .doc("Review items one refused run may open.");
    RML_MAX_SOURCE_BYTES: u64 = Spec::new("OTS_RML_MAX_SOURCE_BYTES", Kind::Bytes { min: 1 }, "sources")
        .default("268435456")
        .doc("Largest RML source read (decompressed).");

    // ── reasoning ──
    DL_BACKEND: Option<String> = Spec::new("OTS_DL_BACKEND", Kind::Enum(&["native", "konclude", "sidecar"]), "reasoning")
        .doc("OWL 2 DL backend; unset: the owl2-dl regime answers 503.");
    KONCLUDE_BIN: String = Spec::new("OTS_KONCLUDE_BIN", Kind::Text, "reasoning")
        .default("Konclude")
        .doc("The Konclude binary.");
    REASONER_URL: Option<String> = Spec::new("OTS_REASONER_URL", Kind::Url, "reasoning")
        .doc("The DL reasoner sidecar.");
    REASONER_TOKEN: Option<String> = Spec::new("OTS_REASONER_TOKEN", Kind::Secret, "reasoning")
        .doc("Token shared with the reasoner sidecar (a secret reference).");
    REASONER_TIMEOUT: Duration = Spec::new("OTS_REASONER_TIMEOUT_SECS", at_least(1), "reasoning")
        .default("300")
        .doc("Timeout of one DL run.");
    REASONER_MAX_TRIPLES: usize = Spec::new("OTS_REASONER_MAX_TRIPLES", at_least(1), "reasoning")
        .default("1000000")
        .doc("Largest input a DL run takes.");
    DL_DEBOUNCE: Duration = Spec::new("OTS_DL_DEBOUNCE_MS", at_least(0), "reasoning")
        .default("2000")
        .doc("Quiet period after a write before a dataset's DL run starts.");
    GEOSPARQL_QUERY_REWRITE_ENABLED: bool = Spec::new("OTS_GEOSPARQL_QUERY_REWRITE_ENABLED", Kind::Bool, "reasoning")
        .default("true")
        .doc("GeoSPARQL query rewrite of relation patterns.");
    REPAIR_CONCURRENCY: usize = Spec::new("OTS_REPAIR_CONCURRENCY", at_least(1), "reasoning")
        .default("1")
        .doc("Repair runs holding a sandbox at once.");
    REPAIR_MAX_QUADS: usize = Spec::new("OTS_REPAIR_MAX_QUADS", at_least(1), "reasoning")
        .derived("from the memory limit", repair_max_quads)
        .doc("Largest repair sandbox.");
    REPAIR_PROPOSAL_TTL_DAYS: u64 = Spec::new("OTS_REPAIR_PROPOSAL_TTL_DAYS", at_least(1), "reasoning")
        .default("30")
        .doc("Repair proposals kept this long.");
    OPM_CALC_MAX_ROWS: usize = Spec::new("OTS_OPM_CALC_MAX_ROWS", at_least(1), "reasoning")
        .default("10000")
        .doc("Rows an OPM calculation's query may produce.");

    // ── SHACL ──
    SHACL_RUN_INDEX_MIN_PROBES: usize = Spec::new("OTS_SHACL_RUN_INDEX_MIN_PROBES", at_least(0), "shacl")
        .default("20000")
        .doc("Probes before a validation run builds its own index.");
    SHACL_RUN_MEMORY_BYTES: u64 = Spec::new("OTS_SHACL_RUN_MEMORY_BYTES", Kind::Bytes { min: 1 }, "shacl")
        .derived("a quarter of the memory limit", shacl_run_memory_bytes)
        .doc("Memory validation runs may hold at once for their class sets, indexes, path-closure memos and results.");
    SHACL_RUN_INDEX_MAX_QUADS: Option<usize> = Spec::new("OTS_SHACL_RUN_INDEX_MAX_QUADS", at_least(1), "shacl")
        .doc("Quads a validation run's index may hold; unset, the index is sized by OTS_SHACL_RUN_MEMORY_BYTES.");
    SPARQL_FUNCTION_GRAPHS: Vec<String> = Spec::new("OTS_SPARQL_FUNCTION_GRAPHS", Kind::Custom { what: "graph list", check: function_graphs }, "shacl")
        .doc("Graphs whose sh:SPARQLFunction definitions become callable functions.");

    // ── data ──
    TILES3D_MAX_FEATURES: usize = Spec::new("OTS_TILES3D_MAX_FEATURES", at_least(1), "data")
        .default("10000")
        .doc("Features one 3D tileset carries.");
    ICDD_ONTOLOGY_DIR: Option<PathBuf> = Spec::new("OTS_ICDD_ONTOLOGY_DIR", Kind::Path, "data")
        .doc("Directory holding ISO 21597 Container.rdf and Linkset.rdf for ICDD exports.");
    DCAT_PROFILE: String = Spec::new("OTS_DCAT_PROFILE", Kind::Enum(&["dcat", "dcat-ap", "dcat-ap-nl"]), "data")
        .default("dcat")
        .doc("The catalogue's application profile.");
    CATALOG_TITLE: Option<String> = Spec::new("OTS_CATALOG_TITLE", Kind::Text, "data")
        .doc("Catalogue title.");
    CATALOG_DESCRIPTION: Option<String> = Spec::new("OTS_CATALOG_DESCRIPTION", Kind::Text, "data")
        .doc("Catalogue description.");
    CATALOG_PUBLISHER_URI: Option<String> = Spec::new("OTS_CATALOG_PUBLISHER_URI", Kind::Custom { what: "IRI", check: iri }, "data")
        .doc("Catalogue publisher IRI.");
    CATALOG_PUBLISHER_NAME: Option<String> = Spec::new("OTS_CATALOG_PUBLISHER_NAME", Kind::Text, "data")
        .doc("Catalogue publisher name.");
    CATALOG_PUBLISHER_IDENTIFIER: Option<String> = Spec::new("OTS_CATALOG_PUBLISHER_IDENTIFIER", Kind::Text, "data")
        .doc("Catalogue publisher identifier (e.g. an OIN).");
    CATALOG_PUBLISHER_TYPE: Option<String> = Spec::new("OTS_CATALOG_PUBLISHER_TYPE", Kind::Custom { what: "publisher type", check: publisher_type }, "data")
        .doc("ADMS publisher type.");
    CATALOG_LANGUAGE: Option<String> = Spec::new("OTS_CATALOG_LANGUAGE", Kind::Custom { what: "ISO 639-3", check: iso639_3 }, "data")
        .doc("Catalogue language (ISO 639-3).");
    CATALOG_LICENSE: Option<String> = Spec::new("OTS_CATALOG_LICENSE", Kind::Custom { what: "IRI", check: iri }, "data")
        .doc("Catalogue licence IRI.");
    CATALOG_CONTACT_NAME: Option<String> = Spec::new("OTS_CATALOG_CONTACT_NAME", Kind::Text, "data")
        .doc("Catalogue contact name.");
    CATALOG_CONTACT_EMAIL: Option<String> = Spec::new("OTS_CATALOG_CONTACT_EMAIL", Kind::Custom { what: "e-mail", check: email }, "data")
        .doc("Catalogue contact e-mail.");
    VOID_PARTITION_LIMIT: usize = Spec::new("OTS_VOID_PARTITION_LIMIT", at_least(0), "data")
        .default("100")
        .doc("VoID class and property partitions listed per kind.");
    VOID_PARTITION_MAX_TRIPLES: usize = Spec::new("OTS_VOID_PARTITION_MAX_TRIPLES", at_least(0), "data")
        .default("5000000")
        .doc("Datasets larger than this get VoID counts but no partitions.");

    // ── seeding ──
    SEED_STANDARDS_DEMO_ENABLED: bool = Spec::new("OTS_SEED_STANDARDS_DEMO_ENABLED", Kind::Bool, "seed")
        .profile(Some("true"), Some("false"))
        .doc("Seed the standards demo datasets at boot.");
    SEED_STANDARD_VOCABS_ENABLED: bool = Spec::new("OTS_SEED_STANDARD_VOCABS_ENABLED", Kind::Bool, "seed")
        .default("true")
        .doc("Seed the standard vocabularies at boot.");
    SEED_IFC_URL: Option<String> = Spec::new("OTS_SEED_IFC_URL", Kind::Url, "seed")
        .doc("Override the headline IFC demo model (downloaded at boot unless OTS_OFFLINE).");
    VOCAB_CORPUS_PATH: Option<PathBuf> = Spec::new("OTS_VOCAB_CORPUS_PATH", Kind::Path, "seed")
        .doc("A local LOV corpus file.");
    VOCAB_CORPUS_URL: String = Spec::new("OTS_VOCAB_CORPUS_URL", Kind::Url, "seed")
        .default(crate::vocab_search::corpus::DEFAULT_CORPUS_URL)
        .doc("Where the LOV corpus is downloaded from at boot (unless OTS_OFFLINE).");
    VOCAB_CORPUS_SHA256: Option<String> = Spec::new("OTS_VOCAB_CORPUS_SHA256", Kind::Custom { what: "SHA-256", check: sha256_hex }, "seed")
        .doc("Expected digest of a custom LOV corpus.");
    VOCAB_LOCAL_METRICS_ENABLED: bool = Spec::new("OTS_VOCAB_LOCAL_METRICS_ENABLED", Kind::Bool, "seed")
        .default("true")
        .doc("Rank vocabulary search with this store's own usage.");

    // ── LLM ──
    LLM_GATEWAY_URL: Option<String> = Spec::new("OTS_LLM_GATEWAY_URL", Kind::Url, "llm")
        .doc("OpenAI-compatible endpoint for the AI features; unset: the AI features are off.");
    LLM_MODEL: String = Spec::new("OTS_LLM_MODEL", Kind::Text, "llm")
        .default("default")
        .doc("Model sent with every completion.");
    LLM_SPARQL_MODEL: Option<String> = Spec::new("OTS_LLM_SPARQL_MODEL", Kind::Text, "llm")
        .doc("Model for NL→SPARQL (default `OTS_LLM_MODEL`).");
    LLM_SHACL_MODEL: Option<String> = Spec::new("OTS_LLM_SHACL_MODEL", Kind::Text, "llm")
        .doc("Model for the SHACL assistant (default `OTS_LLM_MODEL`).");
    LLM_CHAT_MODEL: Option<String> = Spec::new("OTS_LLM_CHAT_MODEL", Kind::Text, "llm")
        .doc("Model for the chat assistant (default `OTS_LLM_MODEL`).");
    LLM_API_KEY: Option<String> = Spec::new("OTS_LLM_API_KEY", Kind::Secret, "llm")
        .doc("Bearer token for the endpoint (a secret reference).");
    LLM_TIMEOUT: Duration = Spec::new("OTS_LLM_TIMEOUT_SECS", at_least(1), "llm")
        .default("120")
        .doc("Timeout of one completion.");
    LLM_CONTEXT_TOKENS: Option<usize> = Spec::new("OTS_LLM_CONTEXT_TOKENS", at_least(0), "llm")
        .doc("The model's context window; 0 or unset: no client-side budgeting.");
    LLM_CHAT_MAX_ROUNDS: usize = Spec::new("OTS_LLM_CHAT_MAX_ROUNDS", Kind::Int { min: 1, max: 8 }, "llm")
        .default("3")
        .doc("Query rounds one chat turn may use.");
    LLM_CHAT_QUERY_MAX: Duration = Spec::new("OTS_LLM_CHAT_QUERY_MAX_SECS", Kind::Int { min: 5, max: 600 }, "llm")
        .default("30")
        .doc("Per-round query cap of a chat turn.");
    LLM_CHAT_TOOLS: String = Spec::new("OTS_LLM_CHAT_TOOLS", Kind::Enum(&["auto", "off"]), "llm")
        .default("auto")
        .doc("Offer native tool calling to the model (`auto`) or not (`off`).");
    LLM_GUARD_MAX_MESSAGE_CHARS: usize = Spec::new("OTS_LLM_GUARD_MAX_MESSAGE_CHARS", at_least(1), "llm")
        .default("8000")
        .doc("Longest single chat message.");
    LLM_GUARD_MAX_MESSAGES: usize = Spec::new("OTS_LLM_GUARD_MAX_MESSAGES", at_least(1), "llm")
        .default("40")
        .doc("Most messages in one conversation.");
    LLM_GUARD_MAX_TOTAL_CHARS: usize = Spec::new("OTS_LLM_GUARD_MAX_TOTAL_CHARS", at_least(1), "llm")
        .default("64000")
        .doc("Longest conversation.");
    LLM_GUARD_INJECTION_ACTION: String = Spec::new("OTS_LLM_GUARD_INJECTION_ACTION", Kind::Enum(&["off", "flag", "block"]), "llm")
        .default("block")
        .doc("What a prompt-injection heuristic hit does.");
    LLM_GUARD_BLOCKLIST: Vec<String> = Spec::new("OTS_LLM_GUARD_BLOCKLIST", Kind::List, "llm")
        .doc("Phrases that always block, comma-separated.");
    LLM_RATE_LIMIT_PER_MIN: String = Spec::new("OTS_LLM_RATE_LIMIT_PER_MIN", Kind::Custom { what: "budget", check: llm_budget }, "llm")
        .default("20")
        .doc("LLM requests per signed-in user per minute; `0` refuses AI to signed-in users, `unlimited` lifts the limit.")
        .runtime();
    LLM_RATE_LIMIT_ANON_PER_MIN: String = Spec::new("OTS_LLM_RATE_LIMIT_ANON_PER_MIN", Kind::Custom { what: "budget", check: llm_budget }, "llm")
        .default("5")
        .doc("LLM requests per anonymous client IP per minute; `0` refuses AI to anonymous callers, `unlimited` lifts the limit.")
        .runtime();
    LLM_ANONYMOUS_ENABLED: bool = Spec::new("OTS_LLM_ANONYMOUS_ENABLED", Kind::Bool, "llm")
        .profile(Some("true"), Some("false"))
        .doc("Let anonymous callers use the AI features (within their budget).")
        .runtime();
    LLM_LOG_RETENTION_DAYS: u64 = Spec::new("OTS_LLM_LOG_RETENTION_DAYS", at_least(1), "llm")
        .default("90")
        .doc("LLM request log rows kept this long.");
    LLM_LOG_PREVIEW_ENABLED: bool = Spec::new("OTS_LLM_LOG_PREVIEW_ENABLED", Kind::Bool, "llm")
        .default("true")
        .doc("Keep a short question preview in the LLM request log.");

    // ── e-mail ──
    SMTP_HOST: Option<String> = Spec::new("OTS_SMTP_HOST", Kind::Text, "email")
        .doc("SMTP relay for account e-mail; unset: messages go to the log.");
    SMTP_PORT: Option<u16> = Spec::new("OTS_SMTP_PORT", Kind::Int { min: 1, max: 65535 }, "email")
        .doc("SMTP port (default 587).");
    SMTP_USERNAME: Option<String> = Spec::new("OTS_SMTP_USERNAME", Kind::Text, "email")
        .doc("SMTP user.");
    SMTP_PASSWORD: Option<String> = Spec::new("OTS_SMTP_PASSWORD", Kind::Secret, "email")
        .doc("SMTP password (a secret reference).");
    SMTP_TLS: Option<String> = Spec::new("OTS_SMTP_TLS", Kind::Enum(&["none", "starttls", "implicit"]), "email")
        .doc("SMTP transport security (default: implicit on 465, else STARTTLS).");
    SMTP_FROM: Option<String> = Spec::new("OTS_SMTP_FROM", Kind::Text, "email")
        .doc("Sender of account e-mail.");

    // ── alerting ──
    ALERT_WEBHOOK_URL: Option<String> = Spec::new("OTS_ALERT_WEBHOOK_URL", Kind::Url, "alerting")
        .doc("Webhook operational alerts are posted to.");
    ALERT_SMTP_HOST: Option<String> = Spec::new("OTS_ALERT_SMTP_HOST", Kind::Text, "alerting")
        .doc("SMTP relay for alerts (default: the account relay).");
    ALERT_SMTP_PORT: Option<u16> = Spec::new("OTS_ALERT_SMTP_PORT", Kind::Int { min: 1, max: 65535 }, "alerting")
        .doc("Alert SMTP port.");
    ALERT_SMTP_USERNAME: Option<String> = Spec::new("OTS_ALERT_SMTP_USERNAME", Kind::Text, "alerting")
        .doc("Alert SMTP user.");
    ALERT_SMTP_PASSWORD: Option<String> = Spec::new("OTS_ALERT_SMTP_PASSWORD", Kind::Secret, "alerting")
        .doc("Alert SMTP password (a secret reference).");
    ALERT_SMTP_TLS: Option<String> = Spec::new("OTS_ALERT_SMTP_TLS", Kind::Enum(&["none", "starttls", "implicit"]), "alerting")
        .doc("Alert SMTP transport security.");
    ALERT_SMTP_FROM: Option<String> = Spec::new("OTS_ALERT_SMTP_FROM", Kind::Text, "alerting")
        .doc("Sender of alerts (default `OTS_SMTP_FROM`).");
    ALERT_SMTP_TO: Vec<String> = Spec::new("OTS_ALERT_SMTP_TO", Kind::List, "alerting")
        .doc("Alert recipients, comma-separated.");

    // ── replication ──
    REPLICATION_ROLE: String = Spec::new("OTS_REPLICATION_ROLE", Kind::Enum(&["none", "leader", "follower", "cluster"]), "replication")
        .default("none")
        .doc("This node's replication role.");
    REPLICATION_MODE: String = Spec::new("OTS_REPLICATION_MODE", Kind::Enum(&["cold", "warm", "hot"]), "replication")
        .default("warm")
        .doc("How closely a follower tracks its leader.");
    REPLICATION_GRAPHS: Option<String> = Spec::new("OTS_REPLICATION_GRAPHS", Kind::Text, "replication")
        .doc("Graphs a follower replicates (`all`, `default`, or IRIs).");
    REPLICATION_DATASETS: Option<String> = Spec::new("OTS_REPLICATION_DATASETS", Kind::Text, "replication")
        .doc("The leader's datasets a follower replicates.");
    REPLICATION_LEADER_URL: Option<String> = Spec::new("OTS_REPLICATION_LEADER_URL", Kind::Url, "replication")
        .doc("The leader a follower replicates from.");
    REPLICATION_TOKEN: Option<String> = Spec::new("OTS_REPLICATION_TOKEN", Kind::Secret, "replication")
        .doc("Token a follower presents to its leader (a secret reference).");
    REPLICATION_NODE_ID: Option<String> = Spec::new("OTS_REPLICATION_NODE_ID", Kind::Text, "replication")
        .doc("This node's name (default the host name).");
    REPLICATION_POLL_MS: Option<u64> = Spec::new("OTS_REPLICATION_POLL_MS", Kind::Int { min: 50, max: 60_000 }, "replication")
        .doc("A hot follower's pacing (default 500).");
    REPLICATION_INTERVAL_SECS: Option<u64> = Spec::new("OTS_REPLICATION_INTERVAL_SECS", at_least(1), "replication")
        .doc("How often a follower catches up (default by mode).");
    REPLICATION_IDENTITY_INTERVAL_SECS: Option<u64> = Spec::new("OTS_REPLICATION_IDENTITY_INTERVAL_SECS", at_least(1), "replication")
        .doc("How often a follower checks the leader's identity database.");
    REPLICATION_SYNC_FOLLOWERS: Option<String> = Spec::new("OTS_REPLICATION_SYNC_FOLLOWERS", Kind::Text, "replication")
        .doc("Followers whose acknowledgement a leader's write waits for.");
    REPLICATION_SYNC_REQUIRED: Option<String> = Spec::new("OTS_REPLICATION_SYNC_REQUIRED", Kind::Custom { what: "count or all", check: sync_required }, "replication")
        .doc("How many of them (default 1).");
    REPLICATION_SYNC_TIMEOUT_MS: Option<u64> = Spec::new("OTS_REPLICATION_SYNC_TIMEOUT_MS", Kind::Int { min: 50, max: 60_000 }, "replication")
        .doc("How long a write waits for them (default 2000).");
    REPLICATION_CLUSTER: Option<String> = Spec::new("OTS_REPLICATION_CLUSTER", Kind::Custom { what: "id=url list", check: cluster_members }, "replication")
        .doc("Raft cluster members, `id=url,…`.");
    REPLICATION_CLUSTER_ID: Option<u64> = Spec::new("OTS_REPLICATION_CLUSTER_ID", at_least(0), "replication")
        .doc("This node's number in the cluster.");
    REPLICATION_CLUSTER_SECRET: Option<String> = Spec::new("OTS_REPLICATION_CLUSTER_SECRET", Kind::Secret, "replication")
        .doc("Secret authenticating the Raft routes (a secret reference).");
    REPLICATION_ELECTION_MS: Option<u64> = Spec::new("OTS_REPLICATION_ELECTION_MS", Kind::Int { min: 200, max: 60_000 }, "replication")
        .doc("Raft election timeout (default 1500).");
    REPLICATION_HEARTBEAT_MS: Option<u64> = Spec::new("OTS_REPLICATION_HEARTBEAT_MS", at_least(50), "replication")
        .doc("Raft heartbeat (default a fifth of the election timeout).");

    // ── secrets ──
    SECRET_CACHE_TTL: Duration = Spec::new("OTS_SECRET_CACHE_TTL_SECS", at_least(0), "secrets")
        .default("60")
        .doc("How long a resolved secret reference is cached.");
}

/// Names that no longer mean anything: setting one stops the start, with
/// the name to use instead. (They are not aliases: the old name is refused.)
pub static RETIRED: &[(&str, &str)] = &[
    ("BASE_URL", "renamed OTS_BASE_URL"),
    ("PUBLIC_BASE_URL", "renamed OTS_PUBLIC_BASE_URL"),
    ("SERVE_FRONTEND", "renamed OTS_FRONTEND_ENABLED"),
    ("PORT_FALLBACK", "renamed OTS_PORT_FALLBACK_ENABLED"),
    ("LD_DISCOVERY", "renamed OTS_DISCOVERY_ENABLED"),
    ("LD_REGISTRY_URL", "renamed OTS_REGISTRY_URL"),
    ("LD_REGISTRY_TOKEN", "renamed OTS_REGISTRY_TOKEN"),
    ("TEXT_SEARCH_DIR", "renamed OTS_TEXT_SEARCH_DIR"),
    ("SEED_DIR", "renamed OTS_SEED_DIR"),
    ("PREFIX_CC_FALLBACK", "renamed OTS_PREFIX_CC_FALLBACK_ENABLED"),
    ("CORS_ORIGINS", "renamed OTS_CORS_ORIGINS"),
    ("TRUSTED_PROXY_CIDRS", "renamed OTS_TRUSTED_PROXY_CIDRS"),
    ("SECURE_COOKIES", "renamed OTS_SECURE_COOKIES_ENABLED"),
    ("EMBED_FRAME_ANCESTORS", "renamed OTS_EMBED_FRAME_ANCESTORS"),
    ("ENDPOINT_ACL_ENFORCE", "renamed OTS_ENDPOINT_ACL_ENABLED"),
    ("SPARQL_QUERY_TIMEOUT_SECS", "renamed OTS_SPARQL_QUERY_TIMEOUT_SECS"),
    ("WRITE_TIMEOUT_SECS", "renamed OTS_WRITE_TIMEOUT_SECS"),
    ("JWT_SECRET", "renamed OTS_JWT_SECRET"),
    ("AUTH_DB_PATH", "renamed OTS_AUTH_DB_PATH"),
    ("ACCESS_TOKEN_EXPIRY_SECS", "renamed OTS_ACCESS_TOKEN_EXPIRY_SECS"),
    ("REFRESH_TOKEN_EXPIRY_DAYS", "renamed OTS_REFRESH_TOKEN_EXPIRY_DAYS"),
    ("OTS_REQUIRE_VERIFIED_EMAIL", "renamed OTS_LOGIN_EMAIL_VERIFICATION_ENABLED"),
    ("OTS_OIDC_SESSION_POLICY", "renamed OTS_OIDC_PROVIDER_TOKEN_POLICY"),
    ("OTS_OIDC_WRITE_SCOPES", "renamed OTS_OIDC_PROVIDER_WRITE_SCOPES"),
    ("OIDC_ISSUER", "renamed OTS_OIDC_ISSUER"),
    ("OIDC_AUDIENCE", "renamed OTS_OIDC_AUDIENCE"),
    ("OIDC_DEFAULT_ROLE", "renamed OTS_OIDC_DEFAULT_ROLE"),
    ("ACCEPT_LEGACY_TOKENS", "renamed OTS_SESSION_AND_API_TOKENS_ENABLED"),
    ("OIDC_ROLE_CLAIMS", "renamed OTS_OIDC_ROLE_CLAIMS"),
    ("OIDC_GROUPS_CLAIM", "renamed OTS_OIDC_GROUPS_CLAIM"),
    ("OIDC_ORG_GROUP_PREFIX", "renamed OTS_OIDC_ORG_GROUP_PREFIX"),
    ("OIDC_ROLE_CLAIM_MAP", "renamed OTS_OIDC_ROLE_CLAIM_MAP"),
    ("OAUTH_CLIENTS_JSON", "renamed OTS_OAUTH_CLIENTS_JSON"),
    ("LDP_ROOT_ACL", "renamed OTS_LDP_ROOT_ACL"),
    ("S3_ENDPOINT", "renamed OTS_S3_ENDPOINT"),
    ("S3_BUCKET", "renamed OTS_S3_BUCKET"),
    ("S3_ACCESS_KEY", "renamed OTS_S3_ACCESS_KEY"),
    ("S3_SECRET_KEY", "renamed OTS_S3_SECRET_KEY"),
    ("S3_REGION", "renamed OTS_S3_REGION"),
    ("STORE_AUTO_RECOVER", "renamed OTS_STORE_AUTO_RECOVER_ENABLED"),
    ("CLAMAV_ADDR", "renamed OTS_CLAMAV_ADDR"),
    ("BACKUP_DIR", "renamed OTS_BACKUP_DIR"),
    ("BACKUP_RETENTION_COUNT", "renamed OTS_BACKUP_RETENTION_COUNT"),
    ("BACKUP_SCHEDULE_SECS", "renamed OTS_BACKUP_SCHEDULE_SECS"),
    ("BACKUP_ENCRYPT", "renamed OTS_BACKUP_ENCRYPTION_ENABLED"),
    ("BACKUP_ENCRYPT_KEY_PATH", "renamed OTS_BACKUP_ENCRYPTION_KEY_PATH"),
    ("BACKUP_DECRYPT_IDENTITY_PATH", "renamed OTS_BACKUP_DECRYPTION_IDENTITY_PATH"),
    ("BACKUP_S3_ENABLED", "renamed OTS_BACKUP_S3_ENABLED"),
    ("AUDIT_PSEUDONYMISE_AFTER_DAYS", "renamed OTS_AUDIT_PSEUDONYMISE_AFTER_DAYS"),
    ("OTS_PARALLEL_QUERY", "renamed OTS_PARALLEL_QUERY_ENABLED"),
    ("OTS_COLUMNAR_QUERY", "renamed OTS_COLUMNAR_QUERY_ENABLED"),
    ("OTS_QUERY_CACHE", "renamed OTS_QUERY_CACHE_ENABLED"),
    ("OTS_CHANGE_CAPTURE", "renamed OTS_CHANGE_CAPTURE_ENABLED"),
    ("OTS_GEOSPARQL_QUERY_REWRITE", "renamed OTS_GEOSPARQL_QUERY_REWRITE_ENABLED"),
    ("VALIDATION_API_URL", "renamed OTS_VALIDATION_API_URL"),
    ("DCAT_PROFILE", "renamed OTS_DCAT_PROFILE"),
    ("CATALOG_TITLE", "renamed OTS_CATALOG_TITLE"),
    ("CATALOG_DESCRIPTION", "renamed OTS_CATALOG_DESCRIPTION"),
    ("CATALOG_PUBLISHER_URI", "renamed OTS_CATALOG_PUBLISHER_URI"),
    ("CATALOG_PUBLISHER_NAME", "renamed OTS_CATALOG_PUBLISHER_NAME"),
    ("CATALOG_PUBLISHER_IDENTIFIER", "renamed OTS_CATALOG_PUBLISHER_IDENTIFIER"),
    ("CATALOG_PUBLISHER_TYPE", "renamed OTS_CATALOG_PUBLISHER_TYPE"),
    ("CATALOG_LANGUAGE", "renamed OTS_CATALOG_LANGUAGE"),
    ("CATALOG_LICENSE", "renamed OTS_CATALOG_LICENSE"),
    ("CATALOG_CONTACT_NAME", "renamed OTS_CATALOG_CONTACT_NAME"),
    ("CATALOG_CONTACT_EMAIL", "renamed OTS_CATALOG_CONTACT_EMAIL"),
    ("SEED_STANDARDS_DEMO", "renamed OTS_SEED_STANDARDS_DEMO_ENABLED"),
    ("SEED_STANDARD_VOCABS", "renamed OTS_SEED_STANDARD_VOCABS_ENABLED"),
    ("SEED_IFC_URL", "renamed OTS_SEED_IFC_URL"),
    ("VOCAB_CORPUS_PATH", "renamed OTS_VOCAB_CORPUS_PATH"),
    ("VOCAB_CORPUS_URL", "renamed OTS_VOCAB_CORPUS_URL"),
    ("VOCAB_CORPUS_SHA256", "renamed OTS_VOCAB_CORPUS_SHA256"),
    ("VOCAB_LOCAL_METRICS", "renamed OTS_VOCAB_LOCAL_METRICS_ENABLED"),
    ("LLM_GATEWAY_URL", "renamed OTS_LLM_GATEWAY_URL"),
    ("LLM_MODEL", "renamed OTS_LLM_MODEL"),
    ("LLM_SPARQL_MODEL", "renamed OTS_LLM_SPARQL_MODEL"),
    ("LLM_SHACL_MODEL", "renamed OTS_LLM_SHACL_MODEL"),
    ("LLM_CHAT_MODEL", "renamed OTS_LLM_CHAT_MODEL"),
    ("LLM_API_KEY", "renamed OTS_LLM_API_KEY"),
    ("LLM_TIMEOUT_SECS", "renamed OTS_LLM_TIMEOUT_SECS"),
    ("LLM_CONTEXT_TOKENS", "renamed OTS_LLM_CONTEXT_TOKENS"),
    ("LLM_CHAT_MAX_ROUNDS", "renamed OTS_LLM_CHAT_MAX_ROUNDS"),
    ("LLM_CHAT_QUERY_MAX_SECS", "renamed OTS_LLM_CHAT_QUERY_MAX_SECS"),
    ("LLM_CHAT_TOOLS", "renamed OTS_LLM_CHAT_TOOLS"),
    ("LLM_GUARD_MAX_MESSAGE_CHARS", "renamed OTS_LLM_GUARD_MAX_MESSAGE_CHARS"),
    ("LLM_GUARD_MAX_MESSAGES", "renamed OTS_LLM_GUARD_MAX_MESSAGES"),
    ("LLM_GUARD_MAX_TOTAL_CHARS", "renamed OTS_LLM_GUARD_MAX_TOTAL_CHARS"),
    ("LLM_GUARD_INJECTION_ACTION", "renamed OTS_LLM_GUARD_INJECTION_ACTION"),
    ("LLM_GUARD_BLOCKLIST", "renamed OTS_LLM_GUARD_BLOCKLIST"),
    ("LLM_RATE_LIMIT_PER_MIN", "renamed OTS_LLM_RATE_LIMIT_PER_MIN"),
    ("LLM_RATE_LIMIT_ANON_PER_MIN", "renamed OTS_LLM_RATE_LIMIT_ANON_PER_MIN"),
    ("LLM_ANONYMOUS_ENABLED", "renamed OTS_LLM_ANONYMOUS_ENABLED"),
    ("LLM_LOG_RETENTION_DAYS", "renamed OTS_LLM_LOG_RETENTION_DAYS"),
    ("LLM_LOG_PREVIEW_ENABLED", "renamed OTS_LLM_LOG_PREVIEW_ENABLED"),
    ("SMTP_HOST", "renamed OTS_SMTP_HOST"),
    ("SMTP_PORT", "renamed OTS_SMTP_PORT"),
    ("SMTP_USERNAME", "renamed OTS_SMTP_USERNAME"),
    ("SMTP_PASSWORD", "renamed OTS_SMTP_PASSWORD"),
    ("SMTP_TLS", "renamed OTS_SMTP_TLS"),
    ("SMTP_FROM", "renamed OTS_SMTP_FROM"),
    ("ALERT_WEBHOOK_URL", "renamed OTS_ALERT_WEBHOOK_URL"),
    ("ALERT_SMTP_HOST", "renamed OTS_ALERT_SMTP_HOST"),
    ("ALERT_SMTP_PORT", "renamed OTS_ALERT_SMTP_PORT"),
    ("ALERT_SMTP_USER", "renamed OTS_ALERT_SMTP_USERNAME"),
    ("ALERT_SMTP_PASS", "renamed OTS_ALERT_SMTP_PASSWORD"),
    ("ALERT_SMTP_TLS", "renamed OTS_ALERT_SMTP_TLS"),
    ("ALERT_SMTP_FROM", "renamed OTS_ALERT_SMTP_FROM"),
    ("ALERT_SMTP_TO", "renamed OTS_ALERT_SMTP_TO"),
    ("ACCOUNTS_DASHBOARD_GATEWAY_USAGE_URL", "renamed OTS_ACCOUNTS_DASHBOARD_GATEWAY_USAGE_URL"),
    ("ACCOUNTS_DASHBOARD_GATEWAY_KEY", "renamed OTS_ACCOUNTS_DASHBOARD_GATEWAY_KEY"),
    ("ACCOUNTS_DASHBOARD_APP_GROUPS", "renamed OTS_ACCOUNTS_DASHBOARD_APP_GROUPS"),
    ("RATE_LIMIT_DISABLED", "renamed OTS_RATE_LIMIT_ENABLED, with the opposite sense: set OTS_RATE_LIMIT_ENABLED=false"),
    ("OTS_DISABLE_REGISTRATION", "renamed OTS_REGISTRATION_ENABLED, with the opposite sense: set OTS_REGISTRATION_ENABLED=false"),
    ("LLM_LOG_PREVIEW_DISABLED", "renamed OTS_LLM_LOG_PREVIEW_ENABLED, with the opposite sense: set OTS_LLM_LOG_PREVIEW_ENABLED=false"),
    ("ACCESS_TOKEN_EXPIRY_MINUTES", "renamed OTS_ACCESS_TOKEN_EXPIRY_SECS (seconds)"),
    ("BACKUP_SCHEDULE_HOURS", "renamed OTS_BACKUP_SCHEDULE_SECS (seconds)"),
    ("LLM_TIMEOUT_SECONDS", "renamed OTS_LLM_TIMEOUT_SECS"),
    ("OTS_MAX_UPLOAD_MB", "renamed OTS_MAX_UPLOAD_BYTES (bytes; 512MiB is accepted)"),
    ("OTS_REMOTE_ALLOWLIST", "split by purpose: OTS_SERVICE_ALLOWLIST (SPARQL SERVICE), OTS_LDES_ALLOWLIST, OTS_SOURCES_ALLOWLIST (datasources) and OTS_JSONLD_CONTEXT_ALLOWLIST; there is no shared list"),
    ("SMTP_STARTTLS", "removed: use OTS_SMTP_TLS=starttls|implicit|none"),
    ("OTS_EXTERNAL_REASONER", "removed: set OTS_DL_BACKEND=konclude and OTS_KONCLUDE_BIN"),
    ("OTS_EXTERNAL_REASONER_BIN", "removed: set OTS_DL_BACKEND=konclude and OTS_KONCLUDE_BIN"),
];

/// Checks across settings, run once every setting has validated on its own.
pub(super) fn checks(config: &super::Config) -> Vec<ConfigError> {
    let mut errors = Vec::new();
    let mut fail = |name: &str, message: &str| {
        errors.push(ConfigError {
            name: name.to_string(),
            message: message.to_string(),
        })
    };
    let value = |k: &str| config.value(k);

    // The production profile's refusals: settings production must not run
    // with, whatever else is configured.
    if config.production() {
        if value(SECURE_COOKIES_ENABLED.name()).as_deref() == Some("false") {
            fail(
                SECURE_COOKIES_ENABLED.name(),
                "production refuses `false`: session cookies would travel over plain HTTP. \
                 Serve the store over https (OTS_BASE_URL https://…) behind a TLS proxy",
            );
        }
        if value(RATE_LIMIT_ENABLED.name()).as_deref() == Some("false") {
            fail(
                RATE_LIMIT_ENABLED.name(),
                "production refuses turning rate limiting off",
            );
        }
        if super::parse::list(&value(CORS_ORIGINS.name()).unwrap_or_default())
            .iter()
            .any(|o| o == "*")
        {
            fail(
                CORS_ORIGINS.name(),
                "production refuses `*`: it reflects any origin with credentials; list the \
                 trusted origins",
            );
        }
        for spec in super::SPECS {
            if !matches!(spec.kind, Kind::Secret) {
                continue;
            }
            if let Some(v) = value(spec.name) {
                if !crate::secrets::looks_like_ref(&v) {
                    fail(
                        spec.name,
                        "production refuses a raw secret value: give a reference \
                         (env:NAME, file:/path or vault:…)",
                    );
                }
            }
        }
    }

    if value(OIDC_ISSUER.name()).is_some() && value(OIDC_AUDIENCE.name()).is_none() {
        fail(
            OIDC_AUDIENCE.name(),
            "is required with OTS_OIDC_ISSUER (without it every IdP token is refused)",
        );
    }
    if value(DL_BACKEND.name()).as_deref() == Some("sidecar")
        && value(REASONER_URL.name()).is_none()
    {
        fail(
            REASONER_URL.name(),
            "is required with OTS_DL_BACKEND=sidecar",
        );
    }
    match value(REPLICATION_ROLE.name()).as_deref() {
        Some("follower") if value(REPLICATION_LEADER_URL.name()).is_none() => fail(
            REPLICATION_LEADER_URL.name(),
            "is required with OTS_REPLICATION_ROLE=follower",
        ),
        Some("cluster") => {
            let members = value(REPLICATION_CLUSTER.name()).unwrap_or_default();
            let id = value(REPLICATION_CLUSTER_ID.name());
            if members.is_empty() {
                fail(
                    REPLICATION_CLUSTER.name(),
                    "is required with OTS_REPLICATION_ROLE=cluster",
                );
            }
            match id {
                None => fail(
                    REPLICATION_CLUSTER_ID.name(),
                    "is required with OTS_REPLICATION_ROLE=cluster",
                ),
                Some(id) => {
                    let listed = super::parse::list(&members).iter().any(|m| {
                        m.split_once('=')
                            .is_some_and(|(k, _)| k.trim() == id.as_str())
                    });
                    if !members.is_empty() && !listed {
                        fail(
                            REPLICATION_CLUSTER_ID.name(),
                            "is not one of the OTS_REPLICATION_CLUSTER members",
                        );
                    }
                }
            }
            if value(REPLICATION_CLUSTER_SECRET.name()).is_none() {
                fail(
                    REPLICATION_CLUSTER_SECRET.name(),
                    "is required with OTS_REPLICATION_ROLE=cluster",
                );
            }
        }
        _ => {}
    }
    errors
}
