//! The connector against a real server. Skipped unless
//! `OTS_TEST_MYSQL_HOST` is set; `OTS_TEST_MYSQL_PORT` (3306),
//! `OTS_TEST_MYSQL_USER` (root), `OTS_TEST_MYSQL_PASSWORD` and
//! `OTS_TEST_MYSQL_DB` (ots_live) complete the address. The account is
//! expected to be allowed to create the database: the test builds its own.
//!
//! ```bash
//! docker run -d --rm --name ots-mysql -e MYSQL_ROOT_PASSWORD=pw -p 3399:3306 mysql:8
//! OTS_TEST_MYSQL_HOST=127.0.0.1 OTS_TEST_MYSQL_PORT=3399 OTS_TEST_MYSQL_PASSWORD=pw \
//!   cargo test -p ots-plugin-mysql --test live
//! ```

use std::collections::BTreeMap;

use mysql::prelude::Queryable;
use ots_plugin_api::sources::{
    ConnectParams, SecretString, SourceConnector, SourceError, TableKind, ValueKind,
};
use ots_plugin_mysql::MysqlConnector;

struct Target {
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    db: String,
}

fn target() -> Option<Target> {
    let host = std::env::var("OTS_TEST_MYSQL_HOST").ok()?;
    Some(Target {
        host,
        port: std::env::var("OTS_TEST_MYSQL_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3306),
        user: std::env::var("OTS_TEST_MYSQL_USER").unwrap_or_else(|_| "root".into()),
        password: std::env::var("OTS_TEST_MYSQL_PASSWORD").ok(),
        db: std::env::var("OTS_TEST_MYSQL_DB").unwrap_or_else(|_| "ots_live".into()),
    })
}

fn params(t: &Target, timeout_ms: u64) -> ConnectParams {
    ConnectParams {
        dialect: "mysql".into(),
        host: Some(t.host.clone()),
        port: Some(t.port),
        database: t.db.clone(),
        username: Some(t.user.clone()),
        password: t.password.clone().map(SecretString::new),
        read_only: true,
        statement_timeout_ms: timeout_ms,
        tls: false,
        options: BTreeMap::new(),
    }
}

fn fixture(t: &Target) {
    let opts = mysql::OptsBuilder::new()
        .ip_or_hostname(Some(t.host.clone()))
        .tcp_port(t.port)
        .user(Some(t.user.clone()))
        .pass(t.password.clone());
    let mut admin = mysql::Conn::new(opts).expect("admin connection");
    let db = &t.db;
    for statement in [
        format!("DROP DATABASE IF EXISTS `{db}`"),
        format!("CREATE DATABASE `{db}`"),
        format!("USE `{db}`"),
        "CREATE TABLE parent (pid INT PRIMARY KEY, label VARCHAR(20) NOT NULL) COMMENT = 'the parents'"
            .to_string(),
        "CREATE TABLE child ( \
            cid BIGINT AUTO_INCREMENT PRIMARY KEY, \
            name VARCHAR(40) NOT NULL DEFAULT 'x' COMMENT 'the name', \
            code CHAR(2), \
            amount DECIMAL(10,2), \
            ratio DOUBLE, \
            ok TINYINT(1), \
            flags BIT(8), \
            when_at DATETIME(6), \
            day DATE, \
            blob VARBINARY(16), \
            doc JSON, \
            parent_id INT, \
            UNIQUE KEY child_name_key (name), \
            CONSTRAINT child_parent FOREIGN KEY (parent_id) REFERENCES parent(pid))"
            .to_string(),
        "CREATE VIEW child_v AS SELECT cid, name FROM child".to_string(),
        "INSERT INTO parent VALUES (1, 'p1'), (2, 'p2')".to_string(),
        "INSERT INTO child (name, code, amount, ratio, ok, flags, when_at, day, blob, doc, parent_id) VALUES \
           ('a', 'NL', 1.50, 2.0, 1, b'00000101', '2026-01-01 12:00:00', '2026-01-01', X'DEAD', '{\"k\": 1}', 1), \
           ('b', 'BE', 9.00, 0.5, 0, NULL, NULL, NULL, NULL, NULL, 1), \
           ('c', 'NL', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 2), \
           ('d', 'NL', 4.25, 1.0, 1, NULL, NULL, NULL, NULL, NULL, NULL)"
            .to_string(),
    ] {
        admin.query_drop(&statement).unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
}

#[test]
fn a_real_server_is_introspected_profiled_and_streamed_read_only() {
    let Some(t) = target() else {
        eprintln!("OTS_TEST_MYSQL_HOST unset; skipping the live test");
        return;
    };
    fixture(&t);
    let mut conn = MysqlConnector.connect(&params(&t, 5_000)).expect("connect");

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
    assert!(child
        .indexes
        .iter()
        .any(|i| i.name == "child_name_key" && i.columns == vec!["name"]));
    let name = child.columns.iter().find(|c| c.name == "name").unwrap();
    assert_eq!(name.native_type, "varchar(40)");
    assert_eq!(name.generic_type, ValueKind::Text);
    assert!(!name.nullable);
    assert_eq!(name.comment.as_deref(), Some("the name"));
    assert_eq!(name.default.as_deref(), Some("x"));
    let ok = child.columns.iter().find(|c| c.name == "ok").unwrap();
    assert_eq!(ok.generic_type, ValueKind::Boolean);
    assert_eq!(
        child
            .columns
            .iter()
            .find(|c| c.name == "flags")
            .unwrap()
            .generic_type,
        ValueKind::Integer
    );
    assert_eq!(
        child
            .columns
            .iter()
            .find(|c| c.name == "doc")
            .unwrap()
            .generic_type,
        ValueKind::Json
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
    assert_eq!(
        conn.table_names().unwrap(),
        vec!["child", "child_v", "parent"]
    );

    let mut batches: Vec<usize> = Vec::new();
    let mut rows = Vec::new();
    let n = conn
        .stream("SELECT * FROM child ORDER BY cid", 3, &mut |b| {
            batches.push(b.len());
            rows.extend(b);
            Ok(())
        })
        .expect("stream");
    assert_eq!(n, 4);
    assert_eq!(batches, vec![3, 1]);
    let a = &rows[0];
    assert_eq!(a["cid"].lexical, "1");
    assert_eq!(a["cid"].kind, ValueKind::Integer);
    assert_eq!(a["amount"].lexical, "1.50");
    assert_eq!(a["amount"].kind, ValueKind::Decimal);
    assert_eq!(a["ratio"].lexical, "2");
    assert_eq!(a["ratio"].kind, ValueKind::Float);
    assert_eq!(a["ok"].lexical, "true");
    assert_eq!(a["ok"].kind, ValueKind::Boolean);
    assert_eq!(a["flags"].lexical, "5");
    assert_eq!(a["when_at"].lexical, "2026-01-01T12:00:00");
    assert_eq!(a["when_at"].kind, ValueKind::DateTime);
    assert_eq!(a["day"].lexical, "2026-01-01");
    assert_eq!(a["blob"].lexical, "DEAD");
    assert_eq!(a["blob"].kind, ValueKind::Binary);
    assert_eq!(a["doc"].kind, ValueKind::Json);
    assert!(a["doc"].lexical.contains("\"k\""));
    assert_eq!(a["code"].lexical, "NL");
    let b = &rows[1];
    assert!(!b.contains_key("when_at"), "NULL is an absent key");
    assert_eq!(b["ok"].lexical, "false");

    assert_eq!(
        conn.max_watermark("child", "cid").unwrap().as_deref(),
        Some("4")
    );
    assert_eq!(conn.sample("parent", 1).unwrap().len(), 1);
    assert!(conn.server_version().unwrap().unwrap().starts_with("MySQL"));

    let profile = conn.profile("child").expect("profile");
    assert_eq!(profile.row_count, Some(4));
    assert_eq!(profile.primary_key, vec!["cid"]);
    assert_eq!(
        profile.unique_keys,
        vec![vec!["cid".to_string()], vec!["name".to_string()]]
    );
    let code = profile.columns.iter().find(|c| c.name == "code").unwrap();
    assert_eq!(code.distinct_count, Some(2));
    assert_eq!(code.top_values.len(), 2);
    assert_eq!(code.top_values[0].value, "NL");
    assert_eq!(code.top_values[0].count, 3);
    let amount = profile.columns.iter().find(|c| c.name == "amount").unwrap();
    assert_eq!(amount.null_count, Some(1));
    let numeric = amount.numeric.unwrap();
    assert_eq!(numeric.min, 1.5);
    assert_eq!(numeric.max, 9.0);
    assert_eq!(numeric.p50, 4.25);
    let name = profile.columns.iter().find(|c| c.name == "name").unwrap();
    assert_eq!(name.mean_length, Some(1.0));
    assert!(name.top_values.is_empty(), "a key is not a code list");

    // Read-only is the server's rule: the session refuses to write.
    let refused = conn.stream("INSERT INTO parent VALUES (9, 'nope')", 1, &mut |_| Ok(()));
    assert!(
        matches!(&refused, Err(SourceError::Query(m)) if m.to_ascii_lowercase().contains("read")),
        "{refused:?}"
    );

    // The statement budget is enforced by the server and reported as such.
    let mut short = MysqlConnector.connect(&params(&t, 300)).expect("connect");
    let slow = short.stream("SELECT SLEEP(2)", 1, &mut |_| Ok(()));
    assert!(matches!(slow, Err(SourceError::Timeout)), "{slow:?}");
    assert!(short.server_version().is_ok());

    let mut wrong = params(&t, 1_000);
    wrong.password = Some(SecretString::new("definitely-not-the-password"));
    match MysqlConnector.connect(&wrong) {
        Err(SourceError::Connect(m)) => assert!(!m.contains("definitely-not"), "{m}"),
        other => panic!("expected a connection error, got {:?}", other.map(|_| ())),
    }
}
