//! `ots-plugin-mssql` — the SQL Server datasource connector.
//!
//! The contract every connector keeps, in this dialect's terms:
//!
//! * **Read-only, checked at connect.** SQL Server has no session-level
//!   read-only mode, so the account is examined instead: one that is a
//!   `sysadmin`, or a member of `db_owner`, `db_datawriter` or
//!   `db_ddladmin`, is refused with a configuration error. A datasource
//!   holds a reader's credential or none.
//! * **A statement timeout on everything.** The driver applies the budget
//!   to every statement and to every batch it waits for while streaming;
//!   `SET LOCK_TIMEOUT` keeps a blocked read from waiting past it.
//! * **Streaming.** Rows come off the wire one at a time and are delivered
//!   in batches. Every column is converted to text by the server (ISO 8601
//!   for dates, full precision for floats, hex for binary) and typed from
//!   the result's metadata, so the wire carries lexical forms.
//! * **TLS through rustls** with the platform roots and, when a datasource
//!   names one in `options.sslrootcert`, a private CA — never a trust-all.
//!
//! The driver is asynchronous; each connection owns a small runtime and
//! blocks on it, which is what the host expects of a connector (it opens
//! connections off its own runtime, on a blocking thread).

use std::sync::Arc;
use std::time::Duration;

use futures_util::TryStreamExt;
use ots_plugin_api::sources::catalogue::{self, canonical, Dialect, Executor};
use ots_plugin_api::sources::{
    BatchSink, ConnectParams, Row, SourceConnection, SourceConnector, SourceError, TableInfo,
    TableProfile, Value, ValueKind, PROFILE_SAMPLE_ROWS,
};
use ots_plugin_api::Plugin;
use tiberius::{AuthMethod, Client, ColumnData, ColumnType, Config, EncryptionLevel, QueryItem};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

/// The plugin: no routes, one connector.
#[derive(Default)]
pub struct MssqlPlugin;

impl Plugin for MssqlPlugin {
    fn name(&self) -> &'static str {
        "mssql"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn connectors(&self) -> Vec<Arc<dyn SourceConnector>> {
        vec![Arc::new(MssqlConnector)]
    }
}

pub struct MssqlConnector;

/// The dialect token a datasource names.
pub const DIALECT: &str = "mssql";

/// Roles that can write. Membership in any of them makes the account
/// unfit for a read-only datasource.
const WRITING_ROLES: [&str; 3] = ["db_owner", "db_datawriter", "db_ddladmin"];

type TdsClient = Client<Compat<TcpStream>>;

impl SourceConnector for MssqlConnector {
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
            .ok_or_else(|| SourceError::Config("a mssql datasource needs a host".to_string()))?;
        if params.database.trim().is_empty() {
            return Err(SourceError::Config(
                "a mssql datasource needs a database name".to_string(),
            ));
        }
        let user = params
            .username
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| {
                SourceError::Config("a mssql datasource needs a username".to_string())
            })?;
        let mut config = Config::new();
        config.host(host);
        config.port(params.port.unwrap_or(1433));
        config.database(params.database.trim());
        config.application_name("open-triplestore");
        config.authentication(AuthMethod::sql_server(
            user,
            params.password.as_ref().map(|p| p.expose()).unwrap_or(""),
        ));
        // Read-only intent routes to a readable secondary where one exists;
        // it does not make a primary refuse writes — the role check does.
        config.readonly(true);
        if params.tls {
            config.encryption(EncryptionLevel::Required);
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
                config.trust_cert_ca(path);
            }
        } else {
            config.encryption(EncryptionLevel::NotSupported);
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| SourceError::Connect(format!("runtime: {e}")))?;
        let timeout = Duration::from_millis(params.statement_timeout_ms.max(1));
        let addr = config.get_addr();
        let client = runtime.block_on(async {
            let tcp = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(&addr))
                .await
                .map_err(|_| SourceError::Connect("connect timed out".to_string()))?
                .map_err(|e| SourceError::Connect(e.to_string()))?;
            tcp.set_nodelay(true)
                .map_err(|e| SourceError::Connect(e.to_string()))?;
            Client::connect(config, tcp.compat_write())
                .await
                .map_err(|e| SourceError::Connect(describe(&e)))
        })?;
        let mut conn = MssqlConnection {
            runtime,
            client,
            timeout,
            broken: false,
        };
        conn.session_setup()?;
        Ok(Box::new(conn))
    }

    fn quote_identifier(&self, ident: &str) -> String {
        MssqlDialect.quote(ident)
    }
}

