//! Source connectors: the driver interface behind a registered datasource.
//!
//! The host's `src/sources` registry owns *what* a datasource is (dialect,
//! host, database, a secret **reference** for the credential, read-only flag,
//! statement timeout). A connector owns *how* to talk to one dialect. SQLite
//! ships in core; PostgreSQL, MySQL and SQL Server are plugins that implement
//! [`SourceConnector`] and hand it to the host through
//! [`crate::Plugin::connectors`], so the driver decision is per deployment.
//!
//! The contract a connector must keep, because the host relies on it:
//!
//! * `connect` receives the credential already resolved (a [`SecretString`])
//!   and must neither persist nor log it, nor put it in any error string.
//! * Read-only is enforced at connect time, server-side where the dialect
//!   allows (`SET default_transaction_read_only = on`, `SET SESSION
//!   TRANSACTION READ ONLY`, `SQLITE_OPEN_READ_ONLY`), so a mapping query can
//!   never write.
//! * The statement timeout is applied to every statement.
//! * Rows are streamed in batches; a connector never materialises a whole
//!   table.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use serde::{Deserialize, Serialize};

/// A resolved secret. `Debug` never prints the value, and there is no
/// `Display` at all, so it cannot reach a log line or an error message by
/// accident — a caller has to spell `expose()` to read it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString([redacted])")
    }
}

/// Everything a connector needs to open one connection.
#[derive(Debug, Clone)]
pub struct ConnectParams {
    pub dialect: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    /// Database name — or, for file-backed dialects such as SQLite, the path.
    pub database: String,
    pub username: Option<String>,
    pub password: Option<SecretString>,
    /// Always `true` from the host; a connector enforces it server-side.
    pub read_only: bool,
    /// Per-statement timeout, always `> 0`.
    pub statement_timeout_ms: u64,
    pub tls: bool,
    /// Dialect-specific extras (`sslmode`, `application_name`, …).
    pub options: BTreeMap<String, String>,
}

/// The generic type a column value is reported as, used for the R2RML natural
/// datatype mapping when a term map has no explicit `rr:datatype`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueKind {
    Text,
    Integer,
    Decimal,
    Float,
    Boolean,
    Date,
    Time,
    DateTime,
    Binary,
    Uuid,
    Json,
    Other,
}

/// One cell: its lexical form plus the generic type it came with. A SQL NULL
/// is not a `Value` — it is an absent key in the [`Row`], so a term map over
/// a NULL column produces no triple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    pub lexical: String,
    pub kind: ValueKind,
}

impl Value {
    pub fn new(lexical: impl Into<String>, kind: ValueKind) -> Self {
        Self {
            lexical: lexical.into(),
            kind,
        }
    }
    pub fn text(lexical: impl Into<String>) -> Self {
        Self::new(lexical, ValueKind::Text)
    }
}

/// One source row: column name → value. NULL columns are absent.
pub type Row = HashMap<String, Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TableKind {
    Table,
    View,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    /// The dialect's own type name (`character varying(120)`, `INTEGER`).
    pub native_type: String,
    pub generic_type: ValueKind,
    pub nullable: bool,
    pub default: Option<String>,
    pub comment: Option<String>,
    pub primary_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKey {
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub schema: Option<String>,
    pub name: String,
    pub kind: TableKind,
    pub columns: Vec<ColumnInfo>,
    pub primary_key: Vec<String>,
    pub foreign_keys: Vec<ForeignKey>,
    pub indexes: Vec<IndexInfo>,
    pub row_estimate: Option<u64>,
    pub comment: Option<String>,
}

/// What can go wrong on the source side. Messages must never carry a
/// credential; the host additionally scrubs host, database and user names
/// before a message reaches a caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// The parameters cannot describe a connection for this dialect.
    Config(String),
    /// The connection could not be opened.
    Connect(String),
    /// A statement failed.
    Query(String),
    /// The dialect or this connector does not support the operation.
    Unsupported(String),
    /// The statement timeout elapsed.
    Timeout,
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceError::Config(m) => write!(f, "configuration error: {m}"),
            SourceError::Connect(m) => write!(f, "connection failed: {m}"),
            SourceError::Query(m) => write!(f, "query failed: {m}"),
            SourceError::Unsupported(m) => write!(f, "unsupported: {m}"),
            SourceError::Timeout => f.write_str("statement timeout elapsed"),
        }
    }
}

