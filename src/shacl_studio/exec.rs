//! Execute a pipeline against the live store and persist a `PipelineRun`.
//! Shared by the manual-run handler and the scheduler so both produce
//! identical, recorded runs.

use oxigraph::io::RdfFormat;
use oxigraph::model::{GraphName, NamedNode, Quad};

use crate::auth::db::AuthDb;
use crate::auth::models::GraphKind;
use crate::store::TripleStore;

use super::bindings;
use super::models::{PipelineRun, ResultsTarget, TargetKind, ValidationPipeline, WriteTarget};
use super::store::ShaclStudioStore;

/// Resolve a pipeline's data scope to a concrete list of graph IRIs. Additive
/// over the legacy `graph_iris`/`dataset_ids` fields:
/// * explicit `graph_iris` (legacy) — as-is;
/// * each legacy `dataset_ids` dataset's graphs — only when no explicit graphs
///   are set, preserving the historical "empty graph_iris = all dataset graphs";
/// * each `targets` entry — a Dataset's graphs, a Graph IRI directly, or a
///   ShapeGraph's data graph (meta-validation validates the shapes *as data*).
pub fn resolve_data_graphs(
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    pipeline: &ValidationPipeline,
) -> Vec<String> {
    let mut graphs: Vec<String> = pipeline.graph_iris.clone();

    if pipeline.graph_iris.is_empty() {
        for ds in &pipeline.dataset_ids {
            if let Ok(g) = auth_db.list_dataset_graphs(ds) {
                graphs.extend(g);
            }
        }
    }

    for t in &pipeline.targets {
        match t.kind {
            TargetKind::Dataset => {
                if let Ok(g) = auth_db.list_dataset_graphs(&t.id) {
                    graphs.extend(g);
                }
            }
            TargetKind::Graph => graphs.push(t.id.clone()),
            TargetKind::ShapeGraph => {
                if let Ok(Some(set)) = studio.get_shape_graph(&t.id) {
                    graphs.push(set.graph_iri);
                }
            }
        }
    }

    graphs.sort();
    graphs.dedup();
    graphs
}

/// Resolve a pipeline's shapes to backing graph IRIs. Additive:
/// * each composed `shape_graph_ids` set — its data graph;
/// * for every dataset in scope (legacy `dataset_ids` + Dataset `targets`), the
///   *effective* shape graphs bound to it in the validation layer — including
///   shapes inherited from the dataset's graphs (dynamic inheritance);
/// * for every Graph target, the shapes bound directly to that graph;
/// * for any ShapeGraph target, the built-in SHACL-SHACL meta-shapes (the set's
///   own data graph is added as *data* by `resolve_data_graphs`, so the pair
///   performs SHACL-of-SHACL meta-validation).
pub fn resolve_shape_graphs(
    store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    pipeline: &ValidationPipeline,
) -> Vec<String> {
    let mut graphs: Vec<String> = pipeline
        .shape_graph_ids
        .iter()
        .filter_map(|id| {
            studio
                .get_shape_graph(id)
                .ok()
                .flatten()
                .map(|s| s.graph_iri)
        })
        .collect();

    // Datasets in scope → their effective (bound + inherited) shape graphs.
    let mut dataset_ids: Vec<&str> = pipeline.dataset_ids.iter().map(String::as_str).collect();
    for t in &pipeline.targets {
        if t.kind == TargetKind::Dataset {
            dataset_ids.push(&t.id);
        }
    }
    for ds_id in dataset_ids {
        if let Ok(Some(ds)) = auth_db.get_dataset(ds_id) {
            for set in
                bindings::effective_shape_graphs_for_dataset(store, auth_db, studio, base_url, &ds)
            {
                graphs.push(set.graph_iri);
            }
        }
    }

    // Graph targets → shapes bound directly to those graphs.
    for t in &pipeline.targets {
        if t.kind == TargetKind::Graph {
            graphs.extend(bindings::bindings_for_target(store, &t.id));
        }
    }

    // ShapeGraph targets → the built-in SHACL-SHACL meta-shapes (meta-validation).
    if pipeline
        .targets
        .iter()
        .any(|t| t.kind == TargetKind::ShapeGraph)
    {
        graphs.push(super::seed::SHACL_SHACL_GRAPH.to_string());
    }

    graphs.sort();
    graphs.dedup();
    graphs
}

