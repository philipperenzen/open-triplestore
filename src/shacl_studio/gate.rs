//! Write-gating: before a write to a named graph lands, every SHACL gate that
//! covers the graph is evaluated against the graph's *future* contents (in one
//! throwaway store) and the write is refused if any fails.
//!
//! Three sources gate a write, and every write path — Graph Store `PUT`/`POST`,
//! validate-and-commit, bulk import, RDF Patch, repair — runs all of them in
//! one pass, in this order, stopping at the first that fails:
//!
//! 1. `gate_writes` pipelines whose scope covers the graph, each at its own
//!    severity threshold (a creator who may no longer write what the pipeline
//!    covers gates nothing);
//! 2. validation-layer bindings — shapes bound to the graph itself or to its
//!    dataset — at the `Violation` threshold;
//! 3. the dataset's own `shacl_on_write` flag with its `shapes_graph_iri`,
//!    which refuses on any result (the report does not conform), whatever its
//!    severity.
//!
//! The refusal is the first failing gate's report. The order only decides
//! which report a refused write gets: a write lands only when every gate
//! passes. Each gate is evaluated once per write; the incoming data is staged
//! once and the shape graphs are copied once.
//!
//! A replace (`PUT`, an import) stages its payload, which is the whole future
//! graph. A merge (`POST`) or a delta (RDF Patch, a repair apply) stages the
//! write's additions plus only what the nodes it can affect read from the
//! live graph, and validates those nodes (`gate_focus`); when that
//! cannot be bounded it stages the whole live graph, as every merge used to.
//!
//! A datasource run stages its candidate graph in the store before the graph
//! joins its dataset, so [`discover_gates_joining`] finds the gates of the
//! dataset it is about to join, and [`evaluate_every_gate`] runs them all over
//! the candidate where it lies, in the same order, reporting every gate's
//! verdict rather than stopping at the first refusal.

use std::collections::{BTreeSet, HashMap, HashSet};

use oxigraph::io::RdfFormat;
use oxigraph::model::{GraphName, NamedNode, Quad, Term};

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
            source_constraint_component: String::new(),
            annotations: Vec::new(),
            terms: Default::default(),
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
/// merge whose reach cannot be bounded is validated against the state the
/// write will actually produce.
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
            source_constraint_component: String::new(),
            annotations: Vec::new(),
            terms: Default::default(),
            message,
        }],
        results_count: 1,
        metrics: None,
    }
}

/// Returns `Err(report)` with the first failing gate's report when the
/// graph's future contents would fail a gate that covers this write; `Ok(())`
/// otherwise. All three gate sources run, in the module's order: `gate_writes`
/// pipelines, validation-layer bindings (shapes attached to the written graph
/// or its dataset, which travel with the graph wherever it is mounted), and
/// the dataset's `shacl_on_write` shapes graph. `on_dataset_gate`, if
/// given, is called with the dataset gate's report whenever it runs.
pub fn check_write_gates(
    ctx: GateContext<'_>,
    graph_iri: &str,
    data: &str,
    format: RdfFormat,
    mode: WriteMode,
    on_dataset_gate: Option<DatasetGateHook<'_>>,
) -> Result<(), ValidationReport> {
    let body = Body { data, format, mode };
    write_gates(
        ctx,
        graph_iri,
        body,
        on_dataset_gate,
        true,
        &mut Staging::default(),
    )
}

/// A Graph Store body and how it is applied.
struct Body<'a> {
    data: &'a str,
    format: RdfFormat,
    mode: WriteMode,
}

/// What a gate staged in its sandbox: which nodes it validates, and how much
/// of the written graph it holds.
#[derive(Debug, Default)]
struct Staging {
    /// The nodes the gates validate; `None` validates every target, the whole
    /// future graph being staged.
    focus: Option<HashSet<Term>>,
    /// Why a merge or a delta staged the whole live graph.
    whole_because: Option<String>,
    /// Quads of the written graph the sandbox held.
    data_quads: usize,
}

impl Staging {
    /// Count what the sandbox holds of `graph_iri`, and log it.
    fn record(&mut self, temp: &TripleStore, graph_iri: &str) {
        self.data_quads = temp.graph_count_cached(Some(graph_iri)).unwrap_or(0);
        match (&self.focus, &self.whole_because) {
            (Some(focus), _) => tracing::debug!(
                "shacl gate: <{graph_iri}>: {} focus node(s) over {} staged quad(s)",
                focus.len(),
                self.data_quads
            ),
            (None, Some(why)) => tracing::debug!(
                "shacl gate: <{graph_iri}>: staged the whole graph ({} quads): {why}",
                self.data_quads
            ),
            (None, None) => {}
        }
    }
}

/// A write as quads: what it adds and what it removes (disjoint, all in the
/// written graph).
#[derive(Clone, Copy)]
struct Delta<'a> {
    adds: &'a [Quad],
    removes: &'a [Quad],
}

/// [`check_write_gates`]; `focus_allowed: false` stages the whole live graph
/// for a merge, as every merge used to (the tests' reference).
fn write_gates(
    ctx: GateContext<'_>,
    graph_iri: &str,
    body: Body<'_>,
    on_dataset_gate: Option<DatasetGateHook<'_>>,
    focus_allowed: bool,
    staging: &mut Staging,
) -> Result<(), ValidationReport> {
    let GateContext {
        main_store,
        auth_db,
        studio,
        base_url,
        writer,
    } = ctx;
    let gates = discover_gates(main_store, auth_db, studio, base_url, graph_iri)?;
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
    if temp
        .load_str(body.data, body.format, Some(graph_iri))
        .is_err()
    {
        // Malformed data — let the normal write path surface the parse error.
        // Safe to pass: the write itself will fail on the same parse.
        return Ok(());
    }
    // The payload as parsed: a merge's additions.
    let adds = match body.mode {
        WriteMode::Replace => Vec::new(),
        WriteMode::Merge => temp
            .quads_for_graph(graph_ref(graph_iri)?.as_ref().into())
            .map_err(|e| gate_error(format!("reading the staged payload: {e}")))?,
    };
    copy_shape_graphs(main_store, &temp, &needed_graphs)?;
    if body.mode == WriteMode::Merge {
        let write = Delta {
            adds: &adds,
            removes: &[],
        };
        stage_future(
            main_store,
            &temp,
            graph_iri,
            &needed_graphs,
            write,
            focus_allowed,
            staging,
        )?;
    }
    staging.record(&temp, graph_iri);

    evaluate_gates(
        &temp,
        studio,
        &gates,
        graph_iri,
        on_dataset_gate,
        staging.focus.as_ref(),
    )
    .map_err(|refusal| {
        let (report, shapes) = *refusal;
        report_for_writer(auth_db, writer, report, &shapes)
    })
}

/// The write gates of `graph_iri` over what it would hold after `rows` —
/// each an addition (`true`) or a removal, applied in order, so the last row
/// on a quad wins — as an RDF Patch or a repair apply writes it: the gates a
/// Graph Store write to the same graph passes, in the same order. Rows in
/// other graphs are ignored. `Err` carries the first refusal's report, as the
/// writer may see it; a failed gate lookup refuses.
pub fn check_delta_gates<'q>(
    ctx: GateContext<'_>,
    graph_iri: &str,
    rows: impl IntoIterator<Item = (bool, &'q Quad)>,
) -> Result<(), ValidationReport> {
    delta_gates(ctx, graph_iri, rows, true, &mut Staging::default())
}

