use super::report::{ResultTerms, Severity, ValidationResult};
use super::shapes::*;
use super::view::{DataView, GraphSel};
use oxigraph::model::{Literal, Term};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

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

const SH_QUALIFIED_MAX_COUNT_COMPONENT: &str =
    "http://www.w3.org/ns/shacl#QualifiedMaxCountConstraintComponent";

/// The IRI of the constraint component a result of `constraint` reports as
/// `sh:sourceConstraintComponent` (SHACL §4, SHACL-AF §6 and §7.4). A
/// qualified value shape reports `sh:QualifiedMinCountConstraintComponent`
/// here; its max-count result names its own component at the call site.
/// `sh:property` nested in a property shape never yields a result of its
/// own (its constraints do), so it maps to the component for completeness.
pub(crate) fn component_iri(constraint: &Constraint) -> &str {
    match constraint {
        Constraint::Class(_) => "http://www.w3.org/ns/shacl#ClassConstraintComponent",
        Constraint::Datatype(_) => "http://www.w3.org/ns/shacl#DatatypeConstraintComponent",
        Constraint::NodeKind(_) => "http://www.w3.org/ns/shacl#NodeKindConstraintComponent",
        Constraint::MinCount(_) => "http://www.w3.org/ns/shacl#MinCountConstraintComponent",
        Constraint::MaxCount(_) => "http://www.w3.org/ns/shacl#MaxCountConstraintComponent",
        Constraint::MinExclusive(_) => "http://www.w3.org/ns/shacl#MinExclusiveConstraintComponent",
        Constraint::MinInclusive(_) => "http://www.w3.org/ns/shacl#MinInclusiveConstraintComponent",
        Constraint::MaxExclusive(_) => "http://www.w3.org/ns/shacl#MaxExclusiveConstraintComponent",
        Constraint::MaxInclusive(_) => "http://www.w3.org/ns/shacl#MaxInclusiveConstraintComponent",
        Constraint::MinLength(_) => "http://www.w3.org/ns/shacl#MinLengthConstraintComponent",
        Constraint::MaxLength(_) => "http://www.w3.org/ns/shacl#MaxLengthConstraintComponent",
        Constraint::Pattern { .. } => "http://www.w3.org/ns/shacl#PatternConstraintComponent",
        Constraint::LanguageIn(_) => "http://www.w3.org/ns/shacl#LanguageInConstraintComponent",
        Constraint::UniqueLang(_) => "http://www.w3.org/ns/shacl#UniqueLangConstraintComponent",
        Constraint::Equals(_) => "http://www.w3.org/ns/shacl#EqualsConstraintComponent",
        Constraint::Disjoint(_) => "http://www.w3.org/ns/shacl#DisjointConstraintComponent",
        Constraint::LessThan(_) => "http://www.w3.org/ns/shacl#LessThanConstraintComponent",
        Constraint::LessThanOrEquals(_) => {
            "http://www.w3.org/ns/shacl#LessThanOrEqualsConstraintComponent"
        }
        Constraint::Not(_) => "http://www.w3.org/ns/shacl#NotConstraintComponent",
        Constraint::And(_) => "http://www.w3.org/ns/shacl#AndConstraintComponent",
        Constraint::Or(_) => "http://www.w3.org/ns/shacl#OrConstraintComponent",
        Constraint::Xone(_) => "http://www.w3.org/ns/shacl#XoneConstraintComponent",
        Constraint::Node(_) => "http://www.w3.org/ns/shacl#NodeConstraintComponent",
        Constraint::Property(_) => "http://www.w3.org/ns/shacl#PropertyConstraintComponent",
        Constraint::QualifiedValueShape { .. } => {
            "http://www.w3.org/ns/shacl#QualifiedMinCountConstraintComponent"
        }
        Constraint::Closed { .. } => "http://www.w3.org/ns/shacl#ClosedConstraintComponent",
        Constraint::HasValue(_) => "http://www.w3.org/ns/shacl#HasValueConstraintComponent",
        Constraint::In(_) => "http://www.w3.org/ns/shacl#InConstraintComponent",
        Constraint::SparqlConstraint { .. } => {
            "http://www.w3.org/ns/shacl#SPARQLConstraintComponent"
        }
        Constraint::Custom(cc) => &cc.component,
        Constraint::Expression { .. } => "http://www.w3.org/ns/shacl#ExpressionConstraintComponent",
    }
}

/// A shapes-graph node as the loader names it — an IRI, or `_:label` for a
/// blank node (`Shape::iri`, the `sh:sparql` node) — back as a typed term.
pub(crate) fn lexical_term(s: &str) -> Term {
    match s.strip_prefix("_:") {
        Some(label) => Term::BlankNode(oxigraph::model::BlankNode::new_unchecked(label)),
        None => Term::NamedNode(oxigraph::model::NamedNode::new_unchecked(s)),
    }
}

/// Validate all constraints of an inline `Shape` against `focus_node` and return violations.
///
/// Used by logical constraint operators (sh:not, sh:and, sh:or, sh:xone), sh:node
/// and sh:qualifiedValueShape.
pub(crate) fn validate_inline_shape(
    view: &DataView<'_>,
    shapes: &[Shape],
    focus_node: &Term,
    shape: &Shape,
    severity: &Severity,
) -> Vec<ValidationResult> {
    // Every term conforms to a deactivated shape (SHACL §2.1.6).
    if shape.deactivated {
        return Vec::new();
    }
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
            view, shapes, shape_iri, focus_node, constraint, None, severity,
        ));
    }

    for prop_shape in shape.property_shapes.iter().filter(|ps| !ps.deactivated) {
        let ps_iri = prop_shape.iri.as_deref().unwrap_or(shape_iri);
        // The value nodes are fetched once per (focus node, property shape) and
        // shared by every constraint of the shape.
        let values = value_nodes(view, focus_node, Some(&prop_shape.path));
        for constraint in &prop_shape.constraints {
            results.extend(evaluate_constraint_with_values(
                view,
                shapes,
                ps_iri,
                focus_node,
                constraint,
                Some(&prop_shape.path),
                &values,
                severity,
            ));
        }
    }

    results
}

/// Evaluate a constraint against a (typed) focus node, resolving the value
/// nodes along `path` first. Callers that evaluate several constraints of one
/// property shape use [`evaluate_constraint_with_values`] with the values
/// fetched once.
#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate_constraint(
    view: &DataView<'_>,
    shapes: &[Shape],
    shape_iri: &str,
    focus_node: &Term,
    constraint: &Constraint,
    path: Option<&PropertyPath>,
    severity: &Severity,
) -> Vec<ValidationResult> {
    let values = value_nodes(view, focus_node, path);
    evaluate_constraint_with_values(
        view, shapes, shape_iri, focus_node, constraint, path, &values, severity,
    )
}

