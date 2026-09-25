//! `ots-plugin-postgres` — the PostgreSQL datasource connector.
//!
//! The contract every connector keeps, in this dialect's terms:
//!
//! * **Read-only, server-side.** Every session starts with
//!   `SET default_transaction_read_only = on`, so a mapping query cannot
//!   write whatever the account could.
//! * **A statement timeout on everything.** `SET statement_timeout` for the
//!   session; a cancelled statement (`57014`) is reported as a timeout, not
//!   as a query error.
//! * **Streaming.** A query runs through a server-side cursor inside a
//!   read-only transaction and is fetched in batches, so a table is never
//!   held whole on either side. Every column is cast to text by the server
//!   and typed from the prepared statement's description, so the wire
//!   carries lexical forms and nothing is decoded by guesswork.
//! * **TLS through rustls**, with the platform roots and, when a datasource
//!   names one in `options.sslrootcert`, a private CA — never a trust-all.
//!
//! The catalogue and the profiler come from
//! [`ots_plugin_api::sources::catalogue`]; this crate spells the
//! PostgreSQL fragments and runs the SQL.

use std::sync::Arc;
use std::time::Duration;

use ots_plugin_api::sources::catalogue::{self, canonical, Dialect, Executor};
use ots_plugin_api::sources::{
    BatchSink, ConnectParams, Row, SourceConnection, SourceConnector, SourceError, TableInfo,
    TableProfile, Value, ValueKind, PROFILE_SAMPLE_ROWS,
};
use ots_plugin_api::Plugin;
use postgres::config::SslMode;
use postgres::error::SqlState;
use postgres::{Client, Config, NoTls, SimpleQueryMessage};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::CertificateDer;
use tokio_postgres_rustls::MakeRustlsConnect;

/// The driver crate, for tooling that builds fixtures next to this connector
/// (the host's live tests) without carrying its own copy.
pub use postgres;

/// The plugin: no routes, one connector.
#[derive(Default)]
pub struct PostgresPlugin;

impl Plugin for PostgresPlugin {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn connectors(&self) -> Vec<Arc<dyn SourceConnector>> {
        vec![Arc::new(PostgresConnector)]
    }
}

pub struct PostgresConnector;

/// The dialect token a datasource names.
pub const DIALECT: &str = "postgresql";

impl SourceConnector for PostgresConnector {
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
            .ok_or_else(|| {
                SourceError::Config("a postgresql datasource needs a host".to_string())
            })?;
        if params.database.trim().is_empty() {
            return Err(SourceError::Config(
                "a postgresql datasource needs a database name".to_string(),
            ));
        }
        let mut config = Config::new();
        config
            .host(host)
            .port(params.port.unwrap_or(5432))
            .dbname(params.database.trim())
            .application_name("open-triplestore")
            .connect_timeout(Duration::from_secs(15));
        if let Some(user) = params.username.as_deref().filter(|u| !u.trim().is_empty()) {
            config.user(user.trim());
        }
        if let Some(password) = &params.password {
            config.password(password.expose());
        }
        let client = if params.tls {
            config.ssl_mode(SslMode::Require);
            let tls = tls_connector(params.options.get("sslrootcert").map(String::as_str))?;
            config.connect(tls)
        } else {
            config.ssl_mode(SslMode::Disable);
            config.connect(NoTls)
        }
        .map_err(|e| SourceError::Connect(describe(&e)))?;

        let mut conn = PostgresConnection {
            client,
            timeout_ms: params.statement_timeout_ms.max(1),
        };
        conn.session_setup(params.options.get("search_path").map(String::as_str))?;
        Ok(Box::new(conn))
    }

    fn quote_identifier(&self, ident: &str) -> String {
        PgDialect.quote(ident)
    }
}

