//! SQLite persistence for SHACL Studio (shape graphs, pipelines, runs).
//!
//! Reuses the auth database's connection pool (like `SavedQueryStore` and the
//! audit logger); the tables themselves are created centrally in
//! `AuthDb::migrate`. List-valued columns are JSON `TEXT`.

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, params_from_iter, OptionalExtension};
use uuid::Uuid;

use crate::auth::models::{OwnerType, Visibility};

use super::models::*;

// `status` is appended LAST so the positional indices read by `read_shape_graph` stay
// valid against pre-existing rows.
const SHAPE_GRAPH_COLS: &str = "id, name, description, owner_type, owner_id, visibility, graph_iri, tags, \
    target_classes, shape_count, source, version, created_by, created_at, updated_at, status";

// `targets` is appended LAST for the same reason (see `read_pipeline`).
const PIPE_COLS: &str = "id, name, description, owner_type, owner_id, visibility, dataset_ids, \
    graph_iris, target_classes, shape_set_ids, severity_threshold, run_inference, max_results, \
    trigger_on_write, schedule_cron, gate_writes, retention, last_run_at, last_conforms, \
    created_by, created_at, updated_at, targets, \
    inferred_target_kind, inferred_target_graph, results_target_kind, results_target_graph";

const RUN_COLS: &str = "id, pipeline_id, triggered_by, actor, ran_at, conforms, results_count, \
    violation_count, warning_count, info_count, duration_ms";

/// SHACL Studio persistence handle. Cheap to construct (`Pool` clone).
pub struct ShaclStudioStore {
    pool: Pool<SqliteConnectionManager>,
}

fn list_json(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string())
}
fn parse_list(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}
fn targets_json(v: &[ValidationTarget]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string())
}
fn parse_targets(s: &str) -> Vec<ValidationTarget> {
    serde_json::from_str(s).unwrap_or_default()
}

fn read_shape_graph(row: &rusqlite::Row) -> rusqlite::Result<ShapeGraph> {
    let owner_type: String = row.get(3)?;
    let visibility: String = row.get(5)?;
    let tags: String = row.get(7)?;
    let target_classes: String = row.get(8)?;
    let source: String = row.get(10)?;
    let status: String = row.get(15)?;
    Ok(ShapeGraph {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        owner_type: OwnerType::from_str(&owner_type).unwrap_or(OwnerType::User),
        owner_id: row.get(4)?,
        visibility: Visibility::from_str(&visibility).unwrap_or(Visibility::Private),
        graph_iri: row.get(6)?,
        tags: parse_list(&tags),
        target_classes: parse_list(&target_classes),
        shape_count: row.get(9)?,
        source: ShapeSource::from_str_or_manual(&source),
        version: row.get(11)?,
        created_by: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        status: VersionStatus::from_str(&status).unwrap_or(VersionStatus::Draft),
    })
}

fn read_pipeline(row: &rusqlite::Row) -> rusqlite::Result<ValidationPipeline> {
    let owner_type: String = row.get(3)?;
    let visibility: String = row.get(5)?;
    let dataset_ids: String = row.get(6)?;
    let graph_iris: String = row.get(7)?;
    let target_classes: String = row.get(8)?;
    let shape_graph_ids: String = row.get(9)?;
    let threshold: String = row.get(10)?;
    let last_conforms: Option<i64> = row.get(18)?;
    let targets: String = row.get(22)?;
    let inferred_kind: String = row.get(23)?;
    let results_kind: String = row.get(25)?;
    Ok(ValidationPipeline {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        owner_type: OwnerType::from_str(&owner_type).unwrap_or(OwnerType::User),
        owner_id: row.get(4)?,
        visibility: Visibility::from_str(&visibility).unwrap_or(Visibility::Private),
        targets: parse_targets(&targets),
        dataset_ids: parse_list(&dataset_ids),
        graph_iris: parse_list(&graph_iris),
        target_classes: parse_list(&target_classes),
        shape_graph_ids: parse_list(&shape_graph_ids),
        severity_threshold: SeverityThreshold::from_str_or_default(&threshold),
        run_inference: row.get::<_, i64>(11)? != 0,
        max_results: row.get(12)?,
        trigger_on_write: row.get::<_, i64>(13)? != 0,
        schedule_cron: row.get(14)?,
        gate_writes: row.get::<_, i64>(15)? != 0,
        retention: row.get(16)?,
        last_run_at: row.get(17)?,
        last_conforms: last_conforms.map(|v| v != 0),
        created_by: row.get(19)?,
        created_at: row.get(20)?,
        updated_at: row.get(21)?,
        inferred_target: WriteTarget::from_str_or_default(&inferred_kind),
        inferred_target_graph: row.get(24)?,
        results_target: ResultsTarget::from_str_or_default(&results_kind),
        results_target_graph: row.get(26)?,
    })
}

