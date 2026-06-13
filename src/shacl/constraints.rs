use super::report::{Severity, ValidationResult};
use super::shapes::*;
use crate::store::TripleStore;
use oxigraph::model::{GraphNameRef, Literal, NamedNodeRef, SubjectRef, Term};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_SUBCLASS: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";

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

/// Display form used in validation reports: bare IRI for named nodes, lexical
/// value for literals, `_:label` for blank nodes. This is the historical report
/// format — the HTTP layer and UI consume these strings, so it must not change.
pub fn display_term(term: &Term) -> String {
    match term {
        Term::NamedNode(nn) => nn.as_str().to_string(),
        Term::Literal(lit) => lit.value().to_string(),
        Term::BlankNode(bn) => format!("_:{}", bn.as_str()),
        other => other.to_string(),
    }
}

/// Validate all constraints of an inline `Shape` against `focus_node` and return violations.
///
/// Used by logical constraint operators (sh:not, sh:and, sh:or, sh:xone), sh:node
/// and sh:qualifiedValueShape.
fn validate_inline_shape(
    store: &TripleStore,
    shapes: &[Shape],
    focus_node: &Term,
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

    for constraint in &shape.constraints {
        results.extend(evaluate_constraint(
            store,
            shapes,
            shape_iri,
            focus_node,
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
                constraint,
                Some(&prop_shape.path),
                data_graphs,
                severity,
            ));
        }
    }

    results
}

