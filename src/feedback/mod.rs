//! User feedback: bug reports, feature requests and questions.
//!
//! Signed-in users send a report from the in-app dialog; it lands in the
//! `feedback_reports` table (created in `AuthDb::migrate`) and the deployment's
//! admins work through it at `/admin/feedback`. Reports go to the people
//! running *this* instance, not to the upstream project.
//!
//! Visibility is backend-enforced: a reporter lists only their own reports and
//! sees their status and the admin's `admin_response`, never `admin_note`
//! (internal) or anyone else's reports. The inbox, triage and deletion are
//! admin-only. Submission needs a write-capable principal and is capped per
//! user per day on top of the per-IP rate limit the router applies.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    routing::{get, patch, post},
    Extension, Json, Router,
};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;

type ApiErr = (StatusCode, String);

/// Reports one user may file in a rolling 24 hours.
pub const DAILY_LIMIT: i64 = 20;

const TITLE_MAX: usize = 200;
const BODY_MAX: usize = 10_000;
const PAGE_MAX: usize = 500;
const USER_AGENT_MAX: usize = 300;
const TEXT_MAX: usize = 5_000;

pub const KINDS: &[&str] = &["bug", "feature", "question", "other"];
pub const STATUSES: &[&str] = &["open", "in_progress", "resolved", "closed"];

/// A report as the reporter sees it.
#[derive(Debug, Clone, Serialize)]
pub struct MyReport {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub page: Option<String>,
    pub status: String,
    pub admin_response: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A report as an admin sees it: everything, plus who sent it.
#[derive(Debug, Clone, Serialize)]
pub struct AdminReport {
    pub id: String,
    pub user_id: String,
    pub username: Option<String>,
    pub email: Option<String>,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub page: Option<String>,
    pub user_agent: Option<String>,
    pub app_version: Option<String>,
    pub status: String,
    pub admin_response: Option<String>,
    pub admin_note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub struct NewReport<'a> {
    pub user_id: &'a str,
    pub kind: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub page: Option<&'a str>,
    pub user_agent: Option<&'a str>,
    pub app_version: &'a str,
}

/// SQLite-backed feedback persistence (reuses the auth DB pool).
pub struct FeedbackStore {
    pool: Pool<SqliteConnectionManager>,
}

const MY_COLS: &str = "id, kind, title, body, page, status, admin_response, created_at, updated_at";

fn read_mine(r: &rusqlite::Row) -> rusqlite::Result<MyReport> {
    Ok(MyReport {
        id: r.get(0)?,
        kind: r.get(1)?,
        title: r.get(2)?,
        body: r.get(3)?,
        page: r.get(4)?,
        status: r.get(5)?,
        admin_response: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

const ADMIN_COLS: &str = "f.id, f.user_id, u.username, u.email, f.kind, f.title, f.body, f.page, \
     f.user_agent, f.app_version, f.status, f.admin_response, f.admin_note, f.created_at, f.updated_at";

fn read_admin(r: &rusqlite::Row) -> rusqlite::Result<AdminReport> {
    Ok(AdminReport {
        id: r.get(0)?,
        user_id: r.get(1)?,
        username: r.get(2)?,
        email: r.get(3)?,
        kind: r.get(4)?,
        title: r.get(5)?,
        body: r.get(6)?,
        page: r.get(7)?,
        user_agent: r.get(8)?,
        app_version: r.get(9)?,
        status: r.get(10)?,
        admin_response: r.get(11)?,
        admin_note: r.get(12)?,
        created_at: r.get(13)?,
        updated_at: r.get(14)?,
    })
}

impl FeedbackStore {
    pub fn new(pool: Pool<SqliteConnectionManager>) -> Self {
        Self { pool }
    }

    /// Reports `user_id` filed since `since` (RFC 3339).
    pub fn count_since(&self, user_id: &str, since: &str) -> anyhow::Result<i64> {
        let conn = self.pool.get()?;
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM feedback_reports WHERE user_id = ?1 AND created_at >= ?2",
            params![user_id, since],
            |r| r.get(0),
        )?)
    }

    pub fn create(&self, new: &NewReport) -> anyhow::Result<MyReport> {
        let conn = self.pool.get()?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO feedback_reports (id, user_id, kind, title, body, page, user_agent, app_version, status, created_at, updated_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'open',?9,?9)",
            params![
                id, new.user_id, new.kind, new.title, new.body, new.page, new.user_agent,
                new.app_version, now
            ],
        )?;
        Ok(conn.query_row(
            &format!("SELECT {MY_COLS} FROM feedback_reports WHERE id = ?1"),
            params![id],
            read_mine,
        )?)
    }

    pub fn list_mine(&self, user_id: &str) -> anyhow::Result<Vec<MyReport>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {MY_COLS} FROM feedback_reports WHERE user_id = ?1 ORDER BY created_at DESC LIMIT 200"
        ))?;
        let rows = stmt
            .query_map(params![user_id], read_mine)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The admin inbox, newest first, optionally narrowed by status and kind.
    pub fn list_all(
        &self,
        status: Option<&str>,
        kind: Option<&str>,
    ) -> anyhow::Result<Vec<AdminReport>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {ADMIN_COLS} FROM feedback_reports f LEFT JOIN users u ON u.id = f.user_id \
             WHERE (?1 IS NULL OR f.status = ?1) AND (?2 IS NULL OR f.kind = ?2) \
             ORDER BY f.created_at DESC LIMIT 500"
        ))?;
        let rows = stmt
            .query_map(params![status, kind], read_admin)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_admin(&self, id: &str) -> anyhow::Result<Option<AdminReport>> {
        let conn = self.pool.get()?;
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {ADMIN_COLS} FROM feedback_reports f LEFT JOIN users u ON u.id = f.user_id WHERE f.id = ?1"
                ),
                params![id],
                read_admin,
            )
            .optional()?)
    }

    /// Apply an admin's triage. `None` leaves a field as it is; an empty
    /// string clears a text field. `kind` re-labels a report filed under the
    /// wrong type. Returns false when no such report exists.
    pub fn update(
        &self,
        id: &str,
        kind: Option<&str>,
        status: Option<&str>,
        admin_response: Option<&str>,
        admin_note: Option<&str>,
    ) -> anyhow::Result<bool> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        let n = conn.execute(
            "UPDATE feedback_reports SET \
               kind = COALESCE(?2, kind), \
               status = COALESCE(?3, status), \
               admin_response = CASE WHEN ?4 THEN ?5 ELSE admin_response END, \
               admin_note = CASE WHEN ?6 THEN ?7 ELSE admin_note END, \
               updated_at = ?8 \
             WHERE id = ?1",
            params![
                id,
                kind,
                status,
                admin_response.is_some(),
                non_blank(admin_response),
                admin_note.is_some(),
                non_blank(admin_note),
                now
            ],
        )?;
        Ok(n > 0)
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<bool> {
        let conn = self.pool.get()?;
        Ok(conn.execute("DELETE FROM feedback_reports WHERE id = ?1", params![id])? > 0)
    }
}

