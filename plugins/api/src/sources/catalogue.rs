//! What the networked dialects share: the catalogue read from
//! `INFORMATION_SCHEMA`, the aggregate profiler, and the lexical forms the
//! R2RML natural mapping expects.
//!
//! PostgreSQL, MySQL/MariaDB and SQL Server all publish their catalogue
//! through `INFORMATION_SCHEMA` and all answer the same aggregate SQL. What
//! differs is spelling — how an identifier is quoted, how a result is
//! limited, what the schema of the moment is called — and a handful of
//! catalogue views that are the dialect's own (foreign keys in full, unique
//! indexes, row estimates, comments). A driver therefore supplies two
//! things: an [`Executor`] that runs SQL and hands rows back as lexical
//! cells, and a [`Dialect`] that spells the fragments. Everything built on
//! those — [`introspect`], [`profile`], [`unique_keys`] — is written once
//! here, so three drivers cannot disagree on what a primary key, a code
//! list or a row count is.
//!
//! Streaming a query's rows stays with the driver: that is the one place
//! the wire protocol matters.

use std::collections::BTreeMap;

use super::{
    is_low_cardinality, ColumnInfo, ColumnProfile, DetectedPattern, ForeignKey, IndexInfo,
    NumericSummary, Row, SourceError, TableInfo, TableKind, TableProfile, ValueCount, ValueKind,
    LOW_CARDINALITY_MAX_VALUE_LEN, TOP_K,
};

/// Runs SQL and returns every row as lexical cells, `None` for NULL. The
/// catalogue and the profiler are written against this and nothing else.
pub trait Executor {
    fn rows(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>, SourceError>;
}

/// The SQL a dialect spells differently. Every method takes and returns
/// SQL text; identifiers arrive raw and leave quoted.
pub trait Dialect: Send + Sync {
    /// Quote an identifier: `"x"`, `` `x` ``, `[x]`.
    fn quote(&self, ident: &str) -> String;

    /// A string literal for a catalogue lookup by name. Data values never
    /// travel this way.
    fn literal(&self, value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }

    /// The schema the catalogue is read from, as an SQL expression the
    /// server evaluates: `current_schema()`, `DATABASE()`, `SCHEMA_NAME()`.
    fn schema_expr(&self) -> String;

    /// `SELECT <body>` limited to `limit` rows, in the dialect's spelling
    /// (`LIMIT n` at the end, or `TOP n` after `SELECT`). `body` is
    /// everything after the `SELECT` keyword.
    fn select_limited(&self, body: &str, limit: usize) -> String;

    /// A scalar subquery yielding the value at `offset` (0-based) of
    /// `column_q` in ascending order over the non-NULL rows of `table_q`.
    /// Both arguments are already quoted.
    fn nth_value(&self, table_q: &str, column_q: &str, offset: u64) -> String;

    /// Character length of an expression.
    fn char_length(&self, expr: &str) -> String;

    /// An expression cast to the dialect's text type.
    fn to_text(&self, expr: &str) -> String;

    /// `AVG` over an expression as a floating-point number — a dialect whose
    /// `AVG` of an integer column is an integer casts here.
    fn avg(&self, expr: &str) -> String {
        format!("AVG({expr})")
    }

    /// Column listing for `table`, in this order: name, native type,
    /// nullable (`YES` / `NO`), default, comment.
    fn columns_sql(&self, table: &str) -> String;

    /// Foreign keys of `table`, one row per column, in this order:
    /// constraint name, column, referenced table, referenced column —
    /// ordered by constraint then position.
    fn foreign_keys_sql(&self, table: &str) -> String;

    /// Unique indexes of `table` that are not also constraints: index
    /// name, column, position — unique, whole-table indexes only (a partial
    /// or filtered index does not constrain every row). `None` when the
    /// dialect has nothing beyond its constraints.
    fn unique_indexes_sql(&self, _table: &str) -> Option<String> {
        None
    }

    /// One row, one cell: the planner's row estimate for `table`, or NULL.
    fn row_estimate_sql(&self, _table: &str) -> Option<String> {
        None
    }

    /// One row, one cell: the table's comment, or NULL.
    fn table_comment_sql(&self, _table: &str) -> Option<String> {
        None
    }

    /// The generic type of a native type name. [`standard_type`] covers
    /// the SQL-standard names; a dialect refines it for its own.
    fn map_type(&self, native: &str) -> ValueKind {
        standard_type(native)
    }