/// Does the pipeline's owning principal (`created_by`) currently have write access to `graph`?
///
/// Every derived write and in-place inference is gated on this so a pipeline can never write (or
/// materialise inference into) a graph its owner couldn't write directly. It is re-evaluated on
/// each run — manual **and** scheduled — so the scheduler runs with the creator's authority rather
/// than ambient authority, and a later loss of grants takes effect immediately. Admins (the
/// pipeline owner being an admin) bypass, mirroring `acl::check_graph_permission`.
fn owner_can_write(auth_db: &AuthDb, created_by: &Option<String>, graph: &str) -> bool {
    let Some(uid) = created_by.as_deref() else {
        return false;
    };
    match auth_db.get_user_by_id(uid) {
        Ok(Some(u)) if u.is_admin() => true,
        Ok(Some(u)) => auth_db
            .check_graph_permission(uid, u.role.as_str(), graph, "write")
            .unwrap_or(false),
        _ => false,
    }
}

/// Run the pipeline now against `main_store`, store the run + report, and update
/// the pipeline's last-run bookkeeping. `triggered_by` is "manual" | "schedule".
pub fn execute_pipeline(
    main_store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    pipeline: &ValidationPipeline,
    triggered_by: &str,
    actor: Option<&str>,
) -> anyhow::Result<PipelineRun> {
    let started = std::time::Instant::now();
    let data_graphs = resolve_data_graphs(auth_db, studio, pipeline);
    let shape_graphs = resolve_shape_graphs(main_store, auth_db, studio, base_url, pipeline);

    // In-place SHACL-AF inference mutates the data graphs themselves; only allow it when the
    // pipeline's owner may write every data graph in scope, otherwise validate read-only. This
    // stops a pipeline (manual or scheduled) from materialising triples into graphs its owner
    // cannot write — the cross-tenant in-place-tamper path.
    let infer_ok = pipeline.run_inference
        && data_graphs
            .iter()
            .all(|g| owner_can_write(auth_db, &pipeline.created_by, g));
    if pipeline.run_inference && !infer_ok {
        tracing::warn!(
            "shacl pipeline {}: inference disabled this run — owner lacks write on all data graphs",
            pipeline.id
        );
    }

    let (outcome, inferred_quads) = super::run::run_validation_capturing(
        main_store,
        &shape_graphs,
        &data_graphs,
        pipeline.severity_threshold,
        infer_ok,
    )
    .map_err(|e| anyhow::anyhow!(e))?;

    let duration_ms = started.elapsed().as_millis() as i64;
    let report_json = serde_json::to_string(&outcome.report).unwrap_or_else(|_| "{}".to_string());

    let run = studio.insert_pipeline_run(
        &pipeline.id,
        triggered_by,
        actor,
        outcome.passes,
        outcome.report.results_count as i64,
        outcome.violation_count,
        outcome.warning_count,
        outcome.info_count,
        duration_ms,
        &report_json,
        pipeline.retention,
    )?;
    studio.touch_pipeline_run(&pipeline.id, &run.ran_at, outcome.passes)?;

    // Route any derived data (inferred triples + the report-as-RDF) to the
    // pipeline's configured destinations. Best-effort: the run is already
    // recorded, so a persistence hiccup here must not fail the whole run.
    persist_derived(
        main_store,
        auth_db,
        base_url,
        pipeline,
        &data_graphs,
        &inferred_quads,
        &outcome.report,
        &run.id,
    );

    Ok(PipelineRun {
        report: Some(outcome.report),
        ..run
    })
}

