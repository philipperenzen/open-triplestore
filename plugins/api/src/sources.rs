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

pub mod catalogue;

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

impl ValueKind {
    /// Whether a column of this type holds text whose *shape* is worth
    /// sampling. A typed column already tells the reader what it is;
    /// pattern detection exists for the text column that does not.
    pub fn is_stringish(self) -> bool {
        matches!(
            self,
            ValueKind::Text | ValueKind::Uuid | ValueKind::Json | ValueKind::Other
        )
    }

    /// Whether the dialect reports this column as a number, and a numeric
    /// summary is therefore about magnitudes rather than about text.
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            ValueKind::Integer | ValueKind::Decimal | ValueKind::Float
        )
    }
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

// ─────────────────────────────── Profiling ───────────────────────────────

/// Rows the pattern heuristic looks at per table. One bounded statement, so
/// profiling a production replica costs a `LIMIT`, not a table scan.
pub const PROFILE_SAMPLE_ROWS: usize = 200;

/// Most distinct values a column may hold and still be treated as a code
/// list.
///
/// Fifty is the practical ceiling of a value map a human maintains: an
/// enumeration is spelled out once per value in the mapping (`fn:mapping`,
/// one entry each), and past a few dozen entries nobody is writing them by
/// hand — the column is data, and listing it would export row content.
pub const LOW_CARDINALITY_MAX_DISTINCT: u64 = 50;

/// Longest value, in characters, that may be reported as a code.
///
/// A code is a token a human maintains in a value map: it is typed into a
/// mapping entry, read in a proposal and recognised on sight. Nothing longer
/// is that, whatever its cardinality — five distinct document bodies repeated
/// across a thousand rows count as low-cardinality and are still row content,
/// and writing them into the profile graph would hand whole documents to the
/// offline proposer that reads it.
pub const LOW_CARDINALITY_MAX_VALUE_LEN: usize = 128;

/// A code list repeats. Distinct values must be at most half the non-NULL
/// values, or the column is an identifier or free text wearing a short type:
/// a value that never repeats is not a code, and emitting one would be
/// emitting a row.
pub const LOW_CARDINALITY_MAX_RATIO: f64 = 0.5;

/// Top values reported for a low-cardinality column. Equal to
/// [`LOW_CARDINALITY_MAX_DISTINCT`] on purpose: within the ceiling the top-k
/// is the *complete* value set, so a proposer that turns it into an
/// enumeration cannot silently drop a value it never saw. Ordering by
/// frequency is what makes it a top-k — it also tells the reader the skew.
pub const TOP_K: usize = LOW_CARDINALITY_MAX_DISTINCT as usize;

/// A pattern is reported only when this fraction of the sampled non-NULL
/// values match it.
pub const PATTERN_MIN_CONFIDENCE: f64 = 0.9;

/// …and only when the sample held at least this many non-NULL values. Three
/// values agreeing is a coincidence, not a shape.
pub const PATTERN_MIN_SAMPLE: usize = 8;

/// Whether a column's value set is a code list: few distinct values, and
/// values that repeat. Both drivers ask this one function, so two dialects
/// cannot disagree on what a code list is.
pub fn is_low_cardinality(distinct: u64, non_null: u64) -> bool {
    if distinct == 0 || non_null == 0 || distinct > LOW_CARDINALITY_MAX_DISTINCT {
        return false;
    }
    (distinct as f64) / (non_null as f64) <= LOW_CARDINALITY_MAX_RATIO
}

/// The shape a column's values shared in the sample the profiler looked at.
///
/// **A detection is a claim about lexical shape, nothing more.** It says the
/// sampled values looked like this; it does not say the column *is* this, that
/// every value matches, or that the values are valid — a `Phone` is a run of
/// digits with separators, not a dialable number, and a `Code` is any short
/// token, which is the weakest signal here. Use it to propose a mapping for a
/// human to approve, never to skip validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DetectedPattern {
    Email,
    Iri,
    Uuid,
    Date,
    Code,
    Phone,
}

