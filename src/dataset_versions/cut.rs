//! Cutting a dataset version: a **delta** when one can be chained from a
//! checkpoint, a **checkpoint** — a physical copy — otherwise (delta note
//! §3.1-§3.2).
//!
//! A delta is cut when change capture is recording, the version is not
//! Published (a published version is always physical, so it is in every
//! backup), no checkpoint was asked for, none of its graphs is shared with
//! another dataset or is a version's own graph, and an *anchored* checkpoint
//! of the current epoch covers its graphs with an intact chain of rows from
//! that checkpoint to now. It is not cut when `OTS_CHECKPOINT_EVERY` (100)
//! deltas already chain from that checkpoint, or the changes since it exceed
//! `OTS_CHECKPOINT_FRACTION` (0.25) of its size — then a new checkpoint
//! keeps chains short. A delta copies nothing: it records its base, its range
//! of the change log, and a content hash computed from the base's and the
//! range's, and pins the range against retention.
//!
//! A checkpoint copies its graphs, hashing them on the way. It is *anchored*
//! — usable as a base — when change capture recorded every write: the change
//! log's head is read before the copy, and if no row touched the copied
//! graphs between that read and the end of the copy, the copy is exactly the
//! graphs' content at that sequence number. A write that raced the copy
//! leaves it unanchored, which costs only that the next cut is a checkpoint
//! too; nothing is guessed.
//!
//! The caller holds the dataset's version lock (`lock::dataset`).

use std::collections::HashSet;

use super::models::{Anchor, DatasetVersion, PatchRange, VersionKind, VersionStatus};
use super::{registry, snapshot};
use crate::store::changes::{self, Chain};
use crate::store::set_hash::SetHash;
use crate::store::TripleStore;

pub const DEFAULT_CHECKPOINT_EVERY: usize = 100;
pub const DEFAULT_CHECKPOINT_FRACTION: f64 = 0.25;
/// Below this many changed quads the size rule never forces a checkpoint:
/// a small base is cheap to copy, but a handful of edits is cheaper still.
const SIZE_RULE_FLOOR: i64 = 1_000;

fn checkpoint_every() -> usize {
    crate::config::CHECKPOINT_EVERY.get()
}

fn checkpoint_fraction() -> f64 {
    crate::config::CHECKPOINT_FRACTION.get()
}

/// What to cut.
#[derive(Debug, Clone)]
pub struct CutRequest {
    pub dataset_id: String,
    pub version: String,
    pub source_graphs: Vec<String>,
    pub status: VersionStatus,
    pub created_by: Option<String>,
    pub notes: Option<String>,
    pub branch: Option<String>,
    pub derived_from: Option<String>,
    /// Cut a checkpoint even where a delta could be cut.
    pub force_checkpoint: bool,
    /// Source graphs also registered to another dataset: a delta never covers
    /// them (delta note §8.5 Q5). See [`shared_graphs`].
    pub shared: HashSet<String>,
    /// The model version the dataset's instances were pinned to when the
    /// version is cut (`registry::dataset_pin`), recorded on the version so it
    /// keeps saying which model version it conformed to.
    pub conforms_to_model: Option<String>,
    pub conforms_to_version: Option<String>,
}

/// The cut version, and why it is a checkpoint when it is one.
#[derive(Debug, Clone)]
pub struct Cut {
    pub record: DatasetVersion,
    pub checkpoint_reason: Option<String>,
}

/// The name of the retention pin a delta holds on its range.
pub fn pin_name(version_iri: &str) -> String {
    format!("version:{version_iri}")
}

fn version_iri(base_url: &str, dataset_id: &str, version: &str) -> String {
    format!("{base_url}/dataset/{dataset_id}/version/{version}")
}

