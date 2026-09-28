//! The SQLite connector, compiled into core.
//!
//! It is the reference implementation of [`SourceConnector`]: read-only is
//! enforced by opening the file with `SQLITE_OPEN_READ_ONLY`, and the
//! statement timeout by a progress handler that aborts the statement once the
//! budget is spent. Every identifier is quoted, never interpolated raw.

use std::time::{Duration, Instant};

use ots_plugin_api::sources::{
    is_low_cardinality, BatchSink, ColumnInfo, ColumnProfile, ConnectParams, DetectedPattern,
    ForeignKey, IndexInfo, NumericSummary, Row, SourceConnection, SourceConnector, SourceError,
    TableInfo, TableKind, TableProfile, Value, ValueCount, ValueKind,
    LOW_CARDINALITY_MAX_VALUE_LEN, PROFILE_SAMPLE_ROWS, TOP_K,
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

/// A SQL string literal. Only ever used for a catalogue lookup by name —
/// data values reach SQLite as bound parameters or not at all.
fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Columns of one table covered by a single aggregate statement. SQLite caps
/// a result set at `SQLITE_MAX_COLUMN` (1 000 by default) and each profiled
/// column asks for up to five aggregates, so a very wide table is split
/// rather than refused.
const AGG_COLUMNS_PER_STATEMENT: usize = 64;

/// Where one column's aggregates landed in the select list.
#[derive(Default)]
struct Slots {
    distinct: usize,
    nulls: usize,
    min: Option<usize>,
    max: Option<usize>,
    mean: Option<usize>,
    mean_length: Option<usize>,
}

/// …and what came back.
#[derive(Clone, Default)]
struct Aggregates {
    distinct: Option<u64>,
    nulls: Option<u64>,
    min: Option<f64>,
    max: Option<f64>,
    mean: Option<f64>,
    mean_length: Option<f64>,
}

/// Append an aggregate and return the index it will come back at.
fn push(select: &mut Vec<String>, expression: String) -> usize {
    select.push(expression);
    select.len() - 1
}

impl SqliteConnection {
    /// What the catalogue lists: one row per table and view, name and kind.
    ///
    /// The whole of [`SourceConnection::table_names`], and the first step of
    /// [`SourceConnection::introspect`] — one statement against
    /// `sqlite_master`, which reads no table's contents.
    fn listing(&mut self) -> Result<Vec<(String, TableKind)>, SourceError> {
        Ok(self
            .strings(
                "SELECT name, type FROM sqlite_master WHERE type IN ('table','view') \
                 AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )?
            .into_iter()
            .map(|row| {
                let name = row.first().cloned().flatten().unwrap_or_default();
                let kind = match row.get(1).cloned().flatten().as_deref() {
                    Some("view") => TableKind::View,
                    _ => TableKind::Table,
                };
                (name, kind)
            })
            .collect())
    }

    /// One table's catalogue entry.
    ///
    /// Split out of [`SourceConnection::introspect`] so profiling one table
    /// costs that table's PRAGMAs instead of the whole schema's.
    /// `row_estimate` is opt-in because it is a `COUNT(*)`: the profiler
    /// counts rows in its own aggregate statement and does not want it twice.
    fn structure(
        &mut self,
        name: &str,
        kind: TableKind,
        row_estimate: bool,
    ) -> Result<TableInfo, SourceError> {
        let q = quote(name);

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

        let row_estimate = row_estimate
            .then(|| {
                self.strings(&format!("SELECT COUNT(*) FROM {q}"))
                    .ok()
                    .and_then(|r| r.first().and_then(|c| c.first().cloned()).flatten())
                    .and_then(|v| v.parse::<u64>().ok())
            })
            .flatten();

        Ok(TableInfo {
            schema: None,
            name: name.to_string(),
            kind,
            columns,
            primary_key: primary_key.into_iter().map(|(_, c)| c).collect(),
            foreign_keys: by_id.into_values().collect(),
            indexes,
            row_estimate,
            comment: None,
        })
    }

    /// What the catalogue calls `name`, or `None` when it holds no such
    /// table or view.
    fn kind_of(&mut self, name: &str) -> Result<Option<TableKind>, SourceError> {
        let rows = self.strings(&format!(
            "SELECT type FROM sqlite_master WHERE name = {} AND type IN ('table','view')",
            literal(name)
        ))?;
        Ok(rows
            .first()
            .and_then(|r| r.first().cloned())
            .flatten()
            .map(|t| match t.as_str() {
                "view" => TableKind::View,
                _ => TableKind::Table,
            }))
    }

    /// The code list of a low-cardinality column, most frequent first.
    ///
    /// Ties break on the value itself, so two profiling runs over unchanged
    /// data produce identical RDF and a version diff shows drift rather than
    /// the database's choice of order.
    ///
    /// **The length ceiling.** A count alone does not make a value a code: a
    /// handful of document bodies repeated across a table is low-cardinality
    /// and is still row content. One value past
    /// [`LOW_CARDINALITY_MAX_VALUE_LEN`] therefore discards the *whole*
    /// column's list rather than that value — a column holding a document is
    /// not a value map with one odd entry, and a list with an entry silently
    /// removed is worse than none: the top-k is documented as the complete
    /// value set, so a proposer would build an enumeration missing a member
    /// and no reader could tell.
    ///
    /// Only the ceiling plus one character of any value crosses the wire, so
    /// an over-long value is detected without ever being read.
    fn top_values(
        &mut self,
        quoted_table: &str,
        column: &str,
    ) -> Result<Vec<ValueCount>, SourceError> {
        let qc = quote(column);
        let rows = self.strings(&format!(
            "SELECT SUBSTR({qc}, 1, {probe}), COUNT(*) AS n, LENGTH({qc}) FROM {quoted_table} \
             WHERE {qc} IS NOT NULL GROUP BY {qc} ORDER BY n DESC, {qc} ASC LIMIT {TOP_K}",
            probe = LOW_CARDINALITY_MAX_VALUE_LEN + 1
        ))?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let length: usize = r
                .get(2)
                .cloned()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            if length > LOW_CARDINALITY_MAX_VALUE_LEN {
                return Ok(Vec::new());
            }
            let (Some(value), Some(count)) = (
                r.first().cloned().flatten(),
                r.get(1).cloned().flatten().and_then(|v| v.parse().ok()),
            ) else {
                continue;
            };
            out.push(ValueCount { value, count });
        }
        Ok(out)
    }

    /// The median and 99th percentile of a numeric column.
    ///
    /// SQLite has no percentile function, so the ordered row at the offset is
    /// the percentile — still one statement that returns two values, not a
    /// scan that returns the column. Nearest-rank, so `p99` keeps the property
    /// people read it for: at least 99% of the values are at or below it.
    fn percentiles(
        &mut self,
        quoted_table: &str,
        column: &str,
        non_null: u64,
    ) -> Result<(Option<f64>, Option<f64>), SourceError> {
        let qc = quote(column);
        let offset = |p: f64| {
            (((non_null as f64 * p).ceil() as u64).max(1) - 1).min(non_null.saturating_sub(1))
        };
        let at = |o: u64| {
            format!(
                "(SELECT {qc} FROM {quoted_table} WHERE {qc} IS NOT NULL ORDER BY {qc} \
                 LIMIT 1 OFFSET {o})"
            )
        };
        let rows = self.strings(&format!(
            "SELECT {}, {}",
            at(offset(0.50)),
            at(offset(0.99))
        ))?;
        let row = rows.into_iter().next().unwrap_or_default();
        let cell = |i: usize| -> Option<f64> { row.get(i).cloned().flatten()?.parse().ok() };
        Ok((cell(0), cell(1)))
    }
}

impl SourceConnection for SqliteConnection {
    fn introspect(&mut self) -> Result<Vec<TableInfo>, SourceError> {
        let listing = self.listing()?;
        let mut tables = Vec::with_capacity(listing.len());
        for (name, kind) in listing {
            tables.push(self.structure(&name, kind, true)?);
        }
        Ok(tables)
    }

    /// Overridden so listing the catalogue does not trigger the row-estimate
    /// `COUNT(*)` that full introspection runs per table.
    fn table_names(&mut self) -> Result<Vec<String>, SourceError> {
        Ok(self.listing()?.into_iter().map(|(name, _)| name).collect())
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

    /// Overridden so the join planner does not trigger the row-estimate
    /// `COUNT(*)` that full introspection runs per table.
    fn unique_keys(&mut self, table: &str) -> Result<Vec<Vec<String>>, SourceError> {
        let q = quote(table);
        let mut keys: Vec<Vec<String>> = Vec::new();

        let mut pk: Vec<(i64, String)> = Vec::new();
        for c in self.strings(&format!("PRAGMA table_info({q})"))? {
            let name = c.get(1).cloned().flatten().unwrap_or_default();
            let pos = c
                .get(5)
                .cloned()
                .flatten()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0);
            if pos > 0 {
                pk.push((pos, name));
            }
        }
        pk.sort_by_key(|(pos, _)| *pos);
        if !pk.is_empty() {
            keys.push(pk.into_iter().map(|(_, c)| c).collect());
        }

        for i in self.strings(&format!("PRAGMA index_list({q})"))? {
            if i.get(2).cloned().flatten().as_deref() != Some("1") {
                continue;
            }
            let idx_name = i.get(1).cloned().flatten().unwrap_or_default();
            // A partial unique index does not constrain every row, so it is not
            // a key the planner may rely on. `PRAGMA index_list` reports that in
            // its `partial` column.
            if i.get(4).cloned().flatten().as_deref() == Some("1") {
                continue;
            }
            let cols: Vec<String> = self
                .strings(&format!("PRAGMA index_info({})", quote(&idx_name)))?
                .into_iter()
                .filter_map(|r| r.get(2).cloned().flatten())
                .collect();
            if !cols.is_empty() && !keys.contains(&cols) {
                keys.push(cols);
            }
        }
        Ok(keys)
    }

    /// Profile a table with aggregate SQL: the database counts, this code
    /// only reads the answers back.
    ///
    /// Statement budget per table: one aggregate statement covering the row
    /// count and every column's distinct and NULL counts (split into chunks
    /// only because SQLite caps the columns in a result set), one bounded
    /// sample for the pattern heuristic, one ordered statement per numeric
    /// column for its percentiles, and one grouped statement per code-list
    /// column. Nothing streams the table into memory — the point of profiling
    /// a production replica is that it stays cheap.
    fn profile(&mut self, table: &str) -> Result<TableProfile, SourceError> {
        let kind = self
            .kind_of(table)?
            .ok_or_else(|| SourceError::Query(format!("no table or view named '{table}'")))?;
        let info = self.structure(table, kind, false)?;
        let unique_keys = self.unique_keys(table)?;
        let q = quote(table);

        let mut row_count = 0u64;
        let mut aggregates: Vec<Aggregates> = Vec::with_capacity(info.columns.len());
        for chunk in info.columns.chunks(AGG_COLUMNS_PER_STATEMENT) {
            let mut select = vec!["COUNT(*)".to_string()];
            let mut slots: Vec<Slots> = Vec::with_capacity(chunk.len());
            for c in chunk {
                let qc = quote(&c.name);
                let mut s = Slots {
                    distinct: push(&mut select, format!("COUNT(DISTINCT {qc})")),
                    nulls: push(
                        &mut select,
                        format!("SUM(CASE WHEN {qc} IS NULL THEN 1 ELSE 0 END)"),
                    ),
                    ..Slots::default()
                };
                if c.generic_type.is_numeric() {
                    s.min = Some(push(&mut select, format!("MIN({qc})")));
                    s.max = Some(push(&mut select, format!("MAX({qc})")));
                    s.mean = Some(push(&mut select, format!("AVG({qc})")));
                } else if c.generic_type == ValueKind::Text {
                    // The LENGTH and nothing else: MIN or MAX of a text column
                    // would *be* a row value, dressed up as a statistic.
                    s.mean_length = Some(push(&mut select, format!("AVG(LENGTH({qc}))")));
                }
                slots.push(s);
            }
            let rows = self.strings(&format!("SELECT {} FROM {q}", select.join(", ")))?;
            let row = rows.into_iter().next().unwrap_or_default();
            let cell = |i: usize| -> Option<String> { row.get(i).cloned().flatten() };
            row_count = cell(0).and_then(|v| v.parse().ok()).unwrap_or(0);
            for s in slots {
                aggregates.push(Aggregates {
                    distinct: cell(s.distinct).and_then(|v| v.parse().ok()),
                    // SUM over no rows is NULL, which is zero NULLs, not an
                    // unknown count.
                    nulls: cell(s.nulls).and_then(|v| v.parse().ok()).or(Some(0)),
                    min: s.min.and_then(cell).and_then(|v| v.parse().ok()),
                    max: s.max.and_then(cell).and_then(|v| v.parse().ok()),
                    mean: s.mean.and_then(cell).and_then(|v| v.parse().ok()),
                    mean_length: s.mean_length.and_then(cell).and_then(|v| v.parse().ok()),
                });
            }
        }

        let sample = self.sample(table, PROFILE_SAMPLE_ROWS)?;

        let mut columns = Vec::with_capacity(info.columns.len());
        for (i, c) in info.columns.iter().enumerate() {
            let agg = aggregates.get(i).cloned().unwrap_or_default();
            let nulls = agg.nulls.unwrap_or(0);
            let non_null = row_count.saturating_sub(nulls);
            let distinct = agg.distinct;
            let ratio = match (distinct, non_null) {
                (Some(d), n) if n > 0 => Some(d as f64 / n as f64),
                _ => None,
            };

            let top_values = match distinct {
                Some(d) if is_low_cardinality(d, non_null) => self.top_values(&q, &c.name)?,
                _ => Vec::new(),
            };

            let numeric = match (agg.min, agg.max, agg.mean) {
                (Some(min), Some(max), Some(mean)) if non_null > 0 => {
                    let (p50, p99) = self.percentiles(&q, &c.name, non_null)?;
                    Some(NumericSummary {
                        min,
                        max,
                        mean,
                        p50: p50.unwrap_or(min),
                        p99: p99.unwrap_or(max),
                    })
                }
                _ => None,
            };

            let pattern = if c.generic_type.is_stringish() {
                DetectedPattern::of_sample(
                    sample
                        .iter()
                        .filter_map(|row| row.get(&c.name))
                        .map(|v| v.lexical.as_str()),
                )
            } else {
                None
            };

            columns.push(ColumnProfile {
                name: c.name.clone(),
                position: i as u32 + 1,
                native_type: c.native_type.clone(),
                generic_type: c.generic_type,
                nullable: c.nullable,
                primary_key: c.primary_key,
                distinct_count: distinct,
                null_count: Some(nulls),
                cardinality_ratio: ratio,
                top_values,
                numeric,
                mean_length: agg.mean_length,
                pattern: pattern.map(|(p, _)| p),
                pattern_confidence: pattern.map(|(_, conf)| conf),
            });
        }

        Ok(TableProfile {
            table: info.name,
            kind: info.kind,
            row_count: Some(row_count),
            columns,
            primary_key: info.primary_key,
            unique_keys,
            foreign_keys: info.foreign_keys,
            sampled_rows: sample.len() as u64,
        })
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
    fn unique_keys_reports_the_primary_key_and_unique_indexes() {
        let (_d, p) = fixture();
        let mut c = SqliteConnector.connect(&p).unwrap();
        let keys = c.unique_keys("child").unwrap();
        assert!(
            keys.contains(&vec!["cid".to_string()]),
            "primary key: {keys:?}"
        );
        assert!(
            keys.contains(&vec!["name".to_string()]),
            "unique index: {keys:?}"
        );
        // A non-unique column is not a key, and an unknown table is empty
        // rather than an error — the planner treats both as "cannot push down".
        assert!(!keys.contains(&vec!["parent_id".to_string()]), "{keys:?}");
        assert!(c.unique_keys("no_such_table").unwrap().is_empty());
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

    /// A table wide enough for the aggregates to have something to say.
    fn profiling_fixture() -> (tempfile::TempDir, ConnectParams) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE entry (
                entry_id INTEGER PRIMARY KEY,
                state TEXT NOT NULL,
                note TEXT,
                amount DECIMAL(10,2));
             CREATE TABLE unused (unused_id INTEGER PRIMARY KEY, label TEXT);",
        )
        .unwrap();
        for i in 1..=12 {
            let state = ["alpha", "beta", "gamma"][i % 3];
            let note = if i % 6 == 0 {
                "NULL".to_string()
            } else {
                format!("'note text number {i} of the set'")
            };
            let amount = if i == 1 {
                "NULL".to_string()
            } else {
                format!("{}.5", i)
            };
            conn.execute_batch(&format!(
                "INSERT INTO entry VALUES ({i}, '{state}', {note}, {amount});"
            ))
            .unwrap();
        }
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
    fn profiling_counts_in_sql_and_reports_a_code_list_but_not_free_text() {
        let (_d, p) = profiling_fixture();
        let mut c = SqliteConnector.connect(&p).unwrap();
        let profile = c.profile("entry").unwrap();
        assert_eq!(profile.row_count, Some(12));
        assert_eq!(profile.primary_key, vec!["entry_id".to_string()]);
        assert_eq!(profile.unique_keys, vec![vec!["entry_id".to_string()]]);
        assert_eq!(profile.sampled_rows, 12);

        let col = |name: &str| profile.columns.iter().find(|c| c.name == name).unwrap();

        // Few values that repeat: a code list, complete and ordered.
        let state = col("state");
        assert_eq!(state.distinct_count, Some(3));
        assert_eq!(state.null_count, Some(0));
        assert_eq!(state.cardinality_ratio, Some(0.25));
        assert_eq!(state.top_values.len(), 3);
        assert_eq!(state.top_values.iter().map(|v| v.count).sum::<u64>(), 12);
        assert!(state
            .top_values
            .windows(2)
            .all(|w| w[0].count >= w[1].count));
        assert!(state.numeric.is_none(), "text gets no numeric summary");

        // Free text: counted and measured, never listed.
        let note = col("note");
        assert_eq!(note.null_count, Some(2));
        assert_eq!(note.distinct_count, Some(10));
        assert_eq!(note.cardinality_ratio, Some(1.0));
        assert!(note.top_values.is_empty(), "values that never repeat");
        assert!(note.mean_length.is_some_and(|l| l > 20.0));
        assert!(note.numeric.is_none());

        // Numeric: the database computed every one of these.
        let amount = col("amount");
        assert_eq!(amount.null_count, Some(1));
        let n = amount.numeric.expect("a numeric summary");
        assert_eq!((n.min, n.max), (2.5, 12.5));
        assert_eq!(n.mean, 7.5);
        assert_eq!(n.p50, 7.5, "the ordered value at the median offset");
        assert_eq!(n.p99, 12.5);
        assert!(amount.top_values.is_empty());

        // The primary key is unique, so it is never mistaken for a code list.
        assert_eq!(col("entry_id").cardinality_ratio, Some(1.0));
        assert!(col("entry_id").top_values.is_empty());

        assert!(matches!(
            c.profile("no_such_table"),
            Err(SourceError::Query(_))
        ));
    }

    #[test]
    fn listing_the_catalogue_reads_no_table() {
        let (_d, p) = profiling_fixture();
        // A view whose body names a table that does not exist: listing it is
        // free, and anything that reaches into it fails.
        Connection::open(&p.database)
            .unwrap()
            .execute_batch("CREATE VIEW excluded AS SELECT * FROM does_not_exist;")
            .unwrap();
        let mut c = SqliteConnector.connect(&p).unwrap();

        assert_eq!(
            c.table_names().unwrap(),
            vec![
                "entry".to_string(),
                "excluded".to_string(),
                "unused".to_string()
            ]
        );
        assert!(
            c.introspect().is_err(),
            "full introspection is what table_names must not be"
        );
        // …and profiling one table still works while that view exists.
        assert_eq!(c.profile("entry").unwrap().row_count, Some(12));
    }

    #[test]
    fn a_repeating_value_too_long_to_be_a_code_takes_the_whole_list_with_it() {
        let (dir, mut p) = profiling_fixture();
        let path = dir.path().join("doc.db");
        p.database = path.to_string_lossy().into_owned();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE doc (doc_id INTEGER PRIMARY KEY, body TEXT NOT NULL, tag TEXT NOT NULL);",
        )
        .unwrap();
        for i in 0..100u32 {
            let body = format!(
                "body {} {}",
                i % 5,
                "x".repeat(LOW_CARDINALITY_MAX_VALUE_LEN)
            );
            conn.execute(
                "INSERT INTO doc (body, tag) VALUES (?1, ?2)",
                rusqlite::params![body, format!("t{}", i % 5)],
            )
            .unwrap();
        }
        drop(conn);

        let mut c = SqliteConnector.connect(&p).unwrap();
        let profile = c.profile("doc").unwrap();
        let col = |name: &str| profile.columns.iter().find(|c| c.name == name).unwrap();

        // Five distinct values over a hundred rows is low-cardinality by
        // count, and still row content.
        let body = col("body");
        assert_eq!(body.distinct_count, Some(5));
        assert!(
            body.top_values.is_empty(),
            "a document is not a code: {:?}",
            body.top_values
        );
        assert!(body.mean_length.is_some(), "the aggregate over it remains");

        // A code list of the same shape, within the ceiling, is untouched.
        let tag = col("tag");
        assert_eq!(tag.distinct_count, Some(5));
        assert_eq!(tag.top_values.len(), 5);
        assert_eq!(tag.top_values.iter().map(|v| v.count).sum::<u64>(), 100);
    }

    #[test]
    fn profiling_a_view_and_an_empty_table_stays_within_the_contract() {
        let (_d, p) = fixture();
        let mut c = SqliteConnector.connect(&p).unwrap();
        let view = c.profile("child_v").unwrap();
        assert_eq!(view.kind, TableKind::View);
        assert_eq!(view.row_count, Some(2));
        assert!(view.primary_key.is_empty());

        // Two rows, two distinct names: nothing repeats, so nothing is listed.
        let name = view.columns.iter().find(|c| c.name == "name").unwrap();
        assert_eq!(name.distinct_count, Some(2));
        assert!(name.top_values.is_empty());

        // An empty table reports zeroes, not unknowns, and no summaries: a
        // ratio over no rows is undefined, and undefined is `None`.
        let (_d2, p2) = profiling_fixture();
        let mut c2 = SqliteConnector.connect(&p2).unwrap();
        let empty = c2.profile("unused").unwrap();
        assert_eq!(empty.row_count, Some(0));
        assert_eq!(empty.sampled_rows, 0);
        for column in &empty.columns {
            assert_eq!(column.distinct_count, Some(0), "{}", column.name);
            assert_eq!(column.null_count, Some(0), "{}", column.name);
            assert_eq!(column.cardinality_ratio, None, "{}", column.name);
            assert!(column.numeric.is_none() && column.top_values.is_empty());
            assert!(column.pattern.is_none(), "no sample, no shape");
        }
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
