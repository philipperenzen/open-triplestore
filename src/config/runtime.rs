//! Settings an administrator can change while the server runs.
//!
//! A few settings are policy rather than plumbing — who may register, what
//! guests and delegated tokens may do, who may frame the embed pages, the AI
//! budgets — and an operator should not need a restart to change them. Those
//! are declared `runtime` in the registry. Their effective value is the
//! administrator's override, stored in the identity database's `app_settings`
//! table under `setting:<NAME>`, when there is one; otherwise it is the
//! configured value (environment, profile or default), exactly as for any
//! other setting.
//!
//! The environment is therefore the default, not a first-boot seed: an
//! override is written only when an administrator sets one, and resetting it
//! removes the row, so a changed environment variable takes effect again.
//! (Seeding the table on first boot would make every later change to the
//! variable silently do nothing — the trap `OTS_LDP_ROOT_ACL` has, since its ACL
//! is written once.)
//!
//! The value is read from the database on each use, not cached: the identity
//! database is replicated, and a follower must see a change the leader's
//! administrator made without a restart. Every read is one indexed SQLite
//! lookup, on paths that are not hot (a guest's request, an OIDC token, an
//! AI request, an `/embed` page, a registration).

use super::{parse, FromSetting, Key, Source, Spec, SPECS};
use crate::auth::db::AuthDb;

/// The `app_settings` key prefix of runtime overrides.
pub const PREFIX: &str = "setting:";

fn row_key(name: &str) -> String {
    format!("{PREFIX}{name}")
}

/// The runtime-editable settings, in registry order.
pub fn specs() -> impl Iterator<Item = &'static Spec> {
    SPECS.iter().filter(|s| s.runtime)
}

/// The administrator's stored override of `spec`, normalised, if any. A row
/// that no longer validates (written by another version) is ignored, with an
/// error in the log: the configured value applies.
pub fn stored(db: &AuthDb, spec: &Spec) -> Option<String> {
    if !spec.runtime {
        return None;
    }
    match db.get_app_setting(&row_key(spec.name)) {
        Ok(Some(raw)) => match parse::validate(spec, &raw) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::error!(
                    "runtime setting {} holds an invalid value ({e}); using the configured one",
                    spec.name
                );
                None
            }
        },
        Ok(None) => None,
        Err(e) => {
            tracing::error!(
                "runtime setting {} could not be read ({e}); using the configured one",
                spec.name
            );
            None
        }
    }
}

/// The effective value of a runtime setting: the stored override, else the
/// configured value.
pub fn get<T: FromSetting>(db: &AuthDb, key: &Key<T>) -> T {
    match stored(db, &key.spec) {
        Some(v) => T::from_setting(&key.spec, Some(&v))
            .unwrap_or_else(|e| panic!("setting {}: {e}", key.spec.name)),
        None => key.get(),
    }
}

/// One runtime setting as the admin settings page shows it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RuntimeSetting {
    pub name: &'static str,
    pub group: &'static str,
    pub kind: &'static str,
    pub doc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<&'static [&'static str]>,
    /// The effective value.
    pub value: Option<String>,
    /// `admin` when overridden, else where the configured value came from.
    pub source: Source,
    /// What applies without the override (environment, profile or default).
    pub configured: Option<String>,
    pub configured_source: Source,
    pub overridden: bool,
}

/// Every runtime setting with its effective and configured values.
pub fn list(db: &AuthDb, config: &super::Config) -> Vec<RuntimeSetting> {
    specs()
        .map(|spec| {
            let configured = config.entry(spec.name);
            let stored = stored(db, spec);
            RuntimeSetting {
                name: spec.name,
                group: spec.group,
                kind: spec.kind.label(),
                doc: spec.doc,
                choices: match spec.kind {
                    super::Kind::Enum(c) => Some(c),
                    _ => None,
                },
                overridden: stored.is_some(),
                source: if stored.is_some() {
                    Source::Admin
                } else {
                    configured.source
                },
                value: stored.or_else(|| configured.value.clone()),
                configured: configured.value,
                configured_source: configured.source,
            }
        })
        .collect()
}

/// Why a runtime setting was not changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetError {
    /// No such setting, or it cannot be changed at runtime.
    NotRuntime(String),
    /// The value does not validate.
    Invalid(String),
    /// The database refused the write.
    Storage(String),
}

impl std::fmt::Display for SetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetError::NotRuntime(n) => {
                write!(f, "{n} is not a setting that can be changed at runtime")
            }
            SetError::Invalid(m) | SetError::Storage(m) => f.write_str(m),
        }
    }
}

/// Store an override of the runtime setting `name` (validated exactly as the
/// environment's value would be), or with `None` remove it so the configured
/// value applies again. Returns the normalised value stored.
pub fn set(db: &AuthDb, name: &str, value: Option<&str>) -> Result<Option<String>, SetError> {
    let spec = super::spec(name)
        .filter(|s| s.runtime)
        .ok_or_else(|| SetError::NotRuntime(name.to_string()))?;
    match value {
        None => {
            db.delete_app_setting(&row_key(spec.name))
                .map_err(|e| SetError::Storage(e.to_string()))?;
            Ok(None)
        }
        Some(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                return Err(SetError::Invalid(format!(
                    "{name}: an empty value is not a value; reset the setting instead"
                )));
            }
            let v = parse::validate(spec, raw)
                .map_err(|e| SetError::Invalid(format!("{name}: {e}")))?;
            db.set_app_setting(&row_key(spec.name), &v)
                .map_err(|e| SetError::Storage(e.to_string()))?;
            Ok(Some(v))
        }
    }
}
