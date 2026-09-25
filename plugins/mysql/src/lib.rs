//! `ots-plugin-mysql` — the MySQL / MariaDB datasource connector.
//!
//! The contract every connector keeps, in this dialect's terms:
//!
//! * **Read-only, server-side.** Every session starts with
//!   `SET SESSION TRANSACTION READ ONLY`, so a mapping query cannot write
//!   whatever the account could.
//! * **A statement timeout on everything.** `max_execution_time` (MySQL) or
//!   `max_statement_time` (MariaDB) for the session; a server that accepts
//!   neither is refused at connect, because a budget that is not enforced
//!   is not a budget. A killed statement (`3024` / `1969`) is reported as a
//!   timeout.
//! * **Streaming.** Rows come off the text protocol one at a time and are
//!   delivered in batches; a table is never held whole.
//! * **TLS through rustls**, with the bundled roots and, when a datasource
//!   names one in `options.sslrootcert`, a private CA — never a trust-all.
//!
//! The catalogue and the profiler come from
//! [`ots_plugin_api::sources::catalogue`]; this crate spells the MySQL
//! fragments, runs the SQL and turns the protocol's values into lexical
//! forms.

use std::sync::Arc;
use std::time::Duration;

use mysql::consts::ColumnType;
use mysql::prelude::Queryable;
use mysql::{Column, Conn, Opts, OptsBuilder, SslOpts, Value as MyValue};
use ots_plugin_api::sources::catalogue::{self, canonical, Dialect, Executor};
use ots_plugin_api::sources::{
    BatchSink, ConnectParams, Row, SourceConnection, SourceConnector, SourceError, TableInfo,
    TableProfile, Value, ValueKind, PROFILE_SAMPLE_ROWS,
};
use ots_plugin_api::Plugin;

/// The plugin: no routes, one connector.
#[derive(Default)]
pub struct MysqlPlugin;

impl Plugin for MysqlPlugin {
    fn name(&self) -> &'static str {
        "mysql"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn connectors(&self) -> Vec<Arc<dyn SourceConnector>> {
        vec![Arc::new(MysqlConnector)]
    }
}

pub struct MysqlConnector;

/// The dialect token a datasource names. MariaDB speaks the same protocol
/// and is registered under it as well.
pub const DIALECT: &str = "mysql";

/// MySQL's `ER_QUERY_TIMEOUT` and MariaDB's `ER_STATEMENT_TIMEOUT`.
const TIMEOUT_CODES: [u16; 2] = [3024, 1969];
/// `ER_UNKNOWN_SYSTEM_VARIABLE`: the server does not know the setting.
const UNKNOWN_VARIABLE: u16 = 1193;
/// The `binary` character set: what makes a string column binary.
const BINARY_CHARSET: u16 = 63;

impl SourceConnector for MysqlConnector {
    fn dialect(&self) -> &'static str {
        DIALECT
    }

    fn connect(&self, params: &ConnectParams) -> Result<Box<dyn SourceConnection>, SourceError> {
        if !params.read_only {
            return Err(SourceError::Config(
                "this store only opens datasources read-only".to_string(),
            ));
        }
        let host = params
            .host
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .ok_or_else(|| SourceError::Config("a mysql datasource needs a host".to_string()))?;
        if params.database.trim().is_empty() {
            return Err(SourceError::Config(
                "a mysql datasource needs a database name".to_string(),
            ));
        }
        let mut builder = OptsBuilder::new()
            .ip_or_hostname(Some(host))
            .tcp_port(params.port.unwrap_or(3306))
            .db_name(Some(params.database.trim()))
            .tcp_connect_timeout(Some(Duration::from_secs(15)))
            .user(
                params
                    .username
                    .as_deref()
                    .map(str::trim)
                    .filter(|u| !u.is_empty()),
            )
            .pass(params.password.as_ref().map(|p| p.expose().to_string()));
        if params.tls {
            let mut ssl = SslOpts::default();
            if let Some(path) = params
                .options
                .get("sslrootcert")
                .map(|p| p.trim())
                .filter(|p| !p.is_empty())
            {
                if !std::path::Path::new(path).is_file() {
                    return Err(SourceError::Config(format!(
                        "sslrootcert could not be read: {path}"
                    )));
                }
                ssl = ssl.with_root_cert_path(Some(std::path::PathBuf::from(path)));
            }
            builder = builder.ssl_opts(ssl);
        }
        let opts: Opts = builder.into();
        let conn = Conn::new(opts).map_err(|e| SourceError::Connect(describe(&e)))?;
        let mut conn = MysqlConnection {
            conn,
            timeout_ms: params.statement_timeout_ms.max(1),
        };
        conn.session_setup()?;
        Ok(Box::new(conn))
    }

    fn quote_identifier(&self, ident: &str) -> String {
        MysqlDialect.quote(ident)
    }
}

