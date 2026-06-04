//! Dataset versioning: snapshot a dataset's named graphs into versioned graphs
//! with a draft→staged→published→deprecated lifecycle, named branches, and
//! restore-onto-live-graphs.
pub mod commit;
pub mod handlers;
pub mod models;
pub mod registry;
pub mod reports;
pub mod routes;
pub mod share_links;
pub mod snapshot;

use crate::store::TripleStore;
use models::{DatasetVersion, VersionStatus};

/// Snapshot `source_graphs` into a new version with the given lifecycle status
/// and record it in the registry. `update_latest_draft` is only meaningful for
/// draft versions, so it is driven off the status.
pub fn snapshot_as_version(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
    source_graphs: &[String],
    status: VersionStatus,
    created_by: Option<&str>,
    notes: Option<&str>,
) -> Result<DatasetVersion, String> {
    let source_map = snapshot::snapshot_graphs(store, base_url, dataset_id, version, source_graphs)
        .map_err(|e| e.to_string())?;
    let snapshot_graphs: Vec<String> = source_map.iter().map(|m| m.snapshot_graph.clone()).collect();

    let record = DatasetVersion {
        dataset_id: dataset_id.to_string(),
        version: version.to_string(),
        status,
        graph_iri: format!("{base_url}/dataset/{dataset_id}/version/{version}"),
        snapshot_graphs,
        source_map,
        created_at: chrono::Utc::now().to_rfc3339(),
        created_by: created_by.map(|u| u.to_string()),
        derived_from: None,
        notes: notes.map(|n| n.to_string()),
        branch: None,
    };
    registry::insert_version(store, base_url, &record).map_err(|e| e.to_string())?;
    if status == VersionStatus::Draft {
        registry::update_latest_draft(store, base_url, dataset_id, version)
            .map_err(|e| e.to_string())?;
    }
    // Snapshot the validation layer alongside the data (best-effort).
    if let Err(e) = crate::shacl_studio::bindings::snapshot_dataset_bindings(
        store, base_url, dataset_id, version, source_graphs,
    ) {
        tracing::warn!("failed to snapshot validation bindings for {dataset_id} v{version}: {e}");
    }
    Ok(record)
}

/// Compute the next semantic version for a dataset from its existing versions.
///
/// Only well-formed `MAJOR.MINOR.PATCH` numeric versions count toward the
/// baseline (auto-timestamp and branch labels are ignored). With no prior
/// semver the baseline is `0.0.0`, so the first `major` becomes `1.0.0`, the
/// first `minor` `0.1.0`, and the first `patch` `0.0.1`. `bump` is one of
/// `"major" | "minor" | "patch"` (anything else is treated as `patch`).
pub fn next_semver(existing: &[DatasetVersion], bump: &str) -> String {
    let (mut maj, mut min, mut pat) = (0u64, 0u64, 0u64);
    for v in existing {
        if let Some(parsed) = parse_semver(&v.version) {
            if parsed > (maj, min, pat) {
                (maj, min, pat) = parsed;
            }
        }
    }
    let next = match bump {
        "major" => (maj + 1, 0, 0),
        "minor" => (maj, min + 1, 0),
        _ => (maj, min, pat + 1),
    };
    format!("{}.{}.{}", next.0, next.1, next.2)
}

/// Parse a strict `MAJOR.MINOR.PATCH` triple of non-negative integers.
fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.split('.');
    let maj = parts.next()?.parse().ok()?;
    let min = parts.next()?.parse().ok()?;
    let pat = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((maj, min, pat))
}

#[cfg(test)]
mod semver_tests {
    use super::*;

    fn ver(v: &str) -> DatasetVersion {
        DatasetVersion {
            dataset_id: "ds".into(),
            version: v.into(),
            status: VersionStatus::Published,
            graph_iri: String::new(),
            snapshot_graphs: vec![],
            source_map: vec![],
            created_at: String::new(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
        }
    }

    #[test]
    fn first_version_from_empty_baseline() {
        assert_eq!(next_semver(&[], "patch"), "0.0.1");
        assert_eq!(next_semver(&[], "minor"), "0.1.0");
        assert_eq!(next_semver(&[], "major"), "1.0.0");
    }

    #[test]
    fn bumps_from_highest_semver_ignoring_non_semver() {
        let vs = vec![ver("1.2.3"), ver("auto-20240101-000000"), ver("1.1.9")];
        assert_eq!(next_semver(&vs, "patch"), "1.2.4");
        assert_eq!(next_semver(&vs, "minor"), "1.3.0");
        assert_eq!(next_semver(&vs, "major"), "2.0.0");
    }
}