/// The graphs among `graphs` that another dataset registers too.
pub fn shared_graphs(
    auth_db: &crate::auth::db::AuthDb,
    dataset_id: &str,
    graphs: &[String],
) -> HashSet<String> {
    graphs
        .iter()
        .filter(|g| {
            auth_db
                .datasets_with_graph(g)
                .map(|ds| ds.iter().any(|d| d != dataset_id))
                // Fail closed: an unreadable registry makes the graph count
                // as shared, which only costs a checkpoint.
                .unwrap_or(true)
        })
        .cloned()
        .collect()
}

/// Cut `req.version`. The caller has checked that it does not exist yet.
pub fn cut(store: &TripleStore, base_url: &str, req: &CutRequest) -> Result<Cut, String> {
    crate::data_models::version_iri::validate_version(&req.version)?;
    if req.source_graphs.is_empty() {
        return Err("a version needs at least one graph".into());
    }
    match plan_delta(store, base_url, req) {
        Ok(plan) => cut_delta(store, base_url, req, plan).map(|record| Cut {
            record,
            checkpoint_reason: None,
        }),
        Err(reason) => cut_checkpoint(store, base_url, req).map(|record| Cut {
            record,
            checkpoint_reason: Some(reason),
        }),
    }
}

struct DeltaPlan {
    base: DatasetVersion,
    from: i64,
    head: i64,
    epoch: String,
    chain: Chain,
}

/// Whether `req` can be a delta, and on what; the reason it cannot otherwise.
fn plan_delta(store: &TripleStore, base_url: &str, req: &CutRequest) -> Result<DeltaPlan, String> {
    let log = store.changes();
    if !log.enabled() {
        return Err("change capture is off".into());
    }
    if req.force_checkpoint {
        return Err("a checkpoint was asked for".into());
    }
    if req.status == VersionStatus::Published {
        return Err("a published version is always physical".into());
    }
    if let Some(g) = req
        .source_graphs
        .iter()
        .find(|g| changes::is_version_snapshot_graph(g) || g.starts_with("urn:ots:restore:"))
    {
        return Err(format!("<{g}> is a version's own graph"));
    }
    if let Some(g) = req.source_graphs.iter().find(|g| req.shared.contains(*g)) {
        return Err(format!("<{g}> is registered to another dataset too"));
    }
    let epoch = log.epoch().to_string();
    let versions = registry::list_versions(store, base_url, &req.dataset_id);
    let base = versions
        .iter()
        .filter(|v| !v.is_delta() && !v.chain_broken)
        .filter(|v| v.anchor.as_ref().is_some_and(|a| a.epoch == epoch))
        .filter(|v| {
            req.source_graphs.iter().all(|g| {
                v.source_map
                    .iter()
                    .any(|m| m.source_graph == *g && m.content_hash.is_some())
            })
        })
        .max_by_key(|v| v.anchor.as_ref().map(|a| a.seq))
        .ok_or("no anchored checkpoint of this dataset covers these graphs")?
        .clone();
    let from = base.anchor.as_ref().map(|a| a.seq).unwrap_or_default();
    let head = log.head_seq();
    let chain = log.chain(&req.source_graphs, &epoch, from, head);
    if !chain.intact {
        return Err(chain
            .reason
            .clone()
            .unwrap_or_else(|| "the chain is broken".into()));
    }
    let on_base = versions
        .iter()
        .filter(|v| v.base_version.as_deref() == Some(base.version.as_str()))
        .count();
    let every = checkpoint_every();
    if on_base >= every {
        return Err(format!(
            "{on_base} deltas already chain from {} (OTS_CHECKPOINT_EVERY = {every})",
            base.version
        ));
    }
    let changed = chain.change_count();
    let base_quads: i64 = base
        .source_map
        .iter()
        .filter(|m| req.source_graphs.contains(&m.source_graph))
        .filter_map(|m| m.quad_count)
        .sum();
    let fraction = checkpoint_fraction();
    if changed > SIZE_RULE_FLOOR && changed as f64 > fraction * base_quads as f64 {
        return Err(format!(
            "{changed} quads changed since {}, over {:.0}% of its {base_quads}",
            base.version,
            fraction * 100.0
        ));
    }
    Ok(DeltaPlan {
        base,
        from,
        head,
        epoch,
        chain,
    })
}