/// A driver error as a message: the server's own text for a server error
/// (it never carries the password), the driver's summary otherwise. The
/// host scrubs host, database and account on top.
fn describe(e: &mysql::Error) -> String {
    match e {
        mysql::Error::MySqlError(server) => server.message.clone(),
        other => other.to_string(),
    }
}

fn server_code(e: &mysql::Error) -> Option<u16> {
    match e {
        mysql::Error::MySqlError(server) => Some(server.code),
        _ => None,
    }
}

struct MysqlConnection {
    conn: Conn,
    timeout_ms: u64,
}

impl MysqlConnection {
    fn session_setup(&mut self) -> Result<(), SourceError> {
        self.conn
            .query_drop("SET SESSION TRANSACTION READ ONLY")
            .map_err(|e| SourceError::Connect(describe(&e)))?;
        // MySQL counts milliseconds, MariaDB seconds; a server that knows
        // neither variable cannot enforce the budget and is refused.
        let mysql_form = format!("SET SESSION max_execution_time = {}", self.timeout_ms);
        match self.conn.query_drop(&mysql_form) {
            Ok(()) => return Ok(()),
            Err(e) if server_code(&e) == Some(UNKNOWN_VARIABLE) => {}
            Err(e) => return Err(SourceError::Connect(describe(&e))),
        }
        let mariadb_form = format!(
            "SET SESSION max_statement_time = {}",
            self.timeout_ms as f64 / 1000.0
        );
        self.conn.query_drop(&mariadb_form).map_err(|e| {
            SourceError::Config(format!(
                "the server accepts neither max_execution_time nor max_statement_time, so the \
                 statement timeout cannot be enforced: {}",
                describe(&e)
            ))
        })
    }

    fn error(e: mysql::Error) -> SourceError {
        if server_code(&e).is_some_and(|c| TIMEOUT_CODES.contains(&c)) {
            SourceError::Timeout
        } else {
            SourceError::Query(describe(&e))
        }
    }
}

/// What a result column is, from the protocol's column definition.
fn kind_of(column: &Column) -> ValueKind {
    use ColumnType::*;
    let binary = column.character_set() == BINARY_CHARSET;
    match column.column_type() {
        MYSQL_TYPE_TINY if column.column_length() == 1 => ValueKind::Boolean,
        MYSQL_TYPE_BIT if column.column_length() == 1 => ValueKind::Boolean,
        MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_LONG | MYSQL_TYPE_LONGLONG
        | MYSQL_TYPE_INT24 | MYSQL_TYPE_YEAR | MYSQL_TYPE_BIT => ValueKind::Integer,
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => ValueKind::Decimal,
        MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => ValueKind::Float,
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => ValueKind::Date,
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => ValueKind::Time,
        MYSQL_TYPE_DATETIME
        | MYSQL_TYPE_DATETIME2
        | MYSQL_TYPE_TIMESTAMP
        | MYSQL_TYPE_TIMESTAMP2 => ValueKind::DateTime,
        MYSQL_TYPE_JSON => ValueKind::Json,
        MYSQL_TYPE_TINY_BLOB
        | MYSQL_TYPE_MEDIUM_BLOB
        | MYSQL_TYPE_LONG_BLOB
        | MYSQL_TYPE_BLOB
        | MYSQL_TYPE_VARCHAR
        | MYSQL_TYPE_VAR_STRING
        | MYSQL_TYPE_STRING => {
            if binary {
                ValueKind::Binary
            } else {
                ValueKind::Text
            }
        }
        MYSQL_TYPE_ENUM | MYSQL_TYPE_SET => ValueKind::Text,
        MYSQL_TYPE_GEOMETRY => ValueKind::Binary,
        _ => ValueKind::Other,
    }
}

