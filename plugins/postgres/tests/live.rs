//! The connector against a real server. Skipped unless
//! `OTS_TEST_POSTGRES_HOST` is set; `OTS_TEST_POSTGRES_PORT` (5432),
//! `OTS_TEST_POSTGRES_USER` (postgres), `OTS_TEST_POSTGRES_PASSWORD` and
//! `OTS_TEST_POSTGRES_DB` (postgres) complete the address. The account is
//! expected to own the database: the test creates its own schema.
//!
//! ```bash
//! docker run -d --rm --name ots-pg -e POSTGRES_PASSWORD=pw -p 5499:5432 postgres:16
//! OTS_TEST_POSTGRES_HOST=127.0.0.1 OTS_TEST_POSTGRES_PORT=5499 OTS_TEST_POSTGRES_PASSWORD=pw \
//!   cargo test -p ots-plugin-postgres --test live
//! ```

use std::collections::BTreeMap;

use ots_plugin_api::sources::{
    ConnectParams, SecretString, SourceConnector, SourceError, TableKind, ValueKind,
};
use ots_plugin_postgres::PostgresConnector;

struct Target {
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    db: String,
}

fn target() -> Option<Target> {
    let host = std::env::var("OTS_TEST_POSTGRES_HOST").ok()?;
    Some(Target {
        host,
        port: std::env::var("OTS_TEST_POSTGRES_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(5432),
        user: std::env::var("OTS_TEST_POSTGRES_USER").unwrap_or_else(|_| "postgres".into()),
        password: std::env::var("OTS_TEST_POSTGRES_PASSWORD").ok(),
        db: std::env::var("OTS_TEST_POSTGRES_DB").unwrap_or_else(|_| "postgres".into()),
    })
}

fn params(t: &Target, timeout_ms: u64, schema: &str) -> ConnectParams {
    ConnectParams {
        dialect: "postgresql".into(),
        host: Some(t.host.clone()),
        port: Some(t.port),
        database: t.db.clone(),
        username: Some(t.user.clone()),
        password: t.password.clone().map(SecretString::new),
        read_only: true,
        statement_timeout_ms: timeout_ms,
        tls: false,
        options: BTreeMap::from([("search_path".to_string(), schema.to_string())]),
    }
}

/// A schema of its own, so a shared server can host several runs.
fn fixture(t: &Target) -> String {
    let schema = format!("ots_live_{}", std::process::id());
    let mut admin = postgres::Config::new();
    admin.host(&t.host).port(t.port).user(&t.user).dbname(&t.db);
    if let Some(p) = &t.password {
        admin.password(p);
    }
    let mut client = admin.connect(postgres::NoTls).expect("admin connection");
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; \
             SET search_path = {schema}; \
             CREATE TABLE parent (pid INTEGER PRIMARY KEY, label TEXT NOT NULL); \
             COMMENT ON TABLE parent IS 'the parents'; \
             CREATE TABLE child ( \
                cid BIGSERIAL PRIMARY KEY, \
                name VARCHAR(40) NOT NULL DEFAULT 'x', \
                code CHAR(2), \
                amount NUMERIC(10,2), \
                ratio DOUBLE PRECISION, \
                ok BOOLEAN, \
                when_at TIMESTAMPTZ, \
                day DATE, \
                blob BYTEA, \
                tags TEXT[], \
                parent_id INTEGER REFERENCES parent(pid)); \
             COMMENT ON COLUMN child.name IS 'the name'; \
             CREATE UNIQUE INDEX child_name_key ON child(name); \
             CREATE VIEW child_v AS SELECT cid, name FROM child; \
             INSERT INTO parent VALUES (1, 'p1'), (2, 'p2'); \
             INSERT INTO child (name, code, amount, ratio, ok, when_at, day, blob, tags, parent_id) VALUES \
               ('a', 'NL', 1.50, 2.0, true, '2026-01-01T12:00:00Z', '2026-01-01', '\\xdead', ARRAY['x','y'], 1), \
               ('b', 'BE', 9.00, 0.5, false, NULL, NULL, NULL, NULL, 1), \
               ('c', 'NL', NULL, NULL, NULL, NULL, NULL, NULL, NULL, 2), \
               ('d', 'NL', 4.25, 1.0, true, NULL, NULL, NULL, NULL, NULL);"
        ))
        .expect("fixture");
    schema
}

