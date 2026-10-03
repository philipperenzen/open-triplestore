//! The W3C R2RML test cases (RDB2RDF Working Group), run on SQLite here and
//! on PostgreSQL and MySQL in the live-database CI job.
//!
//! **Fetched, not vendored.** `scripts/fetch-w3c-r2rml-tests.sh` downloads the
//! cases at a pinned commit and checks every file's sha256; whether W3C's
//! 3-clause BSD option covers this suite is unconfirmed, so the files stay out
//! of the repository. Under W3C's test-suite policy no score is published from
//! them: the runner is a development and regression ratchet, and records no
//! pass count.
//!
//! The corpus is read from `OTS_TEST_R2RML_DIR` (default
//! `tests/fixtures/w3c-r2rml`). Without it the tests skip — unless
//! `OTS_TEST_R2RML_REQUIRED` is set, as the CI jobs that fetch it do, which
//! turns a missing corpus into a failure. The PostgreSQL and MySQL runs need
//! their plugin features and `OTS_TEST_POSTGRES_HOST` / `OTS_TEST_MYSQL_HOST`
//! (see `tests/sources_postgres_http.rs` and `plugins/mysql/tests/live.rs`).
//!
//! **What a case checks.** Each `rdb2rdftest:R2RML` case loads its database's
//! SQL script into a fresh database, runs the mapping through the relational
//! executor with the suite's base IRI `http://example.com/base/`, and writes
//! every triple to the graphs its graph maps name. A case with an expected
//! output passes when the output dataset is isomorphic to it (blank nodes
//! matched by canonicalisation; literals compared after the store's own
//! canonical encoding on both sides). A case without one passes when the
//! mapping is refused or the run fails. The Direct Mapping cases of the suite
//! are out of scope.
//!
//! **Ratchet.** `KNOWN_FAILURES` lists each case that does not pass, per
//! database, with the reason. A case that fails without being listed fails
//! the test, and so does a listed case that passes — remove it then.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use open_triplestore::rml::checks::OnDataError;
use open_triplestore::rml::parse_rml;
use open_triplestore::rml::sql::execute_relational_as_mapped;
use open_triplestore::store::TripleStore;
use ots_plugin_api::sources::{ConnectParams, SourceConnection, SourceConnector};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::dataset::CanonicalizationAlgorithm;
use oxigraph::model::Dataset;
use oxigraph::sparql::QueryResults;

/// The base IRI the suite's expected outputs were generated with.
const BASE: &str = "http://example.com/base/";

/// Cases that do not pass, per database: `(database, case, reason)`.
const KNOWN_FAILURES: &[(&str, &str, &str)] = &[
    (
        "sqlite",
        "R2RMLTC0002f",
        "SQL 2008 folds the regular identifier Name to NAME, which the delimited column \"Name\" \
         is not; SQLite compares identifiers without regard to case, and the engine matches a \
         regular identifier as the database reports its columns",
    ),
    (
        "postgresql",
        "R2RMLTC0002f",
        "SQL 2008 folds the regular identifier Name to NAME (PostgreSQL to name), which the \
         delimited column \"Name\" is not; the engine matches a regular identifier as written, \
         as the database reports its columns",
    ),
    (
        "mysql",
        "R2RMLTC0002f",
        "MySQL compares column names without regard to case, so Name names the column \
         \"Name\"; the suite itself lists this case as MySQL non-compliance",
    ),
    (
        "mysql",
        "R2RMLTC0018a",
        "MySQL strips the padding of CHAR values unless the server runs with the deprecated \
         PAD_CHAR_TO_FULL_LENGTH SQL mode; the suite lists this case as MySQL non-compliance",
    ),
    (
        "sqlite",
        "R2RMLTC0018a",
        "SQLite does not pad CHAR(15) values to their declared length, so \"Venus\" comes back \
         without the trailing spaces the case expects",
    ),
];

/// One R2RML case from the manifest.
#[derive(Debug, Clone)]
struct Case {
    id: String,
    mapping: String,
    /// `None`: the mapping must be refused or the run must fail.
    output: Option<String>,
}