    /// Whether a column of this native type can be counted distinct and
    /// grouped. A dialect with types its own aggregates refuse (SQL Server's
    /// `text`, `xml`, the spatial types) says so, and the profiler reports
    /// no distinct count for such a column rather than failing the table.
    fn countable(&self, _native: &str) -> bool {
        true
    }
}

/// The SQL-standard type names, and the common vendor spellings, as generic
/// types. A length or precision in parentheses is ignored.
pub fn standard_type(native: &str) -> ValueKind {
    let lower = native.trim().to_ascii_lowercase();
    let base = lower.split('(').next().unwrap_or("").trim();
    let base = base
        .strip_suffix(" unsigned")
        .or_else(|| base.strip_suffix(" signed"))
        .unwrap_or(base);
    match base {
        "" => ValueKind::Other,
        "interval" => ValueKind::Other,
        "bool" | "boolean" | "bit" => ValueKind::Boolean,
        "uuid" | "uniqueidentifier" => ValueKind::Uuid,
        "json" | "jsonb" => ValueKind::Json,
        "date" => ValueKind::Date,
        t if t.starts_with("time ") || t == "time" || t == "timetz" => ValueKind::Time,
        t if t.starts_with("timestamp") || t.contains("datetime") || t == "smalldatetime" => {
            ValueKind::DateTime
        }
        "real" | "float" | "float4" | "float8" | "double" | "double precision" => ValueKind::Float,
        "decimal" | "numeric" | "money" | "smallmoney" | "dec" | "fixed" => ValueKind::Decimal,
        "year" => ValueKind::Integer,
        t if t.contains("int") || t == "serial" || t == "bigserial" || t == "smallserial" => {
            ValueKind::Integer
        }
        "bytea" | "binary" | "varbinary" | "image" | "blob" | "tinyblob" | "mediumblob"
        | "longblob" => ValueKind::Binary,
        "xml" => ValueKind::Other,
        t if t.contains("char")
            || t.contains("text")
            || t == "clob"
            || t == "citext"
            || t == "enum"
            || t == "set"
            || t == "name" =>
        {
            ValueKind::Text
        }
        _ => ValueKind::Other,
    }
}

/// The lexical form the R2RML natural mapping wants, from what a server
/// prints: booleans as `true` / `false`, timestamps with a `T` and a full
/// `±hh:mm` offset. Anything else passes through as the server wrote it.
pub fn canonical(kind: ValueKind, lexical: &str) -> String {
    match kind {
        ValueKind::Boolean => match lexical.trim().to_ascii_lowercase().as_str() {
            "t" | "true" | "1" | "y" | "yes" => "true".to_string(),
            "f" | "false" | "0" | "n" | "no" => "false".to_string(),
            _ => lexical.to_string(),
        },
        ValueKind::DateTime => {
            let mut s = lexical.trim().to_string();
            // `2026-01-01 00:00:00` → `2026-01-01T00:00:00`.
            if s.len() > 10
                && s.as_bytes()[10] == b' '
                && s[..10].bytes().all(|b| b.is_ascii_digit() || b == b'-')
            {
                s.replace_range(10..11, "T");
            }
            with_full_offset(&s)
        }
        ValueKind::Time => with_full_offset(lexical.trim()),
        ValueKind::Float => match lexical.trim() {
            "Infinity" | "inf" | "+Infinity" => "INF".to_string(),
            "-Infinity" | "-inf" => "-INF".to_string(),
            other => other.to_string(),
        },
        _ => lexical.to_string(),
    }
}

/// `…+00` → `…+00:00`: PostgreSQL prints a whole-hour offset without its
/// minutes, which XSD does not accept.
fn with_full_offset(s: &str) -> String {
    let b = s.as_bytes();
    let n = b.len();
    // An offset follows a time, and a time has colons; a bare date such as
    // `2026-01-01` ends in `-01` and is not one.
    if n >= 6
        && matches!(b[n - 3], b'+' | b'-')
        && b[n - 2].is_ascii_digit()
        && b[n - 1].is_ascii_digit()
        && s[..n - 3].contains(':')
    {
        return format!("{s}:00");
    }
    s.to_string()
}

fn cell(row: &[Option<String>], i: usize) -> Option<String> {
    row.get(i).cloned().flatten()
}

fn kind_of(table_type: &str) -> TableKind {
    if table_type.trim().eq_ignore_ascii_case("view") {
        TableKind::View
    } else {
        TableKind::Table
    }
}

/// The catalogue's tables and views, by name.
pub fn listing(
    exec: &mut dyn Executor,
    d: &dyn Dialect,
) -> Result<Vec<(String, TableKind)>, SourceError> {
    let sql = format!(
        "SELECT TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = {} \
         AND TABLE_TYPE IN ('BASE TABLE', 'VIEW') ORDER BY TABLE_NAME",
        d.schema_expr()
    );
    Ok(exec
        .rows(&sql)?
        .iter()
        .filter_map(|r| Some((cell(r, 0)?, kind_of(&cell(r, 1).unwrap_or_default()))))
        .collect())
}

/// What the catalogue calls `name`, or `None`.
pub fn kind_of_table(
    exec: &mut dyn Executor,
    d: &dyn Dialect,
    name: &str,
) -> Result<Option<TableKind>, SourceError> {
    let sql = format!(
        "SELECT TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {} \
         AND TABLE_TYPE IN ('BASE TABLE', 'VIEW')",
        d.schema_expr(),
        d.literal(name)
    );
    Ok(exec
        .rows(&sql)?
        .first()
        .and_then(|r| cell(r, 0))
        .map(|t| kind_of(&t)))
}

/// Primary key and unique constraints of `table`, as ordered column lists,
/// the primary key first.
fn constraints(
    exec: &mut dyn Executor,
    d: &dyn Dialect,
    table: &str,
) -> Result<(Vec<String>, Vec<Vec<String>>), SourceError> {
    let sql = format!(
        "SELECT TC.CONSTRAINT_NAME, TC.CONSTRAINT_TYPE, KCU.COLUMN_NAME, KCU.ORDINAL_POSITION \
         FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS TC \
         JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE KCU \
           ON KCU.CONSTRAINT_NAME = TC.CONSTRAINT_NAME \
          AND KCU.TABLE_SCHEMA = TC.TABLE_SCHEMA AND KCU.TABLE_NAME = TC.TABLE_NAME \
         WHERE TC.TABLE_SCHEMA = {schema} AND TC.TABLE_NAME = {name} \
           AND TC.CONSTRAINT_TYPE IN ('PRIMARY KEY', 'UNIQUE') \
         ORDER BY TC.CONSTRAINT_NAME, KCU.ORDINAL_POSITION",
        schema = d.schema_expr(),
        name = d.literal(table)
    );
    let mut primary: Vec<(i64, String)> = Vec::new();
    let mut unique: BTreeMap<String, Vec<(i64, String)>> = BTreeMap::new();
    for r in exec.rows(&sql)? {
        let (Some(name), Some(kind), Some(column)) = (cell(&r, 0), cell(&r, 1), cell(&r, 2)) else {
            continue;
        };
        let position = cell(&r, 3).and_then(|p| p.parse().ok()).unwrap_or(0);
        if kind.eq_ignore_ascii_case("PRIMARY KEY") {
            primary.push((position, column));
        } else {
            unique.entry(name).or_default().push((position, column));
        }
    }
    primary.sort_by_key(|(p, _)| *p);
    let mut keys: Vec<Vec<String>> = Vec::new();
    for (_, mut cols) in unique {
        cols.sort_by_key(|(p, _)| *p);
        let cols: Vec<String> = cols.into_iter().map(|(_, c)| c).collect();
        if !cols.is_empty() && !keys.contains(&cols) {
            keys.push(cols);
        }
    }
    Ok((primary.into_iter().map(|(_, c)| c).collect(), keys))
}

/// Unique indexes beyond the constraints, as ordered column lists.
fn unique_indexes(
    exec: &mut dyn Executor,
    d: &dyn Dialect,
    table: &str,
) -> Result<Vec<IndexInfo>, SourceError> {
    let Some(sql) = d.unique_indexes_sql(table) else {
        return Ok(Vec::new());
    };
    let mut by_name: BTreeMap<String, Vec<(i64, String)>> = BTreeMap::new();
    for r in exec.rows(&sql)? {
        let (Some(name), Some(column)) = (cell(&r, 0), cell(&r, 1)) else {
            continue;
        };
        let position = cell(&r, 2).and_then(|p| p.parse().ok()).unwrap_or(0);
        by_name.entry(name).or_default().push((position, column));
    }
    Ok(by_name
        .into_iter()
        .map(|(name, mut cols)| {
            cols.sort_by_key(|(p, _)| *p);
            IndexInfo {
                name,
                columns: cols.into_iter().map(|(_, c)| c).collect(),
                unique: true,
            }
        })
        .collect())
}

fn foreign_keys(
    exec: &mut dyn Executor,
    d: &dyn Dialect,
    table: &str,
) -> Result<Vec<ForeignKey>, SourceError> {
    let mut by_name: BTreeMap<String, ForeignKey> = BTreeMap::new();
    for r in exec.rows(&d.foreign_keys_sql(table))? {
        let (Some(name), Some(column), Some(ref_table), Some(ref_column)) =
            (cell(&r, 0), cell(&r, 1), cell(&r, 2), cell(&r, 3))
        else {
            continue;
        };
        let fk = by_name.entry(name).or_insert_with(|| ForeignKey {
            columns: Vec::new(),
            ref_table,
            ref_columns: Vec::new(),
        });
        fk.columns.push(column);
        fk.ref_columns.push(ref_column);
    }
    Ok(by_name.into_values().collect())
}

/// One table's catalogue entry. `row_estimate` is opt-in: the profiler
/// counts rows itself and does not want the planner's guess beside it.
pub fn structure(
    exec: &mut dyn Executor,
    d: &dyn Dialect,
    name: &str,
    kind: TableKind,
    row_estimate: bool,
) -> Result<TableInfo, SourceError> {
    let (primary_key, _) = constraints(exec, d, name)?;
    let mut columns = Vec::new();
    for r in exec.rows(&d.columns_sql(name))? {
        let Some(col) = cell(&r, 0) else { continue };
        let native = cell(&r, 1).unwrap_or_default();
        columns.push(ColumnInfo {
            generic_type: d.map_type(&native),
            primary_key: primary_key.contains(&col),
            name: col,
            native_type: native,
            nullable: !cell(&r, 2).is_some_and(|n| n.eq_ignore_ascii_case("NO")),
            default: cell(&r, 3),
            comment: cell(&r, 4).filter(|c| !c.trim().is_empty()),
        });
    }
    let foreign_keys = foreign_keys(exec, d, name)?;
    let indexes = unique_indexes(exec, d, name)?;
    let row_estimate = if row_estimate {
        match d.row_estimate_sql(name) {
            Some(sql) => exec
                .rows(&sql)
                .ok()
                .and_then(|rows| rows.first().and_then(|r| cell(r, 0)))
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| *v >= 0.0)
                .map(|v| v as u64),
            None => None,
        }
    } else {
        None
    };
    let comment = match d.table_comment_sql(name) {
        Some(sql) => exec
            .rows(&sql)
            .ok()
            .and_then(|rows| rows.first().and_then(|r| cell(r, 0)))
            .filter(|c| !c.trim().is_empty()),
        None => None,
    };
    Ok(TableInfo {
        schema: None,
        name: name.to_string(),
        kind,
        columns,
        primary_key,
        foreign_keys,
        indexes,
        row_estimate,
        comment,
    })
}