/// Evaluate a constraint against a (typed) focus node.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_constraint(
    store: &TripleStore,
    shapes: &[Shape],
    shape_iri: &str,
    focus_node: &Term,
    constraint: &Constraint,
    path: Option<&PropertyPath>,
    data_graphs: &[String],
    severity: &Severity,
) -> Vec<ValidationResult> {
    let mut results = Vec::new();
    let focus_str = display_term(focus_node);
    let path_str = || path.map(|p| p.to_sparql());
    // sh:value for value-node-oriented results (SHACL sets it to the offending
    // value node — the focus itself in a node-shape context).
    let mk = |value: Option<String>,
              path: Option<String>,
              source_constraint: String,
              message: String|
     -> ValidationResult {
        ValidationResult {
            severity: severity.clone(),
            focus_node: focus_str.clone(),
            path,
            value,
            source_shape: shape_iri.to_string(),
            source_constraint,
            message,
        }
    };

    match constraint {
        Constraint::Class(class_iri) => {
            // Every value node must be a SHACL instance of the class
            // (rdf:type/rdfs:subClassOf*). Literals are never instances.
            for v in value_nodes(store, focus_node, path, data_graphs) {
                if !is_instance_of(store, &v, class_iri, data_graphs) {
                    results.push(mk(
                        Some(display_term(&v)),
                        path_str(),
                        format!("sh:class <{}>", class_iri),
                        format!("Value does not have class <{}>", class_iri),
                    ));
                }
            }
        }

        Constraint::Datatype(dt_iri) => {
            // The value must be a literal whose datatype IRI matches AND whose
            // lexical form is valid for that datatype (ill-formed literals like
            // "aldi"^^xsd:integer violate sh:datatype — SHACL §4.1.2).
            for v in value_nodes(store, focus_node, path, data_graphs) {
                let ok = match &v {
                    Term::Literal(lit) => {
                        lit.datatype().as_str() == dt_iri.as_str() && xsd_lexical_valid(lit)
                    }
                    _ => false,
                };
                if !ok {
                    results.push(mk(
                        Some(display_term(&v)),
                        path_str(),
                        format!("sh:datatype <{}>", dt_iri),
                        format!("Value has wrong datatype, expected <{}>", dt_iri),
                    ));
                }
            }
        }

        Constraint::NodeKind(expected) => {
            for v in value_nodes(store, focus_node, path, data_graphs) {
                let (is_iri, is_blank, is_literal) = match &v {
                    Term::NamedNode(_) => (true, false, false),
                    Term::BlankNode(_) => (false, true, false),
                    Term::Literal(_) => (false, false, true),
                    _ => (false, false, false),
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
                    results.push(mk(
                        Some(display_term(&v)),
                        path_str(),
                        format!("sh:nodeKind {:?}", expected),
                        format!("Value does not match expected node kind {:?}", expected),
                    ));
                }
            }
        }

        Constraint::MinCount(min) => {
            let count = value_nodes(store, focus_node, path, data_graphs).len();
            if count < *min {
                results.push(mk(
                    None,
                    path_str(),
                    format!("sh:minCount {}", min),
                    format!("Expected at least {} values, found {}", min, count),
                ));
            }
        }

        Constraint::MaxCount(max) => {
            let count = value_nodes(store, focus_node, path, data_graphs).len();
            if count > *max {
                results.push(mk(
                    None,
                    path_str(),
                    format!("sh:maxCount {}", max),
                    format!("Expected at most {} values, found {}", max, count),
                ));
            }
        }

        Constraint::MinLength(min_len) => {
            for v in value_nodes(store, focus_node, path, data_graphs) {
                // sh:minLength applies to the string representation of the value:
                // literals by lexical form, IRIs by IRI string; blank nodes always violate.
                let ok = match string_repr(&v) {
                    Some(s) => s.chars().count() >= *min_len,
                    None => false,
                };
                if !ok {
                    results.push(mk(
                        Some(display_term(&v)),
                        path_str(),
                        format!("sh:minLength {}", min_len),
                        format!("Value length is less than minimum {}", min_len),
                    ));
                }
            }
        }

        Constraint::MaxLength(max_len) => {
            for v in value_nodes(store, focus_node, path, data_graphs) {
                let ok = match string_repr(&v) {
                    Some(s) => s.chars().count() <= *max_len,
                    None => false,
                };
                if !ok {
                    results.push(mk(
                        Some(display_term(&v)),
                        path_str(),
                        format!("sh:maxLength {}", max_len),
                        format!("Value length exceeds maximum {}", max_len),
                    ));
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
                results.push(mk(
                    None,
                    path_str(),
                    "sh:pattern".to_string(),
                    "sh:pattern is too long to evaluate".to_string(),
                ));
                return results;
            }
            for v in value_nodes(store, focus_node, path, data_graphs)
                .into_iter()
                .take(MAX_PATTERN_VALUES)
            {
                // Blank nodes always violate sh:pattern (SHACL §4.4.2).
                let Some(value) = string_repr(&v) else {
                    results.push(mk(
                        Some(display_term(&v)),
                        path_str(),
                        format!("sh:pattern \"{}\"", pattern),
                        format!("Value does not match pattern \"{}\"", pattern),
                    ));
                    continue;
                };
                let regex_flags = flags.as_deref().unwrap_or("");
                // Escape BOTH backslash and quote: a trailing `\` would otherwise
                // escape the closing quote and corrupt the query (and `\d`-style
                // regex escapes need `\\` in the SPARQL string literal anyway).
                let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
                let query = format!(
                    "ASK {{ FILTER(REGEX(\"{}\", \"{}\", \"{}\")) }}",
                    esc(&value),
                    esc(pattern),
                    regex_flags.replace(['\\', '"'], "")
                );
                if let Ok(oxigraph::sparql::QueryResults::Boolean(matches)) = store.query(&query) {
                    if !matches {
                        results.push(mk(
                            Some(value.clone()),
                            path_str(),
                            format!("sh:pattern \"{}\"", pattern),
                            format!("Value does not match pattern \"{}\"", pattern),
                        ));
                    }
                }
            }
        }

        Constraint::HasValue(expected) => {
            let values = value_nodes(store, focus_node, path, data_graphs);
            if !values.iter().any(|v| v == expected) {
                results.push(mk(
                    None,
                    path_str(),
                    format!("sh:hasValue {}", display_term(expected)),
                    format!("Missing required value: {}", display_term(expected)),
                ));
            }
        }

        Constraint::In(allowed) => {
            for v in value_nodes(store, focus_node, path, data_graphs) {
                if !allowed.iter().any(|a| a == &v) {
                    results.push(mk(
                        Some(display_term(&v)),
                        path_str(),
                        "sh:in".to_string(),
                        format!("Value \"{}\" is not in the allowed list", display_term(&v)),
                    ));
                }
            }
        }

        Constraint::UniqueLang(unique) => {
            if *unique {
                // One result per language tag carried by more than one value node.
                let mut langs: BTreeMap<String, usize> = BTreeMap::new();
                for v in value_nodes(store, focus_node, path, data_graphs) {
                    if let Term::Literal(lit) = &v {
                        if let Some(lang) = lit.language() {
                            *langs.entry(lang.to_ascii_lowercase()).or_insert(0) += 1;
                        }
                    }
                }
                for (lang, n) in langs {
                    if n > 1 {
                        results.push(mk(
                            None,
                            path_str(),
                            "sh:uniqueLang true".to_string(),
                            format!("Duplicate language tag: {}", lang),
                        ));
                    }
                }
            }
        }

        Constraint::LanguageIn(allowed_langs) => {
            for v in value_nodes(store, focus_node, path, data_graphs) {
                let lang_ok = match &v {
                    Term::Literal(lit) => lit
                        .language()
                        .map(|l| allowed_langs.iter().any(|al| lang_matches(l, al)))
                        .unwrap_or(false),
                    _ => false,
                };
                if !lang_ok {
                    results.push(mk(
                        Some(display_term(&v)),
                        path_str(),
                        "sh:languageIn".to_string(),
                        "Language tag not in allowed list".to_string(),
                    ));
                }
            }
        }

        Constraint::Closed {
            ignored_properties,
            allowed_properties,
        } => {
            // One result per (predicate, value) pair on the focus node whose
            // predicate is neither a declared property-shape path nor ignored.
            for (p, o) in subject_predicate_objects(store, focus_node, data_graphs) {
                if !ignored_properties.contains(&p) && !allowed_properties.contains(&p) {
                    results.push(mk(
                        Some(display_term(&o)),
                        Some(format!("<{}>", p)),
                        "sh:closed true".to_string(),
                        format!("Property <{}> is not allowed by closed shape", p),
                    ));
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
            // textually replaced), otherwise it cannot appear in the SELECT
            // projection or GROUP BY of an aggregate query. We therefore rewrite
            // `$this` to `?this` and inject `VALUES ?this { <focus> }`.
            // Blank-node focus nodes cannot be addressed from SPARQL — skip.
            if matches!(focus_node, Term::BlankNode(_)) {
                return results;
            }
            let query = bind_this(select, focus_node, data_graphs);
            if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&query) {
                for solution in solutions.filter_map(|s| s.ok()) {
                    let msg = message.as_deref().unwrap_or("SPARQL constraint violated");
                    let value = solution.get("value").map(|v| v.to_string());
                    let path_val = solution.get("path").map(|v| v.to_string());

                    results.push(ValidationResult {
                        severity: eff_severity.clone(),
                        focus_node: focus_str.clone(),
                        path: path_val.or_else(path_str),
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
                    check,
                    Some(expr_path),
                    data_graphs,
                    severity,
                ));
            }
            if !inner.is_empty() {
                results.push(mk(
                    inner.into_iter().next().and_then(|r| r.value),
                    Some(expr_path.to_sparql()),
                    "sh:expression".to_string(),
                    message
                        .clone()
                        .unwrap_or_else(|| "sh:expression constraint not satisfied".to_string()),
                ));
            }
        }

        // ---- Value range constraints ----
        // Violation unless the comparison is *definitively* satisfied: literals of
        // incomparable types, IRIs and blank nodes all violate (SHACL §4.3).
        Constraint::MinExclusive(bound) => {
            for v in value_nodes(store, focus_node, path, data_graphs) {
                if !matches!(compare_terms(&v, bound), Some(Ordering::Greater)) {
                    results.push(mk(
                        Some(display_term(&v)),
                        path_str(),
                        format!("sh:minExclusive {}", display_term(bound)),
                        format!(
                            "Value {} is not > {}",
                            display_term(&v),
                            display_term(bound)
                        ),
                    ));
                }
            }
        }

        Constraint::MinInclusive(bound) => {
            for v in value_nodes(store, focus_node, path, data_graphs) {
                if !matches!(
                    compare_terms(&v, bound),
                    Some(Ordering::Greater | Ordering::Equal)
                ) {
                    results.push(mk(
                        Some(display_term(&v)),
                        path_str(),
                        format!("sh:minInclusive {}", display_term(bound)),
                        format!(
                            "Value {} is not >= {}",
                            display_term(&v),
                            display_term(bound)
                        ),
                    ));
                }
            }
        }

        Constraint::MaxExclusive(bound) => {
            for v in value_nodes(store, focus_node, path, data_graphs) {
                if !matches!(compare_terms(&v, bound), Some(Ordering::Less)) {
                    results.push(mk(
                        Some(display_term(&v)),
                        path_str(),
                        format!("sh:maxExclusive {}", display_term(bound)),
                        format!(
                            "Value {} is not < {}",
                            display_term(&v),
                            display_term(bound)
                        ),
                    ));
                }
            }
        }

        Constraint::MaxInclusive(bound) => {
            for v in value_nodes(store, focus_node, path, data_graphs) {
                if !matches!(
                    compare_terms(&v, bound),
                    Some(Ordering::Less | Ordering::Equal)
                ) {
                    results.push(mk(
                        Some(display_term(&v)),
                        path_str(),
                        format!("sh:maxInclusive {}", display_term(bound)),
                        format!(
                            "Value {} is not <= {}",
                            display_term(&v),
                            display_term(bound)
                        ),
                    ));
                }
            }
        }

        // ---- Property pair constraints ----
        Constraint::Equals(prop_iri) => {
            // One result per value in the symmetric difference of the two value sets.
            let path_values = term_set(value_nodes(store, focus_node, path, data_graphs));
            let other_path = PropertyPath::Predicate(prop_iri.clone());
            let other_values = term_set(value_nodes(
                store,
                focus_node,
                Some(&other_path),
                data_graphs,
            ));
            for (_, v) in path_values
                .iter()
                .filter(|(k, _)| !other_values.contains_key(*k))
                .chain(
                    other_values
                        .iter()
                        .filter(|(k, _)| !path_values.contains_key(*k)),
                )
            {
                results.push(mk(
                    Some(display_term(v)),
                    path_str(),
                    format!("sh:equals <{}>", prop_iri),
                    format!(
                        "Value set at path does not equal value set at <{}>",
                        prop_iri
                    ),
                ));
            }
        }

        Constraint::Disjoint(prop_iri) => {
            let path_values = term_set(value_nodes(store, focus_node, path, data_graphs));
            let other_path = PropertyPath::Predicate(prop_iri.clone());
            let other_values = term_set(value_nodes(
                store,
                focus_node,
                Some(&other_path),
                data_graphs,
            ));
            for (_, v) in path_values
                .iter()
                .filter(|(k, _)| other_values.contains_key(*k))
            {
                results.push(mk(
                    Some(display_term(v)),
                    path_str(),
                    format!("sh:disjoint <{}>", prop_iri),
                    format!(
                        "Value \"{}\" appears in both path and <{}>",
                        display_term(v),
                        prop_iri
                    ),
                ));
            }
        }

        Constraint::LessThan(prop_iri) => {
            let path_values = value_nodes(store, focus_node, path, data_graphs);
            let other_path = PropertyPath::Predicate(prop_iri.clone());
            let other_values = value_nodes(store, focus_node, Some(&other_path), data_graphs);
            for pv in &path_values {
                for ov in &other_values {
                    // Violated unless definitively pv < ov (incomparable pairs violate).
                    if !matches!(compare_terms(pv, ov), Some(Ordering::Less)) {
                        results.push(mk(
                            Some(display_term(pv)),
                            path_str(),
                            format!("sh:lessThan <{}>", prop_iri),
                            format!(
                                "Value {} is not < {} (value at <{}>)",
                                display_term(pv),
                                display_term(ov),
                                prop_iri
                            ),
                        ));
                    }
                }
            }
        }

        Constraint::LessThanOrEquals(prop_iri) => {
            let path_values = value_nodes(store, focus_node, path, data_graphs);
            let other_path = PropertyPath::Predicate(prop_iri.clone());
            let other_values = value_nodes(store, focus_node, Some(&other_path), data_graphs);
            for pv in &path_values {
                for ov in &other_values {
                    if !matches!(
                        compare_terms(pv, ov),
                        Some(Ordering::Less | Ordering::Equal)
                    ) {
                        results.push(mk(
                            Some(display_term(pv)),
                            path_str(),
                            format!("sh:lessThanOrEquals <{}>", prop_iri),
                            format!(
                                "Value {} is not <= {} (value at <{}>)",
                                display_term(pv),
                                display_term(ov),
                                prop_iri
                            ),
                        ));
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
                    results.push(mk(
                        Some(display_term(&value)),
                        path_str(),
                        "sh:not".to_string(),
                        "Value conforms to sh:not shape (must not conform)".to_string(),
                    ));
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
                    results.push(mk(
                        Some(display_term(&value)),
                        path_str(),
                        "sh:and".to_string(),
                        "Value does not conform to all sh:and shapes".to_string(),
                    ));
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
                    results.push(mk(
                        Some(display_term(&value)),
                        path_str(),
                        "sh:or".to_string(),
                        "Value does not conform to any sh:or shape".to_string(),
                    ));
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
                    results.push(mk(
                        Some(display_term(&value)),
                        path_str(),
                        "sh:xone".to_string(),
                        format!(
                            "Value conforms to {} sh:xone shapes, expected exactly 1",
                            conforming_count
                        ),
                    ));
                }
            }
        }

        // ---- Shape reference constraint ----
        Constraint::Node(ref_shape) => {
            // Each value node must conform to the referenced shape; one
            // violation per non-conforming value (sh:node, SHACL §4.6.3).
            for value in value_nodes(store, focus_node, path, data_graphs) {
                let inner =
                    validate_inline_shape(store, shapes, &value, ref_shape, data_graphs, severity);
                if !inner.is_empty() {
                    results.push(mk(
                        Some(display_term(&value)),
                        path_str(),
                        format!("sh:node <{}>", ref_shape.iri),
                        format!("Value does not conform to shape <{}>", ref_shape.iri),
                    ));
                }
            }
        }

        // ---- Nested property shape (sh:property on a property shape) ----
        Constraint::Property(inner_ps) => {
            // Each value node along the outer path becomes the focus node of the
            // nested property shape (SHACL §2.1.3).
            let inner_iri = inner_ps.iri.as_deref().unwrap_or(shape_iri);
            for value in value_nodes(store, focus_node, path, data_graphs) {
                for c in &inner_ps.constraints {
                    results.extend(evaluate_constraint(
                        store,
                        shapes,
                        inner_iri,
                        &value,
                        c,
                        Some(&inner_ps.path),
                        data_graphs,
                        severity,
                    ));
                }
            }
        }

        // ---- Qualified value shape ----
        Constraint::QualifiedValueShape {
            shape: qvs,
            min_count,
            max_count,
            disjoint,
            sibling_shapes,
        } => {
            // Count the values along the path that conform to the qualified value
            // shape; with sh:qualifiedValueShapesDisjoint, values conforming to a
            // sibling property shape's qualified value shape are excluded.
            let values = value_nodes(store, focus_node, path, data_graphs);
            let conforming_count = values
                .iter()
                .filter(|v| {
                    validate_inline_shape(store, shapes, v, qvs, data_graphs, severity).is_empty()
                        && !(*disjoint
                            && sibling_shapes.iter().any(|sib| {
                                validate_inline_shape(store, shapes, v, sib, data_graphs, severity)
                                    .is_empty()
                            }))
                })
                .count();

            if let Some(min) = min_count {
                if conforming_count < *min {
                    results.push(mk(
                        None,
                        path_str(),
                        format!("sh:qualifiedMinCount {}", min),
                        format!(
                            "Only {} values conform to qualified shape, expected at least {}",
                            conforming_count, min
                        ),
                    ));
                }
            }
            if let Some(max) = max_count {
                if conforming_count > *max {
                    results.push(mk(
                        None,
                        path_str(),
                        format!("sh:qualifiedMaxCount {}", max),
                        format!(
                            "{} values conform to qualified shape, expected at most {}",
                            conforming_count, max
                        ),
                    ));
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
/// substitution, which yields invalid SPARQL (`SELECT <iri>` / `GROUP BY <iri>`).
fn bind_this(select: &str, focus_node: &Term, data_graphs: &[String]) -> String {
    // N-Triples serialisation is valid in VALUES for IRIs and literals.
    let focus_nt = focus_node.to_string();
    let with_var = select.replace("$this", "?this");
    let upper = with_var.to_uppercase();
    let (where_pos, brace_at) = match upper
        .find("WHERE")
        .and_then(|wp| with_var[wp..].find('{').map(|br| (wp, wp + br + 1)))
    {
        Some(v) => v,
        // No WHERE block to rewrite into: fall back to textual substitution.
        None => return select.replace("$this", &focus_nt),
    };
    // `FROM <g>` makes the data graphs the query's default graph — SHACL-SPARQL
    // evaluates the constraint against the data graph, so default-graph patterns
    // like `?this ex:p ?v` must resolve there rather than the (empty) default graph.
    let from: String = data_graphs.iter().map(|g| format!("FROM <{g}> ")).collect();
    // `VALUES` pre-binds $this to the focus node (usable in SELECT/GROUP BY).
    let values = format!("VALUES ?this {{ {} }} ", focus_nt);
    let mut q = String::with_capacity(with_var.len() + from.len() + values.len() + 2);
    q.push_str(&with_var[..where_pos]);
    q.push_str(&from);
    q.push_str(&with_var[where_pos..brace_at]);
    q.push(' ');
    q.push_str(&values);
    q.push_str(&with_var[brace_at..]);
    q
}

// ---------------------------------------------------------------------------
// Value-node resolution
// ---------------------------------------------------------------------------

/// The value nodes a (possibly path-less) constraint applies to: the values
/// along the path in a property-shape context, or the focus node itself in a
/// node-shape context (SHACL §3.4). Distinct — SHACL value nodes form a *set*,
/// so duplicate bindings from diamond-shaped paths collapse.
fn value_nodes(
    store: &TripleStore,
    focus_node: &Term,
    path: Option<&PropertyPath>,
    data_graphs: &[String],
) -> Vec<Term> {
    match path {
        None => vec![focus_node.clone()],
        Some(p) => get_path_values(store, focus_node, p, data_graphs),
    }
}

/// Distinct set of terms keyed by N-Triples form (Term is not Ord).
fn term_set(values: Vec<Term>) -> BTreeMap<String, Term> {
    values.into_iter().map(|t| (t.to_string(), t)).collect()
}

/// Resolve the (distinct) value nodes along `path` from `focus`.
///
/// IRI focus nodes go through a single SPARQL property-path query (with the
/// per-thread path cache); blank-node and literal focus nodes — which SPARQL
/// surface syntax cannot address — are walked natively over the raw quad index.
fn get_path_values(
    store: &TripleStore,
    focus: &Term,
    path: &PropertyPath,
    data_graphs: &[String],
) -> Vec<Term> {
    if let Term::NamedNode(nn) = focus {
        let path_sparql = path.to_sparql();

        // Check the per-thread path cache (stores N-Triples forms) first.
        if let Some(cached) =
            crate::store::path_cache::tl_get(store.cache_id(), nn.as_str(), &path_sparql)
        {
            return cached
                .iter()
                .filter_map(|s| Term::from_str(s).ok())
                .collect();
        }

        let query = format!(
            "SELECT DISTINCT ?value WHERE {{ {} }}",
            super::engine::graph_scoped(
                data_graphs,
                &format!("<{}> {} ?value", nn.as_str(), path_sparql)
            )
        );
        let results: Vec<Term> =
            if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&query) {
                let mut seen = BTreeSet::new();
                solutions
                    .filter_map(|s| s.ok())
                    .filter_map(|s| s.get("value").cloned())
                    // DISTINCT already dedups within one branch; graph_scoped UNIONs
                    // can still produce cross-graph duplicates.
                    .filter(|t| seen.insert(t.to_string()))
                    .collect()
            } else {
                Vec::new()
            };

        crate::store::path_cache::tl_insert(
            store.cache_id(),
            nn.as_str().to_string(),
            path_sparql,
            results.iter().map(|t| t.to_string()).collect(),
        );
        return results;
    }

    // Blank-node (or literal) focus: native path evaluation.
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for t in eval_path_native(store, focus, path, data_graphs) {
        if seen.insert(t.to_string()) {
            out.push(t);
        }
    }
    out
}

/// Native SHACL path evaluation over the raw quad index, used for focus nodes
/// SPARQL cannot address (stored blank nodes, literals). Mirrors SPARQL property
/// path semantics, including the focus node itself for `zeroOrMore`/`zeroOrOne`.
fn eval_path_native(
    store: &TripleStore,
    from: &Term,
    path: &PropertyPath,
    data_graphs: &[String],
) -> Vec<Term> {
    match path {
        PropertyPath::Predicate(pred) => step(store, from, pred, false, data_graphs),
        PropertyPath::Inverse(inner) => match inner.as_ref() {
            PropertyPath::Predicate(pred) => step(store, from, pred, true, data_graphs),
            // Inverse of a composite path: push the inversion inwards.
            PropertyPath::Sequence(parts) => {
                let reversed = PropertyPath::Sequence(
                    parts
                        .iter()
                        .rev()
                        .map(|p| PropertyPath::Inverse(Box::new(p.clone())))
                        .collect(),
                );
                eval_path_native(store, from, &reversed, data_graphs)
            }
            PropertyPath::Alternative(parts) => parts
                .iter()
                .flat_map(|p| {
                    eval_path_native(
                        store,
                        from,
                        &PropertyPath::Inverse(Box::new(p.clone())),
                        data_graphs,
                    )
                })
                .collect(),
            PropertyPath::Inverse(inner2) => eval_path_native(store, from, inner2, data_graphs),
            PropertyPath::ZeroOrMore(p) => eval_path_native(
                store,
                from,
                &PropertyPath::ZeroOrMore(Box::new(PropertyPath::Inverse(p.clone()))),
                data_graphs,
            ),
            PropertyPath::OneOrMore(p) => eval_path_native(
                store,
                from,
                &PropertyPath::OneOrMore(Box::new(PropertyPath::Inverse(p.clone()))),
                data_graphs,
            ),
            PropertyPath::ZeroOrOne(p) => eval_path_native(
                store,
                from,
                &PropertyPath::ZeroOrOne(Box::new(PropertyPath::Inverse(p.clone()))),
                data_graphs,
            ),
        },
        PropertyPath::Sequence(parts) => {
            let mut frontier = vec![from.clone()];
            for part in parts {
                let mut next = Vec::new();
                let mut seen = BTreeSet::new();
                for node in &frontier {
                    for t in eval_path_native(store, node, part, data_graphs) {
                        if seen.insert(t.to_string()) {
                            next.push(t);
                        }
                    }
                }
                frontier = next;
                if frontier.is_empty() {
                    break;
                }
            }
            frontier
        }
        PropertyPath::Alternative(parts) => parts
            .iter()
            .flat_map(|p| eval_path_native(store, from, p, data_graphs))
            .collect(),
        PropertyPath::ZeroOrMore(inner) => closure(store, from, inner, true, data_graphs),
        PropertyPath::OneOrMore(inner) => closure(store, from, inner, false, data_graphs),
        PropertyPath::ZeroOrOne(inner) => {
            let mut out = vec![from.clone()];
            out.extend(eval_path_native(store, from, inner, data_graphs));
            out
        }
    }
}

/// Transitive closure of `inner` starting at `from` (BFS with a visited set);
/// `include_start` distinguishes `*` from `+`.
fn closure(
    store: &TripleStore,
    from: &Term,
    inner: &PropertyPath,
    include_start: bool,
    data_graphs: &[String],
) -> Vec<Term> {
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    visited.insert(from.to_string());
    if include_start {
        out.push(from.clone());
    }
    queue.push_back(from.clone());
    while let Some(node) = queue.pop_front() {
        for next in eval_path_native(store, &node, inner, data_graphs) {
            if visited.insert(next.to_string()) {
                out.push(next.clone());
                queue.push_back(next);
            }
        }
    }
    out
}

/// One forward (`from p ?o`) or inverse (`?s p from`) predicate step over the
/// raw quad index, scoped to the data graphs (empty = default graph).
fn step(
    store: &TripleStore,
    from: &Term,
    predicate: &str,
    inverse: bool,
    data_graphs: &[String],
) -> Vec<Term> {
    let Ok(pred) = NamedNodeRef::new(predicate) else {
        return Vec::new();
    };
    let raw = store.store();
    let mut out = Vec::new();
    let mut for_graph = |graph: GraphNameRef<'_>| {
        if inverse {
            let term_ref = from.as_ref();
            for q in raw
                .quads_for_pattern(None, Some(pred), Some(term_ref), Some(graph))
                .flatten()
            {
                match q.subject {
                    oxigraph::model::Subject::NamedNode(nn) => out.push(Term::NamedNode(nn)),
                    oxigraph::model::Subject::BlankNode(bn) => out.push(Term::BlankNode(bn)),
                    _ => {} // RDF-star subjects are out of scope here
                }
            }
        } else {
            let subj: SubjectRef<'_> = match from {
                Term::NamedNode(nn) => SubjectRef::NamedNode(nn.as_ref()),
                Term::BlankNode(bn) => SubjectRef::BlankNode(bn.as_ref()),
                _ => return, // literals have no outgoing edges
            };
            for q in raw
                .quads_for_pattern(Some(subj), Some(pred), None, Some(graph))
                .flatten()
            {
                out.push(q.object);
            }
        }
    };
    if data_graphs.is_empty() {
        for_graph(GraphNameRef::DefaultGraph);
    } else {
        for g in data_graphs {
            if let Ok(gn) = NamedNodeRef::new(g) {
                for_graph(GraphNameRef::NamedNode(gn));
            }
        }
    }
    out
}

/// All `(predicate, object)` pairs of `focus` in the data graphs — used by
/// `sh:closed`. Works for IRI and blank-node focus nodes alike.
fn subject_predicate_objects(
    store: &TripleStore,
    focus: &Term,
    data_graphs: &[String],
) -> Vec<(String, Term)> {
    let subj: SubjectRef<'_> = match focus {
        Term::NamedNode(nn) => SubjectRef::NamedNode(nn.as_ref()),
        Term::BlankNode(bn) => SubjectRef::BlankNode(bn.as_ref()),
        _ => return Vec::new(),
    };
    let raw = store.store();
    let mut out = Vec::new();
    let mut for_graph = |graph: GraphNameRef<'_>| {
        for q in raw
            .quads_for_pattern(Some(subj), None, None, Some(graph))
            .flatten()
        {
            out.push((q.predicate.as_str().to_string(), q.object));
        }
    };
    if data_graphs.is_empty() {
        for_graph(GraphNameRef::DefaultGraph);
    } else {
        for g in data_graphs {
            if let Ok(gn) = NamedNodeRef::new(g) {
                for_graph(GraphNameRef::NamedNode(gn));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Term classification & comparison
// ---------------------------------------------------------------------------

/// SHACL instance check with subclass closure: `term rdf:type/rdfs:subClassOf* class`.
/// Literals are never instances; blank-node focus types are resolved through the
/// raw quad index (SPARQL cannot re-address a stored blank node).
fn is_instance_of(
    store: &TripleStore,
    term: &Term,
    class_iri: &str,
    data_graphs: &[String],
) -> bool {
    match term {
        Term::Literal(_) => false,
        Term::NamedNode(nn) => {
            let query = format!(
                "ASK {{ {} }}",
                super::engine::graph_scoped(
                    data_graphs,
                    &format!(
                        "<{}> <{RDF_TYPE}>/<{RDFS_SUBCLASS}>* <{}>",
                        crate::store::escape_sparql_iri(nn.as_str()),
                        crate::store::escape_sparql_iri(class_iri)
                    )
                )
            );
            matches!(
                store.query(&query),
                Ok(oxigraph::sparql::QueryResults::Boolean(true))
            )
        }
        Term::BlankNode(_) => {
            // Types of the blank node, then subclass closure per type.
            for ty in step(store, term, RDF_TYPE, false, data_graphs) {
                if let Term::NamedNode(ty_nn) = ty {
                    if ty_nn.as_str() == class_iri {
                        return true;
                    }
                    let query = format!(
                        "ASK {{ {} }}",
                        super::engine::graph_scoped(
                            data_graphs,
                            &format!(
                                "<{}> <{RDFS_SUBCLASS}>* <{}>",
                                crate::store::escape_sparql_iri(ty_nn.as_str()),
                                crate::store::escape_sparql_iri(class_iri)
                            )
                        )
                    );
                    if matches!(
                        store.query(&query),
                        Ok(oxigraph::sparql::QueryResults::Boolean(true))
                    ) {
                        return true;
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// String representation for sh:minLength/maxLength/pattern: literals by lexical
/// form, IRIs by IRI string; blank nodes have none (and always violate).
fn string_repr(term: &Term) -> Option<String> {
    match term {
        Term::NamedNode(nn) => Some(nn.as_str().to_string()),
        Term::Literal(lit) => Some(lit.value().to_string()),
        _ => None,
    }
}

/// Basic language-range match for sh:languageIn: exact tag or a `tag-…` extension,
/// ASCII case-insensitive (BCP47).
fn lang_matches(lang: &str, range: &str) -> bool {
    if lang.len() == range.len() {
        return lang.eq_ignore_ascii_case(range);
    }
    lang.len() > range.len()
        && lang.as_bytes()[range.len()] == b'-'
        && lang[..range.len()].eq_ignore_ascii_case(range)
}

const NUMERIC_TYPES: &[&str] = &[
    "integer",
    "decimal",
    "float",
    "double",
    "long",
    "int",
    "short",
    "byte",
    "nonNegativeInteger",
    "nonPositiveInteger",
    "negativeInteger",
    "positiveInteger",
    "unsignedLong",
    "unsignedInt",
    "unsignedShort",
    "unsignedByte",
];

fn is_numeric_datatype(dt: &str) -> bool {
    dt.strip_prefix(XSD)
        .is_some_and(|local| NUMERIC_TYPES.contains(&local))
}

fn is_string_datatype(dt: &str) -> bool {
    dt == "http://www.w3.org/2001/XMLSchema#string"
}

/// Definite comparison of two terms per SPARQL operator semantics, with XSD
/// partial-order rules for dateTime/date mixing timezoned and naive values.
/// `None` = not definitively comparable (which range/pair constraints treat as
/// a violation). Only literals are comparable.
fn compare_terms(a: &Term, b: &Term) -> Option<Ordering> {
    let (Term::Literal(la), Term::Literal(lb)) = (a, b) else {
        return None;
    };
    let dta = la.datatype().as_str().to_string();
    let dtb = lb.datatype().as_str().to_string();

    if is_numeric_datatype(&dta) && is_numeric_datatype(&dtb) {
        let va: f64 = la.value().trim().parse().ok()?;
        let vb: f64 = lb.value().trim().parse().ok()?;
        return va.partial_cmp(&vb);
    }

    if dta == format!("{XSD}dateTime") && dtb == format!("{XSD}dateTime") {
        return cmp_temporal(
            parse_xsd_date_time(la.value())?,
            parse_xsd_date_time(lb.value())?,
        );
    }
    if dta == format!("{XSD}date") && dtb == format!("{XSD}date") {
        return cmp_temporal(parse_xsd_date(la.value())?, parse_xsd_date(lb.value())?);
    }

    if dta == format!("{XSD}boolean") && dtb == format!("{XSD}boolean") {
        let pb = |s: &str| match s {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        };
        return Some(pb(la.value())?.cmp(&pb(lb.value())?));
    }

    // Plain / xsd:string literals compare lexically (SPARQL `<`/`>` on strings).
    if is_string_datatype(&dta) && is_string_datatype(&dtb) {
        return Some(la.value().cmp(lb.value()));
    }

    None
}

/// XSD temporal comparison. Values are `(utc_epoch_seconds, has_timezone)`.
/// Same timezone-presence compares directly; mixed presence is definite only
/// when the values are more than ±14h apart (XSD 1.1 partial order).
fn cmp_temporal(a: (f64, bool), b: (f64, bool)) -> Option<Ordering> {
    if a.1 == b.1 {
        return a.0.partial_cmp(&b.0);
    }
    const WINDOW: f64 = 14.0 * 3600.0;
    let (alo, ahi) = if a.1 {
        (a.0, a.0)
    } else {
        (a.0 - WINDOW, a.0 + WINDOW)
    };
    let (blo, bhi) = if b.1 {
        (b.0, b.0)
    } else {
        (b.0 - WINDOW, b.0 + WINDOW)
    };
    if ahi < blo {
        Some(Ordering::Less)
    } else if alo > bhi {
        Some(Ordering::Greater)
    } else {
        None
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian civil date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse a timezone suffix (`Z` | `±HH:MM` | empty) → (offset_seconds, has_tz, rest_len_consumed_from_end).
fn split_timezone(s: &str) -> (f64, bool, &str) {
    if let Some(rest) = s.strip_suffix('Z') {
        return (0.0, true, rest);
    }
    if s.len() >= 6 {
        let (head, tz) = s.split_at(s.len() - 6);
        let bytes = tz.as_bytes();
        if (bytes[0] == b'+' || bytes[0] == b'-') && bytes[3] == b':' {
            if let (Ok(h), Ok(m)) = (tz[1..3].parse::<i64>(), tz[4..6].parse::<i64>()) {
                let sign = if bytes[0] == b'-' { -1.0 } else { 1.0 };
                return ((h * 3600 + m * 60) as f64 * sign, true, head);
            }
        }
    }
    (0.0, false, s)
}

/// Parse `YYYY-MM-DD` (no timezone handling — caller splits it off first).
fn parse_ymd(s: &str) -> Option<(i64, u32, u32)> {
    let mut parts = s.splitn(3, '-');
    // Negative years would produce an empty first segment; not supported.
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

/// Parse an `xsd:dateTime` lexical form → `(utc_epoch_seconds, has_timezone)`.
fn parse_xsd_date_time(s: &str) -> Option<(f64, bool)> {
    let (offset, has_tz, body) = split_timezone(s.trim());
    let (date, time) = body.split_once('T')?;
    let (y, m, d) = parse_ymd(date)?;
    let mut tparts = time.splitn(3, ':');
    let hh: u32 = tparts.next()?.parse().ok()?;
    let mm: u32 = tparts.next()?.parse().ok()?;
    let ss: f64 = tparts.next()?.parse().ok()?;
    if hh > 24 || mm > 59 || !(0.0..62.0).contains(&ss) {
        return None;
    }
    let epoch =
        days_from_civil(y, m, d) as f64 * 86_400.0 + hh as f64 * 3600.0 + mm as f64 * 60.0 + ss
            - offset;
    Some((epoch, has_tz))
}

/// Parse an `xsd:date` lexical form → `(utc_epoch_seconds_at_midnight, has_timezone)`.
fn parse_xsd_date(s: &str) -> Option<(f64, bool)> {
    let (offset, has_tz, body) = split_timezone(s.trim());
    let (y, m, d) = parse_ymd(body)?;
    Some((days_from_civil(y, m, d) as f64 * 86_400.0 - offset, has_tz))
}

/// Whether a literal's lexical form is valid for its (known XSD) datatype —
/// `"aldi"^^xsd:integer` and `"300"^^xsd:byte` are ill-formed and violate
/// `sh:datatype`. Unknown datatypes are assumed valid (the engine cannot judge).
fn xsd_lexical_valid(lit: &Literal) -> bool {
    let Some(local) = lit.datatype().as_str().strip_prefix(XSD) else {
        return true; // rdf:langString, rdf:HTML, custom datatypes: no lexical check
    };
    let v = lit.value();
    let int_in = |min: i128, max: i128| -> bool {
        v.parse::<i128>()
            .map(|n| n >= min && n <= max)
            .unwrap_or(false)
    };
    match local {
        "string" | "anyURI" | "normalizedString" | "token" | "language" | "Name" | "NCName"
        | "NMTOKEN" | "anySimpleType" | "hexBinary" | "base64Binary" | "duration" | "gYear"
        | "gYearMonth" | "gMonth" | "gMonthDay" | "gDay" | "QName" | "NOTATION" => true,
        "boolean" => matches!(v, "true" | "false" | "1" | "0"),
        "integer" => v.parse::<i128>().is_ok(),
        "nonNegativeInteger" => v.parse::<i128>().map(|n| n >= 0).unwrap_or(false),
        "positiveInteger" => v.parse::<i128>().map(|n| n > 0).unwrap_or(false),
        "nonPositiveInteger" => v.parse::<i128>().map(|n| n <= 0).unwrap_or(false),
        "negativeInteger" => v.parse::<i128>().map(|n| n < 0).unwrap_or(false),
        "long" => int_in(i64::MIN as i128, i64::MAX as i128),
        "int" => int_in(i32::MIN as i128, i32::MAX as i128),
        "short" => int_in(i16::MIN as i128, i16::MAX as i128),
        "byte" => int_in(i8::MIN as i128, i8::MAX as i128),
        "unsignedLong" => int_in(0, u64::MAX as i128),
        "unsignedInt" => int_in(0, u32::MAX as i128),
        "unsignedShort" => int_in(0, u16::MAX as i128),
        "unsignedByte" => int_in(0, u8::MAX as i128),
        "decimal" => {
            let t = v.strip_prefix(['+', '-']).unwrap_or(v);
            !t.is_empty()
                && t.chars().all(|c| c.is_ascii_digit() || c == '.')
                && t.matches('.').count() <= 1
                && t.chars().any(|c| c.is_ascii_digit())
        }
        "float" | "double" => {
            matches!(v, "NaN" | "INF" | "-INF" | "+INF") || v.parse::<f64>().is_ok()
        }
        "dateTime" => parse_xsd_date_time(v).is_some(),
        "date" => parse_xsd_date(v).is_some(),
        "time" => {
            let (_, _, body) = split_timezone(v.trim());
            let mut tparts = body.splitn(3, ':');
            (|| -> Option<()> {
                let hh: u32 = tparts.next()?.parse().ok()?;
                let mm: u32 = tparts.next()?.parse().ok()?;
                let ss: f64 = tparts.next()?.parse().ok()?;
                (hh <= 24 && mm <= 59 && (0.0..62.0).contains(&ss)).then_some(())
            })()
            .is_some()
        }
        _ => true,
    }
}