impl std::error::Error for SourceError {}

/// The sink `stream` delivers batches to. Returning `Err` stops the stream.
pub type BatchSink<'a> = &'a mut dyn FnMut(Vec<Row>) -> Result<(), SourceError>;

/// One open, read-only connection.
pub trait SourceConnection: Send {
    /// Tables and views with columns (generic + native types, nullability,
    /// defaults, comments), primary and foreign keys, indexes and a row
    /// estimate.
    fn introspect(&mut self) -> Result<Vec<TableInfo>, SourceError>;

    /// The first `limit` rows of `table` (an identifier, quoted by the
    /// connector — never interpolated raw).
    fn sample(&mut self, table: &str, limit: usize) -> Result<Vec<Row>, SourceError>;

    /// Run `query` and deliver its rows in batches of at most `batch_size`
    /// to `sink`. Returns the number of rows delivered.
    fn stream(
        &mut self,
        query: &str,
        batch_size: usize,
        sink: BatchSink<'_>,
    ) -> Result<u64, SourceError>;

    /// `MAX(column)` of `table`, as a lexical value, for incremental runs.
    fn max_watermark(&mut self, table: &str, column: &str) -> Result<Option<String>, SourceError>;

    /// Column sets that are unique in `table`, primary key first.
    ///
    /// A join planner needs this and nothing else: a join whose parent columns
    /// cover a unique key matches at most one parent row, so it can be pushed
    /// into the child's query without duplicating child rows. The default
    /// derives it from [`Self::introspect`]; a connector whose introspection is
    /// expensive (row estimates, comments) should override it with a cheaper
    /// catalogue query.
    fn unique_keys(&mut self, table: &str) -> Result<Vec<Vec<String>>, SourceError> {
        let tables = self.introspect()?;
        let Some(t) = tables.iter().find(|t| t.name == table) else {
            return Ok(Vec::new());
        };
        let mut keys = Vec::new();
        if !t.primary_key.is_empty() {
            keys.push(t.primary_key.clone());
        }
        for index in &t.indexes {
            if index.unique && !index.columns.is_empty() && !keys.contains(&index.columns) {
                keys.push(index.columns.clone());
            }
        }
        Ok(keys)
    }

    /// The server's version string, when the dialect reports one.
    fn server_version(&mut self) -> Result<Option<String>, SourceError> {
        Ok(None)
    }
}

/// A driver for one SQL dialect.
pub trait SourceConnector: Send + Sync {
    /// The dialect token a datasource names (`sqlite`, `postgresql`, `mysql`,
    /// `mssql`). Lower-case, stable.
    fn dialect(&self) -> &'static str;

    /// Whether the dialect reaches a network host (and therefore falls under
    /// the egress allowlist) or a local file.
    fn is_networked(&self) -> bool {
        true
    }

    /// Open a read-only connection with the statement timeout applied.
    fn connect(&self, params: &ConnectParams) -> Result<Box<dyn SourceConnection>, SourceError>;

    /// Quote an identifier for this dialect. The default is the SQL standard
    /// double-quote form.
    fn quote_identifier(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_string_never_debugs_its_value() {
        let s = SecretString::new("hunter2");
        assert_eq!(format!("{s:?}"), "SecretString([redacted])");
        assert_eq!(s.expose(), "hunter2");
        let p = ConnectParams {
            dialect: "sqlite".into(),
            host: None,
            port: None,
            database: "x.db".into(),
            username: None,
            password: Some(s),
            read_only: true,
            statement_timeout_ms: 1000,
            tls: false,
            options: BTreeMap::new(),
        };
        assert!(!format!("{p:?}").contains("hunter2"));
    }

    #[test]
    fn default_identifier_quoting_doubles_quotes() {
        struct Dummy;
        impl SourceConnector for Dummy {
            fn dialect(&self) -> &'static str {
                "dummy"
            }
            fn connect(&self, _: &ConnectParams) -> Result<Box<dyn SourceConnection>, SourceError> {
                Err(SourceError::Unsupported("dummy".into()))
            }
        }
        assert_eq!(Dummy.quote_identifier("a\"b"), "\"a\"\"b\"");
    }
}