fn hex_upper(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02X}"));
    }
    out
}

/// A big-endian `BIT(n)` value as an integer.
fn bits_as_integer(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(0u128, |acc, b| (acc << 8) | u128::from(*b))
        .to_string()
}

fn two(n: impl std::fmt::Display) -> String {
    format!("{n:02}")
}

/// A protocol value as the lexical form the natural mapping wants for
/// `kind`: binary as upper-case hex, dates and times in XSD shape, the rest
/// as the server spelled it.
fn lexical(kind: ValueKind, value: &MyValue) -> String {
    match value {
        MyValue::NULL => String::new(),
        MyValue::Bytes(b) => match kind {
            ValueKind::Binary => hex_upper(b),
            ValueKind::Boolean | ValueKind::Integer
                if b.len() <= 16 && b.iter().any(|x| *x > b'9' || *x < b'0') =>
            {
                // A BIT column arrives as raw bits, not digits.
                canonical(kind, &bits_as_integer(b))
            }
            other => canonical(other, &String::from_utf8_lossy(b)),
        },
        MyValue::Int(i) => canonical(kind, &i.to_string()),
        MyValue::UInt(u) => canonical(kind, &u.to_string()),
        MyValue::Float(f) => canonical(kind, &f.to_string()),
        MyValue::Double(d) => canonical(kind, &d.to_string()),
        MyValue::Date(y, mo, d, h, mi, s, us) => {
            let date = format!("{y:04}-{}-{}", two(mo), two(d));
            if kind == ValueKind::Date {
                date
            } else {
                let mut out = format!("{date}T{}:{}:{}", two(h), two(mi), two(s));
                if *us > 0 {
                    out.push_str(&format!(".{us:06}"));
                }
                canonical(ValueKind::DateTime, &out)
            }
        }
        MyValue::Time(negative, days, h, mi, s, us) => {
            let hours = u64::from(*days) * 24 + u64::from(*h);
            let mut out = format!(
                "{}{}:{}:{}",
                if *negative { "-" } else { "" },
                two(hours),
                two(mi),
                two(s)
            );
            if *us > 0 {
                out.push_str(&format!(".{us:06}"));
            }
            canonical(ValueKind::Time, &out)
        }
    }
}

/// A catalogue cell, where no column definition is needed: text as text,
/// numbers as numbers.
fn plain(value: &MyValue) -> Option<String> {
    match value {
        MyValue::NULL => None,
        MyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        other => Some(lexical(ValueKind::Other, other)),
    }
}

impl Executor for MysqlConnection {
    fn rows(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>, SourceError> {
        let mut result = self.conn.query_iter(sql).map_err(Self::error)?;
        let mut out = Vec::new();
        while let Some(set) = result.iter() {
            for row in set {
                let row = row.map_err(Self::error)?;
                out.push(
                    (0..row.len())
                        .map(|i| row.as_ref(i).and_then(plain))
                        .collect(),
                );
            }
        }
        Ok(out)
    }
}

impl SourceConnection for MysqlConnection {
    fn introspect(&mut self) -> Result<Vec<TableInfo>, SourceError> {
        catalogue::introspect(self, &MysqlDialect)
    }

