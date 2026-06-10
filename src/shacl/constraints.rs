use super::engine::FocusKind;
use super::report::{Severity, ValidationResult};
use super::shapes::*;
use crate::store::{escape_sparql_iri, TripleStore};
use std::collections::BTreeSet;

/// Maximum nesting depth for recursive shape evaluation (sh:node / sh:and / sh:or
/// / sh:xone / sh:not / sh:qualifiedValueShape). A shapes graph with a cycle
/// (e.g. shape A `sh:node` B and B `sh:node` A) would otherwise recurse without
/// bound and overflow the (rayon worker) thread stack, aborting the whole process
/// — a remote, no-payload DoS reachable by anyone who can author shapes. The SHACL
/// spec leaves recursion implementation-defined; refusing past this depth is safe.
const MAX_SHACL_SHAPE_DEPTH: u32 = 50;

thread_local! {
    static SHACL_SHAPE_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// RAII guard that increments the per-thread shape-recursion depth on entry and
/// decrements it on drop. `ok()` is false once the depth limit is exceeded.
struct ShapeDepthGuard;
impl ShapeDepthGuard {
    fn enter() -> (Self, bool) {
        let depth = SHACL_SHAPE_DEPTH.with(|d| {
            let v = d.get() + 1;
            d.set(v);
            v
        });
        (ShapeDepthGuard, depth <= MAX_SHACL_SHAPE_DEPTH)
    }
}
impl Drop for ShapeDepthGuard {
    fn drop(&mut self) {
        SHACL_SHAPE_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Validate all constraints of an inline `Shape` against `focus_node` and return violations.
///
/// Used by logical constraint operators (sh:not, sh:and, sh:or, sh:xone) and sh:node.
fn validate_inline_shape(
    store: &TripleStore,
    shapes: &[Shape],
    focus_node: &str,
    shape: &Shape,
    data_graphs: &[String],
    severity: &Severity,
) -> Vec<ValidationResult> {
    // Bound recursion so a cyclic shapes graph cannot overflow the stack.
    let (_depth_guard, within_limit) = ShapeDepthGuard::enter();
    if !within_limit {
        tracing::warn!(
            shape = %shape.iri,
            "SHACL shape recursion exceeded max depth {}; refusing to recurse further \
             (possible cyclic sh:node / logical-shape reference)",
            MAX_SHACL_SHAPE_DEPTH
        );
        return Vec::new();
    }

    let mut results = Vec::new();
    let shape_iri = &shape.iri;

    // Inline shapes validate *value nodes*, whose term kind was never recorded
    // (they come from lexical value lookups) — node-kind checks fall back to
    // the lexical heuristic.
    for constraint in &shape.constraints {
        results.extend(evaluate_constraint(
            store,
            shapes,
            shape_iri,
            focus_node,
            FocusKind::Unknown,
            constraint,
            None,
            data_graphs,
            severity,
        ));
    }

    for prop_shape in &shape.property_shapes {
        let ps_iri = prop_shape.iri.as_deref().unwrap_or(shape_iri);
        for constraint in &prop_shape.constraints {
            results.extend(evaluate_constraint(
                store,
                shapes,
                ps_iri,
                focus_node,
                FocusKind::Unknown,
                constraint,
                Some(&prop_shape.path),
                data_graphs,
                severity,
            ));
        }
    }

    results
}

/// Try to parse a string as f64 for numeric range comparisons.
fn parse_numeric(s: &str) -> Option<f64> {
    s.parse::<f64>().ok()
}

/// Evaluate a constraint against a focus node. `focus_kind` is the focus
/// node's term kind when target resolution could record it (see
/// [`FocusKind`]); `FocusKind::Unknown` falls back to lexical heuristics.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_constraint(
    store: &TripleStore,
    shapes: &[Shape],
    shape_iri: &str,
    focus_node: &str,
    focus_kind: FocusKind,
    constraint: &Constraint,
    path: Option<&PropertyPath>,
    data_graphs: &[String],
    severity: &Severity,
) -> Vec<ValidationResult> {
    let mut results = Vec::new();

    match constraint {
        Constraint::Class(class_iri) => {
            let values = get_values(store, focus_node, path, data_graphs);
            if values.is_empty() && path.is_none() {
                // Node shape class constraint: check if focus node is instance of class.
                // Scope to the data graphs — the instance triple lives there, not in
                // the (unscoped) default graph; an unscoped ASK made every class check
                // fail for datasets that use named graphs (e.g. sh:qualifiedValueShape).
                let query = format!(
                    "ASK {{ {} }}",
                    super::engine::graph_scoped(
                        data_graphs,
                        &format!(
                            "<{}> a <{}>",
                            escape_sparql_iri(focus_node),
                            escape_sparql_iri(class_iri)
                        )
                    )
                );
                if let Ok(oxigraph::sparql::QueryResults::Boolean(is_instance)) =
                    store.query(&query)
                {
                    if !is_instance {
                        results.push(ValidationResult {
                            severity: severity.clone(),
                            focus_node: focus_node.to_string(),
                            path: path.map(|p| p.to_sparql()),
                            value: None,
                            source_shape: shape_iri.to_string(),
                            source_constraint: format!("sh:class <{}>", class_iri),
                            message: format!("Value does not have class <{}>", class_iri),
                        });
                    }
                }
            } else {
                for value in &values {
                    if value.starts_with("http://")
                        || value.starts_with("https://")
                        || value.starts_with("urn:")
                    {
                        let query = format!(
                            "ASK {{ {} }}",
                            super::engine::graph_scoped(
                                data_graphs,
                                &format!(
                                    "<{}> a <{}>",
                                    escape_sparql_iri(value),
                                    escape_sparql_iri(class_iri)
                                )
                            )
                        );
                        if let Ok(oxigraph::sparql::QueryResults::Boolean(is_instance)) =
                            store.query(&query)
                        {
                            if !is_instance {
                                results.push(ValidationResult {
                                    severity: severity.clone(),
                                    focus_node: focus_node.to_string(),
                                    path: path.map(|p| p.to_sparql()),
                                    value: Some(value.clone()),
                                    source_shape: shape_iri.to_string(),
                                    source_constraint: format!("sh:class <{}>", class_iri),
                                    message: format!(
                                        "Value <{}> does not have class <{}>",
                                        value, class_iri
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }

        Constraint::Datatype(dt_iri) => {
            let values = get_value_terms(store, focus_node, path, data_graphs);
            for (value, datatype, _lang) in &values {
                if datatype.as_deref() != Some(dt_iri.as_str()) {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: Some(value.clone()),
                        source_shape: shape_iri.to_string(),
                        source_constraint: format!("sh:datatype <{}>", dt_iri),
                        message: format!("Value has wrong datatype, expected <{}>", dt_iri),
                    });
                }
            }
        }

        Constraint::NodeKind(expected) => {
            if path.is_none() {
                // Node-level: the focus node itself must match the kind. Target
                // resolution records the exact term kind where it can (class /
                // subjectsOf / objectsOf / SPARQL targets); a string literal
                // like "mailto:x@y.org" reached via sh:targetObjectsOf is then
                // classified as a literal instead of being mistaken for an IRI
                // by its scheme-shaped lexical form. Only when the kind is
                // genuinely unknown (sh:targetNode, inline value-node
                // recursion) do we fall back to the lexical heuristic: not a
                // blank node and not scheme-shaped ⇒ literal.
                let (is_iri, is_blank, is_literal) = match focus_kind {
                    FocusKind::Iri => (true, false, false),
                    FocusKind::BlankNode => (false, true, false),
                    FocusKind::Literal => (false, false, true),
                    FocusKind::Unknown => {
                        let is_blank = focus_node.starts_with("_:");
                        let is_iri = !is_blank && looks_like_iri(focus_node);
                        (is_iri, is_blank, !is_blank && !is_iri)
                    }
                };
                let is_valid = match expected {
                    NodeKind::IRI => is_iri,
                    NodeKind::BlankNode => is_blank,
                    NodeKind::Literal => is_literal,
                    NodeKind::BlankNodeOrIRI => is_blank || is_iri,
                    NodeKind::IRIOrLiteral => is_iri || is_literal,
                    NodeKind::BlankNodeOrLiteral => is_blank || is_literal,
                };
                if !is_valid {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: None,
                        value: None,
                        source_shape: shape_iri.to_string(),
                        source_constraint: format!("sh:nodeKind {:?}", expected),
                        message: format!(
                            "Focus node does not match expected node kind {:?}",
                            expected
                        ),
                    });
                }
            } else {
                // Property-level: every value at the path must match the kind.
                for (value, dt, _lang) in get_value_terms(store, focus_node, path, data_graphs) {
                    if !value_matches_node_kind(expected, &value, &dt) {
                        results.push(ValidationResult {
                            severity: severity.clone(),
                            focus_node: focus_node.to_string(),
                            path: path.map(|p| p.to_sparql()),
                            value: Some(value),
                            source_shape: shape_iri.to_string(),
                            source_constraint: format!("sh:nodeKind {:?}", expected),
                            message: format!(
                                "Value does not match expected node kind {:?}",
                                expected
                            ),
                        });
                    }
                }
            }
        }

        Constraint::MinCount(min) => {
            let values = get_values(store, focus_node, path, data_graphs);
            if values.len() < *min {
                results.push(ValidationResult {
                    severity: severity.clone(),
                    focus_node: focus_node.to_string(),
                    path: path.map(|p| p.to_sparql()),
                    value: None,
                    source_shape: shape_iri.to_string(),
                    source_constraint: format!("sh:minCount {}", min),
                    message: format!("Expected at least {} values, found {}", min, values.len()),
                });
            }
        }

        Constraint::MaxCount(max) => {
            let values = get_values(store, focus_node, path, data_graphs);
            if values.len() > *max {
                results.push(ValidationResult {
                    severity: severity.clone(),
                    focus_node: focus_node.to_string(),
                    path: path.map(|p| p.to_sparql()),
                    value: None,
                    source_shape: shape_iri.to_string(),
                    source_constraint: format!("sh:maxCount {}", max),
                    message: format!("Expected at most {} values, found {}", max, values.len()),
                });
            }
        }

        Constraint::MinLength(min_len) => {
            let values = get_values(store, focus_node, path, data_graphs);
            for value in &values {
                if value.len() < *min_len {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: Some(value.clone()),
                        source_shape: shape_iri.to_string(),
                        source_constraint: format!("sh:minLength {}", min_len),
                        message: format!(
                            "Value length {} is less than minimum {}",
                            value.len(),
                            min_len
                        ),
                    });
                }
            }
        }

        Constraint::MaxLength(max_len) => {
            let values = get_values(store, focus_node, path, data_graphs);
            for value in &values {
                if value.len() > *max_len {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: Some(value.clone()),
                        source_shape: shape_iri.to_string(),
                        source_constraint: format!("sh:maxLength {}", max_len),
                        message: format!(
                            "Value length {} exceeds maximum {}",
                            value.len(),
                            max_len
                        ),
                    });
                }
            }
        }

        Constraint::Pattern { pattern, flags } => {
            // DoS bounds: a shape's `sh:pattern` is attacker-controllable, and this
            // evaluates one SPARQL ASK per value. Cap the pattern length and the
            // number of values so a shape targeting a huge class can't fan out into
            // unbounded query work. (The regex engine itself is linear.)
            const MAX_PATTERN_LEN: usize = 1000;
            const MAX_PATTERN_VALUES: usize = 10_000;
            if pattern.len() > MAX_PATTERN_LEN {
                results.push(ValidationResult {
                    severity: severity.clone(),
                    focus_node: focus_node.to_string(),
                    path: path.map(|p| p.to_sparql()),
                    value: None,
                    source_shape: shape_iri.to_string(),
                    source_constraint: "sh:pattern".to_string(),
                    message: "sh:pattern is too long to evaluate".to_string(),
                });
                return results;
            }
            let values = get_values(store, focus_node, path, data_graphs);
            for value in values.iter().take(MAX_PATTERN_VALUES) {
                let regex_flags = flags.as_deref().unwrap_or("");
                // Escape BOTH backslash and quote: a trailing `\` would otherwise
                // escape the closing quote and corrupt the query (and `\d`-style
                // regex escapes need `\\` in the SPARQL string literal anyway).
                let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
                let query = format!(
                    "ASK {{ FILTER(REGEX(\"{}\", \"{}\", \"{}\")) }}",
                    esc(value),
                    esc(pattern),
                    regex_flags.replace(['\\', '"'], "")
                );
                if let Ok(oxigraph::sparql::QueryResults::Boolean(matches)) = store.query(&query) {
                    if !matches {
                        results.push(ValidationResult {
                            severity: severity.clone(),
                            focus_node: focus_node.to_string(),
                            path: path.map(|p| p.to_sparql()),
                            value: Some(value.clone()),
                            source_shape: shape_iri.to_string(),
                            source_constraint: format!("sh:pattern \"{}\"", pattern),
                            message: format!("Value does not match pattern \"{}\"", pattern),
                        });
                    }
                }
            }
        }

        Constraint::HasValue(expected) => {
            let values = get_values(store, focus_node, path, data_graphs);
            let found = values.iter().any(|v| v == expected);
            if !found {
                results.push(ValidationResult {
                    severity: severity.clone(),
                    focus_node: focus_node.to_string(),
                    path: path.map(|p| p.to_sparql()),
                    value: None,
                    source_shape: shape_iri.to_string(),
                    source_constraint: format!("sh:hasValue {}", expected),
                    message: format!("Missing required value: {}", expected),
                });
            }
        }

        Constraint::In(allowed) => {
            let values = get_values(store, focus_node, path, data_graphs);
            for value in &values {
                if !allowed.contains(value) {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: Some(value.clone()),
                        source_shape: shape_iri.to_string(),
                        source_constraint: "sh:in".to_string(),
                        message: format!("Value \"{}\" is not in the allowed list", value),
                    });
                }
            }
        }

        Constraint::UniqueLang(unique) => {
            if *unique {
                let value_terms = get_value_terms(store, focus_node, path, data_graphs);
                let mut seen_langs: Vec<String> = Vec::new();
                for (_value, _dt, lang) in &value_terms {
                    if let Some(lang) = lang {
                        if seen_langs.contains(lang) {
                            results.push(ValidationResult {
                                severity: severity.clone(),
                                focus_node: focus_node.to_string(),
                                path: path.map(|p| p.to_sparql()),
                                value: None,
                                source_shape: shape_iri.to_string(),
                                source_constraint: "sh:uniqueLang true".to_string(),
                                message: format!("Duplicate language tag: {}", lang),
                            });
                        }
                        seen_langs.push(lang.clone());
                    }
                }
            }
        }

        Constraint::LanguageIn(allowed_langs) => {
            let value_terms = get_value_terms(store, focus_node, path, data_graphs);
            for (value, _dt, lang) in &value_terms {
                match lang {
                    Some(l) if allowed_langs.iter().any(|al| l.starts_with(al.as_str())) => {}
                    _ => {
                        results.push(ValidationResult {
                            severity: severity.clone(),
                            focus_node: focus_node.to_string(),
                            path: path.map(|p| p.to_sparql()),
                            value: Some(value.clone()),
                            source_shape: shape_iri.to_string(),
                            source_constraint: "sh:languageIn".to_string(),
                            message: "Language tag not in allowed list".to_string(),
                        });
                    }
                }
            }
        }

        Constraint::Closed { ignored_properties } => {
            // Get all predicates used by the focus node
            let query = format!("SELECT DISTINCT ?p WHERE {{ <{}> ?p ?o }}", focus_node);
            if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&query) {
                for solution in solutions.filter_map(|s| s.ok()) {
                    if let Some(p) = solution.get("p") {
                        let p_str = match p {
                            oxigraph::model::Term::NamedNode(nn) => nn.as_str().to_string(),
                            _ => continue,
                        };
                        // Check if this property is declared in any property shape or in ignored list
                        if !ignored_properties.contains(&p_str) {
                            results.push(ValidationResult {
                                severity: severity.clone(),
                                focus_node: focus_node.to_string(),
                                path: Some(p_str.clone()),
                                value: None,
                                source_shape: shape_iri.to_string(),
                                source_constraint: "sh:closed true".to_string(),
                                message: format!(
                                    "Property <{}> is not allowed by closed shape",
                                    p_str
                                ),
                            });
                        }
                    }
                }
            }
        }

        Constraint::SparqlConstraint {
            select,
            message,
            severity: severity_override,
        } => {
            // A sh:severity on the SPARQLConstraint node overrides the shape's severity.
            let eff_severity = severity_override
                .as_deref()
                .map(Severity::from_iri)
                .unwrap_or_else(|| severity.clone());
            // SHACL-SPARQL: execute the SELECT with $this PRE-BOUND to the focus
            // node; each result row is a violation. $this must be bound (not
            // textually replaced by `<iri>`), otherwise it cannot appear in the
            // SELECT projection or GROUP BY of an aggregate query — `SELECT <iri>`
            // / `GROUP BY <iri>` is invalid SPARQL. We therefore rewrite `$this`
            // to `?this` and inject `VALUES ?this { <focus> }` into the WHERE block.
            let query = bind_this(select, focus_node, data_graphs);
            if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&query) {
                for solution in solutions.filter_map(|s| s.ok()) {
                    let msg = message.as_deref().unwrap_or("SPARQL constraint violated");
                    let value = solution.get("value").map(|v| v.to_string());
                    let path_val = solution.get("path").map(|v| v.to_string());

                    results.push(ValidationResult {
                        severity: eff_severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path_val.or_else(|| path.map(|p| p.to_sparql())),
                        value,
                        source_shape: shape_iri.to_string(),
                        source_constraint: "sh:SPARQLConstraint".to_string(),
                        message: msg.to_string(),
                    });
                }
            }
        }

        // ---- SHACL-AF node expression (path + comparison subset) ----
        Constraint::Expression {
            path: expr_path,
            checks,
            message,
        } => {
            // Evaluate the inner comparison constraints against the values reached
            // along the expression path; any inner violation fails the expression.
            let mut inner = Vec::new();
            for check in checks {
                inner.extend(evaluate_constraint(
                    store,
                    shapes,
                    shape_iri,
                    focus_node,
                    focus_kind,
                    check,
                    Some(expr_path),
                    data_graphs,
                    severity,
                ));
            }
            if !inner.is_empty() {
                results.push(ValidationResult {
                    severity: severity.clone(),
                    focus_node: focus_node.to_string(),
                    path: Some(expr_path.to_sparql()),
                    value: inner.into_iter().next().and_then(|r| r.value),
                    source_shape: shape_iri.to_string(),
                    source_constraint: "sh:expression".to_string(),
                    message: message
                        .clone()
                        .unwrap_or_else(|| "sh:expression constraint not satisfied".to_string()),
                });
            }
        }

        // ---- Numeric range constraints ----
        Constraint::MinExclusive(min_val) => {
            let values = get_value_terms(store, focus_node, path, data_graphs);
            for (value, _dt, _lang) in &values {
                let ok = match (parse_numeric(value), parse_numeric(min_val)) {
                    (Some(v), Some(m)) => v > m,
                    _ => true, // non-numeric: skip
                };
                if !ok {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: Some(value.clone()),
                        source_shape: shape_iri.to_string(),
                        source_constraint: format!("sh:minExclusive {}", min_val),
                        message: format!("Value {} is not > {}", value, min_val),
                    });
                }
            }
        }

        Constraint::MinInclusive(min_val) => {
            let values = get_value_terms(store, focus_node, path, data_graphs);
            for (value, _dt, _lang) in &values {
                let ok = match (parse_numeric(value), parse_numeric(min_val)) {
                    (Some(v), Some(m)) => v >= m,
                    _ => true,
                };
                if !ok {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: Some(value.clone()),
                        source_shape: shape_iri.to_string(),
                        source_constraint: format!("sh:minInclusive {}", min_val),
                        message: format!("Value {} is not >= {}", value, min_val),
                    });
                }
            }
        }

        Constraint::MaxExclusive(max_val) => {
            let values = get_value_terms(store, focus_node, path, data_graphs);
            for (value, _dt, _lang) in &values {
                let ok = match (parse_numeric(value), parse_numeric(max_val)) {
                    (Some(v), Some(m)) => v < m,
                    _ => true,
                };
                if !ok {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: Some(value.clone()),
                        source_shape: shape_iri.to_string(),
                        source_constraint: format!("sh:maxExclusive {}", max_val),
                        message: format!("Value {} is not < {}", value, max_val),
                    });
                }
            }
        }

        Constraint::MaxInclusive(max_val) => {
            let values = get_value_terms(store, focus_node, path, data_graphs);
            for (value, _dt, _lang) in &values {
                let ok = match (parse_numeric(value), parse_numeric(max_val)) {
                    (Some(v), Some(m)) => v <= m,
                    _ => true,
                };
                if !ok {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: Some(value.clone()),
                        source_shape: shape_iri.to_string(),
                        source_constraint: format!("sh:maxInclusive {}", max_val),
                        message: format!("Value {} is not <= {}", value, max_val),
                    });
                }
            }
        }

        // ---- Property comparison constraints ----
        Constraint::Equals(prop_iri) => {
            let path_values: BTreeSet<String> = get_values(store, focus_node, path, data_graphs)
                .into_iter()
                .collect();
            let other_path = PropertyPath::Predicate(prop_iri.clone());
            let other_values: BTreeSet<String> =
                get_values(store, focus_node, Some(&other_path), data_graphs)
                    .into_iter()
                    .collect();
            if path_values != other_values {
                results.push(ValidationResult {
                    severity: severity.clone(),
                    focus_node: focus_node.to_string(),
                    path: path.map(|p| p.to_sparql()),
                    value: None,
                    source_shape: shape_iri.to_string(),
                    source_constraint: format!("sh:equals <{}>", prop_iri),
                    message: format!(
                        "Value set at path does not equal value set at <{}>",
                        prop_iri
                    ),
                });
            }
        }

        Constraint::Disjoint(prop_iri) => {
            let path_values: BTreeSet<String> = get_values(store, focus_node, path, data_graphs)
                .into_iter()
                .collect();
            let other_path = PropertyPath::Predicate(prop_iri.clone());
            let other_values: BTreeSet<String> =
                get_values(store, focus_node, Some(&other_path), data_graphs)
                    .into_iter()
                    .collect();
            let intersection: BTreeSet<_> = path_values.intersection(&other_values).collect();
            for v in intersection {
                results.push(ValidationResult {
                    severity: severity.clone(),
                    focus_node: focus_node.to_string(),
                    path: path.map(|p| p.to_sparql()),
                    value: Some(v.clone()),
                    source_shape: shape_iri.to_string(),
                    source_constraint: format!("sh:disjoint <{}>", prop_iri),
                    message: format!("Value \"{}\" appears in both path and <{}>", v, prop_iri),
                });
            }
        }

        Constraint::LessThan(prop_iri) => {
            let path_values = get_values(store, focus_node, path, data_graphs);
            let other_path = PropertyPath::Predicate(prop_iri.clone());
            let other_values = get_values(store, focus_node, Some(&other_path), data_graphs);
            for pv in &path_values {
                for ov in &other_values {
                    let violated = match (parse_numeric(pv), parse_numeric(ov)) {
                        (Some(a), Some(b)) => a >= b,
                        _ => pv >= ov, // lexicographic fallback
                    };
                    if violated {
                        results.push(ValidationResult {
                            severity: severity.clone(),
                            focus_node: focus_node.to_string(),
                            path: path.map(|p| p.to_sparql()),
                            value: Some(pv.clone()),
                            source_shape: shape_iri.to_string(),
                            source_constraint: format!("sh:lessThan <{}>", prop_iri),
                            message: format!(
                                "Value {} is not < {} (value at <{}>)",
                                pv, ov, prop_iri
                            ),
                        });
                    }
                }
            }
        }

        Constraint::LessThanOrEquals(prop_iri) => {
            let path_values = get_values(store, focus_node, path, data_graphs);
            let other_path = PropertyPath::Predicate(prop_iri.clone());
            let other_values = get_values(store, focus_node, Some(&other_path), data_graphs);
            for pv in &path_values {
                for ov in &other_values {
                    let violated = match (parse_numeric(pv), parse_numeric(ov)) {
                        (Some(a), Some(b)) => a > b,
                        _ => pv > ov,
                    };
                    if violated {
                        results.push(ValidationResult {
                            severity: severity.clone(),
                            focus_node: focus_node.to_string(),
                            path: path.map(|p| p.to_sparql()),
                            value: Some(pv.clone()),
                            source_shape: shape_iri.to_string(),
                            source_constraint: format!("sh:lessThanOrEquals <{}>", prop_iri),
                            message: format!(
                                "Value {} is not <= {} (value at <{}>)",
                                pv, ov, prop_iri
                            ),
                        });
                    }
                }
            }
        }

        // ---- Logical constraints ----
        // In a property-shape context these apply to EACH VALUE NODE along the
        // path (SHACL §4.6); only in a node-shape context (no path) do they apply
        // to the focus node itself. Results keep the original focus node and
        // carry the offending value in sh:value.
        Constraint::Not(inner_shape) => {
            for value in value_nodes(store, focus_node, path, data_graphs) {
                // The value must NOT conform; zero inner violations → violation.
                let inner_violations = validate_inline_shape(
                    store,
                    shapes,
                    &value,
                    inner_shape,
                    data_graphs,
                    severity,
                );
                if inner_violations.is_empty() {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: value_field(&value, focus_node),
                        source_shape: shape_iri.to_string(),
                        source_constraint: "sh:not".to_string(),
                        message: "Value conforms to sh:not shape (must not conform)".to_string(),
                    });
                }
            }
        }

        Constraint::And(inner_shapes) => {
            // Every value must conform to ALL inner shapes; one violation per
            // value that fails any of them.
            for value in value_nodes(store, focus_node, path, data_graphs) {
                let fails = inner_shapes.iter().any(|inner| {
                    !validate_inline_shape(store, shapes, &value, inner, data_graphs, severity)
                        .is_empty()
                });
                if fails {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: value_field(&value, focus_node),
                        source_shape: shape_iri.to_string(),
                        source_constraint: "sh:and".to_string(),
                        message: "Value does not conform to all sh:and shapes".to_string(),
                    });
                }
            }
        }

        Constraint::Or(inner_shapes) => {
            // Every value must conform to at least one inner shape.
            for value in value_nodes(store, focus_node, path, data_graphs) {
                let any_conforms = inner_shapes.iter().any(|inner| {
                    validate_inline_shape(store, shapes, &value, inner, data_graphs, severity)
                        .is_empty()
                });
                if !any_conforms {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: value_field(&value, focus_node),
                        source_shape: shape_iri.to_string(),
                        source_constraint: "sh:or".to_string(),
                        message: "Value does not conform to any sh:or shape".to_string(),
                    });
                }
            }
        }

        Constraint::Xone(inner_shapes) => {
            // Every value must conform to exactly one inner shape.
            for value in value_nodes(store, focus_node, path, data_graphs) {
                let conforming_count = inner_shapes
                    .iter()
                    .filter(|inner| {
                        validate_inline_shape(store, shapes, &value, inner, data_graphs, severity)
                            .is_empty()
                    })
                    .count();
                if conforming_count != 1 {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: value_field(&value, focus_node),
                        source_shape: shape_iri.to_string(),
                        source_constraint: "sh:xone".to_string(),
                        message: format!(
                            "Value conforms to {} sh:xone shapes, expected exactly 1",
                            conforming_count
                        ),
                    });
                }
            }
        }

        // ---- Shape reference constraint ----
        Constraint::Node(ref_shape_iri) => {
            // Look up the referenced shape in the loaded shapes collection.
            if let Some(ref_shape) = shapes.iter().find(|s| &s.iri == ref_shape_iri) {
                let ref_shape = ref_shape.clone();
                // Each value node must conform to the referenced shape; one
                // violation per non-conforming value (sh:node, SHACL §4.6.3).
                for value in value_nodes(store, focus_node, path, data_graphs) {
                    let inner = validate_inline_shape(
                        store,
                        shapes,
                        &value,
                        &ref_shape,
                        data_graphs,
                        severity,
                    );
                    if !inner.is_empty() {
                        results.push(ValidationResult {
                            severity: severity.clone(),
                            focus_node: focus_node.to_string(),
                            path: path.map(|p| p.to_sparql()),
                            value: value_field(&value, focus_node),
                            source_shape: shape_iri.to_string(),
                            source_constraint: format!("sh:node <{}>", ref_shape_iri),
                            message: format!("Value does not conform to shape <{}>", ref_shape_iri),
                        });
                    }
                }
            }
            // If shape not found, skip silently (may be in a different shapes graph).
        }

        // ---- Qualified value shape ----
        Constraint::QualifiedValueShape {
            shape: qvs,
            min_count,
            max_count,
        } => {
            // Collect values along the path; count those that conform to the (inline)
            // qualified value shape.
            let values = get_values(store, focus_node, path, data_graphs);
            let conforming_count = values
                .iter()
                .filter(|v| {
                    validate_inline_shape(store, shapes, v, qvs, data_graphs, severity).is_empty()
                })
                .count();

            if let Some(min) = min_count {
                if conforming_count < *min {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: None,
                        source_shape: shape_iri.to_string(),
                        source_constraint: format!("sh:qualifiedMinCount {}", min),
                        message: format!(
                            "Only {} values conform to qualified shape, expected at least {}",
                            conforming_count, min
                        ),
                    });
                }
            }
            if let Some(max) = max_count {
                if conforming_count > *max {
                    results.push(ValidationResult {
                        severity: severity.clone(),
                        focus_node: focus_node.to_string(),
                        path: path.map(|p| p.to_sparql()),
                        value: None,
                        source_shape: shape_iri.to_string(),
                        source_constraint: format!("sh:qualifiedMaxCount {}", max),
                        message: format!(
                            "{} values conform to qualified shape, expected at most {}",
                            conforming_count, max
                        ),
                    });
                }
            }
        }
    }

    results
}