/// Every table and view, with its structure.
pub fn introspect(exec: &mut dyn Executor, d: &dyn Dialect) -> Result<Vec<TableInfo>, SourceError> {
    let listing = listing(exec, d)?;
    let mut out = Vec::with_capacity(listing.len());
    for (name, kind) in listing {
        out.push(structure(exec, d, &name, kind, true)?);
    }
    Ok(out)
}

/// Column sets that are unique in `table`, primary key first: the
/// constraints, then the unique indexes.
pub fn unique_keys(
    exec: &mut dyn Executor,
    d: &dyn Dialect,
    table: &str,
) -> Result<Vec<Vec<String>>, SourceError> {
    let (primary, mut keys) = constraints(exec, d, table)?;
    let mut out = Vec::new();
    if !primary.is_empty() {
        out.push(primary);
    }
    out.append(&mut keys);
    for index in unique_indexes(exec, d, table)? {
        if !index.columns.is_empty() && !out.contains(&index.columns) {
            out.push(index.columns);
        }
    }
    Ok(out)
}

/// `SELECT * FROM table` limited, for a sample.
pub fn sample_sql(d: &dyn Dialect, table: &str, limit: usize) -> String {
    d.select_limited(
        &format!("* FROM {}", d.quote(table)),
        limit.clamp(1, 10_000),
    )
}

