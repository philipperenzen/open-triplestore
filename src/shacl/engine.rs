use super::constraints::evaluate_constraint;
use super::report::{Severity, ValidationReport, ValidationResult};
use super::shapes::*;
use crate::store::TripleStore;
use rayon::prelude::*;
use tracing::{debug, info, warn};

const SH: &str = "http://www.w3.org/ns/shacl#";

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

    // Reset the calling thread's property-path cache before this validation pass.
    // Each rayon worker thread also has its own cache that seeds itself lazily on
    // first use; they hold at most MAX_ENTRIES (10 000) entries with LRU eviction.
    crate::store::path_cache::tl_clear();

    let shapes = load_shapes(store, shapes_graph)?;
    debug!("Loaded {} shapes", shapes.len());

    // Evaluate shapes in parallel using rayon (4-8x speedup on multi-core).
    // Each shape is independent — evaluate_constraint() only reads from Arc<Store>.
    // `shapes_slice` is a shared immutable reference passed into parallel closures so
    // that logical constraint operators (sh:not, sh:and, sh:or, sh:xone, sh:node,
    // sh:qualifiedValueShape) can look up sibling shapes by IRI.
    let shapes_slice: &[Shape] = &shapes;
    let all_results: Vec<ValidationResult> = shapes_slice
        .par_iter()
        .filter(|shape| !shape.deactivated)
        .flat_map(|shape| {
            let severity = shape
                .severity
                .as_deref()
                .map(Severity::from_iri)
                .unwrap_or(Severity::Violation);

            let focus_nodes = resolve_targets(store, shape, data_graphs);
            debug!(
                "Shape <{}> has {} target nodes",
                shape.iri,
                focus_nodes.len()
            );

            focus_nodes
                .par_iter()
                .flat_map(|focus_node| {
                    let mut results = Vec::new();

                    // Node-level constraints
                    for constraint in &shape.constraints {
                        results.extend(evaluate_constraint(
                            store,
                            shapes_slice,
                            &shape.iri,
                            focus_node,
                            constraint,
                            None,
                            data_graphs,
                            &severity,
                        ));
                    }

                    // Property shape constraints
                    for prop_shape in &shape.property_shapes {
                        let shape_iri = prop_shape.iri.as_deref().unwrap_or(&shape.iri);

                        for constraint in &prop_shape.constraints {
                            results.extend(evaluate_constraint(
                                store,
                                shapes_slice,
                                shape_iri,
                                focus_node,
                                constraint,
                                Some(&prop_shape.path),
                                data_graphs,
                                &severity,
                            ));
                        }
                    }

                    results
                })
                .collect::<Vec<_>>()
        })
        .collect();

    let conforms = all_results.is_empty();
    let results_count = all_results.len();

    debug!(
        "SHACL validation complete: {} violations; path-cache entries this thread: {}",
        results_count,
        crate::store::path_cache::tl_len()
    );

    Ok(ValidationReport {
        conforms,
        results: all_results,
        results_count,
    })
}