/// Pre-bind SHACL's `$this` to the focus node for a SPARQL-based constraint.
///
/// `$this`/`?this` is rewritten to `?this` and bound via a `VALUES` clause
/// injected at the start of the outermost `WHERE { … }` block, so it works in the
/// SELECT projection and `GROUP BY` of aggregate validators — unlike textual
/// `<iri>` substitution, which yields invalid SPARQL (`SELECT <iri>` /
/// `GROUP BY <iri>`).
fn bind_this(select: &str, focus_node: &str, data_graphs: &[String]) -> String {
    let with_var = select.replace("$this", "?this");
    let upper = with_var.to_uppercase();
    let (where_pos, brace_at) = match upper
        .find("WHERE")
        .and_then(|wp| with_var[wp..].find('{').map(|br| (wp, wp + br + 1)))
    {
        Some(v) => v,
        // No WHERE block to rewrite into: fall back to textual IRI substitution.
        None => return select.replace("$this", &format!("<{}>", focus_node)),
    };
    // `FROM <g>` makes the data graphs the query's default graph — SHACL-SPARQL
    // evaluates the constraint against the data graph, so default-graph patterns
    // like `?this ex:p ?v` must resolve there rather than the (empty) default graph.
    let from: String = data_graphs.iter().map(|g| format!("FROM <{g}> ")).collect();
    // `VALUES` pre-binds $this to the focus node (usable in SELECT/GROUP BY).
    let values = format!("VALUES ?this {{ <{}> }} ", focus_node);
    let mut q = String::with_capacity(with_var.len() + from.len() + values.len() + 2);
    q.push_str(&with_var[..where_pos]);
    q.push_str(&from);
    q.push_str(&with_var[where_pos..brace_at]);
    q.push(' ');
    q.push_str(&values);
    q.push_str(&with_var[brace_at..]);
    q
}