/// A rustls connector over the platform roots plus an optional private CA
/// bundle. Host names are always verified.
fn tls_connector(root_bundle: Option<&str>) -> Result<MakeRustlsConnect, SourceError> {
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        let _ = roots.add(cert);
    }
    if let Some(path) = root_bundle.map(str::trim).filter(|p| !p.is_empty()) {
        let certs = CertificateDer::pem_file_iter(path)
            .map_err(|e| SourceError::Config(format!("sslrootcert could not be read: {e}")))?;
        for cert in certs {
            let cert = cert.map_err(|e| {
                SourceError::Config(format!("sslrootcert holds no valid certificate: {e}"))
            })?;
            roots
                .add(cert)
                .map_err(|e| SourceError::Config(format!("sslrootcert refused: {e}")))?;
        }
    }
    if roots.is_empty() {
        return Err(SourceError::Config(
            "no trusted root certificates: none on the platform and no sslrootcert given"
                .to_string(),
        ));
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| SourceError::Config(format!("tls configuration: {e}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(MakeRustlsConnect::new(config))
}

/// A driver error as a message. The server's own message when there is one
/// — it never carries the password — else the driver's summary. The host
/// scrubs host, database and account on top.
fn describe(e: &postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => db.message().to_string(),
        None => e.to_string(),
    }
}

struct PostgresConnection {
    client: Client,
    timeout_ms: u64,
}

impl PostgresConnection {
    fn session_setup(&mut self, search_path: Option<&str>) -> Result<(), SourceError> {
        // Read-only and the statement budget are session settings, applied
        // before anything else runs; ISO dates keep timestamps in the shape
        // the natural mapping expects.
        let mut setup = format!(
            "SET default_transaction_read_only = on; SET statement_timeout = {}; \
             SET DateStyle = 'ISO, YMD'; SET TIME ZONE 'UTC'",
            self.timeout_ms
        );
        // A datasource in a schema of its own names it in `options.search_path`
        // (one or more schemas, comma-separated); the catalogue is read from
        // the first, the queries resolve through all.
        if let Some(path) = search_path.map(str::trim).filter(|p| !p.is_empty()) {
            let schemas: Vec<String> = path
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| PgDialect.quote(s))
                .collect();
            if !schemas.is_empty() {
                setup.push_str(&format!("; SET search_path = {}", schemas.join(", ")));
            }
        }
        self.client
            .batch_execute(&setup)
            .map_err(|e| SourceError::Connect(describe(&e)))
    }

    fn error(&self, e: postgres::Error) -> SourceError {
        if e.code() == Some(&SqlState::QUERY_CANCELED) {
            SourceError::Timeout
        } else {
            SourceError::Query(describe(&e))
        }
    }
}

/// The generic type of a prepared column's PostgreSQL type.
fn kind_of(t: &postgres::types::Type) -> ValueKind {
    match t.name() {
        "int2" | "int4" | "int8" | "oid" => ValueKind::Integer,
        "numeric" | "money" => ValueKind::Decimal,
        "float4" | "float8" => ValueKind::Float,
        "bool" => ValueKind::Boolean,
        "date" => ValueKind::Date,
        "time" | "timetz" => ValueKind::Time,
        "timestamp" | "timestamptz" => ValueKind::DateTime,
        "bytea" => ValueKind::Binary,
        "uuid" => ValueKind::Uuid,
        "json" | "jsonb" => ValueKind::Json,
        "text" | "varchar" | "bpchar" | "name" | "citext" => ValueKind::Text,
        _ => ValueKind::Other,
    }
}

/// A cell as the natural mapping wants it: `bytea` text (`\x…`) as upper-case
/// hex, everything else through [`canonical`].
fn lexical(kind: ValueKind, text: &str) -> String {
    match kind {
        ValueKind::Binary => text
            .strip_prefix("\\x")
            .map(str::to_ascii_uppercase)
            .unwrap_or_else(|| text.to_string()),
        other => canonical(other, text),
    }
}

