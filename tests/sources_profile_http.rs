//! Per-source profile graphs over HTTP, against the SQLite connector in core.
//!
//! Acceptance checks (written before the implementation):
//! * a profile reports every column, the keys and the NULL counts the source
//!   actually holds, computed by the database rather than sampled;
//! * a low-cardinality column yields its complete, ranked code list and a
//!   high-cardinality one yields none of its values — the profile carries no
//!   row content beyond that, because the external proposer reads it;
//! * re-profiling writes a *new* version and leaves the previous one readable,
//!   so drift is the diff between two versions;
//! * the structural hash survives an INSERT and moves on an added column;
//! * both endpoints are admin-gated, like the rest of `/api/sources`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const PROF: &str = "https://w3id.org/open-triplestore/profile#";
const CSVW: &str = "http://www.w3.org/ns/csvw#";
const VOID: &str = "http://rdfs.org/ns/void#";

/// A free-text value that must never reach the profile graph.
const SENTINEL_NOTE: &str = "sentinel-note-that-must-not-be-profiled";

/// …and a document body, which is free text that repeats.
const SENTINEL_BODY: &str = "document-body-that-must-not-be-profiled";

/// The prefix label the served profile declares for [`PROF`].
const PROF_LABEL: &str = "dsprof";

/// One scratch directory per test binary: `OTS_SOURCES_DIR` is process-global.
fn sources_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("ots-profile-http-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("OTS_SOURCES_DIR", &dir);
        dir
    })
}

/// Twelve rows over a code column, a free-text column, contact addresses, a
/// numeric column and a foreign key — enough for every aggregate to differ.
fn fresh_sqlite(name: &str) -> PathBuf {
    let path = sources_dir().join(format!("{name}-{}.db", uuid::Uuid::new_v4()));
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE lookup (lookup_id INTEGER PRIMARY KEY, label TEXT NOT NULL);
         CREATE TABLE entry (
             entry_id INTEGER PRIMARY KEY,
             state TEXT NOT NULL,
             note TEXT,
             contact TEXT,
             amount REAL,
             lookup_id INTEGER REFERENCES lookup(lookup_id),
             changed_at TEXT
         );
         INSERT INTO lookup VALUES (1, 'first label'), (2, 'second label'), (3, 'third label');",
    )
    .unwrap();
    for i in 1..=12u32 {
        insert_entry(&conn, i);
    }
    path
}

fn insert_entry(conn: &rusqlite::Connection, i: u32) {
    let state = ["alpha", "beta", "gamma"][(i % 3) as usize];
    let note = match i {
        4 | 9 => "NULL".to_string(),
        7 => format!("'{SENTINEL_NOTE}'"),
        _ => format!("'free text row {i}, shared with no other row'"),
    };
    let amount = if i == 1 {
        "NULL".to_string()
    } else {
        format!("{i}.5")
    };
    conn.execute_batch(&format!(
        "INSERT INTO entry VALUES ({i}, '{state}', {note}, 'person{i}@example.invalid', \
         {amount}, {}, '2026-01-{:02}');",
        1 + (i % 3),
        (i % 28) + 1
    ))
    .unwrap();
}

/// A view whose body names a table that does not exist.
///
/// The catalogue lists it, but *every* statement that reaches into it fails —
/// so it is a table that says out loud when something touched it.
fn add_untouchable_view(db: &Path) {
    rusqlite::Connection::open(db)
        .unwrap()
        .execute_batch("CREATE VIEW excluded AS SELECT * FROM does_not_exist;")
        .unwrap();
}

