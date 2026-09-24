use super::constraints::{evaluate_constraint, evaluate_constraint_with_values};
use super::report::{RunMetrics, Severity, ValidationReport, ValidationResult};
use super::shapes::*;
use super::view::{DataView, GraphSel};
use crate::store::TripleStore;
use opengraph::spargebra::{Query as SpargebraQuery, SparqlParser};
use oxigraph::model::{GraphName, NamedNode, NamedOrBlankNode, Quad, Term, Triple};
use rayon::prelude::*;
use tracing::{debug, info, warn};

const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// Maximum loader recursion depth for inline shapes (sh:node / sh:not / sh:and /
/// nested sh:property …). Inline shapes are loaded eagerly, so a cyclic shapes
/// graph (A `sh:node` B, B `sh:node` A) must be cut at load time as well as at
/// evaluation time.
const MAX_SHAPE_LOAD_DEPTH: u32 = 50;

/// Marks the loader's own recursion bound, as opposed to a shape that is
/// genuinely malformed. A shapes graph may legitimately be recursive (SHACL
/// §3.4.3 leaves recursive shapes undefined and lets a processor stop), so
/// hitting the bound drops the member being loaded and leaves the rest of
/// the run intact; every OTHER load error fails the shapes graph.
const RECURSION_BOUND: &str = "shape load recursion bound";

