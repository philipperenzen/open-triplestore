//! Drift between profile versions, and the re-map tickets it opens, over HTTP.
//!
//! Acceptance checks (written before the implementation):
//! * a drift check compares the newest profile with the one the mapping was
//!   registered against, and reports a new column and a shifted code list;
//! * a finding opens one re-map ticket for the mapping, a second check
//!   updates that ticket rather than opening another, and the ticket closes
//!   explicitly;
//! * approving a mapping moves its baseline to the profile of the moment;
//! * a source with one profile version cannot drift yet, and says so.

mod common;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt as _;

fn sources_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("ots-drift-http-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("OTS_SOURCES_DIR", &dir);
        dir
    })
}

/// Twelve entries over a balanced code column.
fn fresh_sqlite(name: &str) -> PathBuf {
    let path = sources_dir().join(format!("{name}-{}.db", uuid::Uuid::new_v4()));
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE entry (entry_id INTEGER PRIMARY KEY, state TEXT NOT NULL, note TEXT);",
    )
    .unwrap();
    for i in 1..=12u32 {
        let state = ["alpha", "beta", "gamma"][(i % 3) as usize];
        conn.execute_batch(&format!(
            "INSERT INTO entry VALUES ({i}, '{state}', 'note {i}');"
        ))
        .unwrap();
    }
    path
}

/// A new column, and a code list that is now almost all one value.
fn drift_the_schema(db: &Path) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute_batch(
        "ALTER TABLE entry ADD COLUMN added TEXT;
         UPDATE entry SET state = 'alpha' WHERE entry_id <> 12;",
    )
    .unwrap();
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