    fn table_names(&mut self) -> Result<Vec<String>, SourceError> {
        Ok(catalogue::listing(self, &MysqlDialect)?
            .into_iter()
            .map(|(name, _)| name)
            .collect())
    }

    fn sample(&mut self, table: &str, limit: usize) -> Result<Vec<Row>, SourceError> {
        let sql = catalogue::sample_sql(&MysqlDialect, table, limit);
        let mut rows = Vec::new();
        self.stream(&sql, limit.max(1), &mut |batch| {
            rows.extend(batch);
            Ok(())
        })?;
        Ok(rows)
    }

    fn stream(
        &mut self,
        query: &str,
        batch_size: usize,
        sink: BatchSink<'_>,
    ) -> Result<u64, SourceError> {
        let batch_size = batch_size.max(1);
        let mut result = self.conn.query_iter(query).map_err(Self::error)?;
        let columns: Vec<(String, ValueKind)> = result
            .columns()
            .as_ref()
            .iter()
            .map(|c| (c.name_str().into_owned(), kind_of(c)))
            .collect();
        let mut delivered = 0u64;
        let mut batch: Vec<Row> = Vec::with_capacity(batch_size);
        while let Some(set) = result.iter() {
            for row in set {
                let row = row.map_err(|e| match server_code(&e) {
                    Some(c) if TIMEOUT_CODES.contains(&c) => SourceError::Timeout,
                    _ => SourceError::Query(describe(&e)),
                })?;
                let mut out: Row = Row::with_capacity(columns.len());
                for (i, (name, kind)) in columns.iter().enumerate() {
                    // A SQL NULL is an absent key, so a term map over it
                    // produces no triple.
                    match row.as_ref(i) {
                        None | Some(MyValue::NULL) => continue,
                        Some(v) => {
                            out.insert(name.clone(), Value::new(lexical(*kind, v), *kind));
                        }
                    }
                }
                batch.push(out);
                delivered += 1;
                if batch.len() >= batch_size {
                    sink(std::mem::replace(
                        &mut batch,
                        Vec::with_capacity(batch_size),
                    ))?;
                }
            }
        }
        drop(result);
        if !batch.is_empty() {
            sink(batch)?;
        }
        Ok(delivered)
    }

    fn max_watermark(&mut self, table: &str, column: &str) -> Result<Option<String>, SourceError> {
        catalogue::max_watermark(self, &MysqlDialect, table, column)
    }

    fn unique_keys(&mut self, table: &str) -> Result<Vec<Vec<String>>, SourceError> {
        catalogue::unique_keys(self, &MysqlDialect, table)
    }

    fn server_version(&mut self) -> Result<Option<String>, SourceError> {
        Ok(self
            .rows("SELECT VERSION()")?
            .first()
            .and_then(|r| r.first().cloned())
            .flatten()
            .map(|v| {
                if v.contains("MariaDB") {
                    format!("MariaDB {v}")
                } else {
                    format!("MySQL {v}")
                }
            }))
    }

    fn profile(&mut self, table: &str) -> Result<TableProfile, SourceError> {
        let sample = self.sample(table, PROFILE_SAMPLE_ROWS)?;
        catalogue::profile(self, &MysqlDialect, table, &sample)
    }
}

/// MySQL's spelling of the shared catalogue and profiler SQL.
pub struct MysqlDialect;

impl Dialect for MysqlDialect {
    fn quote(&self, ident: &str) -> String {
        format!("`{}`", ident.replace('`', "``"))
    }

    fn schema_expr(&self) -> String {
        "DATABASE()".to_string()
    }

    fn select_limited(&self, body: &str, limit: usize) -> String {
        format!("SELECT {body} LIMIT {limit}")
    }

    fn nth_value(&self, table_q: &str, column_q: &str, offset: u64) -> String {
        format!(
            "(SELECT {column_q} FROM {table_q} WHERE {column_q} IS NOT NULL ORDER BY {column_q} \
             LIMIT 1 OFFSET {offset})"
        )
    }

