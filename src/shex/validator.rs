//! ShEx shape evaluation engine.
//!
//! Validates RDF nodes against shape expressions by matching outgoing (and
//! optionally incoming) triples against triple expression constraints with
//! cardinality checking.

use std::collections::{HashMap, HashSet};

use oxigraph::model::NamedNodeRef;
use tracing::debug;

use super::report::{ShExReport, ShExResult, ShExStatus};
use super::schema::*;
use crate::store::TripleStore;

/// Validate focus nodes against shapes in a ShEx schema.
///
/// `shape_map` maps shape IRIs to lists of focus node IRIs to validate.
/// If `shape_map` is empty, uses the schema's `start` shape (if any) and
/// validates all nodes that match the relevant triple patterns.
pub fn validate(
    store: &TripleStore,
    schema: &ShExSchema,
    shape_map: &HashMap<String, Vec<String>>,
) -> ShExReport {
    let mut results = Vec::new();
    let mut visited = HashSet::new();

    if shape_map.is_empty() {
        // If no explicit shape map, validate all shapes against nodes found via patterns
        for shape_decl in &schema.shapes {
            let focus_nodes = find_candidate_nodes(store, &shape_decl.shape_expr);
            for node in &focus_nodes {
                let status =
                    evaluate_shape_expr(store, schema, node, &shape_decl.shape_expr, &mut visited);
                results.push(ShExResult {
                    focus_node: node.clone(),
                    shape: shape_decl.id.clone(),
                    status,
                });
            }
        }
    } else {
        for (shape_iri, nodes) in shape_map {
            let shape_decl = match schema.find_shape(shape_iri) {
                Some(s) => s,
                None => {
                    for node in nodes {
                        results.push(ShExResult {
                            focus_node: node.clone(),
                            shape: shape_iri.clone(),
                            status: ShExStatus::NonConformant(format!(
                                "Shape <{}> not found in schema",
                                shape_iri
                            )),
                        });
                    }
                    continue;
                }
            };
            for node in nodes {
                visited.clear();
                let status =
                    evaluate_shape_expr(store, schema, node, &shape_decl.shape_expr, &mut visited);
                results.push(ShExResult {
                    focus_node: node.clone(),
                    shape: shape_iri.clone(),
                    status,
                });
            }
        }
    }

    ShExReport {
        conforms: results
            .iter()
            .all(|r| matches!(r.status, ShExStatus::Conformant)),
        results,
    }
}

/// Find candidate focus nodes for a shape expression by looking at its
/// triple constraints' predicates.
fn find_candidate_nodes(store: &TripleStore, expr: &ShapeExpr) -> Vec<String> {
    let mut predicates = Vec::new();
    collect_predicates(expr, &mut predicates);

    if predicates.is_empty() {
        return vec![];
    }

    let mut nodes = HashSet::new();
    for pred in &predicates {
        if let Ok(pred_ref) = NamedNodeRef::new(pred.as_str()) {
            for quad in store
                .store()
                .quads_for_pattern(None, Some(pred_ref), None, None)
            {
                if let Ok(q) = quad {
                    nodes.insert(q.subject.to_string());
                }
            }
        }
    }
    nodes.into_iter().collect()
}

/// Collect all predicate IRIs from triple constraints in a shape expression.
fn collect_predicates(expr: &ShapeExpr, predicates: &mut Vec<String>) {
    match expr {
        ShapeExpr::Shape { expression, .. } => {
            if let Some(te) = expression {
                collect_predicates_from_te(te, predicates);
            }
        }
        ShapeExpr::ShapeAnd(exprs) | ShapeExpr::ShapeOr(exprs) => {
            for e in exprs {
                collect_predicates(e, predicates);
            }
        }
        ShapeExpr::ShapeNot(inner) => collect_predicates(inner, predicates),
        ShapeExpr::ShapeRef(_) | ShapeExpr::NodeConstraint(_) | ShapeExpr::NodeConstraintAny => {}
    }
}

fn collect_predicates_from_te(te: &TripleExpr, predicates: &mut Vec<String>) {
    match te {
        TripleExpr::TripleConstraint { predicate, .. } => {
            predicates.push(predicate.clone());
        }
        TripleExpr::EachOf(exprs) | TripleExpr::OneOf(exprs) => {
            for e in exprs {
                collect_predicates_from_te(e, predicates);
            }
        }
    }
}

