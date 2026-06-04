//! Write-gating: before a write to a named graph lands, evaluate every
//! `gate_writes` pipeline whose scope covers that graph against the *incoming*
//! data (in a throwaway store) and reject the write if any fails its severity
//! threshold. This generalises the legacy per-dataset `shacl_on_write` gate to
//! reusable, composable pipelines.

use std::collections::BTreeSet;

use oxigraph::io::RdfFormat;

use crate::auth::db::AuthDb;
use crate::shacl::report::ValidationReport;
use crate::store::TripleStore;

use super::bindings;
use super::models::{SeverityThreshold, TargetKind, ValidationPipeline};
// Only the test `pipe()` builder constructs these write-target fields.
#[cfg(test)]
use super::models::{ResultsTarget, WriteTarget};
use super::store::ShaclStudioStore;

/// Returns `Err(report)` with the first failing gate's report when the incoming
/// data would violate a gating pipeline **or** a validation-layer binding that
/// applies to this write; `Ok(())` otherwise.
///
/// Two independent gate sources are evaluated:
/// 1. `gate_writes` pipelines whose scope covers the graph (at each pipeline's
///    own severity threshold);
/// 2. validation-layer bindings — shapes attached directly to the written graph
///    or to its owning dataset. A binding gates on its own (no pipeline needed)
///    at the default `Violation` threshold, so graph-attached shapes travel
///    with the graph and are enforced wherever it is mounted.
pub fn check_write_gates(
    main_store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    graph_iri: &str,
    data: &str,
    format: RdfFormat,
) -> Result<(), ValidationReport> {
    // Which dataset (if any) owns the graph being written — needed both for
    // pipelines scoped by dataset and for dataset-level bindings.
    let owning_dataset = auth_db.find_dataset_by_graph_iri(graph_iri).ok().flatten();

    // (a) Gating pipelines whose scope covers this graph.
    let gating = studio.list_gating_pipelines().unwrap_or_default();
    let covering: Vec<&ValidationPipeline> = gating
        .iter()
        .filter(|p| pipeline_covers_graph(p, graph_iri, owning_dataset.as_ref().map(|d| d.id.as_str())))
        .collect();

    // (b) Bindings that apply to this write: shapes on the written graph itself
    // plus dataset-level shapes on its owner.
    let mut binding_graphs: BTreeSet<String> = bindings::bindings_for_target(main_store, graph_iri)
        .into_iter()
        .collect();
    if let Some(ds) = &owning_dataset {
        let ds_iri = bindings::dataset_target_iri(base_url, &ds.id);
        binding_graphs.extend(bindings::bindings_for_target(main_store, &ds_iri));
    }

    if covering.is_empty() && binding_graphs.is_empty() {
        return Ok(());
    }

    // Union of shape-graph graphs needed (pipelines + bindings), resolved once.
    let mut needed_graphs: BTreeSet<String> = binding_graphs.clone();
    for p in &covering {
        for set_id in &p.shape_graph_ids {
            if let Ok(Some(set)) = studio.get_shape_graph(set_id) {
                needed_graphs.insert(set.graph_iri);
            }
        }
    }
    if needed_graphs.is_empty() {
        return Ok(());
    }

    // Build a temp store: incoming data + the shapes (copied from the live store).
    let temp = match TripleStore::in_memory() {
        Ok(t) => t,
        Err(_) => return Ok(()), // never block a write on our own infra hiccup
    };
    if temp.load_str(data, format, Some(graph_iri)).is_err() {
        // Malformed data — let the normal write path surface the parse error.
        return Ok(());
    }
    for g in &needed_graphs {
        if let Ok(bytes) = main_store.dump(RdfFormat::Turtle, Some(g)) {
            if let Ok(ttl) = String::from_utf8(bytes) {
                let _ = temp.load_str(&ttl, RdfFormat::Turtle, Some(g));
            }
        }
    }

    let data_graphs = [graph_iri.to_string()];

    // Pipeline gates (each at its own severity threshold). Inference is never
    // run here — gating must not mutate any store.
    for p in &covering {
        let shape_graphs: Vec<String> = p
            .shape_graph_ids
            .iter()
            .filter_map(|id| studio.get_shape_graph(id).ok().flatten().map(|s| s.graph_iri))
            .collect();
        if shape_graphs.is_empty() {
            continue;
        }
        match super::run::run_validation(&temp, &shape_graphs, &data_graphs, p.severity_threshold, false) {
            Ok(outcome) if !outcome.passes => return Err(outcome.report),
            _ => {}
        }
    }

    // Binding gates — enforced at the default Violation threshold.
    if !binding_graphs.is_empty() {
        let shape_graphs: Vec<String> = binding_graphs.into_iter().collect();
        match super::run::run_validation(&temp, &shape_graphs, &data_graphs, SeverityThreshold::Violation, false) {
            Ok(outcome) if !outcome.passes => return Err(outcome.report),
            _ => {}
        }
    }

    Ok(())
}

