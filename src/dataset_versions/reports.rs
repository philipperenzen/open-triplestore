//! Validation-report persistence + read endpoints (Unified Accounts plan, Phase 5).
//!
//! Stored reports carry provenance (dataset, version, who, when, data/shapes refs)
//! and are written by the validate-and-commit path and the continuous
//! (validate-on-write) path. The read endpoints serve a report only to a caller
//! who may read the graphs it is about ([`ReportReader`]).

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use chrono::Utc;
use uuid::Uuid;

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{Dataset, ValidationReportRecord};
use crate::server::error::AppError;
use crate::server::AppState;

use super::registry;

/// `data_ref` of a report whose data came in the request body rather than
/// from a graph (validate-and-commit): what it validated is the graph its
/// `version` was cut from.
pub(crate) const INLINE_DATA: &str = "inline";

/// Persist a validation report. Returns the new report id.
#[allow(clippy::too_many_arguments)]
pub fn persist_report(
    state: &AppState,
    dataset_id: &str,
    version: Option<&str>,
    conforms: bool,
    report_ttl: &str,
    data_ref: Option<&str>,
    shapes_ref: Option<&str>,
    source: &str,
    created_by: Option<&str>,
) -> anyhow::Result<String> {
    let id = Uuid::new_v4().to_string();
    let rec = ValidationReportRecord {
        id: id.clone(),
        dataset_id: dataset_id.to_string(),
        version: version.map(String::from),
        conforms,
        report_ttl: report_ttl.to_string(),
        data_ref: data_ref.map(String::from),
        shapes_ref: shapes_ref.map(String::from),
        source: source.to_string(),
        created_by: created_by.map(String::from),
        created_at: Utc::now().to_rfc3339(),
    };
    state.auth_db.insert_validation_report(&rec)?;
    Ok(id)
}

/// The dataset, when `uid` may read it; `404` otherwise, as for one that does
/// not exist.
fn require_read(
    state: &AppState,
    dataset_id: &str,
    uid: Option<&str>,
) -> Result<Dataset, AppError> {
    let ds = state
        .auth_db
        .get_dataset(dataset_id)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Dataset '{dataset_id}' not found")))?;
    if state
        .auth_db
        .can_access_dataset(uid, &ds)
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        Ok(ds)
    } else {
        Err(AppError::NotFound(format!(
            "Dataset '{dataset_id}' not found"
        )))
    }
}

/// What a caller may read of a dataset's stored reports.
///
/// A report is about graphs: the one it validated and its shapes graph. Its
/// record names them (`data_ref`, `shapes_ref`), and a commit's report is the
/// validator's findings on the committed data. So a report is served by the
/// rule a stored validation run's report is (`stored_run_view` in
/// `server::routes`), asked at every read:
///
/// * nobody but an admin reads a report whose shapes graph some dataset holds
///   as private and they may not read
///   ([`crate::auth::acl::withheld_private_graphs`]), the dataset's writers
///   included: a shapes graph linked from another dataset need not be theirs
///   to read;
/// * the dataset's writers, admins among them, read every other report;
/// * anyone else reads a report when they may read each graph it validated,
///   by the `/sparql` rule ([`crate::auth::acl::readable_graph_iris`]): a
///   private graph only with a graph-ACL read grant. A report that does not
///   say what it validated goes to writers only.
///
/// A report the caller may not read is not served at all (left out of the
/// listing, `404` by id) rather than served without the two names: what is
/// left of it (that a write to some graph was checked, when, and how it came
/// out) is about that graph too.
///
/// Who ran it (`created_by`) is a separate matter: a report the caller reads
/// names the account only to `OTS_ACTOR_VISIBILITY`'s audience
/// ([`shows_actors`]).
///
/// Fails closed: when the private flags or the caller's graphs cannot be
/// read, resolving is an error (`500`) and no report is served.
struct ReportReader {
    /// May write the dataset.
    writer: bool,
    /// Graphs some dataset holds as private that the caller may not read;
    /// none for an admin.
    withheld: HashSet<String>,
    /// The graphs a caller who is no writer may read.
    readable: HashSet<String>,
}

impl ReportReader {
    fn resolve(
        state: &AppState,
        ds: &Dataset,
        user: Option<&AuthenticatedUser>,
    ) -> Result<Self, AppError> {
        let internal = |e: anyhow::Error| AppError::Internal(e.to_string());
        let writer = match user {
            Some(u) => state
                .auth_db
                .can_write_dataset(&u.user_id, ds)
                .map_err(internal)?,
            None => false,
        };
        let withheld =
            crate::auth::acl::withheld_private_graphs(&state.auth_db, user).map_err(internal)?;
        let readable = if writer {
            HashSet::new()
        } else {
            crate::auth::acl::readable_graph_iris(
                &state.auth_db,
                user.map(|u| (u.user_id.as_str(), u.role.as_str())),
            )
            .map_err(internal)?
        };
        Ok(Self {
            writer,
            withheld,
            readable,
        })
    }

