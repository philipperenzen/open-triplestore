//! The connector registry: dialect token → driver.
//!
//! SQLite is compiled into core (it is already a dependency, and it makes the
//! whole pipeline testable without a database server). Every other dialect is
//! a plugin that returns its driver from
//! [`Plugin::connectors`](ots_plugin_api::Plugin::connectors), so the
//! PostgreSQL / MySQL / SQL Server decision is per deployment rather than a
//! dependency every operator carries.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

use ots_plugin_api::sources::{ConnectParams, SourceConnection, SourceConnector, SourceError};

use super::sqlite::SqliteConnector;

fn registry() -> &'static RwLock<BTreeMap<String, Arc<dyn SourceConnector>>> {
    static REG: OnceLock<RwLock<BTreeMap<String, Arc<dyn SourceConnector>>>> = OnceLock::new();
    REG.get_or_init(|| {
        let mut m: BTreeMap<String, Arc<dyn SourceConnector>> = BTreeMap::new();
        let sqlite: Arc<dyn SourceConnector> = Arc::new(SqliteConnector);
        m.insert(sqlite.dialect().to_string(), sqlite);
        RwLock::new(m)
    })
}

/// Register a plugin-supplied driver. A dialect that is already taken is
/// refused (core and first-registered win) and logged: silently replacing the
/// driver a datasource was registered against would change what its
/// credentials reach.
pub fn register(connector: Arc<dyn SourceConnector>) -> bool {
    let dialect = connector.dialect().to_ascii_lowercase();
    let mut reg = registry().write().unwrap_or_else(|e| e.into_inner());
    if reg.contains_key(&dialect) {
        tracing::warn!(
            dialect = %dialect,
            "a source connector for this dialect is already registered; ignoring the duplicate"
        );
        return false;
    }
    tracing::info!(dialect = %dialect, "source connector registered");
    reg.insert(dialect, connector);
    true
}

/// Register every connector the compiled-in plugins contribute. Called once
/// at boot, before any datasource is used.
pub fn register_plugin_connectors() {
    for plugin in crate::plugins::registered_plugins() {
        for c in plugin.connectors() {
            register(c);
        }
    }
}

pub fn get(dialect: &str) -> Option<Arc<dyn SourceConnector>> {
    registry()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(&dialect.trim().to_ascii_lowercase())
        .cloned()
}

/// Every dialect this binary can talk to, for error messages and the API.
pub fn dialects() -> Vec<String> {
    registry()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .cloned()
        .collect()
}

/// Open a connection for `params`, mapping an unknown dialect to a message
/// that names what *is* available.
pub fn connect(params: &ConnectParams) -> Result<Box<dyn SourceConnection>, SourceError> {
    let connector = get(&params.dialect).ok_or_else(|| {
        SourceError::Config(format!(
            "no connector for dialect '{}'; this build supports: {}",
            params.dialect,
            dialects().join(", ")
        ))
    })?;
    connector.connect(params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_is_always_available_and_is_not_networked() {
        let c = get("SQLite").expect("dialect lookup is case-insensitive");
        assert_eq!(c.dialect(), "sqlite");
        assert!(!c.is_networked());
        assert!(dialects().contains(&"sqlite".to_string()));
    }

    #[test]
    fn unknown_dialect_names_what_is_available() {
        let Err(err) = connect(&ConnectParams {
            dialect: "oracle".into(),
            host: None,
            port: None,
            database: "x".into(),
            username: None,
            password: None,
            read_only: true,
            statement_timeout_ms: 1000,
            tls: false,
            options: Default::default(),
        }) else {
            panic!("an unknown dialect cannot produce a connection");
        };
        let msg = err.to_string();
        assert!(msg.contains("oracle") && msg.contains("sqlite"), "{msg}");
    }

    #[test]
    fn a_duplicate_dialect_is_refused_rather_than_overriding() {
        struct Fake;
        impl SourceConnector for Fake {
            fn dialect(&self) -> &'static str {
                "sqlite"
            }
            fn connect(&self, _: &ConnectParams) -> Result<Box<dyn SourceConnection>, SourceError> {
                Err(SourceError::Unsupported("fake".into()))
            }
        }
        assert!(!register(Arc::new(Fake)), "core's sqlite driver keeps the dialect");
        // …and the original is still the one that answers.
        assert!(get("sqlite").is_some());
    }
}