/// Evaluate a shape expression against a focus node.
fn evaluate_shape_expr(
    store: &TripleStore,
    schema: &ShExSchema,
    focus_node: &str,
    expr: &ShapeExpr,
    visited: &mut HashSet<(String, String)>,
) -> ShExStatus {
    match expr {
        ShapeExpr::NodeConstraint(nc) => evaluate_node_constraint(store, focus_node, nc),

        ShapeExpr::Shape {
            expression,
            closed,
            extra,
        } => evaluate_shape(
            store, schema, focus_node, expression, *closed, extra, visited,
        ),

        ShapeExpr::ShapeAnd(exprs) => {
            let mut errors = Vec::new();
            for e in exprs {
                match evaluate_shape_expr(store, schema, focus_node, e, visited) {
                    ShExStatus::Conformant => {}
                    ShExStatus::NonConformant(msg) => errors.push(msg),
                }
            }
            if errors.is_empty() {
                ShExStatus::Conformant
            } else {
                ShExStatus::NonConformant(errors.join("; "))
            }
        }

        ShapeExpr::ShapeOr(exprs) => {
            for e in exprs {
                if matches!(
                    evaluate_shape_expr(store, schema, focus_node, e, visited),
                    ShExStatus::Conformant
                ) {
                    return ShExStatus::Conformant;
                }
            }
            ShExStatus::NonConformant("None of the OR alternatives matched".to_string())
        }

        ShapeExpr::ShapeNot(inner) => {
            match evaluate_shape_expr(store, schema, focus_node, inner, visited) {
                ShExStatus::Conformant => {
                    ShExStatus::NonConformant("NOT constraint violated: shape matched".to_string())
                }
                ShExStatus::NonConformant(_) => ShExStatus::Conformant,
            }
        }

        ShapeExpr::ShapeRef(iri) => {
            let key = (focus_node.to_string(), iri.clone());
            if visited.contains(&key) {
                // Recursive reference — assume conformant (optimistic)
                debug!(
                    "Recursive shape reference detected: {} @ {}",
                    focus_node, iri
                );
                return ShExStatus::Conformant;
            }
            visited.insert(key);

            match schema.find_shape(iri) {
                Some(decl) => {
                    evaluate_shape_expr(store, schema, focus_node, &decl.shape_expr, visited)
                }
                None => ShExStatus::NonConformant(format!("Referenced shape <{}> not found", iri)),
            }
        }

        ShapeExpr::NodeConstraintAny => ShExStatus::Conformant,
    }
}

/// Evaluate a Shape body (triple expression + CLOSED/EXTRA).
fn evaluate_shape(
    store: &TripleStore,
    schema: &ShExSchema,
    focus_node: &str,
    expression: &Option<TripleExpr>,
    closed: bool,
    extra: &[String],
    visited: &mut HashSet<(String, String)>,
) -> ShExStatus {
    // Get all outgoing triples for the focus node
    let outgoing = get_outgoing_triples(store, focus_node);

    if let Some(te) = expression {
        // Track which triples are consumed by the triple expression
        let mut consumed = HashSet::new();
        let result = evaluate_triple_expr(
            store,
            schema,
            focus_node,
            te,
            &outgoing,
            &mut consumed,
            visited,
        );

        if let ShExStatus::NonConformant(msg) = result {
            return ShExStatus::NonConformant(msg);
        }

        // CLOSED check: all unconsumed triples must have predicates in EXTRA
        if closed {
            for (i, (pred, _)) in outgoing.iter().enumerate() {
                if !consumed.contains(&i) && !extra.contains(pred) {
                    return ShExStatus::NonConformant(format!(
                        "CLOSED shape violation: unexpected predicate <{}>",
                        pred
                    ));
                }
            }
        }
    }

    ShExStatus::Conformant
}

/// Get outgoing triples (predicate, object) for a focus node.
fn get_outgoing_triples(store: &TripleStore, focus_node: &str) -> Vec<(String, String)> {
    let clean_node = focus_node
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string();

    let mut triples = Vec::new();
    if let Ok(subj) = NamedNodeRef::new(clean_node.as_str()) {
        for quad in store
            .store()
            .quads_for_pattern(Some(subj.into()), None, None, None)
        {
            if let Ok(q) = quad {
                triples.push((q.predicate.to_string(), q.object.to_string()));
            }
        }
    }
    triples
}

