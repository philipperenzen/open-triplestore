//! SQLite persistence for LDES: stream configuration, the member log, and
//! sync bookmarks. Lives in the identity DB next to the dataset registry.

use rusqlite::{params, OptionalExtension};

use crate::auth::db::AuthDb;

#[derive(Debug, Clone, serde::Serialize)]
pub struct StreamConfig {
    pub enabled: bool,
    pub page_size: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Member {
    pub id: i64,
    pub dataset_id: String,
    pub entity_iri: String,
    pub graph_iri: String,
    pub created_at: String,
    pub deleted: bool,
    /// The entity's description at that moment, as N-Triples (empty for a
    /// tombstone).
    pub ntriples: String,
}

pub fn stream(db: &AuthDb, dataset_id: &str) -> anyhow::Result<Option<StreamConfig>> {
    let conn = db.pool().get()?;
    Ok(conn
        .query_row(
            "SELECT enabled, page_size, created_at FROM ldes_streams WHERE dataset_id = ?1",
            params![dataset_id],
            |r| {
                Ok(StreamConfig {
                    enabled: r.get::<_, i64>(0)? != 0,
                    page_size: r.get::<_, i64>(1)?.max(1) as u64,
                    created_at: r.get(2)?,
                })
            },
        )
        .optional()?)
}

pub fn set_stream(
    db: &AuthDb,
    dataset_id: &str,
    enabled: bool,
    page_size: u64,
) -> anyhow::Result<()> {
    let conn = db.pool().get()?;
    conn.execute(
        "INSERT INTO ldes_streams (dataset_id, enabled, page_size, created_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(dataset_id) DO UPDATE SET enabled = excluded.enabled, page_size = excluded.page_size",
        params![dataset_id, enabled as i64, page_size.max(1) as i64, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// `(graph, dataset)` pairs for the given graphs that belong to a dataset with
/// an enabled stream. The common case — no streams at all — is one indexed
/// query returning nothing.
pub fn tracked(db: &AuthDb, graphs: &[String]) -> anyhow::Result<Vec<(String, String)>> {
    if graphs.is_empty() {
        return Ok(Vec::new());
    }
    let conn = db.pool().get()?;
    let mut stmt = conn.prepare(
        "SELECT g.graph_iri, g.dataset_id FROM dataset_graphs g \
         JOIN ldes_streams s ON s.dataset_id = g.dataset_id AND s.enabled = 1 \
         WHERE g.graph_iri = ?1",
    )?;
    let mut out = Vec::new();
    for g in graphs {
        let rows = stmt.query_map(params![g], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub fn insert_member(
    db: &AuthDb,
    dataset_id: &str,
    entity_iri: &str,
    graph_iri: &str,
    created_at: &str,
    deleted: bool,
    ntriples: &str,
) -> anyhow::Result<i64> {
    let conn = db.pool().get()?;
    conn.execute(
        "INSERT INTO ldes_members (dataset_id, entity_iri, graph_iri, created_at, deleted, ntriples) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![dataset_id, entity_iri, graph_iri, created_at, deleted as i64, ntriples],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn member_count(db: &AuthDb, dataset_id: &str) -> anyhow::Result<u64> {
    let conn = db.pool().get()?;
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM ldes_members WHERE dataset_id = ?1",
        params![dataset_id],
        |r| r.get::<_, i64>(0),
    )? as u64)
}

/// Members of page `page` (1-based) in insertion order — the `page_size`
/// members after skipping `(page - 1) * page_size`.
pub fn members_page(
    db: &AuthDb,
    dataset_id: &str,
    page: u64,
    page_size: u64,
) -> anyhow::Result<Vec<Member>> {
    let conn = db.pool().get()?;
    let mut stmt = conn.prepare(
        "SELECT id, dataset_id, entity_iri, graph_iri, created_at, deleted, ntriples \
         FROM ldes_members WHERE dataset_id = ?1 ORDER BY id LIMIT ?2 OFFSET ?3",
    )?;
    let offset = page.saturating_sub(1).saturating_mul(page_size) as i64;
    let rows = stmt.query_map(params![dataset_id, page_size as i64, offset], |r| {
        Ok(Member {
            id: r.get(0)?,
            dataset_id: r.get(1)?,
            entity_iri: r.get(2)?,
            graph_iri: r.get(3)?,
            created_at: r.get(4)?,
            deleted: r.get::<_, i64>(5)? != 0,
            ntriples: r.get(6)?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// The newest member timestamp applied from `source_url` into `dataset_id`.
pub fn sync_bookmark(
    db: &AuthDb,
    dataset_id: &str,
    source_url: &str,
) -> anyhow::Result<Option<String>> {
    let conn = db.pool().get()?;
    Ok(conn
        .query_row(
            "SELECT last_timestamp FROM ldes_sync_state WHERE dataset_id = ?1 AND source_url = ?2",
            params![dataset_id, source_url],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

pub fn set_sync_bookmark(
    db: &AuthDb,
    dataset_id: &str,
    source_url: &str,
    last_timestamp: Option<&str>,
    applied: u64,
) -> anyhow::Result<()> {
    let conn = db.pool().get()?;
    conn.execute(
        "INSERT INTO ldes_sync_state (dataset_id, source_url, last_timestamp, members_applied, synced_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(dataset_id, source_url) DO UPDATE SET \
           last_timestamp = COALESCE(excluded.last_timestamp, ldes_sync_state.last_timestamp), \
           members_applied = ldes_sync_state.members_applied + excluded.members_applied, \
           synced_at = excluded.synced_at",
        params![dataset_id, source_url, last_timestamp, applied as i64, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}
