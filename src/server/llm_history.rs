//! Spark chat history + user memory.
//!
//! Conversations belong to exactly one authenticated user — every query here is
//! keyed by `(id, user_id)` so one user can never read or touch another user's
//! chats. The *client* drives persistence: it creates a conversation on the
//! first send and appends each finished turn (its own message plus the
//! assistant's reply with the retrieval trail), which keeps the streaming chat
//! turn itself stateless. Guests simply never call these endpoints.
//!
//! "Memory" is one editable block of standing user preferences that the chat
//! injects into the system prompt (see `llm_sparql::user_memory_block`). It is
//! user-supplied prompt text, so [`crate::server::llm_guard`] screens it for
//! injection patterns *at save time* — a stored jailbreak would otherwise ride
//! along with every future turn.

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Extension, Json, Router,
};
use chrono::Utc;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::auth::middleware::AuthenticatedUser;

use super::error::AppError;
use super::AppState;

/// Hard caps. Oldest conversations are pruned past the per-user cap (the chat
/// is a working surface, not an archive); the per-conversation and per-message
/// caps reject instead, because silently dropping the user's own words is worse.
const MAX_CONVERSATIONS_PER_USER: usize = 200;
const MAX_MESSAGES_PER_CONVERSATION: i64 = 500;
const MAX_MESSAGE_CHARS: usize = 64_000;
const MAX_QUERIES_JSON_CHARS: usize = 256_000;
const MAX_TITLE_CHARS: usize = 120;
/// Memory is prompt text — keep it well under the model's patience.
pub const MAX_MEMORY_CHARS: usize = 4_000;

pub fn llm_history_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/llm/conversations",
            get(list_conversations).post(create_conversation),
        )
        .route(
            "/api/llm/conversations/:id",
            get(get_conversation)
                .patch(rename_conversation)
                .delete(delete_conversation),
        )
        .route("/api/llm/conversations/:id/messages", post(append_message))
        .route("/api/llm/memory", get(get_memory).put(put_memory))
}

// ─── Store ──────────────────────────────────────────────────────────────────

pub struct ChatHistoryStore {
    pool: Pool<SqliteConnectionManager>,
}

#[derive(Serialize)]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: i64,
}

#[derive(Serialize)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queries: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    pub created_at: String,
}

impl ChatHistoryStore {
    pub fn new(pool: Pool<SqliteConnectionManager>) -> Self {
        Self { pool }
    }

    fn conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>, AppError> {
        self.pool
            .get()
            .map_err(|e| AppError::Internal(format!("db pool: {e}")))
    }

