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
    /// IRI of the version's base graph: the primary (or merged) named graph.
    /// An upload names it after the version and keeps the content in the
    /// sub-graphs under it (or in the base graph itself when merged); a
    /// seed bundle names its first graph.
    pub graph_iri: String,
    /// IRIs of every graph that holds the version's content, the base graph
    /// included when it is one of them. Empty for a version recorded without
    /// sub-graphs, whose content is the base graph. See
    /// [`DataModelVersion::content_graphs`].
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

impl DataModelVersion {
    /// The graphs that hold this version's triples: its sub-graphs, or, for a
    /// version recorded with none, its base graph. This is what a download
    /// serves, a revision hash covers and a delete drops.
    ///
    /// The base graph is *not* added to a non-empty list: for an upload it is
    /// only the name the sub-graphs sit under and holds nothing, and a graph
    /// that does hold content is listed among the sub-graphs (a merged upload
    /// lists itself, and so does a seed bundle's first graph). Code that asks
    /// which graphs a record *names*, to guard or authorise them, wants the
    /// wider set of [`super::content_digest::version_graphs`].
    pub fn content_graphs(&self) -> Vec<String> {
        if self.sub_graphs.is_empty() {
            vec![self.graph_iri.clone()]
        } else {
            self.sub_graphs.clone()
        }
    }
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

/// How a draft patch finds one blank node that is already in the draft.
///
/// Either `id`, or a description: the statement the node is the object of
/// (`s` and `p`), the statements it has itself (`where`), or both. A
/// description must fit exactly one blank node of the graph.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlankNodeLocator {
    /// The store's identifier of the node: its label in a read of the draft
    /// (`_:` in front is allowed).
    #[serde(default)]
    pub id: Option<String>,
    /// With `p`: the node is the object of this statement. An IRI, or the
    /// `_:label` of another declared node.
    #[serde(default)]
    pub s: Option<String>,
    #[serde(default)]
    pub p: Option<String>,
    /// Statements the node is the subject of.
    #[serde(default, rename = "where")]
    pub has: Vec<NodeStatement>,
    /// The graph to look in, as on a statement; the request's graph when absent.
    #[serde(default)]
    pub graph: Option<String>,
}

/// One statement a blank node has, in a [`BlankNodeLocator`]: `o` is written
/// as in [`RdfTriple`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeStatement {
    pub p: String,
    pub o: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct PatchVersionRequest {
    #[serde(default)]
    pub add: Vec<RdfTriple>,
    #[serde(default)]
    pub remove: Vec<RdfTriple>,
    /// Blank nodes of the draft this request refers to, by label: `_:label`
    /// in `add` and `remove` is then that node rather than a new one.
    #[serde(default)]
    pub nodes: std::collections::BTreeMap<String, BlankNodeLocator>,
    /// Labels declared in `nodes` to remove with everything below them.
    #[serde(default)]
    pub remove_nodes: Vec<String>,
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
    /// The subgraph to transition: its full IRI, or its name in the version
    /// (`shapes` for `{version graph}/shapes`).
    pub graph: String,
}

// ─── Query params ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct VersionDataParams {
    pub graph: Option<String>,
    pub format: Option<String>,
}

/// `DELETE /api/models/:id/versions/:ver?force=true` — `force` lets a
/// published version go; it never overrides datasets that depend on it.
#[derive(Debug, Default, Deserialize)]
pub struct DeleteVersionParams {
    #[serde(default)]
    pub force: bool,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn version(graph_iri: &str, sub_graphs: &[&str]) -> DataModelVersion {
        DataModelVersion {
            data_model_id: "m".into(),
            version: "1".into(),
            status: VersionStatus::Published,
            graph_iri: graph_iri.into(),
            sub_graphs: sub_graphs.iter().map(|g| g.to_string()).collect(),
            created_at: "2026-01-01T00:00:00Z".into(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: vec![],
        }
    }

    #[test]
    fn content_graphs_are_the_sub_graphs_or_else_the_base_graph() {
        assert_eq!(version("urn:g:base", &[]).content_graphs(), ["urn:g:base"]);
        assert_eq!(
            version("urn:g:base", &["urn:g:a", "urn:g:b"]).content_graphs(),
            ["urn:g:a", "urn:g:b"],
            "an upload's base graph is a name, not content"
        );
        assert_eq!(
            version("urn:g:a", &["urn:g:a", "urn:g:b"]).content_graphs(),
            ["urn:g:a", "urn:g:b"],
            "a base graph that holds content lists itself"
        );
    }
}