/// Evaluate a constraint against a focus node whose value nodes along `path`
/// (`values`, distinct) have already been resolved.
///
/// The constraints that recurse into other shapes (`sh:not`, `sh:and`,
/// `sh:or`, `sh:xone`, `sh:node`, nested `sh:property`,
/// `sh:qualifiedValueShape`, `sh:expression`) are evaluated here; every other
/// constraint in [`evaluate_leaf_constraint`]. The split keeps this frame — the
/// one repeated at every level of a nested or cyclic shapes graph, up to
/// `MAX_SHACL_SHAPE_DEPTH` levels — small: an unoptimised build gives every
/// result built in a function its own stack slot, and with all ~40 arms in one
/// function 50 levels no longer fit a 2 MiB thread stack.
#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate_constraint_with_values(
    view: &DataView<'_>,
    shapes: &[Shape],
    shape_iri: &str,
    focus_node: &Term,
    constraint: &Constraint,
    path: Option<&PropertyPath>,
    values: &[Term],
    severity: &Severity,
) -> Vec<ValidationResult> {
    let mut results = Vec::new();
    let ctx = ResultCtx::new(severity, shape_iri, focus_node);
    let component = component_iri(constraint);
    match constraint {
        // ---- SHACL-AF §7: expression constraint ----
        // A result for every value node whose expression, evaluated with the
        // value node as its focus node, does not produce exactly `{ true }`.
        Constraint::Expression {
            node,
            expr,
            message,
        } => {
            for v in values.iter() {
                let outcome = super::node_expr::eval(view, shapes, expr, v);
                let detail = match &outcome {
                    Ok(nodes) if super::node_expr::is_exactly_true(nodes) => continue,
                    Ok(nodes) => format!(
                        "sh:expression produced {{ {} }}, not {{ true }}",
                        nodes
                            .iter()
                            .map(display_term)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    Err(e) => format!("sh:expression could not be evaluated: {e}"),
                };
                // The expression is the result's sh:sourceConstraint (§7).
                ctx.push_expression(
                    &mut results,
                    component,
                    Some(v),
                    path,
                    node,
                    message.clone().unwrap_or(detail),
                );
            }
        }

        // ---- Logical constraints ----
        // In a property-shape context these apply to EACH VALUE NODE along the
        // path (SHACL §4.6); only in a node-shape context (no path) do they apply
        // to the focus node itself. Results keep the original focus node and
        // carry the offending value in sh:value.
        Constraint::Not(inner_shape) => {
            for value in values.iter() {
                // The value must NOT conform; zero inner violations → violation.
                let inner_violations =
                    validate_inline_shape(view, shapes, value, inner_shape, severity);
                if inner_violations.is_empty() {
                    ctx.push(
                        &mut results,
                        component,
                        Some(value),
                        path,
                        "sh:not".to_string(),
                        "Value conforms to sh:not shape (must not conform)".to_string(),
                    );
                }
            }
        }

        Constraint::And(inner_shapes) => {
            // Every value must conform to ALL inner shapes; one violation per
            // value that fails any of them.
            for value in values.iter() {
                let fails = inner_shapes.iter().any(|inner| {
                    !validate_inline_shape(view, shapes, value, inner, severity).is_empty()
                });
                if fails {
                    ctx.push(
                        &mut results,
                        component,
                        Some(value),
                        path,
                        "sh:and".to_string(),
                        "Value does not conform to all sh:and shapes".to_string(),
                    );
                }
            }
        }

        Constraint::Or(inner_shapes) => {
            // Every value must conform to at least one inner shape.
            for value in values.iter() {
                let any_conforms = inner_shapes.iter().any(|inner| {
                    validate_inline_shape(view, shapes, value, inner, severity).is_empty()
                });
                if !any_conforms {
                    ctx.push(
                        &mut results,
                        component,
                        Some(value),
                        path,
                        "sh:or".to_string(),
                        "Value does not conform to any sh:or shape".to_string(),
                    );
                }
            }
        }

        Constraint::Xone(inner_shapes) => {
            // Every value must conform to exactly one inner shape.
            for value in values.iter() {
                let conforming_count = inner_shapes
                    .iter()
                    .filter(|inner| {
                        validate_inline_shape(view, shapes, value, inner, severity).is_empty()
                    })
                    .count();
                if conforming_count != 1 {
                    ctx.push(
                        &mut results,
                        component,
                        Some(value),
                        path,
                        "sh:xone".to_string(),
                        format!(
                            "Value conforms to {} sh:xone shapes, expected exactly 1",
                            conforming_count
                        ),
                    );
                }
            }
        }

        // ---- Shape reference constraint ----
        Constraint::Node(ref_shape) => {
            // Each value node must conform to the referenced shape; one
            // violation per non-conforming value (sh:node, SHACL §4.6.3).
            for value in values.iter() {
                let inner = validate_inline_shape(view, shapes, value, ref_shape, severity);
                if !inner.is_empty() {
                    ctx.push(
                        &mut results,
                        component,
                        Some(value),
                        path,
                        format!("sh:node <{}>", ref_shape.iri),
                        format!("Value does not conform to shape <{}>", ref_shape.iri),
                    );
                }
            }
        }

        // ---- Nested property shape (sh:property on a property shape) ----
        Constraint::Property(inner_ps) if inner_ps.deactivated => {}
        Constraint::Property(inner_ps) => {
            // Each value node along the outer path becomes the focus node of the
            // nested property shape (SHACL §2.1.3).
            let inner_iri = inner_ps.iri.as_deref().unwrap_or(shape_iri);
            for value in values.iter() {
                // One fetch per (outer value node, nested property shape).
                let inner_values = value_nodes(view, value, Some(&inner_ps.path));
                for c in &inner_ps.constraints {
                    results.extend(evaluate_constraint_with_values(
                        view,
                        shapes,
                        inner_iri,
                        value,
                        c,
                        Some(&inner_ps.path),
                        &inner_values,
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
            let conforming_count = values
                .iter()
                .filter(|v| {
                    validate_inline_shape(view, shapes, v, qvs, severity).is_empty()
                        && !(*disjoint
                            && sibling_shapes.iter().any(|sib| {
                                validate_inline_shape(view, shapes, v, sib, severity).is_empty()
                            }))
                })
                .count();

            if let Some(min) = min_count {
                if conforming_count < *min {
                    ctx.push(
                        &mut results,
                        component,
                        None,
                        path,
                        format!("sh:qualifiedMinCount {}", min),
                        format!(
                            "Only {} values conform to qualified shape, expected at least {}",
                            conforming_count, min
                        ),
                    );
                }
            }
            if let Some(max) = max_count {
                if conforming_count > *max {
                    ctx.push(
                        &mut results,
                        SH_QUALIFIED_MAX_COUNT_COMPONENT,
                        None,
                        path,
                        format!("sh:qualifiedMaxCount {}", max),
                        format!(
                            "{} values conform to qualified shape, expected at most {}",
                            conforming_count, max
                        ),
                    );
                }
            }
        }
        _ => {
            return evaluate_leaf_constraint(
                view, shape_iri, focus_node, constraint, path, values, severity,
            )
        }
    }

    results
}

/// What the results of one (shape, focus node) share, and how one is built:
/// the display strings plus the typed terms the RDF report is written from.
/// The focus node's display string is built only when a result is produced —
/// the overwhelmingly common outcome of a constraint is "no result".
struct ResultCtx<'a> {
    severity: &'a Severity,
    shape_iri: &'a str,
    focus_node: &'a Term,
    focus_str: std::cell::OnceCell<String>,
}

impl<'a> ResultCtx<'a> {
    fn new(severity: &'a Severity, shape_iri: &'a str, focus_node: &'a Term) -> Self {
        ResultCtx {
            severity,
            shape_iri,
            focus_node,
            focus_str: std::cell::OnceCell::new(),
        }
    }

    /// One result of `component`. `value` is sh:value for value-node-oriented
    /// results (SHACL sets it to the offending value node — the focus itself
    /// in a node-shape context).
    fn result(
        &self,
        component: &str,
        value: Option<&Term>,
        path: Option<&PropertyPath>,
        source_constraint: String,
        message: String,
    ) -> ValidationResult {
        ValidationResult {
            severity: self.severity.clone(),
            focus_node: self
                .focus_str
                .get_or_init(|| display_term(self.focus_node))
                .clone(),
            path: path.map(|p| p.to_sparql()),
            value: value.map(display_term),
            source_shape: self.shape_iri.to_string(),
            source_constraint,
            source_constraint_component: component.to_string(),
            message,
            annotations: Vec::new(),
            terms: ResultTerms {
                focus_node: Some(self.focus_node.clone()),
                value: value.cloned(),
                path: path.cloned(),
                source_shape: Some(lexical_term(self.shape_iri)),
                source_constraint: None,
                severity: None,
                annotations: Vec::new(),
            },
        }
    }

    /// [`ResultCtx::result`], pushed onto `results`.
    fn push(
        &self,
        results: &mut Vec<ValidationResult>,
        component: &str,
        value: Option<&Term>,
        path: Option<&PropertyPath>,
        source_constraint: String,
        message: String,
    ) {
        results.push(self.result(component, value, path, source_constraint, message));
    }

    /// [`ResultCtx::push`] for an expression constraint: the node expression
    /// is the result's `sh:sourceConstraint` (SHACL-AF §7). (Built here rather
    /// than in the caller, whose frame is the one repeated at every recursion
    /// level.)
    fn push_expression(
        &self,
        results: &mut Vec<ValidationResult>,
        component: &str,
        value: Option<&Term>,
        path: Option<&PropertyPath>,
        constraint: &Term,
        message: String,
    ) {
        let mut r = self.result(component, value, path, "sh:expression".into(), message);
        r.terms.source_constraint = Some(constraint.clone());
        results.push(r);
    }
}

/// Every constraint that does not recurse into other shapes (see
/// [`evaluate_constraint_with_values`], the only caller).
#[inline(never)]
fn evaluate_leaf_constraint(
    view: &DataView<'_>,
    shape_iri: &str,
    focus_node: &Term,
    constraint: &Constraint,
    path: Option<&PropertyPath>,
    values: &[Term],
    severity: &Severity,
) -> Vec<ValidationResult> {
    let mut results = Vec::new();
    let ctx = ResultCtx::new(severity, shape_iri, focus_node);
    let component = component_iri(constraint);
    let mk = |value: Option<&Term>,
              path: Option<&PropertyPath>,
              source_constraint: String,
              message: String|
     -> ValidationResult {
        ctx.result(component, value, path, source_constraint, message)
    };

    match constraint {
        Constraint::Class(class_iri) => {
            // Every value node must be a SHACL instance of the class
            // (rdf:type/rdfs:subClassOf*). Literals are never instances.
            for v in values.iter() {
                if !view.is_instance_of(v, class_iri) {
                    results.push(mk(
                        Some(v),
                        path,
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
            for v in values.iter() {
                let ok = match &v {
                    Term::Literal(lit) => {
                        lit.datatype().as_str() == dt_iri.as_str() && xsd_lexical_valid(lit)
                    }
                    _ => false,
                };
                if !ok {
                    results.push(mk(
                        Some(v),
                        path,
                        format!("sh:datatype <{}>", dt_iri),
                        format!("Value has wrong datatype, expected <{}>", dt_iri),
                    ));
                }
            }
        }

        Constraint::NodeKind(expected) => {
            for v in values.iter() {
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
                        Some(v),
                        path,
                        format!("sh:nodeKind {:?}", expected),
                        format!("Value does not match expected node kind {:?}", expected),
                    ));
                }
            }
        }

        Constraint::MinCount(min) => {
            let count = values.len();
            if count < *min {
                results.push(mk(
                    None,
                    path,
                    format!("sh:minCount {}", min),
                    format!("Expected at least {} values, found {}", min, count),
                ));
            }
        }

        Constraint::MaxCount(max) => {
            let count = values.len();
            if count > *max {
                results.push(mk(
                    None,
                    path,
                    format!("sh:maxCount {}", max),
                    format!("Expected at most {} values, found {}", max, count),
                ));
            }
        }

        Constraint::MinLength(min_len) => {
            for v in values.iter() {
                // sh:minLength applies to the string representation of the value:
                // literals by lexical form, IRIs by IRI string; blank nodes always violate.
                let ok = match string_repr(v) {
                    Some(s) => s.chars().count() >= *min_len,
                    None => false,
                };
                if !ok {
                    results.push(mk(
                        Some(v),
                        path,
                        format!("sh:minLength {}", min_len),
                        format!("Value length is less than minimum {}", min_len),
                    ));
                }
            }
        }

        Constraint::MaxLength(max_len) => {
            for v in values.iter() {
                let ok = match string_repr(v) {
                    Some(s) => s.chars().count() <= *max_len,
                    None => false,
                };
                if !ok {
                    results.push(mk(
                        Some(v),
                        path,
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
                    path,
                    "sh:pattern".to_string(),
                    "sh:pattern is too long to evaluate".to_string(),
                ));
                return results;
            }
            for v in values.iter().take(MAX_PATTERN_VALUES) {
                // Blank nodes always violate sh:pattern (SHACL §4.4.2).
                let Some(value) = string_repr(v) else {
                    results.push(mk(
                        Some(v),
                        path,
                        format!("sh:pattern \"{}\"", pattern),
                        format!("Value does not match pattern \"{}\"", pattern),
                    ));
                    continue;
                };
                let regex_flags = flags.as_deref().unwrap_or("");
                // Compiled once per (pattern, flags) per thread; this used to
                // run a SPARQL `ASK { FILTER(REGEX(…)) }` — parse, plan, regex
                // compile, execute — for every single value.
                let matches = match cached_regex_match(pattern, regex_flags, &value) {
                    Some(m) => m,
                    None => {
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
                        match view.store.query(&query) {
                            Ok(oxigraph::sparql::QueryResults::Boolean(m)) => m,
                            _ => true,
                        }
                    }
                };
                {
                    if !matches {
                        results.push(mk(
                            Some(v),
                            path,
                            format!("sh:pattern \"{}\"", pattern),
                            format!("Value does not match pattern \"{}\"", pattern),
                        ));
                    }
                }
            }
        }

        Constraint::HasValue(expected) => {
            if !values.iter().any(|v| v == expected) {
                results.push(mk(
                    None,
                    path,
                    format!("sh:hasValue {}", display_term(expected)),
                    format!("Missing required value: {}", display_term(expected)),
                ));
            }
        }

        Constraint::In(allowed) => {
            for v in values.iter() {
                if !allowed.iter().any(|a| a == v) {
                    results.push(mk(
                        Some(v),
                        path,
                        "sh:in".to_string(),
                        format!("Value \"{}\" is not in the allowed list", display_term(v)),
                    ));
                }
            }
        }

        Constraint::UniqueLang(unique) => {
            if *unique {
                // One result per language tag carried by more than one value node.
                let mut langs: BTreeMap<String, usize> = BTreeMap::new();
                for v in values.iter() {
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
                            path,
                            "sh:uniqueLang true".to_string(),
                            format!("Duplicate language tag: {}", lang),
                        ));
                    }
                }
            }
        }

        Constraint::LanguageIn(allowed_langs) => {
            for v in values.iter() {
                let lang_ok = match &v {
                    Term::Literal(lit) => lit
                        .language()
                        .map(|l| allowed_langs.iter().any(|al| lang_matches(l, al)))
                        .unwrap_or(false),
                    _ => false,
                };
                if !lang_ok {
                    results.push(mk(
                        Some(v),
                        path,
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
            for (p, o) in view.subject_predicate_objects(focus_node, GraphSel::All) {
                if !ignored_properties.contains(&p) && !allowed_properties.contains(&p) {
                    results.push(mk(
                        Some(&o),
                        Some(&PropertyPath::Predicate(p.clone())),
                        "sh:closed true".to_string(),
                        format!("Property <{}> is not allowed by closed shape", p),
                    ));
                }
            }
        }

        Constraint::SparqlConstraint {
            node,
            select,
            message,
            severity: severity_override,
            annotations,
        } => {
            // A sh:severity on the SPARQLConstraint node overrides the shape's severity.
            let eff_severity = severity_override
                .as_deref()
                .map(Severity::from_iri)
                .unwrap_or_else(|| severity.clone());
            // A constraint that cannot be evaluated is a failure, not a pass:
            // `if let Ok(..)` used to drop the error, so a `sh:select` that did
            // not parse (or a SELECT that errored at evaluation) produced no
            // violations and the focus node conformed by accident — and a
            // write gate built on it waved the write through.
            // Every result names the sh:sparql node (sh:sourceConstraint).
            let sparql_result = |value: Option<&Term>,
                                 path: Option<&PropertyPath>,
                                 message: String|
             -> ValidationResult {
                let mut r = mk(value, path, "sh:SPARQLConstraint".to_string(), message);
                r.terms.source_constraint = Some(lexical_term(node));
                r
            };
            let unevaluable = |reason: String| ValidationResult {
                severity: Severity::Violation,
                ..sparql_result(
                    None,
                    path,
                    format!("SPARQL constraint could not be evaluated: {reason}"),
                )
            };
            // SHACL-SPARQL (§5.3): run the SELECT with $this PRE-BOUND to the
            // focus node — as a term, so a blank-node focus is checked like any
            // other — and $PATH replaced by the shape's path. Each solution is
            // a violation, except that one binding ?failure to true makes the
            // whole constraint a failure.
            let path_sparql = path.map(|p| p.to_sparql());
            let solutions = prepare_prebound(select, &["this"], path_sparql.as_deref())
                .and_then(|q| view.query_prebound(q, &[("this", focus_node)]));
            match solutions {
                Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) => {
                    for solution in solutions {
                        let solution = match solution {
                            Ok(s) => s,
                            Err(e) => {
                                results.push(unevaluable(e.to_string()));
                                break;
                            }
                        };
                        if is_failure(&solution) {
                            results.push(unevaluable(FAILURE.to_string()));
                            continue;
                        }
                        let msg = result_message(&solution, message.as_deref(), |_| None)
                            .unwrap_or_else(|| "SPARQL constraint violated".to_string());
                        // sh:value is ?value; unbound, the focus node of a
                        // node shape (SHACL §5.3.2).
                        let value = solution
                            .get("value")
                            .or(path.is_none().then_some(focus_node));
                        let row_path = result_path(&solution);
                        let mut r = sparql_result(value, row_path.as_ref().or(path), msg);
                        // The value's display keeps the N-Triples form it has
                        // always had for sh:sparql results.
                        r.value = value.map(|v| v.to_string());
                        r.severity = eff_severity.clone();
                        r.terms.severity = severity_override.clone();
                        annotate(&mut r, annotations, Some(&solution));
                        results.push(r);
                    }
                }
                Ok(_) => results.push(unevaluable("sh:select must be a SELECT query".to_string())),
                Err(e) => results.push(unevaluable(e)),
            }
        }

        // ---- SHACL-AF constraint component (sh:validator / sh:ask / sh:select) ----
        Constraint::Custom(cc) => {
            let path_sparql = path.map(|p| p.to_sparql());
            let unevaluable = |reason: String| ValidationResult {
                severity: Severity::Violation,
                ..mk(
                    None,
                    path,
                    cc.component.clone(),
                    format!(
                        "constraint component <{}> could not be evaluated: {reason}",
                        cc.component
                    ),
                )
            };
            // Every parameter value is pre-bound under the parameter's name
            // (§6.3), alongside $this and — for ASK validators — $value.
            let param = |name: &str| -> Option<Term> {
                cc.params
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, t)| t.clone())
            };
            match &cc.validator {
                CustomValidator::Ask(ask) => {
                    let mut names = vec!["this", "value"];
                    names.extend(cc.params.iter().map(|(n, _)| n.as_str()));
                    let prepared = match prepare_prebound(ask, &names, path_sparql.as_deref()) {
                        Ok(q) => q,
                        Err(e) => {
                            results.push(unevaluable(e));
                            return results;
                        }
                    };
                    for v in values.iter() {
                        let mut bindings: Vec<(&str, &Term)> =
                            vec![("this", focus_node), ("value", v)];
                        bindings.extend(cc.params.iter().map(|(n, t)| (n.as_str(), t)));
                        match view.query_prebound(prepared.clone(), &bindings) {
                            Ok(oxigraph::sparql::QueryResults::Boolean(true)) => {}
                            Ok(oxigraph::sparql::QueryResults::Boolean(false)) => {
                                // An ASK validator's solution is ($this, $value)
                                // plus the parameters (§6.3).
                                let lookup = |name: &str| match name {
                                    "this" => Some(focus_node.clone()),
                                    "value" => Some(v.clone()),
                                    other => param(other),
                                };
                                let message = match &cc.message {
                                    Some(t) => fill_template(t, lookup),
                                    None => format!(
                                        "Value does not satisfy constraint component <{}>",
                                        cc.component
                                    ),
                                };
                                // No solution to read: only the annotations'
                                // sh:annotationValue defaults apply.
                                let mut r = mk(Some(v), path, cc.component.clone(), message);
                                annotate(&mut r, &cc.annotations, None);
                                results.push(r);
                            }
                            Ok(_) => {
                                results.push(unevaluable("sh:ask must be an ASK query".to_string()))
                            }
                            Err(e) => results.push(unevaluable(e)),
                        }
                    }
                }
                CustomValidator::Select(select) => {
                    let mut names = vec!["this"];
                    names.extend(cc.params.iter().map(|(n, _)| n.as_str()));
                    let mut bindings: Vec<(&str, &Term)> = vec![("this", focus_node)];
                    bindings.extend(cc.params.iter().map(|(n, t)| (n.as_str(), t)));
                    let solutions = prepare_prebound(select, &names, path_sparql.as_deref())
                        .and_then(|q| view.query_prebound(q, &bindings));
                    match solutions {
                        Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) => {
                            for solution in solutions {
                                let solution = match solution {
                                    Ok(s) => s,
                                    Err(e) => {
                                        results.push(unevaluable(e.to_string()));
                                        break;
                                    }
                                };
                                if is_failure(&solution) {
                                    results.push(unevaluable(FAILURE.to_string()));
                                    continue;
                                }
                                // As for sh:sparql (SHACL §5.3.2, §6.3).
                                let value = solution
                                    .get("value")
                                    .or(path.is_none().then_some(focus_node))
                                    .cloned();
                                let message =
                                    result_message(&solution, cc.message.as_deref(), param)
                                        .unwrap_or_else(|| {
                                            format!(
                                                "Constraint component <{}> reports a violation",
                                                cc.component
                                            )
                                        });
                                let row_path = result_path(&solution);
                                let mut r = mk(
                                    value.as_ref(),
                                    row_path.as_ref().or(path),
                                    cc.component.clone(),
                                    message,
                                );
                                annotate(&mut r, &cc.annotations, Some(&solution));
                                results.push(r);
                            }
                        }
                        Ok(_) => results
                            .push(unevaluable("sh:select must be a SELECT query".to_string())),
                        Err(e) => results.push(unevaluable(e)),
                    }
                }
            }
        }

        // ---- Value range constraints ----
        // Violation unless the comparison is *definitively* satisfied: literals of
        // incomparable types, IRIs and blank nodes all violate (SHACL §4.3).
        Constraint::MinExclusive(bound) => {
            for v in values.iter() {
                if !matches!(compare_terms(v, bound), Some(Ordering::Greater)) {
                    results.push(mk(
                        Some(v),
                        path,
                        format!("sh:minExclusive {}", display_term(bound)),
                        format!("Value {} is not > {}", display_term(v), display_term(bound)),
                    ));
                }
            }
        }

        Constraint::MinInclusive(bound) => {
            for v in values.iter() {
                if !matches!(
                    compare_terms(v, bound),
                    Some(Ordering::Greater | Ordering::Equal)
                ) {
                    results.push(mk(
                        Some(v),
                        path,
                        format!("sh:minInclusive {}", display_term(bound)),
                        format!(
                            "Value {} is not >= {}",
                            display_term(v),
                            display_term(bound)
                        ),
                    ));
                }
            }
        }

        Constraint::MaxExclusive(bound) => {
            for v in values.iter() {
                if !matches!(compare_terms(v, bound), Some(Ordering::Less)) {
                    results.push(mk(
                        Some(v),
                        path,
                        format!("sh:maxExclusive {}", display_term(bound)),
                        format!("Value {} is not < {}", display_term(v), display_term(bound)),
                    ));
                }
            }
        }

        Constraint::MaxInclusive(bound) => {
            for v in values.iter() {
                if !matches!(
                    compare_terms(v, bound),
                    Some(Ordering::Less | Ordering::Equal)
                ) {
                    results.push(mk(
                        Some(v),
                        path,
                        format!("sh:maxInclusive {}", display_term(bound)),
                        format!(
                            "Value {} is not <= {}",
                            display_term(v),
                            display_term(bound)
                        ),
                    ));
                }
            }
        }

        // ---- Property pair constraints ----
        Constraint::Equals(prop_iri) => {
            // One result per value in the symmetric difference of the two value sets.
            let path_values = term_set(values.to_vec());
            let other_path = PropertyPath::Predicate(prop_iri.clone());
            let other_values = term_set(value_nodes(view, focus_node, Some(&other_path)));
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
                    Some(v),
                    path,
                    format!("sh:equals <{}>", prop_iri),
                    format!(
                        "Value set at path does not equal value set at <{}>",
                        prop_iri
                    ),
                ));
            }
        }

        Constraint::Disjoint(prop_iri) => {
            let path_values = term_set(values.to_vec());
            let other_path = PropertyPath::Predicate(prop_iri.clone());
            let other_values = term_set(value_nodes(view, focus_node, Some(&other_path)));
            for (_, v) in path_values
                .iter()
                .filter(|(k, _)| other_values.contains_key(*k))
            {
                results.push(mk(
                    Some(v),
                    path,
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
            let path_values = values.to_vec();
            let other_path = PropertyPath::Predicate(prop_iri.clone());
            let other_values = value_nodes(view, focus_node, Some(&other_path));
            for pv in &path_values {
                for ov in &other_values {
                    // Violated unless definitively pv < ov (incomparable pairs violate).
                    if !matches!(compare_terms(pv, ov), Some(Ordering::Less)) {
                        results.push(mk(
                            Some(pv),
                            path,
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
            let path_values = values.to_vec();
            let other_path = PropertyPath::Predicate(prop_iri.clone());
            let other_values = value_nodes(view, focus_node, Some(&other_path));
            for pv in &path_values {
                for ov in &other_values {
                    if !matches!(
                        compare_terms(pv, ov),
                        Some(Ordering::Less | Ordering::Equal)
                    ) {
                        results.push(mk(
                            Some(pv),
                            path,
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

        Constraint::Not(_)
        | Constraint::And(_)
        | Constraint::Or(_)
        | Constraint::Xone(_)
        | Constraint::Node(_)
        | Constraint::Property(_)
        | Constraint::QualifiedValueShape { .. }
        | Constraint::Expression { .. } => {
            unreachable!("evaluate_constraint_with_values evaluates the recursive constraints")
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
/// Parse-check a `sh:select` the way it will be run — `$this` pre-bound to a
/// placeholder focus node, the `sh:prefixes` prologue already prepended — so
/// a shapes graph whose SPARQL constraint cannot be evaluated fails to load
/// (and a write gate built on it refuses the write) instead of silently
/// producing no violations. A query using a feature SHACL forbids under
/// pre-binding (Appendix A) is rejected the same way.
pub(crate) fn check_sparql_constraint(select: &str) -> Result<(), String> {
    check_prebound_query(select, &["this"], false)
}

/// As [`check_sparql_constraint`] for a constraint-component validator:
/// `$this`, (for ASK validators) `$value` and the component's parameters
/// `params` are pre-bound, and a property validator may use `$PATH`.
pub(crate) fn check_validator_query(query: &str, ask: bool, params: &[&str]) -> Result<(), String> {
    let mut vars: Vec<&str> = if ask {
        vec!["this", "value"]
    } else {
        vec!["this"]
    };
    vars.extend_from_slice(params);
    check_prebound_query(query, &vars, true)
}

fn check_prebound_query(query: &str, vars: &[&str], with_path: bool) -> Result<(), String> {
    // SHACL §5.3.1 leaves `$shapesGraph` / `$currentShape` to processors
    // that expose the shapes graph to constraint queries; this one does
    // not, and a constraint that relies on them must fail, not pass with
    // the variables unbound.
    for var in ["shapesGraph", "currentShape"] {
        if mentions_variable(query, var) {
            return Err(format!(
                "${var} is not supported (the shapes graph is not pre-bound in constraint queries)"
            ));
        }
    }
    if let Some(feature) = prebinding_violation(query, vars) {
        return Err(format!(
            "{feature} is not supported in a pre-bound SHACL-SPARQL query"
        ));
    }
    let path = with_path.then_some("<urn:ots:shacl:path>");
    prepare_prebound(query, vars, path).map(|_| ())
}

// ---------------------------------------------------------------------------
// SHACL-SPARQL pre-binding (SHACL §5.3.1, Appendix A) and its restrictions
// ---------------------------------------------------------------------------

/// Parse a SHACL-SPARQL query for pre-bound evaluation (SHACL §5.3.1, §6.3):
/// `$PATH` replaced by the shape's `path`, and every variable in `vars`
/// pre-bound in every scope and projected ([`crate::sparql::prebind::rewrite`],
/// SHACL Appendix A), so that message templates can name it.
/// [`DataView::query_prebound`] supplies the values.
pub(crate) fn prepare_prebound(
    query: &str,
    vars: &[&str],
    path: Option<&str>,
) -> Result<spargebra::Query, String> {
    let text = match path {
        Some(p) => query.replace("$PATH", p),
        None => query.to_string(),
    };
    let mut parsed = crate::sparql::parser()
        .parse_query(&text)
        .map_err(|e| e.to_string())?;
    crate::sparql::prebind::rewrite(&mut parsed, vars)?;
    Ok(parsed)
}

/// Set the result annotations (SHACL-AF §4) of a result produced by
/// `solution` — or, for an ASK validator, by no solution: each annotation's
/// property gets the solution's binding of its variable, else its
/// `sh:annotationValue` defaults.
fn annotate(
    r: &mut ValidationResult,
    annotations: &[ResultAnnotation],
    solution: Option<&oxigraph::sparql::QuerySolution>,
) {
    for a in annotations {
        let bound = solution.and_then(|s| s.get(a.var_name.as_str()));
        let values: Vec<Term> = match bound {
            Some(t) => vec![t.clone()],
            None => a.defaults.clone(),
        };
        for v in values {
            r.annotations.push(super::report::ResultAnnotationValue {
                property: a.property.as_str().to_string(),
                value: display_term(&v),
            });
            r.terms.annotations.push((a.property.clone(), v));
        }
    }
}

/// What a solution binding `?failure` to `true` reports (SHACL §5.3).
const FAILURE: &str = "the query reported a failure (?failure is true)";

/// Whether a constraint solution binds `?failure` to `true`: the validator
/// then produces a failure instead of a validation result (SHACL §5.3).
fn is_failure(solution: &oxigraph::sparql::QuerySolution) -> bool {
    matches!(solution.get("failure"), Some(Term::Literal(l))
        if l.datatype() == oxigraph::model::vocab::xsd::BOOLEAN
            && matches!(l.value(), "true" | "1"))
}

/// `sh:resultPath` from a solution: the binding of `?path`, if it is an IRI
/// (SHACL §5.3.2); otherwise the caller falls back to the shape's path.
fn result_path(solution: &oxigraph::sparql::QuerySolution) -> Option<PropertyPath> {
    match solution.get("path") {
        Some(Term::NamedNode(p)) => Some(PropertyPath::Predicate(p.as_str().to_string())),
        _ => None,
    }
}

/// `sh:resultMessage` from a solution (SHACL §5.3.2): the binding of
/// `?message`, else `template` with its `{?var}` / `{$var}` blocks filled from
/// the solution and then from `extra` (a component's parameters). `None` when
/// there is neither.
fn result_message(
    solution: &oxigraph::sparql::QuerySolution,
    template: Option<&str>,
    extra: impl Fn(&str) -> Option<Term>,
) -> Option<String> {
    if let Some(Term::Literal(l)) = solution.get("message") {
        return Some(l.value().to_string());
    }
    template.map(|t| {
        fill_template(t, |name| {
            solution.get(name).cloned().or_else(|| extra(name))
        })
    })
}

/// Replace each `{?name}` and `{$name}` in a message template with the
/// display form of `lookup(name)`. A block whose variable has no value is left
/// as written, so a misspelt name shows up in the message instead of vanishing.
pub(crate) fn fill_template(template: &str, lookup: impl Fn(&str) -> Option<Term>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let block = after
            .strip_prefix('?')
            .or_else(|| after.strip_prefix('$'))
            .and_then(|tail| {
                let end = tail.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
                (end > 0 && tail[end..].starts_with('}')).then(|| (&tail[..end], end + 3))
            });
        match block.and_then(|(name, len)| lookup(name).map(|t| (t, len))) {
            Some((term, len)) => {
                out.push_str(&display_term(&term));
                rest = &rest[open + len..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether `query` uses the variable `name` (`$name` or `?name`) outside
/// strings, IRIs and comments.
pub(crate) fn mentions_variable(query: &str, name: &str) -> bool {
    let mut i = 0;
    let bytes = query.as_bytes();
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '#' {
            i = query[i..].find('\n').map(|n| i + n).unwrap_or(query.len());
            continue;
        }
        if c == '<' {
            if let Some(n) = query[i..].find('>') {
                if !query[i..=i + n].contains(char::is_whitespace) {
                    i += n + 1;
                    continue;
                }
            }
        }
        if c == '"' || c == '\'' {
            let q = c;
            let long = query[i..].starts_with(&format!("{q}{q}{q}"));
            let delim = if long {
                format!("{q}{q}{q}")
            } else {
                q.to_string()
            };
            i += delim.len();
            while i < bytes.len() && !bytes[i..].starts_with(delim.as_bytes()) {
                i += if bytes[i] as char == '\\' { 2 } else { 1 };
            }
            i = (i + delim.len()).min(query.len());
            continue;
        }
        if (c == '$' || c == '?') && query[i + 1..].starts_with(name) {
            let after = query[i + 1 + name.len()..].chars().next();
            if !after.is_some_and(|ch| ch.is_alphanumeric() || ch == '_') {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// The SPARQL features SHACL forbids in a pre-bound query (Appendix A):
/// `MINUS`, `VALUES`, `SERVICE`, assigning to a pre-bound variable
/// (`AS $this`), and a nested `SELECT` that does not project every pre-bound
/// variable explicitly (`SELECT *` included). Returns the offending feature.
pub(crate) fn prebinding_violation(query: &str, vars: &[&str]) -> Option<String> {
    // The first pre-bound variable a sub-select leaves out of its projection.
    let unprojected = |names: &[String], star: bool| -> Option<String> {
        if star {
            return Some(vars.first().copied().unwrap_or("this").to_string());
        }
        vars.iter()
            .find(|v| !names.iter().any(|n| n == *v))
            .map(|v| v.to_string())
    };
    let bytes = query.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    let mut prev_as = false;
    let mut nested_select: Option<(Vec<String>, bool)> = None; // (projected names, star)
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '#' {
            i = query[i..].find('\n').map(|n| i + n).unwrap_or(query.len());
            continue;
        }
        if c == '<' {
            if let Some(n) = query[i..].find('>') {
                if !query[i..=i + n].contains(char::is_whitespace) {
                    i += n + 1;
                    continue;
                }
            }
        }
        if c == '"' || c == '\'' {
            let q = c;
            let long = query[i..].starts_with(&format!("{q}{q}{q}"));
            let delim = if long {
                format!("{q}{q}{q}")
            } else {
                q.to_string()
            };
            i += delim.len();
            loop {
                if i >= bytes.len() {
                    break;
                }
                if bytes[i] as char == '\\' {
                    i += 2;
                    continue;
                }
                if bytes[i..].starts_with(delim.as_bytes()) {
                    i += delim.len();
                    break;
                }
                i += 1;
            }
            continue;
        }
        if c == '{' {
            depth += 1;
            if let Some((names, star)) = nested_select.take() {
                if let Some(missing) = unprojected(&names, star) {
                    return Some(format!("a nested SELECT that does not project ${missing}"));
                }
            }
            i += 1;
            continue;
        }
        if c == '}' {
            depth = depth.saturating_sub(1);
            i += 1;
            continue;
        }
        if c == '*' {
            if let Some((_, star)) = nested_select.as_mut() {
                *star = true;
            }
            i += 1;
            continue;
        }
        if (c == '$' || c == '?') && i + 1 < bytes.len() {
            let name_end = query[i + 1..]
                .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                .map(|n| i + 1 + n)
                .unwrap_or(query.len());
            let name = &query[i + 1..name_end];
            if prev_as && vars.contains(&name) {
                return Some(format!("assigning to the pre-bound variable ${name}"));
            }
            if let Some((names, _)) = nested_select.as_mut() {
                names.push(name.to_string());
            }
            prev_as = false;
            i = name_end;
            continue;
        }
        if c.is_alphabetic() {
            let end = query[i..]
                .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                .map(|n| i + n)
                .unwrap_or(query.len());
            let upper = query[i..end].to_ascii_uppercase();
            match upper.as_str() {
                "MINUS" => return Some("MINUS".to_string()),
                "VALUES" => return Some("VALUES".to_string()),
                "SERVICE" => return Some("SERVICE".to_string()),
                "SELECT" if depth > 0 => nested_select = Some((Vec::new(), false)),
                "WHERE" => {
                    if let Some((names, star)) = nested_select.take() {
                        if let Some(missing) = unprojected(&names, star) {
                            return Some(format!(
                                "a nested SELECT that does not project ${missing}"
                            ));
                        }
                    }
                }
                _ => {}
            }
            prev_as = upper == "AS";
            i = end;
            continue;
        }
        if !c.is_whitespace() && c != '(' && c != ')' {
            prev_as = false;
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Value-node resolution
// ---------------------------------------------------------------------------

/// The value nodes a (possibly path-less) constraint applies to: the values
/// along the path in a property-shape context, or the focus node itself in a
/// node-shape context (SHACL §3.4). Distinct — SHACL value nodes form a *set*,
/// so duplicate bindings from diamond-shaped paths collapse.
pub(crate) fn value_nodes(
    view: &DataView<'_>,
    focus_node: &Term,
    path: Option<&PropertyPath>,
) -> Vec<Term> {
    match path {
        None => vec![focus_node.clone()],
        Some(p) => get_path_values(view, focus_node, p),
    }
}

/// Distinct set of terms keyed by N-Triples form (Term is not Ord).
fn term_set(values: Vec<Term>) -> BTreeMap<String, Term> {
    values.into_iter().map(|t| (t.to_string(), t)).collect()
}

/// Resolve the (distinct) value nodes along `path` from `focus`, natively over
/// the run's quad index.
///
/// SHACL validates against ONE data graph (§3.4), and a run over several data
/// graphs validates their merge: every hop of a sequence, alternative or
/// closure may continue in any of them, for every kind of focus node. That is
/// how `sh:sparql`, `sh:class` and the target machinery already read the run,
/// so a rule now gives one answer however it is written. IRI focus nodes used
/// to be walked inside each data graph in turn, results unioned, so a path
/// whose hops live in different graphs found nothing while the same path from
/// a blank-node focus — or the same rule as a `sh:sparql` constraint — found
/// the value.
fn get_path_values(view: &DataView<'_>, focus: &Term, path: &PropertyPath) -> Vec<Term> {
    // One graph and a single predicate step (forward or inverse): the quad
    // index yields each `(focus, p, ?v)` match once, so the result is already
    // a set and the per-value clone-and-hash pass below is pure overhead on
    // the commonest shape there is.
    if view.graph_count() == 1 && is_single_step(path) {
        return eval_path_native(view, focus, path, GraphSel::One(0));
    }
    let mut seen: HashSet<Term> = HashSet::new();
    eval_path_native(view, focus, path, GraphSel::All)
        .into_iter()
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

/// A plain predicate or its inverse — one quad-index step, whose results are
/// distinct within a graph.
fn is_single_step(path: &PropertyPath) -> bool {
    match path {
        PropertyPath::Predicate(_) => true,
        PropertyPath::Inverse(inner) => matches!(inner.as_ref(), PropertyPath::Predicate(_)),
        _ => false,
    }
}

/// Native SHACL path evaluation over the run's quad index. Mirrors SPARQL
/// property path semantics, including the focus node itself for
/// `zeroOrMore`/`zeroOrOne`.
fn eval_path_native(
    view: &DataView<'_>,
    from: &Term,
    path: &PropertyPath,
    sel: GraphSel,
) -> Vec<Term> {
    match path {
        PropertyPath::Predicate(pred) => view.step(from, pred, false, sel),
        PropertyPath::Inverse(inner) => match inner.as_ref() {
            PropertyPath::Predicate(pred) => view.step(from, pred, true, sel),
            // Inverse of a composite path: push the inversion inwards.
            PropertyPath::Sequence(parts) => {
                let reversed = PropertyPath::Sequence(
                    parts
                        .iter()
                        .rev()
                        .map(|p| PropertyPath::Inverse(Box::new(p.clone())))
                        .collect(),
                );
                eval_path_native(view, from, &reversed, sel)
            }
            PropertyPath::Alternative(parts) => parts
                .iter()
                .flat_map(|p| {
                    eval_path_native(view, from, &PropertyPath::Inverse(Box::new(p.clone())), sel)
                })
                .collect(),
            PropertyPath::Inverse(inner2) => eval_path_native(view, from, inner2, sel),
            PropertyPath::ZeroOrMore(p) => eval_path_native(
                view,
                from,
                &PropertyPath::ZeroOrMore(Box::new(PropertyPath::Inverse(p.clone()))),
                sel,
            ),
            PropertyPath::OneOrMore(p) => eval_path_native(
                view,
                from,
                &PropertyPath::OneOrMore(Box::new(PropertyPath::Inverse(p.clone()))),
                sel,
            ),
            PropertyPath::ZeroOrOne(p) => eval_path_native(
                view,
                from,
                &PropertyPath::ZeroOrOne(Box::new(PropertyPath::Inverse(p.clone()))),
                sel,
            ),
        },
        PropertyPath::Sequence(parts) => {
            let mut frontier = vec![from.clone()];
            for part in parts {
                let mut next = Vec::new();
                let mut seen: HashSet<Term> = HashSet::new();
                for node in &frontier {
                    for t in eval_path_native(view, node, part, sel) {
                        if seen.insert(t.clone()) {
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
            .flat_map(|p| eval_path_native(view, from, p, sel))
            .collect(),
        PropertyPath::ZeroOrMore(inner) => closure(view, from, inner, true, sel),
        PropertyPath::OneOrMore(inner) => closure(view, from, inner, false, sel),
        PropertyPath::ZeroOrOne(inner) => {
            let mut out = vec![from.clone()];
            out.extend(eval_path_native(view, from, inner, sel));
            out
        }
    }
}

/// Transitive closure of `inner` starting at `from` (BFS with a visited set);
/// `include_start` distinguishes `*` from `+`.
///
/// Emission is tracked apart from termination: for `+` the start node is not
/// emitted up front, but it *is* emitted once when a cycle leads back to it —
/// `<s> <p>+ ?v` in SPARQL yields `s` for `s p a . a p s`. The old walk marked
/// the start visited before the search and only emitted on first visit, so a
/// focus node on a cycle never counted among its own `+` values.
fn closure(
    view: &DataView<'_>,
    from: &Term,
    inner: &PropertyPath,
    include_start: bool,
    sel: GraphSel,
) -> Vec<Term> {
    let mut visited: HashSet<Term> = HashSet::new();
    let mut emitted: HashSet<Term> = HashSet::new();
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    visited.insert(from.clone());
    if include_start {
        emitted.insert(from.clone());
        out.push(from.clone());
    }
    queue.push_back(from.clone());
    while let Some(node) = queue.pop_front() {
        for next in eval_path_native(view, &node, inner, sel) {
            if emitted.insert(next.clone()) {
                out.push(next.clone());
            }
            if visited.insert(next.clone()) {
                queue.push_back(next);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Term classification & comparison
// ---------------------------------------------------------------------------

/// `sh:pattern` via a per-thread cache of compiled regexes, with the XPath
/// flags SPARQL's REGEX accepts (`i`, `s`, `m`, `x`, `q`). `None` when the
/// pattern or flags are outside what the regex crate handles the same way —
/// the caller then falls back to the SPARQL evaluation, which is exact.
fn cached_regex_match(pattern: &str, flags: &str, value: &str) -> Option<bool> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;
    // pattern -> flags -> compiled regex; nested so a lookup borrows the &strs
    // it has instead of allocating a key tuple per value node.
    type RegexCache = HashMap<String, HashMap<String, Option<Arc<regex::Regex>>>>;
    thread_local! {
        static REGEXES: RefCell<RegexCache> = RefCell::new(HashMap::new());
    }
    if let Some(hit) = REGEXES.with(|c| c.borrow().get(pattern).and_then(|m| m.get(flags)).cloned())
    {
        return hit.map(|re| re.is_match(value));
    }
    let mut literal = false;
    let mut ok = true;
    let mut b = regex::RegexBuilder::new(pattern);
    for f in flags.chars() {
        match f {
            'i' => {
                b.case_insensitive(true);
            }
            's' => {
                b.dot_matches_new_line(true);
            }
            'm' => {
                b.multi_line(true);
            }
            'x' => {
                b.ignore_whitespace(true);
            }
            'q' => literal = true,
            _ => ok = false,
        }
    }
    let built = if !ok {
        None
    } else if literal {
        regex::RegexBuilder::new(&regex::escape(pattern))
            .case_insensitive(flags.contains('i'))
            .size_limit(1 << 20)
            .build()
            .ok()
    } else {
        b.size_limit(1 << 20).build().ok()
    }
    .map(Arc::new);
    REGEXES.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 1024 {
            c.clear();
        }
        c.entry(pattern.to_string())
            .or_default()
            .insert(flags.to_string(), built.clone());
    });
    built.map(|re| re.is_match(value))
}

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

    // xsd:dateTimeStamp is xsd:dateTime with a required time zone: the two
    // compare as one family (the store keeps the derived type as written).
    let is_date_time = |dt: &str| {
        dt.strip_prefix(XSD)
            .is_some_and(|l| l == "dateTime" || l == "dateTimeStamp")
    };
    if is_date_time(&dta) && is_date_time(&dtb) {
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
/// Whether `lit`'s lexical form is in the lexical space of its XSD datatype
/// (`true` for datatypes without a lexical check). Shared with the OWL 2 RL
/// `dt-not-type` rule.
pub(crate) fn xsd_lexical_valid(lit: &Literal) -> bool {
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
        "dateTimeStamp" => parse_xsd_date_time(v).is_some_and(|(_, has_tz)| has_tz),
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
