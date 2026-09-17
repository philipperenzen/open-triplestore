//! The SQLite connector, compiled into core.
//!
//! It is the reference implementation of [`SourceConnector`]: read-only is
//! enforced by opening the file with `SQLITE_OPEN_READ_ONLY`, and the
//! statement timeout by a progress handler that aborts the statement once the
//! budget is spent. Every identifier is quoted, never interpolated raw.

use std::time::{Duration, Instant};

use ots_plugin_api::sources::{
    BatchSink, ColumnInfo, ConnectParams, ForeignKey, IndexInfo, Row, SourceConnection,
    SourceConnector, SourceError, TableInfo, TableKind, Value, ValueKind,
};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

pub struct SqliteConnector;

impl SourceConnector for SqliteConnector {
    fn dialect(&self) -> &'static str {
        "sqlite"
    }

    /// File-backed: there is no egress host to allowlist.
    fn is_networked(&self) -> bool {
        false
    }

    fn connect(&self, params: &ConnectParams) -> Result<Box<dyn SourceConnection>, SourceError> {
        if params.database.trim().is_empty() {
            return Err(SourceError::Config(
                "a sqlite datasource needs a database file path".to_string(),
            ));
        }
        if !params.read_only {
            return Err(SourceError::Config(
                "this store only opens datasources read-only".to_string(),
            ));
        }
        // READ_ONLY is the enforcement, not a convention: the handle cannot
        // write even if a mapping's query tried to.
        let conn = Connection::open_with_flags(
            &params.database,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| SourceError::Connect(scrub(&e.to_string())))?;
        Ok(Box::new(SqliteConnection {
            conn,
            timeout: Duration::from_millis(params.statement_timeout_ms.max(1)),
        }))
    }

    fn quote_identifier(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }
}

/// SQLite error text can embed the database file path. The path is
/// deployment information the caller did not supply, so it never travels back
/// in an error; the host adds its own scrubbing on top.
fn scrub(message: &str) -> String {
    match message.split_once(':') {
        // "unable to open database file" and friends keep their first clause.
        Some((head, _)) if head.len() < 60 => head.trim().to_string(),
        _ => message
            .split_whitespace()
            .take(8)
            .collect::<Vec<_>>()
            .join(" "),
    }
}

struct SqliteConnection {
    conn: Connection,
    timeout: Duration,
}

impl SqliteConnection {
    /// Arm the per-statement budget. SQLite calls the progress handler every
    /// `n` VM steps; returning `true` aborts the statement, which surfaces as
    /// `SQLITE_INTERRUPT`.
    fn arm_timeout(&self) {
        let deadline = Instant::now() + self.timeout;
        self.conn
            .progress_handler(1_000, Some(move || Instant::now() >= deadline));
    }

    fn disarm(&self) {
        self.conn.progress_handler(0, None::<fn() -> bool>);
    }

    fn query_error(&self, e: rusqlite::Error) -> SourceError {
        if matches!(
            e,
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::OperationInterrupted,
                    ..
                },
                _
            )
        ) {
            SourceError::Timeout
        } else {
            SourceError::Query(scrub(&e.to_string()))
        }
    }

    fn strings(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, SourceError> {
        self.arm_timeout();
        let result = (|| -> Result<Vec<Vec<Option<String>>>, rusqlite::Error> {
            let mut stmt = self.conn.prepare(sql)?;
            let width = stmt.column_count();
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                let mut cells = Vec::with_capacity(width);
                for i in 0..width {
                    cells.push(match row.get_ref(i)? {
                        ValueRef::Null => None,
                        other => Some(lexical(other)),
                    });
                }
                out.push(cells);
            }
            Ok(out)
        })();
        self.disarm();
        result.map_err(|e| self.query_error(e))
    }
}

fn lexical(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Null => String::new(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => {
            // A whole float keeps a fractional part so an xsd:decimal /
            // xsd:double literal stays lexically valid ("2" vs "2.0").
            if f.fract() == 0.0 && f.is_finite() {
                format!("{f:.1}")
            } else {
                f.to_string()
            }
        }
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
        ValueRef::Blob(b) => crate::sources::hex_upper(b),
    }
}

