//! Write-gating: before a write to a named graph lands, evaluate every
//! `gate_writes` pipeline whose scope covers that graph against the *incoming*
//! data (in a throwaway store) and reject the write if any fails its severity
//! threshold. This generalises the legacy per-dataset `shacl_on_write` gate to
//! reusable, composable pipelines.

use std::collections::BTreeSet;

use oxigraph::io::RdfFormat;
use oxigraph::model::{GraphName, NamedNode, Quad};

use crate::auth::db::AuthDb;
use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::Dataset;
use crate::shacl::report::ValidationReport;
use crate::store::TripleStore;

use super::bindings;
use super::models::{SeverityThreshold, TargetKind, ValidationPipeline};

// Only the test `pipe()` builder constructs these write-target fields.
#[cfg(test)]
use super::models::{ResultsTarget, WriteTarget};
use super::store::ShaclStudioStore;

/// The stores and configuration every gate lookup needs, and who is writing.
///
/// These travel together through `discover_gates`, `check_write_gates` and
/// `check_import_gates`; bundling them keeps those signatures readable (and
/// under clippy's argument-count limit) as gates gain parameters of their own.
#[derive(Clone, Copy)]
pub struct GateContext<'a> {
    pub main_store: &'a TripleStore,
    pub auth_db: &'a AuthDb,
    pub studio: &'a ShaclStudioStore,
    pub base_url: &'a str,
    /// The writer a refusal's report goes to (`None`: an anonymous writer).
    /// It carries the gate's shapes, so it is theirs only as far as they may
    /// read them ([`report_for_writer`]).
    pub writer: Option<&'a AuthenticatedUser>,
}

/// A gate's refusal: the failing gate's report, and the shape graphs that
/// gate validated against (none for a gate that could not be evaluated).
type Refusal = Box<(ValidationReport, Vec<String>)>;

fn refusal(report: ValidationReport, shape_graphs: Vec<String>) -> Refusal {
    Box::new((report, shape_graphs))
}

/// A refusal's report as `writer` (`None`: anonymous) may see it.
///
/// A report names its shapes (`sh:sourceShape`), their paths and their
/// messages, and a default message quotes a constraint's parameters. So when
/// the refusing gate validated against a graph some dataset holds as private
/// that the writer may not read by the `/sparql` rule
/// ([`crate::auth::acl::private_graph_withheld`]), the writer gets only that
/// the write was refused, and by how many results. The gate still gates: this
/// changes what the refusal says, never whether the write is refused. A
/// lookup error withholds.
pub(crate) fn report_for_writer(
    auth_db: &AuthDb,
    writer: Option<&AuthenticatedUser>,
    report: ValidationReport,
    shape_graphs: &[String],
) -> ValidationReport {
    use crate::shacl::report::{Severity, ValidationResult};
    let withheld = shape_graphs
        .iter()
        .any(|g| crate::auth::acl::private_graph_withheld(auth_db, writer, g).unwrap_or(true));
    if !withheld {
        return report;
    }
    ValidationReport {
        conforms: false,
        results: vec![ValidationResult {
            severity: Severity::Violation,
            focus_node: String::new(),
            path: None,
            value: None,
            source_shape: String::new(),
            source_constraint: "withheld".to_string(),
            message: format!(
                "The write does not conform to the shapes that gate this graph ({} result(s)). \
                 Some of those shapes are in a graph you may not read, so the details are \
                 withheld: ask the dataset's writers.",
                report.results_count
            ),
        }],
        results_count: 1,
        metrics: None,
    }
}

/// How the incoming data will be applied to the target graph.
///
/// Validation must see the graph as it will be AFTER the write. Validating the
/// payload in isolation is only correct for a replace: for a merge it is wrong
/// in both directions — a POST adding a second `ex:name` passes `sh:maxCount 1`
/// because the temp store holds only the new value, and a POST supplying one
/// missing property is rejected by `sh:minCount 1` on every property the
/// payload does not repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// PUT — the payload replaces the graph, so it is the whole future state.
    Replace,
    /// POST — the payload is merged, so the future state is existing + payload.
    Merge,
}

/// Copy the target graph's current contents into the throwaway store, so a
/// merge is validated against the state the write will actually produce.
fn seed_existing_graph(
    main_store: &TripleStore,
    temp: &TripleStore,
    graph_iri: &str,
) -> Result<(), ValidationReport> {
    let bytes = main_store
        .dump(RdfFormat::Turtle, Some(graph_iri))
        .map_err(|e| gate_error(format!("reading existing graph <{graph_iri}>: {e}")))?;
    let ttl = String::from_utf8(bytes)
        .map_err(|e| gate_error(format!("graph <{graph_iri}> is not valid UTF-8: {e}")))?;
    temp.load_str(&ttl, RdfFormat::Turtle, Some(graph_iri))
        .map_err(|e| gate_error(format!("staging existing graph <{graph_iri}>: {e}")))
}