/// [`check_delta_gates`]; `focus_allowed: false` stages the whole future
/// graph, as every delta used to (the tests' reference).
fn delta_gates<'q>(
    ctx: GateContext<'_>,
    graph_iri: &str,
    rows: impl IntoIterator<Item = (bool, &'q Quad)>,
    focus_allowed: bool,
    staging: &mut Staging,
) -> Result<(), ValidationReport> {
    let GateContext {
        main_store,
        auth_db,
        studio,
        base_url,
        writer,
    } = ctx;
    let gates = discover_gates(main_store, auth_db, studio, base_url, graph_iri)?;
    if gates.is_empty() {
        return Ok(());
    }
    let needed_graphs = needed_shape_graphs(studio, &gates)?;
    let graph = GraphName::NamedNode(graph_ref(graph_iri)?);

    // The net effect on each quad: the last row wins.
    let mut net: HashMap<&Quad, bool> = HashMap::new();
    for (add, q) in rows {
        if q.graph_name == graph {
            net.insert(q, add);
        }
    }
    let (adds, removes): (Vec<Quad>, Vec<Quad>) = {
        let (a, r): (Vec<_>, Vec<_>) = net.into_iter().partition(|(_, add)| *add);
        (
            a.into_iter().map(|(q, _)| q.clone()).collect(),
            r.into_iter().map(|(q, _)| q.clone()).collect(),
        )
    };

    let temp = TripleStore::in_memory().map_err(|e| gate_error(format!("temp store: {e}")))?;
    copy_shape_graphs(main_store, &temp, &needed_graphs)?;
    if !adds.is_empty() {
        temp.bulk_insert_quads(adds.clone(), &[graph_iri.to_string()])
            .map_err(|e| gate_error(format!("staging the incoming quads: {e}")))?;
    }
    let write = Delta {
        adds: &adds,
        removes: &removes,
    };
    stage_future(
        main_store,
        &temp,
        graph_iri,
        &needed_graphs,
        write,
        focus_allowed,
        staging,
    )?;
    staging.record(&temp, graph_iri);

    evaluate_gates(
        &temp,
        studio,
        &gates,
        graph_iri,
        None,
        staging.focus.as_ref(),
    )
    .map_err(|refusal| {
        let (report, shapes) = *refusal;
        report_for_writer(auth_db, writer, report, &shapes)
    })
}

/// `graph_iri` as a node; `Err` refuses the write.
fn graph_ref(graph_iri: &str) -> Result<NamedNode, ValidationReport> {
    NamedNode::new(graph_iri).map_err(|e| {
        gate_error(format!(
            "target graph <{graph_iri}> is not a valid IRI: {e}"
        ))
    })
}

/// Stage the rest of a merge's or a delta's future graph in `temp`, which
/// holds the shapes and the write's additions, and record in `staging` which
/// nodes to validate: the nodes the write can affect, with the triples they
/// read ([`super::gate_focus`]); or, when that cannot be bounded, every
/// target, with the whole live graph less the write's removals, as before.
fn stage_future(
    main_store: &TripleStore,
    temp: &TripleStore,
    graph_iri: &str,
    needed_graphs: &BTreeSet<String>,
    write: Delta<'_>,
    focus_allowed: bool,
    staging: &mut Staging,
) -> Result<(), ValidationReport> {
    use super::gate_focus::{self, Plan};
    let plan = if !focus_allowed {
        Plan::Whole("the whole graph was asked for".to_string())
    } else if needed_graphs.contains(graph_iri) {
        // The write changes a gate's own shapes, which reach every target.
        Plan::Whole("the written graph holds a gate's shapes".to_string())
    } else {
        match gate_shapes(temp, needed_graphs) {
            Err(why) => Plan::Whole(why),
            Ok(shapes) => gate_focus::plan(
                main_store,
                &graph_ref(graph_iri)?,
                &shapes,
                write.adds,
                write.removes,
            )
            .map_err(gate_error)?,
        }
    };
    match plan {
        Plan::Focused {
            focus,
            neighbourhood,
        } => {
            if !neighbourhood.is_empty() {
                temp.bulk_insert_quads(neighbourhood, &[graph_iri.to_string()])
                    .map_err(|e| gate_error(format!("staging the graph's neighbourhood: {e}")))?;
            }
            staging.focus = Some(focus);
        }
        Plan::Whole(why) => {
            staging.whole_because = Some(why);
            if write.removes.is_empty() {
                seed_existing_graph(main_store, temp, graph_iri)?;
            } else {
                let removed: HashSet<&Quad> = write.removes.iter().collect();
                let rest: Vec<Quad> = main_store
                    .quads_for_graph(graph_ref(graph_iri)?.as_ref().into())
                    .map_err(|e| gate_error(format!("reading existing graph <{graph_iri}>: {e}")))?
                    .into_iter()
                    .filter(|q| !removed.contains(q))
                    .collect();
                if !rest.is_empty() {
                    temp.bulk_insert_quads(rest, &[graph_iri.to_string()])
                        .map_err(|e| {
                            gate_error(format!("staging existing graph <{graph_iri}>: {e}"))
                        })?;
                }
            }
        }
    }
    Ok(())
}

/// Every shape the gates validate against, as the engine loads them; `Err`
/// says why the write's reach cannot be planned. A shapes graph that declares
/// an entailment regime validates inferred triples no plan bounds; one that
/// fails to load fails the gate when it is evaluated.
fn gate_shapes(
    temp: &TripleStore,
    needed_graphs: &BTreeSet<String>,
) -> Result<Vec<crate::shacl::shapes::Shape>, String> {
    let mut shapes = Vec::new();
    for g in needed_graphs {
        match crate::shacl::entailment::declared_regimes(temp, g) {
            Ok(r) if r.is_empty() => {}
            Ok(_) => return Err(format!("shapes graph <{g}> declares sh:entailment")),
            Err(e) => return Err(e),
        }
        shapes.extend(crate::shacl::engine::load_shapes(temp, g)?);
    }
    Ok(shapes)
}

/// Cheap pre-check for bulk import: does *any* gate apply to writes into
/// `graph_iri`? Considers `gate_writes` pipelines, validation-layer bindings
/// (graph- and dataset-level) and the owning dataset's `shacl_on_write`
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
    ) {
        Ok(gates) => !gates.is_empty(),
        Err(_) => true,
    }
}