/// `MAX(column)` of `table`, lexical.
pub fn max_watermark(
    exec: &mut dyn Executor,
    d: &dyn Dialect,
    table: &str,
    column: &str,
) -> Result<Option<String>, SourceError> {
    let sql = format!("SELECT MAX({}) FROM {}", d.quote(column), d.quote(table));
    Ok(exec.rows(&sql)?.first().and_then(|r| cell(r, 0)))
}

/// Columns of one table covered by a single aggregate statement.
const AGG_COLUMNS_PER_STATEMENT: usize = 64;

#[derive(Default)]
struct Slots {
    distinct: usize,
    nulls: usize,
    min: Option<usize>,
    max: Option<usize>,
    mean: Option<usize>,
    mean_length: Option<usize>,
}

#[derive(Clone, Default)]
struct Aggregates {
    distinct: Option<u64>,
    nulls: Option<u64>,
    min: Option<f64>,
    max: Option<f64>,
    mean: Option<f64>,
    mean_length: Option<f64>,
}

fn push(select: &mut Vec<String>, expression: String) -> usize {
    select.push(expression);
    select.len() - 1
}

/// The code list of a low-cardinality column, most frequent first, ties on
/// the value. One value past [`LOW_CARDINALITY_MAX_VALUE_LEN`] discards the
/// whole list: a column holding documents is not a value map, and a list
/// with a member silently dropped is worse than none. Only the ceiling plus
/// one character of any value crosses the wire.
fn top_values(
    exec: &mut dyn Executor,
    d: &dyn Dialect,
    table_q: &str,
    column: &str,
) -> Result<Vec<ValueCount>, SourceError> {
    let qc = d.quote(column);
    let text = d.to_text(&qc);
    let probe = LOW_CARDINALITY_MAX_VALUE_LEN + 1;
    let body = format!(
        "SUBSTRING({text}, 1, {probe}) AS v, COUNT(*) AS n, MAX({len}) AS l FROM {table_q} \
         WHERE {qc} IS NOT NULL GROUP BY {qc} ORDER BY n DESC, v ASC",
        len = d.char_length(&text)
    );
    let rows = exec.rows(&d.select_limited(&body, TOP_K))?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let length: usize = cell(&r, 2).and_then(|v| v.parse().ok()).unwrap_or(0);
        if length > LOW_CARDINALITY_MAX_VALUE_LEN {
            return Ok(Vec::new());
        }
        let (Some(value), Some(count)) = (cell(&r, 0), cell(&r, 1).and_then(|v| v.parse().ok()))
        else {
            continue;
        };
        out.push(ValueCount { value, count });
    }
    Ok(out)
}

