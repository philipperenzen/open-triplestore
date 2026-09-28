//! Dataset versioning system — snapshots of a dataset's named graphs with a
//! draft→staged→published→deprecated lifecycle and named branches.
//!
//! Unlike data-models/vocabularies (whose "working copy" *is* a version graph),
//! a dataset's working copy is its live set of registered named graphs. A
//! dataset *version* is an immutable snapshot of (selected) live graphs copied
//! into version-scoped named graphs, plus a `source → snapshot` mapping used to
//! restore a snapshot back onto the live graphs.

use serde::{Deserialize, Serialize};

// Reuse the lifecycle enum from the data-model module so status semantics and
// serialization match across the whole versioning surface.
pub use crate::data_models::models::VersionStatus;

/// A `snapshot → source` mapping for one graph in a dataset version, used to
/// restore the snapshot back onto the dataset's live graphs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphMapping {
    pub snapshot_graph: String,
    pub source_graph: String,
}

/// Metadata for a single dataset version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetVersion {
    pub dataset_id: String,
    pub version: String,
    pub status: VersionStatus,
    /// Root IRI of this version (`{base}/dataset/{id}/version/{ver}`).
    pub graph_iri: String,
    /// Snapshot graph IRIs that make up this version.
    pub snapshot_graphs: Vec<String>,
    /// snapshot → source graph mapping (for restore).
    #[serde(default)]
    pub source_map: Vec<GraphMapping>,
    pub created_at: String,
    pub created_by: Option<String>,
    /// Version this was derived from (semver string).
    pub derived_from: Option<String>,
    pub notes: Option<String>,
    /// Branch name; `None` is the default "main" line.
    #[serde(default)]
    pub branch: Option<String>,
}

// ─── Request bodies ─────────────────────────────────────────────────────────

/// `POST /api/datasets/:id/versions` — snapshot the dataset's current graphs
/// into a new draft version.
#[derive(Debug, Deserialize)]
pub struct CreateDatasetVersionRequest {
    /// Semver-ish label for the new version (must be IRI-safe).
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    /// Optional branch name. `None` = main line.
    #[serde(default)]
    pub branch: Option<String>,
    /// Restrict the snapshot to these source graph IRIs. Empty / omitted means
    /// snapshot *all* of the dataset's registered graphs.
    #[serde(default)]
    pub graphs: Vec<String>,
}

/// `POST /api/datasets/:id/branches` — fork a new draft from an existing version.
#[derive(Debug, Deserialize)]
pub struct CreateDatasetBranchRequest {
    pub branch: String,
    pub from_version: String,
    #[serde(default)]
    pub target_version: Option<String>,
}

/// `POST /api/datasets/:id/versions/gc` — retention: delete all but the newest
/// `keep` non-published versions. Published versions are never collected.
#[derive(Debug, Deserialize)]
pub struct GcDatasetVersionsRequest {
    pub keep: usize,
}

/// `PATCH /api/datasets/:id/versions/:ver` — edit version notes.
#[derive(Debug, Deserialize)]
pub struct UpdateDatasetVersionRequest {
    pub notes: Option<String>,
}

// ─── Query params ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct VersionDataParams {
    pub graph: Option<String>,
    pub format: Option<String>,
}

// ─── Responses ──────────────────────────────────────────────────────────────

/// Tip summary of one dataset branch.
#[derive(Debug, Serialize)]
pub struct DatasetBranchInfo {
    pub branch: String,
    pub tip_version: String,
    pub status: String,
    pub base_version: Option<String>,
    pub owner: Option<String>,
    pub created_at: String,
    pub ahead: usize,
    pub behind: usize,
}