/// The value nodes a (possibly path-less) constraint applies to: the values
/// along the path in a property-shape context, or the focus node itself in a
/// node-shape context (SHACL §3.4). Distinct — SHACL counts value *nodes*, not
/// SPARQL path bindings, so duplicates from diamond-shaped paths collapse.
fn value_nodes(
    store: &TripleStore,
    focus_node: &str,
    path: Option<&PropertyPath>,
    data_graphs: &[String],
) -> Vec<String> {
    match path {
        None => vec![focus_node.to_string()],
        Some(_) => {
            let mut vals = get_values(store, focus_node, path, data_graphs);
            let mut seen = BTreeSet::new();
            vals.retain(|v| seen.insert(v.clone()));
            vals
        }
    }
}

/// `sh:value` for a result: the offending value node in property context, or
/// `None` when the "value" is just the focus node itself (node context).
fn value_field(value: &str, focus_node: &str) -> Option<String> {
    (value != focus_node).then(|| value.to_string())
}

/// Get the string values for a focus node along a property path.
fn get_values(
    store: &TripleStore,
    focus_node: &str,
    path: Option<&PropertyPath>,
    data_graphs: &[String],
) -> Vec<String> {
    let path = match path {
        Some(p) => p,
        None => return vec![focus_node.to_string()],
    };

    // Blank-node focus: SPARQL cannot address a specific stored blank node, so
    // resolve a simple predicate path through the raw quad index instead. (A
    // complex path from a blank focus falls through to the SPARQL branch, which
    // only matches IRI subjects — an accepted, rare limitation.)
    if let Some(label) = focus_node.strip_prefix("_:") {
        if let PropertyPath::Predicate(pred) = path {
            return store
                .blank_subject_objects(label, pred, data_graphs)
                .iter()
                .map(term_to_value_string)
                .collect();
        }
    }

    let path_sparql = path.to_sparql();

    // Check the per-thread path cache before executing a SPARQL query
    if let Some(cached) =
        crate::store::path_cache::tl_get(store.cache_id(), focus_node, &path_sparql)
    {
        return cached;
    }

    let query = format!(
        "SELECT ?value WHERE {{ {} }}",
        super::engine::graph_scoped(data_graphs, &format!("<{focus_node}> {path_sparql} ?value"))
    );

    let results =
        if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&query) {
            solutions
                .filter_map(|s| s.ok())
                .filter_map(|s| {
                    s.get("value").map(|v| match v {
                        oxigraph::model::Term::NamedNode(nn) => nn.as_str().to_string(),
                        oxigraph::model::Term::Literal(lit) => lit.value().to_string(),
                        oxigraph::model::Term::BlankNode(bn) => format!("_:{}", bn.as_str()),
                        _ => v.to_string(),
                    })
                })
                .collect()
        } else {
            Vec::new()
        };

    crate::store::path_cache::tl_insert(
        store.cache_id(),
        focus_node.to_string(),
        path_sparql,
        results.clone(),
    );
    results
}