/// A trimmed value, or `None` for absent or blank input.
fn non_blank(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|v| !v.is_empty())
}

// ── HTTP handlers ───────────────────────────────────────────────────────────────

fn store_of(state: &AppState) -> FeedbackStore {
    FeedbackStore::new(state.auth_db.pool())
}

fn e500<E: std::fmt::Display>(e: E) -> ApiErr {
    // Log the detail, return a generic body: raw rusqlite text can name schema.
    tracing::error!("feedback handler internal error: {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Internal server error".to_string(),
    )
}

fn bad(msg: impl Into<String>) -> ApiErr {
    (StatusCode::BAD_REQUEST, msg.into())
}

fn require_admin(user: &AuthenticatedUser) -> Result<(), ApiErr> {
    if user.is_admin() {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "Admin access required".into()))
    }
}

/// Trim, then refuse empty or over-long input. Lengths count characters.
fn required(field: &str, value: &str, max: usize) -> Result<String, ApiErr> {
    let v = value.trim();
    if v.is_empty() {
        return Err(bad(format!("{field} is required")));
    }
    if v.chars().count() > max {
        return Err(bad(format!("{field} is longer than {max} characters")));
    }
    Ok(v.to_string())
}

fn optional(field: &str, value: Option<&str>, max: usize) -> Result<Option<String>, ApiErr> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) if v.chars().count() > max => {
            Err(bad(format!("{field} is longer than {max} characters")))
        }
        Some(v) => Ok(Some(v.to_string())),
    }
}

