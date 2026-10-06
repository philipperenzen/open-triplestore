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

use ots_plugin_api::sources::{
    BatchSink, ConnectParams, Row, SourceConnection, SourceConnector, SourceError, TableInfo,
    TableProfile,
};

use super::sqlite::SqliteConnector;

fn registry() -> &'static RwLock<BTreeMap<String, Arc<dyn SourceConnector>>> {
    static REG: OnceLock<RwLock<BTreeMap<String, Arc<dyn SourceConnector>>>> = OnceLock::new();
    REG.get_or_init(|| {
        let mut m: BTreeMap<String, Arc<dyn SourceConnector>> = BTreeMap::new();
        let sqlite: Arc<dyn SourceConnector> = Arc::new(SqliteConnector);
        m.insert(sqlite.dialect().to_string(), sqlite);
        // A SPARQL endpoint is HTTP, which core already speaks: no plugin.
        let sparql: Arc<dyn SourceConnector> = Arc::new(super::virtual_source::SparqlConnector);
        m.insert(sparql.dialect().to_string(), sparql);
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
///
/// The connection comes back guarded (`Guarded`): a panic inside the driver,
/// here or in any later call, becomes [`SourceError::Driver`] instead of
/// unwinding through a blocking task into an opaque 500.
pub fn connect(params: &ConnectParams) -> Result<Box<dyn SourceConnection>, SourceError> {
    let connector = get(&params.dialect).ok_or_else(|| {
        SourceError::Config(format!(
            "no connector for dialect '{}'; this build supports: {}",
            params.dialect,
            dialects().join(", ")
        ))
    })?;
    let dialect = connector.dialect();
    let inner = contain(dialect, || connector.connect(params))?;
    Ok(Box::new(Guarded {
        dialect,
        inner,
        retired: false,
    }))
}

/// Run one driver call, turning a panic inside it into a named error. The
/// panic's text goes to the log only: it is the driver's, and nothing
/// vouches that it carries no host, account or query detail.
fn contain<T>(
    dialect: &str,
    call: impl FnOnce() -> Result<T, SourceError>,
) -> Result<T, SourceError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).unwrap_or_else(|payload| {
        let detail = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        tracing::error!(dialect, panic = %detail, "the source driver panicked");
        Err(driver_failure(dialect))
    })
}

fn driver_failure(dialect: &str) -> SourceError {
    SourceError::Driver(format!(
        "the {dialect} driver failed unexpectedly; the server log has the detail"
    ))
}

/// A connection whose driver calls are [contained](contain). After a panic
/// the driver's state is unknown, so the connection is retired: every later
/// call fails with the same named error instead of running on it.
struct Guarded {
    dialect: &'static str,
    inner: Box<dyn SourceConnection>,
    retired: bool,
}

impl Guarded {
    fn call<T>(
        &mut self,
        call: impl FnOnce(&mut dyn SourceConnection) -> Result<T, SourceError>,
    ) -> Result<T, SourceError> {
        if self.retired {
            return Err(driver_failure(self.dialect));
        }
        let inner = self.inner.as_mut();
        let result = contain(self.dialect, || call(inner));
        if matches!(result, Err(SourceError::Driver(_))) {
            self.retired = true;
        }
        result
    }
}

impl SourceConnection for Guarded {
    fn introspect(&mut self) -> Result<Vec<TableInfo>, SourceError> {
        self.call(|c| c.introspect())
    }

    fn table_names(&mut self) -> Result<Vec<String>, SourceError> {
        self.call(|c| c.table_names())
    }

    fn sample(&mut self, table: &str, limit: usize) -> Result<Vec<Row>, SourceError> {
        self.call(|c| c.sample(table, limit))
    }