/// Get value terms with datatype and language information.
fn get_value_terms(
    store: &TripleStore,
    focus_node: &str,
    path: Option<&PropertyPath>,
    data_graphs: &[String],
) -> Vec<(String, Option<String>, Option<String>)> {
    let path = match path {
        Some(p) => p,
        None => return Vec::new(),
    };

    // Blank-node focus: see `get_values` — resolve a simple predicate path via
    // the raw quad index, since SPARQL cannot reference a stored blank node.
    if let Some(label) = focus_node.strip_prefix("_:") {
        if let PropertyPath::Predicate(pred) = path {
            return store
                .blank_subject_objects(label, pred, data_graphs)
                .iter()
                .map(term_to_value_triple)
                .collect();
        }
    }

    let path_sparql = path.to_sparql();
    let query = format!(
        "SELECT ?value (DATATYPE(?value) AS ?dt) (LANG(?value) AS ?lang) WHERE {{ {} }}",
        super::engine::graph_scoped(data_graphs, &format!("<{focus_node}> {path_sparql} ?value"))
    );

    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&query) {
        solutions
            .filter_map(|s| s.ok())
            .map(|s| {
                let value = s
                    .get("value")
                    .map(|v| match v {
                        oxigraph::model::Term::Literal(lit) => lit.value().to_string(),
                        oxigraph::model::Term::NamedNode(nn) => nn.as_str().to_string(),
                        _ => v.to_string(),
                    })
                    .unwrap_or_default();

                let dt = s.get("dt").and_then(|v| match v {
                    oxigraph::model::Term::NamedNode(nn) => Some(nn.as_str().to_string()),
                    _ => None,
                });

                let lang = s.get("lang").and_then(|v| match v {
                    oxigraph::model::Term::Literal(lit) => {
                        let l = lit.value().to_string();
                        if l.is_empty() {
                            None
                        } else {
                            Some(l)
                        }
                    }
                    _ => None,
                });

                (value, dt, lang)
            })
            .collect()
    } else {
        Vec::new()
    }
}

