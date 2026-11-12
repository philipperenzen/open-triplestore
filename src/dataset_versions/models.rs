//! Dataset versioning system — snapshots of a dataset's named graphs with a
//! draft→staged→published→deprecated lifecycle and named branches.
//!
//! Unlike data-models/vocabularies (whose "working copy" *is* a version graph),
//! a dataset's working copy is its live set of registered named graphs. A
//! dataset *version* is an immutable snapshot of (selected) live graphs copied
//! into version-scoped named graphs, plus a `source → snapshot` mapping used to
//! restore a snapshot back onto the live graphs.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::store::set_hash::SetHash;

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

    /// The snapshot graphs left once the source graphs in `hidden` are taken
    /// out. With nothing hidden, all of them — a record cut before
    /// `source_map` existed included. Otherwise only the snapshots the map
    /// ties to a source graph that is not hidden: one it does not map goes
    /// too, since nothing says which graph it copies (fail-closed).
    pub fn snapshots_outside(&self, hidden: &HashSet<String>) -> Vec<String> {
        if hidden.is_empty() {
            return self.snapshot_graphs.clone();
        }
        self.source_map
            .iter()
            .filter(|m| !hidden.contains(&m.source_graph))
            .map(|m| m.snapshot_graph.clone())
            .collect()
    }

    /// Reduce the record to the graphs [`Self::snapshots_outside`] leaves,
    /// for a caller who may not read the source graphs in `hidden`: the two
    /// graph lists name a graph (and, in a snapshot's IRI, its slug), and the
    /// totals say how much it holds.
    ///
    /// When a graph is taken out, `quad_count` and `content_hash` become the
    /// totals of the graphs that remain — `None` when one of those has no
    /// figure of its own. The registry's totals would not do: the count less
    /// the remaining graphs' is the hidden graphs' size, and the hash is
    /// additive (`store::set_hash`), so the same subtraction gives a hash of
    /// their content, to test a guess against. `change_count` is over all of a
    /// delta's graphs and cannot be split from the record, so it goes.
    ///
    /// A record no hidden graph is part of is left exactly as it is. Only for
    /// what is sent to that caller: never store the result, and never build
    /// or restore a version from it.
    pub fn hide_sources(&mut self, hidden: &HashSet<String>) {
        if hidden.is_empty() {
            return;
        }
        let kept = self.snapshots_outside(hidden);
        let whole = self.snapshot_graphs.iter().all(|g| kept.contains(g))
            && self
                .source_map
                .iter()
                .all(|m| !hidden.contains(&m.source_graph));
        if whole {
            return;
        }
        self.source_map
            .retain(|m| !hidden.contains(&m.source_graph));
        self.snapshot_graphs = kept;
        if self.quad_count.is_some() {
            self.quad_count = self.source_map.iter().map(|m| m.quad_count).sum();
        }
        if self.content_hash.is_some() {
            self.content_hash = self
                .source_map
                .iter()
                .try_fold(SetHash::new(), |mut total, m| {
                    total.merge(&m.content_hash.as_deref().and_then(SetHash::from_hex)?);
                    Some(total)
                })
                .map(SetHash::to_hex);
        }
        self.change_count = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    const PUB: &str = "urn:t:public";
    const PRIV: &str = "urn:t:private";

    fn hash_of(element: &str) -> SetHash {
        let mut h = SetHash::new();
        h.add_element(element);
        h
    }

    /// A version of two graphs, one quad in `PUB` and two in `PRIV`, with the
    /// totals a cut records.
    fn record() -> DatasetVersion {
        let mut public = GraphMapping::new("urn:v:1/public".into(), PUB.into());
        public.quad_count = Some(1);
        public.content_hash = Some(hash_of("a").to_hex());
        let mut private = GraphMapping::new("urn:v:1/private".into(), PRIV.into());
        private.quad_count = Some(2);
        let mut h = hash_of("b");
        h.merge(&hash_of("c"));
        private.content_hash = Some(h.to_hex());
        let mut total = hash_of("a");
        total.merge(&h);
        let mut v = DatasetVersion::checkpoint(
            "ds",
            "1.0.0",
            VersionStatus::Published,
            "urn:v:1".into(),
            vec![public, private],
            None,
            None,
            None,
        );
        v.quad_count = Some(3);
        v.content_hash = Some(total.to_hex());
        v
    }

    fn hidden(graphs: &[&str]) -> HashSet<String> {
        graphs.iter().map(|g| g.to_string()).collect()
    }

    #[test]
    fn a_hidden_source_leaves_the_record_with_its_snapshot_and_totals() {
        let mut v = record();
        v.change_count = Some(7);
        v.hide_sources(&hidden(&[PRIV]));
        assert_eq!(v.snapshot_graphs, vec!["urn:v:1/public".to_string()]);
        assert_eq!(v.covered_graphs(), vec![PUB.to_string()]);
        assert_eq!(v.quad_count, Some(1));
        assert_eq!(v.content_hash, Some(hash_of("a").to_hex()));
        assert_eq!(v.change_count, None);
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("private"), "{json}");
    }

    #[test]
    fn a_record_without_a_hidden_source_is_left_as_it_is() {
        for set in [hidden(&[]), hidden(&["urn:t:another"])] {
            let mut v = record();
            v.change_count = Some(7);
            let before = serde_json::to_value(&v).unwrap();
            v.hide_sources(&set);
            assert_eq!(serde_json::to_value(&v).unwrap(), before);
        }
    }

    /// A snapshot the map does not tie to a source graph: kept while nothing
    /// is hidden (a record cut before the map existed), dropped once anything
    /// is — it may be a copy of the hidden graph.
    #[test]
    fn an_unmapped_snapshot_goes_once_anything_is_hidden() {
        let mut legacy = record();
        legacy.snapshot_graphs.push("urn:v:1/unmapped".into());

        let mut v = legacy.clone();
        v.hide_sources(&hidden(&[]));
        assert_eq!(v.snapshot_graphs.len(), 3);

        // Hidden graphs the version does not cover still drop it.
        let mut v = legacy.clone();
        v.hide_sources(&hidden(&["urn:t:another"]));
        assert_eq!(
            v.snapshot_graphs,
            vec!["urn:v:1/public".to_string(), "urn:v:1/private".to_string()]
        );
        assert_eq!(v.source_map.len(), 2);

        // A record with no map at all lists nothing.
        let mut v = legacy;
        v.source_map.clear();
        v.hide_sources(&hidden(&[PRIV]));
        assert!(v.snapshot_graphs.is_empty());
        assert_eq!(v.quad_count, Some(0));
        assert_eq!(v.content_hash, Some(SetHash::new().to_hex()));
    }

    #[test]
    fn totals_are_left_out_when_a_remaining_graph_has_none() {
        let mut v = record();
        v.source_map[0].quad_count = None;
        v.source_map[0].content_hash = None;
        v.hide_sources(&hidden(&[PRIV]));
        assert_eq!(v.quad_count, None);
        assert_eq!(v.content_hash, None);

        // A record that never had totals does not gain them.
        let mut v = record();
        v.quad_count = None;
        v.content_hash = None;
        v.hide_sources(&hidden(&[PRIV]));
        assert_eq!(v.quad_count, None);
        assert_eq!(v.content_hash, None);
    }
}
