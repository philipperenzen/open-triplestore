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
    /// Set hash of this graph's content in the version (`ver:graphHash`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// Quads of this graph in the version (`ver:graphQuads`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quad_count: Option<i64>,
}

impl GraphMapping {
    pub fn new(snapshot_graph: String, source_graph: String) -> Self {
        Self {
            snapshot_graph,
            source_graph,
            content_hash: None,
            quad_count: None,
        }
    }
}

/// How a version holds its data (delta note §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VersionKind {
    /// Physical snapshot graphs, as every version used to be.
    #[default]
    Checkpoint,
    /// A base checkpoint plus a range of the change log; its snapshot graphs
    /// are built on demand (materialised) and dropped again after a while.
    Delta,
}

/// Where a version's content sits in the change log: the content of its
/// covered graphs as of sequence number `seq` in `epoch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub epoch: String,
    pub seq: i64,
}

/// A delta's range of the change log: rows `from_seq < seq <= to_seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchRange {
    pub epoch: String,
    pub from_seq: i64,
    pub to_seq: i64,
}

fn is_false(b: &bool) -> bool {
    !*b
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
    /// `checkpoint` or `delta`. For a delta, `snapshot_graphs` and
    /// `source_map` name the graphs it materialises into — the same IRIs a
    /// checkpoint would have — which hold nothing until it is materialised.
    #[serde(default)]
    pub kind: VersionKind,
    /// A delta's base checkpoint (`ver:baseVersion`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_version: Option<String>,
    /// A delta's range of the change log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_range: Option<PatchRange>,
    /// Where the version's content sits in the change log; `None` for a
    /// checkpoint cut while the log was off or while a write touched its
    /// graphs (it cannot be a delta's base).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    /// Set hash of the content (`store::set_hash`): 64 hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// Quads in the version (a checkpoint's copy; a delta's base plus net).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quad_count: Option<i64>,
    /// A delta's `added_n + removed_n` over its range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_count: Option<i64>,
    /// The chain could not be replayed, or replayed to content that did not
    /// match `content_hash`: the version's data is not available.
    #[serde(default, skip_serializing_if = "is_false")]
    pub chain_broken: bool,
    /// A delta whose snapshot graphs are currently built.
    #[serde(default, skip_serializing_if = "is_false")]
    pub materialised: bool,
    /// The data model this version's instances were pinned to when it was cut
    /// (the dataset's `conforms_to_model` / `conforms_to_version` at that moment),
    /// so a published version keeps saying which model version it conformed to
    /// after the dataset moves on to a newer one.
    #[serde(default)]
    pub conforms_to_model: Option<String>,
    #[serde(default)]
    pub conforms_to_version: Option<String>,
}

impl DatasetVersion {
    /// A checkpoint record with the version fields every caller sets; the
    /// delta fields empty.
    #[allow(clippy::too_many_arguments)] // the registry record's own fields
    pub fn checkpoint(
        dataset_id: &str,
        version: &str,
        status: VersionStatus,
        graph_iri: String,
        source_map: Vec<GraphMapping>,
        created_by: Option<String>,
        notes: Option<String>,
        branch: Option<String>,
    ) -> Self {
        Self {
            dataset_id: dataset_id.to_string(),
            version: version.to_string(),
            status,
            graph_iri,
            snapshot_graphs: source_map
                .iter()
                .map(|m| m.snapshot_graph.clone())
                .collect(),
            source_map,
            created_at: chrono::Utc::now().to_rfc3339(),
            created_by,
            derived_from: None,
            notes,
            branch,
            kind: VersionKind::Checkpoint,
            base_version: None,
            patch_range: None,
            anchor: None,
            content_hash: None,
            quad_count: None,
            change_count: None,
            chain_broken: false,
            materialised: false,
            conforms_to_model: None,
            conforms_to_version: None,
        }
    }

    pub fn is_delta(&self) -> bool {
        self.kind == VersionKind::Delta
    }

    /// The source graphs the version covers.
    pub fn covered_graphs(&self) -> Vec<String> {
        self.source_map
            .iter()
            .map(|m| m.source_graph.clone())
            .collect()
    }
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
    /// snapshot *all* of the dataset's registered graphs (bar its
    /// statement-provenance graph).
    #[serde(default)]
    pub graphs: Vec<String>,
    /// Cut a physical checkpoint even where a delta could be cut.
    #[serde(default)]
    pub checkpoint: bool,
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
