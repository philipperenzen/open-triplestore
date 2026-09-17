//! SQL datasources: registry, connectors, mappings and runs.
//!
//! The shape of the feature:
//!
//! * a **datasource** is RDF in `urn:system:sources` — dialect, location,
//!   read-only account, statement timeout, and a *reference* to the credential
//!   in an external secret store (never the credential itself);
//! * a **connector** ([`connector`]) is the driver for one dialect. SQLite is
//!   in core; PostgreSQL, MySQL and SQL Server arrive as plugins;
//! * a **mapping** is standard RML, stored as a graph per frozen version, so
//!   the store is the mapping registry and a run can name exactly the triples
//!   it executed;
//! * a **run** ([`runs`]) materialises into a fresh graph `urn:run:<id>`,
//!   records a PROV activity, passes the SHACL write gate, and only then takes
//!   the production role — atomically. Rollback re-points; it never re-runs.
//!
//! Security posture, fail-closed in production (`OTS_ENV=production`): a raw
//! secret is refused, a statement timeout is mandatory, a networked source
//! must pass the federation egress allowlist, and a file-backed source must
//! live under `OTS_SOURCES_DIR`.

pub mod connector;
pub mod handlers;
pub mod mappings;
pub mod model;
pub mod registry;
pub mod routes;
pub mod runs;
pub mod sqlite;
pub mod yarrrml;

use std::path::{Path, PathBuf};

use crate::secrets::{self, SecretError, SecretRef};

pub use model::SqlSource;

/// Directory a file-backed datasource must live under in the production
/// posture. Unset means file-backed sources are refused there.
pub const SOURCES_DIR_ENV: &str = "OTS_SOURCES_DIR";

/// Upper-case hex, the lexical form `xsd:hexBinary` wants.
pub fn hex_upper(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02X}");
    }
    out
}

/// The configured sources directory, if any.
pub fn sources_dir() -> Option<PathBuf> {
    std::env::var(SOURCES_DIR_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Whether a source's endpoint may be contacted. A file-backed dialect has no
/// egress, so it is allowed by definition; a networked one is checked against
/// the same allowlist `SERVICE` and LDES sync use.
pub fn egress_allowed(source: &SqlSource) -> bool {
    let networked = connector::get(&source.dialect)
        .map(|c| c.is_networked())
        .unwrap_or(true);
    if !networked {
        return true;
    }
    match source.egress_url() {
        Some(url) => crate::remote::is_allowed(&url),
        // A networked dialect with no host cannot be reached at all.
        None => false,
    }
}

/// Why a datasource registration was refused. Every variant is a 400: the
/// caller supplied something the deployment does not accept.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("'{0}' is not a usable id: use letters, digits, '-', '_' or '.' (max 128)")]
    Id(String),
    #[error("no connector for dialect '{dialect}'; this build supports: {available}")]
    Dialect { dialect: String, available: String },
    #[error("a datasource needs a database name or file path")]
    Database,
    #[error(
        "the credential must be a secret reference — env:NAME, file:/path or \
         vault:<mount>/data/<path>#<key> — so the store holds a pointer and the secret stays in \
         the secret store"
    )]
    RawCredential,
    #[error(
        "the credential is empty; omit it entirely for a datasource that needs none, or give a \
         secret reference"
    )]
    EmptyCredential,
    #[error(transparent)]
    Secret(#[from] SecretError),
    #[error(
        "this store only opens datasources read-only; grant the account SELECT and register it \
         with readOnly true"
    )]
    ReadWrite,
    #[error(
        "a statement timeout is required in the production posture: set statementTimeoutMs so a \
         slow query cannot hold a connection open indefinitely"
    )]
    MissingTimeout,
    #[error("statementTimeoutMs must be between 1 and {max} milliseconds")]
    TimeoutRange { max: u64 },
    #[error(
        "a networked datasource must be on the federation egress allowlist ({env}) in the \
         production posture"
    )]
    NotAllowlisted { env: &'static str },
    #[error(
        "a file-backed datasource must live under {env} in the production posture, so the store \
         cannot be pointed at an arbitrary file on the host"
    )]
    OutsideSourcesDir { env: &'static str },
    #[error("a networked datasource needs a host")]
    MissingHost,
    #[error("dataset '{0}' does not exist")]
    UnknownDataset(String),
}