#[test]
fn a_real_server_is_introspected_profiled_and_streamed_read_only() {
    let Some(t) = target() else {
        eprintln!("OTS_TEST_POSTGRES_HOST unset; skipping the live test");
        return;
    };
    let schema = fixture(&t);
    let p = params(&t, 5_000, &schema);
    let mut conn = PostgresConnector.connect(&p).expect("connect");

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
    assert_eq!(name.native_type, "character varying(40)");
    assert_eq!(name.generic_type, ValueKind::Text);
    assert!(!name.nullable);
    assert_eq!(name.comment.as_deref(), Some("the name"));
    assert_eq!(name.default.as_deref(), Some("'x'::character varying"));
    let amount = child.columns.iter().find(|c| c.name == "amount").unwrap();
    assert_eq!(amount.native_type, "numeric(10,2)");
    assert_eq!(amount.generic_type, ValueKind::Decimal);
    assert_eq!(
        child
            .columns
            .iter()
            .find(|c| c.name == "tags")
            .unwrap()
            .native_type,
        "_text"
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

    // Streaming: typed lexical forms, NULLs absent, batches of the asked size.
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
    assert_eq!(a["when_at"].lexical, "2026-01-01T12:00:00+00:00");
    assert_eq!(a["when_at"].kind, ValueKind::DateTime);
    assert_eq!(a["day"].lexical, "2026-01-01");
    assert_eq!(a["blob"].lexical, "DEAD");
    assert_eq!(a["blob"].kind, ValueKind::Binary);
    assert_eq!(a["tags"].lexical, "{x,y}");
    assert_eq!(a["code"].lexical, "NL");
    let b = &rows[1];
    assert!(!b.contains_key("when_at"), "NULL is an absent key");
    assert_eq!(b["ok"].lexical, "false");
    // Two columns with one name do not break the cast.
    let mut dup = Vec::new();
    conn.stream("SELECT cid, cid FROM child", 10, &mut |b| {
        dup.extend(b);
        Ok(())
    })
    .expect("duplicate names");
    assert_eq!(dup.len(), 4);

    assert_eq!(
        conn.max_watermark("child", "cid").unwrap().as_deref(),
        Some("4")
    );
    assert_eq!(conn.sample("parent", 1).unwrap().len(), 1);
    assert!(conn
        .server_version()
        .unwrap()
        .unwrap()
        .starts_with("PostgreSQL"));

    // The profile: counts by the database, a code list, a numeric summary.
    let profile = conn.profile("child").expect("profile");
    assert_eq!(profile.row_count, Some(4));
    assert_eq!(profile.primary_key, vec!["cid"]);
    assert_eq!(
        profile.unique_keys,
        vec![vec!["cid".to_string()], vec!["name".to_string()]]
    );
    let code = profile.columns.iter().find(|c| c.name == "code").unwrap();
    assert_eq!(code.distinct_count, Some(2));
    assert_eq!(code.null_count, Some(0));
    assert_eq!(code.top_values.len(), 2);
    assert_eq!(code.top_values[0].value.trim(), "NL");
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
    assert_eq!(name.pattern, None, "four values are too few for a shape");
    assert_eq!(profile.sampled_rows, 4);

    // Read-only is the server's rule, not this test's manners: the session
    // itself is read-only, and a statement that is not a query has no path
    // through the cursor at all.
    let mut setting = Vec::new();
    conn.stream(
        "SELECT current_setting('transaction_read_only') AS ro",
        1,
        &mut |b| {
            setting.extend(b);
            Ok(())
        },
    )
    .expect("setting");
    assert_eq!(setting[0]["ro"].lexical, "on");
    let refused = conn.stream("INSERT INTO parent VALUES (9, 'nope')", 1, &mut |_| Ok(()));
    assert!(matches!(refused, Err(SourceError::Query(_))), "{refused:?}");
    let refused = conn.stream("DELETE FROM parent", 1, &mut |_| Ok(()));
    assert!(matches!(refused, Err(SourceError::Query(_))), "{refused:?}");
    let mut still = Vec::new();
    conn.stream("SELECT COUNT(*) AS n FROM parent", 1, &mut |b| {
        still.extend(b);
        Ok(())
    })
    .expect("count");
    assert_eq!(still[0]["n"].lexical, "2");

    // The statement budget is enforced by the server and reported as such.
    let mut short = PostgresConnector
        .connect(&params(&t, 300, &schema))
        .expect("connect");
    let slow = short.stream("SELECT pg_sleep(2)", 1, &mut |_| Ok(()));
    assert!(matches!(slow, Err(SourceError::Timeout)), "{slow:?}");
    // …and the session is still usable after it.
    assert!(short.server_version().is_ok());

    // A wrong password is a connection error that names no secret.
    let mut wrong = params(&t, 1_000, &schema);
    wrong.password = Some(SecretString::new("definitely-not-the-password"));
    match PostgresConnector.connect(&wrong) {
        Err(SourceError::Connect(m)) => assert!(!m.contains("definitely-not"), "{m}"),
        other => panic!("expected a connection error, got {:?}", other.map(|_| ())),
    }
}