fn read_run_summary(row: &rusqlite::Row) -> rusqlite::Result<PipelineRun> {
    Ok(PipelineRun {
        id: row.get(0)?,
        pipeline_id: row.get(1)?,
        triggered_by: row.get(2)?,
        actor: row.get(3)?,
        ran_at: row.get(4)?,
        conforms: row.get::<_, i64>(5)? != 0,
        results_count: row.get(6)?,
        violation_count: row.get(7)?,
        warning_count: row.get(8)?,
        info_count: row.get(9)?,
        duration_ms: row.get(10)?,
        report: None,
    })
}

impl ShaclStudioStore {
    pub fn new(pool: Pool<SqliteConnectionManager>) -> Self {
        Self { pool }
    }

    // ─── Shape graphs ─────────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn create_shape_graph(
        &self,
        name: &str,
        description: Option<&str>,
        owner_type: OwnerType,
        owner_id: &str,
        visibility: Visibility,
        graph_iri: &str,
        tags: &[String],
        source: ShapeSource,
        created_by: Option<&str>,
    ) -> anyhow::Result<ShapeGraph> {
        let conn = self.pool.get()?;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO shape_sets (id, name, description, owner_type, owner_id, visibility, graph_iri, tags, target_classes, shape_count, source, version, created_by, created_at, updated_at, status) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'[]',0,?9,1,?10,?11,?11,'draft')",
            params![id, name, description, owner_type.as_str(), owner_id, visibility.as_str(), graph_iri, list_json(tags), source.as_str(), created_by, now],
        )?;
        Ok(ShapeGraph {
            id,
            name: name.to_string(),
            description: description.map(String::from),
            owner_type,
            owner_id: owner_id.to_string(),
            visibility,
            graph_iri: graph_iri.to_string(),
            tags: tags.to_vec(),
            target_classes: vec![],
            shape_count: 0,
            source,
            version: 1,
            status: VersionStatus::Draft,
            created_by: created_by.map(String::from),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn get_shape_graph(&self, id: &str) -> anyhow::Result<Option<ShapeGraph>> {
        let conn = self.pool.get()?;
        conn.query_row(
            &format!("SELECT {SHAPE_GRAPH_COLS} FROM shape_sets WHERE id = ?1"),
            params![id],
            read_shape_graph,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_shape_graph_by_iri(&self, graph_iri: &str) -> anyhow::Result<Option<ShapeGraph>> {
        let conn = self.pool.get()?;
        conn.query_row(
            &format!("SELECT {SHAPE_GRAPH_COLS} FROM shape_sets WHERE graph_iri = ?1 LIMIT 1"),
            params![graph_iri],
            read_shape_graph,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_shape_graphs(&self) -> anyhow::Result<Vec<ShapeGraph>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!("SELECT {SHAPE_GRAPH_COLS} FROM shape_sets ORDER BY name"))?;
        let rows = stmt.query_map([], read_shape_graph)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn update_shape_graph_meta(
        &self,
        id: &str,
        name: &str,
        description: Option<&str>,
        visibility: Visibility,
        tags: &[String],
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE shape_sets SET name=?1, description=?2, visibility=?3, tags=?4, updated_at=?5 WHERE id=?6",
            params![name, description, visibility.as_str(), list_json(tags), now, id],
        )?;
        Ok(())
    }

    /// Transition a shape graph's lifecycle status (Draft → Staged → Published →
    /// Deprecated). Gating is independent of status — a draft set still gates.
    pub fn set_shape_graph_status(&self, id: &str, status: VersionStatus) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE shape_sets SET status=?1, updated_at=?2 WHERE id=?3",
            params![status.as_str(), now, id],
        )?;
        Ok(())
    }

    /// Record a new Turtle snapshot, bump the version, and refresh the cached
    /// `target_classes` / `shape_count` facets. Returns the new version number.
    pub fn save_shape_graph_revision(
        &self,
        id: &str,
        turtle: &str,
        target_classes: &[String],
        shape_count: i64,
        note: Option<&str>,
        created_by: Option<&str>,
    ) -> anyhow::Result<i64> {
        let mut conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        let tx = conn.transaction()?;
        let next: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(revision), 0) + 1 FROM shape_set_revisions WHERE shape_set_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap_or(1);
        tx.execute(
            "INSERT INTO shape_set_revisions (shape_set_id, revision, turtle, note, created_by, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![id, next, turtle, note, created_by, now],
        )?;
        tx.execute(
            "UPDATE shape_sets SET version=?1, target_classes=?2, shape_count=?3, updated_at=?4 WHERE id=?5",
            params![next, list_json(target_classes), shape_count, now, id],
        )?;
        tx.commit()?;
        Ok(next)
    }

    pub fn list_shape_graph_revisions(&self, id: &str) -> anyhow::Result<Vec<ShapeGraphRevision>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT shape_set_id, revision, note, created_by, created_at FROM shape_set_revisions WHERE shape_set_id = ?1 ORDER BY revision DESC",
        )?;
        let rows = stmt
            .query_map(params![id], |row| {
                Ok(ShapeGraphRevision {
                    shape_graph_id: row.get(0)?,
                    revision: row.get(1)?,
                    turtle: String::new(),
                    note: row.get(2)?,
                    created_by: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_shape_graph_revision(&self, id: &str, revision: i64) -> anyhow::Result<Option<ShapeGraphRevision>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT shape_set_id, revision, turtle, note, created_by, created_at FROM shape_set_revisions WHERE shape_set_id = ?1 AND revision = ?2",
            params![id, revision],
            |row| {
                Ok(ShapeGraphRevision {
                    shape_graph_id: row.get(0)?,
                    revision: row.get(1)?,
                    turtle: row.get(2)?,
                    note: row.get(3)?,
                    created_by: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn delete_shape_graph(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM shape_sets WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ─── Pipelines ────────────────────────────────────────────────────────────

    /// Insert a fully-built pipeline (caller assigns the id + timestamps).
    pub fn insert_pipeline(&self, p: &ValidationPipeline) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO validation_pipelines (id, name, description, owner_type, owner_id, visibility, dataset_ids, graph_iris, target_classes, shape_set_ids, severity_threshold, run_inference, max_results, trigger_on_write, schedule_cron, gate_writes, retention, last_run_at, last_conforms, created_by, created_at, updated_at, targets, inferred_target_kind, inferred_target_graph, results_target_kind, results_target_graph) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27)",
            params![
                p.id, p.name, p.description, p.owner_type.as_str(), p.owner_id, p.visibility.as_str(),
                list_json(&p.dataset_ids), list_json(&p.graph_iris), list_json(&p.target_classes), list_json(&p.shape_graph_ids),
                p.severity_threshold.as_str(), p.run_inference as i32, p.max_results,
                p.trigger_on_write as i32, p.schedule_cron, p.gate_writes as i32, p.retention,
                p.last_run_at, p.last_conforms.map(|b| b as i32), p.created_by, p.created_at, p.updated_at,
                targets_json(&p.targets),
                p.inferred_target.as_str(), p.inferred_target_graph, p.results_target.as_str(), p.results_target_graph,
            ],
        )?;
        Ok(())
    }

    /// Overwrite the mutable fields of an existing pipeline.
    pub fn update_pipeline(&self, p: &ValidationPipeline) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE validation_pipelines SET name=?2, description=?3, visibility=?4, dataset_ids=?5, graph_iris=?6, target_classes=?7, shape_set_ids=?8, severity_threshold=?9, run_inference=?10, max_results=?11, trigger_on_write=?12, schedule_cron=?13, gate_writes=?14, retention=?15, updated_at=?16, targets=?17, inferred_target_kind=?18, inferred_target_graph=?19, results_target_kind=?20, results_target_graph=?21 WHERE id=?1",
            params![
                p.id, p.name, p.description, p.visibility.as_str(),
                list_json(&p.dataset_ids), list_json(&p.graph_iris), list_json(&p.target_classes), list_json(&p.shape_graph_ids),
                p.severity_threshold.as_str(), p.run_inference as i32, p.max_results,
                p.trigger_on_write as i32, p.schedule_cron, p.gate_writes as i32, p.retention, now,
                targets_json(&p.targets),
                p.inferred_target.as_str(), p.inferred_target_graph, p.results_target.as_str(), p.results_target_graph,
            ],
        )?;
        Ok(())
    }

    pub fn get_pipeline(&self, id: &str) -> anyhow::Result<Option<ValidationPipeline>> {
        let conn = self.pool.get()?;
        conn.query_row(
            &format!("SELECT {PIPE_COLS} FROM validation_pipelines WHERE id = ?1"),
            params![id],
            read_pipeline,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_pipelines(&self) -> anyhow::Result<Vec<ValidationPipeline>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!("SELECT {PIPE_COLS} FROM validation_pipelines ORDER BY name"))?;
        let rows = stmt.query_map([], read_pipeline)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Pipelines with `gate_writes` enabled — consulted on every write.
    pub fn list_gating_pipelines(&self) -> anyhow::Result<Vec<ValidationPipeline>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {PIPE_COLS} FROM validation_pipelines WHERE gate_writes = 1"
        ))?;
        let rows = stmt.query_map([], read_pipeline)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Pipelines that carry a cron schedule — polled by the scheduler.
    pub fn list_scheduled_pipelines(&self) -> anyhow::Result<Vec<ValidationPipeline>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {PIPE_COLS} FROM validation_pipelines WHERE schedule_cron IS NOT NULL AND schedule_cron <> ''"
        ))?;
        let rows = stmt.query_map([], read_pipeline)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_pipeline(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM validation_pipelines WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn touch_pipeline_run(&self, id: &str, ran_at: &str, conforms: bool) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE validation_pipelines SET last_run_at=?2, last_conforms=?3 WHERE id=?1",
            params![id, ran_at, conforms as i32],
        )?;
        Ok(())
    }

    // ─── Pipeline runs ──────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn insert_pipeline_run(
        &self,
        pipeline_id: &str,
        triggered_by: &str,
        actor: Option<&str>,
        conforms: bool,
        results_count: i64,
        violation_count: i64,
        warning_count: i64,
        info_count: i64,
        duration_ms: i64,
        report_json: &str,
        retention: i64,
    ) -> anyhow::Result<PipelineRun> {
        let conn = self.pool.get()?;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO pipeline_runs (id, pipeline_id, triggered_by, actor, ran_at, conforms, results_count, violation_count, warning_count, info_count, duration_ms, report_json, created_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?5)",
            params![id, pipeline_id, triggered_by, actor, now, conforms as i32, results_count, violation_count, warning_count, info_count, duration_ms, report_json],
        )?;
        let keep = retention.clamp(1, 500);
        conn.execute(
            "DELETE FROM pipeline_runs WHERE pipeline_id = ?1 AND id NOT IN (
                SELECT id FROM pipeline_runs WHERE pipeline_id = ?1 ORDER BY ran_at DESC LIMIT ?2
            )",
            params![pipeline_id, keep],
        )?;
        Ok(PipelineRun {
            id,
            pipeline_id: pipeline_id.to_string(),
            triggered_by: triggered_by.to_string(),
            actor: actor.map(String::from),
            ran_at: now,
            conforms,
            results_count,
            violation_count,
            warning_count,
            info_count,
            duration_ms,
            report: None,
        })
    }

    pub fn list_pipeline_runs(&self, pipeline_id: &str, limit: i64) -> anyhow::Result<Vec<PipelineRun>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {RUN_COLS} FROM pipeline_runs WHERE pipeline_id = ?1 ORDER BY ran_at DESC LIMIT ?2"
        ))?;
        let rows = stmt
            .query_map(params![pipeline_id, limit.clamp(1, 200)], read_run_summary)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Latest run summary for each of the given pipeline ids.
    pub fn list_latest_runs(&self, pipeline_ids: &[String]) -> anyhow::Result<Vec<PipelineRun>> {
        if pipeline_ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.pool.get()?;
        let placeholders = pipeline_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT {RUN_COLS} FROM pipeline_runs r WHERE pipeline_id IN ({placeholders}) \
             AND ran_at = (SELECT MAX(ran_at) FROM pipeline_runs WHERE pipeline_id = r.pipeline_id)"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(pipeline_ids.iter()), read_run_summary)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_pipeline_run(&self, run_id: &str) -> anyhow::Result<Option<PipelineRun>> {
        let conn = self.pool.get()?;
        let row = conn
            .query_row(
                &format!("SELECT {RUN_COLS}, report_json FROM pipeline_runs WHERE id = ?1"),
                params![run_id],
                |row| {
                    let mut run = read_run_summary(row)?;
                    let report_json: String = row.get(11)?;
                    run.report = serde_json::from_str(&report_json).ok();
                    Ok(run)
                },
            )
            .optional()?;
        Ok(row)
    }
}
