//! Data model for the consolidated SHACL Studio: reusable **ShapeGraph**s,
//! saved **ValidationPipeline**s (with triggers + write-gating), and
//! **PipelineRun** history.
//!
//! List-valued fields (dataset ids, graph iris, shape-graph ids, tags, target
//! classes) are persisted as JSON `TEXT` columns — the same convention the
//! `saved_queries` tables use for `parameters` — so the schema stays flat and
//! we avoid a fistful of many-to-many junction tables for a feature whose
//! cardinality is small (tens of pipelines, not millions).

use serde::{Deserialize, Serialize};

use crate::auth::models::{OwnerType, Visibility};
pub use crate::data_models::models::VersionStatus;

/// What a validation target points at. A pipeline (or a stored binding in the
/// validation layer) can validate a whole dataset, a single named graph, or a
/// shape graph treated *as data* (SHACL-of-SHACL meta-validation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    Dataset,
    Graph,
    /// Serialises as `"shapegraph"`. The `"shapeset"` alias keeps pipeline
    /// `targets` persisted before the shape-set→shape-graph rename readable.
    #[serde(alias = "shapeset")]
    ShapeGraph,
}

impl TargetKind {
    pub fn from_str_or_dataset(s: &str) -> Self {
        match s {
            "graph" => TargetKind::Graph,
            // Accept both the current and pre-rename spellings.
            "shape-graph" | "shapegraph" | "shape-set" | "shapeset" => TargetKind::ShapeGraph,
            _ => TargetKind::Dataset,
        }
    }
}

/// A single validation target: a kind plus its identifier. For `Dataset` the id
/// is the dataset id; for `Graph` it is the graph IRI; for `ShapeGraph` it is the
/// shape-graph id. Generalises the legacy `dataset_ids` / `graph_iris` scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationTarget {
    pub kind: TargetKind,
    pub id: String,
}

/// Where a shape graph's content originated. Purely informational — drives the
/// "source" facet/badge in the Library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeSource {
    /// Hand-authored in the editor.
    Manual,
    /// Induced from existing instance data (draft-from-data).
    Derived,
    /// Imported from a standard / external shape graph.
    Imported,
    /// Drafted by the LLM assistant.
    Ai,
}

impl ShapeSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            ShapeSource::Manual => "manual",
            ShapeSource::Derived => "derived",
            ShapeSource::Imported => "imported",
            ShapeSource::Ai => "ai",
        }
    }
    pub fn from_str_or_manual(s: &str) -> Self {
        match s {
            "derived" => ShapeSource::Derived,
            "imported" => ShapeSource::Imported,
            "ai" => ShapeSource::Ai,
            _ => ShapeSource::Manual,
        }
    }
}

/// A reusable SHACL shape graph — the Library's first-class artifact. Decoupled
/// from any single dataset: a pipeline composes one or more of these.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShapeGraph {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub visibility: Visibility,
    /// Named graph in the triplestore holding this set's Turtle.
    pub graph_iri: String,
    pub tags: Vec<String>,
    /// Cached list of `sh:targetClass` values, refreshed on save.
    pub target_classes: Vec<String>,
    /// Cached count of node + property shapes, refreshed on save.
    pub shape_count: i64,
    pub source: ShapeSource,
    /// Current revision number (head of `shape_graph_revisions`).
    pub version: i64,
    /// Lifecycle status (Draft → Staged → Published → Deprecated), shared with
    /// data models. Independent of gating: a draft set still gates.
    pub status: VersionStatus,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// An append-only Turtle snapshot of a shape graph, enabling rollback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShapeGraphRevision {
    pub shape_graph_id: String,
    pub revision: i64,
    /// Turtle omitted from list responses (size); populated on single fetch.
    #[serde(default)]
    pub turtle: String,
    pub note: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
}

/// The minimum severity that makes a run *fail* (and, for a gating pipeline,
/// blocks the write). `Violation` = only violations fail; `Warning` =
/// violations+warnings fail; `Info` = anything fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeverityThreshold {
    Violation,
    Warning,
    Info,
}

impl SeverityThreshold {
    pub fn as_str(&self) -> &'static str {
        match self {
            SeverityThreshold::Violation => "violation",
            SeverityThreshold::Warning => "warning",
            SeverityThreshold::Info => "info",
        }
    }
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "warning" => SeverityThreshold::Warning,
            "info" => SeverityThreshold::Info,
            _ => SeverityThreshold::Violation,
        }
    }
    /// Rank where a higher number = more severe. A result counts as failing when
    /// its severity rank ≥ this threshold's rank.
    pub fn rank(&self) -> u8 {
        match self {
            SeverityThreshold::Info => 1,
            SeverityThreshold::Warning => 2,
            SeverityThreshold::Violation => 3,
        }
    }
}