/// SQLite is dynamically typed; the *declared* column type is the best
/// generic-type signal, and the value's storage class refines it per row.
fn generic_type(declared: &str) -> ValueKind {
    let d = declared.to_ascii_uppercase();
    let d = d.split('(').next().unwrap_or("").trim().to_string();
    match d.as_str() {
        "" => ValueKind::Other,
        t if t.contains("INT") => ValueKind::Integer,
        "REAL" | "DOUBLE" | "FLOAT" | "DOUBLE PRECISION" => ValueKind::Float,
        t if t.contains("DECIMAL") || t.contains("NUMERIC") || t.contains("MONEY") => {
            ValueKind::Decimal
        }
        "BOOLEAN" | "BOOL" => ValueKind::Boolean,
        "DATE" => ValueKind::Date,
        "TIME" => ValueKind::Time,
        t if t.contains("DATETIME") || t.contains("TIMESTAMP") => ValueKind::DateTime,
        "BLOB" | "BINARY" | "VARBINARY" => ValueKind::Binary,
        "UUID" | "GUID" => ValueKind::Uuid,
        "JSON" | "JSONB" => ValueKind::Json,
        t if t.contains("CHAR") || t.contains("TEXT") || t.contains("CLOB") => ValueKind::Text,
        _ => ValueKind::Other,
    }
}