impl Executor for PostgresConnection {
    fn rows(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>, SourceError> {
        let messages = self.client.simple_query(sql).map_err(|e| self.error(e))?;
        Ok(messages
            .into_iter()
            .filter_map(|m| match m {
                SimpleQueryMessage::Row(row) => Some(
                    (0..row.len())
                        .map(|i| row.get(i).map(str::to_string))
                        .collect(),
                ),
                _ => None,
            })
            .collect())
    }
}

impl SourceConnection for PostgresConnection {
    fn introspect(&mut self) -> Result<Vec<TableInfo>, SourceError> {
        catalogue::introspect(self, &PgDialect)
    }

    fn table_names(&mut self) -> Result<Vec<String>, SourceError> {
        Ok(catalogue::listing(self, &PgDialect)?
            .into_iter()
            .map(|(name, _)| name)
            .collect())
    }

    fn sample(&mut self, table: &str, limit: usize) -> Result<Vec<Row>, SourceError> {
        let sql = catalogue::sample_sql(&PgDialect, table, limit);
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
        // The statement's own description names and types every column;
        // the server then casts each to text, so the rows arrive as
        // lexical forms typed by that description.
        let statement = self.client.prepare(query).map_err(|e| self.error(e))?;
        let names: Vec<String> = statement
            .columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect();
        let kinds: Vec<ValueKind> = statement
            .columns()
            .iter()
            .map(|c| kind_of(c.type_()))
            .collect();
        // Positional aliases, so two columns with one name in the query do
        // not make the cast ambiguous.
        let aliases: Vec<String> = (0..names.len()).map(|i| format!("\"c{i}\"")).collect();
        let casts: Vec<String> = aliases
            .iter()
            .map(|a| format!("CAST({a} AS TEXT) AS {a}"))
            .collect();
        let wrapped = format!(
            "SELECT {} FROM ({query}) AS ots_q ({})",
            casts.join(", "),
            aliases.join(", ")
        );

        self.client
            .batch_execute("BEGIN READ ONLY")
            .map_err(|e| self.error(e))?;
        let outcome = (|| -> Result<u64, SourceError> {
            self.client
                .batch_execute(&format!(
                    "DECLARE ots_cursor NO SCROLL CURSOR FOR {wrapped}"
                ))
                .map_err(|e| self.error(e))?;
            let mut delivered = 0u64;
            loop {
                let messages = self
                    .client
                    .simple_query(&format!("FETCH FORWARD {batch_size} FROM ots_cursor"))
                    .map_err(|e| self.error(e))?;
                let mut batch: Vec<Row> = Vec::with_capacity(batch_size);
                for m in messages {
                    let SimpleQueryMessage::Row(row) = m else {
                        continue;
                    };
                    let mut out: Row = Row::with_capacity(names.len());
                    for (i, name) in names.iter().enumerate() {
                        // A SQL NULL is an absent key, so a term map over it
                        // produces no triple.
                        if let Some(text) = row.get(i) {
                            out.insert(name.clone(), Value::new(lexical(kinds[i], text), kinds[i]));
                        }
                    }
                    batch.push(out);
                }
                let n = batch.len();
                if n == 0 {
                    break;
                }
                delivered += n as u64;
                sink(batch)?;
                if n < batch_size {
                    break;
                }
            }
            Ok(delivered)
        })();
        let _ = self.client.batch_execute("CLOSE ots_cursor");
        let _ = self.client.batch_execute(if outcome.is_ok() {
            "COMMIT"
        } else {
            "ROLLBACK"
        });
        outcome
    }

    fn max_watermark(&mut self, table: &str, column: &str) -> Result<Option<String>, SourceError> {
        catalogue::max_watermark(self, &PgDialect, table, column)
    }

    fn unique_keys(&mut self, table: &str) -> Result<Vec<Vec<String>>, SourceError> {
        catalogue::unique_keys(self, &PgDialect, table)
    }