/// The largest statement timeout that still means "bounded".
const MAX_TIMEOUT_MS: u64 = 3_600_000;
/// Applied when a request omits the timeout outside the production posture.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Parse a datasource credential. Always a reference: unlike the settings
/// migrated from plaintext, the datasource registry has no legacy to carry, so
/// a raw value is refused in every posture — and the refusal never echoes the
/// value.
///
/// Syntax only, no I/O: resolving a `vault:` reference is a network call, and
/// this runs on the request thread. [`resolves`] does that part, off the async
/// runtime.
///
/// `None` (the field omitted) means "no credential", which a file-backed
/// dialect needs. An explicitly empty string is a mistake, not an omission,
/// and is reported as one.
pub fn parse_credential(value: Option<&str>) -> Result<Option<SecretRef>, ValidationError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ValidationError::EmptyCredential);
    }
    if !secrets::looks_like_ref(raw) {
        return Err(ValidationError::RawCredential);
    }
    Ok(Some(SecretRef::parse(raw)?))
}

/// Check that the source's credential resolves *now*, so a broken pointer is
/// an error the admin sees at registration rather than a failure at the first
/// run. Does network I/O for a `vault:` reference — call it from a blocking
/// context.
pub fn resolves(source: &SqlSource) -> Result<(), ValidationError> {
    match &source.credential {
        Some(r) => Ok(secrets::validate(r)?),
        None => Ok(()),
    }
}

/// Everything about a datasource that does not depend on the store.
pub fn validate_source(source: &SqlSource) -> Result<(), ValidationError> {
    if !model::valid_id(&source.id) {
        return Err(ValidationError::Id(source.id.clone()));
    }
    let connector = connector::get(&source.dialect).ok_or_else(|| ValidationError::Dialect {
        dialect: source.dialect.clone(),
        available: connector::dialects().join(", "),
    })?;
    if source.database.trim().is_empty() {
        return Err(ValidationError::Database);
    }
    if !source.read_only {
        return Err(ValidationError::ReadWrite);
    }
    if source.statement_timeout_ms == 0 || source.statement_timeout_ms > MAX_TIMEOUT_MS {
        return Err(ValidationError::TimeoutRange {
            max: MAX_TIMEOUT_MS,
        });
    }
    if connector.is_networked() && source.host.as_deref().unwrap_or("").trim().is_empty() {
        return Err(ValidationError::MissingHost);
    }

    if !secrets::posture().is_production() {
        // Development: the same conditions are warnings, so a laptop does not
        // need Vault and an allowlist to try the feature.
        if connector.is_networked() && !egress_allowed(source) {
            tracing::warn!(
                source = %source.id,
                "datasource host is not on {}; this is a registration error in the production posture",
                crate::remote::ALLOWLIST_ENV
            );
        }
        return Ok(());
    }

    if connector.is_networked() {
        if !egress_allowed(source) {
            return Err(ValidationError::NotAllowlisted {
                env: crate::remote::ALLOWLIST_ENV,
            });
        }
    } else if !under_sources_dir(&source.database) {
        return Err(ValidationError::OutsideSourcesDir {
            env: SOURCES_DIR_ENV,
        });
    }
    Ok(())
}

