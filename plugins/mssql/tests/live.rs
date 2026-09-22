//! The connector against a real server. Skipped unless
//! `OTS_TEST_MSSQL_HOST` is set; `OTS_TEST_MSSQL_PORT` (1433),
//! `OTS_TEST_MSSQL_USER` (sa), `OTS_TEST_MSSQL_PASSWORD` and
//! `OTS_TEST_MSSQL_DB` (ots_live) complete the address, and
//! `OTS_TEST_MSSQL_READER_PASSWORD` is the password the test gives the
//! read-only login it creates. The account named is expected to be an
//! administrator: the test builds its own database and a reader login.
//!
//! ```bash
//! docker run -d --rm --name ots-mssql -e ACCEPT_EULA=Y -e MSSQL_SA_PASSWORD='Str0ng!Pass' \
//!   -p 1499:1433 mcr.microsoft.com/mssql/server:2022-latest
//! OTS_TEST_MSSQL_HOST=127.0.0.1 OTS_TEST_MSSQL_PORT=1499 OTS_TEST_MSSQL_PASSWORD='Str0ng!Pass' \
//!   OTS_TEST_MSSQL_READER_PASSWORD='R3ader!Pass' cargo test -p ots-plugin-mssql --test live
//! ```

use std::collections::BTreeMap;

use ots_plugin_api::sources::{
    ConnectParams, SecretString, SourceConnector, SourceError, TableKind, ValueKind,
};
use ots_plugin_mssql::MssqlConnector;

struct Target {
    host: String,
    port: u16,
    user: String,
    password: String,
    db: String,
    reader_password: String,
}