fn cut_delta(
    store: &TripleStore,
    base_url: &str,
    req: &CutRequest,
    plan: DeltaPlan,
) -> Result<DatasetVersion, String> {
    let fold = changes::fold_rows(&plan.chain.rows);
    let mut map = snapshot::mapping(base_url, &req.dataset_id, &req.version, &req.source_graphs);
    let mut total_hash = SetHash::new();
    let mut total_quads = 0i64;
    for m in map.iter_mut() {
        let base_m = plan
            .base
            .source_map
            .iter()
            .find(|b| b.source_graph == m.source_graph)
            .ok_or("the base does not cover a requested graph")?;
        let mut hash = base_m
            .content_hash
            .as_deref()
            .and_then(SetHash::from_hex)
            .ok_or("the base has no content hash for a requested graph")?;
        let mut quads = base_m.quad_count.unwrap_or_default();
        if let Some(net) = fold.get(&Some(m.source_graph.clone())) {
            for q in &net.added {
                hash.add_quad(q, Some(&m.source_graph));
            }
            for q in &net.removed {
                hash.remove_quad(q, Some(&m.source_graph));
            }
            quads += net.added.len() as i64 - net.removed.len() as i64;
        }
        m.content_hash = Some(hash.to_hex());
        m.quad_count = Some(quads);
        total_hash.merge(&hash);
        total_quads += quads;
    }
    let graph_iri = version_iri(base_url, &req.dataset_id, &req.version);
    let mut record = DatasetVersion::checkpoint(
        &req.dataset_id,
        &req.version,
        req.status,
        graph_iri.clone(),
        map,
        req.created_by.clone(),
        req.notes.clone(),
        req.branch.clone(),
    );
    record.derived_from = req.derived_from.clone();
    record.conforms_to_model = req.conforms_to_model.clone();
    record.conforms_to_version = req.conforms_to_version.clone();
    record.kind = VersionKind::Delta;
    record.base_version = Some(plan.base.version.clone());
    record.patch_range = Some(PatchRange {
        epoch: plan.epoch.clone(),
        from_seq: plan.from,
        to_seq: plan.head,
    });
    record.anchor = Some(Anchor {
        epoch: plan.epoch,
        seq: plan.head,
    });
    record.content_hash = Some(total_hash.to_hex());
    record.quad_count = Some(total_quads);
    record.change_count = Some(plan.chain.change_count());
    // Pin before the record exists: a sweep between the two must not cut
    // the range out from under it.
    store
        .changes()
        .set_pin(&pin_name(&graph_iri), plan.from + 1, Some(&req.dataset_id))
        .map_err(|e| e.to_string())?;
    if let Err(e) = registry::insert_version(store, base_url, &record) {
        store.changes().delete_pin(&pin_name(&graph_iri));
        return Err(e.to_string());
    }
    Ok(record)
}

fn cut_checkpoint(
    store: &TripleStore,
    base_url: &str,
    req: &CutRequest,
) -> Result<DatasetVersion, String> {
    let log = store.changes();
    let before = log.enabled().then(|| log.head_seq());
    let mut map = snapshot::mapping(base_url, &req.dataset_id, &req.version, &req.source_graphs);
    snapshot::copy_into(store, &mut map).map_err(|e| e.to_string())?;
    let anchor = before.and_then(|s0| {
        let s1 = log.head_seq();
        (!log.touched(&req.source_graphs, s0, s1)).then(|| Anchor {
            epoch: log.epoch().to_string(),
            seq: s0,
        })
    });
    let mut hash = SetHash::new();
    for m in &map {
        if let Some(h) = m.content_hash.as_deref().and_then(SetHash::from_hex) {
            hash.merge(&h);
        }
    }
    let quads: i64 = map.iter().filter_map(|m| m.quad_count).sum();
    let graph_iri = version_iri(base_url, &req.dataset_id, &req.version);
    let mut record = DatasetVersion::checkpoint(
        &req.dataset_id,
        &req.version,
        req.status,
        graph_iri,
        map,
        req.created_by.clone(),
        req.notes.clone(),
        req.branch.clone(),
    );
    record.derived_from = req.derived_from.clone();
    record.conforms_to_model = req.conforms_to_model.clone();
    record.conforms_to_version = req.conforms_to_version.clone();
    record.anchor = anchor;
    record.content_hash = Some(hash.to_hex());
    record.quad_count = Some(quads);
    registry::insert_version(store, base_url, &record).map_err(|e| e.to_string())?;
    Ok(record)
}

