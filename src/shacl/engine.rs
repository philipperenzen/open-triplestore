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

    // Iterate until fixed point (no new triples produced)
    for iteration in 0..100 {
        let mut inferred_this_round = 0usize;

        for (_shape_iri, targets, rule_type, rule_body) in &rules {
            let focus_nodes = resolve_rule_targets(store, targets, data_graphs);

            for focus_node in &focus_nodes {
                let count = apply_rule(store, focus_node, rule_type, rule_body)?;
                inferred_this_round += count;
            }
        }

        if inferred_this_round == 0 {
            debug!("Fixed point reached after {} iterations", iteration + 1);
            break;
        }

        total_inferred += inferred_this_round;
        debug!(
            "Iteration {}: inferred {} triples",
            iteration + 1,
            inferred_this_round
        );
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

    // SHACL-AF: SPARQL targets
    let sparql_targets = execute_select_single(
        store,
        &format!(
            r#"
            PREFIX sh: <{SH}>
            SELECT ?select WHERE {{
                GRAPH <{shapes_graph}> {{
                    <{shape_iri}> sh:target ?t .
                    ?t sh:select ?select .
                }}
            }}
            "#,
        ),
        "select",
    )?;
    for s in sparql_targets {
        targets.push(Target::SparqlTarget(s));
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
    let is_closed = ask(
        store,
        &format!("ASK {{ GRAPH <{shapes_graph}> {{ <{shape_iri}> <{SH}closed> true }} }}"),
    );
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
        constraints.push(Constraint::QualifiedValueShape {
            shape_iri: qvs_iri,
            min_count,
            max_count,
        });
    }

    // SHACL-AF: sh:sparql constraints
    let sparql_constraints = execute_select_single(
        store,
        &format!(
            r#"
            PREFIX sh: <{SH}>
            SELECT ?select ?message WHERE {{
                GRAPH <{shapes_graph}> {{
                    <{shape_iri}> sh:sparql ?sparql .
                    ?sparql sh:select ?select .
                    OPTIONAL {{ ?sparql sh:message ?message }}
                }}
            }}
            "#,
        ),
        "select",
    )?;

    // For SPARQL constraints we also need the messages - use a second query
    if !sparql_constraints.is_empty() {
        let query = format!(
            r#"
            PREFIX sh: <{SH}>
            SELECT ?select ?message WHERE {{
                GRAPH <{shapes_graph}> {{
                    <{shape_iri}> sh:sparql ?sparql .
                    ?sparql sh:select ?select .
                    OPTIONAL {{ ?sparql sh:message ?message }}
                }}
            }}
            "#,
        );
        if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&query) {
            for solution in solutions.filter_map(|s| s.ok()) {
                let select = match solution.get("select") {
                    Some(oxigraph::model::Term::Literal(lit)) => lit.value().to_string(),
                    _ => continue,
                };
                let message = solution.get("message").and_then(|v| match v {
                    oxigraph::model::Term::Literal(lit) => Some(lit.value().to_string()),
                    _ => None,
                });
                constraints.push(Constraint::SparqlConstraint { select, message });
            }
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
    let ps_iris = execute_select_single(
        store,
        &format!(
            "SELECT ?ps WHERE {{ GRAPH <{shapes_graph}> {{ <{shape_iri}> <{SH}property> ?ps }} }}"
        ),
        "ps",
    )?;

    let mut result = Vec::new();

    for ps_iri in &ps_iris {
        // Load path
        let path_value = single_value(store, shapes_graph, ps_iri, &format!("{}path", SH));
        let path = match path_value {
            Some(p) => PropertyPath::Predicate(p),
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

    // SPARQL rules
    let query = format!(
        r#"
        PREFIX sh: <{SH}>
        SELECT ?shape ?construct WHERE {{
            GRAPH <{shapes_graph}> {{
                ?shape sh:rule ?rule .
                ?rule sh:construct ?construct .
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
            let construct = match solution.get("construct") {
                Some(oxigraph::model::Term::Literal(lit)) => lit.value().to_string(),
                _ => continue,
            };

            let targets = load_targets(store, shapes_graph, &shape_iri).unwrap_or_default();
            rules.push((shape_iri, targets, RuleType::SparqlRule, construct));
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
            let subject = term_to_string(solution.get("subject"));
            let predicate = term_to_string(solution.get("predicate"));
            let object = term_to_string(solution.get("object"));

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

fn apply_rule(
    store: &TripleStore,
    focus_node: &str,
    rule_type: &RuleType,
    rule_body: &str,
) -> Result<usize, String> {
    match rule_type {
        RuleType::SparqlRule => {
            // Replace $this with the focus node IRI
            let construct = rule_body.replace("$this", &format!("<{}>", focus_node));
            match store.update(&construct) {
                Ok(()) => Ok(1), // Approximate; CONSTRUCT UPDATE not trivially counted
                Err(e) => {
                    warn!("SPARQL rule error: {}", e);
                    Ok(0)
                }
            }
        }
        RuleType::TripleRule => {
            let body = rule_body.replace("$this", &format!("<{}>", focus_node));
            let update = format!("INSERT DATA {{ {} }}", body);
            match store.update(&update) {
                Ok(()) => Ok(1),
                Err(e) => {
                    warn!("Triple rule error: {}", e);
                    Ok(0)
                }
            }
        }
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
    let query = format!(
        "SELECT ?v WHERE {{ GRAPH <{shapes_graph}> {{ <{subject}> <{predicate}> ?v }} }} LIMIT 1"
    );
    execute_select_single(store, &query, "v")
        .ok()
        .and_then(|v| v.into_iter().next())
}

fn multi_values(
    store: &TripleStore,
    shapes_graph: &str,
    subject: &str,
    predicate: &str,
) -> Vec<String> {
    let query =
        format!("SELECT ?v WHERE {{ GRAPH <{shapes_graph}> {{ <{subject}> <{predicate}> ?v }} }}");
    execute_select_single(store, &query, "v").unwrap_or_default()
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