/// A gate that could not be evaluated blocks the write.
///
/// Every failure below used to return `Ok(())` — "let the write through" — so a
/// temp store that would not allocate, a shape graph that failed to copy, or a
/// SHACL engine error silently disabled validation for that request. The write
/// then landed unvalidated and nothing anywhere said so, which is the one
/// outcome a write gate must never produce. Since the gate signals rejection
/// with a `ValidationReport`, an infrastructure failure is reported as a
/// non-conforming report naming the reason, so the caller's existing 422 path
/// carries it to the client.
pub(crate) fn gate_error(reason: impl std::fmt::Display) -> ValidationReport {
    use crate::shacl::report::{Severity, ValidationResult};
    let message = format!(
        "SHACL write gate could not be evaluated, so the write was refused: {reason}. This is a \
         server-side failure, not a data problem — the write was not applied."
    );
    ValidationReport {
        conforms: false,
        results: vec![ValidationResult {
            severity: Severity::Violation,
            focus_node: String::new(),
            path: None,
            value: None,
            source_shape: String::new(),
            source_constraint: "gate-evaluation-failure".to_string(),
            message,
        }],
        results_count: 1,
        metrics: None,
    }
}

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
    ctx: GateContext<'_>,
    graph_iri: &str,
    data: &str,
    format: RdfFormat,
    mode: WriteMode,
) -> Result<(), ValidationReport> {
    let GateContext {
        main_store,
        auth_db,
        studio,
        base_url,
        writer,
    } = ctx;
    // The legacy per-dataset `shacl_on_write` gate is handled separately by
    // `validate_on_write` on this path, so it is excluded here.
    let gates = discover_gates(main_store, auth_db, studio, base_url, graph_iri, false)?;
    if gates.is_empty() {
        return Ok(());
    }
    // Once a gate is discovered, only a successful resolution of every shape
    // graph it names lets the write proceed: a pipeline whose shape graph
    // cannot be resolved is a gate that cannot be evaluated, not a gate that
    // no longer applies.
    let needed_graphs = needed_shape_graphs(studio, &gates)?;

    // Build a temp store: the graph's future contents + the shapes (copied from
    // the live store).
    let temp = TripleStore::in_memory().map_err(|e| gate_error(format!("temp store: {e}")))?;
    if mode == WriteMode::Merge {
        seed_existing_graph(main_store, &temp, graph_iri)?;
    }
    if temp.load_str(data, format, Some(graph_iri)).is_err() {
        // Malformed data — let the normal write path surface the parse error.
        // Safe to pass: the write itself will fail on the same parse.
        return Ok(());
    }
    copy_shape_graphs(main_store, &temp, &needed_graphs)?;

    evaluate_gates(&temp, studio, &gates, graph_iri).map_err(|refusal| {
        let (report, shapes) = *refusal;
        report_for_writer(auth_db, writer, report, &shapes)
    })
}

/// Cheap pre-check for bulk import: does *any* gate apply to writes into
/// `graph_iri`? Considers `gate_writes` pipelines, validation-layer bindings
/// (graph- and dataset-level) and the owning dataset's legacy `shacl_on_write`
/// shapes graph. Metadata lookups only — no quad scans, no temp store — so
/// large imports with no gates configured (the common case) pay near-nothing.
///
/// `true` when a lookup fails: whether a gate applies is then unknown, so the
/// import goes on to [`check_import_gates`], which discovers again and refuses
/// it unless the lookup now succeeds.
pub fn import_gates_apply(ctx: GateContext<'_>, graph_iri: &str) -> bool {
    match discover_gates(
        ctx.main_store,
        ctx.auth_db,
        ctx.studio,
        ctx.base_url,
        graph_iri,
        true,
    ) {
        Ok(gates) => !gates.is_empty(),
        Err(_) => true,
    }
}

/// Quad-based write gate for bulk import: validates `quads` (re-homed into
/// `graph_iri` in a throwaway store) against every gate that applies —
/// `gate_writes` pipelines, validation-layer bindings, and the owning dataset's
/// legacy `shacl_on_write` shapes graph (which Graph Store writes enforce in
/// `validate_on_write` but bulk import must enforce itself). `Err` carries the
/// first failing gate's report.
pub fn check_import_gates(
    ctx: GateContext<'_>,
    graph_iri: &str,
    quads: &[Quad],
) -> Result<(), ValidationReport> {
    let GateContext {
        main_store,
        auth_db,
        studio,
        base_url,
        writer,
    } = ctx;
    let gates = discover_gates(main_store, auth_db, studio, base_url, graph_iri, true)?;
    if gates.is_empty() {
        return Ok(());
    }
    let needed_graphs = needed_shape_graphs(studio, &gates)?;

    let temp = TripleStore::in_memory().map_err(|e| gate_error(format!("temp store: {e}")))?;
    let graph = GraphName::NamedNode(NamedNode::new(graph_iri).map_err(|e| {
        gate_error(format!(
            "target graph <{graph_iri}> is not a valid IRI: {e}"
        ))
    })?);
    // Insert the parsed quads directly (no serialise/re-parse round trip, which
    // would also relabel blank nodes), re-homed under the target graph.
    let rehomed: Vec<Quad> = quads
        .iter()
        .map(|q| {
            Quad::new(
                q.subject.clone(),
                q.predicate.clone(),
                q.object.clone(),
                graph.clone(),
            )
        })
        .collect();
    temp.bulk_insert_quads(rehomed, &[graph_iri.to_string()])
        .map_err(|e| gate_error(format!("staging the incoming quads: {e}")))?;
    copy_shape_graphs(main_store, &temp, &needed_graphs)?;

    evaluate_gates(&temp, studio, &gates, graph_iri).map_err(|refusal| {
        let (report, shapes) = *refusal;
        report_for_writer(auth_db, writer, report, &shapes)
    })
}

