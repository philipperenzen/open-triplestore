//! The premises of a repair run and the throwaway store they are copied into
//! (`docs/notes/repair-layer-design.md` §5).
//!
//! What is read: the dataset's graphs (less its persisted validation-report
//! graph) that the caller may read, the graphs of the model version it
//! conforms to that the caller may read, its materialised entailment graph
//! when it has one, and the shape graphs that apply to it. Rule bodies read a
//! narrower set — the premises: the data-bearing graphs (the reasoning
//! roles, linksets only under `sameas-full`), the entailment graphs and the
//! model graphs; never shapes, system, provenance or catalog graphs. Heads
//! land only in the targets: a registered graph of the dataset whose role is
//! none, instances, domain values or linkset (§4.6).
//!
//! Everything is copied by quad into an in-memory store through one
//! consistent read ([`TripleStore::quads_for_graphs_snapshot`]), so blank-node
//! labels are kept and nothing the chase does is observable outside the
//! request; dropping the store is the rollback.

use std::collections::{BTreeSet, HashSet};
use std::time::Instant;

use oxigraph::model::{GraphName, NamedNode};

use crate::auth::models::{Dataset, GraphKind};
use crate::reasoning::identity::IdentityPolicy;
use crate::store::graph_snapshot::{SnapshotError, SnapshotSource};
use crate::store::TripleStore;

/// The default per-request quad cap when the memory limit cannot be read:
/// the query accelerator's floor (`parallel_mirror::DEFAULT_MAX_TRIPLES`).
const FALLBACK_MAX_QUADS: usize = 2_000_000;
/// Bytes per quad a sandbox is budgeted at: half the accelerator's two-copy
/// figure (`BYTES_PER_TRIPLE_BOTH_COPIES = 1024`).
const BYTES_PER_QUAD: u64 = 512;
/// The share of the memory limit one sandbox may take (the SHACL run
/// index's divisor).
const MEMORY_DIVISOR: u64 = 8;

/// `OTS_REPAIR_MAX_QUADS`, else the memory-derived default (§5.4): never
/// clamped upwards, so a small container gets a small cap.
pub fn max_quads() -> usize {
    if let Some(n) = std::env::var("OTS_REPAIR_MAX_QUADS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
    {
        return n;
    }
    match crate::store::parallel_mirror::detect_memory_limit_bytes() {
        Some(bytes) => (bytes / MEMORY_DIVISOR / BYTES_PER_QUAD) as usize,
        None => FALLBACK_MAX_QUADS,
    }
}

/// What a run reads and where it may write.
#[derive(Debug, Clone, Default)]
pub struct Premises {
    /// The dataset's graphs the caller may read (seeded; validated).
    pub dataset_graphs: Vec<String>,
    /// Dataset graphs left out because the caller may not read them.
    pub withheld: usize,
    /// The conformed model version's graphs the caller may read.
    pub model_graphs: Vec<String>,
    /// What rule bodies read.
    pub premises: Vec<String>,
    /// Where heads may land.
    pub targets: BTreeSet<String>,
    /// The shape graphs that apply.
    pub shapes: Vec<String>,
    /// Shape graphs left out because the caller may not read them.
    pub shapes_withheld: usize,
    /// The materialised entailment graph, when the dataset has one the run
    /// may read.
    pub entailment_graph: Option<String>,
    /// `fresh`, `stale`, `off` or `withheld` (§5.2).
    pub entailment: &'static str,
    pub identity: IdentityPolicy,
}

impl Premises {
    /// The graphs the validator reads for this dataset (the dataset's graphs
    /// and its model graphs, as `POST …/validate` does).
    pub fn validation_graphs(&self) -> Vec<String> {
        let mut out = self.dataset_graphs.clone();
        for g in &self.model_graphs {
            if !out.contains(g) {
                out.push(g.clone());
            }
        }
        out
    }

    /// Every graph copied into the sandbox, deduplicated, in a fixed order.
    pub fn seeded(&self) -> Vec<String> {
        let mut set: BTreeSet<String> = BTreeSet::new();
        set.extend(self.dataset_graphs.iter().cloned());
        set.extend(self.model_graphs.iter().cloned());
        set.extend(self.premises.iter().cloned());
        set.extend(self.shapes.iter().cloned());
        set.into_iter().collect()
    }

    /// Their summed quad counts from the count index.
    pub fn quad_estimate(&self, store: &TripleStore) -> usize {
        self.seeded()
            .iter()
            .filter_map(|g| store.graph_count_cached(Some(g)))
            .sum()
    }
}

/// Whether a dataset graph of this role is a premise (`conformance`'s
/// reasoning roles, linksets only when the identity policy includes them;
/// entailment graphs too).
fn is_premise(role: Option<GraphKind>, identity: IdentityPolicy) -> bool {
    match role {
        None
        | Some(GraphKind::Instances)
        | Some(GraphKind::Model)
        | Some(GraphKind::Vocabulary)
        | Some(GraphKind::DomainValues)
        | Some(GraphKind::Entailment) => true,
        Some(GraphKind::Linkset) => identity.includes_linksets(),
        _ => false,
    }
}

/// Whether a head may land in a dataset graph of this role.
fn is_target(role: Option<GraphKind>) -> bool {
    matches!(
        role,
        None | Some(GraphKind::Instances)
            | Some(GraphKind::DomainValues)
            | Some(GraphKind::Linkset)
    )
}

/// The inputs [`resolve`] needs, so it can be called without an `AppState`
/// in tests.
pub struct ResolveInput<'a> {
    pub state: &'a crate::server::AppState,
    pub user: &'a crate::auth::middleware::AuthenticatedUser,
    pub dataset: &'a Dataset,
    /// Shape graphs (already resolved and filtered to what the caller reads).
    pub shapes: Vec<String>,
    pub shapes_withheld: usize,
    /// `scope.graphs`: restrict the dataset graphs seeded.
    pub scope_graphs: Option<&'a [String]>,
}