fn check_enum<'a>(field: &str, value: &'a str, allowed: &[&str]) -> Result<&'a str, ApiErr> {
    if allowed.contains(&value) {
        Ok(value)
    } else {
        Err(bad(format!(
            "{field} must be one of: {}",
            allowed.join(", ")
        )))
    }
}

/// Clip to at most `max` characters (for values we record but don't validate).
fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[derive(Deserialize)]
pub struct ReportInput {
    pub kind: String,
    pub title: String,
    pub body: String,
    /// The in-app path the report was sent from (`/datasets/x`). Optional:
    /// the dialog lets the reporter leave it out.
    #[serde(default)]
    pub page: Option<String>,
    /// Whether to record the browser's User-Agent with the report.
    #[serde(default)]
    pub include_browser: bool,
}

pub async fn submit_report(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    headers: HeaderMap,
    Json(input): Json<ReportInput>,
) -> Result<(StatusCode, Json<MyReport>), ApiErr> {
    if !user.write_access {
        return Err((
            StatusCode::FORBIDDEN,
            "A read-only API token cannot send feedback".into(),
        ));
    }
    let kind = check_enum("kind", input.kind.trim(), KINDS)?;
    let title = required("title", &input.title, TITLE_MAX)?;
    let body = required("body", &input.body, BODY_MAX)?;
    let page = optional("page", input.page.as_deref(), PAGE_MAX)?;
    let user_agent = input
        .include_browser
        .then(|| {
            headers
                .get(header::USER_AGENT)?
                .to_str()
                .ok()
                .map(|ua| clip(ua, USER_AGENT_MAX))
        })
        .flatten();

    let store = store_of(&state);
    let since = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    if store.count_since(&user.user_id, &since).map_err(e500)? >= DAILY_LIMIT {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            format!("You can send at most {DAILY_LIMIT} reports a day — please try again tomorrow"),
        ));
    }
    let report = store
        .create(&NewReport {
            user_id: &user.user_id,
            kind,
            title: &title,
            body: &body,
            page: page.as_deref(),
            user_agent: user_agent.as_deref(),
            app_version: env!("CARGO_PKG_VERSION"),
        })
        .map_err(e500)?;
    tracing::info!(report = %report.id, kind, "feedback: new report");
    Ok((StatusCode::CREATED, Json(report)))
}

pub async fn list_my_reports(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Vec<MyReport>>, ApiErr> {
    Ok(Json(
        store_of(&state).list_mine(&user.user_id).map_err(e500)?,
    ))
}

#[derive(Deserialize)]
pub struct InboxQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

pub async fn admin_list_reports(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(q): Query<InboxQuery>,
) -> Result<Json<Vec<AdminReport>>, ApiErr> {
    require_admin(&user)?;
    let status = q.status.as_deref().filter(|s| !s.is_empty());
    let kind = q.kind.as_deref().filter(|s| !s.is_empty());
    if let Some(s) = status {
        check_enum("status", s, STATUSES)?;
    }
    if let Some(k) = kind {
        check_enum("kind", k, KINDS)?;
    }
    Ok(Json(store_of(&state).list_all(status, kind).map_err(e500)?))
}

#[derive(Deserialize)]
pub struct TriageInput {
    /// Re-label the report (bug, feature, question, other).
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// Reply shown to the reporter. An empty string clears it.
    #[serde(default)]
    pub admin_response: Option<String>,
    /// Internal note, admins only. An empty string clears it.
    #[serde(default)]
    pub admin_note: Option<String>,
}

pub async fn admin_update_report(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Json(input): Json<TriageInput>,
) -> Result<Json<AdminReport>, ApiErr> {
    require_admin(&user)?;
    if let Some(k) = input.kind.as_deref() {
        check_enum("kind", k, KINDS)?;
    }
    if let Some(s) = input.status.as_deref() {
        check_enum("status", s, STATUSES)?;
    }
    for (field, v) in [
        ("admin_response", &input.admin_response),
        ("admin_note", &input.admin_note),
    ] {
        if v.as_deref().is_some_and(|v| v.chars().count() > TEXT_MAX) {
            return Err(bad(format!("{field} is longer than {TEXT_MAX} characters")));
        }
    }
    let store = store_of(&state);
    let found = store
        .update(
            &id,
            input.kind.as_deref(),
            input.status.as_deref(),
            input.admin_response.as_deref(),
            input.admin_note.as_deref(),
        )
        .map_err(e500)?;
    if !found {
        return Err((StatusCode::NOT_FOUND, "Report not found".into()));
    }
    store
        .get_admin(&id)
        .map_err(e500)?
        .map(Json)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Report not found".into()))
}

pub async fn admin_delete_report(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiErr> {
    require_admin(&user)?;
    if store_of(&state).delete(&id).map_err(e500)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::NOT_FOUND, "Report not found".into()))
    }
}