/// Compact human-readable summary of a failed validation report, for error
/// messages with no structured-report channel (bulk-import rejection).
pub fn summarize_report(report: &ValidationReport, max_results: usize) -> String {
    let mut parts: Vec<String> = report
        .results
        .iter()
        .take(max_results)
        .map(|r| {
            let mut s = format!("{:?} at <{}>", r.severity, r.focus_node);
            if let Some(p) = &r.path {
                s.push_str(&format!(" path <{p}>"));
            }
            if !r.message.is_empty() {
                s.push_str(&format!(": {}", r.message));
            }
            s
        })
        .collect();
    if report.results_count > max_results {
        parts.push(format!("… and {} more", report.results_count - max_results));
    }
    format!(
        "{} validation result(s) — {}",
        report.results_count,
        parts.join("; ")
    )
}

/// Everything that gates a write to one graph: covering `gate_writes`
/// pipelines, validation-layer binding shape graphs, and (import path only)
/// the owning dataset's legacy `shacl_on_write` shapes graph.
struct GateSet {
    pipelines: Vec<ValidationPipeline>,
    binding_graphs: BTreeSet<String>,
    legacy_shapes_graph: Option<String>,
}

impl GateSet {
    fn is_empty(&self) -> bool {
        self.pipelines.is_empty()
            && self.binding_graphs.is_empty()
            && self.legacy_shapes_graph.is_none()
    }
}

/// Every gate that applies to a write to `graph_iri`.
///
/// `Err` when a lookup fails. Each lookup used to fall back to "none" — the
/// owning dataset on `.ok().flatten()`, the gating pipelines on
/// `.unwrap_or_default()`, the bindings on an empty result — so a database
/// error dropped the gates it would have found and the write landed
/// unvalidated. A gate set that could not be discovered is a gate that could
/// not be evaluated, and refuses the write like one.
fn discover_gates(
    main_store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    graph_iri: &str,
    include_legacy_dataset_gate: bool,
) -> Result<GateSet, ValidationReport> {
    // Which dataset (if any) owns the graph being written — needed both for
    // pipelines scoped by dataset and for dataset-level bindings.
    let owning_dataset = auth_db
        .find_dataset_by_graph_iri(graph_iri)
        .map_err(|e| gate_error(format!("looking up the dataset holding <{graph_iri}>: {e}")))?;

    // (a) Gating pipelines whose scope covers this graph, by a route their
    // creator may still write.
    let pipelines: Vec<ValidationPipeline> = studio
        .list_gating_pipelines()
        .map_err(|e| gate_error(format!("listing the gating pipelines: {e}")))?
        .into_iter()
        .filter(|p| {
            pipeline_covers_graph(p, graph_iri, owning_dataset.as_ref().map(|d| d.id.as_str()))
        })
        .filter(
            |p| match creator_may_gate(auth_db, p, graph_iri, owning_dataset.as_ref()) {
                Ok(true) => true,
                Ok(false) => {
                    tracing::warn!(
                        "shacl gate: pipeline {} does not gate this write to <{graph_iri}>: \
                         its creator may no longer write what it covers",
                        p.id
                    );
                    false
                }
                Err(e) => {
                    tracing::warn!(
                        "shacl gate: pipeline {}: checking its creator's write access failed \
                         ({e}); the gate applies",
                        p.id
                    );
                    true
                }
            },
        )
        .collect();

    // (b) Bindings that apply to this write: shapes on the written graph itself
    // plus dataset-level shapes on its owner.
    let bindings_of = |target: &str| {
        bindings::try_bindings_for_target(main_store, target).map_err(|e| {
            gate_error(format!(
                "reading the validation-layer bindings of <{target}>: {e}"
            ))
        })
    };
    let mut binding_graphs: BTreeSet<String> = bindings_of(graph_iri)?.into_iter().collect();
    if let Some(ds) = &owning_dataset {
        let ds_iri = bindings::dataset_target_iri(base_url, &ds.id);
        binding_graphs.extend(bindings_of(&ds_iri)?);
    }

    // (c) Legacy per-dataset gate (`shacl_on_write` + `shapes_graph_iri`).
    let legacy_shapes_graph = if include_legacy_dataset_gate {
        owning_dataset
            .as_ref()
            .filter(|ds| ds.shacl_on_write)
            .and_then(|ds| ds.shapes_graph_iri.clone())
            .filter(|iri| !iri.is_empty())
    } else {
        None
    };

    Ok(GateSet {
        pipelines,
        binding_graphs,
        legacy_shapes_graph,
    })
}