/// A driver error as a message: the server's own text for a server error
/// (it never carries the password), the driver's summary otherwise. The
/// host scrubs host, database and account on top.
fn describe(e: &tiberius::error::Error) -> String {
    match e {
        tiberius::error::Error::Server(token) => format!("{} ({})", token.message(), token.code()),
        other => other.to_string(),
    }
}

struct MssqlConnection {
    runtime: tokio::runtime::Runtime,
    client: TdsClient,
    timeout: Duration,
    /// A statement timed out mid-stream: the connection's state is unknown
    /// and it is not used again.
    broken: bool,
}

/// One statement's rows as text cells, run under the budget.
fn text_rows(rows: Vec<tiberius::Row>) -> Vec<Vec<Option<String>>> {
    rows.into_iter()
        .map(|row| row.cells().map(|(_, data)| cell_text(data)).collect())
        .collect()
}

/// A typed cell as text. Dates and times are never read this way — data
/// streams converted to text by the server — so only the scalar kinds the
/// catalogue answers with are spelled here.
fn cell_text(data: &ColumnData<'_>) -> Option<String> {
    match data {
        ColumnData::U8(v) => v.map(|x| x.to_string()),
        ColumnData::I16(v) => v.map(|x| x.to_string()),
        ColumnData::I32(v) => v.map(|x| x.to_string()),
        ColumnData::I64(v) => v.map(|x| x.to_string()),
        ColumnData::F32(v) => v.map(|x| x.to_string()),
        ColumnData::F64(v) => v.map(|x| x.to_string()),
        ColumnData::Bit(v) => v.map(|b| if b { "1" } else { "0" }.to_string()),
        ColumnData::String(v) => v.as_ref().map(|s| s.to_string()),
        ColumnData::Guid(v) => v.map(|g| g.to_string().to_ascii_uppercase()),
        ColumnData::Binary(v) => v.as_ref().map(|b| hex_upper(b)),
        ColumnData::Numeric(v) => v.map(|n| n.to_string()),
        _ => None,
    }
}

fn hex_upper(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02X}"));
    }
    out
}

/// The generic type of a result column.
fn kind_of(t: ColumnType) -> ValueKind {
    use ColumnType::*;
    match t {
        Bit | Bitn => ValueKind::Boolean,
        Int1 | Int2 | Int4 | Int8 | Intn => ValueKind::Integer,
        Float4 | Float8 | Floatn => ValueKind::Float,
        Money | Money4 | Decimaln | Numericn => ValueKind::Decimal,
        Datetime | Datetime4 | Datetimen | Datetime2 | DatetimeOffsetn => ValueKind::DateTime,
        Daten => ValueKind::Date,
        Timen => ValueKind::Time,
        Guid => ValueKind::Uuid,
        BigVarBin | BigBinary | Image => ValueKind::Binary,
        BigVarChar | BigChar | NVarchar | NChar | Text | NText => ValueKind::Text,
        _ => ValueKind::Other,
    }
}

/// The `CONVERT` that turns a column of type `t` into the text the natural
/// mapping wants: ISO 8601 (style 126) for dates and times, full precision
/// (style 3) for floats, hex without a prefix (style 2) for binary.
fn convert_expr(t: ColumnType, column_q: &str) -> String {
    use ColumnType::*;
    let style = match t {
        Datetime | Datetime4 | Datetimen | Datetime2 | DatetimeOffsetn | Daten | Timen => Some(126),
        Float4 | Float8 | Floatn => Some(3),
        BigVarBin | BigBinary | Image => Some(2),
        _ => None,
    };
    match style {
        Some(s) => format!("CONVERT(NVARCHAR(MAX), {column_q}, {s})"),
        None => format!("CONVERT(NVARCHAR(MAX), {column_q})"),
    }
}