thread_local! {
    static SHAPE_LOAD_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

struct LoadDepthGuard;
impl LoadDepthGuard {
    fn enter() -> (Self, bool) {
        let depth = SHAPE_LOAD_DEPTH.with(|d| {
            let v = d.get() + 1;
            d.set(v);
            v
        });
        (LoadDepthGuard, depth <= MAX_SHAPE_LOAD_DEPTH)
    }
}
impl Drop for LoadDepthGuard {
    fn drop(&mut self) {
        SHAPE_LOAD_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Validate data graphs against shapes in a shapes graph.
///
/// Returns a `ValidationReport` summarising all constraint violations.
pub fn validate(
    store: &TripleStore,
    shapes_graph: &str,
    data_graphs: &[String],
) -> Result<ValidationReport, String> {
    info!(
        "SHACL validation: shapes_graph=<{}>, data_graphs={:?}",
        shapes_graph, data_graphs
    );
    let started = std::time::Instant::now();

    let shapes = load_shapes(store, shapes_graph)?;
    debug!("Loaded {} shapes", shapes.len());

    // One data source for the whole run (see `view.rs`): the accelerator's
    // clean RAM copy, else one RocksDB snapshot, else the live memory store.
    // Class closures and target instance sets are computed up front, in
    // parallel, so the fan-out below takes no lock and runs no SPARQL.
    let mut view = DataView::new(store, data_graphs);
    view.prepare(&shapes);

    // `shapes_slice` is a shared immutable reference passed into parallel closures so
    // that logical constraint operators (sh:not, sh:and, sh:or, sh:xone, sh:node,
    // sh:qualifiedValueShape) can look up sibling shapes by IRI.
    let shapes_slice: &[Shape] = &shapes;

    // Targets first, per shape in parallel.
    let targeted: Vec<(&Shape, Vec<Term>)> = {
        let view = &view;
        shapes_slice
            .par_iter()
            .filter(|shape| !shape.deactivated)
            .map(|shape| {
                let focus_nodes = resolve_targets(view, shape);
                debug!(
                    "Shape <{}> has {} target nodes",
                    shape.iri,
                    focus_nodes.len()
                );
                (shape, focus_nodes)
            })
            .collect()
    };
    // Sized by the real focus counts: one scan per (graph, shape predicate)
    // when the run is large enough for hash lookups to beat index seeks.
    view.build_index(shapes_slice, &targeted);
    debug!(
        "SHACL data source: {} (run index: {})",
        view.source_kind(),
        view.has_index()
    );
    let view = &view;

    // Evaluate shapes and focus nodes in parallel using rayon. Each shape is
    // independent — the evaluator only reads the shared view.
    let all_results: Vec<ValidationResult> = targeted
        .par_iter()
        .flat_map(|(shape, focus_nodes)| {
            let severity = shape
                .severity
                .as_deref()
                .map(Severity::from_iri)
                .unwrap_or(Severity::Violation);

            focus_nodes
                .par_iter()
                .flat_map(|focus_node| {
                    let mut results = Vec::new();

                    // Node-level constraints
                    for constraint in &shape.constraints {
                        let rs = evaluate_constraint(
                            view,
                            shapes_slice,
                            &shape.iri,
                            focus_node,
                            constraint,
                            None,
                            &severity,
                        );
                        results.extend(apply_message(rs, &shape.message, constraint));
                    }

                    // Property shape constraints. A property shape's own
                    // sh:severity / sh:message override the parent shape's
                    // (SHACL §3.6 — the shape that declares the constraint).
                    // The value nodes along the shape's path are fetched once
                    // and shared by all of its constraints.
                    for prop_shape in &shape.property_shapes {
                        let shape_iri = prop_shape.iri.as_deref().unwrap_or(&shape.iri);
                        let prop_severity = prop_shape
                            .severity
                            .as_deref()
                            .map(Severity::from_iri)
                            .unwrap_or_else(|| severity.clone());
                        let values = super::constraints::value_nodes(
                            view,
                            focus_node,
                            Some(&prop_shape.path),
                        );

                        for constraint in &prop_shape.constraints {
                            let rs = evaluate_constraint_with_values(
                                view,
                                shapes_slice,
                                shape_iri,
                                focus_node,
                                constraint,
                                Some(&prop_shape.path),
                                &values,
                                &prop_severity,
                            );
                            results.extend(apply_message(rs, &prop_shape.message, constraint));
                        }
                    }

                    results
                })
                .collect::<Vec<_>>()
        })
        .collect();

    let conforms = all_results.is_empty();
    let results_count = all_results.len();

    debug!("SHACL validation complete: {} violations", results_count);

    // Graph-reach measurement (off unless OTS_SHACL_REACH_PROBE is set): how
    // often a `sh:path` found nothing per data graph where the merge of them
    // would have found values. Reported here and nowhere else — never as a
    // result, because `conforms` is `results.is_empty()`.
    let (diverged, extra_values, examined) = view.reach_probe.totals();
    if examined > 0 {
        warn!(
            shapes_graph = %shapes_graph,
            data_graphs = data_graphs.len(),
            diverged,
            extra_values,
            examined,
            "graph-reach probe: of {examined} cross-graph-capable value-node lookups, \
             {diverged} would yield more values over the merge of the data graphs \
             ({extra_values} extra value nodes in total); sh:path is evaluated per graph \
             while sh:sparql and the class machinery read them merged"
        );
    }

    // The run's own account of itself: for the report, the run row and the
    // workload telemetry (docs/notes/analytical-mirror-design.md §1.4).
    let metrics = RunMetrics {
        path: crate::store::telemetry::validation_path().to_string(),
        duration_ms: started.elapsed().as_millis() as u64,
        quads: data_graphs
            .iter()
            .filter_map(|g| store.graph_count_cached(Some(g.as_str())))
            .map(|n| n as u64)
            .sum(),
        graphs: view.graph_count() as u32,
        source: view.source_kind().to_string(),
        run_index: view.has_index(),
    };
    store
        .telemetry()
        .record_validation(crate::store::telemetry::ValidationSample {
            path: crate::store::telemetry::validation_path(),
            duration_ms: metrics.duration_ms.min(u32::MAX as u64) as u32,
            quads: metrics.quads,
            graphs: metrics.graphs,
            source: view.source_kind(),
            run_index: metrics.run_index,
            results: results_count.min(u32::MAX as usize) as u32,
        });

    Ok(ValidationReport {
        conforms,
        results: all_results,
        results_count,
        metrics: Some(metrics),
    })
}

/// Apply a shape-declared `sh:message` to the results of one constraint
/// evaluation. SPARQL constraints keep their own `sh:message` (declared on the
/// SPARQLConstraint node itself), as do constraint components (the
/// validator's, with its `{$param}` placeholders rendered).
fn apply_message(
    mut results: Vec<ValidationResult>,
    message: &Option<String>,
    constraint: &Constraint,
) -> Vec<ValidationResult> {
    if let Some(msg) = message {
        if !matches!(
            constraint,
            Constraint::SparqlConstraint { .. } | Constraint::Custom(_)
        ) {
            for r in &mut results {
                r.message = msg.clone();
            }
        }
    }
    results
}

/// Apply SHACL-AF inference rules and materialise derived triples, choosing the
/// target graph the way a single-graph run always has: into the one data graph,
/// or — with no data graph at all — into the store's unnamed default graph.
///
/// A caller that runs rules **for** someone (the `/infer` endpoint, for a
/// dataset) should name the target itself with [`infer_into`]: it is the caller
/// that knows which graphs the dataset holds, and a run over several data graphs
/// has no "the" graph to fall back on.
///
/// Returns the number of triples generated.
pub fn infer(
    store: &TripleStore,
    shapes_graph: &str,
    data_graphs: &[String],
) -> Result<usize, String> {
    infer_into(store, shapes_graph, data_graphs, None)
}

/// [`infer`], materialising every derived triple into `target_graph`.
///
/// `target_graph` must be a graph the run is allowed to write — for a dataset,
/// one it holds (`crate::auth::dataset_graph::dataset_holds_graph`). The engine
/// writes nowhere else: rules read `data_graphs` and their output lands here,
/// whatever the rule bodies say.
pub fn infer_into(
    store: &TripleStore,
    shapes_graph: &str,
    data_graphs: &[String],
    target_graph: Option<&str>,
) -> Result<usize, String> {
    info!(
        "SHACL-AF inference: shapes_graph=<{}>, data_graphs={:?}",
        shapes_graph, data_graphs
    );

    let rules = load_rules(store, shapes_graph)?;
    debug!("Loaded {} rules", rules.len());

    let mut total_inferred: usize = 0;

    // Iterate until fixed point. Convergence is measured by the store's *real*
    // triple-count delta across a full round: a `sh:rule` whose materialisation
    // is already present inserts nothing (RDF set semantics), so it does not grow
    // the store. Once a whole round adds zero triples we are at the fixed point.
    // This both terminates early — instead of always running the full iteration
    // cap whenever any rule has a focus node — and reports an accurate count.
    // Where derived triples land. A caller that knows the run's dataset names
    // the graph (see `infer_into`); otherwise SHACL-AF rules materialise into
    // the data graph they infer over, and with several data graphs there is no
    // single "the" graph, so the run keeps the historical default-graph
    // behaviour rather than silently picking one of them.
    let target_graph: Option<&str> = match (target_graph, data_graphs) {
        (Some(g), _) => Some(g),
        (None, [one]) => Some(one.as_str()),
        (None, _) => None,
    };

    for iteration in 0..100 {
        let before = store.len().map_err(|e| e.to_string())?;

        for rule in &rules {
            // One view per rule: the rule's targets and its condition shapes
            // are prepared together, so the condition check reads the same
            // snapshot the targets were resolved from.
            let target_shape = Shape {
                iri: rule.shape_iri.clone(),
                name: None,
                shape_type: ShapeType::NodeShape,
                targets: rule.targets.clone(),
                constraints: Vec::new(),
                property_shapes: Vec::new(),
                severity: None,
                message: None,
                deactivated: false,
            };
            let mut prepared: Vec<Shape> = Vec::with_capacity(1 + rule.conditions.len());
            prepared.push(target_shape.clone());
            prepared.extend(rule.conditions.iter().cloned());
            let mut view = DataView::new(store, data_graphs);
            view.prepare(&prepared);
            let focus_nodes = resolve_targets(&view, &target_shape);

            for focus_node in &focus_nodes {
                // sh:condition: the focus node must conform to every condition
                // shape (SHACL-AF §4.1), else the rule does not fire for it.
                let conforms = rule.conditions.iter().all(|c| {
                    super::constraints::validate_inline_shape(
                        &view,
                        &rule.conditions,
                        focus_node,
                        c,
                        &Severity::Violation,
                    )
                    .is_empty()
                });
                if !conforms {
                    continue;
                }
                apply_rule(store, focus_node, &rule.body, data_graphs, target_graph)?;
            }
        }

        let after = store.len().map_err(|e| e.to_string())?;
        let delta = after.saturating_sub(before);
        total_inferred += delta;
        debug!("Iteration {}: inferred {} triples", iteration + 1, delta);

        if delta == 0 {
            debug!("Fixed point reached after {} iterations", iteration + 1);
            break;
        }
    }

    info!("Total inferred triples: {}", total_inferred);
    Ok(total_inferred)
}

// ---------------------------------------------------------------------------
// Shape loading
// ---------------------------------------------------------------------------

pub(crate) fn load_shapes(store: &TripleStore, shapes_graph: &str) -> Result<Vec<Shape>, String> {
    // Find all node shapes in the shapes graph
    let query = format!(
        r#"
        PREFIX sh: <{SH}>
        PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        SELECT DISTINCT ?shape WHERE {{
            GRAPH <{shapes_graph}> {{
                {{ ?shape rdf:type sh:NodeShape }}
                UNION
                {{ ?shape sh:targetClass ?tc }}
                UNION
                {{ ?shape sh:targetNode ?tn }}
                UNION
                {{ ?shape sh:targetSubjectsOf ?tso }}
                UNION
                {{ ?shape sh:targetObjectsOf ?too }}
                UNION
                {{ ?shape sh:property ?p }}
            }}
        }}
        "#,
    );

    let shape_iris = execute_select_single(store, &query, "shape")?;
    let mut shapes = Vec::new();

    for shape_iri in &shape_iris {
        // A shape that cannot be loaded is an ill-formed shapes graph, and
        // validation against it is an error the caller must see — the write
        // gate turns it into 422. Dropping the shape with a warning made the
        // graph conform by omission: a `sh:sparql` constraint that did not
        // parse silently disabled its whole shape.
        let shape = load_single_shape(store, shapes_graph, shape_iri)
            .map_err(|e| format!("failed to load shape <{shape_iri}>: {e}"))?;
        shapes.push(shape);
    }

    Ok(shapes)
}

fn load_single_shape(
    store: &TripleStore,
    shapes_graph: &str,
    shape_iri: &str,
) -> Result<Shape, String> {
    // Load targets
    let targets = load_targets(store, shapes_graph, shape_iri)?;

    // Deactivated?
    let deactivated = ask(
        store,
        &format!("ASK {{ GRAPH <{shapes_graph}> {{ <{shape_iri}> <{SH}deactivated> true }} }}"),
    );

    // Severity
    let severity = single_value(store, shapes_graph, shape_iri, &format!("{}severity", SH));

    // Message
    let message = single_value(store, shapes_graph, shape_iri, &format!("{}message", SH));
    let name = single_value(store, shapes_graph, shape_iri, &format!("{}name", SH));

    // Load direct constraints on the node shape
    let mut constraints = load_constraints(store, shapes_graph, shape_iri)?;

    // Load property shapes
    let mut property_shapes = load_property_shapes(store, shapes_graph, shape_iri)?;

    // A top-level *property* shape (it has its own sh:path, e.g.
    // `ex:S a sh:PropertyShape ; sh:path ex:p ; sh:minCount 1 ; sh:targetNode …`):
    // its constraints apply along that path, and any nested sh:property children
    // apply to the path's value nodes. Model it as a single own-path property
    // shape so the engine evaluates everything in path context.
    let shape_type = if let Some(own_path) =
        single_value(store, shapes_graph, shape_iri, &format!("{}path", SH))
            .and_then(|p| parse_property_path(store, shapes_graph, &p))
    {
        constraints.extend(
            property_shapes
                .drain(..)
                .map(|ps| Constraint::Property(Box::new(ps))),
        );
        property_shapes = vec![PropertyShape {
            iri: Some(shape_iri.to_string()),
            path: own_path,
            constraints: std::mem::take(&mut constraints),
            name: None,
            description: None,
            severity: None,
            message: None,
        }];
        ShapeType::PropertyShape
    } else {
        ShapeType::NodeShape
    };

    Ok(Shape {
        iri: shape_iri.to_string(),
        name,
        shape_type,
        targets,
        constraints,
        property_shapes,
        severity,
        message,
        deactivated,
    })
}

fn load_targets(
    store: &TripleStore,
    shapes_graph: &str,
    shape_iri: &str,
) -> Result<Vec<Target>, String> {
    let mut targets = Vec::new();

    // Class / predicate targets are resolved through the raw quad index (like
    // sh:targetNode below), not a SPARQL query with `<{shape_iri}>`: the shape
    // query also returns blank-node shapes (inline sh:and/sh:or/sh:node
    // members), for which `<_:…>` is not a valid IRI and the query errored.
    // `load_shapes` used to drop such shapes with a warning; now that a shape
    // that fails to load fails the run, the loader must not fail on them.
    // sh:targetClass
    for c in multi_values(store, shapes_graph, shape_iri, &format!("{SH}targetClass")) {
        targets.push(Target::TargetClass(c));
    }

    // sh:targetNode — keep the *typed* term: target nodes may be literals
    // (`sh:targetNode 42`) whose datatype matters for node-level constraints.
    for t in store.objects_for_subject_in_graph(
        shape_iri,
        &format!("{SH}targetNode"),
        Some(shapes_graph),
    ) {
        targets.push(Target::TargetNode(t));
    }

    // sh:targetSubjectsOf
    for p in multi_values(
        store,
        shapes_graph,
        shape_iri,
        &format!("{SH}targetSubjectsOf"),
    ) {
        targets.push(Target::TargetSubjectsOf(p));
    }

    // sh:targetObjectsOf
    for p in multi_values(
        store,
        shapes_graph,
        shape_iri,
        &format!("{SH}targetObjectsOf"),
    ) {
        targets.push(Target::TargetObjectsOf(p));
    }

    // Implicit class target: if the shape itself is also an rdfs:Class
    let is_class = ask(
        store,
        &format!(
            "ASK {{ GRAPH <{shapes_graph}> {{ <{shape_iri}> a <http://www.w3.org/2000/01/rdf-schema#Class> }} }}"
        ),
    );
    if is_class {
        targets.push(Target::TargetClass(shape_iri.to_string()));
    }

    // SHACL-AF: SPARQL targets. Resolve the target node through the raw quad index
    // (it may be named, e.g. ex:BruggenOverWater, or an inline blank node) so its
    // sh:select and sh:prefixes are both reachable. The previous SPARQL-query form
    // could not read prefixes off a blank declaration node and prepended none.
    for target_node in store
        .objects_for_subject_in_graph(shape_iri, &format!("{SH}target"), Some(shapes_graph))
        .iter()
        .map(term_to_lexical)
    {
        if let Some(select) =
            single_value(store, shapes_graph, &target_node, &format!("{SH}select"))
        {
            let prefixes = sparql_prefixes(store, shapes_graph, &target_node);
            let query = format!("{prefixes}{select}");
            // A target that does not parse, or does not project ?this, used
            // to yield no focus nodes — the shape silently validated nothing.
            // Both fail the shapes graph at load time now.
            check_sparql_target(&query)
                .map_err(|e| format!("shape <{shape_iri}>: sh:target <{target_node}> {e}"))?;
            targets.push(Target::SparqlTarget(query));
        }
    }

    Ok(targets)
}

fn load_constraints(
    store: &TripleStore,
    shapes_graph: &str,
    shape_iri: &str,
) -> Result<Vec<Constraint>, String> {
    let mut constraints = Vec::new();

    // Typed first-object lookup for constraint constants (sh:hasValue, ranges).
    let typed_value = |pred: &str| -> Option<Term> {
        store
            .objects_for_subject_in_graph(shape_iri, &format!("{SH}{pred}"), Some(shapes_graph))
            .into_iter()
            .next()
    };

    // sh:class
    for v in multi_values(store, shapes_graph, shape_iri, &format!("{}class", SH)) {
        constraints.push(Constraint::Class(v));
    }

    // sh:datatype
    for v in multi_values(store, shapes_graph, shape_iri, &format!("{}datatype", SH)) {
        constraints.push(Constraint::Datatype(v));
    }

    // sh:nodeKind
    if let Some(v) = single_value(store, shapes_graph, shape_iri, &format!("{}nodeKind", SH)) {
        if let Some(nk) = NodeKind::from_iri(&v) {
            constraints.push(Constraint::NodeKind(nk));
        }
    }

    // sh:minCount
    if let Some(v) = single_value(store, shapes_graph, shape_iri, &format!("{}minCount", SH)) {
        if let Ok(n) = v.parse::<usize>() {
            constraints.push(Constraint::MinCount(n));
        }
    }

    // sh:maxCount
    if let Some(v) = single_value(store, shapes_graph, shape_iri, &format!("{}maxCount", SH)) {
        if let Ok(n) = v.parse::<usize>() {
            constraints.push(Constraint::MaxCount(n));
        }
    }

    // sh:minLength
    if let Some(v) = single_value(store, shapes_graph, shape_iri, &format!("{}minLength", SH)) {
        if let Ok(n) = v.parse::<usize>() {
            constraints.push(Constraint::MinLength(n));
        }
    }

    // sh:maxLength
    if let Some(v) = single_value(store, shapes_graph, shape_iri, &format!("{}maxLength", SH)) {
        if let Ok(n) = v.parse::<usize>() {
            constraints.push(Constraint::MaxLength(n));
        }
    }

    // sh:pattern + sh:flags
    if let Some(pattern) = single_value(store, shapes_graph, shape_iri, &format!("{}pattern", SH)) {
        let flags = single_value(store, shapes_graph, shape_iri, &format!("{}flags", SH));
        constraints.push(Constraint::Pattern { pattern, flags });
    }

    // sh:minExclusive / sh:minInclusive / sh:maxExclusive / sh:maxInclusive —
    // the bound keeps its typed literal form for typed comparison.
    if let Some(v) = typed_value("minExclusive") {
        constraints.push(Constraint::MinExclusive(v));
    }
    if let Some(v) = typed_value("minInclusive") {
        constraints.push(Constraint::MinInclusive(v));
    }
    if let Some(v) = typed_value("maxExclusive") {
        constraints.push(Constraint::MaxExclusive(v));
    }
    if let Some(v) = typed_value("maxInclusive") {
        constraints.push(Constraint::MaxInclusive(v));
    }

    // sh:equals
    for v in multi_values(store, shapes_graph, shape_iri, &format!("{}equals", SH)) {
        constraints.push(Constraint::Equals(v));
    }

    // sh:disjoint
    for v in multi_values(store, shapes_graph, shape_iri, &format!("{}disjoint", SH)) {
        constraints.push(Constraint::Disjoint(v));
    }

    // sh:lessThan
    for v in multi_values(store, shapes_graph, shape_iri, &format!("{}lessThan", SH)) {
        constraints.push(Constraint::LessThan(v));
    }

    // sh:lessThanOrEquals
    for v in multi_values(
        store,
        shapes_graph,
        shape_iri,
        &format!("{}lessThanOrEquals", SH),
    ) {
        constraints.push(Constraint::LessThanOrEquals(v));
    }

    // sh:hasValue (typed: may be an IRI or a literal)
    if let Some(v) = typed_value("hasValue") {
        constraints.push(Constraint::HasValue(v));
    }

    // sh:in (RDF list, typed members)
    let in_values = load_rdf_list_terms(store, shapes_graph, shape_iri, &format!("{}in", SH));
    if !in_values.is_empty() {
        constraints.push(Constraint::In(in_values));
    }

    // sh:languageIn (RDF list of language-tag strings)
    let lang_values = load_rdf_list(store, shapes_graph, shape_iri, &format!("{}languageIn", SH));
    if !lang_values.is_empty() {
        constraints.push(Constraint::LanguageIn(lang_values));
    }

    // sh:uniqueLang
    if let Some(v) = single_value(store, shapes_graph, shape_iri, &format!("{}uniqueLang", SH)) {
        if v == "true" {
            constraints.push(Constraint::UniqueLang(true));
        }
    }

    // sh:closed + sh:ignoredProperties. A closed shape allows exactly the
    // predicate paths of its own property shapes plus the ignored list.
    let is_closed = store
        .objects_for_subject_in_graph(shape_iri, &format!("{SH}closed"), Some(shapes_graph))
        .iter()
        .any(|t| term_to_lexical(t) == "true");
    if is_closed {
        let ignored = load_rdf_list(
            store,
            shapes_graph,
            shape_iri,
            &format!("{}ignoredProperties", SH),
        );
        let mut allowed = Vec::new();
        for ps in store
            .objects_for_subject_in_graph(shape_iri, &format!("{SH}property"), Some(shapes_graph))
            .iter()
            .map(term_to_lexical)
        {
            if let Some(Term::NamedNode(p)) = store
                .objects_for_subject_in_graph(&ps, &format!("{SH}path"), Some(shapes_graph))
                .into_iter()
                .next()
            {
                allowed.push(p.as_str().to_string());
            }
        }
        constraints.push(Constraint::Closed {
            ignored_properties: ignored,
            allowed_properties: allowed,
        });
    }

    // sh:node — loaded inline (named or blank) so inline `sh:node [ … ]` bodies
    // are enforced rather than looked up — and silently skipped — in the
    // top-level shapes list. A member shape that fails to load fails the
    // shapes graph: it used to be skipped with `if let Ok(..)`, so a malformed
    // `sh:sparql` inside an `sh:node` body validated nothing and passed.
    for v in multi_values(store, shapes_graph, shape_iri, &format!("{}node", SH)) {
        if let Some(node_shape) = load_member_shape(store, shapes_graph, &v)? {
            constraints.push(Constraint::Node(Box::new(node_shape)));
        }
    }

    // sh:not
    if let Some(not_iri) = single_value(store, shapes_graph, shape_iri, &format!("{}not", SH)) {
        if let Some(not_shape) = load_member_shape(store, shapes_graph, &not_iri)? {
            constraints.push(Constraint::Not(Box::new(not_shape)));
        }
    }

    // sh:and (RDF list of shape IRIs)
    let and_iris = load_rdf_list(store, shapes_graph, shape_iri, &format!("{}and", SH));
    if !and_iris.is_empty() {
        let mut and_shapes = Vec::new();
        for iri in &and_iris {
            if let Some(s) = load_member_shape(store, shapes_graph, iri)? {
                and_shapes.push(s);
            }
        }
        if !and_shapes.is_empty() {
            constraints.push(Constraint::And(and_shapes));
        }
    }

    // sh:or (RDF list of shape IRIs)
    let or_iris = load_rdf_list(store, shapes_graph, shape_iri, &format!("{}or", SH));
    if !or_iris.is_empty() {
        let mut or_shapes = Vec::new();
        for iri in &or_iris {
            if let Some(s) = load_member_shape(store, shapes_graph, iri)? {
                or_shapes.push(s);
            }
        }
        if !or_shapes.is_empty() {
            constraints.push(Constraint::Or(or_shapes));
        }
    }

    // sh:xone (RDF list of shape IRIs)
    let xone_iris = load_rdf_list(store, shapes_graph, shape_iri, &format!("{}xone", SH));
    if !xone_iris.is_empty() {
        let mut xone_shapes = Vec::new();
        for iri in &xone_iris {
            if let Some(s) = load_member_shape(store, shapes_graph, iri)? {
                xone_shapes.push(s);
            }
        }
        if !xone_shapes.is_empty() {
            constraints.push(Constraint::Xone(xone_shapes));
        }
    }

    // sh:qualifiedValueShape + sh:qualifiedMinCount / sh:qualifiedMaxCount
    if let Some(qvs_iri) = single_value(
        store,
        shapes_graph,
        shape_iri,
        &format!("{}qualifiedValueShape", SH),
    ) {
        let min_count = single_value(
            store,
            shapes_graph,
            shape_iri,
            &format!("{}qualifiedMinCount", SH),
        )
        .and_then(|v| v.parse::<usize>().ok());
        let max_count = single_value(
            store,
            shapes_graph,
            shape_iri,
            &format!("{}qualifiedMaxCount", SH),
        )
        .and_then(|v| v.parse::<usize>().ok());
        let disjoint = single_value(
            store,
            shapes_graph,
            shape_iri,
            &format!("{}qualifiedValueShapesDisjoint", SH),
        )
        .is_some_and(|v| v == "true");
        // Load the value shape inline (named or blank) so an inline `[ … ]` is enforced
        // rather than looked up — and silently skipped — in the top-level shapes list.
        if let Some(qvs_shape) = load_member_shape(store, shapes_graph, &qvs_iri)? {
            constraints.push(Constraint::QualifiedValueShape {
                shape: Box::new(qvs_shape),
                min_count,
                max_count,
                disjoint,
                // Wired by load_property_shapes_inner once all siblings are loaded.
                sibling_shapes: Vec::new(),
            });
        }
    }

    // SHACL-AF: sh:sparql constraints. Resolve through the raw quad index so this
    // works whether the shape — and the SPARQLConstraint node — is named or a blank
    // node. The previous form interpolated `<{shape_iri}>` into a SPARQL query; for
    // a blank-node shape that produced `<_:bn>` (invalid IRI syntax), which made the
    // whole query error and, via `?`, dropped the entire shape — silently disabling
    // every blank-node-authored shape.
    for sparql_node in store
        .objects_for_subject_in_graph(shape_iri, &format!("{SH}sparql"), Some(shapes_graph))
        .iter()
        .map(term_to_lexical)
    {
        if let Some(select) =
            single_value(store, shapes_graph, &sparql_node, &format!("{SH}select"))
        {
            // Prepend the SHACL prefixes-mechanism prologue so prefixed names resolve.
            let prefixes = sparql_prefixes(store, shapes_graph, &sparql_node);
            let message = single_value(store, shapes_graph, &sparql_node, &format!("{SH}message"));
            // sh:severity may sit on the SPARQLConstraint node (e.g. sh:Warning) and
            // overrides the shape-level severity for this constraint's results.
            let severity =
                single_value(store, shapes_graph, &sparql_node, &format!("{SH}severity"));
            let select = format!("{prefixes}{select}");
            // Fail closed at load time: a `sh:select` that does not parse is an
            // ill-formed shapes graph, and validation against it must be an
            // error the caller sees (the write gate turns it into 422), never
            // a shape that quietly never fires.
            super::constraints::check_sparql_constraint(&select).map_err(|e| {
                format!("shape <{shape_iri}>: sh:sparql constraint does not parse: {e}")
            })?;
            constraints.push(Constraint::SparqlConstraint {
                select,
                message,
                severity,
            });
        }
    }

    // SHACL-AF §6: constraint components declared in the shapes graph. A shape
    // that carries a component's parameter predicates instantiates it — with
    // the node validator on a node shape, the property validator on a property
    // shape (one with sh:path), or the plain sh:validator — and every mandatory
    // parameter must be present (§6.2.1) for the component to apply.
    let is_property_shape =
        single_value(store, shapes_graph, shape_iri, &format!("{SH}path")).is_some();
    for comp in constraint_components(store, shapes_graph)?.iter() {
        let mut params: Vec<(String, Term)> = Vec::new();
        let mut complete = true;
        for p in &comp.parameters {
            match store
                .objects_for_subject_in_graph(shape_iri, &p.path, Some(shapes_graph))
                .into_iter()
                .next()
            {
                Some(v) => params.push((p.name.clone(), v)),
                None if p.optional => {}
                None => complete = false,
            }
        }
        if params.is_empty() || !complete {
            continue;
        }
        let validator = if is_property_shape {
            comp.property_validator
                .clone()
                .or_else(|| comp.validator.clone())
        } else {
            comp.node_validator
                .clone()
                .or_else(|| comp.validator.clone())
        };
        let Some(validator) = validator else {
            // A component the shape uses but that has no validator for the
            // shape's kind is an ill-formed shapes graph (§6.2.2), not a
            // constraint that quietly never fires.
            return Err(format!(
                "shape <{shape_iri}>: constraint component <{}> has no validator for a {} shape",
                comp.iri,
                if is_property_shape {
                    "property"
                } else {
                    "node"
                }
            ));
        };
        constraints.push(Constraint::Custom(Box::new(CustomConstraint {
            component: comp.iri.clone(),
            params,
            validator: validator.query,
            message: validator.message,
        })));
    }

    // SHACL-AF: sh:expression node expressions (path + comparison subset). The
    // expression node carries an sh:path and comparison constraints (e.g.
    // sh:minExclusive); values along the path from the focus must satisfy them.
    for expr_node in store
        .objects_for_subject_in_graph(shape_iri, &format!("{SH}expression"), Some(shapes_graph))
        .iter()
        .map(term_to_lexical)
    {
        let Some(path_val) = single_value(store, shapes_graph, &expr_node, &format!("{SH}path"))
        else {
            continue;
        };
        let Some(path) = parse_property_path(store, shapes_graph, &path_val) else {
            continue;
        };
        // Comparison/value constraints declared on the expression node (recursion is
        // bounded: the expression node carries no further sh:expression).
        let checks = load_constraints(store, shapes_graph, &expr_node)?;
        if !checks.is_empty() {
            let message = single_value(store, shapes_graph, &expr_node, &format!("{SH}message"));
            constraints.push(Constraint::Expression {
                path,
                checks,
                message,
            });
        }
    }

    Ok(constraints)
}

/// Load an inline (possibly blank-node) shape by IRI for use in logical constraint
/// operators, sh:node and sh:qualifiedValueShape. An inline shape carrying its own
/// `sh:path` is a *property* shape: its constraints apply along that path.
/// A `sh:parameter` of a constraint component.
#[derive(Debug, Clone)]
struct ComponentParameter {
    /// The parameter predicate a shape uses to supply the value.
    path: String,
    /// The SPARQL variable the validator sees: the path's local name (§6.2.1).
    name: String,
    optional: bool,
}

#[derive(Debug, Clone)]
struct ComponentValidator {
    query: CustomValidator,
    message: Option<String>,
}

/// A `sh:ConstraintComponent` (or an instance of a subclass) declared in the
/// shapes graph, with its parameters and validators.
#[derive(Debug, Clone)]
struct ConstraintComponentDecl {
    iri: String,
    parameters: Vec<ComponentParameter>,
    validator: Option<ComponentValidator>,
    node_validator: Option<ComponentValidator>,
    property_validator: Option<ComponentValidator>,
}

/// (shapes graph, store instance, store write generation, declarations).
type ComponentCacheEntry = (
    String,
    u64,
    u64,
    std::sync::Arc<Vec<ConstraintComponentDecl>>,
);

thread_local! {
    /// Components of the last shapes graph loaded on this thread, keyed by
    /// graph, store instance and store write generation: `load_constraints`
    /// runs once per shape, a shapes graph can hold thousands, and the
    /// declaration set is the same for all of them.
    static COMPONENTS: std::cell::RefCell<Option<ComponentCacheEntry>> =
        const { std::cell::RefCell::new(None) };
}

/// Every constraint component declared in `shapes_graph` (instances of
/// `sh:ConstraintComponent` or of one of its subclasses). Validator queries
/// are parse-checked here, prefixes prepended, so an ill-formed component
/// fails the shapes graph.
fn constraint_components(
    store: &TripleStore,
    shapes_graph: &str,
) -> Result<std::sync::Arc<Vec<ConstraintComponentDecl>>, String> {
    let instance = store.instance_id();
    let generation = store.write_generation();
    if let Some(hit) = COMPONENTS.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(g, id, gen, _)| g == shapes_graph && *id == instance && *gen == generation)
            .map(|(_, _, _, v)| v.clone())
    }) {
        return Ok(hit);
    }
    let iris = execute_select_single(
        store,
        &format!(
            r#"
            PREFIX sh: <{SH}>
            PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
            PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
            SELECT DISTINCT ?c WHERE {{
                GRAPH <{shapes_graph}> {{ ?c rdf:type/rdfs:subClassOf* sh:ConstraintComponent }}
            }}
            "#,
        ),
        "c",
    )?;
    let mut out = Vec::new();
    for iri in iris.iter().filter(|c| !c.starts_with("_:")) {
        let mut parameters = Vec::new();
        for pnode in store
            .objects_for_subject_in_graph(iri, &format!("{SH}parameter"), Some(shapes_graph))
            .iter()
            .map(term_to_lexical)
        {
            let Some(path) = single_value(store, shapes_graph, &pnode, &format!("{SH}path")) else {
                continue;
            };
            let name = path
                .rsplit(['#', '/'])
                .next()
                .unwrap_or(path.as_str())
                .to_string();
            let optional = single_value(store, shapes_graph, &pnode, &format!("{SH}optional"))
                .is_some_and(|v| v == "true" || v == "1");
            parameters.push(ComponentParameter {
                path,
                name,
                optional,
            });
        }
        let load_validator = |pred: &str| -> Result<Option<ComponentValidator>, String> {
            let Some(vnode) = single_value(store, shapes_graph, iri, &format!("{SH}{pred}")) else {
                return Ok(None);
            };
            let prefixes = sparql_prefixes(store, shapes_graph, &vnode);
            let message = single_value(store, shapes_graph, &vnode, &format!("{SH}message"));
            let query = if let Some(ask) =
                single_value(store, shapes_graph, &vnode, &format!("{SH}ask"))
            {
                let q = format!("{prefixes}{ask}");
                super::constraints::check_validator_query(&q, true).map_err(|e| {
                    format!("constraint component <{iri}>: sh:{pred} sh:ask does not parse: {e}")
                })?;
                CustomValidator::Ask(q)
            } else if let Some(select) =
                single_value(store, shapes_graph, &vnode, &format!("{SH}select"))
            {
                let q = format!("{prefixes}{select}");
                super::constraints::check_validator_query(&q, false).map_err(|e| {
                    format!("constraint component <{iri}>: sh:{pred} sh:select does not parse: {e}")
                })?;
                CustomValidator::Select(q)
            } else {
                return Err(format!(
                    "constraint component <{iri}>: sh:{pred} carries neither sh:ask nor sh:select"
                ));
            };
            Ok(Some(ComponentValidator { query, message }))
        };
        out.push(ConstraintComponentDecl {
            iri: iri.clone(),
            parameters,
            validator: load_validator("validator")?,
            node_validator: load_validator("nodeValidator")?,
            property_validator: load_validator("propertyValidator")?,
        });
    }
    let out = std::sync::Arc::new(out);
    COMPONENTS.with(|c| {
        *c.borrow_mut() = Some((shapes_graph.to_string(), instance, generation, out.clone()));
    });
    Ok(out)
}