/// A database script and the cases that run against it.
#[derive(Debug, Clone)]
struct Database {
    script: String,
    cases: Vec<Case>,
}

/// The corpus directory, or `None` when it has not been fetched.
fn corpus() -> Option<PathBuf> {
    let dir = std::env::var_os("OTS_TEST_R2RML_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/w3c-r2rml"));
    if dir.join("manifest.ttl").is_file() {
        return Some(dir);
    }
    assert!(
        std::env::var_os("OTS_TEST_R2RML_REQUIRED").is_none(),
        "OTS_TEST_R2RML_REQUIRED is set but {} holds no manifest.ttl; run \
         scripts/fetch-w3c-r2rml-tests.sh",
        dir.display()
    );
    eprintln!(
        "skipping: the W3C R2RML test cases are not fetched (scripts/fetch-w3c-r2rml-tests.sh)"
    );
    None
}

fn solutions(store: &TripleStore, q: &str) -> Vec<BTreeMap<String, String>> {
    let QueryResults::Solutions(sols) = store.query(q).expect("manifest query") else {
        panic!("expected solutions")
    };
    sols.map(|s| {
        let s = s.expect("solution");
        s.iter()
            .map(|(v, t)| {
                let text = match t {
                    oxigraph::model::Term::Literal(l) => l.value().to_string(),
                    oxigraph::model::Term::NamedNode(n) => n.as_str().to_string(),
                    other => other.to_string(),
                };
                (v.as_str().to_string(), text)
            })
            .collect()
    })
    .collect()
}

/// The manifest's databases and their R2RML cases, in identifier order.
fn manifest(dir: &Path) -> Vec<Database> {
    let store = TripleStore::in_memory().unwrap();
    let text = std::fs::read_to_string(dir.join("manifest.ttl")).expect("manifest.ttl");
    store
        .load_str(&text, RdfFormat::Turtle, None)
        .expect("manifest parses");
    let rows = solutions(
        &store,
        "PREFIX rdb2rdftest: <http://purl.org/NET/rdb2rdf-test#>
         PREFIX dcterms: <http://purl.org/dc/terms/>
         SELECT ?script ?id ?mapping ?expected ?output WHERE {
           ?case a rdb2rdftest:R2RML ;
                 dcterms:identifier ?id ;
                 rdb2rdftest:database ?db ;
                 rdb2rdftest:mappingDocument ?mapping ;
                 rdb2rdftest:hasExpectedOutput ?expected .
           OPTIONAL { ?case rdb2rdftest:output ?output }
           ?db rdb2rdftest:sqlScriptFile ?script .
         } ORDER BY ?script ?id",
    );
    let mut by_script: BTreeMap<String, Vec<Case>> = BTreeMap::new();
    for r in rows {
        let expected = r["expected"] == "true";
        by_script
            .entry(r["script"].clone())
            .or_default()
            .push(Case {
                id: r["id"].clone(),
                mapping: r["mapping"].clone(),
                output: expected.then(|| r["output"].clone()),
            });
    }
    by_script
        .into_iter()
        .map(|(script, cases)| Database { script, cases })
        .collect()
}

/// A database the cases run against: a fresh one per script.
trait Backend {
    /// The name `KNOWN_FAILURES` uses.
    fn name(&self) -> &'static str;
    /// The script to load for `script`: some databases need a variant.
    fn script_for(&self, script: &str, dir: &Path) -> PathBuf {
        dir.join("databases").join(script)
    }
    /// The mapping to run for a case: some databases need a variant.
    fn mapping_for(&self, case_dir: &Path, mapping: &str) -> PathBuf {
        case_dir.join(mapping)
    }
    /// Replace the database's contents with what `sql` creates.
    fn load(&mut self, sql: &str) -> Result<(), String>;
    fn connect(&self) -> Box<dyn SourceConnection>;
    fn quote(&self, ident: &str) -> String;
}