/// The graphs holding a pipeline's shapes, resolved from its shape-graph ids.
///
/// Every id must resolve. This used to be `if let Ok(Some(set)) = ..` — a
/// lookup error or a deleted shape-graph record was silently dropped, the
/// pipeline then validated against fewer (or no) shapes, and a gate that had
/// been *discovered* as covering the graph stopped gating without a trace.
/// A gate the server cannot evaluate must refuse the write, like a shape
/// graph that will not copy or an engine error.
fn pipeline_shape_graphs(
    studio: &ShaclStudioStore,
    p: &ValidationPipeline,
) -> Result<Vec<String>, ValidationReport> {
    p.shape_graph_ids
        .iter()
        .map(|set_id| {
            studio
                .get_shape_graph(set_id)
                .map_err(|e| {
                    gate_error(format!(
                        "pipeline '{}': looking up shape graph '{set_id}': {e}",
                        p.id
                    ))
                })?
                .map(|set| set.graph_iri)
                .ok_or_else(|| {
                    gate_error(format!(
                        "pipeline '{}': shape graph '{set_id}' not found",
                        p.id
                    ))
                })
        })
        .collect()
}

/// Union of shape-graph graphs needed by every gate source, resolved once.
/// `Err` when a pipeline names a shape graph that cannot be resolved.
fn needed_shape_graphs(
    studio: &ShaclStudioStore,
    gates: &GateSet,
) -> Result<BTreeSet<String>, ValidationReport> {
    let mut needed: BTreeSet<String> = gates.binding_graphs.clone();
    for p in &gates.pipelines {
        needed.extend(pipeline_shape_graphs(studio, p)?);
    }
    if let Some(g) = &gates.legacy_shapes_graph {
        needed.insert(g.clone());
    }
    Ok(needed)
}

/// Copy each shape graph from the live store into the throwaway store.
///
/// A shape graph that fails to copy would leave the gate validating against
/// *missing* shapes — which conforms trivially, i.e. silently no gate at all.
fn copy_shape_graphs(
    main_store: &TripleStore,
    temp: &TripleStore,
    graphs: &BTreeSet<String>,
) -> Result<(), ValidationReport> {
    for g in graphs {
        let bytes = main_store
            .dump(RdfFormat::Turtle, Some(g))
            .map_err(|e| gate_error(format!("reading shape graph <{g}>: {e}")))?;
        let ttl = String::from_utf8(bytes)
            .map_err(|e| gate_error(format!("shape graph <{g}> is not valid UTF-8: {e}")))?;
        temp.load_str(&ttl, RdfFormat::Turtle, Some(g))
            .map_err(|e| gate_error(format!("loading shape graph <{g}>: {e}")))?;
    }
    Ok(())
}

/// Run every gate source against the prepared temp store. Returns the first
/// failing gate's report, with the shape graphs that gate validated against.
fn evaluate_gates(
    temp: &TripleStore,
    studio: &ShaclStudioStore,
    gates: &GateSet,
    graph_iri: &str,
) -> Result<(), Refusal> {
    let data_graphs = [graph_iri.to_string()];
    // Every run below is a gate, for the workload telemetry.
    let _path = crate::store::telemetry::ValidationPathGuard::set("gate");

    // Pipeline gates (each at its own severity threshold). Inference is never
    // run here — gating must not mutate any store.
    for p in &gates.pipelines {
        // Resolved again rather than threaded through from
        // `needed_shape_graphs`, and with the same fail-closed rule: an
        // unresolvable pipeline is an error here too, never a skipped gate.
        let shape_graphs = pipeline_shape_graphs(studio, p).map_err(|r| refusal(r, Vec::new()))?;
        match super::run::run_validation(
            temp,
            &shape_graphs,
            &data_graphs,
            p.severity_threshold,
            false,
        ) {
            Ok(outcome) if !outcome.passes => return Err(refusal(outcome.report, shape_graphs)),
            Ok(_) => {}
            // `_ => {}` used to swallow this arm, so an engine error read as a
            // pass and the gate quietly stopped gating.
            Err(e) => {
                return Err(refusal(
                    gate_error(format!("pipeline '{}': {e}", p.id)),
                    Vec::new(),
                ))
            }
        }
    }

    // Binding gates — enforced at the default Violation threshold.
    if !gates.binding_graphs.is_empty() {
        let shape_graphs: Vec<String> = gates.binding_graphs.iter().cloned().collect();
        match super::run::run_validation(
            temp,
            &shape_graphs,
            &data_graphs,
            SeverityThreshold::Violation,
            false,
        ) {
            Ok(outcome) if !outcome.passes => return Err(refusal(outcome.report, shape_graphs)),
            Ok(_) => {}
            Err(e) => {
                return Err(refusal(
                    gate_error(format!("validation-layer binding: {e}")),
                    Vec::new(),
                ))
            }
        }
    }

    // Legacy per-dataset gate: mirrors `validate_on_write` — a direct engine
    // run against the dataset's configured shapes graph, failing on
    // `!conforms`.
    if let Some(shapes_graph) = &gates.legacy_shapes_graph {
        // `if let Ok(..)` dropped the error case, so a failing engine run meant
        // "no violations" and the write sailed through ungated.
        let report = crate::shacl::validate(temp, shapes_graph, &data_graphs).map_err(|e| {
            refusal(
                gate_error(format!("dataset shapes graph <{shapes_graph}>: {e}")),
                Vec::new(),
            )
        })?;
        if !report.conforms {
            return Err(refusal(report, vec![shapes_graph.clone()]));
        }
    }

    Ok(())
}