/// Load an inline member shape (`sh:node`, `sh:not`, an `sh:and`/`sh:or`/
/// `sh:xone` member, a qualified value shape). `Ok(None)` means the loader's
/// recursion bound was reached and this member is dropped -- the shapes graph
/// stays usable, which is what keeps a recursive shapes graph validating and
/// its dataset writable. Any other error fails the shapes graph, so a
/// malformed member can never be silently skipped.
fn load_member_shape(
    store: &TripleStore,
    shapes_graph: &str,
    shape_iri: &str,
) -> Result<Option<Shape>, String> {
    match load_inline_shape(store, shapes_graph, shape_iri) {
        Ok(shape) => Ok(Some(shape)),
        Err(e) if e.starts_with(RECURSION_BOUND) => {
            warn!("{e}; the member shape is not enforced for this run");
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

fn load_inline_shape(
    store: &TripleStore,
    shapes_graph: &str,
    shape_iri: &str,
) -> Result<Shape, String> {
    // Inline shapes load eagerly and may reference each other — bound the loader
    // recursion so a cyclic shapes graph cannot overflow the stack at load time.
    let (_guard, within_limit) = LoadDepthGuard::enter();
    if !within_limit {
        return Err(format!(
            "{RECURSION_BOUND}: exceeded max depth {MAX_SHAPE_LOAD_DEPTH} at <{shape_iri}>"
        ));
    }

    let mut constraints = load_constraints(store, shapes_graph, shape_iri)?;
    let mut property_shapes = load_property_shapes_inner(store, shapes_graph, shape_iri)?;

    if let Some(own_path) = single_value(store, shapes_graph, shape_iri, &format!("{SH}path"))
        .and_then(|p| parse_property_path(store, shapes_graph, &p))
    {
        constraints.extend(
            property_shapes
                .drain(..)
                .map(|ps| Constraint::Property(Box::new(ps))),
        );
        property_shapes = vec![PropertyShape {
            iri: Some(shape_iri.to_string()),
            path: own_path,
            constraints: std::mem::take(&mut constraints),
            name: None,
            description: None,
            severity: None,
            message: None,
        }];
    }

    Ok(Shape {
        iri: shape_iri.to_string(),
        name: single_value(store, shapes_graph, shape_iri, &format!("{}name", SH)),
        shape_type: ShapeType::NodeShape,
        targets: vec![],
        constraints,
        property_shapes,
        severity: None,
        message: None,
        deactivated: false,
    })
}

fn load_property_shapes(
    store: &TripleStore,
    shapes_graph: &str,
    shape_iri: &str,
) -> Result<Vec<PropertyShape>, String> {
    load_property_shapes_inner(store, shapes_graph, shape_iri)
}

fn load_property_shapes_inner(
    store: &TripleStore,
    shapes_graph: &str,
    shape_iri: &str,
) -> Result<Vec<PropertyShape>, String> {
    // Bound recursion: nested `sh:property` chains load eagerly and could cycle
    // through named property shapes.
    let (_guard, within_limit) = LoadDepthGuard::enter();
    if !within_limit {
        return Err(format!(
            "{RECURSION_BOUND}: property shape exceeded max depth {MAX_SHAPE_LOAD_DEPTH} at <{shape_iri}>"
        ));
    }

    // Use the raw quad index so a blank-node parent (an inline `sh:node` /
    // `sh:qualifiedValueShape` body) can have its property shapes dereferenced.
    let ps_iris: Vec<String> = store
        .objects_for_subject_in_graph(shape_iri, &format!("{SH}property"), Some(shapes_graph))
        .iter()
        .map(term_to_lexical)
        .collect();

    let mut result = Vec::new();

    for ps_iri in &ps_iris {
        // Load and parse the property path: a predicate IRI, or a blank-node path
        // (sequence list, sh:inversePath, sh:alternativePath, sh:zeroOrMorePath, …).
        let path = match single_value(store, shapes_graph, ps_iri, &format!("{}path", SH)) {
            Some(p) => match parse_property_path(store, shapes_graph, &p) {
                Some(pp) => pp,
                None => {
                    warn!(
                        "Property shape <{}> has an unparseable sh:path, skipping",
                        ps_iri
                    );
                    continue;
                }
            },
            None => {
                warn!("Property shape <{}> has no sh:path, skipping", ps_iri);
                continue;
            }
        };

        // Load constraints on the property shape
        let mut constraints = load_constraints(store, shapes_graph, ps_iri)?;

        // Nested `sh:property` on a property shape: each value node along this
        // shape's path is validated against the nested property shape
        // (SHACL §2.1.3, see w3c property/property-001).
        if let Ok(nested) = load_property_shapes_inner(store, shapes_graph, ps_iri) {
            constraints.extend(
                nested
                    .into_iter()
                    .map(|ps| Constraint::Property(Box::new(ps))),
            );
        }

        let name = single_value(store, shapes_graph, ps_iri, &format!("{}name", SH));
        let description = single_value(store, shapes_graph, ps_iri, &format!("{}description", SH));
        let severity = single_value(store, shapes_graph, ps_iri, &format!("{}severity", SH));
        let message = single_value(store, shapes_graph, ps_iri, &format!("{}message", SH));

        result.push(PropertyShape {
            iri: Some(ps_iri.clone()),
            path,
            constraints,
            name,
            description,
            severity,
            message,
        });
    }

    // sh:qualifiedValueShapesDisjoint: a property shape's *sibling shapes* are the
    // qualified value shapes of the other property shapes that share its parent
    // (SHACL §4.5.4). Wire them now that every sibling is loaded.
    let sibling_qvs: Vec<Option<Shape>> = result
        .iter()
        .map(|ps| {
            ps.constraints.iter().find_map(|c| match c {
                Constraint::QualifiedValueShape { shape, .. } => Some((**shape).clone()),
                _ => None,
            })
        })
        .collect();
    for (i, ps) in result.iter_mut().enumerate() {
        for c in ps.constraints.iter_mut() {
            if let Constraint::QualifiedValueShape {
                disjoint: true,
                sibling_shapes,
                ..
            } = c
            {
                *sibling_shapes = sibling_qvs
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .filter_map(|(_, s)| s.clone())
                    .collect();
            }
        }
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Target resolution
// ---------------------------------------------------------------------------

fn resolve_targets(view: &DataView<'_>, shape: &Shape) -> Vec<Term> {
    let mut focus_nodes: Vec<Term> = Vec::new();

    for target in &shape.targets {
        match target {
            Target::TargetClass(class_iri) => {
                // All SHACL instances of the class across the dataset's data
                // graphs — including instances of subclasses
                // (rdf:type/rdfs:subClassOf*, SHACL §2.1.3.1). The type triple
                // is looked up per data graph; the subclass chain is read
                // across all of them, so this agrees with `sh:class` on what a
                // SHACL instance is (see `DataView::prepare`).
                for i in 0..view.graph_count() {
                    let set = match view.instances_of(class_iri, GraphSel::One(i)) {
                        Some(set) => set,
                        None => {
                            // Not prepared (a target added after `prepare`):
                            // compute it now from the same source, with the
                            // subclass chain read across every data graph as
                            // `prepare` does — see the note there.
                            let closure = view.subclass_closure(class_iri, GraphSel::All);
                            std::sync::Arc::new(
                                closure
                                    .iter()
                                    .flat_map(|c| match c {
                                        Term::NamedNode(_) | Term::BlankNode(_) => {
                                            view.step(c, RDF_TYPE, true, GraphSel::One(i))
                                        }
                                        _ => Vec::new(),
                                    })
                                    .collect::<std::collections::HashSet<Term>>(),
                            )
                        }
                    };
                    focus_nodes.extend(set.iter().cloned());
                }
            }
            Target::TargetNode(node) => {
                focus_nodes.push(node.clone());
            }
            Target::TargetSubjectsOf(pred_iri) => {
                focus_nodes.extend(view.subjects_of(pred_iri, GraphSel::All));
            }
            Target::TargetObjectsOf(pred_iri) => {
                focus_nodes.extend(view.objects_of(pred_iri, GraphSel::All));
            }
            Target::SparqlTarget(sparql) => {
                // SHACL-AF custom SPARQL target, scoped to the run's data
                // graphs. It used to run against the bare store: a `sh:target`
                // in any shapes graph the caller could write selected focus
                // nodes from EVERY graph in the store, other tenants' included,
                // and `sh:value` carried their terms back in the report. The
                // same `FROM <g>` prologue a `sh:sparql` constraint gets
                // confines it to the graphs this run may read; with no
                // `FROM NAMED`, a `GRAPH` block inside the target matches
                // nothing, exactly as for constraints.
                let scoped = super::constraints::prebind(sparql, &[], None, view.data_graphs);
                if let Ok(nodes) = execute_select_terms(view, &scoped, "this") {
                    focus_nodes.extend(nodes);
                }
            }
        }
    }

    // Deduplicate by term identity — the same node may arrive via multiple
    // targets, from several data graphs, or several times from one predicate
    // scan. A single target class or node over a single graph is already a
    // set (the instance set is a `HashSet`), so it skips the pass.
    let already_distinct = view.graph_count() == 1
        && matches!(
            shape.targets.as_slice(),
            [Target::TargetClass(_)] | [Target::TargetNode(_)]
        );
    if !already_distinct {
        dedup_terms(&mut focus_nodes);
    }
    focus_nodes
}

/// Drop repeated terms, keeping first occurrences in order, without cloning a
/// term: the membership set borrows the vector, and the keep-mask drives the
/// compaction afterwards.
fn dedup_terms(terms: &mut Vec<Term>) {
    let keep: Vec<bool> = {
        let mut seen: std::collections::HashSet<&Term> =
            std::collections::HashSet::with_capacity(terms.len());
        terms.iter().map(|t| seen.insert(t)).collect()
    };
    if keep.iter().all(|k| *k) {
        return;
    }
    let mut i = 0;
    terms.retain(|_| {
        let k = keep[i];
        i += 1;
        k
    });
}

// ---------------------------------------------------------------------------
// SHACL-AF rules
// ---------------------------------------------------------------------------

/// One term of a `sh:TripleRule`, kept as an RDF term rather than as text:
/// `sh:this` stands for the focus node (SHACL-AF §4.3), anything else is the
/// term the shapes graph gave.
///
/// It used to be a string, spliced into a generated `INSERT DATA { … }` with
/// the focus node pasted in as `<{focus}>` — and a focus node may be a
/// *literal* (`sh:targetNode "…"`), whose lexical form the shapes author
/// writes. A literal holding `> } } ; DROP GRAPH <…> ; INSERT DATA { GRAPH <g> { <x`
/// therefore closed the generated update and appended operations of its own.
#[derive(Debug, Clone)]
enum RuleTerm {
    /// `sh:this` — the focus node this run of the rule fires for.
    This,
    Fixed(Term),
}

/// A SHACL-AF rule's executable body.
enum RuleBody {
    /// `sh:SPARQLRule`: its `sh:construct`, parsed as the CONSTRUCT query
    /// SHACL-AF says it is, with its `sh:prefixes` prologue already in place.
    /// `binds_this` records whether the query mentions `$this`, since the
    /// evaluator refuses to substitute a variable the query never uses.
    Construct {
        query: Box<SpargebraQuery>,
        binds_this: bool,
    },
    /// `sh:TripleRule`: the `sh:subject` / `sh:predicate` / `sh:object` terms.
    Triple {
        subject: RuleTerm,
        predicate: RuleTerm,
        object: RuleTerm,
    },
}

/// A SHACL-AF rule ready to run: its shape's targets, the executable body,
/// `sh:order` (rules run in ascending order; default 0) and the `sh:condition`
/// shapes a focus node must conform to for the rule to fire.
struct Rule {
    shape_iri: String,
    targets: Vec<Target>,
    body: RuleBody,
    order: f64,
    conditions: Vec<Shape>,
}

/// `sh:order`, `sh:condition` and `sh:deactivated` of a rule node
/// (SHACL-AF §4.1–4.2). `Ok(None)` when the rule — or its shape — is
/// deactivated.
fn rule_modifiers(
    store: &TripleStore,
    shapes_graph: &str,
    shape_iri: &str,
    rule_node: &str,
) -> Result<Option<(f64, Vec<Shape>)>, String> {
    let deactivated = |node: &str| {
        single_value(store, shapes_graph, node, &format!("{SH}deactivated"))
            .is_some_and(|v| v == "true" || v == "1")
    };
    if deactivated(rule_node) || deactivated(shape_iri) {
        return Ok(None);
    }
    let order = single_value(store, shapes_graph, rule_node, &format!("{SH}order"))
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    let mut conditions = Vec::new();
    for cond in store
        .objects_for_subject_in_graph(rule_node, &format!("{SH}condition"), Some(shapes_graph))
        .iter()
        .map(term_to_lexical)
    {
        conditions.push(load_inline_shape(store, shapes_graph, &cond).map_err(|e| {
            format!("rule of shape <{shape_iri}>: sh:condition <{cond}> cannot be loaded: {e}")
        })?);
    }
    Ok(Some((order, conditions)))
}

fn load_rules(store: &TripleStore, shapes_graph: &str) -> Result<Vec<Rule>, String> {
    let mut rules: Vec<Rule> = Vec::new();

    // SPARQL rules. Discover shapes carrying a CONSTRUCT rule, then resolve the rule
    // node and its sh:prefixes through the raw quad index — the rule node (`sh:rule
    // [ … ]`) is typically blank, and the prefixes prologue must be prepended so a
    // prefixed CONSTRUCT body parses instead of being silently dropped.
    let sparql_rule_shapes = execute_select_single(
        store,
        &format!(
            r#"
            PREFIX sh: <{SH}>
            SELECT DISTINCT ?shape WHERE {{
                GRAPH <{shapes_graph}> {{ ?shape sh:rule ?rule . ?rule sh:construct ?c . }}
            }}
            "#,
        ),
        "shape",
    )?;
    for shape_iri in &sparql_rule_shapes {
        let targets = load_targets(store, shapes_graph, shape_iri).unwrap_or_default();
        for rule_node in store
            .objects_for_subject_in_graph(shape_iri, &format!("{SH}rule"), Some(shapes_graph))
            .iter()
            .map(term_to_lexical)
        {
            if let Some(construct) =
                single_value(store, shapes_graph, &rule_node, &format!("{SH}construct"))
            {
                let Some((order, conditions)) =
                    rule_modifiers(store, shapes_graph, shape_iri, &rule_node)?
                else {
                    continue;
                };
                let prefixes = sparql_prefixes(store, shapes_graph, &rule_node);
                let text = format!("{prefixes}{construct}");
                // Parsed here, once, so a rule body that is not a CONSTRUCT
                // query stops the run at load time with a message naming its
                // shape — rather than reaching the evaluator per focus node.
                let query = parse_construct_rule(&text)
                    .map_err(|e| format!("rule of shape <{shape_iri}>: {e}"))?;
                rules.push(Rule {
                    shape_iri: shape_iri.clone(),
                    targets: targets.clone(),
                    body: RuleBody::Construct {
                        binds_this: super::constraints::mentions_variable(&text, "this"),
                        query: Box::new(query),
                    },
                    order,
                    conditions,
                });
            }
        }
    }

    // Triple rules
    let query = format!(
        r#"
        PREFIX sh: <{SH}>
        SELECT ?shape ?rule ?subject ?predicate ?object WHERE {{
            GRAPH <{shapes_graph}> {{
                ?shape sh:rule ?rule .
                ?rule sh:subject ?subject ;
                      sh:predicate ?predicate ;
                      sh:object ?object .
            }}
        }}
        "#,
    );

    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&query) {
        for solution in solutions.filter_map(|s| s.ok()) {
            let shape_iri = match solution.get("shape") {
                Some(oxigraph::model::Term::NamedNode(nn)) => nn.as_str().to_string(),
                _ => continue,
            };
            let rule_node = match solution.get("rule") {
                Some(t) => term_to_lexical(t),
                None => continue,
            };
            let Some((order, conditions)) =
                rule_modifiers(store, shapes_graph, &shape_iri, &rule_node)?
            else {
                continue;
            };
            let (Some(subject), Some(predicate), Some(object)) = (
                triple_rule_term(solution.get("subject")),
                triple_rule_term(solution.get("predicate")),
                triple_rule_term(solution.get("object")),
            ) else {
                continue;
            };

            let targets = load_targets(store, shapes_graph, &shape_iri).unwrap_or_default();
            rules.push(Rule {
                shape_iri,
                targets,
                body: RuleBody::Triple {
                    subject,
                    predicate,
                    object,
                },
                order,
                conditions,
            });
        }
    }

    // sh:order: ascending, ties in discovery order (SHACL-AF §4.2).
    rules.sort_by(|a, b| a.order.total_cmp(&b.order));
    Ok(rules)
}

/// Apply one rule to one focus node: evaluate it read-only over `data_graphs`,
/// and materialise what it derives into `target_graph` (the unnamed default
/// graph when there is none). The number of *new* triples is not measured here —
/// `infer` tracks it via the store's count delta per round (see there), so a
/// rule whose output already exists costs nothing and the fixed point is exact.
///
/// A rule's reach is the caller's, not the store's. Deriving triples is a read
/// plus an insert the engine performs itself — never an UPDATE the rule text
/// gets to write. That is the whole security boundary of SHACL-AF inference:
/// the body is data that any writer of a dataset can upload, and it used to be
/// handed to `TripleStore::update`, which authorizes nothing.
fn apply_rule(
    store: &TripleStore,
    focus_node: &Term,
    body: &RuleBody,
    data_graphs: &[String],
    target_graph: Option<&str>,
) -> Result<(), String> {
    let triples = match body {
        RuleBody::Construct { query, binds_this } => {
            // The focus node is bound as a *term*, never pasted into the query
            // text, and the query reads `data_graphs` and nothing else.
            let bindings: Vec<(&str, Term)> = if *binds_this {
                vec![("this", focus_node.clone())]
            } else {
                Vec::new()
            };
            store
                .construct_confined(query, data_graphs, &bindings)
                // An erroring rule used to be logged and swallowed, so `infer`
                // reported success with 0 inferred triples whether the rules ran
                // or every one of them failed. Surface it: the caller decides.
                .map_err(|e| format!("SHACL rule could not be evaluated: {e}"))?
        }
        RuleBody::Triple {
            subject,
            predicate,
            object,
        } => triple_rule_output(subject, predicate, object, focus_node)
            .into_iter()
            .collect(),
    };
    if triples.is_empty() {
        return Ok(());
    }

    let graph = match target_graph {
        Some(g) => GraphName::NamedNode(
            NamedNode::new(g).map_err(|e| format!("inference target graph <{g}>: {e}"))?,
        ),
        None => GraphName::DefaultGraph,
    };
    let quads: Vec<Quad> = triples
        .into_iter()
        .map(|t| Quad::new(t.subject, t.predicate, t.object, graph.clone()))
        .collect();
    store
        .insert_quads(quads)
        .map_err(|e| format!("SHACL rule output could not be materialised: {e}"))
}

/// The triple a `sh:TripleRule` derives for one focus node, or `None` when the
/// terms do not form one — `sh:subject sh:this` with a literal focus node, say,
/// since RDF has no literal subjects. The rule simply does not fire there.
fn triple_rule_output(
    subject: &RuleTerm,
    predicate: &RuleTerm,
    object: &RuleTerm,
    focus_node: &Term,
) -> Option<Triple> {
    let resolve = |t: &RuleTerm| match t {
        RuleTerm::This => focus_node.clone(),
        RuleTerm::Fixed(term) => term.clone(),
    };
    let subject = NamedOrBlankNode::try_from(resolve(subject)).ok()?;
    let Term::NamedNode(predicate) = resolve(predicate) else {
        return None;
    };
    Some(Triple::new(subject, predicate, resolve(object)))
}

/// Parse a `sh:construct` rule body as the CONSTRUCT query SHACL-AF says it is.
///
/// SHACL-AF's `sh:construct` carries `CONSTRUCT { template } WHERE { pattern }`.
/// This store has always also accepted the convenience form
/// `INSERT { … } WHERE { … }`, which is the same query under a different
/// keyword, so the leading keyword is rewritten and the result must still parse
/// as a CONSTRUCT query. A `PREFIX`/`BASE` prologue is skipped first, so an
/// `insert` substring inside a prefix IRI is never mistaken for the keyword.
///
/// **Everything else is refused**, and that is the point. The body used to be
/// rewritten into a SPARQL UPDATE textually — anything that was not a leading
/// `CONSTRUCT` was passed through verbatim — and then executed with the store's
/// own authority. `DROP ALL` was a rule. So was
/// `INSERT { GRAPH <anywhere> { ?s ?p ?o } } WHERE { GRAPH <anywhere> { ?s ?p ?o } }`,
/// which the `WITH <g>` prefix added for a single-graph run did nothing to
/// confine. A CONSTRUCT query cannot write, cannot drop, and cannot name a
/// destination graph: its template has no `GRAPH` clause in the grammar.
fn parse_construct_rule(body: &str) -> Result<SpargebraQuery, String> {
    let head_len = prologue_len(body);
    let rest = &body[head_len..];
    let token = leading_token(rest);
    let text = if token.eq_ignore_ascii_case("insert") {
        format!("{}CONSTRUCT{}", &body[..head_len], &rest[token.len()..])
    } else {
        body.to_string()
    };
    let query = SparqlParser::new()
        .parse_query(&text)
        .map_err(|e| format!("sh:construct must be a CONSTRUCT query ({e})"))?;
    if !matches!(query, SpargebraQuery::Construct { .. }) {
        return Err(
            "sh:construct must be a CONSTRUCT query (`CONSTRUCT { … } WHERE { … }`, or the \
             equivalent `INSERT { … } WHERE { … }` form), not another query or an update"
                .to_string(),
        );
    }
    Ok(query)
}

/// Byte length of the leading `PREFIX`/`BASE` prologue of a SPARQL body,
/// trailing whitespace included, so `body[..len]` is the prologue and
/// `body[len..]` starts at the first operation keyword. Zero when there is no
/// prologue or a declaration is unterminated (the parser reports that).
fn prologue_len(body: &str) -> usize {
    let mut rest = body.trim_start();
    loop {
        let token = leading_token(rest);
        if !(token.eq_ignore_ascii_case("prefix") || token.eq_ignore_ascii_case("base")) {
            return body.len() - rest.len();
        }
        match rest.find('>') {
            Some(gt) => rest = rest[gt + 1..].trim_start(),
            None => return 0,
        }
    }
}

/// The first keyword-like token of `s` (up to whitespace, `<` or `{`).
fn leading_token(s: &str) -> &str {
    s.split(|c: char| c.is_whitespace() || c == '<' || c == '{')
        .next()
        .unwrap_or("")
}

/// A triple-rule term, mapping `sh:this` to the focus-node placeholder so
/// `apply_rule` resolves it per focus node (SHACL-AF §4.3 — `sh:this` denotes the
/// focus node, not the literal `sh:this` IRI).
fn triple_rule_term(term: Option<&Term>) -> Option<RuleTerm> {
    match term? {
        Term::NamedNode(nn) if nn.as_str() == "http://www.w3.org/ns/shacl#this" => {
            Some(RuleTerm::This)
        }
        other => Some(RuleTerm::Fixed(other.clone())),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn execute_select_single(
    store: &TripleStore,
    query: &str,
    var: &str,
) -> Result<Vec<String>, String> {
    match store.query(query) {
        Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) => {
            let result: Vec<String> = solutions
                .filter_map(|s| s.ok())
                .filter_map(|s| {
                    s.get(var).map(|v| match v {
                        oxigraph::model::Term::NamedNode(nn) => nn.as_str().to_string(),
                        oxigraph::model::Term::Literal(lit) => lit.value().to_string(),
                        oxigraph::model::Term::BlankNode(bn) => format!("_:{}", bn.as_str()),
                        #[cfg(feature = "rdf-12")]
                        oxigraph::model::Term::Triple(t) => t.to_string(),
                    })
                })
                .collect();
            Ok(result)
        }
        Ok(_) => Ok(Vec::new()),
        Err(e) => Err(format!("Query error: {}", e)),
    }
}

/// Like [`execute_select_single`], but keeps the full typed terms so target
/// resolution carries term kind, datatype and language into constraint
/// evaluation.
/// A SHACL-AF SPARQL target (`sh:target [ sh:select … ]`) must be a SELECT
/// that projects `?this` (SHACL-AF §2.1.2); anything else produces no focus
/// nodes and would let the shape pass over nothing.
fn check_sparql_target(query: &str) -> Result<(), String> {
    use opengraph::spargebra::algebra::GraphPattern;
    use opengraph::spargebra::{Query, SparqlParser};
    let parsed = SparqlParser::new()
        .parse_query(query)
        .map_err(|e| format!("sh:select does not parse: {e}"))?;
    let Query::Select { pattern, .. } = parsed else {
        return Err("sh:select must be a SELECT query".to_string());
    };
    // Walk the solution-modifier wrappers down to the projection.
    let mut p = &pattern;
    loop {
        match p {
            GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. }
            | GraphPattern::OrderBy { inner, .. } => p = inner,
            GraphPattern::Project { variables, .. } => {
                if variables.iter().any(|v| v.as_str() == "this") {
                    return Ok(());
                }
                return Err("sh:select must project ?this".to_string());
            }
            _ => return Ok(()),
        }
    }
}

/// Run a SELECT against the run's own data source and collect one variable.
fn execute_select_terms(view: &DataView<'_>, query: &str, var: &str) -> Result<Vec<Term>, String> {
    match view.query(query) {
        Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) => Ok(solutions
            .filter_map(|s| s.ok())
            .filter_map(|s| s.get(var).cloned())
            .collect()),
        Ok(_) => Ok(Vec::new()),
        Err(e) => Err(format!("Query error: {}", e)),
    }
}

fn single_value(
    store: &TripleStore,
    shapes_graph: &str,
    subject: &str,
    predicate: &str,
) -> Option<String> {
    // Resolve through the raw quad index so blank-node subjects are dereferenced
    // correctly. The standard SHACL idiom uses blank nodes for property shapes
    // (`sh:property [ … ]`), inline `sh:node`/`sh:qualifiedValueShape`/`sh:not`
    // shapes, and SPARQL-constraint nodes; SPARQL surface syntax cannot re-address
    // a stored blank node via `<_:bn>`, so the old query-based form silently
    // matched nothing and left those constraints unenforced.
    store
        .objects_for_subject_in_graph(subject, predicate, Some(shapes_graph))
        .into_iter()
        .next()
        .map(|t| term_to_lexical(&t))
}

fn multi_values(
    store: &TripleStore,
    shapes_graph: &str,
    subject: &str,
    predicate: &str,
) -> Vec<String> {
    store
        .objects_for_subject_in_graph(subject, predicate, Some(shapes_graph))
        .iter()
        .map(term_to_lexical)
        .collect()
}

/// Parse a SHACL property path (SHACL §2.3) starting at `node` into a [`PropertyPath`].
///
/// Handles a predicate IRI; an RDF-list **sequence** path `( p1 p2 … )`; and the blank-node
/// path operators `sh:inversePath`, `sh:alternativePath` (an RDF list), `sh:zeroOrMorePath`,
/// `sh:oneOrMorePath`, `sh:zeroOrOnePath`. Blank-node cells are walked through the raw quad
/// index (SPARQL surface syntax cannot re-address them). Returns `None` for an empty or
/// malformed path so the caller can skip the property shape rather than mis-bind it.
///
/// A node carrying BOTH list cells (`rdf:first`/`rdf:rest`) and a path operator is
/// interpreted as the sequence path — matching the W3C suite's `path-strange-*`
/// expectations, which treat the list reading as authoritative.
fn parse_property_path(
    store: &TripleStore,
    shapes_graph: &str,
    node: &str,
) -> Option<PropertyPath> {
    // A predicate path is a plain IRI.
    if !node.starts_with("_:") {
        return Some(PropertyPath::Predicate(node.to_string()));
    }
    // Blank node: an RDF-list sequence path takes precedence over operators.
    let seq: Vec<PropertyPath> = rdf_list_elements(store, shapes_graph, node)
        .iter()
        .filter_map(|e| parse_property_path(store, shapes_graph, e))
        .collect();
    if !seq.is_empty() {
        return Some(PropertyPath::Sequence(seq));
    }
    let op = |p: &str| -> Option<String> {
        store
            .objects_for_subject_in_graph(node, &format!("{SH}{p}"), Some(shapes_graph))
            .first()
            .map(term_to_lexical)
    };
    if let Some(inner) = op("inversePath") {
        return parse_property_path(store, shapes_graph, &inner)
            .map(|p| PropertyPath::Inverse(Box::new(p)));
    }
    if let Some(head) = op("alternativePath") {
        let parts: Vec<PropertyPath> = rdf_list_elements(store, shapes_graph, &head)
            .iter()
            .filter_map(|e| parse_property_path(store, shapes_graph, e))
            .collect();
        return (!parts.is_empty()).then_some(PropertyPath::Alternative(parts));
    }
    if let Some(inner) = op("zeroOrMorePath") {
        return parse_property_path(store, shapes_graph, &inner)
            .map(|p| PropertyPath::ZeroOrMore(Box::new(p)));
    }
    if let Some(inner) = op("oneOrMorePath") {
        return parse_property_path(store, shapes_graph, &inner)
            .map(|p| PropertyPath::OneOrMore(Box::new(p)));
    }
    if let Some(inner) = op("zeroOrOnePath") {
        return parse_property_path(store, shapes_graph, &inner)
            .map(|p| PropertyPath::ZeroOrOne(Box::new(p)));
    }
    None
}

/// Walk the RDF list whose head is `head`, returning each member's lexical node form
/// (IRI, `_:label`, or literal value) via the raw quad index. Empty if `head` is not a list.
fn rdf_list_elements(store: &TripleStore, shapes_graph: &str, head: &str) -> Vec<String> {
    const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
    const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
    const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
    let mut out = Vec::new();
    let mut current = head.to_string();
    for _ in 0..10_000 {
        if current == RDF_NIL {
            break;
        }
        match store
            .objects_for_subject_in_graph(&current, RDF_FIRST, Some(shapes_graph))
            .first()
        {
            Some(first) => out.push(term_to_lexical(first)),
            None => break,
        }
        match store
            .objects_for_subject_in_graph(&current, RDF_REST, Some(shapes_graph))
            .first()
        {
            Some(rest) => current = term_to_lexical(rest),
            None => break,
        }
    }
    out
}

/// Build the SPARQL `PREFIX` prologue declared via SHACL's prefixes mechanism for a
/// constraint / rule / target `node`: `node sh:prefixes ?owner`, `?owner sh:declare
/// [ sh:prefix "p" ; sh:namespace "ns"^^xsd:anyURI ]`. Returns `""` when none are
/// declared. The declaration nodes are typically blank, so they are resolved through
/// the raw quad index (SPARQL surface syntax cannot re-address a stored blank node).
///
/// Without this prologue a SHACL-SPARQL body that uses prefixed names (`da:`, `geo:`,
/// `geof:` …) fails to parse, and the `if let Ok(..)` guards in evaluation silently
/// drop the whole constraint/rule/target — see SHACL-SPARQL §5.2 (prefixes mechanism).
/// The `PREFIX` prologue of a SPARQL-based constraint, rule or validator:
/// the `sh:declare` declarations of every `sh:prefixes` value and,
/// transitively, of the ontologies it `owl:imports` (SHACL §5.2.1), as far
/// as they are in the shapes graph.
fn sparql_prefixes(store: &TripleStore, shapes_graph: &str, node: &str) -> String {
    let mut out = String::new();
    let mut owners: Vec<String> = store
        .objects_for_subject_in_graph(node, &format!("{SH}prefixes"), Some(shapes_graph))
        .iter()
        .map(term_to_lexical)
        .collect();
    let mut i = 0;
    while i < owners.len() {
        let owner = owners[i].clone();
        i += 1;
        for imported in store
            .objects_for_subject_in_graph(
                &owner,
                "http://www.w3.org/2002/07/owl#imports",
                Some(shapes_graph),
            )
            .iter()
            .map(term_to_lexical)
        {
            if owners.len() < 64 && !owners.contains(&imported) {
                owners.push(imported);
            }
        }
        for decl in store
            .objects_for_subject_in_graph(&owner, &format!("{SH}declare"), Some(shapes_graph))
            .iter()
            .map(term_to_lexical)
        {
            let prefix = single_value(store, shapes_graph, &decl, &format!("{SH}prefix"));
            let namespace = single_value(store, shapes_graph, &decl, &format!("{SH}namespace"));
            if let (Some(p), Some(ns)) = (prefix, namespace) {
                out.push_str(&format!("PREFIX {p}: <{ns}>\n"));
            }
        }
    }
    out
}

fn ask(store: &TripleStore, query: &str) -> bool {
    matches!(
        store.query(query),
        Ok(oxigraph::sparql::QueryResults::Boolean(true))
    )
}

fn load_rdf_list(
    store: &TripleStore,
    shapes_graph: &str,
    subject: &str,
    predicate: &str,
) -> Vec<String> {
    load_rdf_list_terms(store, shapes_graph, subject, predicate)
        .iter()
        .map(term_to_lexical)
        .collect()
}

/// Walk the RDF list reached from `subject` via `predicate`, keeping each
/// member's *typed* term (sh:in members may be typed literals). In standard
/// Turtle `( … )` syntax the list cells are blank nodes, which SPARQL surface
/// syntax cannot re-address (`_:x` in a query is a fresh existential), so cells
/// are resolved through the raw quad index.
fn load_rdf_list_terms(
    store: &TripleStore,
    shapes_graph: &str,
    subject: &str,
    predicate: &str,
) -> Vec<Term> {
    const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
    const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
    const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";

    let mut values = Vec::new();

    let mut current = match store
        .objects_for_subject_in_graph(subject, predicate, Some(shapes_graph))
        .into_iter()
        .next()
    {
        Some(h) => term_to_lexical(&h),
        None => return values,
    };

    for _ in 0..10_000 {
        if current == RDF_NIL {
            break;
        }
        if let Some(first) = store
            .objects_for_subject_in_graph(&current, RDF_FIRST, Some(shapes_graph))
            .into_iter()
            .next()
        {
            values.push(first);
        }
        match store
            .objects_for_subject_in_graph(&current, RDF_REST, Some(shapes_graph))
            .into_iter()
            .next()
        {
            Some(rest) => current = term_to_lexical(&rest),
            None => break,
        }
    }

    values
}

/// Lexical form of a term matching [`execute_select_single`]'s convention:
/// bare IRI for named nodes, lexical value for literals, `_:label` for blank
/// nodes. Used both for list member values and to re-address the next cell.
fn term_to_lexical(term: &oxigraph::model::Term) -> String {
    match term {
        oxigraph::model::Term::NamedNode(nn) => nn.as_str().to_string(),
        oxigraph::model::Term::Literal(lit) => lit.value().to_string(),
        oxigraph::model::Term::BlankNode(bn) => format!("_:{}", bn.as_str()),
        #[cfg(feature = "rdf-12")]
        oxigraph::model::Term::Triple(t) => t.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;

    const SHAPES: &str = r#"
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix ex: <http://example.org/> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        ex:PersonShape a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:property ex:NameProp .
        ex:NameProp sh:path ex:name ; sh:minCount 1 ; sh:datatype xsd:string .
    "#;

    // Shapes live in a named graph (the engine loads them via `GRAPH <shapes>`).
    // Data is loaded into the *default* graph and validated with an empty
    // `data_graphs` list: constraint value lookups query the default graph, so
    // this is the configuration the engine evaluates correctly.
    fn store_with(shapes: &str, data: &str) -> TripleStore {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(shapes, RdfFormat::Turtle, Some("urn:shapes"))
            .unwrap();
        store.load_str(data, RdfFormat::Turtle, None).unwrap();
        store
    }

    #[test]
    fn min_count_violation_is_reported() {
        // bob has no ex:name → violates sh:minCount 1; alice conforms.
        let data = r#"
            @prefix ex: <http://example.org/> .
            ex:alice a ex:Person ; ex:name "Alice" .
            ex:bob a ex:Person .
        "#;
        let store = store_with(SHAPES, data);

        let report = validate(&store, "urn:shapes", &[]).unwrap();

        assert!(!report.conforms, "expected non-conformance for bob");
        assert!(report.results_count >= 1, "expected at least one result");
        assert!(
            report
                .results
                .iter()
                .any(|r| matches!(r.severity, Severity::Violation)),
            "expected a Violation-severity result",
        );
        assert!(
            report.results.iter().any(|r| r.focus_node.contains("bob")),
            "violation should name bob as the focus node",
        );
    }

    #[test]
    fn min_count_violation_in_named_data_graph() {
        // Regression: data in a *named* graph, validated with a non-empty
        // data_graphs list — the configuration the dataset-level validate
        // endpoint always uses. Target resolution previously emitted
        // `GRAPH <g> ?s a <C>` (missing braces) → invalid SPARQL → zero focus
        // nodes → a false "conforms". With graph_scoped this must now find bob.
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(SHAPES, RdfFormat::Turtle, Some("urn:shapes"))
            .unwrap();
        let data = r#"
            @prefix ex: <http://example.org/> .
            ex:alice a ex:Person ; ex:name "Alice" .
            ex:bob a ex:Person .
        "#;
        store
            .load_str(data, RdfFormat::Turtle, Some("urn:data"))
            .unwrap();

        let report = validate(&store, "urn:shapes", &["urn:data".to_string()]).unwrap();

        assert!(
            !report.conforms,
            "expected non-conformance for bob in a named graph"
        );
        assert!(
            report.results.iter().any(|r| r.focus_node.contains("bob")),
            "expected a violation naming bob, got {:?}",
            report.results,
        );
    }

    #[test]
    fn fully_valid_data_conforms() {
        let data = r#"
            @prefix ex: <http://example.org/> .
            ex:alice a ex:Person ; ex:name "Alice" .
            ex:carol a ex:Person ; ex:name "Carol" .
        "#;
        let store = store_with(SHAPES, data);

        let report = validate(&store, "urn:shapes", &[]).unwrap();

        assert!(
            report.conforms,
            "expected conformance, got {:?}",
            report.results
        );
        assert_eq!(report.results_count, 0);
    }

    // Regression: `sh:in ( … )` builds an RDF list whose cells are blank nodes.
    // The list walker previously addressed cells via `<_:bn>` SPARQL
    // interpolation (a fresh existential that matched nothing), so the allowed
    // set was always empty and the constraint silently never fired. Cells are
    // now resolved through the raw quad index.
    #[test]
    fn in_constraint_walks_blank_node_list_and_flags_disallowed_value() {
        let shapes = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            ex:ColorShape a sh:NodeShape ;
                sh:targetClass ex:Widget ;
                sh:property ex:ColorProp .
            ex:ColorProp sh:path ex:color ; sh:in ( "red" "green" "blue" ) .
        "#;
        let data = r#"
            @prefix ex: <http://example.org/> .
            ex:ok a ex:Widget ; ex:color "green" .
            ex:bad a ex:Widget ; ex:color "purple" .
        "#;
        let store = store_with(shapes, data);

        let report = validate(&store, "urn:shapes", &[]).unwrap();

        assert!(!report.conforms, "purple is not in the allowed set");
        assert!(
            report.results.iter().any(|r| r.focus_node.contains("bad")),
            "expected an sh:in violation naming ex:bad, got {:?}",
            report.results,
        );
        assert!(
            !report.results.iter().any(|r| r.focus_node.contains("ok")),
            "ex:ok has an allowed color and must not be flagged, got {:?}",
            report.results,
        );
    }

    // Regression: sh:nodeKind on a (named) property shape previously only fired
    // when applied at node level (path.is_none()); at property level it was a
    // no-op. A literal value where sh:nodeKind sh:IRI is required must now be
    // flagged.
    #[test]
    fn node_kind_iri_on_property_flags_literal_value() {
        let shapes = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            ex:KnowsShape a sh:NodeShape ;
                sh:targetClass ex:Person ;
                sh:property ex:KnowsProp .
            ex:KnowsProp sh:path ex:knows ; sh:nodeKind sh:IRI .
        "#;
        let data = r#"
            @prefix ex: <http://example.org/> .
            ex:alice a ex:Person ; ex:knows ex:bob .
            ex:carol a ex:Person ; ex:knows "not-an-iri" .
        "#;
        let store = store_with(shapes, data);

        let report = validate(&store, "urn:shapes", &[]).unwrap();

        assert!(
            !report.conforms,
            "a literal value violates sh:nodeKind sh:IRI"
        );
        assert!(
            report
                .results
                .iter()
                .any(|r| r.focus_node.contains("carol")),
            "expected a nodeKind violation naming ex:carol, got {:?}",
            report.results,
        );
        assert!(
            !report
                .results
                .iter()
                .any(|r| r.focus_node.contains("alice")),
            "ex:alice points at an IRI and must not be flagged, got {:?}",
            report.results,
        );
    }

    // Regression: focus nodes were carried as bare lexical strings, so a string
    // literal reached via sh:targetObjectsOf whose lexical form is scheme-shaped
    // ("mailto:info@example.org") was misclassified as an IRI by node-level
    // sh:nodeKind — wrongly passing sh:IRI and wrongly violating sh:Literal.
    // Focus nodes are typed terms end-to-end now.
    const MAILTO_DATA: &str = r#"
        @prefix ex: <http://example.org/> .
        ex:x ex:p "mailto:info@example.org" .
    "#;

    #[test]
    fn literal_focus_via_target_objects_of_conforms_to_node_kind_literal() {
        let shapes = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            ex:MailShape a sh:NodeShape ;
                sh:targetObjectsOf ex:p ;
                sh:nodeKind sh:Literal .
        "#;
        let store = store_with(shapes, MAILTO_DATA);

        let report = validate(&store, "urn:shapes", &[]).unwrap();

        assert!(
            report.conforms,
            "a literal object must satisfy sh:nodeKind sh:Literal, got {:?}",
            report.results,
        );
    }

    #[test]
    fn literal_focus_via_target_objects_of_violates_node_kind_iri() {
        let shapes = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            ex:MailShape a sh:NodeShape ;
                sh:targetObjectsOf ex:p ;
                sh:nodeKind sh:IRI .
        "#;
        let store = store_with(shapes, MAILTO_DATA);

        let report = validate(&store, "urn:shapes", &[]).unwrap();

        assert!(
            !report.conforms,
            "a literal object must violate sh:nodeKind sh:IRI"
        );
        assert!(
            report
                .results
                .iter()
                .any(|r| r.focus_node == "mailto:info@example.org"),
            "violation should name the literal focus node, got {:?}",
            report.results,
        );
    }

    // Typed focus nodes: node-level value constraints (range, datatype) must
    // evaluate against the focus literal's datatype — `sh:targetNode 7` with
    // `sh:minInclusive 8` is a violation, `9` conforms.
    #[test]
    fn node_level_range_constraint_on_literal_targets() {
        let shapes = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            ex:RangeShape a sh:NodeShape ;
                sh:minInclusive 8 ;
                sh:targetNode 7 ;
                sh:targetNode 9 .
        "#;
        let store = store_with(shapes, "");

        let report = validate(&store, "urn:shapes", &[]).unwrap();

        assert!(!report.conforms, "7 < 8 must violate sh:minInclusive");
        assert_eq!(
            report.results_count, 1,
            "only 7 violates: {:?}",
            report.results
        );
        assert_eq!(report.results[0].focus_node, "7");
    }

    /// `+` on a cycle yields the focus node itself, as SPARQL's `<s> <p>+ ?v`
    /// does — for an IRI focus and for a blank-node focus alike. The old native
    /// walk marked the start visited up front and never emitted it.
    #[test]
    fn one_or_more_path_on_cycle_includes_focus() {
        let shapes = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            ex:S a sh:NodeShape ; sh:targetClass ex:T ;
                sh:property [ sh:path [ sh:oneOrMorePath ex:p ] ; sh:minCount 2 ] .
        "#;
        // a -> b -> a: from a, `p+` reaches b and (via the cycle) a: 2 values.
        // c -> c: from c, `p+` reaches c only: 1 value → violation.
        // The blank node _:x -> _:y -> _:x mirrors the IRI case.
        let data = r#"
            @prefix ex: <http://example.org/> .
            ex:a a ex:T ; ex:p ex:b . ex:b ex:p ex:a .
            ex:c a ex:T ; ex:p ex:c .
            _:x a ex:T ; ex:p _:y . _:y ex:p _:x .
        "#;
        let store = store_with(shapes, data);
        let report = validate(&store, "urn:shapes", &[]).unwrap();
        let focus: Vec<&str> = report
            .results
            .iter()
            .map(|r| r.focus_node.as_str())
            .collect();
        assert_eq!(focus, vec!["http://example.org/c"], "{:?}", report.results);
    }

    /// `sh:class` and `sh:targetClass` agree on instances typed through an
    /// anonymous subclass (`ex:x a [ rdfs:subClassOf ex:C ]`): both follow
    /// `rdf:type/rdfs:subClassOf*`, blank-node classes included (SHACL §2.1.3.1).
    #[test]
    fn anonymous_subclasses_count_for_targets_and_sh_class() {
        let shapes = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            ex:S a sh:NodeShape ; sh:targetClass ex:C ;
                sh:property [ sh:path ex:friend ; sh:class ex:C ; sh:minCount 1 ] .
        "#;
        let data = r#"
            @prefix ex: <http://example.org/> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            ex:x a [ rdfs:subClassOf ex:C ] ; ex:friend ex:y .
            ex:y a ex:C ; ex:friend ex:x .
            ex:z a ex:C ; ex:friend ex:stranger .
        "#;
        let store = store_with(shapes, data);
        let report = validate(&store, "urn:shapes", &[]).unwrap();
        // x is targeted (anonymous subclass) and its friend y conforms; y's
        // friend x conforms via the anonymous subclass; z's friend is untyped.
        let focus: Vec<&str> = report
            .results
            .iter()
            .map(|r| r.focus_node.as_str())
            .collect();
        assert_eq!(focus, vec!["http://example.org/z"], "{:?}", report.results);
    }

    /// One data graph named by an invalid IRI no longer voids the whole run: the
    /// valid graphs are still validated (the UNION query this used to build
    /// failed to parse and yielded zero targets and values everywhere).
    #[test]
    fn invalid_data_graph_iri_skips_only_that_graph() {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(SHAPES, RdfFormat::Turtle, Some("urn:shapes"))
            .unwrap();
        store
            .load_str(
                r#"@prefix ex: <http://example.org/> . ex:bob a ex:Person ."#,
                RdfFormat::Turtle,
                Some("http://example.org/data"),
            )
            .unwrap();
        let graphs = vec![
            "http://example.org/data".to_string(),
            "not an iri".to_string(),
        ];
        let report = validate(&store, "urn:shapes", &graphs).unwrap();
        assert!(!report.conforms);
        assert!(report.results.iter().any(|r| r.focus_node.contains("bob")));
    }

    /// A run over the accelerator's clean RAM copy reports exactly what a run
    /// over the live store reports, and a write after it is seen by the next
    /// run (the copy is a peek at the clean state, never a stale snapshot).
    #[test]
    fn mirror_backed_run_matches_live_run_and_refreshes_after_write() {
        let store = TripleStore::in_memory()
            .unwrap()
            .with_parallel_rebuild_quiet_ms(0);
        store
            .load_str(SHAPES, RdfFormat::Turtle, Some("urn:shapes"))
            .unwrap();
        let g1 = "http://example.org/g1";
        let g2 = "http://example.org/g2";
        store
            .load_str(
                r#"@prefix ex: <http://example.org/> . ex:alice a ex:Person ; ex:name "Alice" . ex:bob a ex:Person ."#,
                RdfFormat::Turtle,
                Some(g1),
            )
            .unwrap();
        store
            .load_str(
                r#"@prefix ex: <http://example.org/> . ex:carol a ex:Person ."#,
                RdfFormat::Turtle,
                Some(g2),
            )
            .unwrap();
        let graphs = vec![g1.to_string(), g2.to_string()];
        let summary = |r: &ValidationReport| {
            let mut v: Vec<String> = r
                .results
                .iter()
                .map(|x| format!("{} {} {:?}", x.focus_node, x.source_constraint, x.path))
                .collect();
            v.sort();
            (r.conforms, v)
        };
        // Live path: the mirror is dirty right after the loads.
        assert!(store.mirror_full_copy().is_none());
        let live = validate(&store, "urn:shapes", &graphs).unwrap();
        assert_eq!(live.results_count, 2, "{:?}", live.results);
        // Build the mirror and run again on its copy.
        store.accelerator_tick();
        assert_eq!(store.parallel_build_count(), 1);
        assert!(store.mirror_full_copy().is_some(), "clean copy published");
        let mirrored = validate(&store, "urn:shapes", &graphs).unwrap();
        assert_eq!(summary(&mirrored), summary(&live));
        // A write lands: the next run must see it, not the old copy.
        store
            .load_str(
                r#"@prefix ex: <http://example.org/> . ex:carol ex:name "Carol" ."#,
                RdfFormat::Turtle,
                Some(g2),
            )
            .unwrap();
        let after = validate(&store, "urn:shapes", &graphs).unwrap();
        assert_eq!(after.results_count, 1, "{:?}", after.results);
        assert!(after.results[0].focus_node.contains("bob"));
    }

    /// The run index (adjacency built from one scan per shape predicate) is a
    /// pure cache: a run forced to build it reports exactly what a run forced
    /// to answer every probe from the store reports, on a multi-graph fixture
    /// with every path form and a property-pair constraint.
    #[test]
    fn run_index_is_invisible_in_the_report() {
        use super::super::view::{set_index_policy_override, IndexPolicy};
        let shapes = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
            ex:S a sh:NodeShape ; sh:targetClass ex:T ;
                sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:datatype xsd:string ] ,
                            [ sh:path [ sh:inversePath ex:partOf ] ; sh:maxCount 1 ] ,
                            [ sh:path ( ex:partOf ex:name ) ; sh:minCount 1 ] ,
                            [ sh:path [ sh:alternativePath ( ex:name ex:code ) ] ; sh:minCount 2 ] ,
                            [ sh:path [ sh:zeroOrMorePath ex:partOf ] ; sh:class ex:T ] ,
                            [ sh:path ex:low ; sh:lessThan ex:high ] .
        "#;
        let g1 = "http://example.org/g1";
        let g2 = "http://example.org/g2";
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(shapes, RdfFormat::Turtle, Some("urn:shapes"))
            .unwrap();
        store
            .load_str(
                r#"@prefix ex: <http://example.org/> .
                   ex:a a ex:T ; ex:name "a" ; ex:code "A" ; ex:partOf ex:root ; ex:low 1 ; ex:high 2 .
                   ex:b a ex:T ; ex:name "b" ; ex:partOf ex:root ; ex:low 5 ; ex:high 2 .
                   ex:root a ex:T ; ex:name "root" .
                   ex:c a ex:T ; ex:partOf ex:root, ex:a ."#,
                RdfFormat::Turtle,
                Some(g1),
            )
            .unwrap();
        store
            .load_str(
                r#"@prefix ex: <http://example.org/> .
                   ex:d a ex:T ; ex:name "d" ; ex:code "D" ; ex:partOf ex:e .
                   ex:e ex:name "e" .
                   ex:root ex:partOf ex:d ."#,
                RdfFormat::Turtle,
                Some(g2),
            )
            .unwrap();
        let graphs = vec![g1.to_string(), g2.to_string()];
        let summary = |r: &ValidationReport| {
            let mut v: Vec<String> = r
                .results
                .iter()
                .map(|x| {
                    format!(
                        "{} | {} | {:?} | {:?}",
                        x.focus_node, x.source_constraint, x.path, x.value
                    )
                })
                .collect();
            v.sort();
            v
        };
        set_index_policy_override(Some(IndexPolicy {
            min_probes: usize::MAX,
            max_quads: 0,
        }));
        let without = validate(&store, "urn:shapes", &graphs).unwrap();
        set_index_policy_override(Some(IndexPolicy {
            min_probes: 0,
            max_quads: usize::MAX,
        }));
        let with = validate(&store, "urn:shapes", &graphs).unwrap();
        // A budget too small for every pair: some pairs indexed, the rest raw.
        set_index_policy_override(Some(IndexPolicy {
            min_probes: 0,
            max_quads: 1,
        }));
        let partial = validate(&store, "urn:shapes", &graphs).unwrap();
        set_index_policy_override(None);
        assert!(!without.conforms, "fixture has violations");
        assert!(without.results_count >= 4, "{:?}", summary(&without));
        assert_eq!(summary(&with), summary(&without));
        assert_eq!(summary(&partial), summary(&without));
    }

    /// A run reads one instant. `sh:sparql` constraints, custom-component
    /// validators and SHACL-AF SPARQL targets used to go through
    /// `TripleStore::query` — the LIVE store — while every native probe read
    /// the snapshot taken when the run started, so a write landing mid-run was
    /// visible to half a shapes graph and invisible to the other half. Both
    /// halves now read the view's own source. Deterministic: the write happens
    /// after the view exists, with no threads and no sleeps.
    #[test]
    fn sparql_and_native_probes_read_the_same_snapshot() {
        use super::super::view::DataView;
        let dir = tempfile::tempdir().unwrap();
        let store = TripleStore::open(dir.path()).unwrap();
        assert!(store.is_persistent());
        let g = "http://example.org/g";
        store
            .load_str(
                r#"@prefix ex: <http://example.org/> .
                   ex:a ex:p ex:before ."#,
                RdfFormat::Turtle,
                Some(g),
            )
            .unwrap();

        let graphs = vec![g.to_string()];
        let view = DataView::new(&store, &graphs);
        assert_eq!(
            view.source_kind(),
            "snapshot",
            "the persistent backend must take the snapshot path for this test to mean anything"
        );

        // A write that lands after the run's snapshot was taken.
        store
            .update(
                "INSERT DATA { GRAPH <http://example.org/g>                  { <http://example.org/a> <http://example.org/p> <http://example.org/after> } }",
            )
            .unwrap();
        assert!(
            store
                .query("ASK { GRAPH ?g { ?s ?p <http://example.org/after> } }")
                .is_ok(),
            "the write landed in the store"
        );

        let native = view.step(
            &Term::NamedNode(oxigraph::model::NamedNode::new_unchecked(
                "http://example.org/a",
            )),
            "http://example.org/p",
            false,
            super::super::view::GraphSel::All,
        );
        let via_sparql = execute_select_terms(
            &view,
            "SELECT ?this FROM <http://example.org/g>              WHERE { <http://example.org/a> <http://example.org/p> ?this }",
            "this",
        )
        .unwrap();

        let mut a: Vec<String> = native.iter().map(term_to_lexical).collect();
        let mut b: Vec<String> = via_sparql.iter().map(term_to_lexical).collect();
        a.sort();
        b.sort();
        assert_eq!(
            a, b,
            "the native probe and the SPARQL route must see the same instant"
        );
        assert!(
            !a.iter().any(|t| t.ends_with("after")),
            "neither may see a write that landed after the run's snapshot: {a:?}"
        );
    }

    /// The persistent backend takes the snapshot path (one RocksDB readable
    /// transaction per run, projected scans for the class sets and the run
    /// index). Same fixture, same report, with and without the index.
    #[test]
    fn snapshot_path_on_rocksdb_matches_with_and_without_the_index() {
        use super::super::view::{set_index_policy_override, IndexPolicy};
        let dir = tempfile::tempdir().unwrap();
        let store = TripleStore::open(dir.path()).unwrap();
        assert!(store.is_persistent());
        let shapes = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            ex:S a sh:NodeShape ; sh:targetClass ex:T ;
                sh:property [ sh:path ex:name ; sh:minCount 1 ] ,
                            [ sh:path [ sh:inversePath ex:partOf ] ; sh:maxCount 1 ] ,
                            [ sh:path ex:partOf ; sh:class ex:T ] .
        "#;
        let g1 = "http://example.org/g1";
        let g2 = "http://example.org/g2";
        store
            .load_str(shapes, RdfFormat::Turtle, Some("urn:shapes"))
            .unwrap();
        store
            .load_str(
                r#"@prefix ex: <http://example.org/> .
                   @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
                   ex:Sub rdfs:subClassOf ex:T .
                   ex:a a ex:T ; ex:name "a" ; ex:partOf ex:root .
                   ex:b a ex:Sub ; ex:partOf ex:root .
                   ex:root a ex:T ; ex:name "root" .
                   ex:c a ex:T ; ex:name "c" ; ex:partOf ex:a, ex:b ."#,
                RdfFormat::Turtle,
                Some(g1),
            )
            .unwrap();
        store
            .load_str(
                r#"@prefix ex: <http://example.org/> .
                   ex:d a ex:T ; ex:partOf ex:nobody ."#,
                RdfFormat::Turtle,
                Some(g2),
            )
            .unwrap();
        let graphs = vec![g1.to_string(), g2.to_string()];
        let summary = |r: &ValidationReport| {
            let mut v: Vec<String> = r
                .results
                .iter()
                .map(|x| format!("{} | {} | {:?}", x.focus_node, x.source_constraint, x.value))
                .collect();
            v.sort();
            v
        };
        set_index_policy_override(Some(IndexPolicy {
            min_probes: usize::MAX,
            max_quads: 0,
        }));
        let without = validate(&store, "urn:shapes", &graphs).unwrap();
        set_index_policy_override(Some(IndexPolicy {
            min_probes: 0,
            max_quads: usize::MAX,
        }));
        let with = validate(&store, "urn:shapes", &graphs).unwrap();
        set_index_policy_override(None);
        // b (a Sub) lacks a name; a has two children (c and ... no: a is partOf
        // root only) — root has two children a and b → inverse maxCount 1
        // violated for root; d's partOf points at an untyped node → sh:class.
        assert_eq!(summary(&with), summary(&without));
        let focus: std::collections::BTreeSet<&str> = without
            .results
            .iter()
            .map(|r| r.focus_node.as_str())
            .collect();
        assert!(
            focus.contains("http://example.org/b"),
            "{:?}",
            summary(&without)
        );
        assert!(
            focus.contains("http://example.org/root"),
            "{:?}",
            summary(&without)
        );
        assert!(
            focus.contains("http://example.org/d"),
            "{:?}",
            summary(&without)
        );
    }
}