/// Apply SHACL-AF inference rules and materialise derived triples.
///
/// Returns the number of triples generated.
pub fn infer(
    store: &TripleStore,
    shapes_graph: &str,
    data_graphs: &[String],
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
    for iteration in 0..100 {
        let before = store.len().map_err(|e| e.to_string())?;

        for (_shape_iri, targets, rule_type, rule_body) in &rules {
            let focus_nodes = resolve_rule_targets(store, targets, data_graphs);

            for focus_node in &focus_nodes {
                apply_rule(store, focus_node, rule_type, rule_body)?;
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

fn load_shapes(store: &TripleStore, shapes_graph: &str) -> Result<Vec<Shape>, String> {
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
        match load_single_shape(store, shapes_graph, shape_iri) {
            Ok(shape) => shapes.push(shape),
            Err(e) => warn!("Failed to load shape <{}>: {}", shape_iri, e),
        }
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

    // Load direct constraints on the node shape
    let constraints = load_constraints(store, shapes_graph, shape_iri)?;

    // Load property shapes
    let property_shapes = load_property_shapes(store, shapes_graph, shape_iri)?;

    Ok(Shape {
        iri: shape_iri.to_string(),
        shape_type: ShapeType::NodeShape,
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

    // sh:targetClass
    let classes = execute_select_single(
        store,
        &format!(
            "SELECT ?v WHERE {{ GRAPH <{shapes_graph}> {{ <{shape_iri}> <{SH}targetClass> ?v }} }}"
        ),
        "v",
    )?;
    for c in classes {
        targets.push(Target::TargetClass(c));
    }

    // sh:targetNode
    let nodes = execute_select_single(
        store,
        &format!(
            "SELECT ?v WHERE {{ GRAPH <{shapes_graph}> {{ <{shape_iri}> <{SH}targetNode> ?v }} }}"
        ),
        "v",
    )?;
    for n in nodes {
        targets.push(Target::TargetNode(n));
    }

    // sh:targetSubjectsOf
    let preds = execute_select_single(
        store,
        &format!(
            "SELECT ?v WHERE {{ GRAPH <{shapes_graph}> {{ <{shape_iri}> <{SH}targetSubjectsOf> ?v }} }}"
        ),
        "v",
    )?;
    for p in preds {
        targets.push(Target::TargetSubjectsOf(p));
    }

    // sh:targetObjectsOf
    let preds = execute_select_single(
        store,
        &format!(
            "SELECT ?v WHERE {{ GRAPH <{shapes_graph}> {{ <{shape_iri}> <{SH}targetObjectsOf> ?v }} }}"
        ),
        "v",
    )?;
    for p in preds {
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
            targets.push(Target::SparqlTarget(format!("{prefixes}{select}")));
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

    // sh:minExclusive / sh:minInclusive / sh:maxExclusive / sh:maxInclusive
    if let Some(v) = single_value(
        store,
        shapes_graph,
        shape_iri,
        &format!("{}minExclusive", SH),
    ) {
        constraints.push(Constraint::MinExclusive(v));
    }
    if let Some(v) = single_value(
        store,
        shapes_graph,
        shape_iri,
        &format!("{}minInclusive", SH),
    ) {
        constraints.push(Constraint::MinInclusive(v));
    }
    if let Some(v) = single_value(
        store,
        shapes_graph,
        shape_iri,
        &format!("{}maxExclusive", SH),
    ) {
        constraints.push(Constraint::MaxExclusive(v));
    }
    if let Some(v) = single_value(
        store,
        shapes_graph,
        shape_iri,
        &format!("{}maxInclusive", SH),
    ) {
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

    // sh:hasValue
    if let Some(v) = single_value(store, shapes_graph, shape_iri, &format!("{}hasValue", SH)) {
        constraints.push(Constraint::HasValue(v));
    }

    // sh:in (RDF list)
    let in_values = load_rdf_list(store, shapes_graph, shape_iri, &format!("{}in", SH));
    if !in_values.is_empty() {
        constraints.push(Constraint::In(in_values));
    }

    // sh:languageIn (RDF list)
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

    // sh:closed + sh:ignoredProperties
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
        constraints.push(Constraint::Closed {
            ignored_properties: ignored,
        });
    }

    // sh:node
    for v in multi_values(store, shapes_graph, shape_iri, &format!("{}node", SH)) {
        constraints.push(Constraint::Node(v));
    }

    // sh:not
    if let Some(not_iri) = single_value(store, shapes_graph, shape_iri, &format!("{}not", SH)) {
        if let Ok(not_shape) = load_inline_shape(store, shapes_graph, &not_iri) {
            constraints.push(Constraint::Not(Box::new(not_shape)));
        }
    }

    // sh:and (RDF list of shape IRIs)
    let and_iris = load_rdf_list(store, shapes_graph, shape_iri, &format!("{}and", SH));
    if !and_iris.is_empty() {
        let mut and_shapes = Vec::new();
        for iri in &and_iris {
            if let Ok(s) = load_inline_shape(store, shapes_graph, iri) {
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
            if let Ok(s) = load_inline_shape(store, shapes_graph, iri) {
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
            if let Ok(s) = load_inline_shape(store, shapes_graph, iri) {
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
        // Load the value shape inline (named or blank) so an inline `[ … ]` is enforced
        // rather than looked up — and silently skipped — in the top-level shapes list.
        if let Ok(qvs_shape) = load_inline_shape(store, shapes_graph, &qvs_iri) {
            constraints.push(Constraint::QualifiedValueShape {
                shape: Box::new(qvs_shape),
                min_count,
                max_count,
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
            constraints.push(Constraint::SparqlConstraint {
                select: format!("{prefixes}{select}"),
                message,
                severity,
            });
        }
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

/// Load an inline (possibly blank-node) shape by IRI for use in logical constraint operators.
fn load_inline_shape(
    store: &TripleStore,
    shapes_graph: &str,
    shape_iri: &str,
) -> Result<Shape, String> {
    let constraints = load_constraints(store, shapes_graph, shape_iri)?;
    let property_shapes = load_property_shapes_inner(store, shapes_graph, shape_iri)?;
    Ok(Shape {
        iri: shape_iri.to_string(),
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
        let constraints = load_constraints(store, shapes_graph, ps_iri)?;

        let name = single_value(store, shapes_graph, ps_iri, &format!("{}name", SH));
        let description = single_value(store, shapes_graph, ps_iri, &format!("{}description", SH));

        result.push(PropertyShape {
            iri: Some(ps_iri.clone()),
            path,
            constraints,
            name,
            description,
        });
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Target resolution
// ---------------------------------------------------------------------------

fn resolve_targets(store: &TripleStore, shape: &Shape, data_graphs: &[String]) -> Vec<String> {
    let mut focus_nodes = Vec::new();

    for target in &shape.targets {
        match target {
            Target::TargetClass(class_iri) => {
                // All instances of the class across the dataset's data graphs.
                let query = format!(
                    "SELECT DISTINCT ?s WHERE {{ {} }}",
                    graph_scoped(data_graphs, &format!("?s a <{class_iri}>"))
                );
                if let Ok(nodes) = execute_select_single(store, &query, "s") {
                    focus_nodes.extend(nodes);
                }
            }
            Target::TargetNode(node_iri) => {
                focus_nodes.push(node_iri.clone());
            }
            Target::TargetSubjectsOf(pred_iri) => {
                let query = format!(
                    "SELECT DISTINCT ?s WHERE {{ {} }}",
                    graph_scoped(data_graphs, &format!("?s <{pred_iri}> ?o"))
                );
                if let Ok(nodes) = execute_select_single(store, &query, "s") {
                    focus_nodes.extend(nodes);
                }
            }
            Target::TargetObjectsOf(pred_iri) => {
                let query = format!(
                    "SELECT DISTINCT ?o WHERE {{ {} }}",
                    graph_scoped(data_graphs, &format!("?s <{pred_iri}> ?o"))
                );
                if let Ok(nodes) = execute_select_single(store, &query, "o") {
                    focus_nodes.extend(nodes);
                }
            }
            Target::SparqlTarget(sparql) => {
                // SHACL-AF custom SPARQL target
                if let Ok(nodes) = execute_select_single(store, sparql, "this") {
                    focus_nodes.extend(nodes);
                }
            }
        }
    }

    // Deduplicate
    focus_nodes.sort();
    focus_nodes.dedup();
    focus_nodes
}

// ---------------------------------------------------------------------------
// SHACL-AF rules
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum RuleType {
    SparqlRule,
    TripleRule,
}

#[allow(clippy::type_complexity)]
fn load_rules(
    store: &TripleStore,
    shapes_graph: &str,
) -> Result<Vec<(String, Vec<Target>, RuleType, String)>, String> {
    let mut rules: Vec<(String, Vec<Target>, RuleType, String)> = Vec::new();

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
                let prefixes = sparql_prefixes(store, shapes_graph, &rule_node);
                rules.push((
                    shape_iri.clone(),
                    targets.clone(),
                    RuleType::SparqlRule,
                    format!("{prefixes}{construct}"),
                ));
            }
        }
    }

    // Triple rules
    let query = format!(
        r#"
        PREFIX sh: <{SH}>
        SELECT ?shape ?subject ?predicate ?object WHERE {{
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
            let subject = triple_rule_term(solution.get("subject"));
            let predicate = triple_rule_term(solution.get("predicate"));
            let object = triple_rule_term(solution.get("object"));

            let body = format!("{} {} {}", subject, predicate, object);
            let targets = load_targets(store, shapes_graph, &shape_iri).unwrap_or_default();
            rules.push((shape_iri, targets, RuleType::TripleRule, body));
        }
    }

    Ok(rules)
}

fn resolve_rule_targets(
    store: &TripleStore,
    targets: &[Target],
    data_graphs: &[String],
) -> Vec<String> {
    let dummy_shape = Shape {
        iri: String::new(),
        shape_type: ShapeType::NodeShape,
        targets: targets.to_vec(),
        constraints: Vec::new(),
        property_shapes: Vec::new(),
        severity: None,
        message: None,
        deactivated: false,
    };
    resolve_targets(store, &dummy_shape, data_graphs)
}

/// Apply one rule to one focus node. The number of *new* triples is not measured
/// here — `infer` tracks it via the store's count delta per round (see there), so
/// a rule whose output already exists costs nothing and the fixed point is exact.
///
/// A single malformed/erroring rule is logged and skipped rather than failing the
/// whole inference run; because it materialises nothing, it cannot prevent
/// convergence.
fn apply_rule(
    store: &TripleStore,
    focus_node: &str,
    rule_type: &RuleType,
    rule_body: &str,
) -> Result<(), String> {
    let update = match rule_type {
        RuleType::SparqlRule => {
            // Bind the focus node, then accept either the spec CONSTRUCT-template
            // form (`CONSTRUCT { t } WHERE { p }`) or the convenience
            // `INSERT { t } WHERE { p }` form — both materialise into the store.
            let bound = rule_body.replace("$this", &format!("<{}>", focus_node));
            construct_to_update(&bound)
        }
        RuleType::TripleRule => {
            // `$this` (from `sh:this`, mapped in `load_rules`) binds to the focus.
            let body = rule_body.replace("$this", &format!("<{}>", focus_node));
            format!("INSERT DATA {{ {} }}", body)
        }
    };
    if let Err(e) = store.update(&update) {
        warn!("SHACL rule application error: {}", e);
    }
    Ok(())
}

/// Translate a `sh:construct` rule body into an executable SPARQL UPDATE.
///
/// SHACL-AF's `sh:construct` carries a SPARQL **CONSTRUCT** query
/// (`CONSTRUCT { template } WHERE { pattern }`); its output is materialised by
/// running it as `INSERT { template } WHERE { pattern }`. The convenience
/// `INSERT { … } WHERE { … }` form is already an update and is passed through
/// unchanged. `$this` is expected to be already substituted.
///
/// Only the leading `CONSTRUCT` query keyword is rewritten — the template and
/// `WHERE` clause are kept verbatim. A `PREFIX`/`BASE` prologue is skipped first
/// so a `construct` substring inside a prefix IRI is never mistaken for it.
fn construct_to_update(body: &str) -> String {
    let mut rest = body.trim_start();
    loop {
        let token = rest
            .split(|c: char| c.is_whitespace() || c == '<' || c == '{')
            .next()
            .unwrap_or("");
        if token.eq_ignore_ascii_case("prefix") || token.eq_ignore_ascii_case("base") {
            match rest.find('>') {
                Some(gt) => rest = rest[gt + 1..].trim_start(),
                None => return body.to_string(),
            }
        } else if token.eq_ignore_ascii_case("construct") {
            let head_len = body.len() - rest.len();
            return format!("{}INSERT{}", &body[..head_len], &rest[token.len()..]);
        } else {
            return body.to_string();
        }
    }
}

/// Stringify a triple-rule term, mapping `sh:this` to the `$this` placeholder so
/// `apply_rule` binds it to each focus node (SHACL-AF §4.3 — `sh:this` denotes the
/// focus node, not the literal `sh:this` IRI).
fn triple_rule_term(term: Option<&oxigraph::model::Term>) -> String {
    if let Some(oxigraph::model::Term::NamedNode(nn)) = term {
        if nn.as_str() == "http://www.w3.org/ns/shacl#this" {
            return "$this".to_string();
        }
    }
    term_to_string(term)
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
/// malformed path so the caller can skip the property shape rather than mis-bind it. The
/// previous loader treated every path as a single predicate, so a blank-node path collapsed
/// to `Predicate("_:bn")` and matched nothing.
fn parse_property_path(
    store: &TripleStore,
    shapes_graph: &str,
    node: &str,
) -> Option<PropertyPath> {
    // A predicate path is a plain IRI.
    if !node.starts_with("_:") {
        return Some(PropertyPath::Predicate(node.to_string()));
    }
    // Blank node: a path-operator object, otherwise an RDF-list sequence path.
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
    // Otherwise: an RDF-list sequence path `( p1 p2 … )`.
    let seq: Vec<PropertyPath> = rdf_list_elements(store, shapes_graph, node)
        .iter()
        .filter_map(|e| parse_property_path(store, shapes_graph, e))
        .collect();
    (!seq.is_empty()).then_some(PropertyPath::Sequence(seq))
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
fn sparql_prefixes(store: &TripleStore, shapes_graph: &str, node: &str) -> String {
    let mut out = String::new();
    for owner in store
        .objects_for_subject_in_graph(node, &format!("{SH}prefixes"), Some(shapes_graph))
        .iter()
        .map(term_to_lexical)
    {
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
    // Walk the RDF list via rdf:first/rdf:rest. In standard Turtle `( … )`
    // syntax the list cells are blank nodes, which SPARQL surface syntax cannot
    // re-address (`_:x` in a query is a fresh existential, so the old
    // `<_:bn>`-interpolated queries silently matched nothing and left every
    // list-based constraint — sh:in, sh:languageIn, sh:and/or/xone,
    // sh:ignoredProperties — empty). Resolve cells through the raw quad index.
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
            values.push(term_to_lexical(&first));
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
        oxigraph::model::Term::Triple(t) => t.to_string(),
    }
}

/// Wrap a triple-pattern `body` so it is matched within the dataset's data graphs.
///
/// - Empty `data_graphs`: match in the default graph (unscoped), preserving the
///   behaviour for datasets without explicitly registered graphs.
/// - One or more graphs: a UNION of `GRAPH <g> { body }` blocks, evaluating the
///   pattern against exactly those graphs.
///
/// Replaces an earlier form that emitted `GRAPH <g> body` without the required
/// braces — invalid SPARQL that silently matched nothing, so any dataset with a
/// registered graph always reported `conforms: true`.
pub(crate) fn graph_scoped(data_graphs: &[String], body: &str) -> String {
    if data_graphs.is_empty() {
        body.to_string()
    } else {
        data_graphs
            .iter()
            .map(|g| format!("{{ GRAPH <{g}> {{ {body} }} }}"))
            .collect::<Vec<_>>()
            .join(" UNION ")
    }
}

fn term_to_string(term: Option<&oxigraph::model::Term>) -> String {
    match term {
        Some(oxigraph::model::Term::NamedNode(nn)) => format!("<{}>", nn.as_str()),
        Some(oxigraph::model::Term::Literal(lit)) => {
            if let Some(lang) = lit.language() {
                format!("\"{}\"@{}", lit.value(), lang)
            } else {
                format!("\"{}\"", lit.value())
            }
        }
        Some(oxigraph::model::Term::BlankNode(bn)) => format!("_:{}", bn.as_str()),
        _ => "\"\"".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;

    // Uses a *named* property shape (ex:NameProp) rather than a blank node: the
    // engine resolves property-shape constraints by IRI, so a blank-node
    // `sh:property [ ... ]` is not currently picked up.
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
}