/// What a `gate_writes` pipeline gates: the graphs it names and the datasets
/// whose graphs it covers. Shape-graph targets gate nothing.
pub(crate) struct GatedScope<'a> {
    /// Graph targets and the legacy `graph_iris`.
    pub graphs: BTreeSet<&'a str>,
    /// Dataset targets, which always cover their graphs, and the legacy
    /// `dataset_ids`, which do only when no explicit `graph_iris` narrow the
    /// scope (preserving the historical "empty graph_iris = all dataset
    /// graphs").
    pub datasets: BTreeSet<&'a str>,
}

/// The scope `p` gates, read by the gate ([`pipeline_covers_graph`],
/// [`creator_may_gate`]) and by the authority its author needs
/// (`handlers::authorize_pipeline_gate`), so the two cannot drift apart.
pub(crate) fn gated_scope(p: &ValidationPipeline) -> GatedScope<'_> {
    let targets = move |kind: TargetKind| {
        p.targets
            .iter()
            .filter(move |t| t.kind == kind)
            .map(|t| t.id.as_str())
    };
    let graphs = p
        .graph_iris
        .iter()
        .map(String::as_str)
        .chain(targets(TargetKind::Graph))
        .collect();
    let mut datasets: BTreeSet<&str> = targets(TargetKind::Dataset).collect();
    if p.graph_iris.is_empty() {
        datasets.extend(p.dataset_ids.iter().map(String::as_str));
    }
    GatedScope { graphs, datasets }
}

fn pipeline_covers_graph(
    p: &ValidationPipeline,
    graph_iri: &str,
    dataset_id: Option<&str>,
) -> bool {
    let scope = gated_scope(p);
    scope.graphs.contains(graph_iri) || dataset_id.is_some_and(|ds| scope.datasets.contains(ds))
}