/// Evaluate a triple expression against outgoing triples.
fn evaluate_triple_expr(
    store: &TripleStore,
    schema: &ShExSchema,
    focus_node: &str,
    te: &TripleExpr,
    outgoing: &[(String, String)],
    consumed: &mut HashSet<usize>,
    visited: &mut HashSet<(String, String)>,
) -> ShExStatus {
    match te {
        TripleExpr::TripleConstraint {
            predicate,
            inverse,
            value_expr,
            min,
            max,
            ..
        } => {
            if *inverse {
                // Inverse: check incoming triples
                return evaluate_inverse_constraint(
                    store, schema, focus_node, predicate, value_expr, *min, max, visited,
                );
            }

            // Find matching triples
            let pred_str = format!(
                "<{}>",
                predicate.trim_start_matches('<').trim_end_matches('>')
            );
            let pred_bare = predicate.trim_start_matches('<').trim_end_matches('>');
            let mut match_count = 0;

            for (i, (p, obj)) in outgoing.iter().enumerate() {
                let p_clean = p.trim_start_matches('<').trim_end_matches('>');
                if p_clean == pred_bare || *p == pred_str || *p == *predicate {
                    // Check value constraint if present
                    if let Some(ve) = value_expr {
                        let obj_status = evaluate_shape_expr(store, schema, obj, ve, visited);
                        if matches!(obj_status, ShExStatus::Conformant) {
                            match_count += 1;
                            consumed.insert(i);
                        }
                    } else {
                        match_count += 1;
                        consumed.insert(i);
                    }
                }
            }

            // Check cardinality
            if match_count < *min {
                return ShExStatus::NonConformant(format!(
                    "Cardinality violation for <{}>: found {} but minimum is {}",
                    predicate, match_count, min
                ));
            }
            if !max.allows(match_count) {
                return ShExStatus::NonConformant(format!(
                    "Cardinality violation for <{}>: found {} but maximum is {:?}",
                    predicate, match_count, max
                ));
            }

            ShExStatus::Conformant
        }

        TripleExpr::EachOf(exprs) => {
            for e in exprs {
                let status =
                    evaluate_triple_expr(store, schema, focus_node, e, outgoing, consumed, visited);
                if let ShExStatus::NonConformant(msg) = status {
                    return ShExStatus::NonConformant(msg);
                }
            }
            ShExStatus::Conformant
        }

        TripleExpr::OneOf(exprs) => {
            for e in exprs {
                let mut local_consumed = consumed.clone();
                let status = evaluate_triple_expr(
                    store,
                    schema,
                    focus_node,
                    e,
                    outgoing,
                    &mut local_consumed,
                    visited,
                );
                if matches!(status, ShExStatus::Conformant) {
                    *consumed = local_consumed;
                    return ShExStatus::Conformant;
                }
            }
            ShExStatus::NonConformant("None of the OneOf alternatives matched".to_string())
        }
    }
}

/// Evaluate an inverse triple constraint (^ prefix).
fn evaluate_inverse_constraint(
    store: &TripleStore,
    schema: &ShExSchema,
    focus_node: &str,
    predicate: &str,
    value_expr: &Option<Box<ShapeExpr>>,
    min: usize,
    max: &Cardinality,
    visited: &mut HashSet<(String, String)>,
) -> ShExStatus {
    let clean_node = focus_node
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string();
    let pred_clean = predicate.trim_start_matches('<').trim_end_matches('>');

    let mut match_count = 0;

    if let (Ok(pred_ref), Ok(obj_ref)) = (
        NamedNodeRef::new(pred_clean),
        NamedNodeRef::new(clean_node.as_str()),
    ) {
        for quad in
            store
                .store()
                .quads_for_pattern(None, Some(pred_ref), Some(obj_ref.into()), None)
        {
            if let Ok(q) = quad {
                if let Some(ve) = value_expr {
                    let subj_str = q.subject.to_string();
                    if matches!(
                        evaluate_shape_expr(store, schema, &subj_str, ve, visited),
                        ShExStatus::Conformant
                    ) {
                        match_count += 1;
                    }
                } else {
                    match_count += 1;
                }
            }
        }
    }

    if match_count < min {
        return ShExStatus::NonConformant(format!(
            "Inverse cardinality violation for ^<{}>: found {} but minimum is {}",
            predicate, match_count, min
        ));
    }
    if !max.allows(match_count) {
        return ShExStatus::NonConformant(format!(
            "Inverse cardinality violation for ^<{}>: found {} but maximum is {:?}",
            predicate, match_count, max
        ));
    }

    ShExStatus::Conformant
}