/// Stringify a value term the same way the SPARQL value lookups do: IRIs and
/// literal lexical forms as-is, blank nodes as `_:label`.
fn term_to_value_string(term: &oxigraph::model::Term) -> String {
    match term {
        oxigraph::model::Term::NamedNode(nn) => nn.as_str().to_string(),
        oxigraph::model::Term::Literal(lit) => lit.value().to_string(),
        oxigraph::model::Term::BlankNode(bn) => format!("_:{}", bn.as_str()),
        other => other.to_string(),
    }
}

/// `(value, datatype, lang)` for a value term, mirroring the SPARQL
/// `DATATYPE()`/`LANG()` projection: a literal carries its datatype (and lang
/// for language-tagged literals); IRIs and blank nodes carry neither.
fn term_to_value_triple(term: &oxigraph::model::Term) -> (String, Option<String>, Option<String>) {
    match term {
        oxigraph::model::Term::NamedNode(nn) => (nn.as_str().to_string(), None, None),
        oxigraph::model::Term::BlankNode(bn) => (format!("_:{}", bn.as_str()), None, None),
        oxigraph::model::Term::Literal(lit) => {
            let lang = lit.language().map(|l| l.to_string());
            let dt = Some(lit.datatype().as_str().to_string());
            (lit.value().to_string(), dt, lang)
        }
        other => (other.to_string(), None, None),
    }
}