/// A branch of `from`: the same content under a new label, so — when `from`
/// is anchored — a delta on the same base (or on `from` itself when it is a
/// checkpoint) with no data copied. An unanchored checkpoint is copied as
/// before (delta note §3.3, `create_branch`). `pin` is the dataset's model
/// pin now (`registry::dataset_pin`), which the branch records.
pub fn branch(
    store: &TripleStore,
    base_url: &str,
    from: &DatasetVersion,
    target_version: &str,
    branch_name: &str,
    created_by: Option<String>,
    pin: (Option<String>, Option<String>),
) -> Result<DatasetVersion, String> {
    crate::data_models::version_iri::validate_version(target_version)?;
    let graph_iri = version_iri(base_url, &from.dataset_id, target_version);
    let sources: Vec<String> = from.covered_graphs();
    let mut map = snapshot::mapping(base_url, &from.dataset_id, target_version, &sources);
    for (m, src) in map.iter_mut().zip(from.source_map.iter()) {
        m.content_hash = src.content_hash.clone();
        m.quad_count = src.quad_count;
    }
    let mut record = DatasetVersion::checkpoint(
        &from.dataset_id,
        target_version,
        VersionStatus::Draft,
        graph_iri.clone(),
        map,
        created_by,
        None,
        Some(branch_name.to_string()),
    );
    record.derived_from = Some(from.version.clone());
    (record.conforms_to_model, record.conforms_to_version) = pin;
    record.content_hash = from.content_hash.clone();
    record.quad_count = from.quad_count;
    let chainable = from.anchor.is_some()
        && !from.chain_broken
        && from.source_map.iter().all(|m| m.content_hash.is_some());
    if !chainable || !store.changes().readable() {
        // A physical copy of the snapshots, as branches always were.
        let new_map = snapshot::clone_version(
            store,
            base_url,
            &from.dataset_id,
            &from.source_map,
            target_version,
        )
        .map_err(|e| e.to_string())?;
        record.snapshot_graphs = new_map.iter().map(|m| m.snapshot_graph.clone()).collect();
        record.source_map = new_map;
        record.anchor = from.anchor.clone();
        registry::insert_version(store, base_url, &record).map_err(|e| e.to_string())?;
        return Ok(record);
    }
    let anchor = from.anchor.clone().unwrap_or(Anchor {
        epoch: String::new(),
        seq: 0,
    });
    let (base, range) = if from.is_delta() {
        (from.base_version.clone(), from.patch_range.clone())
    } else {
        (
            Some(from.version.clone()),
            Some(PatchRange {
                epoch: anchor.epoch.clone(),
                from_seq: anchor.seq,
                to_seq: anchor.seq,
            }),
        )
    };
    let (Some(base), Some(range)) = (base, range) else {
        return Err("the version to branch from names no base".into());
    };
    record.kind = VersionKind::Delta;
    record.base_version = Some(base);
    record.change_count = Some(from.change_count.unwrap_or(0));
    record.anchor = Some(anchor);
    let from_seq = range.from_seq;
    record.patch_range = Some(range);
    store
        .changes()
        .set_pin(&pin_name(&graph_iri), from_seq + 1, Some(&from.dataset_id))
        .map_err(|e| e.to_string())?;
    if let Err(e) = registry::insert_version(store, base_url, &record) {
        store.changes().delete_pin(&pin_name(&graph_iri));
        return Err(e.to_string());
    }
    Ok(record)
}
