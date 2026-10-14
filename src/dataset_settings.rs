//! A dataset's opt-ins from the delta-versioning design (§5.2, §6.4).
//!
//! * `skolem` — blank nodes in RDF imported into the dataset's graphs become
//!   durable Skolem IRIs under `{base}/.well-known/genid/`, derived from the
//!   content, so re-importing the same data stops producing a spurious
//!   delete-and-add of every anonymous node, and a patch can name them.
//!   Exported data then holds no blank nodes; SPARQL `INSERT DATA` with `_:`
//!   terms still mints blank nodes.
//! * `provenance` — `off` (default), `minimal` or `full`: statement-level
//!   provenance, written by a change-log consumer into the dataset's
//!   statement-provenance graph (`crate::statement_provenance`). Needs change
//!   capture; without it nothing is recorded.
//!
//! Stored in its own small table on the identity database, created on first
//! use — the identity schema (src/auth) is not touched, as
//! `entailment::identity_policy` does.
//!
//! `GET /api/datasets/:id/settings` (readers), `PUT …/settings` (writers).

use axum::extract::{Extension, Path, State};
use axum::response::IntoResponse;
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth::db::AuthDb;
use crate::auth::middleware::AuthenticatedUser;
use crate::server::error::AppError;
use crate::server::AppState;

/// How much statement provenance a dataset records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceProfile {
    #[default]
    Off,
    /// Per change: the operation, the commit, the sequence number.
    Minimal,
    /// Minimal plus the actor and the time, for readers who cannot join the
    /// commit trail (admin-only over `/store`).
    Full,
}

impl ProvenanceProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            ProvenanceProfile::Off => "off",
            ProvenanceProfile::Minimal => "minimal",
            ProvenanceProfile::Full => "full",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(ProvenanceProfile::Off),
            "minimal" => Some(ProvenanceProfile::Minimal),
            "full" => Some(ProvenanceProfile::Full),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatasetSettings {
    #[serde(default)]
    pub skolem: bool,
    #[serde(default)]
    pub provenance: ProvenanceProfile,
}

fn ensure_table(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS dataset_settings (
            dataset_id TEXT PRIMARY KEY,
            skolem INTEGER NOT NULL DEFAULT 0,
            provenance TEXT NOT NULL DEFAULT 'off',
            updated_at TEXT NOT NULL
        )",
    )
}

/// The dataset's settings; the defaults when it has none (or on error —
/// every setting's default is the behaviour without the feature).
pub fn get(db: &AuthDb, dataset_id: &str) -> DatasetSettings {
    let read = || -> anyhow::Result<Option<DatasetSettings>> {
        let conn = db.pool().get()?;
        ensure_table(&conn)?;
        Ok(conn
            .query_row(
                "SELECT skolem, provenance FROM dataset_settings WHERE dataset_id = ?1",
                params![dataset_id],
                |r| {
                    Ok(DatasetSettings {
                        skolem: r.get::<_, i64>(0)? != 0,
                        provenance: ProvenanceProfile::parse(&r.get::<_, String>(1)?)
                            .unwrap_or_default(),
                    })
                },
            )
            .optional()?)
    };
    read().ok().flatten().unwrap_or_default()
}

pub fn set(db: &AuthDb, dataset_id: &str, s: &DatasetSettings) -> anyhow::Result<()> {
    let conn = db.pool().get()?;
    ensure_table(&conn)?;
    conn.execute(
        "INSERT INTO dataset_settings (dataset_id, skolem, provenance, updated_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(dataset_id) DO UPDATE SET skolem = excluded.skolem, provenance = excluded.provenance, \
         updated_at = excluded.updated_at",
        params![
            dataset_id,
            s.skolem as i64,
            s.provenance.as_str(),
            chrono::Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

/// Datasets that record statement provenance, with their profile.
pub fn with_provenance(db: &AuthDb) -> Vec<(String, ProvenanceProfile)> {
    let read = || -> anyhow::Result<Vec<(String, ProvenanceProfile)>> {
        let conn = db.pool().get()?;
        ensure_table(&conn)?;
        let mut stmt = conn.prepare(
            "SELECT dataset_id, provenance FROM dataset_settings WHERE provenance != 'off'",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .flatten()
            .filter_map(|(id, p)| ProvenanceProfile::parse(&p).map(|p| (id, p)))
            .collect();
        Ok(rows)
    };
    read().unwrap_or_default()
}

/// The dataset `graph` belongs to that imports with Skolem IRIs, if any.
pub fn skolem_dataset_for_graph(db: &AuthDb, graph: &str) -> bool {
    db.datasets_with_graph(graph)
        .map(|ds| ds.iter().any(|d| get(db, d).skolem))
        .unwrap_or(false)
}

fn load_dataset(
    state: &AppState,
    id: &str,
    uid: Option<&str>,
) -> Result<crate::auth::models::Dataset, AppError> {
    let ds = state
        .auth_db
        .get_dataset(id)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Dataset '{id}' not found")))?;
    if !state
        .auth_db
        .can_access_dataset(uid, &ds)
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound(format!("Dataset '{id}' not found")));
    }
    Ok(ds)
}

/// GET /api/datasets/:dataset_id/settings
pub async fn get_settings(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    load_dataset(
        &state,
        &dataset_id,
        user.as_deref().map(|u| u.user_id.as_str()),
    )?;
    let s = get(&state.auth_db, &dataset_id);
    Ok(Json(serde_json::json!({
        "dataset_id": dataset_id,
        "skolem": s.skolem,
        "provenance": s.provenance,
        "provenance_graph": crate::provenance::statement_provenance_graph(&dataset_id),
        "change_capture": state.store.changes().enabled(),
    })))
}

/// PUT /api/datasets/:dataset_id/settings — writers only. Fields left out
/// keep their value.
#[derive(Debug, Deserialize)]
pub struct SettingsBody {
    pub skolem: Option<bool>,
    pub provenance: Option<ProvenanceProfile>,
}

pub async fn put_settings(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Json(body): Json<SettingsBody>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &dataset_id, Some(&user.user_id))?;
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Forbidden(
            "Write access to this dataset required".into(),
        ));
    }
    let mut s = get(&state.auth_db, &dataset_id);
    if let Some(k) = body.skolem {
        s.skolem = k;
    }
    if let Some(p) = body.provenance {
        s.provenance = p;
    }
    set(&state.auth_db, &dataset_id, &s).map_err(|e| AppError::Internal(e.to_string()))?;
    if s.provenance != ProvenanceProfile::Off {
        crate::statement_provenance::ensure_graph(&state, &dataset_id)
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }
    Ok(Json(serde_json::json!({
        "dataset_id": dataset_id,
        "skolem": s.skolem,
        "provenance": s.provenance,
        "change_capture": state.store.changes().enabled(),
    })))
}