fn pipeline_covers_graph(p: &ValidationPipeline, graph_iri: &str, dataset_id: Option<&str>) -> bool {
    // Explicit graph coverage — legacy `graph_iris` or a `Graph` target.
    if p.graph_iris.iter().any(|g| g == graph_iri) {
        return true;
    }
    if p.targets.iter().any(|t| t.kind == TargetKind::Graph && t.id == graph_iri) {
        return true;
    }
    // Dataset coverage — a `Dataset` target always covers its graphs; the legacy
    // `dataset_ids` only do so when no explicit `graph_iris` narrow the scope
    // (preserving the historical "empty graph_iris = all dataset graphs").
    if let Some(ds) = dataset_id {
        if p.targets.iter().any(|t| t.kind == TargetKind::Dataset && t.id == ds) {
            return true;
        }
        if p.graph_iris.is_empty() && p.dataset_ids.iter().any(|d| d == ds) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::models::{OwnerType, Visibility};
    use crate::shacl_studio::models::ValidationTarget;

    fn pipe(targets: Vec<ValidationTarget>, dataset_ids: Vec<&str>, graph_iris: Vec<&str>) -> ValidationPipeline {
        ValidationPipeline {
            id: "p".into(),
            name: "p".into(),
            description: None,
            owner_type: OwnerType::User,
            owner_id: "u".into(),
            visibility: Visibility::Private,
            targets,
            dataset_ids: dataset_ids.into_iter().map(String::from).collect(),
            graph_iris: graph_iris.into_iter().map(String::from).collect(),
            target_classes: vec![],
            shape_graph_ids: vec![],
            severity_threshold: SeverityThreshold::Violation,
            run_inference: false,
            max_results: None,
            trigger_on_write: false,
            schedule_cron: None,
            gate_writes: true,
            retention: 50,
            inferred_target: WriteTarget::InPlace,
            inferred_target_graph: None,
            results_target: ResultsTarget::None,
            results_target_graph: None,
            last_run_at: None,
            last_conforms: None,
            created_by: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn graph_target(iri: &str) -> ValidationTarget {
        ValidationTarget { kind: TargetKind::Graph, id: iri.into() }
    }
    fn dataset_target(id: &str) -> ValidationTarget {
        ValidationTarget { kind: TargetKind::Dataset, id: id.into() }
    }

    #[test]
    fn legacy_graph_and_dataset_scope_still_covers() {
        let p = pipe(vec![], vec!["ds1"], vec![]);
        assert!(pipeline_covers_graph(&p, "urn:g", Some("ds1")));
        assert!(!pipeline_covers_graph(&p, "urn:g", Some("other")));

        let p = pipe(vec![], vec!["ds1"], vec!["urn:only"]);
        // Explicit graph narrows scope: the dataset no longer blanket-covers.
        assert!(pipeline_covers_graph(&p, "urn:only", Some("ds1")));
        assert!(!pipeline_covers_graph(&p, "urn:other", Some("ds1")));
    }

    #[test]
    fn graph_target_covers_that_graph() {
        let p = pipe(vec![graph_target("urn:g")], vec![], vec![]);
        assert!(pipeline_covers_graph(&p, "urn:g", None));
        assert!(!pipeline_covers_graph(&p, "urn:h", None));
    }

    #[test]
    fn dataset_target_covers_its_graphs() {
        let p = pipe(vec![dataset_target("ds1")], vec![], vec![]);
        assert!(pipeline_covers_graph(&p, "urn:any", Some("ds1")));
        assert!(!pipeline_covers_graph(&p, "urn:any", Some("ds2")));
        assert!(!pipeline_covers_graph(&p, "urn:any", None));
    }
}