/// Persist a run's derived outputs according to the pipeline's write-target
/// options. Inferred triples (already materialised in place by the engine) are
/// additionally copied to a dedicated graph and/or captured in a new version;
/// the validation report is serialised to standard `sh:ValidationReport` RDF and
/// written to its configured destination. All steps are best-effort.
#[allow(clippy::too_many_arguments)] // cohesive persist inputs; a struct adds churn
fn persist_derived(
    store: &TripleStore,
    auth_db: &AuthDb,
    base_url: &str,
    pipeline: &ValidationPipeline,
    data_graphs: &[String],
    inferred: &[Quad],
    report: &crate::shacl::report::ValidationReport,
    run_id: &str,
) {
    // 1. Inferred triples → dedicated graph (in-place is already done by infer).
    //    A caller-supplied (explicit) target graph must be writable by the pipeline owner; the
    //    auto-namespaced default `urn:system:inferred:{id}` is server-owned and pipeline-scoped.
    if pipeline.run_inference
        && !inferred.is_empty()
        && pipeline.inferred_target == WriteTarget::NewGraph
    {
        let explicit = pipeline
            .inferred_target_graph
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());
        let target = pipeline
            .inferred_target_graph
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("urn:system:inferred:{}", pipeline.id));
        if !explicit || owner_can_write(auth_db, &pipeline.created_by, &target) {
            write_quads_to_graph(store, inferred, &target);
            register_derived_graph(auth_db, data_graphs, &target, GraphKind::Entailment);
        } else {
            tracing::warn!(
                "shacl pipeline {}: skipped inferred write to <{}> (owner lacks write access)",
                pipeline.id,
                target
            );
        }
    }

    // 2. Validation report → RDF, written to its destination. Same rule: an explicit results
    //    graph must be writable by the owner; the default per-pipeline report graph is exempt.
    if pipeline.results_target.is_enabled() {
        let explicit = matches!(pipeline.results_target, ResultsTarget::NewGraph)
            && pipeline
                .results_target_graph
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty());
        let target = match pipeline.results_target {
            ResultsTarget::NewGraph => pipeline
                .results_target_graph
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("urn:system:reports:{}", pipeline.id)),
            // in_place / new_version both accumulate into the per-pipeline report graph
            _ => format!("urn:system:reports:{}", pipeline.id),
        };
        if explicit && !owner_can_write(auth_db, &pipeline.created_by, &target) {
            tracing::warn!(
                "shacl pipeline {}: skipped report write to <{}> (owner lacks write access)",
                pipeline.id,
                target
            );
        } else {
            let report_iri = format!("{target}#run-{run_id}");
            let ttl = super::report_rdf::report_to_turtle(report, &report_iri);
            // POST (append) so successive runs accumulate rather than overwrite.
            let _ = store.graph_store_post(Some(&target), &ttl, RdfFormat::Turtle);
            if matches!(
                pipeline.results_target,
                ResultsTarget::InPlace | ResultsTarget::NewGraph
            ) {
                register_derived_graph(auth_db, data_graphs, &target, GraphKind::System);
            }
        }
    }

    // 3. New version — snapshot the affected datasets when either target asks.
    if pipeline.inferred_target == WriteTarget::NewVersion
        || pipeline.results_target == ResultsTarget::NewVersion
    {
        snapshot_affected_versions(store, auth_db, base_url, pipeline, data_graphs);
    }
}

/// Re-home `quads` into `target` (they still carry their source graph name) and
/// bulk-insert them.
fn write_quads_to_graph(store: &TripleStore, quads: &[Quad], target: &str) {
    let Ok(nn) = NamedNode::new(target) else {
        return;
    };
    let gn = GraphName::NamedNode(nn);
    let rehomed: Vec<Quad> = quads
        .iter()
        .map(|q| {
            Quad::new(
                q.subject.clone(),
                q.predicate.clone(),
                q.object.clone(),
                gn.clone(),
            )
        })
        .collect();
    let _ = store.bulk_insert_quads(rehomed, &[target.to_string()]);
}

/// Attach a derived graph to its owning dataset (with a role) — but only when
/// the pipeline's data scope maps to exactly one dataset, so we never guess
/// which dataset a cross-dataset derived graph belongs to.
fn register_derived_graph(auth_db: &AuthDb, data_graphs: &[String], target: &str, role: GraphKind) {
    let mut owner: Option<String> = None;
    for g in data_graphs {
        if let Ok(Some(ds)) = auth_db.find_dataset_by_graph_iri(g) {
            match &owner {
                None => owner = Some(ds.id),
                Some(existing) if *existing == ds.id => {}
                Some(_) => return, // scope spans multiple datasets — leave unattached
            }
        }
    }
    if let Some(ds_id) = owner {
        let _ = auth_db.add_dataset_graph(&ds_id, target);
        let _ = auth_db.set_dataset_graph_role(&ds_id, target, Some(role));
    }
}