    /// Whether `report` is the caller's to read. `sources_of` gives the
    /// graphs a version of the dataset was cut from ([`version_sources`]); it
    /// is asked only for a caller who is no writer.
    fn serves(
        &self,
        report: &ValidationReportRecord,
        sources_of: &mut dyn FnMut(&str) -> Option<Vec<String>>,
    ) -> bool {
        if report
            .shapes_ref
            .as_deref()
            .is_some_and(|g| self.withheld.contains(g))
        {
            return false;
        }
        if self.writer {
            return true;
        }
        let mut validated: Vec<String> = match report.data_ref.as_deref() {
            None | Some(INLINE_DATA) => Vec::new(),
            Some(graph) => vec![graph.to_string()],
        };
        if let Some(version) = report.version.as_deref() {
            match sources_of(version) {
                Some(sources) if !sources.is_empty() => validated.extend(sources),
                // The version is gone, or its record maps no graph: what the
                // report validated cannot be told.
                _ => return false,
            }
        }
        !validated.is_empty() && validated.iter().all(|g| self.readable.contains(g))
    }
}

/// Whether `uid` sees who ran the dataset's validations (`created_by`):
/// `OTS_ACTOR_VISIBILITY`'s audience, as for the rest of its history.
fn shows_actors(state: &AppState, ds: &Dataset, uid: Option<&str>) -> Result<bool, AppError> {
    crate::auth::policy::shows_actors(&state.auth_db, ds, uid)
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// The graphs `version` of the dataset was cut from, by its registry record;
/// `None` when the registry holds no such version.
fn version_sources(state: &AppState, dataset_id: &str, version: &str) -> Option<Vec<String>> {
    registry::get_version(&state.store, &state.base_url, dataset_id, version).map(|record| {
        record
            .source_map
            .into_iter()
            .map(|m| m.source_graph)
            .collect()
    })
}

/// GET /api/datasets/:dataset_id/validation-reports — the reports the caller
/// may read ([`ReportReader`]), newest first, without their bodies, and
/// without `created_by` below `OTS_ACTOR_VISIBILITY`'s audience.
pub async fn list_reports(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let uid = user.as_ref().map(|u| u.user_id.as_str());
    let ds = require_read(&state, &dataset_id, uid)?;
    let reader = ReportReader::resolve(&state, &ds, user.as_deref())?;
    let mut reports = state
        .auth_db
        .list_validation_reports(&dataset_id)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    // One registry read per version, however many reports name it.
    let mut versions: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let mut sources_of = |version: &str| {
        versions
            .entry(version.to_string())
            .or_insert_with(|| version_sources(&state, &dataset_id, version))
            .clone()
    };
    reports.retain(|r| reader.serves(r, &mut sources_of));
    if !shows_actors(&state, &ds, uid)? {
        for r in &mut reports {
            r.created_by = None;
        }
    }
    Ok(Json(reports))
}

/// GET /api/datasets/:dataset_id/validation-reports/:rid — one report with
/// its body. A report the caller may not read ([`ReportReader`]) answers as
/// one that does not exist.
pub async fn get_report(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, rid)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let uid = user.as_ref().map(|u| u.user_id.as_str());
    let ds = require_read(&state, &dataset_id, uid)?;
    let reader = ReportReader::resolve(&state, &ds, user.as_deref())?;
    let not_found = || AppError::NotFound(format!("Report '{rid}' not found"));
    let mut report = state
        .auth_db
        .get_validation_report(&dataset_id, &rid)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(not_found)?;
    if !reader.serves(&report, &mut |version| {
        version_sources(&state, &dataset_id, version)
    }) {
        return Err(not_found());
    }
    if !shows_actors(&state, &ds, uid)? {
        report.created_by = None;
    }
    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use crate::auth::db::AuthDb;
    use crate::auth::models::{OwnerType, ValidationReportRecord, Visibility};
    use std::sync::Arc;

    #[test]
    fn report_persist_list_get_roundtrip() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        db.create_dataset(
            "ds1",
            "DS",
            None,
            OwnerType::User,
            "u1",
            Visibility::Private,
            None,
        )
        .unwrap();

        let rec = ValidationReportRecord {
            id: "r1".to_string(),
            dataset_id: "ds1".to_string(),
            version: Some("v1".to_string()),
            conforms: true,
            report_ttl: "@prefix sh: <http://www.w3.org/ns/shacl#> .\n[] a sh:ValidationReport ."
                .to_string(),
            data_ref: Some("inline".to_string()),
            shapes_ref: None,
            source: "platform".to_string(),
            created_by: Some("u1".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        db.insert_validation_report(&rec).unwrap();

        let list = db.list_validation_reports("ds1").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "r1");
        // report_ttl is omitted from list payloads for size.
        assert_eq!(list[0].report_ttl, "");

        let got = db.get_validation_report("ds1", "r1").unwrap().unwrap();
        assert!(got.conforms);
        assert!(got.report_ttl.contains("sh:ValidationReport"));
        assert!(db
            .get_validation_report("ds1", "missing")
            .unwrap()
            .is_none());
    }
}
