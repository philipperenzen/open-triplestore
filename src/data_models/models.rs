//! data-model versioning system.

use serde::{Deserialize, Serialize};

/// Status of a data model version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionStatus {
    Published,
    Staged,
    Draft,
    Deprecated,
}

impl VersionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            VersionStatus::Published => "published",
            VersionStatus::Staged => "staged",
            VersionStatus::Draft => "draft",
            VersionStatus::Deprecated => "deprecated",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "published" => Some(VersionStatus::Published),
            "staged" => Some(VersionStatus::Staged),
            "draft" => Some(VersionStatus::Draft),
            "deprecated" => Some(VersionStatus::Deprecated),
            _ => None,
        }
    }
}

/// Summary of a data model (for list views).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataModelRecord {
    pub id: String,
    pub title: String,
    pub namespace: String,
    pub description: Option<String>,
    pub is_public: bool,
    /// "user" | "organisation" — mirrors the dataset owner model.
    pub owner_type: Option<String>,
    pub owner_id: Option<String>,
    pub latest_published: Option<String>,
    pub latest_draft: Option<String>,
    pub version_count: usize,
    pub created_at: String,
    pub created_by: Option<String>,
    /// Logical kind of this entry — `data-model` (OWL/RDFS ontology) or
    /// `vocabulary` (SKOS concept scheme), auto-detected from the uploaded RDF.
    /// Drives the type badge/filter in the UI and the publish-time version
    /// stamping (OWL `owl:versionIRI` vs DCAT/PAV/SKOS metadata).
    #[serde(default)]
    pub kind: crate::kind_detector::RegistryKind,
}

/// Metadata for a single data model version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataModelVersion {
    pub data_model_id: String,
    pub version: String,
    pub status: VersionStatus,
    /// IRI of the primary (or merged) named graph for this version.
    pub graph_iri: String,
    /// IRIs of all sub-graphs under this version.
    pub sub_graphs: Vec<String>,
    pub created_at: String,
    pub created_by: Option<String>,
    /// Version this was derived from (semver string).
    pub derived_from: Option<String>,
    pub notes: Option<String>,
    /// Branch name this version belongs to. `None` is treated as the default
    /// "main" line (published versions and their direct drafts).
    #[serde(default)]
    pub branch: Option<String>,
    /// Per-subgraph lifecycle overrides. When a subgraph appears here, its
    /// effective status is this value rather than the version-level `status`.
    /// Empty means every subgraph inherits the version status.
    #[serde(default)]
    pub sub_graph_status: Vec<SubGraphStatus>,
}

/// Lifecycle status of a single subgraph within a version (Phase 6 —
/// per-subgraph publishing). Lets e.g. the `shapes` subgraph be published
/// while `concepts` stays draft.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubGraphStatus {
    pub graph_iri: String,
    pub status: VersionStatus,
}

/// Version response with RDF kind detection metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataModelVersionWithDetection {
    pub version: DataModelVersion,
    pub detected: Option<crate::kind_detector::RegistryKind>,
    pub mixed: bool,
    pub evidence: crate::kind_detector::Evidence,
}

// ─── Request bodies ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateDataModelRequest {
    pub title: String,
    pub namespace: String,
    pub description: Option<String>,
    pub is_public: Option<bool>,
    pub owner_type: Option<String>,
    pub owner_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateDataModelRequest {
    pub title: Option<String>,
    pub namespace: Option<String>,
    pub description: Option<String>,
    pub is_public: Option<bool>,
    pub owner_type: Option<String>,
    pub owner_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateVersionRequest {
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateDraftRequest {
    pub target_version: String,
    /// Optional commit message recorded with the draft creation.
    #[serde(default)]
    pub message: Option<String>,
}

/// Body for `POST /api/models/:id/branches` — start a named branch as a new
/// draft derived from `from_version`.
#[derive(Debug, Deserialize)]
pub struct CreateBranchRequest {
    /// Branch name (e.g. "feature-x"). Must be non-empty and unique per model.
    pub branch: String,
    /// Published/staged version the branch forks from (can be a branch tip).
    pub from_version: String,
    /// Optional explicit version string for the branch tip. Defaults to
    /// "{from_version}-{branch}" when omitted.
    #[serde(default)]
    pub target_version: Option<String>,
    /// Optional commit message recorded with the branch creation.
    #[serde(default)]
    pub message: Option<String>,
}

/// Body for `POST /api/models/:id/versions/:ver/rebase` — rebase a branch
/// version onto a newer base (defaults to the latest published version).
#[derive(Debug, Deserialize)]
pub struct RebaseRequest {
    /// The version to rebase onto. Defaults to `latest_published` when omitted.
    #[serde(default)]
    pub onto: Option<String>,
    /// Optional explicit version string for the rebased draft.
    #[serde(default)]
    pub target_version: Option<String>,
    /// Optional commit message recorded with the rebase.
    #[serde(default)]
    pub message: Option<String>,
}

/// Tip summary of one branch, returned by `GET /api/.../:id/branches`.
#[derive(Debug, serde::Serialize)]
pub struct BranchInfo {
    pub branch: String,
    pub tip_version: String,
    pub status: String,
    pub base_version: Option<String>,
    pub owner: Option<String>,
    pub created_at: String,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Debug, Deserialize)]
pub struct RdfTriple {
    pub s: String,
    pub p: String,
    pub o: serde_json::Value,
    pub graph: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PatchVersionRequest {
    pub add: Vec<RdfTriple>,
    pub remove: Vec<RdfTriple>,
    pub graph: Option<String>,
    /// Optional human commit message describing this change.
    #[serde(default)]
    pub message: Option<String>,
    /// Optional free-form provenance metadata stored with the commit.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// Body for per-subgraph lifecycle transitions
/// (`POST /api/models/:id/versions/:ver/subgraph/{stage,publish,deprecate}`).
#[derive(Debug, Deserialize)]
pub struct SubGraphActionRequest {
    /// Subgraph IRI or trailing suffix identifying which subgraph to transition.
    pub graph: String,
}

// ─── Query params ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct VersionDataParams {
    pub graph: Option<String>,
    pub format: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DiffParams {
    pub from: String,
    pub to: String,
    pub graph: Option<String>,
}

// ─── Diff output ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TripleView {
    pub s: String,
    pub p: String,
    pub o: String,
    pub graph: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedTriple {
    pub s: String,
    pub p: String,
    pub before: String,
    pub after: String,
    pub graph: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResult {
    pub added: Vec<TripleView>,
    pub removed: Vec<TripleView>,
    pub changed: Vec<ChangedTriple>,
    pub summary: DiffSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffSummary {
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
}
