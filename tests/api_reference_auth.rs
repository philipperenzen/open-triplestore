//! The API reference's per-endpoint auth level, checked against the router.
//!
//! `docs/api-reference.md` states, per endpoint, whether it answers **none**
//! (anonymously, public data only), **token** (any valid session or API token)
//! or **admin**. A reader — a deployer deciding what to expose, an integrator
//! deciding what to send a token with — takes that table at its word, so it must
//! not be able to drift away from `build_router`.
//!
//! The document is embedded with `include_str!` exactly as `src/docs` embeds the
//! same file for the in-app docs, the table is parsed back out of it, and every
//! row is fired at the real router as an anonymous request: a **none** row must
//! not be refused for want of credentials, a **token** or **admin** row must
//! answer `401`. A handful of **admin** rows are additionally probed with an
//! ordinary user's token, which must be `403` — otherwise "admin" would be
//! indistinguishable from "token" here.
//!
//! Rows whose path carries a `{param}` this test cannot fill are skipped, so the
//! count of rows actually exercised is asserted: a table that stopped parsing —
//! renamed heading, reformatted column — must fail loudly rather than pass by
//! checking nothing.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::SystemRole;
use tower::ServiceExt as _;

/// The shipped reference, embedded the way `src/docs/mod.rs` embeds it, so this
/// test reads the same bytes the instance serves at `/api/docs`.
const API_REFERENCE: &str = include_str!("../docs/api-reference.md");

/// The heading the auth table lives under. Renaming it in the document without
/// renaming it here fails the row-count assertion rather than passing silently.
const AUTH_SECTION_HEADING: &str = "## Authentication — the level every endpoint needs";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    /// Answers without a token; the row says what it discloses.
    None,
    /// Any valid session or API token.
    Token,
    /// An admin or super-admin.
    Admin,
}

#[derive(Debug)]
struct Row {
    method: Method,
    path: String,
    level: Level,
}

/// Strips the Markdown emphasis a cell is written with (`` `GET` ``, `**none**`).
fn unwrap_cell(cell: &str) -> &str {
    cell.trim().trim_matches(|c| c == '`' || c == '*').trim()
}

/// Every row of the auth table, in document order.
///
/// Only the section under [`AUTH_SECTION_HEADING`] is read — the document holds
/// other Method/Path tables (the change log, replication) whose third column is
/// prose, and silently treating one of those as an unparseable auth row is
/// exactly the failure this test exists to prevent.
fn documented_rows() -> Vec<Row> {
    let section = API_REFERENCE
        .split_once(AUTH_SECTION_HEADING)
        .unwrap_or_else(|| {
            panic!("docs/api-reference.md has no \"{AUTH_SECTION_HEADING}\" section")
        })
        .1;
    let section = section
        .split_once("\n## ")
        .map(|(s, _)| s)
        .unwrap_or(section);

    let mut rows = Vec::new();
    for line in section.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').collect();
        if cells.len() < 3 {
            continue;
        }
        let level = match unwrap_cell(cells[2]) {
            "none" => Level::None,
            "token" => Level::Token,
            "admin" => Level::Admin,
            // The header row and its `|---|` separator land here, and so would a
            // row with an invented level; the row count below catches the case
            // where that swallows the whole table.
            _ => continue,
        };
        let method = Method::from_bytes(unwrap_cell(cells[0]).as_bytes())
            .unwrap_or_else(|_| panic!("row names no HTTP method: {line}"));
        let path = unwrap_cell(cells[1]).to_string();
        assert!(path.starts_with('/'), "row names no absolute path: {line}");
        rows.push(Row {
            method,
            path,
            level,
        });
    }
    rows
}