/// Scheme-shaped heuristic for classifying a lexical focus node as an IRI:
/// `scheme:rest` with an alpha-led scheme and no whitespace. Misclassifies rare
/// literals like bare time strings ("12:30:00"), which is accepted — the
/// alternative (treating every focus as non-literal) failed all node-level
/// `sh:nodeKind sh:Literal` checks.
fn looks_like_iri(s: &str) -> bool {
    if s.contains(char::is_whitespace) {
        return false;
    }
    match s.split_once(':') {
        Some((scheme, _)) => {
            !scheme.is_empty()
                && scheme
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        }
        None => false,
    }
}

/// Whether a value term (as produced by `get_value_terms`) matches an expected
/// node kind. A datatype marks a literal; a `_:` prefix marks a blank node;
/// anything else is treated as an IRI.
fn value_matches_node_kind(expected: &NodeKind, value: &str, dt: &Option<String>) -> bool {
    let is_literal = dt.is_some();
    let is_blank = !is_literal && value.starts_with("_:");
    let is_iri = !is_literal && !is_blank;
    match expected {
        NodeKind::IRI => is_iri,
        NodeKind::BlankNode => is_blank,
        NodeKind::Literal => is_literal,
        NodeKind::BlankNodeOrIRI => is_blank || is_iri,
        NodeKind::IRIOrLiteral => is_iri || is_literal,
        NodeKind::BlankNodeOrLiteral => is_blank || is_literal,
    }
}