async fn register_source(app: &Router, token: &str, id: &str, db: &Path) {
    let (st, _, txt) = req(
        app,
        Method::POST,
        "/api/sources",
        token,
        json!({
            "id": id, "name": id, "dialect": "sqlite",
            "database": db.to_string_lossy(), "readOnly": true, "statementTimeoutMs": 5000,
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register {id}: {txt}");
}

async fn profile(app: &Router, token: &str, id: &str) -> u32 {
    let (st, v, txt) = req(
        app,
        Method::POST,
        &format!("/api/sources/{id}/profile"),
        token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "profile {id}: {txt}");
    v["version"].as_u64().unwrap() as u32
}

fn mapping_for(source_id: &str) -> String {
    format!(
        r#"
@prefix rr:  <http://www.w3.org/ns/r2rml#> .
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix ex:  <http://example.org/> .
ex:EntryMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:{source_id}> ; rr:tableName "entry" ] ;
  rr:subjectMap [ rr:template "http://example.org/entry_{{entry_id}}" ; rr:class ex:Entry ] ;
  rr:predicateObjectMap [ rr:predicate ex:state ; rr:objectMap [ rr:column "state" ] ] .
"#
    )
}

async fn register_mapping(app: &Router, token: &str, id: &str, source_id: &str) -> Value {
    let (st, v, txt) = req(
        app,
        Method::POST,
        "/api/mappings",
        token,
        json!({ "id": id, "title": id, "rml": mapping_for(source_id) }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register mapping {id}: {txt}");
    v
}

async fn drift(
    app: &Router,
    token: &str,
    source: &str,
    body: Value,
) -> (StatusCode, Value, String) {
    req(
        app,
        Method::POST,
        &format!("/api/sources/{source}/drift"),
        token,
        body,
    )
    .await
}

#[tokio::test]
async fn drift_is_measured_against_the_mappings_baseline_and_opens_one_ticket() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("drift");
    register_source(&app, &token, "drift", &db).await;

    // Profiled once, then mapped: the mapping's baseline is version 1.
    assert_eq!(profile(&app, &token, "drift").await, 1);
    let m = register_mapping(&app, &token, "entry-map", "drift").await;
    assert_eq!(m["profileVersion"], 1, "{m}");

    // Nothing to compare yet.
    let (st, _, txt) = drift(&app, &token, "drift", json!({ "mapping": "entry-map" })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("profile it again"), "{txt}");

    // The schema and the data move; profile again.
    drift_the_schema(&db);
    assert_eq!(profile(&app, &token, "drift").await, 2);

    let (st, d, txt) = drift(&app, &token, "drift", json!({ "mapping": "entry-map" })).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(d["baseline"], 1, "the mapping's baseline: {txt}");
    assert_eq!(d["candidate"], 2, "{txt}");
    assert_eq!(d["klThreshold"], 0.1, "from the gates: {txt}");
    let entry = d["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["table"] == "entry")
        .expect("the entry table");
    assert_eq!(entry["structuralHashChanged"], true, "{txt}");
    assert_eq!(entry["newColumns"], json!(["added"]), "{txt}");
    assert!(
        entry["removedColumns"].as_array().unwrap().is_empty(),
        "{txt}"
    );
    let shifts = entry["distributionShifts"].as_array().unwrap();
    assert_eq!(shifts.len(), 1, "the code list moved: {txt}");
    assert_eq!(shifts[0]["column"], "state", "{txt}");
    assert!(shifts[0]["klDivergence"].as_f64().unwrap() > 0.1, "{txt}");
    assert_eq!(entry["affected"], true, "{txt}");
    assert_eq!(d["affectedTables"], json!(["entry"]), "{txt}");
    assert!(d["modelVersionBump"].is_null(), "no model named: {txt}");

    // One ticket, for this mapping.
    let ticket = &d["ticket"];
    assert_eq!(ticket["status"], "open", "{txt}");
    assert_eq!(ticket["reason"], "schema-drift", "{txt}");
    assert_eq!(ticket["affectedTables"], json!(["entry"]), "{txt}");
    assert_eq!(ticket["mapping"], "urn:mapping:entry-map", "{txt}");
    assert_eq!(ticket["baselineProfile"], 1, "{txt}");
    assert_eq!(ticket["candidateProfile"], 2, "{txt}");
    let ticket_id = ticket["id"].as_str().unwrap().to_string();

    // A second check updates the same ticket rather than opening another.
    let (st, d2, txt) = drift(&app, &token, "drift", json!({ "mapping": "entry-map" })).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(d2["ticket"]["id"], ticket_id, "{txt}");
    let (st, list, txt) = req(
        &app,
        Method::GET,
        "/api/sources/drift/tickets",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(list.as_array().unwrap().len(), 1, "{txt}");

    // A check that must not open a ticket does not.
    let (st, d3, txt) = drift(
        &app,
        &token,
        "drift",
        json!({ "baseline": 1, "candidate": 2, "openTicket": false }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(d3["ticket"].is_null(), "{txt}");
    assert!(d3["mapping"].is_null(), "{txt}");

    // Closing is explicit, and recorded.
    let (st, closed, txt) = req(
        &app,
        Method::POST,
        &format!("/api/tickets/{ticket_id}/close"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(closed["status"], "closed", "{txt}");
    let (st, t, _) = req(
        &app,
        Method::GET,
        &format!("/api/tickets/{ticket_id}"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(t["status"], "closed");
    let (st, _, _) = req(&app, Method::GET, "/api/tickets/nope", &token, Value::Null).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Approving the mapping re-baselines it on the newest profile, after
    // which the same two versions have to be named explicitly.
    let (st, m, txt) = req(
        &app,
        Method::PUT,
        "/api/mappings/entry-map",
        &token,
        json!({ "state": "approved" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(m["profileVersion"], 2, "{txt}");
    assert_eq!(m["version"], 1, "a state change mints no version: {txt}");
    let (st, d4, txt) = drift(&app, &token, "drift", json!({ "mapping": "entry-map" })).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(
        (d4["baseline"].as_u64(), d4["candidate"].as_u64()),
        (Some(1), Some(2)),
        "the baseline equals the newest version, so the previous one is used: {txt}"
    );

    // Deleting the datasource takes its tickets with it.
    let (st, _, txt) = req(
        &app,
        Method::DELETE,
        "/api/mappings/entry-map",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{txt}");
    let (st, _, txt) = req(
        &app,
        Method::DELETE,
        "/api/sources/drift",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{txt}");
    register_source(&app, &token, "drift", &db).await;
    let (_, list, _) = req(
        &app,
        Method::GET,
        "/api/sources/drift/tickets",
        &token,
        Value::Null,
    )
    .await;
    assert!(list.as_array().unwrap().is_empty(), "{list}");
}

#[tokio::test]
async fn the_request_is_checked() {
    sources_dir();
    let (state, token) = admin_state();
    let app = test_app(state);
    let db = fresh_sqlite("checked");
    register_source(&app, &token, "checked", &db).await;

    let (st, _, txt) = drift(&app, &token, "checked", json!({})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "not profiled: {txt}");
    assert!(txt.contains("profile"), "{txt}");

    profile(&app, &token, "checked").await;
    profile(&app, &token, "checked").await;
    let (st, _, txt) = drift(&app, &token, "checked", json!({ "baseline": 7 })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("no profile version 7"), "{txt}");
    let (st, _, txt) = drift(
        &app,
        &token,
        "checked",
        json!({ "baseline": 2, "candidate": 2 }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    let (st, _, txt) = drift(&app, &token, "checked", json!({ "mapping": "missing" })).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{txt}");
    let (st, _, _) = drift(&app, &token, "missing", json!({})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // An unchanged source between two profiles: nothing affected, no ticket.
    let (st, d, txt) = drift(&app, &token, "checked", json!({})).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(d["affectedTables"].as_array().unwrap().is_empty(), "{txt}");
    assert!(d["ticket"].is_null(), "{txt}");
}