/// The statements of a script, one per `;`-terminated line group.
fn statements(sql: &str) -> Vec<String> {
    sql.split(";\n")
        .flat_map(|chunk| chunk.split(";\r\n"))
        .map(|s| s.trim().trim_end_matches(';').trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

struct Sqlite {
    dir: tempfile::TempDir,
    generation: u32,
}

impl Sqlite {
    fn new() -> Self {
        Sqlite {
            dir: tempfile::tempdir().unwrap(),
            generation: 0,
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join(format!("d{}.db", self.generation))
    }
}

impl Backend for Sqlite {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn load(&mut self, sql: &str) -> Result<(), String> {
        self.generation += 1;
        let conn = rusqlite::Connection::open(self.path()).map_err(|e| e.to_string())?;
        for stmt in statements(sql) {
            // SQLite has no `DROP TABLE … CASCADE`, and a fresh file has
            // nothing to drop.
            if stmt.to_ascii_uppercase().starts_with("DROP TABLE") {
                continue;
            }
            conn.execute_batch(&stmt)
                .map_err(|e| format!("{e}: {stmt}"))?;
        }
        Ok(())
    }

    fn connect(&self) -> Box<dyn SourceConnection> {
        open_triplestore::sources::sqlite::SqliteConnector
            .connect(&ConnectParams {
                dialect: "sqlite".into(),
                host: None,
                port: None,
                database: self.path().to_string_lossy().into_owned(),
                username: None,
                password: None,
                read_only: true,
                statement_timeout_ms: 10_000,
                tls: false,
                options: Default::default(),
            })
            .expect("sqlite connects")
    }

    fn quote(&self, ident: &str) -> String {
        open_triplestore::sources::sqlite::SqliteConnector.quote_identifier(ident)
    }
}

/// The store's contents as a dataset with canonical blank-node labels. Both
/// sides of a comparison come out of a store, so their literals carry the
/// same canonical encoding.
fn canonical(store: &TripleStore) -> Dataset {
    let mut nq = Vec::new();
    store.dump_all_nquads(&mut nq).expect("dump");
    let mut ds = Dataset::new();
    for q in RdfParser::from_format(RdfFormat::NQuads).for_slice(&nq) {
        ds.insert(&q.expect("own dump parses"));
    }
    ds.canonicalize(CanonicalizationAlgorithm::Unstable);
    ds
}

/// What running one case produced.
enum Outcome {
    Pass,
    Fail(String),
}

fn run_case(backend: &dyn Backend, case_dir: &Path, case: &Case) -> Outcome {
    let mapping_path = backend.mapping_for(case_dir, &case.mapping);
    let text = match std::fs::read_to_string(&mapping_path) {
        Ok(t) => t,
        Err(e) => return Outcome::Fail(format!("reading {}: {e}", mapping_path.display())),
    };
    let result = parse_rml(&text).and_then(|mut mapping| {
        mapping.base_iri = Some(BASE.to_string());
        let store = TripleStore::in_memory().unwrap();
        let mut conn = backend.connect();
        let quote = |i: &str| backend.quote(i);
        execute_relational_as_mapped(
            &mapping,
            conn.as_mut(),
            &quote,
            &store,
            1_000,
            "w3c",
            OnDataError::Abort,
        )
        .map(|_| store)
    });
    match (&case.output, result) {
        (None, Err(_)) => Outcome::Pass,
        (None, Ok(store)) => Outcome::Fail(format!(
            "expected an error, but the run produced {} quads",
            store.len().unwrap_or(0)
        )),
        (Some(_), Err(e)) => Outcome::Fail(format!("error: {e}")),
        (Some(output), Ok(store)) => {
            let expected_text = match std::fs::read_to_string(case_dir.join(output)) {
                Ok(t) => t,
                Err(e) => return Outcome::Fail(format!("reading {output}: {e}")),
            };
            let expected = TripleStore::in_memory().unwrap();
            if let Err(e) = expected.load_str(&expected_text, RdfFormat::NQuads, None) {
                return Outcome::Fail(format!("expected output does not parse: {e}"));
            }
            let (want, got) = (canonical(&expected), canonical(&store));
            if want == got {
                Outcome::Pass
            } else {
                let w: BTreeSet<String> = want.iter().map(|q| q.to_string()).collect();
                let g: BTreeSet<String> = got.iter().map(|q| q.to_string()).collect();
                let missing: Vec<&String> = w.difference(&g).take(5).collect();
                let extra: Vec<&String> = g.difference(&w).take(5).collect();
                Outcome::Fail(format!(
                    "output differs; missing {missing:?}, unexpected {extra:?}"
                ))
            }
        }
    }
}

/// Run every case on `backend` and hold the result to `KNOWN_FAILURES`.
fn run_suite(dir: &Path, backend: &mut dyn Backend) {
    let known: BTreeMap<&str, &str> = KNOWN_FAILURES
        .iter()
        .filter(|(db, _, _)| *db == backend.name())
        .map(|(_, id, why)| (*id, *why))
        .collect();
    let mut unexpected: Vec<String> = Vec::new();
    let mut fixed: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for db in manifest(dir) {
        let script_path = backend.script_for(&db.script, dir);
        let sql = std::fs::read_to_string(&script_path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", script_path.display()));
        backend
            .load(&sql)
            .unwrap_or_else(|e| panic!("{}: loading {}: {e}", backend.name(), db.script));
        for case in &db.cases {
            seen.insert(case.id.clone());
            let outcome = run_case(&*backend, &dir.join(&case.id), case);
            match (outcome, known.get(case.id.as_str())) {
                (Outcome::Pass, Some(_)) => fixed.push(case.id.clone()),
                (Outcome::Pass, None) => {}
                (Outcome::Fail(_), Some(_)) => {}
                (Outcome::Fail(why), None) => unexpected.push(format!("{}: {why}", case.id)),
            }
        }
    }
    let stale: Vec<&&str> = known.keys().filter(|id| !seen.contains(**id)).collect();
    assert!(
        unexpected.is_empty() && fixed.is_empty() && stale.is_empty(),
        "{}: {} unexpected failure(s):\n  {}\n{} known failure(s) now pass (remove them from \
         KNOWN_FAILURES): {:?}\nknown failures the manifest does not have: {:?}",
        backend.name(),
        unexpected.len(),
        unexpected.join("\n  "),
        fixed.len(),
        fixed,
        stale
    );
}

#[test]
fn w3c_r2rml_test_cases_on_sqlite() {
    let Some(dir) = corpus() else { return };
    run_suite(&dir, &mut Sqlite::new());
}

#[test]
fn known_failures_name_a_database_and_a_reason() {
    for (db, id, why) in KNOWN_FAILURES {
        assert!(
            matches!(*db, "sqlite" | "postgresql" | "mysql"),
            "{id}: unknown database {db}"
        );
        assert!(id.starts_with("R2RMLTC"), "{id} is not an R2RML case");
        assert!(!why.trim().is_empty(), "{id} on {db} gives no reason");
    }
}

#[test]
fn script_statements_split_on_terminators() {
    assert_eq!(
        statements("DROP TABLE x;\nCREATE TABLE \"a;b\" (c INT);\nINSERT INTO x VALUES (1);"),
        vec![
            "DROP TABLE x".to_string(),
            "CREATE TABLE \"a;b\" (c INT)".to_string(),
            "INSERT INTO x VALUES (1)".to_string()
        ]
    );
}

/// The connection details of a live server, from the variables the
/// live-database CI job sets.
#[cfg(any(feature = "plugin-postgres", feature = "plugin-mysql"))]
struct Live {
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    db: String,
}

#[cfg(any(feature = "plugin-postgres", feature = "plugin-mysql"))]
fn live(prefix: &str, port: u16, user: &str, db: &str) -> Option<Live> {
    let Ok(host) = std::env::var(format!("OTS_TEST_{prefix}_HOST")) else {
        assert!(
            std::env::var_os("OTS_TEST_LIVE_REQUIRED").is_none(),
            "OTS_TEST_LIVE_REQUIRED is set but OTS_TEST_{prefix}_HOST is not"
        );
        eprintln!("skipping: OTS_TEST_{prefix}_HOST is not set");
        return None;
    };
    Some(Live {
        host,
        port: std::env::var(format!("OTS_TEST_{prefix}_PORT"))
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(port),
        user: std::env::var(format!("OTS_TEST_{prefix}_USER")).unwrap_or_else(|_| user.into()),
        password: std::env::var(format!("OTS_TEST_{prefix}_PASSWORD")).ok(),
        db: std::env::var(format!("OTS_TEST_{prefix}_DB")).unwrap_or_else(|_| db.into()),
    })
}

/// The server may still be starting when a CI job reaches this test.
#[cfg(any(feature = "plugin-postgres", feature = "plugin-mysql"))]
fn patiently<T, E: std::fmt::Display>(mut attempt: impl FnMut() -> Result<T, E>) -> T {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match attempt() {
            Ok(v) => return v,
            Err(e) if std::time::Instant::now() < deadline => {
                eprintln!("waiting for the server: {e}");
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            Err(e) => panic!("admin connection: {e}"),
        }
    }
}

#[cfg(feature = "plugin-postgres")]
mod postgresql {
    use super::*;
    use ots_plugin_postgres::postgres;

    /// A schema of the run's own, emptied before every script; the connector
    /// reads it through `options.search_path`.
    struct Postgres {
        live: Live,
        schema: String,
        admin: postgres::Client,
    }

    impl Backend for Postgres {
        fn name(&self) -> &'static str {
            "postgresql"
        }

        fn script_for(&self, script: &str, dir: &Path) -> PathBuf {
            // The suite's PostgreSQL variant of the datatype script (BYTEA).
            let variant = script.replace(".sql", "-postgresql.sql");
            let path = dir.join("databases").join(&variant);
            if path.is_file() {
                path
            } else {
                dir.join("databases").join(script)
            }
        }

        fn load(&mut self, sql: &str) -> Result<(), String> {
            let s = &self.schema;
            self.admin
                .batch_execute(&format!(
                    "DROP SCHEMA IF EXISTS {s} CASCADE; CREATE SCHEMA {s}; SET search_path = {s}"
                ))
                .map_err(|e| e.to_string())?;
            for stmt in statements(sql) {
                self.admin
                    .batch_execute(&stmt)
                    .map_err(|e| format!("{e}: {stmt}"))?;
            }
            Ok(())
        }

        fn connect(&self) -> Box<dyn SourceConnection> {
            ots_plugin_postgres::PostgresConnector
                .connect(&ConnectParams {
                    dialect: "postgresql".into(),
                    host: Some(self.live.host.clone()),
                    port: Some(self.live.port),
                    database: self.live.db.clone(),
                    username: Some(self.live.user.clone()),
                    password: self
                        .live
                        .password
                        .clone()
                        .map(ots_plugin_api::sources::SecretString::new),
                    read_only: true,
                    statement_timeout_ms: 10_000,
                    tls: false,
                    options: [("search_path".to_string(), self.schema.clone())]
                        .into_iter()
                        .collect(),
                })
                .expect("postgresql connects")
        }

        fn quote(&self, ident: &str) -> String {
            ots_plugin_postgres::PostgresConnector.quote_identifier(ident)
        }
    }

    impl Drop for Postgres {
        fn drop(&mut self) {
            let _ = self
                .admin
                .batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.schema));
        }
    }

    #[test]
    fn w3c_r2rml_test_cases_on_postgresql() {
        let Some(dir) = corpus() else { return };
        let Some(live) = live("POSTGRES", 5432, "postgres", "postgres") else {
            return;
        };
        let mut config = postgres::Config::new();
        config
            .host(&live.host)
            .port(live.port)
            .user(&live.user)
            .dbname(&live.db);
        if let Some(p) = &live.password {
            config.password(p);
        }
        let admin = patiently(|| config.connect(postgres::NoTls));
        let schema = format!("ots_r2rml_{}", std::process::id());
        run_suite(
            &dir,
            &mut Postgres {
                live,
                schema,
                admin,
            },
        );
    }
}

#[cfg(feature = "plugin-mysql")]
mod mysql_server {
    use super::*;
    use ots_plugin_mysql::mysql;
    use ots_plugin_mysql::mysql::prelude::Queryable;

    /// A database of the run's own, recreated before every script. The suite
    /// writes SQL identifiers in double quotes, which MySQL reads as
    /// identifiers only under `ANSI_QUOTES` — set globally, so the
    /// connector's own sessions read the mappings' queries the same way.
    struct Mysql {
        live: Live,
        database: String,
        admin: mysql::Conn,
        /// The server's `sql_mode` before the run, restored after it.
        previous_mode: String,
    }

    impl Backend for Mysql {
        fn name(&self) -> &'static str {
            "mysql"
        }

        fn mapping_for(&self, case_dir: &Path, mapping: &str) -> PathBuf {
            // The suite's MySQL variants of mappings whose SQL MySQL reads
            // differently.
            let variant = case_dir.join(mapping.replace(".ttl", "-mysql.ttl"));
            if variant.is_file() {
                variant
            } else {
                case_dir.join(mapping)
            }
        }

        fn load(&mut self, sql: &str) -> Result<(), String> {
            let d = &self.database;
            self.admin
                .query_drop(format!("DROP DATABASE IF EXISTS {d}"))
                .and_then(|_| self.admin.query_drop(format!("CREATE DATABASE {d}")))
                .and_then(|_| self.admin.query_drop(format!("USE {d}")))
                .map_err(|e| e.to_string())?;
            for stmt in statements(sql) {
                self.admin
                    .query_drop(&stmt)
                    .map_err(|e| format!("{e}: {stmt}"))?;
            }
            Ok(())
        }

        fn connect(&self) -> Box<dyn SourceConnection> {
            ots_plugin_mysql::MysqlConnector
                .connect(&ConnectParams {
                    dialect: "mysql".into(),
                    host: Some(self.live.host.clone()),
                    port: Some(self.live.port),
                    database: self.database.clone(),
                    username: Some(self.live.user.clone()),
                    password: self
                        .live
                        .password
                        .clone()
                        .map(ots_plugin_api::sources::SecretString::new),
                    read_only: true,
                    statement_timeout_ms: 10_000,
                    tls: false,
                    options: Default::default(),
                })
                .expect("mysql connects")
        }

        fn quote(&self, ident: &str) -> String {
            ots_plugin_mysql::MysqlConnector.quote_identifier(ident)
        }
    }

    impl Drop for Mysql {
        fn drop(&mut self) {
            let _ = self
                .admin
                .query_drop(format!("DROP DATABASE IF EXISTS {}", self.database));
            let _ = self
                .admin
                .exec_drop("SET GLOBAL sql_mode = ?", (self.previous_mode.clone(),));
        }
    }

    #[test]
    fn w3c_r2rml_test_cases_on_mysql() {
        let Some(dir) = corpus() else { return };
        let Some(live) = live("MYSQL", 3306, "root", "mysql") else {
            return;
        };
        let opts = mysql::OptsBuilder::new()
            .ip_or_hostname(Some(live.host.clone()))
            .tcp_port(live.port)
            .user(Some(live.user.clone()))
            .pass(live.password.clone());
        let mut admin = patiently(|| mysql::Conn::new(opts.clone()));
        let previous_mode: String = admin
            .query_first("SELECT @@GLOBAL.sql_mode")
            .expect("reading sql_mode")
            .unwrap_or_default();
        admin
            .query_drop("SET GLOBAL sql_mode = 'ANSI_QUOTES'")
            .and_then(|_| admin.query_drop("SET SESSION sql_mode = 'ANSI_QUOTES'"))
            .expect("the suite's SQL needs ANSI_QUOTES");
        let database = format!("ots_r2rml_{}", std::process::id());
        run_suite(
            &dir,
            &mut Mysql {
                live,
                database,
                admin,
                previous_mode,
            },
        );
    }
}
