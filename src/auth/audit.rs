//! Append-only audit log.
//!
//! Every security-relevant event (auth, ACL decisions, admin actions,
//! data mutations) is recorded in the `audit_events` SQLite table. The
//! table has `BEFORE UPDATE`/`BEFORE DELETE` triggers that enforce
//! append-only semantics at the database level — the application code
//! only ever INSERTs.

use chrono::Utc;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tracing::warn;
use uuid::Uuid;

/// High-level event categories. Stored as strings so adding new variants
/// does not require a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditEventType {
    LoginSuccess,
    LoginFailure,
    Logout,
    TokenCreated,
    TokenRevoked,
    PasswordChanged,
    PasswordResetForced,
    PasswordResetRequested,
    EmailVerified,
    EmailChangeRequested,
    EmailChanged,
    UsernameReminderRequested,
    TwoFactorEnabled,
    TwoFactorDisabled,
    UserCreated,
    UserUpdated,
    UserDeleted,
    UserActivated,
    UserDeactivated,
    RoleChanged,
    SparqlUpdate,
    GraphCreated,
    GraphDeleted,
    PermissionDenied,
    AclError,
    AclGranted,
    AclRevoked,
    EndpointAclChanged,
    TripleLabelChanged,
    AlertSent,
    BackupCreated,
    BackupFailed,
    BackupVerified,
}

impl AuditEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LoginSuccess => "login_success",
            Self::LoginFailure => "login_failure",
            Self::Logout => "logout",
            Self::TokenCreated => "token_created",
            Self::TokenRevoked => "token_revoked",
            Self::PasswordChanged => "password_changed",
            Self::PasswordResetForced => "password_reset_forced",
            Self::PasswordResetRequested => "password_reset_requested",
            Self::EmailVerified => "email_verified",
            Self::EmailChangeRequested => "email_change_requested",
            Self::EmailChanged => "email_changed",
            Self::UsernameReminderRequested => "username_reminder_requested",
            Self::TwoFactorEnabled => "two_factor_enabled",
            Self::TwoFactorDisabled => "two_factor_disabled",
            Self::UserCreated => "user_created",
            Self::UserUpdated => "user_updated",
            Self::UserDeleted => "user_deleted",
            Self::UserActivated => "user_activated",
            Self::UserDeactivated => "user_deactivated",
            Self::RoleChanged => "role_changed",
            Self::SparqlUpdate => "sparql_update",
            Self::GraphCreated => "graph_created",
            Self::GraphDeleted => "graph_deleted",
            Self::PermissionDenied => "permission_denied",
            Self::AclError => "acl_error",
            Self::AclGranted => "acl_granted",
            Self::AclRevoked => "acl_revoked",
            Self::EndpointAclChanged => "endpoint_acl_changed",
            Self::TripleLabelChanged => "triple_label_changed",
            Self::AlertSent => "alert_sent",
            Self::BackupCreated => "backup_created",
            Self::BackupFailed => "backup_failed",
            Self::BackupVerified => "backup_verified",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditOutcome {
    Success,
    Failure,
    Denied,
}

impl AuditOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub timestamp: String,
    pub actor_id: Option<String>,
    pub actor_username: Option<String>,
    pub actor_role: Option<String>,
    pub event_type: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub action: Option<String>,
    pub outcome: String,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub details: Option<JsonValue>,
    pub request_id: Option<String>,
}

/// Builder for an audit event. All fields except `event_type` and `outcome`
/// are optional and may be filled in incrementally before calling
/// [`AuditLogger::log`].
#[derive(Debug, Default)]
pub struct AuditEventBuilder {
    pub actor_id: Option<String>,
    pub actor_username: Option<String>,
    pub actor_role: Option<String>,
    pub event_type: Option<AuditEventType>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub action: Option<String>,
    pub outcome: Option<AuditOutcome>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub details: Option<JsonValue>,
    pub request_id: Option<String>,
}

impl AuditEventBuilder {
    pub fn new(event_type: AuditEventType, outcome: AuditOutcome) -> Self {
        Self {
            event_type: Some(event_type),
            outcome: Some(outcome),
            ..Default::default()
        }
    }

    pub fn actor(
        mut self,
        id: impl Into<String>,
        username: impl Into<String>,
        role: impl Into<String>,
    ) -> Self {
        self.actor_id = Some(id.into());
        self.actor_username = Some(username.into());
        self.actor_role = Some(role.into());
        self
    }

