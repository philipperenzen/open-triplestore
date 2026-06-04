//! SQLite persistence for saved queries, their revisions, and test history.
//!
//! Reuses the auth database's connection pool (like `AuditLogger` does) rather
//! than owning a second one; the tables themselves are created centrally in
//! `AuthDb::migrate`.

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use super::models::*;

const Q_COLS: &str = "id, owner_type, owner_id, name, slug, description, current_revision, \
    parameters, test_parameters, visibility, is_active, created_by, created_at, updated_at";

const T_COLS: &str = "id, query_id, revision, dataset_id, dataset_version, prev_version, status, \
    result_hash, result_rowcount, error_message, acknowledged, acknowledged_by, acknowledged_at, created_at";

/// Saved-query persistence handle. Cheap to construct (`Pool` clone).
pub struct SavedQueryStore {
    pool: Pool<SqliteConnectionManager>,
}

fn read_query(row: &rusqlite::Row) -> rusqlite::Result<SavedQuery> {
    let scope_str: String = row.get(1)?;
    let params_str: String = row
        .get::<_, Option<String>>(7)?
        .unwrap_or_else(|| "[]".to_string());
    let test_params_str: Option<String> = row.get(8)?;
    Ok(SavedQuery {
        id: row.get(0)?,
        scope: QueryScope::from_str(&scope_str).unwrap_or(QueryScope::Dataset),
        owner_id: row.get(2)?,
        name: row.get(3)?,
        slug: row.get(4)?,
        description: row.get(5)?,
        current_revision: row.get(6)?,
        parameters: serde_json::from_str(&params_str).unwrap_or_default(),
        test_parameters: test_params_str.and_then(|s| serde_json::from_str(&s).ok()),
        visibility: row.get(9)?,
        is_active: row.get::<_, i32>(10)? != 0,
        created_by: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        sparql: None,
    })
}

fn read_test(row: &rusqlite::Row) -> rusqlite::Result<QueryTest> {
    Ok(QueryTest {
        id: row.get(0)?,
        query_id: row.get(1)?,
        revision: row.get(2)?,
        dataset_id: row.get(3)?,
        dataset_version: row.get(4)?,
        prev_version: row.get(5)?,
        status: row.get(6)?,
        result_hash: row.get(7)?,
        result_rowcount: row.get(8)?,
        error_message: row.get(9)?,
        acknowledged: row.get::<_, i32>(10)? != 0,
        acknowledged_by: row.get(11)?,
        acknowledged_at: row.get(12)?,
        created_at: row.get(13)?,
    })
}

fn revision_text(conn: &Connection, id: &str, rev: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT sparql FROM saved_query_revisions WHERE query_id=?1 AND revision=?2",
        params![id, rev],
        |row| row.get(0),
    )
    .optional()
}

impl SavedQueryStore {
    pub fn new(pool: Pool<SqliteConnectionManager>) -> Self {
        Self { pool }
    }