/// Submission route. The server mounts it behind `require_auth` and its own
/// per-IP rate limit, separately from [`routes`], so reading your reports or
/// working the inbox doesn't spend the submit budget.
pub fn submit_routes() -> Router<AppState> {
    Router::new().route("/api/feedback", post(submit_report))
}

/// Reporter and admin routes. Mounted behind `require_auth`; admin is enforced
/// in-handler.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/feedback/mine", get(list_my_reports))
        .route("/api/admin/feedback", get(admin_list_reports))
        .route(
            "/api/admin/feedback/:id",
            patch(admin_update_report).delete(admin_delete_report),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::db::AuthDb;
    use crate::auth::models::SystemRole;

    fn store() -> (AuthDb, FeedbackStore) {
        let db = AuthDb::in_memory().unwrap();
        db.create_user("u1", "alice", "a@x.test", "h", SystemRole::User)
            .unwrap();
        db.create_user("u2", "bob", "b@x.test", "h", SystemRole::User)
            .unwrap();
        let s = FeedbackStore::new(db.pool());
        (db, s)
    }

    fn file(s: &FeedbackStore, user: &str, title: &str) -> MyReport {
        s.create(&NewReport {
            user_id: user,
            kind: "bug",
            title,
            body: "steps",
            page: Some("/sparql"),
            user_agent: None,
            app_version: "0.0.0",
        })
        .unwrap()
    }

    #[test]
    fn reporters_see_only_their_own_reports() {
        let (_db, s) = store();
        file(&s, "u1", "a1");
        file(&s, "u2", "b1");
        let mine = s.list_mine("u1").unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].title, "a1");
        assert_eq!(s.list_all(None, None).unwrap().len(), 2);
    }

    #[test]
    fn triage_updates_only_the_fields_given_and_blank_clears() {
        let (_db, s) = store();
        let r = file(&s, "u1", "t");
        assert!(s
            .update(
                &r.id,
                Some("feature"),
                Some("in_progress"),
                Some("Looking"),
                Some("dup of #3")
            )
            .unwrap());
        assert!(s.update(&r.id, None, None, None, Some("  ")).unwrap());
        let a = s.get_admin(&r.id).unwrap().unwrap();
        assert_eq!(a.status, "in_progress");
        assert_eq!(a.kind, "feature");
        assert_eq!(a.admin_response.as_deref(), Some("Looking"));
        assert_eq!(a.admin_note, None);
        assert_eq!(a.username.as_deref(), Some("alice"));
        assert!(!s.update("nope", None, Some("closed"), None, None).unwrap());
    }

    #[test]
    fn inbox_filters_by_status_and_kind() {
        let (_db, s) = store();
        let r = file(&s, "u1", "t1");
        file(&s, "u1", "t2");
        s.update(&r.id, None, Some("resolved"), None, None).unwrap();
        assert_eq!(s.list_all(Some("resolved"), None).unwrap().len(), 1);
        assert_eq!(s.list_all(Some("open"), Some("bug")).unwrap().len(), 1);
        assert_eq!(s.list_all(None, Some("feature")).unwrap().len(), 0);
    }

    #[test]
    fn daily_count_and_cascade_on_user_delete() {
        let (db, s) = store();
        file(&s, "u1", "t1");
        file(&s, "u1", "t2");
        let since = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
        assert_eq!(s.count_since("u1", &since).unwrap(), 2);
        db.pool()
            .get()
            .unwrap()
            .execute("DELETE FROM users WHERE id = 'u1'", [])
            .unwrap();
        assert_eq!(s.list_all(None, None).unwrap().len(), 0);
    }

    #[test]
    fn validation_trims_and_bounds() {
        assert!(required("title", "   ", 10).is_err());
        assert_eq!(required("title", "  ok ", 10).unwrap(), "ok");
        assert!(required("title", &"é".repeat(11), 10).is_err());
        assert_eq!(optional("page", Some("  "), 10).unwrap(), None);
        assert!(check_enum("kind", "spam", KINDS).is_err());
    }
}