    pub fn actor_id(mut self, id: impl Into<String>) -> Self {
        self.actor_id = Some(id.into());
        self
    }
    pub fn actor_username(mut self, u: impl Into<String>) -> Self {
        self.actor_username = Some(u.into());
        self
    }
    pub fn resource(mut self, ty: impl Into<String>, id: impl Into<String>) -> Self {
        self.resource_type = Some(ty.into());
        self.resource_id = Some(id.into());
        self
    }
    pub fn action(mut self, a: impl Into<String>) -> Self {
        self.action = Some(a.into());
        self
    }
    pub fn ip(mut self, ip: impl Into<String>) -> Self {
        self.ip_address = Some(ip.into());
        self
    }
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }
    pub fn details(mut self, v: JsonValue) -> Self {
        self.details = Some(v);
        self
    }
    pub fn request_id(mut self, r: impl Into<String>) -> Self {
        self.request_id = Some(r.into());
        self
    }
}

#[derive(Clone)]
pub struct AuditLogger {
    pool: Pool<SqliteConnectionManager>,
}

impl AuditLogger {
    pub fn new(pool: Pool<SqliteConnectionManager>) -> Self {
        Self { pool }
    }

    /// Insert an audit event. Failures are logged at WARN but never propagated
    /// — auditing must not break the calling request path.
    pub fn log(&self, b: AuditEventBuilder) {
        let event_type = match b.event_type {
            Some(t) => t.as_str(),
            None => {
                warn!("audit log called without event_type — dropping");
                return;
            }
        };
        let outcome = match b.outcome {
            Some(o) => o.as_str(),
            None => {
                warn!("audit log called without outcome — dropping");
                return;
            }
        };
        let id = Uuid::new_v4().to_string();
        let ts = Utc::now().to_rfc3339();
        let details_json = b.details.as_ref().map(|v| v.to_string());

        let conn = match self.pool.get() {
            Ok(c) => c,
            Err(e) => {
                warn!("audit: pool acquire failed: {e}");
                return;
            }
        };
        if let Err(e) = conn.execute(
            "INSERT INTO audit_events (id, timestamp, actor_id, actor_username, actor_role,
                                       event_type, resource_type, resource_id, action, outcome,
                                       ip_address, user_agent, details, request_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                id,
                ts,
                b.actor_id,
                b.actor_username,
                b.actor_role,
                event_type,
                b.resource_type,
                b.resource_id,
                b.action,
                outcome,
                b.ip_address,
                b.user_agent,
                details_json,
                b.request_id,
            ],
        ) {
            warn!("audit: insert failed for event_type={}: {}", event_type, e);
        }
    }

    /// Convenience for a denied permission check.
    pub fn log_denied(
        &self,
        actor_id: Option<String>,
        actor_username: Option<String>,
        resource_type: &str,
        resource_id: &str,
        action: &str,
        request_id: Option<String>,
    ) {
        let mut b = AuditEventBuilder::new(AuditEventType::PermissionDenied, AuditOutcome::Denied)
            .resource(resource_type, resource_id)
            .action(action);
        b.actor_id = actor_id;
        b.actor_username = actor_username;
        b.request_id = request_id;
        self.log(b);
    }

    /// List recent events with simple filters. Used by the admin API.
    pub fn list(
        &self,
        limit: i64,
        offset: i64,
        event_type: Option<&str>,
        actor_id: Option<&str>,
        since: Option<&str>,
    ) -> anyhow::Result<Vec<AuditEvent>> {
        let conn = self.pool.get()?;
        let mut sql = String::from(
            "SELECT id, timestamp, actor_id, actor_username, actor_role, event_type,
                    resource_type, resource_id, action, outcome, ip_address, user_agent,
                    details, request_id
             FROM audit_events WHERE 1=1",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(t) = event_type {
            sql.push_str(" AND event_type = ?");
            args.push(Box::new(t.to_string()));
        }
        if let Some(a) = actor_id {
            sql.push_str(" AND actor_id   = ?");
            args.push(Box::new(a.to_string()));
        }
        if let Some(s) = since {
            sql.push_str(" AND timestamp >= ?");
            args.push(Box::new(s.to_string()));
        }
        sql.push_str(" ORDER BY timestamp DESC LIMIT ? OFFSET ?");
        args.push(Box::new(limit));
        args.push(Box::new(offset));

        let mut stmt = conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            let details_str: Option<String> = row.get(12)?;
            let details = details_str.and_then(|s| serde_json::from_str(&s).ok());
            Ok(AuditEvent {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                actor_id: row.get(2)?,
                actor_username: row.get(3)?,
                actor_role: row.get(4)?,
                event_type: row.get(5)?,
                resource_type: row.get(6)?,
                resource_id: row.get(7)?,
                action: row.get(8)?,
                outcome: row.get(9)?,
                ip_address: row.get(10)?,
                user_agent: row.get(11)?,
                details,
                request_id: row.get(13)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

/// Best-effort extraction of client IP from typical proxy headers, falling
/// back to the connect-info socket address. None of these are authoritative;
/// they reflect what the request claimed at the audit moment.
pub fn client_ip(
    headers: &axum::http::HeaderMap,
    connect_addr: Option<std::net::SocketAddr>,
) -> Option<String> {
    if let Some(v) = headers.get("x-forwarded-for").and_then(|h| h.to_str().ok()) {
        if let Some(first) = v.split(',').next() {
            let ip = first.trim();
            if !ip.is_empty() {
                return Some(ip.to_string());
            }
        }
    }
    if let Some(v) = headers.get("x-real-ip").and_then(|h| h.to_str().ok()) {
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    connect_addr.map(|a| a.ip().to_string())
}

pub fn user_agent(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
}

pub fn request_id_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("x-request-id")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
}

// ─── GDPR / AVG pseudonymisation ─────────────────────────────────────────────

impl AuditLogger {
    /// Pseudonymise rows older than `cutoff_rfc3339`. Replaces direct PII with
    /// `PSEUDONYMISED`, zeros the IP, and rewrites `actor_id` as a SHA-256
    /// hash so events stay linkable for forensic analysis without retaining
    /// the original UUID.
    ///
    /// The append-only triggers reject `UPDATE`, so this runs over a raw
    /// connection that bypasses them by temporarily dropping and recreating
    /// them inside a transaction.
    pub fn pseudonymise_older_than(&self, cutoff_rfc3339: &str) -> anyhow::Result<usize> {
        use sha2::{Digest, Sha256};
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;

        // Collect the rows we will rewrite first (need original actor_id to hash).
        let rows: Vec<(String, Option<String>)> = {
            let mut stmt = tx.prepare(
                "SELECT id, actor_id FROM audit_events
                 WHERE timestamp < ?1
                   AND actor_username IS NOT 'PSEUDONYMISED'",
            )?;
            let mapped = stmt.query_map(params![cutoff_rfc3339], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()?
        };

        if rows.is_empty() {
            tx.commit()?;
            return Ok(0);
        }

        // Drop append-only triggers, perform UPDATEs, then recreate them.
        tx.execute_batch(
            "DROP TRIGGER IF EXISTS audit_events_no_update;
             DROP TRIGGER IF EXISTS audit_events_no_delete;",
        )?;
        let mut count = 0usize;
        for (id, actor_id) in &rows {
            let hashed = actor_id.as_deref().map(|a| {
                let mut h = Sha256::new();
                h.update(a.as_bytes());
                format!("sha256:{:x}", h.finalize())
            });
            tx.execute(
                "UPDATE audit_events
                 SET actor_id = ?2,
                     actor_username = 'PSEUDONYMISED',
                     actor_role = 'PSEUDONYMISED',
                     user_agent = 'PSEUDONYMISED',
                     ip_address = '0.0.0.0'
                 WHERE id = ?1",
                params![id, hashed],
            )?;
            count += 1;
        }
        tx.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS audit_events_no_update
                BEFORE UPDATE ON audit_events
                BEGIN SELECT RAISE(ABORT, 'audit_events is append-only'); END;
             CREATE TRIGGER IF NOT EXISTS audit_events_no_delete
                BEFORE DELETE ON audit_events
                BEGIN SELECT RAISE(ABORT, 'audit_events is append-only'); END;",
        )?;
        tx.commit()?;
        Ok(count)
    }
}

/// Spawn a Tokio task that pseudonymises audit rows older than
/// `pseudonymise_after_days` once every 24 hours. Returns immediately.
pub fn spawn_pseudonymisation_task(
    logger: std::sync::Arc<AuditLogger>,
    pseudonymise_after_days: u64,
) {
    tokio::spawn(async move {
        let day = std::time::Duration::from_secs(86_400);
        loop {
            // Run on startup, then every 24h.
            let cutoff = (chrono::Utc::now()
                - chrono::Duration::days(pseudonymise_after_days as i64))
            .to_rfc3339();
            match logger.pseudonymise_older_than(&cutoff) {
                Ok(0) => tracing::debug!(
                    "audit: pseudonymisation found nothing older than {}",
                    cutoff
                ),
                Ok(n) => tracing::info!("audit: pseudonymised {} row(s) older than {}", n, cutoff),
                Err(e) => tracing::warn!("audit: pseudonymisation failed: {}", e),
            }
            tokio::time::sleep(day).await;
        }
    });
}

// ─── HTTP handlers for admin audit endpoints ─────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub event_type: Option<String>,
    pub actor_id: Option<String>,
    pub since: Option<String>,
    pub format: Option<String>,
}

/// `GET /api/admin/audit` — paginated list of audit events. super_admin only.
pub async fn admin_list_audit(
    axum::Extension(current_user): axum::Extension<crate::auth::middleware::AuthenticatedUser>,
    axum::extract::State(logger): axum::extract::State<std::sync::Arc<AuditLogger>>,
    axum::extract::Query(q): axum::extract::Query<AuditQuery>,
) -> Result<axum::Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    if current_user.role != crate::auth::models::SystemRole::SuperAdmin {
        return Err((axum::http::StatusCode::FORBIDDEN, "super_admin only".into()));
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);
    let events = logger
        .list(
            limit,
            offset,
            q.event_type.as_deref(),
            q.actor_id.as_deref(),
            q.since.as_deref(),
        )
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(axum::Json(serde_json::json!({
        "events": events,
        "limit": limit,
        "offset": offset,
    })))
}

/// `GET /api/admin/audit/export` — full export as JSON (default) or CSV.
/// super_admin only.
pub async fn admin_export_audit(
    axum::Extension(current_user): axum::Extension<crate::auth::middleware::AuthenticatedUser>,
    axum::extract::State(logger): axum::extract::State<std::sync::Arc<AuditLogger>>,
    axum::extract::Query(q): axum::extract::Query<AuditQuery>,
) -> Result<axum::response::Response, (axum::http::StatusCode, String)> {
    use axum::response::IntoResponse;
    if current_user.role != crate::auth::models::SystemRole::SuperAdmin {
        return Err((axum::http::StatusCode::FORBIDDEN, "super_admin only".into()));
    }
    // Cap at 100k rows per export to avoid runaway memory.
    let events = logger
        .list(
            100_000,
            0,
            q.event_type.as_deref(),
            q.actor_id.as_deref(),
            q.since.as_deref(),
        )
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if q.format.as_deref() == Some("csv") {
        let mut out = String::from(
            "id,timestamp,actor_id,actor_username,actor_role,event_type,resource_type,resource_id,action,outcome,ip_address,user_agent,request_id,details\n",
        );
        for e in &events {
            // Minimal CSV escaping: wrap any field containing comma/quote/newline in quotes,
            // doubling embedded quotes.
            fn esc(s: &str) -> String {
                if s.contains(['\n', ',', '"']) {
                    format!("\"{}\"", s.replace('"', "\"\""))
                } else {
                    s.to_string()
                }
            }
            let details = e
                .details
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or_default();
            let row = [
                e.id.as_str(),
                e.timestamp.as_str(),
                e.actor_id.as_deref().unwrap_or(""),
                e.actor_username.as_deref().unwrap_or(""),
                e.actor_role.as_deref().unwrap_or(""),
                e.event_type.as_str(),
                e.resource_type.as_deref().unwrap_or(""),
                e.resource_id.as_deref().unwrap_or(""),
                e.action.as_deref().unwrap_or(""),
                e.outcome.as_str(),
                e.ip_address.as_deref().unwrap_or(""),
                e.user_agent.as_deref().unwrap_or(""),
                e.request_id.as_deref().unwrap_or(""),
                details.as_str(),
            ];
            let line: Vec<String> = row.iter().map(|f| esc(f)).collect();
            out.push_str(&line.join(","));
            out.push('\n');
        }
        Ok(([(axum::http::header::CONTENT_TYPE, "text/csv")], out).into_response())
    } else {
        Ok(axum::Json(events).into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::db::AuthDb;

    #[test]
    fn append_only_inserts_and_lists() {
        let db = AuthDb::in_memory().unwrap();
        let logger = AuditLogger::new(db.pool());
        logger.log(
            AuditEventBuilder::new(AuditEventType::LoginSuccess, AuditOutcome::Success)
                .actor("u1", "alice", "user")
                .ip("127.0.0.1"),
        );
        let events = logger.list(10, 0, None, None, None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "login_success");
        assert_eq!(events[0].actor_username.as_deref(), Some("alice"));
    }

    #[test]
    fn update_and_delete_are_blocked_by_trigger() {
        let db = AuthDb::in_memory().unwrap();
        let logger = AuditLogger::new(db.pool());
        logger.log(AuditEventBuilder::new(
            AuditEventType::Logout,
            AuditOutcome::Success,
        ));
        let conn = db.pool().get().unwrap();
        let upd = conn.execute("UPDATE audit_events SET outcome='failure'", []);
        assert!(upd.is_err(), "UPDATE on audit_events must be rejected");
        let del = conn.execute("DELETE FROM audit_events", []);
        assert!(del.is_err(), "DELETE on audit_events must be rejected");
    }
}