impl MssqlConnection {
    fn session_setup(&mut self) -> Result<(), SourceError> {
        let lock_timeout = format!("SET LOCK_TIMEOUT {}", self.timeout.as_millis());
        self.rows(&lock_timeout)
            .map_err(|e| SourceError::Connect(e.to_string()))?;
        // The account must not be able to write. Each answer is 1, 0 or
        // NULL; only a 1 is a refusal, and unknown (NULL) is not proof of
        // anything either way — a principal that cannot even ask is one
        // the check cannot vouch for.
        let question = format!(
            "SELECT IS_SRVROLEMEMBER('sysadmin'), {}",
            WRITING_ROLES
                .iter()
                .map(|r| format!("IS_MEMBER('{r}')"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let answer = self
            .rows(&question)
            .map_err(|e| SourceError::Connect(e.to_string()))?;
        let row = answer.into_iter().next().unwrap_or_default();
        let names = [
            "sysadmin",
            WRITING_ROLES[0],
            WRITING_ROLES[1],
            WRITING_ROLES[2],
        ];
        for (i, name) in names.iter().enumerate() {
            if row.get(i).cloned().flatten().as_deref() == Some("1") {
                return Err(SourceError::Config(format!(
                    "the account can write to this database (member of {name}); register a \
                     read-only account"
                )));
            }
        }
        Ok(())
    }

    fn error(&mut self, e: tiberius::error::Error) -> SourceError {
        SourceError::Query(describe(&e))
    }

    fn timed_out(&mut self) -> SourceError {
        self.broken = true;
        SourceError::Timeout
    }

    fn check_usable(&self) -> Result<(), SourceError> {
        if self.broken {
            return Err(SourceError::Connect(
                "the connection timed out mid-statement and is no longer usable".to_string(),
            ));
        }
        Ok(())
    }

    /// The names and types the server describes a query with, from an
    /// empty `TOP 0` projection over it.
    fn describe_query(&mut self, query: &str) -> Result<Vec<(String, ColumnType)>, SourceError> {
        let probe = format!("SELECT TOP 0 * FROM ({query}) AS ots_q");
        let timeout = self.timeout;
        let outcome = self.runtime.block_on(async {
            tokio::time::timeout(timeout, async {
                let mut stream = self.client.simple_query(&probe).await?;
                let mut columns = Vec::new();
                while let Some(item) = stream.try_next().await? {
                    if let QueryItem::Metadata(meta) = item {
                        columns = meta
                            .columns()
                            .iter()
                            .map(|c| (c.name().to_string(), c.column_type()))
                            .collect();
                    }
                }
                Ok::<_, tiberius::error::Error>(columns)
            })
            .await
        });
        match outcome {
            Ok(Ok(columns)) => Ok(columns),
            Ok(Err(e)) => Err(self.error(e)),
            Err(_) => Err(self.timed_out()),
        }
    }
}

impl Executor for MssqlConnection {
    fn rows(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>, SourceError> {
        self.check_usable()?;
        let timeout = self.timeout;
        let outcome = self.runtime.block_on(async {
            tokio::time::timeout(timeout, async {
                let stream = self.client.simple_query(sql).await?;
                stream.into_first_result().await
            })
            .await
        });
        match outcome {
            Ok(Ok(rows)) => Ok(text_rows(rows)),
            Ok(Err(e)) => Err(self.error(e)),
            Err(_) => Err(self.timed_out()),
        }
    }
}

impl SourceConnection for MssqlConnection {
    fn introspect(&mut self) -> Result<Vec<TableInfo>, SourceError> {
        catalogue::introspect(self, &MssqlDialect)
    }

    fn table_names(&mut self) -> Result<Vec<String>, SourceError> {
        Ok(catalogue::listing(self, &MssqlDialect)?
            .into_iter()
            .map(|(name, _)| name)
            .collect())
    }

    fn sample(&mut self, table: &str, limit: usize) -> Result<Vec<Row>, SourceError> {
        let sql = catalogue::sample_sql(&MssqlDialect, table, limit);
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
        self.check_usable()?;
        let batch_size = batch_size.max(1);
        let described = self.describe_query(query)?;
        let names: Vec<String> = described.iter().map(|(n, _)| n.clone()).collect();
        let kinds: Vec<ValueKind> = described.iter().map(|(_, t)| kind_of(*t)).collect();
        let converts: Vec<String> = described
            .iter()
            .enumerate()
            .map(|(i, (name, t))| {
                format!("{} AS [c{i}]", convert_expr(*t, &MssqlDialect.quote(name)))
            })
            .collect();
        let wrapped = format!("SELECT {} FROM ({query}) AS ots_q", converts.join(", "));

        let timeout = self.timeout;
        let width = names.len();
        let mut delivered = 0u64;
        let mut timed_out = false;
        let outcome: Result<(), SourceError> = self.runtime.block_on(async {
            let mut stream =
                match tokio::time::timeout(timeout, self.client.simple_query(&wrapped)).await {
                    Ok(Ok(s)) => s,
                    Ok(Err(e)) => return Err(SourceError::Query(describe(&e))),
                    Err(_) => {
                        timed_out = true;
                        return Err(SourceError::Timeout);
                    }
                };
            let mut batch: Vec<Row> = Vec::with_capacity(batch_size);
            loop {
                // The budget bounds the wait for each next row, not the
                // whole stream: a long table is not a stalled statement.
                let next = match tokio::time::timeout(timeout, stream.try_next()).await {
                    Ok(Ok(item)) => item,
                    Ok(Err(e)) => return Err(SourceError::Query(describe(&e))),
                    Err(_) => {
                        timed_out = true;
                        return Err(SourceError::Timeout);
                    }
                };
                let Some(item) = next else { break };
                let QueryItem::Row(row) = item else { continue };
                let mut out: Row = Row::with_capacity(width);
                for (i, name) in names.iter().enumerate() {
                    // A SQL NULL is an absent key, so a term map over it
                    // produces no triple.
                    let text: Option<&str> = row
                        .try_get::<&str, _>(i)
                        .map_err(|e| SourceError::Query(describe(&e)))?;
                    if let Some(text) = text {
                        out.insert(
                            name.clone(),
                            Value::new(canonical(kinds[i], text), kinds[i]),
                        );
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
            if !batch.is_empty() {
                sink(batch)?;
            }
            Ok(())
        });
        if timed_out {
            self.broken = true;
        }
        outcome.map(|()| delivered)
    }

    fn max_watermark(&mut self, table: &str, column: &str) -> Result<Option<String>, SourceError> {
        catalogue::max_watermark(self, &MssqlDialect, table, column)
    }

    fn unique_keys(&mut self, table: &str) -> Result<Vec<Vec<String>>, SourceError> {
        catalogue::unique_keys(self, &MssqlDialect, table)
    }

    fn server_version(&mut self) -> Result<Option<String>, SourceError> {
        Ok(self
            .rows("SELECT @@VERSION")?
            .first()
            .and_then(|r| r.first().cloned())
            .flatten()
            .map(|v| v.lines().next().unwrap_or("").trim().to_string()))
    }

    fn profile(&mut self, table: &str) -> Result<TableProfile, SourceError> {
        let sample = self.sample(table, PROFILE_SAMPLE_ROWS)?;
        catalogue::profile(self, &MssqlDialect, table, &sample)
    }
}

/// SQL Server's spelling of the shared catalogue and profiler SQL.
pub struct MssqlDialect;

impl MssqlDialect {
    /// `OBJECT_ID` of a table in the current schema, by name.
    fn object_id(&self, table: &str) -> String {
        format!(
            "OBJECT_ID(QUOTENAME(SCHEMA_NAME()) + '.' + QUOTENAME({}))",
            self.literal(table)
        )
    }
}

impl Dialect for MssqlDialect {
    fn quote(&self, ident: &str) -> String {
        format!("[{}]", ident.replace(']', "]]"))
    }

    fn schema_expr(&self) -> String {
        "SCHEMA_NAME()".to_string()
    }

    fn select_limited(&self, body: &str, limit: usize) -> String {
        format!("SELECT TOP {limit} {body}")
    }

    fn nth_value(&self, table_q: &str, column_q: &str, offset: u64) -> String {
        format!(
            "(SELECT {column_q} FROM {table_q} WHERE {column_q} IS NOT NULL ORDER BY {column_q} \
             OFFSET {offset} ROWS FETCH NEXT 1 ROWS ONLY)"
        )
    }

    fn char_length(&self, expr: &str) -> String {
        format!("LEN({expr})")
    }

    fn to_text(&self, expr: &str) -> String {
        format!("CAST({expr} AS NVARCHAR(MAX))")
    }

    fn avg(&self, expr: &str) -> String {
        format!("AVG(CAST({expr} AS FLOAT))")
    }

    fn countable(&self, native: &str) -> bool {
        let base = native.trim().to_ascii_lowercase();
        let base = base.split('(').next().unwrap_or("").trim().to_string();
        !matches!(
            base.as_str(),
            "text"
                | "ntext"
                | "image"
                | "xml"
                | "geometry"
                | "geography"
                | "hierarchyid"
                | "sql_variant"
        )
    }

    fn columns_sql(&self, table: &str) -> String {
        format!(
            "SELECT c.COLUMN_NAME, \
               CASE WHEN c.CHARACTER_MAXIMUM_LENGTH IS NOT NULL \
                      THEN c.DATA_TYPE + '(' + CASE WHEN c.CHARACTER_MAXIMUM_LENGTH = -1 THEN 'max' \
                           ELSE CAST(c.CHARACTER_MAXIMUM_LENGTH AS VARCHAR(10)) END + ')' \
                    WHEN c.DATA_TYPE IN ('decimal', 'numeric') \
                      THEN c.DATA_TYPE + '(' + CAST(c.NUMERIC_PRECISION AS VARCHAR(10)) + ',' + \
                           CAST(c.NUMERIC_SCALE AS VARCHAR(10)) + ')' \
                    ELSE c.DATA_TYPE END, \
               c.IS_NULLABLE, c.COLUMN_DEFAULT, CAST(ep.value AS NVARCHAR(MAX)) \
             FROM INFORMATION_SCHEMA.COLUMNS c \
             LEFT JOIN sys.extended_properties ep \
               ON ep.major_id = {obj} AND ep.minor_id = c.ORDINAL_POSITION \
              AND ep.class = 1 AND ep.name = 'MS_Description' \
             WHERE c.TABLE_SCHEMA = SCHEMA_NAME() AND c.TABLE_NAME = {name} \
             ORDER BY c.ORDINAL_POSITION",
            obj = self.object_id(table),
            name = self.literal(table)
        )
    }

    fn foreign_keys_sql(&self, table: &str) -> String {
        format!(
            "SELECT fk.name, pc.name, OBJECT_NAME(fkc.referenced_object_id), rc.name \
             FROM sys.foreign_keys fk \
             JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
             JOIN sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
             JOIN sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id \
             WHERE fk.parent_object_id = {} \
             ORDER BY fk.name, fkc.constraint_column_id",
            self.object_id(table)
        )
    }

    fn unique_indexes_sql(&self, table: &str) -> Option<String> {
        Some(format!(
            "SELECT i.name, c.name, ic.key_ordinal \
             FROM sys.indexes i \
             JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
             JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
             WHERE i.object_id = {} AND i.is_unique = 1 AND i.has_filter = 0 AND ic.key_ordinal > 0 \
             ORDER BY i.name, ic.key_ordinal",
            self.object_id(table)
        ))
    }

    fn row_estimate_sql(&self, table: &str) -> Option<String> {
        Some(format!(
            "SELECT SUM(p.rows) FROM sys.partitions p WHERE p.object_id = {} AND p.index_id IN (0, 1)",
            self.object_id(table)
        ))
    }

    fn table_comment_sql(&self, table: &str) -> Option<String> {
        Some(format!(
            "SELECT CAST(value AS NVARCHAR(MAX)) FROM sys.extended_properties \
             WHERE major_id = {} AND minor_id = 0 AND class = 1 AND name = 'MS_Description'",
            self.object_id(table)
        ))
    }

    fn map_type(&self, native: &str) -> ValueKind {
        let lower = native.trim().to_ascii_lowercase();
        match lower.split('(').next().unwrap_or("").trim() {
            "sysname" => ValueKind::Text,
            "hierarchyid" | "geometry" | "geography" | "sql_variant" | "xml" => ValueKind::Other,
            _ => catalogue::standard_type(native),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialect_spells_sql_server() {
        assert_eq!(MssqlDialect.quote("a]b"), "[a]]b]");
        assert_eq!(
            MssqlDialect.select_limited("* FROM [t]", 5),
            "SELECT TOP 5 * FROM [t]"
        );
        assert!(MssqlDialect
            .nth_value("[t]", "[c]", 3)
            .ends_with("OFFSET 3 ROWS FETCH NEXT 1 ROWS ONLY)"));
        assert_eq!(MssqlDialect.char_length("x"), "LEN(x)");
        assert_eq!(MssqlDialect.avg("[c]"), "AVG(CAST([c] AS FLOAT))");
        assert!(MssqlDialect
            .columns_sql("t'x")
            .contains("QUOTENAME('t''x')"));
        assert!(MssqlDialect
            .foreign_keys_sql("t")
            .contains("sys.foreign_key_columns"));
        assert!(MssqlDialect
            .unique_indexes_sql("t")
            .unwrap()
            .contains("has_filter = 0"));
        assert!(!MssqlDialect.countable("ntext"));
        assert!(MssqlDialect.countable("nvarchar(max)"));
        assert_eq!(MssqlDialect.map_type("bit"), ValueKind::Boolean);
        assert_eq!(MssqlDialect.map_type("uniqueidentifier"), ValueKind::Uuid);
        assert_eq!(
            MssqlDialect.map_type("datetimeoffset(7)"),
            ValueKind::DateTime
        );
        assert_eq!(MssqlDialect.map_type("sysname"), ValueKind::Text);
        assert_eq!(MssqlDialect.map_type("hierarchyid"), ValueKind::Other);
        assert_eq!(MssqlDialect.map_type("varbinary(max)"), ValueKind::Binary);
    }

    #[test]
    fn result_types_and_conversions() {
        assert_eq!(kind_of(ColumnType::Intn), ValueKind::Integer);
        assert_eq!(kind_of(ColumnType::Bitn), ValueKind::Boolean);
        assert_eq!(kind_of(ColumnType::Numericn), ValueKind::Decimal);
        assert_eq!(kind_of(ColumnType::Floatn), ValueKind::Float);
        assert_eq!(kind_of(ColumnType::DatetimeOffsetn), ValueKind::DateTime);
        assert_eq!(kind_of(ColumnType::Daten), ValueKind::Date);
        assert_eq!(kind_of(ColumnType::Timen), ValueKind::Time);
        assert_eq!(kind_of(ColumnType::Guid), ValueKind::Uuid);
        assert_eq!(kind_of(ColumnType::BigVarBin), ValueKind::Binary);
        assert_eq!(kind_of(ColumnType::NVarchar), ValueKind::Text);
        assert_eq!(kind_of(ColumnType::Xml), ValueKind::Other);
        assert_eq!(
            convert_expr(ColumnType::Datetime2, "[when_at]"),
            "CONVERT(NVARCHAR(MAX), [when_at], 126)"
        );
        assert_eq!(
            convert_expr(ColumnType::Float8, "[ratio]"),
            "CONVERT(NVARCHAR(MAX), [ratio], 3)"
        );
        assert_eq!(
            convert_expr(ColumnType::BigVarBin, "[blob]"),
            "CONVERT(NVARCHAR(MAX), [blob], 2)"
        );
        assert_eq!(
            convert_expr(ColumnType::Int4, "[n]"),
            "CONVERT(NVARCHAR(MAX), [n])"
        );
        assert_eq!(cell_text(&ColumnData::I32(Some(7))).as_deref(), Some("7"));
        assert_eq!(
            cell_text(&ColumnData::Bit(Some(true))).as_deref(),
            Some("1")
        );
        assert_eq!(cell_text(&ColumnData::String(None)), None);
        assert_eq!(
            cell_text(&ColumnData::Binary(Some(std::borrow::Cow::Borrowed(&[
                0xde, 0xad
            ]))))
            .as_deref(),
            Some("DEAD")
        );
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
                username: None,
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
                MssqlConnector.connect(&broken),
                Err(SourceError::Config(_))
            ));
        }
    }
}