/// Quad-based write gate for bulk import: validates `quads` (re-homed into
/// `graph_iri` in a throwaway store) against every gate that applies —
/// `gate_writes` pipelines, validation-layer bindings, and the owning dataset's
/// `shacl_on_write` shapes graph, in that order, as [`check_write_gates`].
/// `Err` carries the first failing gate's report.
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
    let gates = discover_gates(main_store, auth_db, studio, base_url, graph_iri)?;
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

    evaluate_gates(&temp, studio, &gates, graph_iri, None, None).map_err(|refusal| {
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

/// Everything that gates a write to one graph, in evaluation order:
/// covering `gate_writes` pipelines, validation-layer binding shape graphs,
/// and the owning dataset's `shacl_on_write` shapes graph.
pub struct GateSet {
    pipelines: Vec<ValidationPipeline>,
    binding_graphs: BTreeSet<String>,
    dataset_gate: Option<DatasetGate>,
}

/// A dataset's own `shacl_on_write` gate.
struct DatasetGate {
    dataset_id: String,
    shapes_graph: String,
}

/// Called with what a dataset's `shacl_on_write` gate found — its dataset,
/// shapes graph and report, conforming or not — so a caller can keep the
/// on-write report history.
pub type DatasetGateHook<'a> = &'a dyn Fn(&str, &str, &ValidationReport);

impl GateSet {
    /// No gate applies.
    pub fn is_empty(&self) -> bool {
        self.pipelines.is_empty() && self.binding_graphs.is_empty() && self.dataset_gate.is_none()
    }

    /// The dataset's own `shacl_on_write` shapes graph, when that gate
    /// applies.
    pub fn dataset_shapes_graph(&self) -> Option<&str> {
        self.dataset_gate.as_ref().map(|g| g.shapes_graph.as_str())
    }
}

/// Which gate a [`GateVerdict`] comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateSource {
    /// A `gate_writes` pipeline, by id.
    Pipeline(String),
    /// The validation-layer bindings on the graph and its dataset, together.
    Bindings,
    /// The dataset's own `shacl_on_write` shapes, by dataset id.
    Dataset(String),
}

/// What one gate found, and whether that refuses the write by the gate's own
/// rule: a pipeline at its severity threshold, the bindings at `Violation`,
/// the dataset's `shacl_on_write` shapes on any result. A gate that passes
/// may still report results below its threshold.
pub struct GateVerdict {
    pub gate: GateSource,
    pub report: ValidationReport,
    pub refuses: bool,
    /// The shape graphs the gate validated against.
    pub shape_graphs: Vec<String>,
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
) -> Result<GateSet, ValidationReport> {
    // Which dataset (if any) owns the graph being written — needed both for
    // pipelines scoped by dataset and for dataset-level bindings.
    let owning_dataset = auth_db
        .find_dataset_by_graph_iri(graph_iri)
        .map_err(|e| gate_error(format!("looking up the dataset holding <{graph_iri}>: {e}")))?;
    discover_gates_in(
        main_store,
        auth_db,
        studio,
        base_url,
        graph_iri,
        owning_dataset.as_ref(),
    )
}

/// Every gate that will apply to `graph_iri` once it joins `dataset`, found
/// by the rules [`discover_gates`] applies to a graph the dataset holds: the
/// pipelines covering it whose creator may still write what they cover, the
/// bindings on the graph and on the dataset, and the dataset's
/// `shacl_on_write` shapes.
///
/// For a graph staged outside the dataset before it joins — a datasource
/// run's candidate, attached only when it is promoted — which
/// `find_dataset_by_graph_iri` cannot place yet. `Err` when a lookup fails,
/// as for [`discover_gates`].
pub fn discover_gates_joining(
    main_store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    graph_iri: &str,
    dataset: &Dataset,
) -> Result<GateSet, ValidationReport> {
    discover_gates_in(
        main_store,
        auth_db,
        studio,
        base_url,
        graph_iri,
        Some(dataset),
    )
}

/// The gates of a write to `graph_iri`, held by `owning_dataset`.
fn discover_gates_in(
    main_store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    graph_iri: &str,
    owning_dataset: Option<&Dataset>,
) -> Result<GateSet, ValidationReport> {
    // (a) Gating pipelines whose scope covers this graph, by a route their
    // creator may still write.
    let pipelines: Vec<ValidationPipeline> = studio
        .list_gating_pipelines()
        .map_err(|e| gate_error(format!("listing the gating pipelines: {e}")))?
        .into_iter()
        .filter(|p| pipeline_covers_graph(p, graph_iri, owning_dataset.map(|d| d.id.as_str())))
        .filter(
            |p| match creator_may_gate(auth_db, p, graph_iri, owning_dataset) {
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
    if let Some(ds) = owning_dataset {
        let ds_iri = bindings::dataset_target_iri(base_url, &ds.id);
        binding_graphs.extend(bindings_of(&ds_iri)?);
    }

    // (c) The dataset's own gate (`shacl_on_write` + `shapes_graph_iri`).
    let dataset_gate = owning_dataset
        .filter(|ds| ds.shacl_on_write)
        .and_then(|ds| {
            ds.shapes_graph_iri
                .clone()
                .filter(|iri| !iri.is_empty())
                .map(|shapes_graph| DatasetGate {
                    dataset_id: ds.id.clone(),
                    shapes_graph,
                })
        });

    Ok(GateSet {
        pipelines,
        binding_graphs,
        dataset_gate,
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
    if let Some(g) = &gates.dataset_gate {
        needed.insert(g.shapes_graph.clone());
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
    // The functions every query of the live store sees (its admin-designated
    // function graphs) are the scratch store's too, so a gate computes what
    // the same run computes outside it.
    temp.inherit_registered_functions(main_store);
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

/// Run every gate source against the prepared temp store, in the module's
/// order — over `focus` only when given (a focused sandbox), else over every
/// target. Returns the first failing gate's report, with the shape graphs
/// that gate validated against.
fn evaluate_gates(
    temp: &TripleStore,
    studio: &ShaclStudioStore,
    gates: &GateSet,
    graph_iri: &str,
    on_dataset_gate: Option<DatasetGateHook<'_>>,
    focus: Option<&HashSet<Term>>,
) -> Result<(), Refusal> {
    let mut refused = None;
    each_gate(
        temp,
        studio,
        gates,
        graph_iri,
        on_dataset_gate,
        focus,
        &mut |verdict| {
            if verdict.refuses {
                refused = Some(refusal(verdict.report, verdict.shape_graphs));
            }
            refused.is_none()
        },
    )
    .map_err(|report| refusal(report, Vec::new()))?;
    refused.map_or(Ok(()), Err)
}

/// Evaluate every gate in `gates` against `graph_iri` as `store` holds it
/// now, in the module's order, and return each gate's verdict, passing ones
/// included.
///
/// A write stops at the first gate that refuses it. This is for a caller that
/// has already staged the graph's whole future contents in `store` — a
/// datasource run's candidate graph — and turns every refusal into work at
/// once, so a gate further down the order is not discovered only after the
/// first is fixed. Validation reads `store` and never writes it. `Err` is a
/// gate that could not be evaluated, which must refuse like a failing one.
pub fn evaluate_every_gate(
    store: &TripleStore,
    studio: &ShaclStudioStore,
    gates: &GateSet,
    graph_iri: &str,
) -> Result<Vec<GateVerdict>, ValidationReport> {
    let mut verdicts = Vec::new();
    each_gate(
        store,
        studio,
        gates,
        graph_iri,
        None,
        None,
        &mut |verdict| {
            verdicts.push(verdict);
            true
        },
    )?;
    Ok(verdicts)
}

/// Evaluate `gates` against `graph_iri` as `store` holds it, in the module's
/// order — over `focus` only when given (a focused sandbox), else over every
/// target — handing each gate's verdict to `visit` until it returns `false`.
/// `Err` is a gate that could not be evaluated.
fn each_gate(
    store: &TripleStore,
    studio: &ShaclStudioStore,
    gates: &GateSet,
    graph_iri: &str,
    on_dataset_gate: Option<DatasetGateHook<'_>>,
    focus: Option<&HashSet<Term>>,
    visit: &mut dyn FnMut(GateVerdict) -> bool,
) -> Result<(), ValidationReport> {
    let data_graphs = [graph_iri.to_string()];
    // Every run below is a gate, for the workload telemetry.
    let _path = crate::store::telemetry::ValidationPathGuard::set("gate");

    // Pipeline gates (each at its own severity threshold). Inference is never
    // run here — gating must not mutate any store.
    for p in &gates.pipelines {
        // Resolved again rather than threaded through from
        // `needed_shape_graphs`, and with the same fail-closed rule: an
        // unresolvable pipeline is an error here too, never a skipped gate.
        let shape_graphs = pipeline_shape_graphs(studio, p)?;
        // A `_ => {}` arm used to swallow an engine error, so it read as a
        // pass and the gate quietly stopped gating.
        let outcome = super::run::run_gate_validation(
            store,
            &shape_graphs,
            &data_graphs,
            p.severity_threshold,
            focus,
        )
        .map_err(|e| gate_error(format!("pipeline '{}': {e}", p.id)))?;
        let verdict = GateVerdict {
            gate: GateSource::Pipeline(p.id.clone()),
            refuses: !outcome.passes,
            report: outcome.report,
            shape_graphs,
        };
        if !visit(verdict) {
            return Ok(());
        }
    }

    // Binding gates — enforced at the default Violation threshold.
    if !gates.binding_graphs.is_empty() {
        let shape_graphs: Vec<String> = gates.binding_graphs.iter().cloned().collect();
        let outcome = super::run::run_gate_validation(
            store,
            &shape_graphs,
            &data_graphs,
            SeverityThreshold::Violation,
            focus,
        )
        .map_err(|e| gate_error(format!("validation-layer binding: {e}")))?;
        let verdict = GateVerdict {
            gate: GateSource::Bindings,
            refuses: !outcome.passes,
            report: outcome.report,
            shape_graphs,
        };
        if !visit(verdict) {
            return Ok(());
        }
    }

    // The dataset's own gate: a direct engine run against its configured
    // shapes graph, failing on `!conforms` (any result, whatever its
    // severity).
    if let Some(DatasetGate {
        dataset_id,
        shapes_graph,
    }) = &gates.dataset_gate
    {
        // `if let Ok(..)` dropped the error case, so a failing engine run meant
        // "no violations" and the write sailed through ungated.
        let report = match focus {
            Some(focus) => crate::shacl::validate_focused(store, shapes_graph, &data_graphs, focus),
            None => crate::shacl::validate(store, shapes_graph, &data_graphs),
        }
        .map_err(|e| gate_error(format!("dataset shapes graph <{shapes_graph}>: {e}")))?;
        if let Some(hook) = on_dataset_gate {
            hook(dataset_id, shapes_graph, &report);
        }
        visit(GateVerdict {
            gate: GateSource::Dataset(dataset_id.clone()),
            refuses: !report.conforms,
            report,
            shape_graphs: vec![shapes_graph.clone()],
        });
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
    // A statement-provenance graph holds annotations about the data, written
    // by the server behind each write: never gated (delta note §6.3).
    if crate::provenance::is_any_statement_provenance_graph(graph_iri) {
        return false;
    }
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
            None,
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
            None,
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

        // The Graph Store path runs the same gate in the same pass, and
        // reports it to the caller's hook once.
        let runs = std::cell::RefCell::new(Vec::new());
        let hook = |ds: &str, shapes: &str, r: &ValidationReport| {
            runs.borrow_mut()
                .push((ds.to_string(), shapes.to_string(), r.conforms));
        };
        let report = check_write_gates(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH,
            "<http://example.org/p1> a <http://example.org/Person> .",
            RdfFormat::Turtle,
            WriteMode::Replace,
            Some(&hook),
        )
        .unwrap_err();
        assert!(!report.conforms);
        assert_eq!(
            *runs.borrow(),
            vec![("d1".to_string(), SHAPES_GRAPH.to_string(), false)]
        );
        check_write_gates(
            ctx(&store, &auth, &studio, base),
            DATA_GRAPH,
            "<http://example.org/p1> a <http://example.org/Person> ; \
             <http://example.org/name> \"Ann\" .",
            RdfFormat::Turtle,
            WriteMode::Replace,
            None,
        )
        .expect("conforming data passes the dataset gate on the Graph Store path");
    }

    // ─── A graph about to join a dataset ──────────────────────────────────────

    /// A datasource run's candidate, staged before it joins its dataset.
    const STAGED: &str = "urn:run:r1";

    /// A graph staged outside the dataset it is about to join is under that
    /// dataset's gates before it joins, found by the same rules as for a
    /// graph the dataset holds; `discover_gates` cannot place it yet.
    #[test]
    fn a_graph_about_to_join_a_dataset_is_under_its_gates_before_it_joins() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let (studio, set) = studio_with_shapes(&store, &auth);
        let base = "http://x";
        auth.create_user("reader", "reader", "reader@t.com", "hash", SystemRole::User)
            .unwrap();
        for id in ["d1", "d2"] {
            auth.create_dataset(
                id,
                id,
                None,
                OwnerType::User,
                CREATOR,
                Visibility::Private,
                None,
            )
            .unwrap();
        }
        let joining = |dataset: &str| {
            let ds = auth.get_dataset(dataset).unwrap().unwrap();
            discover_gates_joining(&store, &auth, &studio, base, STAGED, &ds).unwrap()
        };
        assert!(joining("d1").is_empty());

        // A pipeline on the dataset, and one whose creator may not write it.
        let mut p = pipe(vec![dataset_target("d1")], vec![], vec![]);
        p.shape_graph_ids = vec![set.id.clone()];
        studio.insert_pipeline(&p).unwrap();
        let mut q = pipe(vec![dataset_target("d1")], vec![], vec![]);
        q.id = "q".into();
        q.shape_graph_ids = vec![set.id.clone()];
        q.created_by = Some("reader".into());
        studio.insert_pipeline(&q).unwrap();
        bindings::add_binding(
            &store,
            &bindings::dataset_target_iri(base, "d1"),
            SHAPES_GRAPH,
        )
        .unwrap();
        auth.update_dataset_shacl("d1", true, Some(SHAPES_GRAPH))
            .unwrap();

        let gates = joining("d1");
        let pipelines: Vec<&str> = gates.pipelines.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            pipelines,
            vec!["p"],
            "only a creator who may write it gates"
        );
        assert!(gates.binding_graphs.contains(SHAPES_GRAPH) && gates.binding_graphs.len() == 1);
        assert_eq!(gates.dataset_shapes_graph(), Some(SHAPES_GRAPH));
        // While no dataset holds it, a write to the graph meets none of them…
        assert!(!import_gates_apply(
            ctx(&store, &auth, &studio, base),
            STAGED
        ));
        // …and another dataset's gates are not its.
        assert!(joining("d2").is_empty());
    }

    /// Every gate's verdict, in the module's order and passing ones included:
    /// a pipeline refuses at its own severity threshold, the bindings at
    /// `Violation`, the dataset's `shacl_on_write` shapes on any result.
    #[test]
    fn every_gate_is_evaluated_in_order_each_by_its_own_rule() {
        const ADVICE_GRAPH: &str = "urn:shapes:advice";
        const ADVICE_TTL: &str = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            ex:AdviceShape a sh:NodeShape ;
                sh:targetClass ex:Person ;
                sh:property [ sh:path ex:email ; sh:minCount 1 ; sh:severity sh:Warning ] .
        "#;
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let (studio, person) = studio_with_shapes(&store, &auth);
        let base = "http://x";
        store
            .load_str(ADVICE_TTL, RdfFormat::Turtle, Some(ADVICE_GRAPH))
            .unwrap();
        let advice = studio
            .create_shape_graph(
                "advice",
                None,
                OwnerType::User,
                "u1",
                Visibility::Private,
                ADVICE_GRAPH,
                &[],
                ShapeSource::Manual,
                None,
            )
            .unwrap();
        // A person with neither a name (a violation) nor an email (a warning).
        store
            .load_str(
                "<http://example.org/p1> a <http://example.org/Person> .",
                RdfFormat::Turtle,
                Some(STAGED),
            )
            .unwrap();
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
        for (id, set, threshold) in [
            ("strict", &person, SeverityThreshold::Violation),
            ("advisory", &advice, SeverityThreshold::Violation),
            ("pedantic", &advice, SeverityThreshold::Warning),
        ] {
            let mut p = pipe(vec![dataset_target("d1")], vec![], vec![]);
            p.id = id.into();
            p.shape_graph_ids = vec![set.id.clone()];
            p.severity_threshold = threshold;
            studio.insert_pipeline(&p).unwrap();
        }
        bindings::add_binding(
            &store,
            &bindings::dataset_target_iri(base, "d1"),
            ADVICE_GRAPH,
        )
        .unwrap();
        auth.update_dataset_shacl("d1", true, Some(ADVICE_GRAPH))
            .unwrap();

        let ds = auth.get_dataset("d1").unwrap().unwrap();
        let gates = discover_gates_joining(&store, &auth, &studio, base, STAGED, &ds).unwrap();
        let verdicts = evaluate_every_gate(&store, &studio, &gates, STAGED).unwrap();
        let seen: Vec<(GateSource, bool)> = verdicts
            .iter()
            .map(|v| (v.gate.clone(), v.refuses))
            .collect();
        assert_eq!(seen.len(), 5, "{seen:?}");
        // The pipelines first, in no particular order among themselves…
        let mut pipelines = seen[..3].to_vec();
        pipelines.sort_by_key(|(gate, _)| format!("{gate:?}"));
        assert_eq!(
            pipelines,
            vec![
                (GateSource::Pipeline("advisory".into()), false),
                (GateSource::Pipeline("pedantic".into()), true),
                (GateSource::Pipeline("strict".into()), true),
            ]
        );
        // …then the bindings, for which a warning is below the threshold…
        assert_eq!(seen[3], (GateSource::Bindings, false));
        // …then the dataset's own gate, which refuses on any result.
        assert_eq!(seen[4], (GateSource::Dataset("d1".into()), true));
        // A gate that passes still says what it found.
        let advisory = verdicts
            .iter()
            .find(|v| v.gate == GateSource::Pipeline("advisory".into()))
            .unwrap();
        assert_eq!(advisory.report.results_count, 1);
        assert_eq!(advisory.report.results[0].severity, Severity::Warning);
        assert_eq!(advisory.shape_graphs, vec![ADVICE_GRAPH.to_string()]);
        // Evaluating wrote nothing.
        assert_eq!(store.count_graph(Some(STAGED)).unwrap(), 1);
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
            source_constraint_component: String::new(),
            annotations: Vec::new(),
            terms: Default::default(),
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

    // ─── Focused gates (#534) ─────────────────────────────────────────────────

    const PREFIXES: &str = "@prefix ex: <http://example.org/> .\n\
        @prefix sh: <http://www.w3.org/ns/shacl#> .\n\
        @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n";
    const DIFF_SHAPES: &str = "urn:shapes:diff";

    /// A write the differential test gates.
    enum Write {
        /// A Graph Store `POST` of this Turtle.
        Post(&'static str),
        /// An RDF Patch: these triples removed, then these added.
        Patch {
            remove: &'static str,
            add: &'static str,
        },
    }

    /// The triples of a Turtle snippet, in the data graph.
    fn triples(ttl: &str) -> Vec<Quad> {
        let s = TripleStore::in_memory().unwrap();
        s.load_str(
            &format!("{PREFIXES}{ttl}"),
            RdfFormat::Turtle,
            Some(DATA_GRAPH),
        )
        .unwrap();
        s.quads_for_graph(NamedNode::new(DATA_GRAPH).unwrap().as_ref().into())
            .unwrap()
    }

    /// A store whose data graph holds `base`, gated by `shapes` in all three
    /// ways: a gating pipeline, a validation-layer binding and the owning
    /// dataset's `shacl_on_write` shapes graph.
    fn gated(shapes: &str, base: &str) -> (TripleStore, AuthDb, ShaclStudioStore) {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        auth.create_user(CREATOR, CREATOR, "u@t.com", "hash", SystemRole::Admin)
            .unwrap();
        let studio = ShaclStudioStore::new(auth.pool());
        store
            .load_str(
                &format!("{PREFIXES}{shapes}"),
                RdfFormat::Turtle,
                Some(DIFF_SHAPES),
            )
            .unwrap();
        store
            .load_str(
                &format!("{PREFIXES}{base}"),
                RdfFormat::Turtle,
                Some(DATA_GRAPH),
            )
            .unwrap();
        let set = studio
            .create_shape_graph(
                "diff",
                None,
                OwnerType::User,
                CREATOR,
                Visibility::Private,
                DIFF_SHAPES,
                &[],
                ShapeSource::Manual,
                None,
            )
            .unwrap();
        let mut p = pipe(vec![graph_target(DATA_GRAPH)], vec![], vec![]);
        p.shape_graph_ids = vec![set.id];
        studio.insert_pipeline(&p).unwrap();
        bindings::add_binding(&store, DATA_GRAPH, DIFF_SHAPES).unwrap();
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
        auth.update_dataset_shacl("d1", true, Some(DIFF_SHAPES))
            .unwrap();
        (store, auth, studio)
    }

    /// The gates' verdict on `write` and what they staged; `focus_allowed:
    /// false` stages the whole graph, the reference.
    fn gate_on(
        store: &TripleStore,
        auth: &AuthDb,
        studio: &ShaclStudioStore,
        write: &Write,
        focus_allowed: bool,
    ) -> (Result<(), ValidationReport>, Staging) {
        let c = ctx(store, auth, studio, "http://x");
        let mut staging = Staging::default();
        let verdict = match write {
            Write::Post(ttl) => {
                let data = format!("{PREFIXES}{ttl}");
                let body = Body {
                    data: &data,
                    format: RdfFormat::Turtle,
                    mode: WriteMode::Merge,
                };
                write_gates(c, DATA_GRAPH, body, None, focus_allowed, &mut staging)
            }
            Write::Patch { remove, add } => {
                let (removed, added) = (triples(remove), triples(add));
                let rows = removed
                    .iter()
                    .map(|q| (false, q))
                    .chain(added.iter().map(|q| (true, q)));
                delta_gates(c, DATA_GRAPH, rows, focus_allowed, &mut staging)
            }
        };
        (verdict, staging)
    }

    /// What a full validation of the data graph says after `write`, computed
    /// without the gate: the base and the write applied in a store of their
    /// own.
    fn after(shapes: &str, base: &str, write: &Write) -> ValidationReport {
        let mut future: HashSet<Quad> = triples(base).into_iter().collect();
        match write {
            Write::Post(ttl) => future.extend(triples(ttl)),
            Write::Patch { remove, add } => {
                for q in triples(remove) {
                    future.remove(&q);
                }
                future.extend(triples(add));
            }
        }
        let s = TripleStore::in_memory().unwrap();
        s.load_str(
            &format!("{PREFIXES}{shapes}"),
            RdfFormat::Turtle,
            Some(DIFF_SHAPES),
        )
        .unwrap();
        s.bulk_insert_quads(future.into_iter().collect(), &[DATA_GRAPH.to_string()])
            .unwrap();
        crate::shacl::validate(&s, DIFF_SHAPES, &[DATA_GRAPH.to_string()]).unwrap()
    }

    /// `s` with every blank-node label dropped: two parses of one document
    /// label its blank nodes differently.
    fn unlabelled(s: &str) -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(i) = rest.find("_:") {
            out.push_str(&rest[..i + 2]);
            rest = rest[i + 2..]
                .trim_start_matches(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        }
        out.push_str(rest);
        out
    }

    /// Results as comparable keys, sorted.
    fn result_keys<'r>(results: impl IntoIterator<Item = &'r ValidationResult>) -> Vec<String> {
        let mut k: Vec<String> = results
            .into_iter()
            .map(|r| {
                unlabelled(&format!(
                    "{} | {:?} | {:?} | {} | {}",
                    r.focus_node, r.path, r.value, r.source_constraint, r.source_shape
                ))
            })
            .collect();
        k.sort();
        k
    }

    fn results_of(verdict: &Result<(), ValidationReport>) -> Vec<String> {
        match verdict {
            Ok(()) => Vec::new(),
            Err(report) => result_keys(&report.results),
        }
    }

    /// One case of the differential test: `write` on a graph holding `base`
    /// (which conforms), gated by `shapes`.
    struct Case {
        name: &'static str,
        shapes: &'static str,
        base: &'static str,
        write: Write,
        refused: bool,
        focused: bool,
    }

    /// Gate `case` with the whole graph staged and with the focused sandbox,
    /// and hold both to a full validation of the graph after the write.
    fn differential(case: &Case) {
        let name = case.name;
        let (store, auth, studio) = gated(case.shapes, case.base);
        let base = crate::shacl::validate(&store, DIFF_SHAPES, &[DATA_GRAPH.to_string()]).unwrap();
        assert!(
            base.conforms,
            "{name}: the base conforms: {:?}",
            base.results
        );
        let reference = after(case.shapes, case.base, &case.write);

        let (whole, whole_staging) = gate_on(&store, &auth, &studio, &case.write, false);
        let (focused, staging) = gate_on(&store, &auth, &studio, &case.write, true);

        assert_eq!(
            whole.is_err(),
            case.refused,
            "{name}: {:?}",
            results_of(&whole)
        );
        assert_eq!(
            whole.is_err(),
            !reference.conforms,
            "{name}: the whole-graph gate refuses exactly when the graph after the write \
             does not conform"
        );
        assert!(whole_staging.focus.is_none(), "{name}");
        assert_eq!(
            staging.focus.is_some(),
            case.focused,
            "{name}: focused? (whole because {:?})",
            staging.whole_because
        );
        assert_eq!(
            focused.is_err(),
            whole.is_err(),
            "{name}: the focused gate refuses exactly when the whole-graph gate does: {:?}",
            results_of(&focused)
        );
        assert_eq!(
            results_of(&whole),
            result_keys(&reference.results),
            "{name}: the whole-graph gate reports what a full validation reports"
        );
        assert_eq!(
            results_of(&focused),
            results_of(&whole),
            "{name}: the focused gate reports what the whole-graph gate reports"
        );
        // Whatever the verdict, a focused run reports exactly what a full
        // validation says about its focus nodes.
        if let Some(focus) = &staging.focus {
            let names: HashSet<String> = focus
                .iter()
                .map(crate::shacl::constraints::display_term)
                .collect();
            assert_eq!(
                results_of(&focused),
                result_keys(
                    reference
                        .results
                        .iter()
                        .filter(|r| names.contains(&r.focus_node))
                ),
                "{name}"
            );
            assert!(
                staging.data_quads <= whole_staging.data_quads,
                "{name}: the focused sandbox holds no more than the whole graph ({} > {})",
                staging.data_quads,
                whole_staging.data_quads
            );
        }
    }

    const S_PERSON: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:Person ; \
        sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:maxCount 1 ] .";
    const B_PERSON: &str =
        "ex:p1 a ex:Person ; ex:name \"Ada\" . ex:p2 a ex:Person ; ex:name \"Bob\" .";
    const S_AGENT: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:Agent ; \
        sh:property [ sh:path ex:name ; sh:minCount 1 ] .";
    const B_AGENT: &str = "ex:Person rdfs:subClassOf ex:Agent . \
        ex:p1 a ex:Person ; ex:name \"Ada\" . ex:r1 a ex:Robot .";
    const S_HQ: &str = "ex:S a sh:NodeShape ; sh:targetNode ex:hq ; \
        sh:property [ sh:path ex:employs ; sh:minCount 1 ; sh:class ex:Person ] .";
    const B_HQ: &str = "ex:hq ex:employs ex:p1 . ex:p1 a ex:Person .";
    const S_SUBJECTS: &str = "ex:S a sh:NodeShape ; sh:targetSubjectsOf ex:manages ; \
        sh:property [ sh:path ex:email ; sh:minCount 1 ] .";
    const B_SUBJECTS: &str = "ex:m1 ex:manages ex:t1 ; ex:email \"m@example.org\" .";
    const S_OBJECTS: &str = "ex:S a sh:NodeShape ; sh:targetObjectsOf ex:manages ; \
        sh:property [ sh:path ex:label ; sh:minCount 1 ] .";
    const B_OBJECTS: &str = "ex:m1 ex:manages ex:t1 . ex:t1 ex:label \"T\" .";
    const S_INVERSE: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:Team ; \
        sh:property [ sh:path [ sh:inversePath ex:memberOf ] ; sh:minCount 1 ; sh:maxCount 2 ] .";
    const B_INVERSE: &str = "ex:t1 a ex:Team . ex:a ex:memberOf ex:t1 . ex:b ex:memberOf ex:t1 .";
    const S_SEQUENCE: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:Order ; \
        sh:property [ sh:path ( ex:customer ex:country ) ; sh:minCount 1 ; sh:in ( \"NL\" \"BE\" ) ] .";
    const B_SEQUENCE: &str = "ex:o1 a ex:Order ; ex:customer ex:c1 . ex:c1 ex:country \"NL\" .";
    const S_CLASS: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:Book ; \
        sh:property [ sh:path ex:author ; sh:class ex:Person ] .";
    const B_CLASS: &str = "ex:Writer rdfs:subClassOf ex:Person . \
        ex:b1 a ex:Book ; ex:author ex:x . ex:x a ex:Person .";
    const S_NODE: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:Site ; \
        sh:property [ sh:path ex:address ; sh:node ex:AddressShape ] . \
        ex:AddressShape a sh:NodeShape ; sh:property [ sh:path ex:city ; sh:minCount 1 ] .";
    const B_NODE: &str = "ex:s1 a ex:Site ; ex:address ex:a1 . ex:a1 ex:city \"Delft\" .";
    const S_QUALIFIED: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:Car ; \
        sh:property [ sh:path ex:wheel ; sh:qualifiedMinCount 2 ; \
          sh:qualifiedValueShape [ sh:class ex:Wheel ; sh:property [ sh:path ex:size ; sh:minCount 1 ] ] ] .";
    const B_QUALIFIED: &str = "ex:c1 a ex:Car ; ex:wheel ex:w1 , ex:w2 . \
        ex:w1 a ex:Wheel ; ex:size 17 . ex:w2 a ex:Wheel ; ex:size 17 .";
    const S_CLOSED: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:Point ; sh:closed true ; \
        sh:ignoredProperties ( rdf:type ) ; sh:property [ sh:path ex:x ] , [ sh:path ex:y ] .";
    const B_CLOSED: &str = "ex:pt a ex:Point ; ex:x 1 ; ex:y 2 .";
    const S_CHAIN: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:Part ; \
        sh:property [ sh:path [ sh:oneOrMorePath ex:partOf ] ; sh:maxCount 3 ] .";
    const B_CHAIN: &str = "ex:a a ex:Part ; ex:partOf ex:b . ex:b ex:partOf ex:c .";
    const S_EQUALS: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:P ; \
        sh:property [ sh:path ex:a ; sh:equals ex:b ] .";
    const B_EQUALS: &str = "ex:p a ex:P ; ex:a 1 ; ex:b 1 .";
    const S_SPARQL: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:Person ; sh:sparql [ \
        sh:select \"SELECT $this WHERE { $this <http://example.org/age> ?a . FILTER(?a < 0) }\" ] .";
    const B_SPARQL: &str = "ex:p1 a ex:Person ; ex:age 3 .";

    fn case(
        name: &'static str,
        shapes: &'static str,
        base: &'static str,
        write: Write,
        refused: bool,
        focused: bool,
    ) -> Case {
        Case {
            name,
            shapes,
            base,
            write,
            refused,
            focused,
        }
    }

    fn post(ttl: &'static str) -> Write {
        Write::Post(ttl)
    }

    fn patch(remove: &'static str, add: &'static str) -> Write {
        Write::Patch { remove, add }
    }

    /// The focused gate against the whole-graph gate, and both against a full
    /// validation of the graph after the write, over the shape constructs
    /// whose results depend on triples about other nodes. Each base conforms,
    /// as a graph every write of which passed its gates does.
    #[test]
    fn the_focused_gate_refuses_exactly_when_the_whole_graph_gate_does() {
        let cases = [
            case(
                "targetClass: a second name on an existing subject",
                S_PERSON,
                B_PERSON,
                post("ex:p1 ex:name \"Eve\" ."),
                true,
                true,
            ),
            case(
                "targetClass: a new instance without a name",
                S_PERSON,
                B_PERSON,
                post("ex:p3 a ex:Person ."),
                true,
                true,
            ),
            case(
                "targetClass: a new instance with a name",
                S_PERSON,
                B_PERSON,
                post("ex:p3 a ex:Person ; ex:name \"Cy\" ."),
                false,
                true,
            ),
            case(
                "patch: deleting an existing subject's only name",
                S_PERSON,
                B_PERSON,
                patch("ex:p2 ex:name \"Bob\" .", ""),
                true,
                true,
            ),
            case(
                "patch: replacing a name",
                S_PERSON,
                B_PERSON,
                patch("ex:p2 ex:name \"Bob\" .", "ex:p2 ex:name \"Bo\" ."),
                false,
                true,
            ),
            case(
                "patch: deleting a target's type with its name",
                S_PERSON,
                B_PERSON,
                patch("ex:p2 a ex:Person ; ex:name \"Bob\" .", ""),
                false,
                true,
            ),
            case(
                "targetClass through rdfs:subClassOf: a new instance of the subclass",
                S_AGENT,
                B_AGENT,
                post("ex:p9 a ex:Person ."),
                true,
                true,
            ),
            case(
                "a new rdfs:subClassOf makes existing nodes targets: staged whole",
                S_AGENT,
                B_AGENT,
                post("ex:Robot rdfs:subClassOf ex:Agent ."),
                true,
                false,
            ),
            case(
                "targetNode: deleting a value node's type",
                S_HQ,
                B_HQ,
                patch("ex:p1 a ex:Person .", ""),
                true,
                true,
            ),
            case(
                "targetNode: a value node without the class",
                S_HQ,
                B_HQ,
                post("ex:hq ex:employs ex:p2 ."),
                true,
                true,
            ),
            case(
                "targetNode: a value node typed by the same payload",
                S_HQ,
                B_HQ,
                post("ex:hq ex:employs ex:p2 . ex:p2 a ex:Person ."),
                false,
                true,
            ),
            case(
                "targetSubjectsOf: a new subject",
                S_SUBJECTS,
                B_SUBJECTS,
                post("ex:m2 ex:manages ex:t1 ."),
                true,
                true,
            ),
            case(
                "targetSubjectsOf: deleting the email",
                S_SUBJECTS,
                B_SUBJECTS,
                patch("ex:m1 ex:email \"m@example.org\" .", ""),
                true,
                true,
            ),
            case(
                "targetObjectsOf: a new object",
                S_OBJECTS,
                B_OBJECTS,
                post("ex:m1 ex:manages ex:t2 ."),
                true,
                true,
            ),
            case(
                "targetObjectsOf: a new object with its label",
                S_OBJECTS,
                B_OBJECTS,
                post("ex:m1 ex:manages ex:t2 . ex:t2 ex:label \"U\" ."),
                false,
                true,
            ),
            case(
                "inverse path: a third member",
                S_INVERSE,
                B_INVERSE,
                post("ex:c ex:memberOf ex:t1 ."),
                true,
                true,
            ),
            case(
                "inverse path: deleting every member",
                S_INVERSE,
                B_INVERSE,
                patch("ex:a ex:memberOf ex:t1 . ex:b ex:memberOf ex:t1 .", ""),
                true,
                true,
            ),
            case(
                "inverse path: moving one member",
                S_INVERSE,
                B_INVERSE,
                patch("ex:a ex:memberOf ex:t1 .", "ex:a ex:memberOf ex:t2 ."),
                false,
                true,
            ),
            case(
                "sequence path: a second value two hops away",
                S_SEQUENCE,
                B_SEQUENCE,
                post("ex:c1 ex:country \"DE\" ."),
                true,
                true,
            ),
            case(
                "sequence path: deleting the value",
                S_SEQUENCE,
                B_SEQUENCE,
                patch("ex:c1 ex:country \"NL\" .", ""),
                true,
                true,
            ),
            case(
                "sequence path: replacing the value",
                S_SEQUENCE,
                B_SEQUENCE,
                patch("ex:c1 ex:country \"NL\" .", "ex:c1 ex:country \"BE\" ."),
                false,
                true,
            ),
            case(
                "sh:class: a value node whose type the payload adds",
                S_CLASS,
                B_CLASS,
                post("ex:b1 ex:author ex:y . ex:y a ex:Writer ."),
                false,
                true,
            ),
            case(
                "sh:class: a value node without a type",
                S_CLASS,
                B_CLASS,
                post("ex:b1 ex:author ex:z ."),
                true,
                true,
            ),
            case(
                "sh:class: deleting a value node's type",
                S_CLASS,
                B_CLASS,
                patch("ex:x a ex:Person .", ""),
                true,
                true,
            ),
            case(
                "sh:node: deleting a property of the referred node",
                S_NODE,
                B_NODE,
                patch("ex:a1 ex:city \"Delft\" .", ""),
                true,
                true,
            ),
            case(
                "sh:node: a new target referring to a node that does not conform",
                S_NODE,
                B_NODE,
                post("ex:s2 a ex:Site ; ex:address [ ex:zip \"2611\" ] ."),
                true,
                true,
            ),
            case(
                "sh:qualifiedValueShape: a value node loses its class",
                S_QUALIFIED,
                B_QUALIFIED,
                patch("ex:w1 a ex:Wheel .", ""),
                true,
                true,
            ),
            case(
                "sh:qualifiedValueShape: a value node loses a nested property",
                S_QUALIFIED,
                B_QUALIFIED,
                patch("ex:w2 ex:size 17 .", ""),
                true,
                true,
            ),
            case(
                "sh:qualifiedValueShape: one more conforming value",
                S_QUALIFIED,
                B_QUALIFIED,
                post("ex:c1 ex:wheel ex:w3 . ex:w3 a ex:Wheel ; ex:size 18 ."),
                false,
                true,
            ),
            case(
                "sh:closed: a property the shape does not allow",
                S_CLOSED,
                B_CLOSED,
                post("ex:pt ex:z 3 ."),
                true,
                true,
            ),
            case(
                "sh:closed: an allowed property",
                S_CLOSED,
                B_CLOSED,
                post("ex:pt ex:x 5 ."),
                false,
                true,
            ),
            case(
                "sh:oneOrMorePath: the chain above a target grows",
                S_CHAIN,
                B_CHAIN,
                post("ex:c ex:partOf ex:d . ex:d ex:partOf ex:e ."),
                true,
                true,
            ),
            case(
                "sh:equals: a value of the other property",
                S_EQUALS,
                B_EQUALS,
                post("ex:p ex:b 2 ."),
                true,
                true,
            ),
            case(
                "a SPARQL constraint: staged whole",
                S_SPARQL,
                B_SPARQL,
                post("ex:p1 ex:age -1 ."),
                true,
                false,
            ),
            case(
                "a SPARQL constraint: staged whole, conforming",
                S_SPARQL,
                B_SPARQL,
                post("ex:p1 ex:name \"Ada\" ."),
                false,
                false,
            ),
        ];
        for c in &cases {
            differential(c);
        }
    }

    /// The one difference from copying the whole graph, by design: a
    /// violation the graph already holds, on a node the write cannot affect,
    /// no longer refuses the write. The full-copy gate refused every merge
    /// into such a graph (written through SPARQL Update, or before the gate
    /// or its shapes existed) until all of it was repaired; the focused gate
    /// refuses a write only for the nodes it can affect, and reports what a
    /// full validation says about those.
    #[test]
    fn a_violation_the_write_cannot_affect_does_not_refuse_it() {
        let base = "ex:p1 a ex:Person ; ex:name \"Ada\" . ex:bad a ex:Person .";
        let (store, auth, studio) = gated(S_PERSON, base);
        let gate =
            |write: &Write, focus_allowed| gate_on(&store, &auth, &studio, write, focus_allowed).0;

        let unrelated = post("ex:p1 ex:nickname \"A\" .");
        assert!(
            gate(&unrelated, false).is_err(),
            "the whole graph does not conform"
        );
        assert!(
            gate(&unrelated, true).is_ok(),
            "the write affects ex:p1 only"
        );

        let bad_too = post("ex:p3 a ex:Person .");
        let whole = results_of(&gate(&bad_too, false));
        let focused = results_of(&gate(&bad_too, true));
        assert_eq!(whole.len(), 2, "{whole:?}");
        assert_eq!(focused.len(), 1, "{focused:?}");
        assert!(
            focused[0].starts_with("http://example.org/p3 "),
            "{focused:?}"
        );
        assert!(whole.contains(&focused[0]));

        let repair = post("ex:bad ex:name \"Bea\" .");
        assert!(gate(&repair, false).is_ok());
        assert!(gate(&repair, true).is_ok());
    }

    /// A small write into a big graph stages its own triples and what the
    /// nodes it can affect read, not the graph: here one new asset, and the
    /// deletion of a type that ten children's `sh:class` reads.
    #[test]
    fn a_small_write_into_a_big_graph_stages_only_what_it_reads() {
        let shapes = "ex:S a sh:NodeShape ; sh:targetClass ex:Asset ; \
            sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:maxCount 1 ] , \
                        [ sh:path ex:partOf ; sh:maxCount 1 ; sh:class ex:Asset ] .";
        let mut base = String::new();
        for i in 0..5_000 {
            base.push_str(&format!("ex:a{i} a ex:Asset ; ex:name \"A{i}\""));
            if i > 0 {
                base.push_str(&format!(" ; ex:partOf ex:a{}", i / 10));
            }
            base.push_str(" .\n");
        }
        let (store, auth, studio) = gated(shapes, &base);
        let graph_quads = store.graph_count_cached(Some(DATA_GRAPH)).unwrap();
        assert_eq!(graph_quads, 14_999);

        let new_asset = post("ex:a5000 a ex:Asset ; ex:name \"New\" ; ex:partOf ex:a500 .");
        let (whole, whole_staging) = gate_on(&store, &auth, &studio, &new_asset, false);
        let (focused, staging) = gate_on(&store, &auth, &studio, &new_asset, true);
        assert!(whole.is_ok() && focused.is_ok());
        assert_eq!(whole_staging.data_quads, graph_quads + 3);
        let focus = staging.focus.expect("a focused run");
        assert_eq!(
            focus.len(),
            1,
            "the new asset; nothing refers to it: {focus:?}"
        );
        // Its three triples, and the type of the asset it is part of.
        assert_eq!(staging.data_quads, 4);

        // Ten assets are part of ex:a1: deleting its type breaks their
        // `sh:class`, and leaves ex:a1 itself no target.
        let untyped = patch("ex:a1 a ex:Asset .", "");
        let (whole, whole_staging) = gate_on(&store, &auth, &studio, &untyped, false);
        let (focused, staging) = gate_on(&store, &auth, &studio, &untyped, true);
        assert_eq!(whole_staging.data_quads, graph_quads - 1);
        let focus = staging.focus.expect("a focused run");
        assert_eq!(focus.len(), 11, "ex:a1 and the ten parts of it: {focus:?}");
        assert!(
            staging.data_quads < 50,
            "{} quads staged",
            staging.data_quads
        );
        assert_eq!(results_of(&whole).len(), 10);
        assert_eq!(results_of(&focused), results_of(&whole));
    }
}