fn value_kind_of(v: ValueRef<'_>, declared: ValueKind) -> ValueKind {
    match v {
        ValueRef::Integer(_) if declared == ValueKind::Other => ValueKind::Integer,
        ValueRef::Real(_) if declared == ValueKind::Other => ValueKind::Float,
        ValueRef::Text(_) if declared == ValueKind::Other => ValueKind::Text,
        ValueRef::Blob(_) if declared == ValueKind::Other => ValueKind::Binary,
        _ => declared,
    }
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

impl SourceConnection for SqliteConnection {
    fn introspect(&mut self) -> Result<Vec<TableInfo>, SourceError> {
        let listing = self.strings(
            "SELECT name, type FROM sqlite_master WHERE type IN ('table','view') \
             AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )?;
        let mut tables = Vec::with_capacity(listing.len());
        for row in listing {
            let name = row.first().cloned().flatten().unwrap_or_default();
            let kind = match row.get(1).cloned().flatten().as_deref() {
                Some("view") => TableKind::View,
                _ => TableKind::Table,
            };
            let q = quote(&name);

            let mut columns = Vec::new();
            let mut primary_key = Vec::new();
            for c in self.strings(&format!("PRAGMA table_info({q})"))? {
                let col_name = c.get(1).cloned().flatten().unwrap_or_default();
                let native = c.get(2).cloned().flatten().unwrap_or_default();
                let not_null = c.get(3).cloned().flatten().as_deref() == Some("1");
                let default = c.get(4).cloned().flatten();
                let pk_pos = c
                    .get(5)
                    .cloned()
                    .flatten()
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0);
                if pk_pos > 0 {
                    primary_key.push((pk_pos, col_name.clone()));
                }
                columns.push(ColumnInfo {
                    generic_type: generic_type(&native),
                    name: col_name,
                    native_type: native,
                    nullable: !not_null,
                    default,
                    // SQLite keeps no column comments.
                    comment: None,
                    primary_key: pk_pos > 0,
                });
            }
            primary_key.sort_by_key(|(pos, _)| *pos);

            let mut by_id: std::collections::BTreeMap<i64, ForeignKey> = Default::default();
            for f in self.strings(&format!("PRAGMA foreign_key_list({q})"))? {
                let id = f
                    .first()
                    .cloned()
                    .flatten()
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0);
                let ref_table = f.get(2).cloned().flatten().unwrap_or_default();
                let from = f.get(3).cloned().flatten().unwrap_or_default();
                let to = f.get(4).cloned().flatten().unwrap_or_default();
                let entry = by_id.entry(id).or_insert_with(|| ForeignKey {
                    columns: Vec::new(),
                    ref_table: ref_table.clone(),
                    ref_columns: Vec::new(),
                });
                entry.columns.push(from);
                entry.ref_columns.push(to);
            }

            let mut indexes = Vec::new();
            for i in self.strings(&format!("PRAGMA index_list({q})"))? {
                let idx_name = i.get(1).cloned().flatten().unwrap_or_default();
                let unique = i.get(2).cloned().flatten().as_deref() == Some("1");
                let cols = self
                    .strings(&format!("PRAGMA index_info({})", quote(&idx_name)))?
                    .into_iter()
                    .filter_map(|r| r.get(2).cloned().flatten())
                    .collect();
                indexes.push(IndexInfo {
                    name: idx_name,
                    columns: cols,
                    unique,
                });
            }

            let row_estimate = self
                .strings(&format!("SELECT COUNT(*) FROM {q}"))
                .ok()
                .and_then(|r| r.first().and_then(|c| c.first().cloned()).flatten())
                .and_then(|v| v.parse::<u64>().ok());

            tables.push(TableInfo {
                schema: None,
                name,
                kind,
                columns,
                primary_key: primary_key.into_iter().map(|(_, c)| c).collect(),
                foreign_keys: by_id.into_values().collect(),
                indexes,
                row_estimate,
                comment: None,
            });
        }
        Ok(tables)
    }

    fn sample(&mut self, table: &str, limit: usize) -> Result<Vec<Row>, SourceError> {
        let sql = format!("SELECT * FROM {} LIMIT {}", quote(table), limit.min(10_000));
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
        self.arm_timeout();
        let mut stmt = match self.conn.prepare(query) {
            Ok(s) => s,
            Err(e) => {
                self.disarm();
                return Err(self.query_error(e));
            }
        };
        // Declared types come from the prepared statement, so a `rml:query`
        // over real columns still reports what SQLite knows. Collected before
        // `query()` takes the statement mutably.
        let (names, declared): (Vec<String>, Vec<ValueKind>) = stmt
            .columns()
            .iter()
            .map(|c| {
                (
                    c.name().to_string(),
                    c.decl_type().map(generic_type).unwrap_or(ValueKind::Other),
                )
            })
            .unzip();

        let mut delivered: u64 = 0;
        let mut batch: Vec<Row> = Vec::with_capacity(batch_size);
        let outcome = (|| -> Result<(), SourceError> {
            let mut rows = stmt.query([]).map_err(|e| self.query_error(e))?;
            loop {
                let row = match rows.next() {
                    Ok(Some(r)) => r,
                    Ok(None) => break,
                    Err(e) => return Err(self.query_error(e)),
                };
                let mut out: Row = Row::with_capacity(names.len());
                for (i, name) in names.iter().enumerate() {
                    let raw = row.get_ref(i).map_err(|e| self.query_error(e))?;
                    // A SQL NULL is an ABSENT key, not an empty value: a term
                    // map over it then produces no triple at all.
                    if matches!(raw, ValueRef::Null) {
                        continue;
                    }
                    out.insert(
                        name.clone(),
                        Value::new(lexical(raw), value_kind_of(raw, declared[i])),
                    );
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
            Ok(())
        })();
        self.disarm();
        outcome?;
        if !batch.is_empty() {
            sink(batch)?;
        }
        Ok(delivered)
    }

    fn max_watermark(&mut self, table: &str, column: &str) -> Result<Option<String>, SourceError> {
        let sql = format!("SELECT MAX({}) FROM {}", quote(column), quote(table));
        Ok(self
            .strings(&sql)?
            .first()
            .and_then(|r| r.first().cloned())
            .flatten())
    }

    fn server_version(&mut self) -> Result<Option<String>, SourceError> {
        Ok(self
            .strings("SELECT sqlite_version()")?
            .first()
            .and_then(|r| r.first().cloned())
            .flatten()
            .map(|v| format!("SQLite {v}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn fixture() -> (tempfile::TempDir, ConnectParams) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE parent (pid INTEGER PRIMARY KEY, label TEXT NOT NULL);
             CREATE TABLE child (
                cid INTEGER PRIMARY KEY, name TEXT NOT NULL DEFAULT 'x',
                amount DECIMAL(10,2), ratio REAL, when_at DATETIME,
                parent_id INTEGER REFERENCES parent(pid));
             CREATE UNIQUE INDEX child_name ON child(name);
             CREATE VIEW child_v AS SELECT cid, name FROM child;
             INSERT INTO parent VALUES (1,'p1');
             INSERT INTO child VALUES (10,'a',1.50,2.0,'2026-01-01T00:00:00Z',1);
             INSERT INTO child (cid,name,parent_id) VALUES (11,'b',1);",
        )
        .unwrap();
        let params = ConnectParams {
            dialect: "sqlite".into(),
            host: None,
            port: None,
            database: path.to_string_lossy().into_owned(),
            username: None,
            password: None,
            read_only: true,
            statement_timeout_ms: 5_000,
            tls: false,
            options: BTreeMap::new(),
        };
        (dir, params)
    }

    #[test]
    fn introspection_reports_columns_keys_indexes_and_views() {
        let (_d, p) = fixture();
        let mut c = SqliteConnector.connect(&p).unwrap();
        let tables = c.introspect().unwrap();
        let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["child", "child_v", "parent"]);

        let child = tables.iter().find(|t| t.name == "child").unwrap();
        assert_eq!(child.kind, TableKind::Table);
        assert_eq!(child.primary_key, vec!["cid"]);
        assert_eq!(child.row_estimate, Some(2));
        let amount = child.columns.iter().find(|c| c.name == "amount").unwrap();
        assert_eq!(amount.generic_type, ValueKind::Decimal);
        assert_eq!(amount.native_type, "DECIMAL(10,2)");
        assert!(amount.nullable);
        let name = child.columns.iter().find(|c| c.name == "name").unwrap();
        assert!(!name.nullable);
        assert_eq!(name.default.as_deref(), Some("'x'"));
        assert_eq!(
            child
                .columns
                .iter()
                .find(|c| c.name == "when_at")
                .unwrap()
                .generic_type,
            ValueKind::DateTime
        );
        assert_eq!(child.foreign_keys.len(), 1);
        assert_eq!(child.foreign_keys[0].ref_table, "parent");
        assert_eq!(child.foreign_keys[0].columns, vec!["parent_id"]);
        assert_eq!(child.foreign_keys[0].ref_columns, vec!["pid"]);
        assert!(child
            .indexes
            .iter()
            .any(|i| i.name == "child_name" && i.unique));
        assert_eq!(
            tables.iter().find(|t| t.name == "child_v").unwrap().kind,
            TableKind::View
        );
    }

    #[test]
    fn null_columns_are_absent_and_reals_stay_lexically_valid() {
        let (_d, p) = fixture();
        let mut c = SqliteConnector.connect(&p).unwrap();
        let rows = c.sample("child", 10).unwrap();
        assert_eq!(rows.len(), 2);
        let with_amount = rows.iter().find(|r| r["cid"].lexical == "10").unwrap();
        assert_eq!(with_amount["amount"].lexical, "1.5");
        assert_eq!(
            with_amount["ratio"].lexical, "2.0",
            "a whole REAL keeps a fraction"
        );
        let without = rows.iter().find(|r| r["cid"].lexical == "11").unwrap();
        assert!(
            !without.contains_key("amount"),
            "a SQL NULL is an absent key"
        );
        assert!(!without.contains_key("ratio"));
    }

    #[test]
    fn streaming_batches_and_counts_every_row() {
        let (_d, p) = fixture();
        let mut c = SqliteConnector.connect(&p).unwrap();
        let mut batches = Vec::new();
        let n = c
            .stream("SELECT cid FROM child ORDER BY cid", 1, &mut |b| {
                batches.push(b.len());
                Ok(())
            })
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(
            batches,
            vec![1, 1],
            "batch_size 1 delivers one row at a time"
        );
    }

    #[test]
    fn a_sink_error_stops_the_stream() {
        let (_d, p) = fixture();
        let mut c = SqliteConnector.connect(&p).unwrap();
        let err = c
            .stream("SELECT cid FROM child", 1, &mut |_| {
                Err(SourceError::Query("stop".into()))
            })
            .unwrap_err();
        assert_eq!(err, SourceError::Query("stop".into()));
    }

    #[test]
    fn the_connection_is_read_only_and_a_missing_file_does_not_leak_its_path() {
        let (_d, p) = fixture();
        let mut c = SqliteConnector.connect(&p).unwrap();
        // A write through the mapping path is refused by SQLite itself.
        let err = c
            .stream("DELETE FROM child", 10, &mut |_| Ok(()))
            .unwrap_err();
        assert!(matches!(err, SourceError::Query(_)), "{err:?}");

        let mut missing = p.clone();
        missing.database = "/nonexistent/ots-secret-dir/db-9f1c.sqlite".into();
        let Err(err) = SqliteConnector.connect(&missing) else {
            panic!("a missing database file cannot open");
        };
        assert!(!err.to_string().contains("9f1c"), "path leaked: {err}");
    }

    #[test]
    fn watermark_reads_the_max_of_a_column() {
        let (_d, p) = fixture();
        let mut c = SqliteConnector.connect(&p).unwrap();
        assert_eq!(
            c.max_watermark("child", "cid").unwrap().as_deref(),
            Some("11")
        );
        assert!(c.server_version().unwrap().unwrap().starts_with("SQLite "));
    }

    #[test]
    fn identifiers_are_quoted_not_interpolated() {
        assert_eq!(quote(r#"we"ird"#), r#""we""ird""#);
        let (_d, p) = fixture();
        let mut c = SqliteConnector.connect(&p).unwrap();
        // A table name that would otherwise close the identifier fails as a
        // missing table, not as injected SQL.
        let err = c.sample(r#"child" ; DROP TABLE child --"#, 1).unwrap_err();
        assert!(matches!(err, SourceError::Query(_)), "{err:?}");
        assert_eq!(c.sample("child", 10).unwrap().len(), 2, "table still there");
    }

    #[test]
    fn a_read_write_request_is_refused_before_connecting() {
        let (_d, mut p) = fixture();
        p.read_only = false;
        assert!(
            matches!(SqliteConnector.connect(&p), Err(SourceError::Config(_))),
            "a read-write request never reaches the driver"
        );
    }
}