impl DetectedPattern {
    pub fn as_str(self) -> &'static str {
        match self {
            DetectedPattern::Email => "email",
            DetectedPattern::Iri => "iri",
            DetectedPattern::Uuid => "uuid",
            DetectedPattern::Date => "date",
            DetectedPattern::Code => "code",
            DetectedPattern::Phone => "phone",
        }
    }

    /// Classify one value. Most specific shape first, so a UUID is not
    /// reported as a code and an `http://…` is not reported as an e-mail.
    pub fn detect(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.is_empty() {
            return None;
        }
        if is_uuid(v) {
            return Some(DetectedPattern::Uuid);
        }
        if is_iri(v) {
            return Some(DetectedPattern::Iri);
        }
        if is_email(v) {
            return Some(DetectedPattern::Email);
        }
        if is_date(v) {
            return Some(DetectedPattern::Date);
        }
        if is_phone(v) {
            return Some(DetectedPattern::Phone);
        }
        if is_code(v) {
            return Some(DetectedPattern::Code);
        }
        None
    }

    /// The pattern the sample agrees on, with the fraction that matched.
    ///
    /// `None` unless the sample is big enough ([`PATTERN_MIN_SAMPLE`]) and the
    /// winner covers [`PATTERN_MIN_CONFIDENCE`] of it: a column where a third
    /// of the values look like dates has no shape worth reporting.
    pub fn of_sample<'a>(values: impl Iterator<Item = &'a str>) -> Option<(Self, f64)> {
        let mut counts: BTreeMap<&'static str, (Self, usize)> = BTreeMap::new();
        let mut total = 0usize;
        for v in values {
            total += 1;
            if let Some(p) = Self::detect(v) {
                counts.entry(p.as_str()).or_insert((p, 0)).1 += 1;
            }
        }
        if total < PATTERN_MIN_SAMPLE {
            return None;
        }
        let (pattern, hits) = counts.into_values().max_by_key(|(_, n)| *n)?;
        let confidence = hits as f64 / total as f64;
        (confidence >= PATTERN_MIN_CONFIDENCE).then_some((pattern, confidence))
    }
}

fn is_uuid(v: &str) -> bool {
    v.len() == 36
        && v.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// An absolute IRI: a scheme, then something. A one- or two-letter "scheme"
/// with no authority is refused, so `12:30` and `a:b` are not IRIs.
fn is_iri(v: &str) -> bool {
    if v.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((scheme, rest)) = v.split_once(':') else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    let mut chars = scheme.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return false;
    }
    rest.starts_with("//") || scheme.len() >= 3
}

fn is_email(v: &str) -> bool {
    if v.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = v.split_once('@') else {
        return false;
    };
    if local.is_empty() || domain.contains('@') {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty())
        && labels
            .last()
            .is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
}

/// ISO-8601 `YYYY-MM-DD`, alone or as the date half of a timestamp. Other
/// orderings (`DD/MM/YYYY`) are deliberately not recognised: they are
/// ambiguous, and a wrong guess about day and month is worse than no guess.
fn is_date(v: &str) -> bool {
    let b = v.as_bytes();
    if b.len() < 10 {
        return false;
    }
    let shaped = b[..10].iter().enumerate().all(|(i, c)| match i {
        4 | 7 => *c == b'-',
        _ => c.is_ascii_digit(),
    });
    shaped && (b.len() == 10 || matches!(b[10], b'T' | b't' | b' '))
}

/// A *formatted* run of 7–15 digits. A bare digit run is not accepted: it is
/// indistinguishable from an identifier, and calling it a phone number would
/// be inventing a fact about the column. `/` is not a separator either,
/// because `17/09/2026` is a date under one convention and `09/17/2026` under
/// another, and neither is a telephone number.
fn is_phone(v: &str) -> bool {
    let body = v.strip_prefix('+').unwrap_or(v);
    let mut digits = 0usize;
    let mut separators = 0usize;
    for c in body.chars() {
        if c.is_ascii_digit() {
            digits += 1;
        } else if matches!(c, ' ' | '-' | '(' | ')' | '.') {
            separators += 1;
        } else {
            return false;
        }
    }
    (7..=15).contains(&digits) && (v.starts_with('+') || separators > 0)
}

/// A short identifier-shaped token. The weakest of the six, and the reason
/// [`DetectedPattern`]'s docs insist a detection is about shape only.
fn is_code(v: &str) -> bool {
    v.len() <= 32
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && v.chars().any(|c| c.is_ascii_alphabetic())
}

/// One value of a code list, with how many rows carry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueCount {
    pub value: String,
    pub count: u64,
}

/// Summary statistics for a numeric column.
///
/// Only ever computed for a numeric column. The same numbers over free text
/// would summarise the text itself — `MIN` of a text column *is* a row value —
/// which is exactly what must not leave a deployment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NumericSummary {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub p50: f64,
    pub p99: f64,
}