/// Snapshot a new draft version for every dataset whose graphs are in scope,
/// capturing the freshly-materialised derived triples alongside the source data.
fn snapshot_affected_versions(
    store: &TripleStore,
    auth_db: &AuthDb,
    base_url: &str,
    pipeline: &ValidationPipeline,
    data_graphs: &[String],
) {
    use std::collections::BTreeMap;
    let mut by_ds: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for g in data_graphs {
        if let Ok(Some(ds)) = auth_db.find_dataset_by_graph_iri(g) {
            by_ds.entry(ds.id).or_default().push(g.clone());
        }
    }
    let version = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    for (ds_id, source_graphs) in by_ds {
        let source_map = match crate::dataset_versions::snapshot::snapshot_graphs(
            store,
            base_url,
            &ds_id,
            &version,
            &source_graphs,
        ) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    "pipeline {} version snapshot for {ds_id} failed: {e}",
                    pipeline.id
                );
                continue;
            }
        };
        let snapshot_graphs: Vec<String> = source_map
            .iter()
            .map(|m| m.snapshot_graph.clone())
            .collect();
        let record = crate::dataset_versions::models::DatasetVersion {
            dataset_id: ds_id.clone(),
            version: version.clone(),
            status: crate::dataset_versions::models::VersionStatus::Draft,
            graph_iri: format!("{base_url}/dataset/{ds_id}/version/{version}"),
            snapshot_graphs,
            source_map,
            created_at: chrono::Utc::now().to_rfc3339(),
            created_by: Some(format!("pipeline:{}", pipeline.id)),
            derived_from: None,
            notes: Some(format!(
                "Auto-versioned by validation pipeline '{}'",
                pipeline.name
            )),
            branch: None,
        };
        if let Err(e) = crate::dataset_versions::registry::insert_version(store, base_url, &record)
        {
            tracing::warn!(
                "pipeline {} insert_version for {ds_id} failed: {e}",
                pipeline.id
            );
            continue;
        }
        let _ = crate::dataset_versions::registry::update_latest_draft(
            store, base_url, &ds_id, &version,
        );
    }
}

/// Run the pipeline now but **do not persist** anything: no run row is written
/// and the pipeline's last-run bookkeeping is left untouched. Used by the
/// "test run" mode so users can check what a pipeline would report without it
/// counting officially. Returns a transient `PipelineRun` (id `"test"`).
pub fn execute_pipeline_dry(
    main_store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    pipeline: &ValidationPipeline,
    actor: Option<&str>,
) -> anyhow::Result<PipelineRun> {
    let started = std::time::Instant::now();
    let data_graphs = resolve_data_graphs(auth_db, studio, pipeline);
    let shape_graphs = resolve_shape_graphs(main_store, auth_db, studio, base_url, pipeline);

    // Even a "test" run materialises inference in place against the live store, so apply the same
    // owner-write gate as a real run before letting it mutate any data graph.
    let infer_ok = pipeline.run_inference
        && data_graphs
            .iter()
            .all(|g| owner_can_write(auth_db, &pipeline.created_by, g));

    let outcome = super::run::run_validation(
        main_store,
        &shape_graphs,
        &data_graphs,
        pipeline.severity_threshold,
        infer_ok,
    )
    .map_err(|e| anyhow::anyhow!(e))?;

    let duration_ms = started.elapsed().as_millis() as i64;
    Ok(PipelineRun {
        id: "test".to_string(),
        pipeline_id: pipeline.id.clone(),
        triggered_by: "test".to_string(),
        actor: actor.map(|s| s.to_string()),
        ran_at: chrono::Utc::now().to_rfc3339(),
        conforms: outcome.passes,
        results_count: outcome.report.results_count as i64,
        violation_count: outcome.violation_count,
        warning_count: outcome.warning_count,
        info_count: outcome.info_count,
        duration_ms,
        report: Some(outcome.report),
    })
}