    pub fn list(&self, user_id: &str) -> Result<Vec<ConversationSummary>, AppError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT c.id, c.title, c.created_at, c.updated_at,
                        (SELECT COUNT(*) FROM chat_messages m WHERE m.conversation_id = c.id)
                 FROM chat_conversations c
                 WHERE c.user_id = ?1
                 ORDER BY c.updated_at DESC",
            )
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![user_id], |r| {
                Ok(ConversationSummary {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    created_at: r.get(2)?,
                    updated_at: r.get(3)?,
                    message_count: r.get(4)?,
                })
            })
            .map_err(internal)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(internal)
    }

    pub fn create(&self, user_id: &str, title: &str) -> Result<ConversationSummary, AppError> {
        let conn = self.conn()?;
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let title = clean_title(title);
        conn.execute(
            "INSERT INTO chat_conversations (id, user_id, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, user_id, title, now],
        )
        .map_err(internal)?;
        // Prune the oldest conversations beyond the cap — bounded storage per user.
        conn.execute(
            "DELETE FROM chat_conversations WHERE user_id = ?1 AND id NOT IN (
                 SELECT id FROM chat_conversations WHERE user_id = ?1
                 ORDER BY updated_at DESC LIMIT ?2)",
            params![user_id, MAX_CONVERSATIONS_PER_USER as i64],
        )
        .map_err(internal)?;
        Ok(ConversationSummary {
            id,
            title,
            created_at: now.clone(),
            updated_at: now,
            message_count: 0,
        })
    }

    /// The conversation row, only if owned by `user_id` (404 otherwise — never
    /// 403, which would confirm the id exists).
    fn owned(&self, id: &str, user_id: &str) -> Result<(), AppError> {
        let conn = self.conn()?;
        let found: Option<String> = conn
            .query_row(
                "SELECT id FROM chat_conversations WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(internal)?;
        found
            .map(|_| ())
            .ok_or_else(|| AppError::NotFound("conversation not found".into()))
    }

    pub fn messages(&self, id: &str, user_id: &str) -> Result<Vec<StoredMessage>, AppError> {
        self.owned(id, user_id)?;
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT role, content, queries, model, stopped, created_at
                 FROM chat_messages WHERE conversation_id = ?1 ORDER BY seq",
            )
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![id], |r| {
                let queries: Option<String> = r.get(2)?;
                Ok(StoredMessage {
                    role: r.get(0)?,
                    content: r.get(1)?,
                    queries: queries.and_then(|s| serde_json::from_str(&s).ok()),
                    model: r.get(3)?,
                    stopped: r.get::<_, i64>(4)? != 0,
                    created_at: r.get(5)?,
                })
            })
            .map_err(internal)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(internal)
    }

    // One persisted chat message has this many independent, non-groupable columns
    // (identity, authorship, payload, model, stop-flag); a params struct would add
    // indirection without removing any of them.
    #[allow(clippy::too_many_arguments)]
    pub fn append(
        &self,
        id: &str,
        user_id: &str,
        role: &str,
        content: &str,
        queries: Option<&Value>,
        model: Option<&str>,
        stopped: bool,
    ) -> Result<(), AppError> {
        self.owned(id, user_id)?;
        let conn = self.conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_messages WHERE conversation_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .map_err(internal)?;
        if count >= MAX_MESSAGES_PER_CONVERSATION {
            return Err(AppError::BadRequest(
                "conversation is full — start a new chat".into(),
            ));
        }
        let queries_json = queries.map(|v| v.to_string());
        if queries_json.as_deref().map(str::len).unwrap_or(0) > MAX_QUERIES_JSON_CHARS {
            return Err(AppError::BadRequest("queries payload too large".into()));
        }
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO chat_messages (id, conversation_id, seq, role, content, queries, model, stopped, created_at)
             VALUES (?1, ?2,
                     (SELECT COALESCE(MAX(seq), 0) + 1 FROM chat_messages WHERE conversation_id = ?2),
                     ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                Uuid::new_v4().to_string(),
                id,
                role,
                content,
                queries_json,
                model,
                stopped as i64,
                now
            ],
        )
        .map_err(internal)?;
        conn.execute(
            "UPDATE chat_conversations SET updated_at = ?2,
                    title = CASE WHEN title = '' AND ?3 = 'user' THEN ?4 ELSE title END
             WHERE id = ?1",
            params![id, now, role, clean_title(content)],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn rename(&self, id: &str, user_id: &str, title: &str) -> Result<(), AppError> {
        self.owned(id, user_id)?;
        let conn = self.conn()?;
        conn.execute(
            "UPDATE chat_conversations SET title = ?3, updated_at = ?4 WHERE id = ?1 AND user_id = ?2",
            params![id, user_id, clean_title(title), Utc::now().to_rfc3339()],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn delete(&self, id: &str, user_id: &str) -> Result<(), AppError> {
        self.owned(id, user_id)?;
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM chat_conversations WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )
        .map_err(internal)?;
        Ok(())
    }

    pub fn memory(&self, user_id: &str) -> Result<(String, bool), AppError> {
        let conn = self.conn()?;
        let row: Option<(String, i64)> = conn
            .query_row(
                "SELECT instructions, enabled FROM chat_user_memory WHERE user_id = ?1",
                params![user_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(internal)?;
        Ok(row
            .map(|(i, e)| (i, e != 0))
            .unwrap_or((String::new(), true)))
    }

    pub fn set_memory(
        &self,
        user_id: &str,
        instructions: &str,
        enabled: bool,
    ) -> Result<(), AppError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO chat_user_memory (user_id, instructions, enabled, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET instructions=?2, enabled=?3, updated_at=?4",
            params![
                user_id,
                instructions,
                enabled as i64,
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(internal)?;
        Ok(())
    }

    /// The memory block ready for prompt injection: enabled and non-empty only.
    pub fn memory_for_prompt(&self, user_id: &str) -> Option<String> {
        let (instructions, enabled) = self.memory(user_id).ok()?;
        let trimmed = instructions.trim();
        (enabled && !trimmed.is_empty()).then(|| {
            let mut s: String = trimmed.chars().take(MAX_MEMORY_CHARS).collect();
            if trimmed.chars().count() > MAX_MEMORY_CHARS {
                s.push('…');
            }
            s
        })
    }
}

fn internal(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("chat history: {e}"))
}

/// First line, trimmed, capped — both for derived and user-chosen titles.
fn clean_title(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut t: String = line.trim().chars().take(MAX_TITLE_CHARS).collect();
    if line.trim().chars().count() > MAX_TITLE_CHARS {
        t.push('…');
    }
    t
}

// ─── Handlers (all behind require_auth) ─────────────────────────────────────

async fn list_conversations(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, AppError> {
    let store = ChatHistoryStore::new(state.auth_db.pool());
    let conversations = store.list(&user.user_id)?;
    Ok(Json(serde_json::json!({ "conversations": conversations })))
}

#[derive(Deserialize)]
struct CreateConversationBody {
    #[serde(default)]
    title: Option<String>,
}

async fn create_conversation(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<CreateConversationBody>,
) -> Result<Json<ConversationSummary>, AppError> {
    let store = ChatHistoryStore::new(state.auth_db.pool());
    let summary = store.create(&user.user_id, body.title.as_deref().unwrap_or(""))?;
    Ok(Json(summary))
}

async fn get_conversation(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let store = ChatHistoryStore::new(state.auth_db.pool());
    let messages = store.messages(&id, &user.user_id)?;
    Ok(Json(serde_json::json!({ "id": id, "messages": messages })))
}

#[derive(Deserialize)]
struct RenameBody {
    title: String,
}

async fn rename_conversation(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Json(body): Json<RenameBody>,
) -> Result<Json<Value>, AppError> {
    if body.title.trim().is_empty() {
        return Err(AppError::BadRequest("title must not be empty".into()));
    }
    let store = ChatHistoryStore::new(state.auth_db.pool());
    store.rename(&id, &user.user_id, &body.title)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_conversation(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let store = ChatHistoryStore::new(state.auth_db.pool());
    store.delete(&id, &user.user_id)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct AppendMessageBody {
    role: String,
    content: String,
    #[serde(default)]
    queries: Option<Value>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    stopped: bool,
}

async fn append_message(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Json(body): Json<AppendMessageBody>,
) -> Result<Json<Value>, AppError> {
    if !matches!(body.role.as_str(), "user" | "assistant") {
        return Err(AppError::BadRequest(
            "role must be user or assistant".into(),
        ));
    }
    if body.content.trim().is_empty() {
        return Err(AppError::BadRequest("content must not be empty".into()));
    }
    if body.content.chars().count() > MAX_MESSAGE_CHARS {
        return Err(AppError::BadRequest("message too long".into()));
    }
    let store = ChatHistoryStore::new(state.auth_db.pool());
    store.append(
        &id,
        &user.user_id,
        &body.role,
        &body.content,
        body.queries.as_ref(),
        body.model.as_deref(),
        body.stopped,
    )?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn get_memory(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, AppError> {
    let store = ChatHistoryStore::new(state.auth_db.pool());
    let (instructions, enabled) = store.memory(&user.user_id)?;
    Ok(Json(
        serde_json::json!({ "instructions": instructions, "enabled": enabled }),
    ))
}

#[derive(Deserialize)]
struct MemoryBody {
    instructions: String,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

async fn put_memory(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<MemoryBody>,
) -> Result<Json<Value>, AppError> {
    if body.instructions.chars().count() > MAX_MEMORY_CHARS {
        return Err(AppError::BadRequest(format!(
            "memory is capped at {MAX_MEMORY_CHARS} characters"
        )));
    }
    // Memory rides along with every future system prompt — never store text
    // that pattern-matches a prompt injection, whatever the configured action.
    if let Some(flag) = super::llm_guard::injection_pattern(&body.instructions) {
        return Err(AppError::BadRequest(format!(
            "memory looks like a prompt-injection attempt ({flag}) and was not saved"
        )));
    }
    let store = ChatHistoryStore::new(state.auth_db.pool());
    store.set_memory(&user.user_id, body.instructions.trim(), body.enabled)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::db::AuthDb;

    fn store() -> ChatHistoryStore {
        let db = AuthDb::in_memory().unwrap();
        // History rows reference users(id) — create the owning user first.
        let conn = db.pool().get().unwrap();
        conn.execute(
            "INSERT INTO users (id, username, email, password_hash, created_at, updated_at)
             VALUES ('u1', 'alice', 'a@example.org', 'x', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO users (id, username, email, password_hash, created_at, updated_at)
             VALUES ('u2', 'bob', 'b@example.org', 'x', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        ChatHistoryStore::new(db.pool())
    }

    #[test]
    fn conversation_roundtrip_with_queries_trail() {
        let s = store();
        let c = s.create("u1", "").unwrap();
        s.append("u1-id-wrong", "u1", "user", "hi", None, None, false)
            .expect_err("unknown conversation must 404");
        s.append(&c.id, "u1", "user", "How many bridges?", None, None, false)
            .unwrap();
        let trail = serde_json::json!([{ "sparql": "SELECT 1", "ok": true }]);
        s.append(
            &c.id,
            "u1",
            "assistant",
            "42 bridges.",
            Some(&trail),
            Some("qwen"),
            false,
        )
        .unwrap();
        let msgs = s.messages(&c.id, "u1").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].queries.as_ref().unwrap()[0]["ok"], true);
        // Title auto-derived from the first user message.
        let list = s.list("u1").unwrap();
        assert_eq!(list[0].title, "How many bridges?");
        assert_eq!(list[0].message_count, 2);
    }

    #[test]
    fn conversations_are_owner_scoped() {
        let s = store();
        let c = s.create("u1", "mine").unwrap();
        assert!(s.messages(&c.id, "u2").is_err(), "other user must not read");
        assert!(s.delete(&c.id, "u2").is_err(), "other user must not delete");
        assert!(s.rename(&c.id, "u2", "x").is_err());
        s.delete(&c.id, "u1").unwrap();
        assert!(s.list("u1").unwrap().is_empty());
    }

    #[test]
    fn memory_roundtrip_and_prompt_gating() {
        let s = store();
        assert_eq!(s.memory_for_prompt("u1"), None, "no memory yet");
        s.set_memory("u1", "Answer in Dutch.", true).unwrap();
        assert_eq!(
            s.memory_for_prompt("u1").as_deref(),
            Some("Answer in Dutch.")
        );
        s.set_memory("u1", "Answer in Dutch.", false).unwrap();
        assert_eq!(s.memory_for_prompt("u1"), None, "disabled memory stays out");
    }
}