    fn stream(
        &mut self,
        query: &str,
        batch_size: usize,
        sink: BatchSink<'_>,
    ) -> Result<u64, SourceError> {
        // The sink is the host's code. A panic there is the host's bug, not
        // the driver's: it is caught at the sink, carried past the driver as
        // an ordinary error, and resumed once the driver has returned.
        let mut host_panic = None;
        let result = self.call(|c| {
            c.stream(query, batch_size, &mut |batch| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(batch)))
                    .unwrap_or_else(|payload| {
                        host_panic = Some(payload);
                        Err(SourceError::Query("the host stopped the stream".into()))
                    })
            })
        });
        if let Some(payload) = host_panic {
            std::panic::resume_unwind(payload);
        }
        result
    }

    fn max_watermark(&mut self, table: &str, column: &str) -> Result<Option<String>, SourceError> {
        self.call(|c| c.max_watermark(table, column))
    }

    fn unique_keys(&mut self, table: &str) -> Result<Vec<Vec<String>>, SourceError> {
        self.call(|c| c.unique_keys(table))
    }

    fn server_version(&mut self) -> Result<Option<String>, SourceError> {
        self.call(|c| c.server_version())
    }

    fn profile(&mut self, table: &str) -> Result<TableProfile, SourceError> {
        self.call(|c| c.profile(table))
    }
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
        assert!(
            !register(Arc::new(Fake)),
            "core's sqlite driver keeps the dialect"
        );
        // …and the original is still the one that answers.
        assert!(get("sqlite").is_some());
    }

    /// A driver that panics: in `connect` when the database is "connect", in
    /// `introspect` otherwise, and in `stream` only after it has handed the
    /// sink one batch.
    struct Panicky;

    struct PanickyConnection;

    impl SourceConnector for Panicky {
        fn dialect(&self) -> &'static str {
            "test-panicky"
        }
        fn connect(
            &self,
            params: &ConnectParams,
        ) -> Result<Box<dyn SourceConnection>, SourceError> {
            if params.database == "connect" {
                panic!("secret-looking driver detail");
            }
            Ok(Box::new(PanickyConnection))
        }
    }

    impl SourceConnection for PanickyConnection {
        fn introspect(&mut self) -> Result<Vec<ots_plugin_api::sources::TableInfo>, SourceError> {
            panic!("secret-looking driver detail");
        }
        fn sample(
            &mut self,
            _: &str,
            _: usize,
        ) -> Result<Vec<ots_plugin_api::sources::Row>, SourceError> {
            Ok(Vec::new())
        }
        fn stream(
            &mut self,
            _: &str,
            _: usize,
            sink: ots_plugin_api::sources::BatchSink<'_>,
        ) -> Result<u64, SourceError> {
            sink(vec![Default::default()])?;
            panic!("secret-looking driver detail");
        }
        fn max_watermark(&mut self, _: &str, _: &str) -> Result<Option<String>, SourceError> {
            Ok(None)
        }
    }

    fn panicky(database: &str) -> ConnectParams {
        register(Arc::new(Panicky));
        ConnectParams {
            dialect: "test-panicky".into(),
            host: Some("db.example".into()),
            port: None,
            database: database.into(),
            username: None,
            password: None,
            read_only: true,
            statement_timeout_ms: 1000,
            tls: false,
            options: Default::default(),
        }
    }

    fn is_named_driver_failure(err: &SourceError) -> bool {
        let SourceError::Driver(msg) = err else {
            return false;
        };
        msg.contains("test-panicky") && !msg.contains("secret-looking")
    }

    #[test]
    fn a_driver_panic_in_connect_is_a_named_error_not_a_crash() {
        let Err(err) = connect(&panicky("connect")) else {
            panic!("a panicking driver cannot produce a connection");
        };
        assert!(is_named_driver_failure(&err), "{err:?}");
    }

    #[test]
    fn a_driver_panic_on_a_connection_is_a_named_error_and_retires_it() {
        let mut conn = connect(&panicky("db")).expect("connects");
        let err = conn.introspect().expect_err("the driver panicked");
        assert!(is_named_driver_failure(&err), "{err:?}");
        // The driver's state is unknown after a panic: nothing runs on it again.
        let err = conn.sample("t", 1).expect_err("the connection is retired");
        assert!(is_named_driver_failure(&err), "{err:?}");
    }

    #[test]
    fn a_driver_panic_mid_stream_is_a_named_error() {
        let mut conn = connect(&panicky("db")).expect("connects");
        let mut batches = 0;
        let err = conn
            .stream("SELECT 1", 10, &mut |_| {
                batches += 1;
                Ok(())
            })
            .expect_err("the driver panicked");
        assert!(is_named_driver_failure(&err), "{err:?}");
        assert_eq!(batches, 1);
    }

    #[test]
    fn a_panic_in_the_hosts_own_sink_is_not_blamed_on_the_driver() {
        let mut conn = connect(&panicky("db")).expect("connects");
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            conn.stream("SELECT 1", 10, &mut |_| panic!("host bug"))
        }));
        let payload = caught.expect_err("the host's panic propagates unchanged");
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"host bug"));
    }
}
