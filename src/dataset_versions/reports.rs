//! Validation-report persistence + read endpoints (Unified Accounts plan, Phase 5).
//!
//! Stored reports carry provenance (dataset, version, who, when, data/shapes refs)
//! and are written by the validate-and-commit path and the continuous
//! (validate-on-write) path. Read endpoints are visibility-scoped like the rest of
//! the dataset API.

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use chrono::Utc;
use uuid::Uuid;

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::ValidationReportRecord;
use crate::server::error::AppError;
use crate::server::AppState;

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

fn require_read(state: &AppState, dataset_id: &str, uid: Option<&str>) -> Result<(), AppError> {
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
        Ok(())
    } else {
        Err(AppError::NotFound(format!(
            "Dataset '{dataset_id}' not found"
        )))
    }
}

/// GET /api/datasets/:dataset_id/validation-reports
pub async fn list_reports(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let uid = user.as_ref().map(|u| u.user_id.as_str());
    require_read(&state, &dataset_id, uid)?;
    let reports = state
        .auth_db
        .list_validation_reports(&dataset_id)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(reports))
}

/// GET /api/datasets/:dataset_id/validation-reports/:rid
pub async fn get_report(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, rid)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let uid = user.as_ref().map(|u| u.user_id.as_str());
    require_read(&state, &dataset_id, uid)?;
    let report = state
        .auth_db
        .get_validation_report(&dataset_id, &rid)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Report '{rid}' not found")))?;
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