    fn char_length(&self, expr: &str) -> String {
        format!("CHAR_LENGTH({expr})")
    }

    fn to_text(&self, expr: &str) -> String {
        format!("CAST({expr} AS CHAR)")
    }

    fn columns_sql(&self, table: &str) -> String {
        // MariaDB spells a default as an expression — `'x'` for the literal
        // x, `NULL` for DEFAULT NULL — where MySQL reports the bare value and
        // no default at all; both come out as MySQL's.
        format!(
            "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, \
               CASE WHEN VERSION() NOT LIKE '%MariaDB%' THEN COLUMN_DEFAULT \
                    WHEN COLUMN_DEFAULT = 'NULL' THEN NULL \
                    WHEN CHAR_LENGTH(COLUMN_DEFAULT) >= 2 AND COLUMN_DEFAULT LIKE '''%''' \
                      THEN REPLACE(SUBSTRING(COLUMN_DEFAULT, 2, CHAR_LENGTH(COLUMN_DEFAULT) - 2), \
                                   '''''', '''') \
                    ELSE COLUMN_DEFAULT END, \
               NULLIF(COLUMN_COMMENT, '') \
             FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = {} \
             ORDER BY ORDINAL_POSITION",
            self.literal(table)
        )
    }

    fn foreign_keys_sql(&self, table: &str) -> String {
        format!(
            "SELECT CONSTRAINT_NAME, COLUMN_NAME, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
             FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = {} AND REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY CONSTRAINT_NAME, ORDINAL_POSITION",
            self.literal(table)
        )
    }

    fn unique_indexes_sql(&self, table: &str) -> Option<String> {
        Some(format!(
            "SELECT INDEX_NAME, COLUMN_NAME, SEQ_IN_INDEX FROM INFORMATION_SCHEMA.STATISTICS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = {} AND NON_UNIQUE = 0 \
             ORDER BY INDEX_NAME, SEQ_IN_INDEX",
            self.literal(table)
        ))
    }

    fn row_estimate_sql(&self, table: &str) -> Option<String> {
        Some(format!(
            "SELECT TABLE_ROWS FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = {}",
            self.literal(table)
        ))
    }

    fn table_comment_sql(&self, table: &str) -> Option<String> {
        Some(format!(
            "SELECT NULLIF(TABLE_COMMENT, '') FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = {}",
            self.literal(table)
        ))
    }