/// Where SHACL-AF/function-derived triples produced by a run are persisted.
/// `InPlace` materialises into the source data graphs (legacy behaviour);
/// `NewGraph` writes only the newly-derived triples to a dedicated named graph
/// (tagged `GraphKind::Entailment`); `NewVersion` materialises in place and then
/// snapshots a new dataset version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WriteTarget {
    #[default]
    InPlace,
    NewGraph,
    NewVersion,
}

impl WriteTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            WriteTarget::InPlace => "in_place",
            WriteTarget::NewGraph => "new_graph",
            WriteTarget::NewVersion => "new_version",
        }
    }
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "new_graph" => WriteTarget::NewGraph,
            "new_version" => WriteTarget::NewVersion,
            _ => WriteTarget::InPlace,
        }
    }
}

/// Where validation results (serialised as standard `sh:ValidationReport` RDF)
/// are persisted. `None` keeps results in the run history only (legacy). The
/// other variants additionally write the report graph in place, to a dedicated
/// named graph, or into a new dataset version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ResultsTarget {
    #[default]
    None,
    InPlace,
    NewGraph,
    NewVersion,
}

impl ResultsTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResultsTarget::None => "none",
            ResultsTarget::InPlace => "in_place",
            ResultsTarget::NewGraph => "new_graph",
            ResultsTarget::NewVersion => "new_version",
        }
    }
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "in_place" => ResultsTarget::InPlace,
            "new_graph" => ResultsTarget::NewGraph,
            "new_version" => ResultsTarget::NewVersion,
            _ => ResultsTarget::None,
        }
    }
    pub fn is_enabled(&self) -> bool {
        !matches!(self, ResultsTarget::None)
    }
}

/// A saved, named validation configuration: a data scope + composed shape graphs
/// + options + triggers. Runnable manually, on-write, or on a schedule, and —
/// when `gate_writes` is set — able to reject non-conforming writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationPipeline {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub visibility: Visibility,

    // ── Data scope ──────────────────────────────────────────────────────────
    /// Generalised scope: datasets, graphs, and/or shape graphs (meta-validation).
    /// Additive over the legacy `dataset_ids`/`graph_iris` fields, which are
    /// still honoured for back-compat (and for the "empty graphs = all dataset
    /// graphs" gating semantics).
    #[serde(default)]
    pub targets: Vec<ValidationTarget>,
    /// Datasets in scope. When `graph_iris` is empty, all of each dataset's
    /// graphs are validated; otherwise only the listed graphs.
    pub dataset_ids: Vec<String>,
    pub graph_iris: Vec<String>,
    /// Optional class filter (advanced). Empty = validate all targets.
    pub target_classes: Vec<String>,

    // ── Shapes ──────────────────────────────────────────────────────────────
    pub shape_graph_ids: Vec<String>,

    // ── Options ─────────────────────────────────────────────────────────────
    pub severity_threshold: SeverityThreshold,
    /// Run SHACL-AF inference before validating (manual/scheduled runs only —
    /// never mutates the store during write-gating).
    pub run_inference: bool,
    pub max_results: Option<i64>,

    // ── Derived-data write targets ────────────────────────────────────────────
    /// Where inferred/function-derived triples are persisted (default in place).
    #[serde(default)]
    pub inferred_target: WriteTarget,
    /// Explicit destination graph for `inferred_target = NewGraph`; when unset a
    /// deterministic `urn:system:inferred:{pipeline_id}` graph is used.
    #[serde(default)]
    pub inferred_target_graph: Option<String>,
    /// Whether/where the validation report (as RDF) is persisted (default none).
    #[serde(default)]
    pub results_target: ResultsTarget,
    /// Explicit destination graph for `results_target = NewGraph`; when unset a
    /// deterministic `urn:system:reports:{pipeline_id}` graph is used.
    #[serde(default)]
    pub results_target_graph: Option<String>,

    // ── Triggers ────────────────────────────────────────────────────────────
    pub trigger_on_write: bool,
    /// Standard 5-field cron expression (UTC), or `None` for no schedule.
    pub schedule_cron: Option<String>,
    pub gate_writes: bool,

    // ── Bookkeeping ─────────────────────────────────────────────────────────
    pub retention: i64,
    pub last_run_at: Option<String>,
    pub last_conforms: Option<bool>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// One execution of a pipeline. Generalises the legacy per-dataset
/// `ShaclValidationRun`. The full report is stored as JSON and omitted from
/// list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineRun {
    pub id: String,
    pub pipeline_id: String,
    /// "manual" | "write" | "schedule".
    pub triggered_by: String,
    pub actor: Option<String>,
    pub ran_at: String,
    pub conforms: bool,
    pub results_count: i64,
    pub violation_count: i64,
    pub warning_count: i64,
    pub info_count: i64,
    pub duration_ms: i64,
    /// Full `ValidationReport`; `None` in list summaries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<crate::shacl::report::ValidationReport>,
}