/// Whether `p`'s creator may still gate a write to `graph_iri`, held by
/// `dataset`: they may write what makes `p` cover it — the graph, for a graph
/// `p` names (a graph-ACL write grant); the dataset, for a dataset `p` names
/// (`can_write_dataset`). Admins may gate anything.
///
/// A gate refuses writes for everyone who writes the graph, so it acts with
/// the authority its author needed to set it (`handlers::authorize_pipeline_gate`),
/// checked here at every write as `exec::owner_can_write` checks a pipeline's
/// own writes at every run: a gate stored before that check, or one whose
/// creator has since lost the grant, been deactivated or deleted, gates
/// nothing. `Err` when a lookup fails; the caller keeps the gate then, since
/// a lookup error must not lift a gate.
fn creator_may_gate(
    auth_db: &AuthDb,
    p: &ValidationPipeline,
    graph_iri: &str,
    dataset: Option<&Dataset>,
) -> anyhow::Result<bool> {
    let Some(creator) = p.created_by.as_deref() else {
        return Ok(false);
    };
    let Some(user) = auth_db.get_user_by_id(creator)?.filter(|u| u.is_active) else {
        return Ok(false);
    };
    if user.is_admin() {
        return Ok(true);
    }
    let scope = gated_scope(p);
    if scope.graphs.contains(graph_iri)
        && auth_db.check_graph_permission(&user.id, user.role.as_str(), graph_iri, "write")?
    {
        return Ok(true);
    }
    match dataset.filter(|ds| scope.datasets.contains(ds.id.as_str())) {
        Some(ds) => auth_db.can_write_dataset(&user.id, ds),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bundle the shared handles the gate functions take, for an anonymous
    /// writer.
    fn ctx<'a>(
        store: &'a TripleStore,
        auth: &'a AuthDb,
        studio: &'a ShaclStudioStore,
        base: &'a str,
    ) -> GateContext<'a> {
        GateContext {
            main_store: store,
            auth_db: auth,
            studio,
            base_url: base,
            writer: None,
        }
    }
    use crate::auth::models::{OwnerType, SystemRole, Visibility};
    use crate::shacl_studio::models::ValidationTarget;

    fn pipe(
        targets: Vec<ValidationTarget>,
        dataset_ids: Vec<&str>,
        graph_iris: Vec<&str>,
    ) -> ValidationPipeline {
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
            // An admin: `studio_with_shapes` creates the user.
            created_by: Some(CREATOR.into()),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn graph_target(iri: &str) -> ValidationTarget {
        ValidationTarget {
            kind: TargetKind::Graph,
            id: iri.into(),
        }
    }
    fn dataset_target(id: &str) -> ValidationTarget {
        ValidationTarget {
            kind: TargetKind::Dataset,
            id: id.into(),
        }
    }

    /// A gate that cannot be evaluated must BLOCK the write, not wave it
    /// through. Every infrastructure failure in this module used to return
    /// `Ok(())`, so a shape graph that would not copy silently validated the
    /// incoming data against no shapes at all — which conforms trivially.
    #[test]
    fn a_shape_graph_that_cannot_be_copied_blocks_the_write() {
        let main = TripleStore::in_memory().unwrap();
        let temp = TripleStore::in_memory().unwrap();
        let mut needed = BTreeSet::new();
        needed.insert("not a valid iri".to_string());

        let err = copy_shape_graphs(&main, &temp, &needed)
            .expect_err("an uncopyable shape graph must not be treated as 'no shapes'");
        assert!(
            !err.conforms,
            "the gate must report a non-conforming outcome"
        );
        assert_eq!(err.results[0].source_constraint, "gate-evaluation-failure");
        assert!(
            err.results[0].message.contains("was not applied"),
            "the message must make clear the write did not land: {}",
            err.results[0].message
        );
    }

    /// A shape graph that copies cleanly leaves the gate free to run.
    #[test]
    fn copying_a_real_shape_graph_succeeds() {
        let main = TripleStore::in_memory().unwrap();
        main.load_str(
            "<http://ex/S> a <http://www.w3.org/ns/shacl#NodeShape> .",
            RdfFormat::Turtle,
            Some("urn:shapes:ok"),
        )
        .unwrap();
        let temp = TripleStore::in_memory().unwrap();
        let mut needed = BTreeSet::new();
        needed.insert("urn:shapes:ok".to_string());

        assert!(copy_shape_graphs(&main, &temp, &needed).is_ok());
        assert!(
            temp.len().unwrap() > 0,
            "shapes must land in the temp store"
        );
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

    // ─── Import gates (quad-based bulk path) ──────────────────────────────────

    use crate::shacl::report::{Severity, ValidationResult};
    use crate::shacl_studio::models::{ShapeGraph, ShapeSource};
    use oxigraph::model::{Literal, Term};

    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    const DATA_GRAPH: &str = "urn:data:g1";
    const SHAPES_GRAPH: &str = "urn:shapes:person";
    const SHAPES_TTL: &str = r#"
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix ex: <http://example.org/> .
        ex:PersonShape a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:property [ sh:path ex:name ; sh:minCount 1 ] .
    "#;

    fn person_quads(with_name: bool) -> Vec<Quad> {
        let p = NamedNode::new("http://example.org/p1").unwrap();
        let mut quads = vec![Quad::new(
            p.clone(),
            NamedNode::new(RDF_TYPE).unwrap(),
            NamedNode::new("http://example.org/Person").unwrap(),
            GraphName::DefaultGraph,
        )];
        if with_name {
            quads.push(Quad::new(
                p,
                NamedNode::new("http://example.org/name").unwrap(),
                Term::Literal(Literal::new_simple_literal("Ada")),
                GraphName::DefaultGraph,
            ));
        }
        quads
    }

    /// The creator of every `pipe()`, whose authority a gate acts with.
    const CREATOR: &str = "u";

    fn studio_with_shapes(store: &TripleStore, auth: &AuthDb) -> (ShaclStudioStore, ShapeGraph) {
        auth.create_user(CREATOR, CREATOR, "u@t.com", "hash", SystemRole::Admin)
            .unwrap();
        let studio = ShaclStudioStore::new(auth.pool());
        let set = studio
            .create_shape_graph(
                "person",
                None,
                OwnerType::User,
                "u1",
                Visibility::Private,
                SHAPES_GRAPH,
                &[],
                ShapeSource::Manual,
                None,
            )
            .unwrap();
        store
            .load_str(SHAPES_TTL, RdfFormat::Turtle, Some(SHAPES_GRAPH))
            .unwrap();
        (studio, set)
    }

    #[test]
    fn import_gates_via_binding_reject_violations_and_pass_conforming() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let (studio, set) = studio_with_shapes(&store, &auth);
        let base = "http://x";

        // No binding yet: nothing applies, nothing is checked.
        assert!(!import_gates_apply(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH
        ));
        check_import_gates(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH,
            &person_quads(false),
        )
        .expect("no gates → no rejection");

        bindings::add_binding(&store, DATA_GRAPH, &set.graph_iri).unwrap();
        assert!(import_gates_apply(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH
        ));

        // Missing ex:name violates sh:minCount 1 → rejected with a report.
        let report = check_import_gates(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH,
            &person_quads(false),
        )
        .unwrap_err();
        assert!(!report.conforms);
        let summary = summarize_report(&report, 5);
        assert!(
            summary.contains("http://example.org/p1"),
            "summary names the focus node: {summary}"
        );

        // Conforming data passes.
        check_import_gates(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH,
            &person_quads(true),
        )
        .expect("conforming data must pass the gate");
    }

    #[test]
    fn import_gates_honor_gating_pipeline() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let (studio, set) = studio_with_shapes(&store, &auth);
        let base = "http://x";

        let mut p = pipe(vec![graph_target(DATA_GRAPH)], vec![], vec![]);
        p.shape_graph_ids = vec![set.id.clone()];
        studio.insert_pipeline(&p).unwrap();

        assert!(import_gates_apply(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH
        ));
        assert!(
            !import_gates_apply(ctx(&store, &auth, &studio, base), "urn:data:uncovered"),
            "pipeline scope must not leak to other graphs"
        );

        let report = check_import_gates(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH,
            &person_quads(false),
        )
        .unwrap_err();
        assert!(!report.conforms);
        check_import_gates(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH,
            &person_quads(true),
        )
        .expect("conforming data must pass the pipeline gate");
    }

    /// A discovered gating pipeline whose shape-graph record is gone must
    /// refuse the write on both paths. `needed_shape_graphs` used to drop the
    /// unresolvable id and the callers returned `Ok(())` on the resulting empty
    /// set — the pipeline still *covered* the graph but no longer gated it.
    #[test]
    fn a_gating_pipeline_with_a_missing_shape_graph_blocks_both_paths() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let (studio, set) = studio_with_shapes(&store, &auth);
        let base = "http://x";

        let mut p = pipe(vec![graph_target(DATA_GRAPH)], vec![], vec![]);
        p.shape_graph_ids = vec![set.id.clone()];
        studio.insert_pipeline(&p).unwrap();
        studio.delete_shape_graph(&set.id).unwrap();

        // The pipeline still covers the graph…
        assert!(import_gates_apply(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH
        ));

        // …so conforming data is refused on the import path…
        let report = check_import_gates(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH,
            &person_quads(true),
        )
        .expect_err("an unresolvable gate must block the import");
        assert_eq!(
            report.results[0].source_constraint,
            "gate-evaluation-failure"
        );
        assert!(
            report.results[0].message.contains("not found")
                && report.results[0].message.contains(&set.id),
            "the refusal names the missing shape graph: {}",
            report.results[0].message
        );

        // …and on the Graph Store path.
        let report = check_write_gates(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH,
            "<http://example.org/p1> a <http://example.org/Person> ; \
             <http://example.org/name> \"Ada\" .",
            RdfFormat::Turtle,
            WriteMode::Replace,
        )
        .expect_err("an unresolvable gate must block the write");
        assert_eq!(
            report.results[0].source_constraint,
            "gate-evaluation-failure"
        );
        assert!(report.results[0].message.contains("was not applied"));
    }

    /// Make every query on `table` fail, as a database error would. The
    /// in-memory pool holds one connection, so every later lookup sees it.
    fn drop_table(auth: &AuthDb, table: &str) {
        auth.pool()
            .get()
            .unwrap()
            .execute_batch(&format!("DROP TABLE {table}"))
            .unwrap();
    }

    /// With gate discovery failing, the import pre-check says a gate applies
    /// and both write paths refuse conforming data with a report naming
    /// `reason`.
    fn assert_both_paths_refuse(
        store: &TripleStore,
        auth: &AuthDb,
        studio: &ShaclStudioStore,
        reason: &str,
    ) {
        let c = ctx(store, auth, studio, "http://x");
        assert!(
            import_gates_apply(c, DATA_GRAPH),
            "a failed discovery must send the import on to check_import_gates"
        );
        let import = check_import_gates(c, DATA_GRAPH, &person_quads(true))
            .expect_err("a failed discovery must block the import");
        let write = check_write_gates(
            c,
            DATA_GRAPH,
            "<http://example.org/p1> a <http://example.org/Person> ; \
             <http://example.org/name> \"Ada\" .",
            RdfFormat::Turtle,
            WriteMode::Replace,
        )
        .expect_err("a failed discovery must block the write");
        for report in [import, write] {
            assert!(!report.conforms);
            assert_eq!(
                report.results[0].source_constraint,
                "gate-evaluation-failure"
            );
            assert!(
                report.results[0].message.contains(reason),
                "the refusal names the failed lookup: {}",
                report.results[0].message
            );
        }
    }

    /// A database error listing the gating pipelines refuses the write.
    /// `.unwrap_or_default()` read it as "no gating pipelines", so every
    /// `gate_writes` pipeline stopped gating and the write landed unvalidated.
    #[test]
    fn a_failed_pipeline_lookup_blocks_both_paths() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let (studio, set) = studio_with_shapes(&store, &auth);
        let mut p = pipe(vec![graph_target(DATA_GRAPH)], vec![], vec![]);
        p.shape_graph_ids = vec![set.id.clone()];
        studio.insert_pipeline(&p).unwrap();

        drop_table(&auth, "validation_pipelines");
        assert!(studio.list_gating_pipelines().is_err());

        assert_both_paths_refuse(&store, &auth, &studio, "listing the gating pipelines");
    }

    /// A database error finding the graph's dataset refuses the write.
    /// `.ok().flatten()` read it as "no dataset", which dropped every gate
    /// that comes with one: dataset-scoped pipelines, dataset-level bindings
    /// and, on the import path, the legacy `shacl_on_write` gate.
    #[test]
    fn a_failed_dataset_lookup_blocks_both_paths() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let (studio, set) = studio_with_shapes(&store, &auth);
        auth.create_dataset(
            "d1",
            "D1",
            None,
            OwnerType::User,
            CREATOR,
            Visibility::Private,
            None,
        )
        .unwrap();
        auth.add_dataset_graph("d1", DATA_GRAPH).unwrap();
        auth.update_dataset_shacl("d1", true, Some(SHAPES_GRAPH))
            .unwrap();
        let mut p = pipe(vec![dataset_target("d1")], vec![], vec![]);
        p.shape_graph_ids = vec![set.id.clone()];
        studio.insert_pipeline(&p).unwrap();

        drop_table(&auth, "dataset_graphs");
        assert!(auth.find_dataset_by_graph_iri(DATA_GRAPH).is_err());

        assert_both_paths_refuse(&store, &auth, &studio, "looking up the dataset holding");
    }

    /// A gate acts with its creator's write authority, checked at every
    /// write: a gate covers a graph only for a creator who may write what
    /// makes it cover the graph — the dataset for a dataset target, the graph
    /// (a graph-ACL write grant) for a graph target — or an admin.
    #[test]
    fn a_gate_gates_only_with_its_creators_write_access() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let (studio, set) = studio_with_shapes(&store, &auth);
        let base = "http://x";
        for id in ["owner", "reader"] {
            auth.create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
                .unwrap();
        }
        auth.create_dataset(
            "d1",
            "D1",
            None,
            OwnerType::User,
            "owner",
            Visibility::Public,
            None,
        )
        .unwrap();
        auth.add_dataset_graph("d1", DATA_GRAPH).unwrap();

        let gates = |target: ValidationTarget, creator: Option<&str>| {
            let mut p = pipe(vec![target], vec![], vec![]);
            p.shape_graph_ids = vec![set.id.clone()];
            p.created_by = creator.map(String::from);
            studio.insert_pipeline(&p).unwrap();
            let applies = import_gates_apply(ctx(&store, &auth, &studio, base), DATA_GRAPH);
            studio.delete_pipeline(&p.id).unwrap();
            applies
        };

        // The dataset's owner may write it; a reader of the public dataset
        // may not.
        assert!(gates(dataset_target("d1"), Some("owner")));
        assert!(!gates(dataset_target("d1"), Some("reader")));
        // A graph target needs a graph-ACL write grant, which covers the
        // graph it names and not the dataset holding it.
        assert!(!gates(graph_target(DATA_GRAPH), Some("reader")));
        auth.grant_graph_permission("w1", DATA_GRAPH, "user", "reader", "write", CREATOR)
            .unwrap();
        assert!(gates(graph_target(DATA_GRAPH), Some("reader")));
        assert!(!gates(dataset_target("d1"), Some("reader")));
        // No creator, an unknown one, or a deactivated one: no authority.
        assert!(!gates(graph_target(DATA_GRAPH), None));
        assert!(!gates(graph_target(DATA_GRAPH), Some("ghost")));
        auth.set_user_active("reader", false).unwrap();
        assert!(!gates(graph_target(DATA_GRAPH), Some("reader")));
        // Admins gate anything.
        assert!(gates(graph_target(DATA_GRAPH), Some(CREATOR)));
    }

    #[test]
    fn import_gates_honor_legacy_shacl_on_write() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let (studio, _set) = studio_with_shapes(&store, &auth);
        let base = "http://x";

        auth.create_dataset(
            "d1",
            "D1",
            None,
            OwnerType::User,
            "u1",
            Visibility::Private,
            None,
        )
        .unwrap();
        auth.add_dataset_graph("d1", DATA_GRAPH).unwrap();

        // shacl_on_write off → no gate.
        assert!(!import_gates_apply(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH
        ));

        auth.update_dataset_shacl("d1", true, Some(SHAPES_GRAPH))
            .unwrap();
        assert!(import_gates_apply(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH
        ));

        let report = check_import_gates(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH,
            &person_quads(false),
        )
        .unwrap_err();
        assert!(!report.conforms);
        check_import_gates(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH,
            &person_quads(true),
        )
        .expect("conforming data must pass the legacy dataset gate");

        // Graph Store path (`check_write_gates`) intentionally excludes the
        // legacy gate — `validate_on_write` runs it separately there.
        check_write_gates(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH,
            "<http://example.org/p1> a <http://example.org/Person> .",
            RdfFormat::Turtle,
            WriteMode::Replace,
        )
        .expect("legacy gate must not double-fire on the GSP path");
    }

    #[test]
    fn summarize_report_truncates() {
        let mk = |n: usize| ValidationResult {
            severity: Severity::Violation,
            focus_node: format!("http://example.org/f{n}"),
            path: Some("http://example.org/name".to_string()),
            value: None,
            source_shape: "http://example.org/S".to_string(),
            source_constraint: "minCount".to_string(),
            message: format!("missing name {n}"),
        };
        let report = ValidationReport {
            conforms: false,
            results: (0..4).map(mk).collect(),
            results_count: 4,
            metrics: None,
        };
        let s = summarize_report(&report, 2);
        assert!(s.starts_with("4 validation result(s)"), "{s}");
        assert!(s.contains("f0") && s.contains("f1"), "{s}");
        assert!(!s.contains("f2"), "truncated: {s}");
        assert!(s.contains("and 2 more"), "{s}");
    }
}