/// Work out the premises (§5.2), with the caller's rights.
pub fn resolve(input: ResolveInput<'_>) -> Result<Premises, String> {
    let ResolveInput {
        state,
        user,
        dataset,
        shapes,
        shapes_withheld,
        scope_graphs,
    } = input;
    let readable: Option<HashSet<String>> = if user.is_admin() {
        None
    } else {
        Some(
            crate::server::routes::accessible_read_graphs(state, Some(user))
                .map_err(|e| e.message())?,
        )
    };
    let may_read = |g: &str| readable.as_ref().is_none_or(|r| r.contains(g));
    let identity = crate::entailment::effective_identity(&state.auth_db, dataset).policy;
    let entries = state
        .auth_db
        .list_dataset_graph_entries(&dataset.id)
        .map_err(|e| e.to_string())?;
    let mut out = Premises {
        identity,
        shapes,
        shapes_withheld,
        ..Premises::default()
    };
    for e in entries {
        if e.graph_iri.starts_with("urn:system:reports:") {
            continue;
        }
        if let Some(scope) = scope_graphs {
            if !scope.contains(&e.graph_iri) {
                continue;
            }
        }
        if !may_read(&e.graph_iri) {
            out.withheld += 1;
            continue;
        }
        if is_premise(e.graph_role, identity) {
            out.premises.push(e.graph_iri.clone());
        }
        if is_target(e.graph_role) {
            out.targets.insert(e.graph_iri.clone());
        }
        out.dataset_graphs.push(e.graph_iri);
    }
    out.model_graphs = crate::conformance::model_graphs_for_dataset(
        &state.store,
        &state.auth_db,
        &state.base_url,
        dataset,
        Some(user.user_id.as_str()),
    );
    for g in &out.model_graphs {
        if !out.premises.contains(g) {
            out.premises.push(g.clone());
        }
        // A model graph is read-only for rules, even when the dataset also
        // registers it.
        out.targets.remove(g);
    }
    // The materialised entailment graph, when the dataset has one: read only
    // when nothing the reasoner read was withheld from the caller (its
    // consequences would carry what they may not see).
    out.entailment = "off";
    if let Ok(Some(cfg)) = crate::entailment::config(&state.auth_db, &dataset.id) {
        if cfg.mode == "materialize" {
            // A declared model the caller may not read: its consequences are
            // in the entailment graph too.
            let model_withheld = dataset
                .conforms_to_model
                .as_deref()
                .is_some_and(|m| !m.is_empty())
                && out.model_graphs.is_empty()
                && !user.is_admin();
            if out.withheld > 0 || model_withheld {
                out.entailment = "withheld";
            } else {
                out.premises.push(cfg.graph.clone());
                out.entailment_graph = Some(cfg.graph.clone());
                out.entailment =
                    entailment_freshness(state, &out.dataset_graphs, cfg.last_run_at.as_deref());
            }
        }
    }
    out.premises.sort();
    out.premises.dedup();
    out.shapes.sort();
    out.shapes.dedup();
    Ok(out)
}