/// Median and 99th percentile, nearest-rank, as two scalar subqueries in
/// one statement.
fn percentiles(
    exec: &mut dyn Executor,
    d: &dyn Dialect,
    table_q: &str,
    column: &str,
    non_null: u64,
) -> Result<(Option<f64>, Option<f64>), SourceError> {
    let qc = d.quote(column);
    let offset =
        |p: f64| (((non_null as f64 * p).ceil() as u64).max(1) - 1).min(non_null.saturating_sub(1));
    let sql = format!(
        "SELECT {}, {}",
        d.nth_value(table_q, &qc, offset(0.50)),
        d.nth_value(table_q, &qc, offset(0.99))
    );
    let rows = exec.rows(&sql)?;
    let row = rows.into_iter().next().unwrap_or_default();
    let number = |i: usize| -> Option<f64> { cell(&row, i)?.parse().ok() };
    Ok((number(0), number(1)))
}

/// Profile `table` with aggregate SQL: the database counts, this code reads
/// the answers back. `sample` is the bounded first-rows sample the driver
/// already fetched, for the pattern heuristic.
///
/// Statement budget per table: one aggregate statement per 64 columns, one
/// ordered statement per numeric column for its percentiles, and one grouped
/// statement per code-list column. Nothing streams the table into memory.
pub fn profile(
    exec: &mut dyn Executor,
    d: &dyn Dialect,
    table: &str,
    sample: &[Row],
) -> Result<TableProfile, SourceError> {
    let kind = kind_of_table(exec, d, table)?
        .ok_or_else(|| SourceError::Query(format!("no table or view named '{table}'")))?;
    let info = structure(exec, d, table, kind, false)?;
    let unique_keys = unique_keys(exec, d, table)?;
    let q = d.quote(table);

    let mut row_count = 0u64;
    let mut aggregates: Vec<Aggregates> = Vec::with_capacity(info.columns.len());
    for chunk in info.columns.chunks(AGG_COLUMNS_PER_STATEMENT) {
        let mut select = vec!["COUNT(*)".to_string()];
        let mut slots: Vec<Slots> = Vec::with_capacity(chunk.len());
        for c in chunk {
            let qc = d.quote(&c.name);
            let distinct = if d.countable(&c.native_type) {
                format!("COUNT(DISTINCT {qc})")
            } else {
                "NULL".to_string()
            };
            let mut s = Slots {
                distinct: push(&mut select, distinct),
                nulls: push(
                    &mut select,
                    format!("SUM(CASE WHEN {qc} IS NULL THEN 1 ELSE 0 END)"),
                ),
                ..Slots::default()
            };
            if c.generic_type.is_numeric() {
                s.min = Some(push(&mut select, format!("MIN({qc})")));
                s.max = Some(push(&mut select, format!("MAX({qc})")));
                s.mean = Some(push(&mut select, d.avg(&qc)));
            } else if c.generic_type == ValueKind::Text {
                // The length and nothing else: MIN or MAX of a text column
                // would *be* a row value, dressed up as a statistic.
                s.mean_length = Some(push(&mut select, d.avg(&d.char_length(&d.to_text(&qc)))));
            }
            slots.push(s);
        }
        let rows = exec.rows(&format!("SELECT {} FROM {q}", select.join(", ")))?;
        let row = rows.into_iter().next().unwrap_or_default();
        let at = |i: usize| -> Option<String> { cell(&row, i) };
        row_count = at(0).and_then(|v| v.parse().ok()).unwrap_or(0);
        for s in slots {
            aggregates.push(Aggregates {
                distinct: at(s.distinct).and_then(|v| v.parse().ok()),
                // SUM over no rows is NULL: zero NULLs, not an unknown count.
                nulls: at(s.nulls).and_then(|v| v.parse().ok()).or(Some(0)),
                min: s.min.and_then(at).and_then(|v| v.parse().ok()),
                max: s.max.and_then(at).and_then(|v| v.parse().ok()),
                mean: s.mean.and_then(at).and_then(|v| v.parse().ok()),
                mean_length: s.mean_length.and_then(at).and_then(|v| v.parse().ok()),
            });
        }
    }

    let mut columns = Vec::with_capacity(info.columns.len());
    for (i, c) in info.columns.iter().enumerate() {
        let agg = aggregates.get(i).cloned().unwrap_or_default();
        let nulls = agg.nulls.unwrap_or(0);
        let non_null = row_count.saturating_sub(nulls);
        let distinct = agg.distinct;
        let ratio = match (distinct, non_null) {
            (Some(dn), n) if n > 0 => Some(dn as f64 / n as f64),
            _ => None,
        };
        let top = match distinct {
            Some(dn) if is_low_cardinality(dn, non_null) => top_values(exec, d, &q, &c.name)?,
            _ => Vec::new(),
        };
        let numeric = match (agg.min, agg.max, agg.mean) {
            (Some(min), Some(max), Some(mean)) if non_null > 0 => {
                let (p50, p99) = percentiles(exec, d, &q, &c.name, non_null)?;
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
            top_values: top,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::Value;

    #[test]
    fn standard_types_cover_the_three_dialects_spellings() {
        for (native, kind) in [
            ("integer", ValueKind::Integer),
            ("bigint", ValueKind::Integer),
            ("int(11) unsigned", ValueKind::Integer),
            ("tinyint(1)", ValueKind::Integer),
            ("serial", ValueKind::Integer),
            ("year", ValueKind::Integer),
            ("numeric(10,2)", ValueKind::Decimal),
            ("decimal", ValueKind::Decimal),
            ("money", ValueKind::Decimal),
            ("double precision", ValueKind::Float),
            ("float", ValueKind::Float),
            ("real", ValueKind::Float),
            ("boolean", ValueKind::Boolean),
            ("bit", ValueKind::Boolean),
            ("character varying(120)", ValueKind::Text),
            ("nvarchar(max)", ValueKind::Text),
            ("text", ValueKind::Text),
            ("enum('a','b')", ValueKind::Text),
            ("date", ValueKind::Date),
            ("time without time zone", ValueKind::Time),
            ("time", ValueKind::Time),
            ("timestamp with time zone", ValueKind::DateTime),
            ("datetime2(7)", ValueKind::DateTime),
            ("smalldatetime", ValueKind::DateTime),
            ("bytea", ValueKind::Binary),
            ("varbinary(max)", ValueKind::Binary),
            ("longblob", ValueKind::Binary),
            ("uuid", ValueKind::Uuid),
            ("uniqueidentifier", ValueKind::Uuid),
            ("jsonb", ValueKind::Json),
            ("interval", ValueKind::Other),
            ("xml", ValueKind::Other),
            ("geometry", ValueKind::Other),
        ] {
            assert_eq!(standard_type(native), kind, "{native}");
        }
    }

    #[test]
    fn canonical_forms_are_what_xsd_accepts() {
        assert_eq!(canonical(ValueKind::Boolean, "t"), "true");
        assert_eq!(canonical(ValueKind::Boolean, "0"), "false");
        assert_eq!(canonical(ValueKind::Boolean, "maybe"), "maybe");
        assert_eq!(
            canonical(ValueKind::DateTime, "2026-01-01 00:00:00"),
            "2026-01-01T00:00:00"
        );
        assert_eq!(
            canonical(ValueKind::DateTime, "2026-01-01 00:00:00.5+00"),
            "2026-01-01T00:00:00.5+00:00"
        );
        assert_eq!(
            canonical(ValueKind::DateTime, "2026-01-01T00:00:00+02:00"),
            "2026-01-01T00:00:00+02:00"
        );
        assert_eq!(canonical(ValueKind::DateTime, "2026-01-01"), "2026-01-01");
        assert_eq!(canonical(ValueKind::Time, "12:30:00+01"), "12:30:00+01:00");
        assert_eq!(canonical(ValueKind::Time, "12:30:00"), "12:30:00");
        assert_eq!(canonical(ValueKind::Float, "Infinity"), "INF");
        assert_eq!(canonical(ValueKind::Float, "-Infinity"), "-INF");
        assert_eq!(canonical(ValueKind::Float, "1.5"), "1.5");
        assert_eq!(canonical(ValueKind::Text, " x "), " x ");
        // A date is not an offset: `2026-01-01` ends in `-01` and stays.
        assert_eq!(canonical(ValueKind::Date, "2026-01-01"), "2026-01-01");
    }

    /// A dialect in the PostgreSQL spelling, over a scripted executor.
    struct Pg;
    impl Dialect for Pg {
        fn quote(&self, ident: &str) -> String {
            format!("\"{}\"", ident.replace('"', "\"\""))
        }
        fn schema_expr(&self) -> String {
            "current_schema()".into()
        }
        fn select_limited(&self, body: &str, limit: usize) -> String {
            format!("SELECT {body} LIMIT {limit}")
        }
        fn nth_value(&self, table_q: &str, column_q: &str, offset: u64) -> String {
            format!(
                "(SELECT {column_q} FROM {table_q} WHERE {column_q} IS NOT NULL ORDER BY {column_q} LIMIT 1 OFFSET {offset})"
            )
        }
        fn char_length(&self, expr: &str) -> String {
            format!("LENGTH({expr})")
        }
        fn to_text(&self, expr: &str) -> String {
            format!("CAST({expr} AS TEXT)")
        }
        fn columns_sql(&self, table: &str) -> String {
            format!("COLUMNS {table}")
        }
        fn foreign_keys_sql(&self, table: &str) -> String {
            format!("FKS {table}")
        }
        fn unique_indexes_sql(&self, table: &str) -> Option<String> {
            Some(format!("INDEXES {table}"))
        }
        fn row_estimate_sql(&self, table: &str) -> Option<String> {
            Some(format!("ESTIMATE {table}"))
        }
    }

    /// Answers each statement from a script keyed on a prefix, and records
    /// what was asked.
    struct Scripted {
        asked: Vec<String>,
    }

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    impl Executor for Scripted {
        fn rows(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>, SourceError> {
            self.asked.push(sql.to_string());
            Ok(if sql.starts_with("SELECT TABLE_NAME, TABLE_TYPE") {
                vec![
                    vec![s("child"), s("BASE TABLE")],
                    vec![s("child_v"), s("VIEW")],
                ]
            } else if sql.starts_with("SELECT TABLE_TYPE") {
                if sql.contains("'child'") {
                    vec![vec![s("BASE TABLE")]]
                } else {
                    vec![]
                }
            } else if sql.starts_with("SELECT TC.CONSTRAINT_NAME") {
                vec![
                    vec![s("child_pkey"), s("PRIMARY KEY"), s("cid"), s("1")],
                    vec![s("child_name_key"), s("UNIQUE"), s("name"), s("1")],
                    vec![s("child_pair_key"), s("UNIQUE"), s("b"), s("2")],
                    vec![s("child_pair_key"), s("UNIQUE"), s("a"), s("1")],
                ]
            } else if sql.starts_with("COLUMNS") {
                vec![
                    vec![s("cid"), s("integer"), s("NO"), None, None],
                    vec![
                        s("name"),
                        s("character varying(40)"),
                        s("NO"),
                        s("'x'::text"),
                        s("the name"),
                    ],
                    vec![s("amount"), s("numeric(10,2)"), s("YES"), None, s("")],
                    vec![s("a"), s("integer"), s("YES"), None, None],
                    vec![s("b"), s("integer"), s("YES"), None, None],
                ]
            } else if sql.starts_with("FKS") {
                vec![
                    vec![s("child_parent_fkey"), s("a"), s("parent"), s("pa")],
                    vec![s("child_parent_fkey"), s("b"), s("parent"), s("pb")],
                ]
            } else if sql.starts_with("INDEXES") {
                vec![
                    vec![s("child_amount_idx"), s("amount"), s("1")],
                    // The constraint's own index comes back too and is deduplicated.
                    vec![s("child_name_key"), s("name"), s("1")],
                ]
            } else if sql.starts_with("ESTIMATE") {
                vec![vec![s("1234")]]
            } else if sql.starts_with("SELECT COUNT(*)") {
                // COUNT(*), then per column: distinct, nulls[, min, max, avg | mean_length]
                vec![vec![
                    s("4"),
                    s("4"),
                    s("0"),
                    s("1"),
                    s("4"),
                    s("2.5"), // cid
                    s("2"),
                    s("0"),
                    s("5.5"), // name (text: mean length)
                    s("3"),
                    s("1"),
                    s("1.50"),
                    s("9.00"),
                    s("4.5"), // amount
                    s("1"),
                    s("2"),
                    s("7"),
                    s("7"),
                    s("7"), // a
                    s("2"),
                    s("0"),
                    s("1"),
                    s("2"),
                    s("1.5"), // b
                ]]
            } else if sql.starts_with("SELECT (SELECT") {
                vec![vec![s("2"), s("4")]]
            } else if sql.starts_with("SELECT SUBSTRING") {
                vec![vec![s("x"), s("3"), s("1")], vec![s("y"), s("1"), s("1")]]
            } else if sql.starts_with("SELECT MAX(") {
                vec![vec![s("42")]]
            } else {
                panic!("unscripted statement: {sql}")
            })
        }
    }

    #[test]
    fn the_catalogue_is_read_through_information_schema() {
        let mut exec = Scripted { asked: Vec::new() };
        let tables = introspect(&mut exec, &Pg).unwrap();
        assert_eq!(tables.len(), 2);
        let child = &tables[0];
        assert_eq!(child.name, "child");
        assert_eq!(child.kind, TableKind::Table);
        assert_eq!(tables[1].kind, TableKind::View);
        assert_eq!(child.primary_key, vec!["cid"]);
        assert_eq!(child.row_estimate, Some(1234));
        let name = child.columns.iter().find(|c| c.name == "name").unwrap();
        assert_eq!(name.generic_type, ValueKind::Text);
        assert!(!name.nullable);
        assert_eq!(name.default.as_deref(), Some("'x'::text"));
        assert_eq!(name.comment.as_deref(), Some("the name"));
        let amount = child.columns.iter().find(|c| c.name == "amount").unwrap();
        assert_eq!(amount.generic_type, ValueKind::Decimal);
        assert!(amount.nullable);
        assert_eq!(amount.comment, None, "an empty comment is no comment");
        assert!(child.columns[0].primary_key && !amount.primary_key);
        assert_eq!(child.foreign_keys.len(), 1);
        assert_eq!(child.foreign_keys[0].columns, vec!["a", "b"]);
        assert_eq!(child.foreign_keys[0].ref_table, "parent");
        assert_eq!(child.foreign_keys[0].ref_columns, vec!["pa", "pb"]);
        // Every statement went through the executor with the schema filter.
        assert!(exec.asked[0].contains("TABLE_SCHEMA = current_schema()"));
        assert!(exec.asked.iter().any(|q| q.contains("'child'")));
    }

    #[test]
    fn unique_keys_are_the_primary_key_then_constraints_then_indexes_in_column_order() {
        let mut exec = Scripted { asked: Vec::new() };
        let keys = unique_keys(&mut exec, &Pg, "child").unwrap();
        assert_eq!(
            keys,
            vec![
                vec!["cid".to_string()],
                vec!["name".to_string()],
                vec!["a".to_string(), "b".to_string()],
                vec!["amount".to_string()],
            ]
        );
    }

    #[test]
    fn the_profiler_reads_the_databases_aggregates_back() {
        let mut exec = Scripted { asked: Vec::new() };
        let sample: Vec<Row> = (0..10)
            .map(|i| Row::from([("name".to_string(), Value::text(format!("a{i}@b.example")))]))
            .collect();
        let p = profile(&mut exec, &Pg, "child", &sample).unwrap();
        assert_eq!(p.row_count, Some(4));
        assert_eq!(p.sampled_rows, 10);
        assert_eq!(p.primary_key, vec!["cid"]);
        assert_eq!(p.unique_keys[0], vec!["cid"]);
        let cid = &p.columns[0];
        assert_eq!(cid.distinct_count, Some(4));
        assert_eq!(cid.null_count, Some(0));
        assert_eq!(cid.cardinality_ratio, Some(1.0));
        let n = cid.numeric.unwrap();
        assert_eq!(
            (n.min, n.max, n.mean, n.p50, n.p99),
            (1.0, 4.0, 2.5, 2.0, 4.0)
        );
        assert!(cid.top_values.is_empty(), "a key is not a code list");
        let name = &p.columns[1];
        assert_eq!(name.mean_length, Some(5.5));
        assert_eq!(name.numeric, None);
        assert_eq!(name.pattern, Some(DetectedPattern::Email));
        // Two distinct values over four rows: a code list, most frequent first.
        assert_eq!(name.top_values.len(), 2);
        assert_eq!(name.top_values[0].value, "x");
        assert_eq!(name.top_values[0].count, 3);
        let amount = &p.columns[2];
        assert_eq!(amount.null_count, Some(1));
        assert_eq!(amount.cardinality_ratio, Some(1.0));
        // The statements the profiler sent: aggregates, percentiles, top values.
        let agg = exec
            .asked
            .iter()
            .find(|q| q.starts_with("SELECT COUNT(*)"))
            .unwrap();
        assert!(agg.contains("COUNT(DISTINCT \"cid\")"), "{agg}");
        assert!(agg.contains("AVG(LENGTH(CAST(\"name\" AS TEXT)))"), "{agg}");
        assert!(!agg.contains("MIN(\"name\")"), "no MIN over text: {agg}");
        let top = exec
            .asked
            .iter()
            .find(|q| q.starts_with("SELECT SUBSTRING"))
            .unwrap();
        assert!(top.ends_with("LIMIT 50"), "{top}");
        assert!(
            top.contains(&format!("1, {}", LOW_CARDINALITY_MAX_VALUE_LEN + 1)),
            "{top}"
        );
        assert!(matches!(
            profile(&mut exec, &Pg, "nope", &[]),
            Err(SourceError::Query(_))
        ));
    }

    #[test]
    fn sample_and_watermark_sql_are_quoted() {
        assert_eq!(
            sample_sql(&Pg, "a\"b", 5),
            "SELECT * FROM \"a\"\"b\" LIMIT 5"
        );
        assert_eq!(sample_sql(&Pg, "t", 0), "SELECT * FROM \"t\" LIMIT 1");
        let mut exec = Scripted { asked: Vec::new() };
        assert_eq!(
            max_watermark(&mut exec, &Pg, "t", "c").unwrap().as_deref(),
            Some("42")
        );
        assert_eq!(exec.asked[0], "SELECT MAX(\"c\") FROM \"t\"");
    }
}