/// A body that gets the request past extraction so the auth decision is what the
/// status reports.
///
/// Most gates are middleware and run before any extractor, so an empty body is
/// enough. `POST /api/datasets` and `POST /api/organisations` are the exceptions:
/// their handlers gate themselves *after* axum has deserialised the JSON, so
/// without a well-formed body the answer would be a deserialisation `400` and
/// the row would prove nothing about auth.
fn probe_body(method: &Method, path: &str) -> Option<&'static str> {
    if method != Method::POST {
        return None;
    }
    match path {
        "/api/datasets" => Some(
            r#"{"name":"probe","owner_type":"user","owner_id":"probe","visibility":"private"}"#,
        ),
        "/api/organisations" => Some(r#"{"name":"Probe","slug":"probe"}"#),
        _ => None,
    }
}

async fn probe(app: &Router, method: &Method, path: &str, token: Option<&str>) -> StatusCode {
    let mut builder = Request::builder().method(method.clone()).uri(path);
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let request = match probe_body(method, path) {
        Some(json) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    app.clone().oneshot(request).await.unwrap().status()
}

/// An instance with an admin (created by the harness) and one ordinary account,
/// so an **admin** row can be probed with a token that is valid but not admin.
fn documented_state() -> (open_triplestore::server::AppState, String) {
    let (state, _admin_token) = admin_state();
    state
        .auth_db
        .create_user(
            "reader",
            "reader",
            "reader@test.com",
            "hash",
            SystemRole::User,
        )
        .unwrap();
    let token = mint_token("reader", "reader", "user");
    (state, token)
}

#[tokio::test]
async fn every_documented_endpoint_enforces_the_level_it_documents() {
    // The SPARQL-tier limiter buckets every request of this harness under one
    // key (no ConnectInfo, so the extractor's unidentified-client fallback
    // applies), and the table is longer than its burst. A 429 would say nothing
    // about auth, so the limiter is taken out of the picture rather than worked
    // around with sleeps.
    std::env::set_var("RATE_LIMIT_DISABLED", "1");

    let (state, _reader_token) = documented_state();
    let app = test_app(state);

    let rows = documented_rows();
    let mut checked = 0usize;
    for row in &rows {
        // A path with a placeholder needs a resource this test would have to
        // invent; the level of the collection routes beside it is the same gate.
        if row.path.contains('{') {
            continue;
        }
        let status = probe(&app, &row.method, &row.path, None).await;
        match row.level {
            Level::None => assert!(
                status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
                "{} {} is documented **none** but refuses an anonymous caller with {status}",
                row.method,
                row.path,
            ),
            Level::Token | Level::Admin => assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{} {} is documented **{}** but answers an anonymous caller {status}",
                row.method,
                row.path,
                if row.level == Level::Token {
                    "token"
                } else {
                    "admin"
                },
            ),
        }
        checked += 1;
    }

    assert!(
        checked >= 15,
        "only {checked} of {} rows were exercised — the auth table under \
         \"{AUTH_SECTION_HEADING}\" no longer parses, so this test proves nothing",
        rows.len(),
    );
}

#[tokio::test]
async fn admin_rows_refuse_an_ordinary_token() {
    std::env::set_var("RATE_LIMIT_DISABLED", "1");

    let (state, reader_token) = documented_state();
    let app = test_app(state);

    // Representatives of each admin-gated group: the legacy directory (handler
    // gate), the `/api/admin/*` router (require_admin middleware), the change log
    // and telemetry (management routes, self-gated behind optional_auth) and the
    // replication manifest. Anonymous 401 alone cannot tell **admin** from
    // **token**; this is what makes the distinction observable.
    let representatives = [
        "/api/users",
        "/api/admin/users",
        "/api/admin/telemetry",
        "/api/admin/changes",
        "/api/replication/manifest",
    ];

    let rows = documented_rows();
    for path in representatives {
        let row = rows
            .iter()
            .find(|r| r.path == path && r.method == Method::GET)
            .unwrap_or_else(|| panic!("GET {path} is no longer in the auth table"));
        assert_eq!(
            row.level,
            Level::Admin,
            "GET {path} is documented as something other than **admin**",
        );
        let status = probe(&app, &Method::GET, path, Some(&reader_token)).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "GET {path} is documented **admin** but answers an ordinary token {status}",
        );
    }
}