    fn server_version(&mut self) -> Result<Option<String>, SourceError> {
        Ok(self
            .rows("SELECT version()")?
            .first()
            .and_then(|r| r.first().cloned())
            .flatten())
    }

    fn profile(&mut self, table: &str) -> Result<TableProfile, SourceError> {
        let sample = self.sample(table, PROFILE_SAMPLE_ROWS)?;
        catalogue::profile(self, &PgDialect, table, &sample)
    }
}

/// PostgreSQL's spelling of the shared catalogue and profiler SQL.
pub struct PgDialect;

impl Dialect for PgDialect {
    fn quote(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    fn schema_expr(&self) -> String {
        "current_schema()".to_string()
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
        format!("LENGTH({expr})")
    }

    fn to_text(&self, expr: &str) -> String {
        format!("CAST({expr} AS TEXT)")
    }

    fn columns_sql(&self, table: &str) -> String {
        format!(
            "SELECT c.column_name, \
               CASE WHEN c.data_type IN ('character varying', 'character') AND c.character_maximum_length IS NOT NULL \
                      THEN c.data_type || '(' || c.character_maximum_length || ')' \
                    WHEN c.data_type = 'numeric' AND c.numeric_precision IS NOT NULL \
                      THEN 'numeric(' || c.numeric_precision || ',' || COALESCE(c.numeric_scale, 0) || ')' \
                    WHEN c.data_type IN ('ARRAY', 'USER-DEFINED') THEN c.udt_name \
                    ELSE c.data_type END, \
               c.is_nullable, c.column_default, \
               col_description(format('%I.%I', c.table_schema, c.table_name)::regclass, c.ordinal_position) \
             FROM information_schema.columns c \
             WHERE c.table_schema = current_schema() AND c.table_name = {} \
             ORDER BY c.ordinal_position",
            self.literal(table)
        )
    }

    fn foreign_keys_sql(&self, table: &str) -> String {
        format!(
            "SELECT kcu.constraint_name, kcu.column_name, ref.table_name, ref.column_name \
             FROM information_schema.referential_constraints rc \
             JOIN information_schema.key_column_usage kcu \
               ON kcu.constraint_schema = rc.constraint_schema AND kcu.constraint_name = rc.constraint_name \
             JOIN information_schema.key_column_usage ref \
               ON ref.constraint_schema = rc.unique_constraint_schema \
              AND ref.constraint_name = rc.unique_constraint_name \
              AND ref.ordinal_position = kcu.position_in_unique_constraint \
             WHERE kcu.table_schema = current_schema() AND kcu.table_name = {} \
             ORDER BY kcu.constraint_name, kcu.ordinal_position",
            self.literal(table)
        )
    }

    fn unique_indexes_sql(&self, table: &str) -> Option<String> {
        Some(format!(
            "SELECT i.relname, a.attname, k.n \
             FROM pg_index x \
             JOIN pg_class t ON t.oid = x.indrelid \
             JOIN pg_class i ON i.oid = x.indexrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN LATERAL unnest(x.indkey) WITH ORDINALITY AS k(attnum, n) ON true \
             JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum \
             WHERE n.nspname = current_schema() AND t.relname = {} \
               AND x.indisunique AND x.indpred IS NULL AND x.indexprs IS NULL \
             ORDER BY i.relname, k.n",
            self.literal(table)
        ))
    }

    fn row_estimate_sql(&self, table: &str) -> Option<String> {
        Some(format!(
            "SELECT CASE WHEN c.reltuples < 0 THEN NULL ELSE CAST(c.reltuples AS BIGINT) END \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = current_schema() AND c.relname = {}",
            self.literal(table)
        ))
    }

    fn table_comment_sql(&self, table: &str) -> Option<String> {
        Some(format!(
            "SELECT obj_description(format('%I.%I', current_schema(), {})::regclass, 'pg_class')",
            self.literal(table)
        ))
    }

    fn map_type(&self, native: &str) -> ValueKind {
        let lower = native.trim().to_ascii_lowercase();
        // A bit string is not a boolean, an array (`_int4`) is not its
        // element type, and `oid` is an integer.
        if lower.starts_with("bit") || lower.starts_with('_') {
            return ValueKind::Other;
        }
        if lower == "oid" {
            return ValueKind::Integer;
        }
        catalogue::standard_type(native)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialect_spells_postgresql() {
        assert_eq!(PgDialect.quote("a\"b"), "\"a\"\"b\"");
        assert_eq!(
            PgDialect.select_limited("* FROM \"t\"", 5),
            "SELECT * FROM \"t\" LIMIT 5"
        );
        assert!(PgDialect
            .nth_value("\"t\"", "\"c\"", 3)
            .ends_with("LIMIT 1 OFFSET 3)"));
        assert_eq!(PgDialect.to_text("\"c\""), "CAST(\"c\" AS TEXT)");
        assert!(PgDialect
            .columns_sql("t'x")
            .contains("c.table_name = 't''x'"));
        assert!(PgDialect
            .foreign_keys_sql("t")
            .contains("position_in_unique_constraint"));
        assert!(PgDialect
            .unique_indexes_sql("t")
            .unwrap()
            .contains("indisunique"));
        assert_eq!(PgDialect.map_type("bit varying"), ValueKind::Other);
        assert_eq!(PgDialect.map_type("_int4"), ValueKind::Other);
        assert_eq!(PgDialect.map_type("oid"), ValueKind::Integer);
        assert_eq!(
            PgDialect.map_type("character varying(120)"),
            ValueKind::Text
        );
        assert_eq!(
            PgDialect.map_type("timestamp with time zone"),
            ValueKind::DateTime
        );
    }

    #[test]
    fn prepared_types_and_lexical_forms() {
        use postgres::types::Type;
        assert_eq!(kind_of(&Type::INT8), ValueKind::Integer);
        assert_eq!(kind_of(&Type::NUMERIC), ValueKind::Decimal);
        assert_eq!(kind_of(&Type::FLOAT8), ValueKind::Float);
        assert_eq!(kind_of(&Type::BOOL), ValueKind::Boolean);
        assert_eq!(kind_of(&Type::TIMESTAMPTZ), ValueKind::DateTime);
        assert_eq!(kind_of(&Type::BYTEA), ValueKind::Binary);
        assert_eq!(kind_of(&Type::UUID), ValueKind::Uuid);
        assert_eq!(kind_of(&Type::JSONB), ValueKind::Json);
        assert_eq!(kind_of(&Type::VARCHAR), ValueKind::Text);
        assert_eq!(kind_of(&Type::INTERVAL), ValueKind::Other);
        assert_eq!(lexical(ValueKind::Binary, "\\xdeadbeef"), "DEADBEEF");
        assert_eq!(lexical(ValueKind::Boolean, "t"), "true");
        assert_eq!(
            lexical(ValueKind::DateTime, "2026-01-01 12:00:00+00"),
            "2026-01-01T12:00:00+00:00"
        );
        assert_eq!(lexical(ValueKind::Text, "as is"), "as is");
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
        let no_host = ConnectParams {
            host: None,
            ..base.clone()
        };
        assert!(matches!(
            PostgresConnector.connect(&no_host),
            Err(SourceError::Config(_))
        ));
        let writable = ConnectParams {
            read_only: false,
            ..base.clone()
        };
        assert!(matches!(
            PostgresConnector.connect(&writable),
            Err(SourceError::Config(_))
        ));
        let no_db = ConnectParams {
            database: " ".into(),
            ..base
        };
        assert!(matches!(
            PostgresConnector.connect(&no_db),
            Err(SourceError::Config(_))
        ));
    }

    #[test]
    fn a_missing_ca_bundle_is_a_configuration_error() {
        assert!(matches!(
            tls_connector(Some("/nonexistent/ca.pem")),
            Err(SourceError::Config(_))
        ));
    }
}