/// Evaluate a node constraint against a focus node value.
fn evaluate_node_constraint(
    _store: &TripleStore,
    focus_node: &str,
    nc: &NodeConstraint,
) -> ShExStatus {
    // Node kind check
    if let Some(ref nk) = nc.node_kind {
        let is_iri = focus_node.starts_with('<') || focus_node.starts_with("http");
        let is_bnode = focus_node.starts_with("_:");
        let is_literal = focus_node.starts_with('"');

        match nk {
            NodeKind::IRI if !is_iri => {
                return ShExStatus::NonConformant(format!("Expected IRI but got: {}", focus_node));
            }
            NodeKind::BNode if !is_bnode => {
                return ShExStatus::NonConformant(format!(
                    "Expected BNode but got: {}",
                    focus_node
                ));
            }
            NodeKind::Literal if !is_literal => {
                return ShExStatus::NonConformant(format!(
                    "Expected Literal but got: {}",
                    focus_node
                ));
            }
            NodeKind::NonLiteral if is_literal => {
                return ShExStatus::NonConformant(format!(
                    "Expected NonLiteral but got: {}",
                    focus_node
                ));
            }
            _ => {}
        }
    }

    // Datatype check
    if let Some(ref dt) = nc.datatype {
        if focus_node.contains("^^") {
            let dt_clean = dt.trim_start_matches('<').trim_end_matches('>');
            if !focus_node.contains(dt_clean) {
                return ShExStatus::NonConformant(format!(
                    "Expected datatype <{}> but got: {}",
                    dt, focus_node
                ));
            }
        }
    }

    // String facets
    let lexical = extract_lexical(focus_node);
    for facet in &nc.string_facets {
        match facet {
            StringFacet::MinLength(n) if lexical.len() < *n => {
                return ShExStatus::NonConformant(format!(
                    "String too short: {} < {}",
                    lexical.len(),
                    n
                ));
            }
            StringFacet::MaxLength(n) if lexical.len() > *n => {
                return ShExStatus::NonConformant(format!(
                    "String too long: {} > {}",
                    lexical.len(),
                    n
                ));
            }
            StringFacet::Length(n) if lexical.len() != *n => {
                return ShExStatus::NonConformant(format!(
                    "String length mismatch: {} != {}",
                    lexical.len(),
                    n
                ));
            }
            StringFacet::Pattern(pat, _flags) => {
                // Simple substring match for basic patterns; full regex
                // support would require adding the `regex` crate as a dependency.
                if !lexical.contains(pat.as_str()) {
                    return ShExStatus::NonConformant(format!(
                        "Pattern '{}' did not match: {}",
                        pat, lexical
                    ));
                }
            }
            _ => {}
        }
    }

    // Value set check
    if !nc.values.is_empty() && !nc.values.contains(&focus_node.to_string()) {
        return ShExStatus::NonConformant(format!("Value {} not in allowed set", focus_node));
    }

    ShExStatus::Conformant
}

/// Extract the lexical form from an RDF term string.
fn extract_lexical(term: &str) -> String {
    if term.starts_with('"') {
        // Literal: extract content between quotes
        if let Some(end) = term[1..].find('"') {
            return term[1..=end].to_string();
        }
    }
    // For IRIs, use the full string
    term.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_constraint_iri() {
        let store = TripleStore::in_memory().unwrap();
        let nc = NodeConstraint {
            node_kind: Some(NodeKind::IRI),
            ..Default::default()
        };
        assert!(matches!(
            evaluate_node_constraint(&store, "<http://example.org/foo>", &nc),
            ShExStatus::Conformant
        ));
        assert!(matches!(
            evaluate_node_constraint(&store, "\"hello\"", &nc),
            ShExStatus::NonConformant(_)
        ));
    }

    #[test]
    fn test_node_constraint_literal() {
        let store = TripleStore::in_memory().unwrap();
        let nc = NodeConstraint {
            node_kind: Some(NodeKind::Literal),
            ..Default::default()
        };
        assert!(matches!(
            evaluate_node_constraint(&store, "\"hello\"", &nc),
            ShExStatus::Conformant
        ));
        assert!(matches!(
            evaluate_node_constraint(&store, "<http://example.org/foo>", &nc),
            ShExStatus::NonConformant(_)
        ));
    }

    #[test]
    fn test_extract_lexical() {
        assert_eq!(extract_lexical("\"hello\""), "hello");
        assert_eq!(
            extract_lexical("<http://example.org/>"),
            "<http://example.org/>"
        );
    }
}