    fn map_type(&self, native: &str) -> ValueKind {
        let lower = native.trim().to_ascii_lowercase();
        // `tinyint(1)` and `bit(1)` are how MySQL spells a boolean; a wider
        // `bit(n)` is an integer, not a boolean.
        if lower.starts_with("tinyint(1)") || lower == "bit(1)" || lower == "bit" {
            return ValueKind::Boolean;
        }
        if lower.starts_with("bit(") {
            return ValueKind::Integer;
        }
        if lower.starts_with("geometry") || lower.ends_with("polygon") || lower.ends_with("point") {
            return ValueKind::Other;
        }
        catalogue::standard_type(native)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialect_spells_mysql() {
        assert_eq!(MysqlDialect.quote("a`b"), "`a``b`");
        assert_eq!(
            MysqlDialect.select_limited("* FROM `t`", 5),
            "SELECT * FROM `t` LIMIT 5"
        );
        assert_eq!(MysqlDialect.char_length("x"), "CHAR_LENGTH(x)");
        assert_eq!(MysqlDialect.to_text("x"), "CAST(x AS CHAR)");
        assert!(MysqlDialect.columns_sql("t").contains("COLUMN_TYPE"));
        assert!(MysqlDialect
            .foreign_keys_sql("t")
            .contains("REFERENCED_TABLE_NAME IS NOT NULL"));
        assert!(MysqlDialect
            .unique_indexes_sql("t")
            .unwrap()
            .contains("NON_UNIQUE = 0"));
        assert_eq!(MysqlDialect.map_type("tinyint(1)"), ValueKind::Boolean);
        assert_eq!(
            MysqlDialect.map_type("tinyint(4) unsigned"),
            ValueKind::Integer
        );
        assert_eq!(MysqlDialect.map_type("bit(1)"), ValueKind::Boolean);
        assert_eq!(MysqlDialect.map_type("bit(8)"), ValueKind::Integer);
        assert_eq!(MysqlDialect.map_type("enum('a','b')"), ValueKind::Text);
        assert_eq!(MysqlDialect.map_type("mediumint(9)"), ValueKind::Integer);
        assert_eq!(MysqlDialect.map_type("point"), ValueKind::Other);
        assert_eq!(MysqlDialect.map_type("json"), ValueKind::Json);
        assert_eq!(MysqlDialect.map_type("varbinary(16)"), ValueKind::Binary);
    }

    #[test]
    fn protocol_values_become_lexical_forms() {
        assert_eq!(
            lexical(ValueKind::DateTime, &MyValue::Date(2026, 1, 2, 3, 4, 5, 0)),
            "2026-01-02T03:04:05"
        );
        assert_eq!(
            lexical(
                ValueKind::DateTime,
                &MyValue::Date(2026, 1, 2, 3, 4, 5, 250000)
            ),
            "2026-01-02T03:04:05.25"
        );
        assert_eq!(
            lexical(ValueKind::Date, &MyValue::Date(2026, 1, 2, 0, 0, 0, 0)),
            "2026-01-02"
        );
        assert_eq!(
            lexical(ValueKind::Time, &MyValue::Time(false, 1, 2, 3, 4, 0)),
            "26:03:04"
        );
        assert_eq!(
            lexical(ValueKind::Time, &MyValue::Time(true, 0, 0, 30, 0, 0)),
            "-00:30:00"
        );
        assert_eq!(lexical(ValueKind::Boolean, &MyValue::Int(1)), "true");
        assert_eq!(
            lexical(ValueKind::Boolean, &MyValue::Bytes(vec![0])),
            "false"
        );
        assert_eq!(
            lexical(ValueKind::Integer, &MyValue::Bytes(vec![1, 0])),
            "256"
        );
        assert_eq!(
            lexical(ValueKind::Integer, &MyValue::Bytes(b"42".to_vec())),
            "42"
        );
        assert_eq!(
            lexical(ValueKind::Binary, &MyValue::Bytes(vec![0xde, 0xad])),
            "DEAD"
        );
        assert_eq!(
            lexical(ValueKind::Decimal, &MyValue::Bytes(b"1.50".to_vec())),
            "1.50"
        );
        assert_eq!(lexical(ValueKind::Float, &MyValue::Double(2.5)), "2.5");
        assert_eq!(
            lexical(ValueKind::Text, &MyValue::Bytes(b"caf\xc3\xa9".to_vec())),
            "café"
        );
        assert_eq!(plain(&MyValue::NULL), None);
        assert_eq!(plain(&MyValue::UInt(7)).as_deref(), Some("7"));
    }

    #[test]
    fn connect_refuses_what_it_cannot_describe() {
        let base = ConnectParams {
            dialect: DIALECT.into(),
            host: Some("db.example".into()),
            port: None,
            database: "shop".into(),
            username: Some("reader".into()),
            password: None,
            read_only: true,
            statement_timeout_ms: 1000,
            tls: false,
            options: Default::default(),
        };
        for broken in [
            ConnectParams {
                host: None,
                ..base.clone()
            },
            ConnectParams {
                read_only: false,
                ..base.clone()
            },
            ConnectParams {
                database: "".into(),
                ..base.clone()
            },
            ConnectParams {
                tls: true,
                options: [("sslrootcert".to_string(), "/nonexistent/ca.pem".to_string())]
                    .into_iter()
                    .collect(),
                ..base.clone()
            },
        ] {
            assert!(matches!(
                MysqlConnector.connect(&broken),
                Err(SourceError::Config(_))
            ));
        }
    }
}