    fn ensure_unique_slug(
        &self,
        scope: QueryScope,
        owner_id: &str,
        base: &str,
    ) -> anyhow::Result<String> {
        let conn = self.pool.get()?;
        let mut candidate = base.to_string();
        let mut n = 1;
        loop {
            let taken = conn
                .query_row(
                    "SELECT 1 FROM saved_queries WHERE owner_type=?1 AND owner_id=?2 AND slug=?3 LIMIT 1",
                    params![scope.as_str(), owner_id, candidate],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            if !taken {
                return Ok(candidate);
            }
            n += 1;
            candidate = format!("{base}-{n}");
        }
    }

    pub fn create(
        &self,
        scope: QueryScope,
        owner_id: &str,
        req: &CreateSavedQueryRequest,
        created_by: &str,
    ) -> anyhow::Result<SavedQuery> {
        let base_slug = req
            .slug
            .as_deref()
            .map(slugify)
            .unwrap_or_else(|| slugify(&req.name));
        let slug = self.ensure_unique_slug(scope, owner_id, &base_slug)?;

        let conn = self.pool.get()?;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let params_json = serde_json::to_string(&req.parameters).unwrap_or_else(|_| "[]".to_string());
        let test_params_json = req.test_parameters.as_ref().map(|v| v.to_string());

        conn.execute(
            "INSERT INTO saved_queries \
             (id, owner_type, owner_id, name, slug, description, current_revision, parameters, \
              test_parameters, visibility, is_active, created_by, created_at, updated_at) \
             VALUES (?1,?2,?3,?4,?5,?6,1,?7,?8,?9,1,?10,?11,?11)",
            params![
                id,
                scope.as_str(),
                owner_id,
                req.name,
                slug,
                req.description,
                params_json,
                test_params_json,
                req.visibility,
                created_by,
                now
            ],
        )?;
        conn.execute(
            "INSERT INTO saved_query_revisions (query_id, revision, name, sparql, note, origin, created_by, created_at) \
             VALUES (?1,1,?2,?3,?4,'manual',?5,?6)",
            params![id, req.version_name, req.sparql, req.note, created_by, now],
        )?;

        let mut sq = read_query_by_id(&conn, &id)?
            .ok_or_else(|| anyhow::anyhow!("saved query vanished after insert"))?;
        sq.sparql = Some(req.sparql.clone());
        Ok(sq)
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Option<SavedQuery>> {
        let conn = self.pool.get()?;
        let mut q = read_query_by_id(&conn, id)?;
        if let Some(ref mut sq) = q {
            sq.sparql = revision_text(&conn, id, sq.current_revision)?;
        }
        Ok(q)
    }

    pub fn get_by_slug(
        &self,
        scope: QueryScope,
        owner_id: &str,
        slug: &str,
    ) -> anyhow::Result<Option<SavedQuery>> {
        let conn = self.pool.get()?;
        let mut q = conn
            .query_row(
                &format!(
                    "SELECT {Q_COLS} FROM saved_queries WHERE owner_type=?1 AND owner_id=?2 AND slug=?3"
                ),
                params![scope.as_str(), owner_id, slug],
                read_query,
            )
            .optional()?;
        if let Some(ref mut sq) = q {
            let id = sq.id.clone();
            sq.sparql = revision_text(&conn, &id, sq.current_revision)?;
        }
        Ok(q)
    }

    /// List a scope's saved queries (without bodies — list view).
    pub fn list(&self, scope: QueryScope, owner_id: &str) -> anyhow::Result<Vec<SavedQuery>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {Q_COLS} FROM saved_queries WHERE owner_type=?1 AND owner_id=?2 ORDER BY name"
        ))?;
        let rows = stmt
            .query_map(params![scope.as_str(), owner_id], read_query)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Active dataset-scoped queries with their head SPARQL filled in — used by
    /// the version-bump test runner.
    pub fn list_active_dataset_queries(&self, dataset_id: &str) -> anyhow::Result<Vec<SavedQuery>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {Q_COLS} FROM saved_queries \
             WHERE owner_type='dataset' AND owner_id=?1 AND is_active=1 ORDER BY name"
        ))?;
        let mut rows = stmt
            .query_map(params![dataset_id], read_query)?
            .collect::<Result<Vec<_>, _>>()?;
        for sq in &mut rows {
            sq.sparql = revision_text(&conn, &sq.id, sq.current_revision)?;
        }
        Ok(rows)
    }

    pub fn update(
        &self,
        id: &str,
        req: &UpdateSavedQueryRequest,
        user: &str,
    ) -> anyhow::Result<Option<SavedQuery>> {
        {
            let conn = self.pool.get()?;
            let now = chrono::Utc::now().to_rfc3339();
            if let Some(ref name) = req.name {
                conn.execute(
                    "UPDATE saved_queries SET name=?1, updated_at=?2 WHERE id=?3",
                    params![name, now, id],
                )?;
            }
            if req.description.is_some() {
                conn.execute(
                    "UPDATE saved_queries SET description=?1, updated_at=?2 WHERE id=?3",
                    params![req.description, now, id],
                )?;
            }
            if let Some(ref ps) = req.parameters {
                let json = serde_json::to_string(ps).unwrap_or_else(|_| "[]".to_string());
                conn.execute(
                    "UPDATE saved_queries SET parameters=?1, updated_at=?2 WHERE id=?3",
                    params![json, now, id],
                )?;
            }
            if let Some(ref tp) = req.test_parameters {
                conn.execute(
                    "UPDATE saved_queries SET test_parameters=?1, updated_at=?2 WHERE id=?3",
                    params![tp.to_string(), now, id],
                )?;
            }
            if req.visibility.is_some() {
                conn.execute(
                    "UPDATE saved_queries SET visibility=?1, updated_at=?2 WHERE id=?3",
                    params![req.visibility, now, id],
                )?;
            }
            if let Some(active) = req.is_active {
                conn.execute(
                    "UPDATE saved_queries SET is_active=?1, updated_at=?2 WHERE id=?3",
                    params![active as i32, now, id],
                )?;
            }
        }
        if let Some(ref sparql) = req.sparql {
            self.add_revision(id, sparql, req.version_name.as_deref(), req.note.as_deref(), "manual", user)?;
        }
        self.get(id)
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM saved_queries WHERE id=?1", params![id])?;
        Ok(())
    }

    /// Append a new revision and advance `current_revision`. Returns the new
    /// revision number. `name` is an optional commit-style custom version title.
    pub fn add_revision(
        &self,
        id: &str,
        sparql: &str,
        name: Option<&str>,
        note: Option<&str>,
        origin: &str,
        created_by: &str,
    ) -> anyhow::Result<i64> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        let next: i64 = conn.query_row(
            "SELECT COALESCE(MAX(revision),0)+1 FROM saved_query_revisions WHERE query_id=?1",
            params![id],
            |row| row.get(0),
        )?;
        conn.execute(
            "INSERT INTO saved_query_revisions (query_id, revision, name, sparql, note, origin, created_by, created_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![id, next, name, sparql, note, origin, created_by, now],
        )?;
        conn.execute(
            "UPDATE saved_queries SET current_revision=?1, updated_at=?2 WHERE id=?3",
            params![next, now, id],
        )?;
        Ok(next)
    }

    pub fn list_revisions(&self, id: &str) -> anyhow::Result<Vec<SavedQueryRevision>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT query_id, revision, name, sparql, note, origin, created_by, created_at \
             FROM saved_query_revisions WHERE query_id=?1 ORDER BY revision DESC",
        )?;
        let rows = stmt
            .query_map(params![id], |row| {
                Ok(SavedQueryRevision {
                    query_id: row.get(0)?,
                    revision: row.get(1)?,
                    name: row.get(2)?,
                    sparql: row.get(3)?,
                    note: row.get(4)?,
                    origin: row.get(5)?,
                    created_by: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ─── Test history ──────────────────────────────────────────────────────

    pub fn insert_test(&self, t: &QueryTest) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO saved_query_tests \
             (id, query_id, revision, dataset_id, dataset_version, prev_version, status, \
              result_hash, result_rowcount, error_message, acknowledged, acknowledged_by, acknowledged_at, created_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                t.id,
                t.query_id,
                t.revision,
                t.dataset_id,
                t.dataset_version,
                t.prev_version,
                t.status,
                t.result_hash,
                t.result_rowcount,
                t.error_message,
                t.acknowledged as i32,
                t.acknowledged_by,
                t.acknowledged_at,
                t.created_at
            ],
        )?;
        Ok(())
    }

    pub fn list_tests(&self, query_id: &str) -> anyhow::Result<Vec<QueryTest>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {T_COLS} FROM saved_query_tests WHERE query_id=?1 ORDER BY created_at DESC"
        ))?;
        let rows = stmt
            .query_map(params![query_id], read_test)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Most recent test recorded for a given (query, dataset_version).
    pub fn latest_test_for_version(
        &self,
        query_id: &str,
        version: &str,
    ) -> anyhow::Result<Option<QueryTest>> {
        let conn = self.pool.get()?;
        conn.query_row(
            &format!(
                "SELECT {T_COLS} FROM saved_query_tests \
                 WHERE query_id=?1 AND dataset_version=?2 ORDER BY created_at DESC LIMIT 1"
            ),
            params![query_id, version],
            read_test,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_test(&self, test_id: &str) -> anyhow::Result<Option<QueryTest>> {
        let conn = self.pool.get()?;
        conn.query_row(
            &format!("SELECT {T_COLS} FROM saved_query_tests WHERE id=?1"),
            params![test_id],
            read_test,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn acknowledge_test(&self, test_id: &str, user: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE saved_query_tests SET acknowledged=1, acknowledged_by=?1, acknowledged_at=?2 WHERE id=?3",
            params![user, now, test_id],
        )?;
        Ok(())
    }
}

fn read_query_by_id(conn: &Connection, id: &str) -> anyhow::Result<Option<SavedQuery>> {
    conn.query_row(
        &format!("SELECT {Q_COLS} FROM saved_queries WHERE id=?1"),
        params![id],
        read_query,
    )
    .optional()
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::db::AuthDb;

    fn store() -> SavedQueryStore {
        let db = AuthDb::in_memory().unwrap();
        SavedQueryStore::new(db.pool())
    }

    fn create_req(name: &str, sparql: &str) -> CreateSavedQueryRequest {
        CreateSavedQueryRequest {
            name: name.to_string(),
            slug: None,
            description: Some("d".into()),
            sparql: sparql.to_string(),
            parameters: vec![],
            test_parameters: None,
            visibility: None,
            version_name: None,
            note: None,
        }
    }

    #[test]
    fn create_get_revision_roundtrip() {
        let s = store();
        let sq = s
            .create(QueryScope::Dataset, "ds1", &create_req("My Query", "SELECT * WHERE { ?s ?p ?o }"), "u1")
            .unwrap();
        assert_eq!(sq.slug, "my-query");
        assert_eq!(sq.current_revision, 1);
        assert_eq!(sq.sparql.as_deref(), Some("SELECT * WHERE { ?s ?p ?o }"));

        // add a revision (with a commit-style custom name)
        let rev = s.add_revision(&sq.id, "ASK {}", Some("v2 — switch to ASK"), Some("fix"), "manual", "u1").unwrap();
        assert_eq!(rev, 2);
        let got = s.get(&sq.id).unwrap().unwrap();
        assert_eq!(got.current_revision, 2);
        assert_eq!(got.sparql.as_deref(), Some("ASK {}"));
        let revs = s.list_revisions(&sq.id).unwrap();
        assert_eq!(revs.len(), 2);
        assert_eq!(revs[0].name.as_deref(), Some("v2 — switch to ASK"));
    }

    #[test]
    fn slug_collisions_get_suffixed() {
        let s = store();
        let a = s.create(QueryScope::Dataset, "ds1", &create_req("Dup", "ASK{}"), "u1").unwrap();
        let b = s.create(QueryScope::Dataset, "ds1", &create_req("Dup", "ASK{}"), "u1").unwrap();
        assert_eq!(a.slug, "dup");
        assert_eq!(b.slug, "dup-2");
        // different owner reuses the base slug
        let c = s.create(QueryScope::Dataset, "ds2", &create_req("Dup", "ASK{}"), "u1").unwrap();
        assert_eq!(c.slug, "dup");
    }

    #[test]
    fn test_history_and_ack() {
        let s = store();
        let sq = s.create(QueryScope::Dataset, "ds1", &create_req("Q", "ASK{}"), "u1").unwrap();
        let t = QueryTest {
            id: Uuid::new_v4().to_string(),
            query_id: sq.id.clone(),
            revision: 1,
            dataset_id: "ds1".into(),
            dataset_version: "1.0.0".into(),
            prev_version: None,
            status: "error".into(),
            result_hash: None,
            result_rowcount: None,
            error_message: Some("boom".into()),
            acknowledged: false,
            acknowledged_by: None,
            acknowledged_at: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        s.insert_test(&t).unwrap();
        assert_eq!(s.list_tests(&sq.id).unwrap().len(), 1);
        assert_eq!(s.latest_test_for_version(&sq.id, "1.0.0").unwrap().unwrap().status, "error");
        s.acknowledge_test(&t.id, "admin").unwrap();
        assert!(s.get_test(&t.id).unwrap().unwrap().acknowledged);
    }

    #[test]
    fn delete_cascades() {
        let s = store();
        let sq = s.create(QueryScope::Dataset, "ds1", &create_req("Q", "ASK{}"), "u1").unwrap();
        s.delete(&sq.id).unwrap();
        assert!(s.get(&sq.id).unwrap().is_none());
        assert_eq!(s.list_revisions(&sq.id).unwrap().len(), 0);
    }
}