/// Five distinct document bodies over a hundred rows: few enough distinct
/// values, and repetitive enough, to pass a count-only code-list test.
fn add_document_table(db: &Path) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute_batch("CREATE TABLE doc (doc_id INTEGER PRIMARY KEY, body TEXT NOT NULL);")
        .unwrap();
    for i in 0..100u32 {
        let body = format!("{SENTINEL_BODY} {} {}", i % 5, "x".repeat(4096));
        conn.execute("INSERT INTO doc (body) VALUES (?1)", [&body])
            .unwrap();
    }
}

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = if body.is_null() {
        Body::empty()
    } else {
        b = b.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

fn source_body(id: &str, db: &Path) -> Value {
    json!({
        "id": id,
        "name": format!("Source {id}"),
        "dialect": "sqlite",
        "database": db.to_string_lossy(),
        "readOnly": true,
        "statementTimeoutMs": 5000,
    })
}

async fn register(app: &Router, token: &str, id: &str, db: &Path) {
    let (st, _, txt) = req(
        app,
        Method::POST,
        "/api/sources",
        token,
        source_body(id, db),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register {id}: {txt}");
}

async fn profile(app: &Router, token: &str, id: &str) -> Value {
    let (st, v, txt) = req(
        app,
        Method::POST,
        &format!("/api/sources/{id}/profile"),
        token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "profile {id}: {txt}");
    v
}

async fn read_profile(app: &Router, token: &str, id: &str, version: Option<u32>) -> String {
    let uri = match version {
        Some(v) => format!("/api/sources/{id}/profile?version={v}"),
        None => format!("/api/sources/{id}/profile"),
    };
    let (st, _, txt) = req(app, Method::GET, &uri, token, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "read profile {id}: {txt}");
    txt
}

/// Objects of `<subject> <predicate>` in the served N-Triples-shaped Turtle.
fn objects(turtle: &str, subject: &str, predicate: &str) -> Vec<String> {
    let head = format!("<{subject}> <{predicate}> ");
    turtle
        .lines()
        .filter_map(|l| l.strip_prefix(head.as_str()))
        .map(|rest| rest.trim().trim_end_matches('.').trim().to_string())
        .collect()
}

fn object(turtle: &str, subject: &str, predicate: &str) -> Option<String> {
    objects(turtle, subject, predicate).into_iter().next()
}

fn table_iri(source: &str, table: &str) -> String {
    format!("urn:source:{source}:table:{table}")
}
fn column_iri(source: &str, table: &str, column: &str) -> String {
    format!("{}:column:{column}", table_iri(source, table))
}

fn hash_of(summary: &Value, table: &str) -> String {
    summary["tables"]
        .as_array()
        .expect("tables")
        .iter()
        .find(|t| t["table"] == table)
        .unwrap_or_else(|| panic!("no summary for '{table}' in {summary}"))["structuralHash"]
        .as_str()
        .expect("a structural hash")
        .to_string()
}

// ───────────────────────── Structure and statistics ─────────────────────────

#[tokio::test]
async fn a_profile_reports_columns_keys_and_null_counts() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("structure");
    register(&app, &token, "src", &db).await;

    let summary = profile(&app, &token, "src").await;
    assert_eq!(summary["version"], 1);
    assert_eq!(summary["source"], "urn:source:src");
    assert_eq!(summary["graph"], "urn:source:src:profile:version:1");
    assert_eq!(
        summary["activity"],
        "urn:source:src:profile:version:1:activity"
    );
    assert!(
        summary["previousVersion"].is_null(),
        "a first profile has no baseline: {summary}"
    );
    let entry = summary["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["table"] == "entry")
        .expect("the entry table");
    assert_eq!(entry["rows"], 12);
    assert_eq!(entry["columns"], 7);
    assert_eq!(entry["kind"], "table");

    let turtle = read_profile(&app, &token, "src", None).await;
    let entry_iri = table_iri("src", "entry");
    let schema = format!("{entry_iri}:schema");

    assert_eq!(
        object(&turtle, &entry_iri, &format!("{VOID}entities")),
        Some("\"12\"^^<http://www.w3.org/2001/XMLSchema#integer>".to_string()),
        "the row count is void:entities: {turtle}"
    );
    assert_eq!(
        objects(&turtle, &schema, &format!("{CSVW}primaryKey")),
        vec!["\"entry_id\"".to_string()]
    );
    let columns = objects(&turtle, &schema, &format!("{CSVW}column"));
    assert_eq!(
        columns.len(),
        7,
        "every column is in the schema: {columns:?}"
    );
    for name in [
        "entry_id",
        "state",
        "note",
        "contact",
        "amount",
        "lookup_id",
        "changed_at",
    ] {
        let c = column_iri("src", "entry", name);
        assert!(
            columns.contains(&format!("<{c}>")),
            "{name} missing from {columns:?}"
        );
        assert_eq!(
            object(&turtle, &c, &format!("{CSVW}name")),
            Some(format!("\"{name}\""))
        );
    }

    // NULL counts are the database's, not a sample's.
    let count = |column: &str, predicate: &str| -> Option<u64> {
        object(
            &turtle,
            &column_iri("src", "entry", column),
            &format!("{PROF}{predicate}"),
        )
        .and_then(|o| o.split('"').nth(1).and_then(|v| v.parse().ok()))
    };
    assert_eq!(count("note", "nullCount"), Some(2));
    assert_eq!(count("state", "nullCount"), Some(0));
    assert_eq!(count("amount", "nullCount"), Some(1));
    assert_eq!(count("state", "distinctValues"), Some(3));
    assert_eq!(count("note", "distinctValues"), Some(10));

    // Nullability is csvw:required, not a term of our own.
    assert_eq!(
        object(
            &turtle,
            &column_iri("src", "entry", "state"),
            &format!("{CSVW}required")
        ),
        Some("\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>".to_string())
    );
    assert_eq!(
        object(
            &turtle,
            &column_iri("src", "entry", "note"),
            &format!("{CSVW}required")
        ),
        Some("\"false\"^^<http://www.w3.org/2001/XMLSchema#boolean>".to_string())
    );

    // The foreign key is csvw's, and it points at the other table's node.
    let fk = object(&turtle, &schema, &format!("{CSVW}foreignKey")).expect("a foreign key");
    let fk = fk.trim_matches(|c| c == '<' || c == '>').to_string();
    assert_eq!(
        objects(&turtle, &fk, &format!("{CSVW}columnReference")),
        vec!["\"lookup_id\"".to_string()]
    );
    let reference = object(&turtle, &fk, &format!("{CSVW}reference")).expect("a reference");
    let reference = reference.trim_matches(|c| c == '<' || c == '>').to_string();
    assert_eq!(
        object(&turtle, &reference, &format!("{CSVW}resource")),
        Some(format!("<{}>", table_iri("src", "lookup")))
    );

    // The run is provenance on the datasource, with an interval and an actor.
    let (st, _, prov) = req(
        &app,
        Method::GET,
        "/api/sources/src/provenance",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let activity = "urn:source:src:profile:version:1:activity";
    assert!(
        prov.contains(&format!(
            "<{activity}> <http://www.w3.org/ns/prov#generated>"
        )),
        "the profiling activity is in the source's PROV trail: {prov}"
    );
    assert!(prov.contains("profile#Profiling"), "{prov}");
    assert!(
        prov.contains("prov#wasAssociatedWith> <http://localhost:7878/users/adm>"),
        "who ran it: {prov}"
    );
    assert!(prov.contains("prov#startedAtTime") && prov.contains("prov#endedAtTime"));
    assert!(prov.contains("datasource#durationMs"), "how long: {prov}");
}

#[tokio::test]
async fn a_code_list_carries_its_values_and_nothing_else_does() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("values");
    register(&app, &token, "src", &db).await;

    let summary = profile(&app, &token, "src").await;
    let turtle = read_profile(&app, &token, "src", None).await;

    // `state` repeats: the complete value set, ranked, with its counts.
    let state_column = column_iri("src", "entry", "state");
    let ranked = objects(&turtle, &state_column, &format!("{PROF}topValue"));
    assert_eq!(ranked.len(), 3, "the whole code list: {ranked:?}");
    let first = format!("{state_column}:value:1");
    assert!(ranked.contains(&format!("<{first}>")), "{ranked:?}");
    let values: Vec<String> = ranked
        .iter()
        .flat_map(|node| {
            objects(
                &turtle,
                node.trim_matches(|c| c == '<' || c == '>'),
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#value",
            )
        })
        .collect();
    for code in ["\"alpha\"", "\"beta\"", "\"gamma\""] {
        assert!(values.contains(&code.to_string()), "{values:?}");
    }
    assert!(
        object(&turtle, &first, &format!("{PROF}occurrences")).is_some(),
        "a code list value carries its count"
    );
    assert!(object(&turtle, &first, &format!("{PROF}rank")).is_some());

    // …and nothing that does not repeat carries a value at all.
    for column in ["note", "contact", "amount", "changed_at", "entry_id"] {
        let c = column_iri("src", "entry", column);
        assert!(
            objects(&turtle, &c, &format!("{PROF}topValue")).is_empty(),
            "{column} is not a code list"
        );
    }
    assert!(
        !turtle.contains(SENTINEL_NOTE),
        "free text reached the profile: {turtle}"
    );
    assert!(
        !turtle.contains("person7@example.invalid"),
        "a contact value reached the profile: {turtle}"
    );
    assert!(
        !turtle.contains("second label"),
        "a label from the other table reached the profile: {turtle}"
    );
    // The JSON summary reports how many code lists there are, never which
    // values they hold.
    let summary_text = summary.to_string();
    assert!(!summary_text.contains("alpha") && !summary_text.contains(SENTINEL_NOTE));

    // A numeric column is summarised; a text column is measured, not summarised.
    let amount = column_iri("src", "entry", "amount");
    for statistic in ["min", "max", "mean", "p50", "p99"] {
        assert!(
            object(&turtle, &amount, &format!("{PROF}{statistic}")).is_some(),
            "amount has no {statistic}: {turtle}"
        );
    }
    let note = column_iri("src", "entry", "note");
    for statistic in ["min", "max", "mean", "p50", "p99"] {
        assert!(
            object(&turtle, &note, &format!("{PROF}{statistic}")).is_none(),
            "free text must not be summarised numerically ({statistic})"
        );
    }
    assert!(
        object(&turtle, &note, &format!("{PROF}meanLength")).is_some(),
        "…but its length is an aggregate, not content"
    );

    // Shapes are detected from the sample, and the sample size is stated.
    assert_eq!(
        object(
            &turtle,
            &column_iri("src", "entry", "contact"),
            &format!("{PROF}pattern")
        ),
        Some(format!("<{PROF}Email>"))
    );
    assert_eq!(
        object(
            &turtle,
            &column_iri("src", "entry", "changed_at"),
            &format!("{PROF}pattern")
        ),
        Some(format!("<{PROF}Date>"))
    );
    assert_eq!(
        object(
            &turtle,
            &table_iri("src", "entry"),
            &format!("{PROF}sampledRows")
        ),
        Some("\"12\"^^<http://www.w3.org/2001/XMLSchema#integer>".to_string()),
        "a detection says which sample it came from"
    );
}

// ───────────────────────────── Versions and drift ─────────────────────────────

#[tokio::test]
async fn re_profiling_writes_a_new_version_and_keeps_the_previous_one() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("versions");
    register(&app, &token, "src", &db).await;

    let first = profile(&app, &token, "src").await;
    assert_eq!(first["version"], 1);

    let conn = rusqlite::Connection::open(&db).unwrap();
    insert_entry(&conn, 13);
    drop(conn);

    let second = profile(&app, &token, "src").await;
    assert_eq!(second["version"], 2);
    assert_eq!(
        second["previousVersion"], 1,
        "the drift baseline is the previous version: {second}"
    );

    let entities = format!("{VOID}entities");
    let newest = read_profile(&app, &token, "src", None).await;
    assert_eq!(
        object(&newest, &table_iri("src", "entry"), &entities),
        Some("\"13\"^^<http://www.w3.org/2001/XMLSchema#integer>".to_string()),
        "the newest version is served by default"
    );
    let previous = read_profile(&app, &token, "src", Some(1)).await;
    assert_eq!(
        object(&previous, &table_iri("src", "entry"), &entities),
        Some("\"12\"^^<http://www.w3.org/2001/XMLSchema#integer>".to_string()),
        "the previous version is still readable"
    );

    // A version that was never written is a 400, not an empty graph.
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/sources/src/profile?version=3",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // A source that has never been profiled says so.
    let other = fresh_sqlite("unprofiled");
    register(&app, &token, "other", &other).await;
    let (st, _, txt) = req(
        &app,
        Method::GET,
        "/api/sources/other/profile",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(txt.contains("has not been profiled"), "{txt}");
}

#[tokio::test]
async fn the_structural_hash_survives_an_insert_and_moves_on_a_new_column() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("hash");
    register(&app, &token, "src", &db).await;

    let first = profile(&app, &token, "src").await;
    let entry_hash = hash_of(&first, "entry");
    let lookup_hash = hash_of(&first, "lookup");
    assert_eq!(entry_hash.len(), 64, "a full SHA-256 in hex");
    assert_ne!(entry_hash, lookup_hash);

    let conn = rusqlite::Connection::open(&db).unwrap();
    insert_entry(&conn, 13);
    insert_entry(&conn, 14);
    drop(conn);

    let after_insert = profile(&app, &token, "src").await;
    assert_eq!(
        hash_of(&after_insert, "entry"),
        entry_hash,
        "rows are not structure"
    );

    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("ALTER TABLE entry ADD COLUMN extra TEXT;")
        .unwrap();
    drop(conn);

    let after_alter = profile(&app, &token, "src").await;
    assert_ne!(
        hash_of(&after_alter, "entry"),
        entry_hash,
        "an added column is structure"
    );
    assert_eq!(
        hash_of(&after_alter, "lookup"),
        lookup_hash,
        "a table that did not change keeps its hash, so a re-map can skip it"
    );

    // …and the hash a caller compares is the one in the graph.
    let turtle = read_profile(&app, &token, "src", None).await;
    assert_eq!(
        object(
            &turtle,
            &table_iri("src", "entry"),
            &format!("{PROF}structuralHash")
        ),
        Some(format!("\"{}\"", hash_of(&after_alter, "entry")))
    );
}

#[tokio::test]
async fn profiling_can_be_narrowed_and_an_unknown_table_is_refused() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("narrow");
    register(&app, &token, "src", &db).await;

    let (st, v, txt) = req(
        &app,
        Method::POST,
        "/api/sources/src/profile",
        &token,
        json!({ "tables": ["lookup"] }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let tables: Vec<&str> = v["tables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["table"].as_str().unwrap())
        .collect();
    assert_eq!(tables, vec!["lookup"], "only what was asked for");

    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources/src/profile",
        &token,
        json!({ "tables": ["no_such_table"] }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(txt.contains("no_such_table"), "{txt}");

    let (st, _, _) = req(
        &app,
        Method::POST,
        "/api/sources/missing/profile",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn narrowing_never_touches_a_table_it_excludes() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("narrow-cost");
    add_untouchable_view(&db);
    register(&app, &token, "src", &db).await;

    // Profiling everything reaches into the broken view, and says so.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources/src/profile",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_GATEWAY,
        "the excluded table is genuinely untouchable: {txt}"
    );

    // …so a run narrowed away from it must not read it at all — not for its
    // structure, and not for the row count full introspection takes.
    let (st, v, txt) = req(
        &app,
        Method::POST,
        "/api/sources/src/profile",
        &token,
        json!({ "tables": ["lookup"] }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CREATED,
        "a narrowed run scanned a table it excluded: {txt}"
    );
    let tables: Vec<&str> = v["tables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["table"].as_str().unwrap())
        .collect();
    assert_eq!(tables, vec!["lookup"]);

    // A name the catalogue does not list is still refused, so the cheap
    // listing has not given up the membership check.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources/src/profile",
        &token,
        json!({ "tables": ["no_such_table"] }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
}

#[tokio::test]
async fn a_repeating_document_is_not_a_code_list() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("documents");
    add_document_table(&db);
    register(&app, &token, "src", &db).await;

    let summary = profile(&app, &token, "src").await;
    let turtle = read_profile(&app, &token, "src", None).await;

    assert!(
        !turtle.contains(SENTINEL_BODY),
        "a document body left the deployment in a profile graph"
    );
    let body = column_iri("src", "doc", "body");
    assert!(
        objects(&turtle, &body, &format!("{PROF}topValue")).is_empty(),
        "a column of documents is not a value map a human maintains"
    );
    // The aggregates over it are still reported: they are not row content.
    assert_eq!(
        object(&turtle, &body, &format!("{PROF}distinctValues")),
        Some("\"5\"^^<http://www.w3.org/2001/XMLSchema#integer>".to_string()),
        "{turtle}"
    );
    assert!(object(&turtle, &body, &format!("{PROF}meanLength")).is_some());

    // …and a real code list in the same database is untouched by the ceiling.
    assert_eq!(
        objects(
            &turtle,
            &column_iri("src", "entry", "state"),
            &format!("{PROF}topValue")
        )
        .len(),
        3
    );
    let doc = summary["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["table"] == "doc")
        .expect("the doc table");
    assert_eq!(doc["codeLists"], 0, "{doc}");
}

#[tokio::test]
async fn a_malformed_request_body_is_refused_rather_than_widened() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("bad-body");
    register(&app, &token, "src", &db).await;

    // A string where the list belongs: narrowing is this endpoint's only
    // defence on a large replica, so a body it cannot read is a 400.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/sources/src/profile",
        &token,
        json!({ "tables": "entry" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");

    // Not JSON at all, likewise.
    let send = |content_type: &'static str, raw: &'static str| {
        let app = app.clone();
        let token = token.clone();
        async move {
            app.oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/sources/src/profile")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, content_type)
                    .body(Body::from(raw))
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };
    assert_eq!(
        send("application/json", "{not json").await.status(),
        StatusCode::BAD_REQUEST
    );

    // Nothing was profiled by any of them.
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/sources/src/profile",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "a refused request profiled");

    // A narrowing the server *can* read is honoured whatever the request
    // claims its type is — the failure that matters is widening, never this.
    let resp = send("text/plain", "{\"tables\": [\"lookup\"]}").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value = serde_json::from_str(&body_text(resp.into_body()).await).unwrap();
    assert_eq!(v["tables"].as_array().unwrap().len(), 1, "{v}");

    // An absent body is still the documented "profile everything".
    let all = profile(&app, &token, "src").await;
    assert_eq!(all["version"], 2);
    assert_eq!(all["tables"].as_array().unwrap().len(), 2, "{all}");
}

#[tokio::test]
async fn deleting_a_datasource_deletes_its_profiles() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let first_db = fresh_sqlite("deleted");
    register(&app, &token, "src", &first_db).await;
    profile(&app, &token, "src").await;
    assert_eq!(profile(&app, &token, "src").await["version"], 2);
    assert!(read_profile(&app, &token, "src", Some(1))
        .await
        .contains("\"alpha\""));

    let (st, _, txt) = req(
        &app,
        Method::DELETE,
        "/api/sources/src",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{txt}");

    // A different database, registered under the same id.
    let second_db = fresh_sqlite("re-registered");
    rusqlite::Connection::open(&second_db)
        .unwrap()
        .execute_batch(
            "UPDATE entry SET state = CASE state WHEN 'alpha' THEN 'one' \
             WHEN 'beta' THEN 'two' ELSE 'three' END;",
        )
        .unwrap();
    register(&app, &token, "src", &second_db).await;

    let fresh = profile(&app, &token, "src").await;
    assert_eq!(
        fresh["version"], 1,
        "a re-registered id inherited the deleted datasource's versions: {fresh}"
    );
    assert!(
        fresh["previousVersion"].is_null(),
        "…and its drift baseline: {fresh}"
    );

    let turtle = read_profile(&app, &token, "src", Some(1)).await;
    for code in ["\"one\"", "\"two\"", "\"three\""] {
        assert!(
            turtle.contains(code),
            "version 1 is this database: {turtle}"
        );
    }
    assert!(
        !turtle.contains("\"alpha\""),
        "the previous database's code list is served as this one's: {turtle}"
    );

    // …and the old version is gone rather than hidden.
    let (st, _, txt) = req(
        &app,
        Method::GET,
        "/api/sources/src/profile?version=2",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("1..1"), "{txt}");
}

#[tokio::test]
async fn the_profile_declares_a_prefix_label_the_store_does_not_already_bind() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("prefix");
    register(&app, &token, "src", &db).await;
    profile(&app, &token, "src").await;

    let turtle = read_profile(&app, &token, "src", None).await;
    assert!(
        turtle.contains(&format!("@prefix {PROF_LABEL}: <{PROF}>")),
        "{turtle}"
    );
    // `prof:` is the W3C Profiles Vocabulary in this store's prefix registry,
    // so a CURIE expanded against the registry would mean the other thing.
    assert!(
        !turtle.contains("@prefix prof:"),
        "the profile vocabulary squats a label the store already binds: {turtle}"
    );
    let bundled = open_triplestore::prefixes::dataset::PrefixDataset::bundled();
    assert_eq!(
        bundled.lookup("prof").map(|e| e.namespace.as_str()),
        Some("http://www.w3.org/ns/dx/prof/"),
        "the collision this label avoids"
    );
    assert!(
        bundled.lookup(PROF_LABEL).is_none(),
        "{PROF_LABEL} is bound to something else too"
    );
}

// ─────────────────────────────── The gate ───────────────────────────────

#[tokio::test]
async fn the_profile_endpoints_are_admin_only() {
    sources_dir();
    let (state, admin) = admin_state();
    state
        .auth_db
        .create_user(
            "usr",
            "user",
            "user@test.com",
            "hash",
            open_triplestore::auth::models::SystemRole::User,
        )
        .unwrap();
    let user = mint_token("usr", "user", "user");
    let app = test_app(state);
    let db = fresh_sqlite("gate");
    register(&app, &admin, "src", &db).await;
    profile(&app, &admin, "src").await;

    let (st, _, _) = req(
        &app,
        Method::POST,
        "/api/sources/src/profile",
        &user,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "a user may not profile a source");
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/sources/src/profile",
        &user,
        Value::Null,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "a profile holds source content; it is admin-only like the registry"
    );

    for method in [Method::GET, Method::POST] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method.clone())
                    .uri("/api/sources/src/profile")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method}");
    }
}