/// What profiling learned about one column.
///
/// Everything here is metadata or an aggregate. The one exception is
/// [`top_values`](Self::top_values), which carries literal values and is
/// emitted only for a code-list column ([`is_low_cardinality`]) — a code list
/// with its values withheld would be useless to the reader it exists for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnProfile {
    pub name: String,
    /// 1-based position in the table.
    pub position: u32,
    pub native_type: String,
    pub generic_type: ValueKind,
    pub nullable: bool,
    pub primary_key: bool,
    /// `None` where the connector could not aggregate — never a sample count
    /// dressed up as a table-wide one.
    pub distinct_count: Option<u64>,
    pub null_count: Option<u64>,
    /// `distinct_count / non-NULL rows`: 1.0 is a key, near 0 is a code list.
    pub cardinality_ratio: Option<f64>,
    /// Empty unless the column is a code list.
    pub top_values: Vec<ValueCount>,
    pub numeric: Option<NumericSummary>,
    /// Average character length, for a text column.
    pub mean_length: Option<f64>,
    pub pattern: Option<DetectedPattern>,
    /// Fraction of the sampled non-NULL values that matched `pattern`.
    pub pattern_confidence: Option<f64>,
}

/// What profiling learned about one table.
///
/// **Provenance of the numbers.** Counts, null counts, the numeric summaries
/// and the top-k are computed by the database, over the whole table. The
/// pattern detections are not: they come from the first
/// [`PROFILE_SAMPLE_ROWS`] rows the table hands back, in whatever order it
/// stores them, which is a sample and not a random one. Read
/// [`sampled_rows`](Self::sampled_rows) before trusting a detection, and read
/// [`DetectedPattern`] before reading anything into one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableProfile {
    pub table: String,
    pub kind: TableKind,
    /// Exact row count where the connector counted; `None` where it only had
    /// an estimate it will not pass off as a count.
    pub row_count: Option<u64>,
    pub columns: Vec<ColumnProfile>,
    pub primary_key: Vec<String>,
    /// Column sets that are unique, primary key first — what a join planner
    /// may push down on.
    pub unique_keys: Vec<Vec<String>>,
    pub foreign_keys: Vec<ForeignKey>,
    /// Rows the pattern heuristic actually looked at.
    pub sampled_rows: u64,
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

    /// What the catalogue lists, by name only.
    ///
    /// Narrowing a profiling run needs the names and nothing else. The default
    /// derives them from [`Self::introspect`]; a connector whose introspection
    /// costs anything per table — row estimates, per-table catalogue queries —
    /// overrides it with the listing query it already has, so excluding a
    /// table means not reading it rather than reading it and dropping it.
    fn table_names(&mut self) -> Result<Vec<String>, SourceError> {
        Ok(self.introspect()?.into_iter().map(|t| t.name).collect())
    }

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

    /// Profile `table`: its structure, plus aggregates over its values.
    ///
    /// The host writes the result into a per-source profile graph that an
    /// offline mapping proposer reads. The proposer never sees the database,
    /// so this is the only thing standing between it and a blind guess — and
    /// equally, everything emitted here has left the database, so a connector
    /// emits metadata and aggregates and nothing else.
    ///
    /// This default derives what it can from the catalogue and one bounded
    /// sample: structure exactly, patterns from the sample, and *no counts at
    /// all*. Reporting a sample's null count as the table's would be a number
    /// that reads as exact and is not. A connector that can aggregate
    /// overrides this and lets the database do the counting — see the SQLite
    /// driver, which computes one aggregate statement per table.
    fn profile(&mut self, table: &str) -> Result<TableProfile, SourceError> {
        let tables = self.introspect()?;
        let info = tables
            .iter()
            .find(|t| t.name == table)
            .ok_or_else(|| SourceError::Query(format!("no table or view named '{table}'")))?;
        let unique_keys = self.unique_keys(table).unwrap_or_default();
        let sample = self.sample(table, PROFILE_SAMPLE_ROWS)?;

        let columns = info
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let pattern = c.generic_type.is_stringish().then(|| {
                    DetectedPattern::of_sample(
                        sample
                            .iter()
                            .filter_map(|row| row.get(&c.name))
                            .map(|v| v.lexical.as_str()),
                    )
                });
                let pattern = pattern.flatten();
                ColumnProfile {
                    name: c.name.clone(),
                    position: i as u32 + 1,
                    native_type: c.native_type.clone(),
                    generic_type: c.generic_type,
                    nullable: c.nullable,
                    primary_key: c.primary_key,
                    distinct_count: None,
                    null_count: None,
                    cardinality_ratio: None,
                    top_values: Vec::new(),
                    numeric: None,
                    mean_length: None,
                    pattern: pattern.map(|(p, _)| p),
                    pattern_confidence: pattern.map(|(_, c)| c),
                }
            })
            .collect();

        Ok(TableProfile {
            table: info.name.clone(),
            kind: info.kind,
            // An estimate is not a count, and the profile graph says
            // `void:entities`, which is a count.
            row_count: None,
            columns,
            primary_key: info.primary_key.clone(),
            unique_keys,
            foreign_keys: info.foreign_keys.clone(),
            sampled_rows: sample.len() as u64,
        })
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

    /// A connector with a catalogue and rows, and no aggregate SQL at all —
    /// the case the default [`SourceConnection::profile`] exists for.
    struct Catalogue;

    impl SourceConnection for Catalogue {
        fn introspect(&mut self) -> Result<Vec<TableInfo>, SourceError> {
            Ok(vec![TableInfo {
                schema: None,
                name: "t".into(),
                kind: TableKind::Table,
                columns: vec![
                    ColumnInfo {
                        name: "k".into(),
                        native_type: "INTEGER".into(),
                        generic_type: ValueKind::Integer,
                        nullable: false,
                        default: None,
                        comment: None,
                        primary_key: true,
                    },
                    ColumnInfo {
                        name: "c".into(),
                        native_type: "TEXT".into(),
                        generic_type: ValueKind::Text,
                        nullable: true,
                        default: None,
                        comment: None,
                        primary_key: false,
                    },
                ],
                primary_key: vec!["k".into()],
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
                row_estimate: Some(9_999),
                comment: None,
            }])
        }
        fn sample(&mut self, _table: &str, _limit: usize) -> Result<Vec<Row>, SourceError> {
            Ok((0..10)
                .map(|i| {
                    Row::from([
                        (
                            "k".to_string(),
                            Value::new(i.to_string(), ValueKind::Integer),
                        ),
                        ("c".to_string(), Value::text(format!("a{i}@b.example"))),
                    ])
                })
                .collect())
        }
        fn stream(&mut self, _: &str, _: usize, _: BatchSink<'_>) -> Result<u64, SourceError> {
            Err(SourceError::Unsupported("stream".into()))
        }
        fn max_watermark(&mut self, _: &str, _: &str) -> Result<Option<String>, SourceError> {
            Ok(None)
        }
    }

    #[test]
    fn the_default_profile_derives_structure_and_refuses_to_invent_counts() {
        let mut c = Catalogue;
        let p = c.profile("t").unwrap();
        assert_eq!(p.primary_key, vec!["k".to_string()]);
        assert_eq!(p.unique_keys, vec![vec!["k".to_string()]]);
        assert_eq!(p.sampled_rows, 10);
        // A row *estimate* must not come back as a row count.
        assert_eq!(p.row_count, None);
        let text = p.columns.iter().find(|c| c.name == "c").unwrap();
        assert_eq!(text.position, 2);
        assert!(text.nullable && !text.primary_key);
        assert_eq!(text.pattern, Some(DetectedPattern::Email));
        assert_eq!(text.distinct_count, None, "no aggregate, no count");
        assert_eq!(text.null_count, None);
        assert!(text.top_values.is_empty() && text.numeric.is_none());
        // A typed column is not sampled for a shape it already declares.
        assert_eq!(p.columns[0].pattern, None);
        assert!(matches!(c.profile("nope"), Err(SourceError::Query(_))));
    }

    #[test]
    fn the_default_table_names_are_the_catalogue_names() {
        assert_eq!(Catalogue.table_names().unwrap(), vec!["t".to_string()]);
    }

    #[test]
    fn a_pattern_needs_a_big_enough_sample_and_a_clear_majority() {
        let emails = ["a@b.example"; PATTERN_MIN_SAMPLE];
        assert_eq!(
            DetectedPattern::of_sample(emails.iter().copied()).map(|(p, _)| p),
            Some(DetectedPattern::Email)
        );
        // One short of the minimum sample is a coincidence, not a shape.
        assert_eq!(
            DetectedPattern::of_sample(emails[1..].iter().copied()),
            None
        );
        // A column that is half e-mail addresses has no shape worth reporting.
        let mixed: Vec<&str> = emails
            .iter()
            .copied()
            .enumerate()
            .map(|(i, e)| if i % 2 == 0 { e } else { "the quick brown" })
            .collect();
        assert_eq!(DetectedPattern::of_sample(mixed.into_iter()), None);
    }

    #[test]
    fn a_code_list_is_few_values_that_repeat() {
        assert!(is_low_cardinality(3, 100));
        assert!(is_low_cardinality(LOW_CARDINALITY_MAX_DISTINCT, 1_000));
        // Too many distinct values to be a hand-maintained value map.
        assert!(!is_low_cardinality(LOW_CARDINALITY_MAX_DISTINCT + 1, 1_000));
        // Values that never repeat are identifiers, not codes — emitting them
        // would be emitting the rows.
        assert!(!is_low_cardinality(10, 10));
        assert!(!is_low_cardinality(6, 10));
        assert!(is_low_cardinality(5, 10));
        assert!(!is_low_cardinality(0, 10) && !is_low_cardinality(3, 0));
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