fn target() -> Option<Target> {
    let host = std::env::var("OTS_TEST_MSSQL_HOST").ok()?;
    Some(Target {
        host,
        port: std::env::var("OTS_TEST_MSSQL_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(1433),
        user: std::env::var("OTS_TEST_MSSQL_USER").unwrap_or_else(|_| "sa".into()),
        password: std::env::var("OTS_TEST_MSSQL_PASSWORD").unwrap_or_default(),
        db: std::env::var("OTS_TEST_MSSQL_DB").unwrap_or_else(|_| "ots_live".into()),
        reader_password: std::env::var("OTS_TEST_MSSQL_READER_PASSWORD")
            .unwrap_or_else(|_| "R3ader!Pass".into()),
    })
}

fn params(t: &Target, user: &str, password: &str, timeout_ms: u64) -> ConnectParams {
    ConnectParams {
        dialect: "mssql".into(),
        host: Some(t.host.clone()),
        port: Some(t.port),
        database: t.db.clone(),
        username: Some(user.to_string()),
        password: Some(SecretString::new(password)),
        read_only: true,
        statement_timeout_ms: timeout_ms,
        tls: false,
        options: BTreeMap::new(),
    }
}

/// The fixture is built through the driver itself, in the master database,
/// by the administrator — which is also the account the read-only check
/// must refuse.
fn fixture(t: &Target) {
    let mut runtime = tokio::runtime::Builder::new_current_thread();
    let runtime = runtime.enable_all().build().unwrap();
    runtime.block_on(async {
        use tokio_util::compat::TokioAsyncWriteCompatExt;
        let mut config = tiberius::Config::new();
        config.host(&t.host);
        config.port(t.port);
        config.authentication(tiberius::AuthMethod::sql_server(&t.user, &t.password));
        config.encryption(tiberius::EncryptionLevel::NotSupported);
        let tcp = tokio::net::TcpStream::connect(config.get_addr()).await.unwrap();
        tcp.set_nodelay(true).unwrap();
        let mut client = tiberius::Client::connect(config, tcp.compat_write()).await.unwrap();
        let db = &t.db;
        let reader_password = &t.reader_password;
        for statement in [
            format!("IF DB_ID('{db}') IS NOT NULL BEGIN ALTER DATABASE [{db}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [{db}]; END"),
            format!("CREATE DATABASE [{db}]"),
            "IF SUSER_ID('ots_reader') IS NOT NULL DROP LOGIN [ots_reader]".to_string(),
            format!("CREATE LOGIN [ots_reader] WITH PASSWORD = '{reader_password}', CHECK_POLICY = OFF"),
            format!("USE [{db}]; CREATE USER [ots_reader] FOR LOGIN [ots_reader]; ALTER ROLE db_datareader ADD MEMBER [ots_reader]"),
            format!("USE [{db}]; CREATE TABLE parent (pid INT PRIMARY KEY, label NVARCHAR(20) NOT NULL)"),
            format!("USE [{db}]; EXEC sp_addextendedproperty 'MS_Description', 'the parents', 'SCHEMA', 'dbo', 'TABLE', 'parent'"),
            format!(
                "USE [{db}]; CREATE TABLE child ( \
                    cid BIGINT IDENTITY PRIMARY KEY, \
                    name NVARCHAR(40) NOT NULL DEFAULT 'x', \
                    code CHAR(2), \
                    amount DECIMAL(10,2), \
                    ratio FLOAT, \
                    ok BIT, \
                    when_at DATETIME2(3), \
                    day DATE, \
                    blob VARBINARY(16), \
                    id UNIQUEIDENTIFIER, \
                    parent_id INT, \
                    CONSTRAINT child_name_key UNIQUE (name), \
                    CONSTRAINT child_parent FOREIGN KEY (parent_id) REFERENCES parent(pid))"
            ),
            format!("USE [{db}]; EXEC sp_addextendedproperty 'MS_Description', 'the name', 'SCHEMA', 'dbo', 'TABLE', 'child', 'COLUMN', 'name'"),
            format!("USE [{db}]; EXEC('CREATE VIEW child_v AS SELECT cid, name FROM child')"),
            format!("USE [{db}]; INSERT INTO parent VALUES (1, 'p1'), (2, 'p2')"),
            format!(
                "USE [{db}]; INSERT INTO child (name, code, amount, ratio, ok, when_at, day, blob, id, parent_id) VALUES \
                   ('a', 'NL', 1.50, 2.0, 1, '2026-01-01T12:00:00', '2026-01-01', 0xDEAD, 'A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11', 1), \
                   ('b', 'BE', 9.00, 0.5, 0, NULL, NULL, NULL, NULL, 1), \
                   ('c', 'NL', NULL, NULL, NULL, NULL, NULL, NULL, NULL, 2), \
                   ('d', 'NL', 4.25, 1.0, 1, NULL, NULL, NULL, NULL, NULL)"
            ),
        ] {
            client
                .simple_query(&statement)
                .await
                .unwrap_or_else(|e| panic!("{statement}: {e}"))
                .into_results()
                .await
                .unwrap_or_else(|e| panic!("{statement}: {e}"));
        }
    });
}

#[test]
fn a_real_server_is_introspected_profiled_and_streamed_read_only() {
    let Some(t) = target() else {
        eprintln!("OTS_TEST_MSSQL_HOST unset; skipping the live test");
        return;
    };
    fixture(&t);

    // The administrator can write, so it is refused as a datasource account.
    match MssqlConnector.connect(&params(&t, &t.user, &t.password, 5_000)) {
        Err(SourceError::Config(m)) => assert!(m.contains("can write"), "{m}"),
        other => panic!(
            "a writing account must be refused, got {:?}",
            other.map(|_| ())
        ),
    }
    let reader = params(&t, "ots_reader", &t.reader_password, 5_000);
    let mut conn = MssqlConnector
        .connect(&reader)
        .expect("connect as the reader");

    let tables = conn.introspect().expect("introspect");
    let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["child", "child_v", "parent"]);
    let child = tables.iter().find(|t| t.name == "child").unwrap();
    assert_eq!(child.kind, TableKind::Table);
    assert_eq!(child.primary_key, vec!["cid"]);
    assert_eq!(child.foreign_keys.len(), 1);
    assert_eq!(child.foreign_keys[0].columns, vec!["parent_id"]);
    assert_eq!(child.foreign_keys[0].ref_table, "parent");
    assert_eq!(child.foreign_keys[0].ref_columns, vec!["pid"]);
    let name = child.columns.iter().find(|c| c.name == "name").unwrap();
    assert_eq!(name.native_type, "nvarchar(40)");
    assert_eq!(name.generic_type, ValueKind::Text);
    assert!(!name.nullable);
    assert_eq!(name.comment.as_deref(), Some("the name"));
    assert_eq!(name.default.as_deref(), Some("('x')"));
    assert_eq!(
        child
            .columns
            .iter()
            .find(|c| c.name == "ok")
            .unwrap()
            .generic_type,
        ValueKind::Boolean
    );
    assert_eq!(
        child
            .columns
            .iter()
            .find(|c| c.name == "id")
            .unwrap()
            .generic_type,
        ValueKind::Uuid
    );
    assert_eq!(
        tables.iter().find(|t| t.name == "child_v").unwrap().kind,
        TableKind::View
    );
    assert_eq!(
        tables
            .iter()
            .find(|t| t.name == "parent")
            .unwrap()
            .comment
            .as_deref(),
        Some("the parents")
    );

    assert_eq!(
        conn.unique_keys("child").unwrap(),
        vec![vec!["cid".to_string()], vec!["name".to_string()]]
    );

    let mut batches: Vec<usize> = Vec::new();
    let mut rows = Vec::new();
    let n = conn
        .stream("SELECT * FROM child", 3, &mut |b| {
            batches.push(b.len());
            rows.extend(b);
            Ok(())
        })
        .expect("stream");
    assert_eq!(n, 4);
    assert_eq!(batches, vec![3, 1]);
    rows.sort_by(|x, y| x["cid"].lexical.cmp(&y["cid"].lexical));
    let a = &rows[0];
    assert_eq!(a["cid"].lexical, "1");
    assert_eq!(a["cid"].kind, ValueKind::Integer);
    assert_eq!(a["amount"].lexical, "1.50");
    assert_eq!(a["amount"].kind, ValueKind::Decimal);
    assert_eq!(a["ratio"].lexical, "2");
    assert_eq!(a["ratio"].kind, ValueKind::Float);
    assert_eq!(a["ok"].lexical, "true");
    assert_eq!(a["ok"].kind, ValueKind::Boolean);
    assert!(
        a["when_at"].lexical.starts_with("2026-01-01T12:00:00"),
        "{}",
        a["when_at"].lexical
    );
    assert_eq!(a["when_at"].kind, ValueKind::DateTime);
    assert_eq!(a["day"].lexical, "2026-01-01");
    assert_eq!(a["blob"].lexical, "DEAD");
    assert_eq!(a["blob"].kind, ValueKind::Binary);
    assert_eq!(a["id"].lexical, "A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11");
    assert_eq!(a["id"].kind, ValueKind::Uuid);
    let b = &rows[1];
    assert!(!b.contains_key("when_at"), "NULL is an absent key");
    assert_eq!(b["ok"].lexical, "false");

    assert_eq!(
        conn.max_watermark("child", "cid").unwrap().as_deref(),
        Some("4")
    );
    assert_eq!(conn.sample("parent", 1).unwrap().len(), 1);
    assert!(conn
        .server_version()
        .unwrap()
        .unwrap()
        .contains("SQL Server"));

    let profile = conn.profile("child").expect("profile");
    assert_eq!(profile.row_count, Some(4));
    assert_eq!(profile.primary_key, vec!["cid"]);
    let code = profile.columns.iter().find(|c| c.name == "code").unwrap();
    assert_eq!(code.distinct_count, Some(2));
    assert_eq!(code.top_values[0].value, "NL");
    assert_eq!(code.top_values[0].count, 3);
    let amount = profile.columns.iter().find(|c| c.name == "amount").unwrap();
    assert_eq!(amount.null_count, Some(1));
    let numeric = amount.numeric.unwrap();
    assert_eq!(numeric.min, 1.5);
    assert_eq!(numeric.max, 9.0);
    assert_eq!(numeric.p50, 4.25);

    // The reader cannot write, and the server says so.
    let refused = conn.stream("INSERT INTO parent VALUES (9, 'nope')", 1, &mut |_| Ok(()));
    assert!(matches!(refused, Err(SourceError::Query(_))), "{refused:?}");

    // The budget bounds every statement.
    let mut short = MssqlConnector
        .connect(&params(&t, "ots_reader", &t.reader_password, 300))
        .expect("connect");
    let slow = short.stream("WAITFOR DELAY '00:00:02'; SELECT 1 AS one", 1, &mut |_| {
        Ok(())
    });
    assert!(matches!(slow, Err(SourceError::Timeout)), "{slow:?}");
}
