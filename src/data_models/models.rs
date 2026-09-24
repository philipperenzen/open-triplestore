//! data-model versioning system.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Status of a data model version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
    #[schema(value_type = String, example = "vocabulary")]
    pub kind: crate::kind_detector::RegistryKind,
}

/// Metadata for a single data model version.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SubGraphStatus {
    pub graph_iri: String,
    pub status: VersionStatus,
}

// ─── Licence and attribution ──────────────────────────────────────────────────

/// A licence, by name and canonical URI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LicenseRef {
    /// Short name, e.g. "CC BY 4.0" or "W3C Document License (2023)".
    pub name: String,
    /// The licence's canonical URI.
    pub uri: String,
}

/// Licence and attribution of third-party content held by a registry entry or
/// version: the bundled standard vocabularies the server seeds as public
/// reference models (`src/data_models/seed_vocab.rs`), vocabularies installed
/// from the LOV corpus, and seed-bundle models that declare a licence
/// (`[data_models.license]`).
///
/// It is registry metadata, stored next to the record in the registry graph and
/// never written into the content's own graph, so the stored triples stay
/// exactly those of the source. For a bundled vocabulary every field is taken
/// from the file's comment header or from `frontend/public/vocab/NOTICE.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ContentAttribution {
    /// Where the content was loaded from: a bundled vocabulary file, relative
    /// to `/vocab/` (for example `dcat/2.0.0.ttl`); a graph of the LOV corpus;
    /// or, as `seed bundle <id>: <files>`, a seed bundle's payload files.
    pub file: String,
    /// The licence(s) the content is under. Empty when no licence is known.
    pub licenses: Vec<LicenseRef>,
    /// The rights holders' copyright notices, verbatim.
    pub copyright: Vec<String>,
    /// The statement the licence asks every copy to carry (for example the W3C
    /// derivative notice or DCMI's schema notice), verbatim.
    pub notice: Option<String>,
    /// The status of the source document, which the W3C Document License asks
    /// every copy to state.
    pub status: Option<String>,
    /// Where the content comes from.
    pub source_url: String,
    /// The specification this version belongs to.
    pub specification_url: Option<String>,
    /// How the bundled file differs from its source, as its header states.
    pub changes: Option<String>,
    /// How the stored copy relates to the bundled file.
    pub stored_copy: String,
    /// The stored triples were checked against the bundled file and are
    /// exactly its triples. `false` for a copy made in this registry (a draft,
    /// branch, merge or rebase) and for a seeded version whose content differs
    /// from the file or may since have been edited: downloads of those say
    /// they may have been modified. A record stored before this field existed
    /// reads as `false`.
    #[serde(default)]
    pub unchanged: bool,
    /// Further licence remarks (for example IMBOR's no-derivatives reading).
    pub remarks: Option<String>,
    /// The rights holder allows no altered copies: the content is redistributed
    /// only unmodified, and no notice is written into downloads of it. The
    /// registry refuses every way of making or publishing other content in an
    /// entry that holds such a record, and serves only the unchanged copy.
    pub no_derivatives: bool,
    /// The bundled file's own comment header, verbatim without the `# `
    /// prefixes. The store drops comments, so this is where the header lives
    /// on for the stored copy.
    pub header: Option<String>,
    /// Full attribution and licence texts for every bundled vocabulary,
    /// served by this server.
    pub notice_url: String,
}

/// A registry entry as the API returns it: the record plus the licence and
/// attribution of the content it holds (`null` for user models).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DataModelResponse {
    #[serde(flatten)]
    pub record: DataModelRecord,
    pub attribution: Option<ContentAttribution>,
}

/// A version as the API returns it: the version record plus the licence and
/// attribution of its content (`null` unless the server seeded it).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DataModelVersionResponse {
    #[serde(flatten)]
    pub version: DataModelVersion,
    pub attribution: Option<ContentAttribution>,
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