/// `stale` when the entailment graph's last run predates the newest commit
/// touching the dataset's graphs (or never ran), `fresh` otherwise.
fn entailment_freshness(
    state: &crate::server::AppState,
    graphs: &[String],
    last_run_at: Option<&str>,
) -> &'static str {
    let Some(last) = last_run_at else {
        return "stale";
    };
    let newest = crate::commit_log::list_commits(
        &state.store,
        &crate::commit_log::CommitScope::Graphs(graphs.to_vec()),
        &crate::commit_log::CommitQuery {
            limit: Some(1),
            ..Default::default()
        },
    );
    match newest.first() {
        Some(c) if c.created_at.as_str() > last => "stale",
        _ => "fresh",
    }
}

/// A sandbox and what its seeding saw.
pub struct Seeded {
    pub store: TripleStore,
    pub source: SnapshotSource,
    pub quads: usize,
}

/// Why seeding stopped.
pub enum SeedError {
    Deadline(String),
    Store(String),
}

/// A fresh in-memory store with nothing between it and the evaluator: no
/// result cache (the chase writes it directly), no accelerator, no change
/// log.
pub fn empty_sandbox() -> Result<TripleStore, String> {
    Ok(TripleStore::in_memory()
        .map_err(|e| e.to_string())?
        .with_query_cache(false, 0, 0)
        .with_parallel_query(false, 1, 0)
        .with_change_capture_disabled())
}

/// Copy `graphs` of `main` into a fresh sandbox through one consistent read.
pub fn seed(main: &TripleStore, graphs: &[String], deadline: Instant) -> Result<Seeded, SeedError> {
    let names: Vec<GraphName> = graphs
        .iter()
        .filter_map(|g| NamedNode::new(g).ok().map(GraphName::NamedNode))
        .collect();
    let snap = main
        .quads_for_graphs_snapshot(&names, Some(deadline))
        .map_err(|e| match e {
            SnapshotError::Deadline { .. } => SeedError::Deadline(e.to_string()),
            SnapshotError::Store(e) => SeedError::Store(e.to_string()),
        })?;
    let sandbox = empty_sandbox().map_err(SeedError::Store)?;
    let quads = snap.quads.len();
    sandbox
        .bulk_insert_quads(snap.quads, graphs)
        .map_err(|e| SeedError::Store(e.to_string()))?;
    Ok(Seeded {
        store: sandbox,
        source: snap.source.unwrap_or(SnapshotSource::Live),
        quads,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premise_and_target_roles() {
        assert!(is_premise(None, IdentityPolicy::Narrow));
        assert!(is_premise(Some(GraphKind::Model), IdentityPolicy::Narrow));
        assert!(!is_premise(Some(GraphKind::Shapes), IdentityPolicy::Narrow));
        assert!(!is_premise(
            Some(GraphKind::Provenance),
            IdentityPolicy::Full
        ));
        assert!(!is_premise(
            Some(GraphKind::Linkset),
            IdentityPolicy::Narrow
        ));
        assert!(is_premise(Some(GraphKind::Linkset), IdentityPolicy::Full));
        assert!(is_target(Some(GraphKind::Instances)));
        assert!(!is_target(Some(GraphKind::Model)));
        assert!(!is_target(Some(GraphKind::Entailment)));
        assert!(!is_target(Some(GraphKind::Shapes)));
        assert!(!is_target(Some(GraphKind::Provenance)));
        assert!(!is_target(Some(GraphKind::System)));
    }

    #[test]
    fn the_cap_is_read_from_the_environment_or_memory() {
        // Without the variable the default is memory-derived or the floor;
        // either way positive.
        assert!(max_quads() > 0);
    }
}