/// Whether a file-backed database sits inside the configured sources
/// directory. Both sides are canonicalised where possible, so `..` cannot
/// walk out of it.
fn under_sources_dir(database: &str) -> bool {
    let Some(root) = sources_dir() else {
        return false;
    };
    let root = root.canonicalize().unwrap_or(root);
    let path = Path::new(database);
    let resolved = path.canonicalize().unwrap_or_else(|_| {
        // A file that does not exist yet still must not escape the root.
        path.to_path_buf()
    });
    resolved.starts_with(&root) && !resolved.components().any(|c| c.as_os_str() == "..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::model::SqlSource;

    fn sqlite_source(db: &str) -> SqlSource {
        SqlSource {
            id: "s".into(),
            name: "s".into(),
            dialect: "sqlite".into(),
            database: db.into(),
            read_only: true,
            statement_timeout_ms: DEFAULT_TIMEOUT_MS,
            ..Default::default()
        }
    }

    #[test]
    fn hex_is_upper_case_for_xsd_hexbinary() {
        assert_eq!(hex_upper(&[0x0f, 0xa0, 0x00]), "0FA000");
        assert_eq!(hex_upper(&[]), "");
    }

    #[test]
    fn a_raw_credential_is_refused_without_echoing_it() {
        let err = parse_credential(Some("hunter2-the-password")).unwrap_err();
        assert_eq!(err, ValidationError::RawCredential);
        assert!(!err.to_string().contains("hunter2"), "{err}");
    }

    #[test]
    fn an_omitted_credential_is_fine_but_an_empty_one_is_a_mistake() {
        assert_eq!(parse_credential(None).unwrap(), None);
        assert_eq!(
            parse_credential(Some("   ")).unwrap_err(),
            ValidationError::EmptyCredential
        );
        assert_eq!(
            parse_credential(Some("")).unwrap_err(),
            ValidationError::EmptyCredential
        );
    }

    #[test]
    fn a_reference_must_be_well_formed_and_resolvable() {
        std::env::set_var("OTS_SOURCES_MOD_TEST", "v");
        let ok = parse_credential(Some("env:OTS_SOURCES_MOD_TEST")).unwrap();
        assert!(ok.is_some());
        assert!(resolves(&SqlSource {
            credential: ok,
            ..Default::default()
        })
        .is_ok());

        // A well-formed pointer at nothing parses, then fails to resolve.
        let dangling = parse_credential(Some("env:OTS_SOURCES_MOD_TEST_MISSING")).unwrap();
        assert!(matches!(
            resolves(&SqlSource {
                credential: dangling,
                ..Default::default()
            }),
            Err(ValidationError::Secret(SecretError::Unresolvable { .. }))
        ));
        assert!(matches!(
            parse_credential(Some("vault:nope")),
            Err(ValidationError::Secret(SecretError::Malformed { .. }))
        ));
        // A source with no credential resolves trivially.
        assert!(resolves(&SqlSource::default()).is_ok());
    }

    #[test]
    fn structural_rules_apply_in_every_posture() {
        let mut s = sqlite_source("/tmp/x.db");
        assert!(validate_source(&s).is_ok());

        s.read_only = false;
        assert_eq!(validate_source(&s).unwrap_err(), ValidationError::ReadWrite);
        s.read_only = true;

        s.statement_timeout_ms = 0;
        assert!(matches!(
            validate_source(&s).unwrap_err(),
            ValidationError::TimeoutRange { .. }
        ));
        s.statement_timeout_ms = DEFAULT_TIMEOUT_MS;

        s.database = "  ".into();
        assert_eq!(validate_source(&s).unwrap_err(), ValidationError::Database);
        s.database = "/tmp/x.db".into();

        s.id = "bad id".into();
        assert!(matches!(
            validate_source(&s).unwrap_err(),
            ValidationError::Id(_)
        ));
        s.id = "s".into();

        s.dialect = "oracle".into();
        let err = validate_source(&s).unwrap_err();
        assert!(err.to_string().contains("sqlite"), "{err}");
    }

    #[test]
    fn a_networked_dialect_needs_a_host() {
        // Every compiled dialect that reaches a network needs one; with only
        // SQLite in core this asserts the file-backed branch is exempt.
        let s = sqlite_source("/tmp/x.db");
        assert!(validate_source(&s).is_ok(), "a file source needs no host");
        assert!(
            egress_allowed(&s),
            "a file source has no egress to allowlist"
        );
    }

    #[test]
    fn sources_dir_confinement_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var(SOURCES_DIR_ENV, dir.path());
        let inside = dir.path().join("a.db");
        std::fs::write(&inside, b"").unwrap();
        assert!(under_sources_dir(&inside.to_string_lossy()));
        assert!(!under_sources_dir("/etc/passwd"));
        assert!(
            !under_sources_dir(&format!("{}/../escape.db", dir.path().display())),
            "a .. segment must not walk out of the root"
        );
        std::env::remove_var(SOURCES_DIR_ENV);
        assert!(
            !under_sources_dir(&inside.to_string_lossy()),
            "unset means refused"
        );
    }
}
