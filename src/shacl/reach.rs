//! What validating a focus node reads, for the write gate (issue #534).
//!
//! A write gate used to copy the whole target graph into its sandbox before a
//! merge, so a 1 000-quad `POST` into a 9M-quad graph serialised and parsed
//! all nine million. It now validates only the nodes the write can affect,
//! and to know which those are it needs, for every shape, where the
//! evaluation of a focus node reads the data graph. That is this module: a
//! [`ReadPlan`] lists each read as a *site* — a path from the focus node to
//! the nodes where the read happens, and what is read there — following
//! [`super::engine`] and [`super::constraints`] constraint by constraint.
//!
//! A changed triple `(s, p, o)` changes a focus node's result only through a
//! read that sees it: an outgoing read of `p` (or of everything) at `s`, or an
//! incoming read of `p` at `o`. So the focus nodes a write affects are the
//! nodes from which some site's path reaches such an `s` or `o` — the site's
//! path walked backwards from it — and the triples a focused run needs are
//! the ones the sites read, walked forwards from those nodes
//! (`shacl_studio::gate_focus`).
//!
//! A shape whose reads no path bounds — a SPARQL constraint or target, a
//! SHACL-AF constraint component or node expression, a shape that refers back
//! to itself — has no plan: [`read_plan`] says why and the gate copies the
//! whole graph, as before. Every [`Constraint`] variant is matched without a
//! wildcard, so a new one cannot be added without saying what it reads.

use std::collections::{BTreeSet, HashMap};

use super::shapes::{Constraint, PropertyPath, PropertyShape, Shape, Target};

/// Path elements a site may lie behind (a sequence's members each count):
/// past this a shapes graph is treated as unbounded.
const MAX_PREFIX: usize = 8;
/// Sites a plan may hold.
const MAX_SITES: usize = 512;
/// Nesting of inline shapes (`sh:node`, `sh:and`, …) a plan follows.
const MAX_NESTING: usize = 32;

/// A sequence of path elements leading from the focus node to the nodes a
/// read happens at, interned in a [`ReadPlan`]. Prefix 0 is the focus node
/// itself.
pub(crate) type PrefixId = usize;

/// What evaluating every shape of a set at one focus node reads (see the
/// module docs).
#[derive(Debug, Default)]
pub(crate) struct ReadPlan {
    /// Interned prefixes: `(parent, last element)`; entry 0, the empty
    /// prefix, has no element.
    prefixes: Vec<(PrefixId, Option<PropertyPath>)>,
    by_key: HashMap<(PrefixId, String), PrefixId>,
    /// `(prefix, p)`: the triples `(x, p, ?)` at every node `x` the prefix
    /// reaches.
    pub out: BTreeSet<(PrefixId, String)>,
    /// Prefixes at whose nodes every outgoing triple is read (`sh:closed`).
    pub all_out: BTreeSet<PrefixId>,
    /// `(prefix, p)`: the triples `(?, p, x)` at every node `x` the prefix
    /// reaches (an inverse path step, `sh:targetObjectsOf`).
    pub inv: BTreeSet<(PrefixId, String)>,
    /// Classes whose `rdfs:subClassOf*` closure the run computes
    /// (`sh:targetClass`, `sh:class`).
    pub classes: BTreeSet<String>,
}

impl ReadPlan {
    fn new() -> Self {
        ReadPlan {
            prefixes: vec![(0, None)],
            ..ReadPlan::default()
        }
    }

    /// The prefix `parent` followed by `element`.
    fn child(&mut self, parent: PrefixId, element: &PropertyPath) -> Result<PrefixId, String> {
        let key = (parent, element.to_sparql());
        if let Some(&id) = self.by_key.get(&key) {
            return Ok(id);
        }
        if self.len(parent) >= MAX_PREFIX {
            return Err(format!(
                "a path reaches more than {MAX_PREFIX} steps from the focus node"
            ));
        }
        let id = self.prefixes.len();
        self.prefixes.push((parent, Some(element.clone())));
        self.by_key.insert(key, id);
        Ok(id)
    }

    fn len(&self, mut id: PrefixId) -> usize {
        let mut n = 0;
        while id != 0 {
            n += 1;
            id = self.prefixes[id].0;
        }
        n
    }

    /// The prefix's parent and last element (`None` for the focus node).
    pub(crate) fn split(&self, id: PrefixId) -> (PrefixId, Option<&PropertyPath>) {
        let (parent, element) = &self.prefixes[id];
        (*parent, element.as_ref())
    }

    /// Number of interned prefixes; ids are `0..count`, every parent before
    /// its children.
    pub(crate) fn prefix_count(&self) -> usize {
        self.prefixes.len()
    }

    fn sites(&self) -> usize {
        self.out.len() + self.all_out.len() + self.inv.len()
    }

    fn check_size(&self) -> Result<(), String> {
        if self.sites() > MAX_SITES {
            return Err(format!("the shapes read at more than {MAX_SITES} places"));
        }
        Ok(())
    }
}

/// The read plan of `shapes` (top-level shapes as [`super::engine`] loads
/// them), or why their reads cannot be bounded.
pub(crate) fn read_plan(shapes: &[Shape]) -> Result<ReadPlan, String> {
    let mut plan = ReadPlan::new();
    for shape in shapes.iter().filter(|s| !s.deactivated) {
        // A shape without targets is evaluated only where another shape
        // refers to it, and is planned there.
        if shape.targets.is_empty() {
            continue;
        }
        for target in &shape.targets {
            match target {
                Target::TargetClass(c) => {
                    plan.classes.insert(c.clone());
                    plan.out.insert((0, RDF_TYPE.to_string()));
                }
                Target::TargetNode(_) => {}
                Target::TargetSubjectsOf(p) => {
                    plan.out.insert((0, p.clone()));
                }
                Target::TargetObjectsOf(p) => {
                    plan.inv.insert((0, p.clone()));
                }
                Target::SparqlTarget(_) | Target::SparqlTargetType { .. } => {
                    return Err(format!(
                        "shape <{}> has a SPARQL target, which selects what no path bounds",
                        shape.iri
                    ))
                }
            }
        }
        let mut stack = Vec::new();
        visit_shape(&mut plan, shape, 0, &mut stack)?;
    }
    Ok(plan)
}

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// `shape` evaluated at the nodes `at` reaches (`validate_inline_shape`, and
/// the top-level loop of `validate_graphs`).
fn visit_shape(
    plan: &mut ReadPlan,
    shape: &Shape,
    at: PrefixId,
    stack: &mut Vec<String>,
) -> Result<(), String> {
    if stack.iter().any(|s| s == &shape.iri) {
        return Err(format!("shape <{}> refers back to itself", shape.iri));
    }
    if stack.len() >= MAX_NESTING {
        return Err(format!("shapes nest more than {MAX_NESTING} deep"));
    }
    stack.push(shape.iri.clone());
    for c in &shape.constraints {
        visit_constraint(plan, c, at, None, stack)?;
    }
    for ps in shape.property_shapes.iter().filter(|ps| !ps.deactivated) {
        visit_property(plan, ps, at, stack)?;
    }
    stack.pop();
    plan.check_size()
}

/// A property shape evaluated with the nodes `at` reaches as focus nodes.
fn visit_property(
    plan: &mut ReadPlan,
    ps: &PropertyShape,
    at: PrefixId,
    stack: &mut Vec<String>,
) -> Result<(), String> {
    let path = normal(&ps.path);
    visit_path(plan, &path, at)?;
    for c in &ps.constraints {
        visit_constraint(plan, c, at, Some(&path), stack)?;
    }
    Ok(())
}

/// One constraint, with the nodes `at` reaches as focus nodes and, in a
/// property-shape context, `path` leading to its value nodes.
fn visit_constraint(
    plan: &mut ReadPlan,
    constraint: &Constraint,
    at: PrefixId,
    path: Option<&PropertyPath>,
    stack: &mut Vec<String>,
) -> Result<(), String> {
    // The value nodes: along the path, or the focus node itself.
    let values = match path {
        Some(p) => plan.child(at, p)?,
        None => at,
    };
    match constraint {
        // `is_instance_of`: the value node's types against the class's
        // closure.
        Constraint::Class(c) => {
            plan.classes.insert(c.clone());
            plan.out.insert((values, RDF_TYPE.to_string()));
        }
        // Read the value nodes themselves, which the path already walked.
        Constraint::Datatype(_)
        | Constraint::NodeKind(_)
        | Constraint::MinCount(_)
        | Constraint::MaxCount(_)
        | Constraint::MinExclusive(_)
        | Constraint::MinInclusive(_)
        | Constraint::MaxExclusive(_)
        | Constraint::MaxInclusive(_)
        | Constraint::MinLength(_)
        | Constraint::MaxLength(_)
        | Constraint::Pattern { .. }
        | Constraint::LanguageIn(_)
        | Constraint::UniqueLang(_)
        | Constraint::HasValue(_)
        | Constraint::In(_) => {}
        // The other property's values at the focus node.
        Constraint::Equals(p)
        | Constraint::Disjoint(p)
        | Constraint::LessThan(p)
        | Constraint::LessThanOrEquals(p) => {
            plan.out.insert((at, p.clone()));
        }
        // Every outgoing triple of the focus node.
        Constraint::Closed { .. } => {
            plan.all_out.insert(at);
        }
        // Inline shapes, evaluated at each value node.
        Constraint::Not(s) | Constraint::Node(s) => visit_shape(plan, s, values, stack)?,
        Constraint::And(v) | Constraint::Or(v) | Constraint::Xone(v) => {
            for s in v {
                visit_shape(plan, s, values, stack)?;
            }
        }
        Constraint::Property(ps) => {
            if !ps.deactivated {
                visit_property(plan, ps, values, stack)?;
            }
        }
        Constraint::QualifiedValueShape {
            shape,
            sibling_shapes,
            ..
        } => {
            visit_shape(plan, shape, values, stack)?;
            for s in sibling_shapes {
                visit_shape(plan, s, values, stack)?;
            }
        }
        Constraint::SparqlConstraint { .. } => {
            return Err("a SPARQL constraint (sh:sparql) reads what no path bounds".to_string())
        }
        Constraint::Custom(cc) => {
            return Err(format!(
                "constraint component <{}> runs SPARQL, which reads what no path bounds",
                cc.component
            ))
        }
        Constraint::Expression { .. } => {
            return Err("a node expression (sh:expression) reads what no path bounds".to_string())
        }
    }
    plan.check_size()
}

/// The reads of walking `path` (normalised) from the nodes `at` reaches, as
/// `eval_path_native` walks it.
fn visit_path(plan: &mut ReadPlan, path: &PropertyPath, at: PrefixId) -> Result<(), String> {
    match path {
        PropertyPath::Predicate(p) => {
            plan.out.insert((at, p.clone()));
        }
        PropertyPath::Inverse(inner) => match inner.as_ref() {
            PropertyPath::Predicate(p) => {
                plan.inv.insert((at, p.clone()));
            }
            other => visit_path(plan, &invert(&normal(other)), at)?,
        },
        PropertyPath::Sequence(parts) => {
            let mut here = at;
            for (i, part) in parts.iter().enumerate() {
                visit_path(plan, part, here)?;
                if i + 1 < parts.len() {
                    here = plan.child(here, part)?;
                }
            }
        }
        PropertyPath::Alternative(parts) => {
            for part in parts {
                visit_path(plan, part, at)?;
            }
        }
        // The inner path is walked from every node the closure reaches,
        // the start included.
        PropertyPath::ZeroOrMore(inner) | PropertyPath::OneOrMore(inner) => {
            let reached = plan.child(at, &PropertyPath::ZeroOrMore(inner.clone()))?;
            visit_path(plan, inner, reached)?;
        }
        PropertyPath::ZeroOrOne(inner) => visit_path(plan, inner, at)?,
    }
    plan.check_size()
}

/// `path` with every inverse pushed down onto a predicate, as
/// `eval_path_native` evaluates an inverse of a composite path.
pub(crate) fn normal(path: &PropertyPath) -> PropertyPath {
    match path {
        PropertyPath::Predicate(_) => path.clone(),
        PropertyPath::Inverse(inner) => invert(&normal(inner)),
        PropertyPath::Sequence(parts) => PropertyPath::Sequence(parts.iter().map(normal).collect()),
        PropertyPath::Alternative(parts) => {
            PropertyPath::Alternative(parts.iter().map(normal).collect())
        }
        PropertyPath::ZeroOrMore(inner) => PropertyPath::ZeroOrMore(Box::new(normal(inner))),
        PropertyPath::OneOrMore(inner) => PropertyPath::OneOrMore(Box::new(normal(inner))),
        PropertyPath::ZeroOrOne(inner) => PropertyPath::ZeroOrOne(Box::new(normal(inner))),
    }
}

/// The inverse of a normalised path, normalised.
pub(crate) fn invert(path: &PropertyPath) -> PropertyPath {
    match path {
        PropertyPath::Predicate(_) => PropertyPath::Inverse(Box::new(path.clone())),
        PropertyPath::Inverse(inner) => normal(inner),
        PropertyPath::Sequence(parts) => {
            PropertyPath::Sequence(parts.iter().rev().map(invert).collect())
        }
        PropertyPath::Alternative(parts) => {
            PropertyPath::Alternative(parts.iter().map(invert).collect())
        }
        PropertyPath::ZeroOrMore(inner) => PropertyPath::ZeroOrMore(Box::new(invert(inner))),
        PropertyPath::OneOrMore(inner) => PropertyPath::OneOrMore(Box::new(invert(inner))),
        PropertyPath::ZeroOrOne(inner) => PropertyPath::ZeroOrOne(Box::new(invert(inner))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shacl::shapes::ShapeType;

    fn pred(p: &str) -> PropertyPath {
        PropertyPath::Predicate(format!("http://ex/{p}"))
    }

    fn shape(iri: &str, targets: Vec<Target>, property_shapes: Vec<PropertyShape>) -> Shape {
        Shape {
            iri: iri.into(),
            name: None,
            shape_type: ShapeType::NodeShape,
            targets,
            constraints: Vec::new(),
            property_shapes,
            severity: None,
            message: None,
            deactivated: false,
        }
    }

    fn property(path: PropertyPath, constraints: Vec<Constraint>) -> PropertyShape {
        PropertyShape {
            iri: None,
            path,
            constraints,
            name: None,
            description: None,
            severity: None,
            message: None,
            deactivated: false,
        }
    }

    #[test]
    fn an_inverse_of_a_sequence_is_a_reversed_sequence_of_inverses() {
        let seq = PropertyPath::Sequence(vec![pred("a"), pred("b")]);
        assert_eq!(
            normal(&PropertyPath::Inverse(Box::new(seq.clone()))),
            PropertyPath::Sequence(vec![
                PropertyPath::Inverse(Box::new(pred("b"))),
                PropertyPath::Inverse(Box::new(pred("a"))),
            ])
        );
        assert_eq!(invert(&invert(&seq)), seq);
    }

    /// A sequence path reads its first predicate at the focus node and its
    /// second at the nodes the first reaches; `sh:class` on its values reads
    /// their types there.
    #[test]
    fn a_sequence_path_reads_each_step_where_it_is_taken() {
        let s = shape(
            "http://ex/S",
            vec![Target::TargetClass("http://ex/C".into())],
            vec![property(
                PropertyPath::Sequence(vec![pred("a"), pred("b")]),
                vec![Constraint::Class("http://ex/D".into())],
            )],
        );
        let plan = read_plan(&[s]).unwrap();
        let after_a = plan.by_key[&(0, pred("a").to_sparql())];
        let seq = PropertyPath::Sequence(vec![pred("a"), pred("b")]);
        let values = plan.by_key[&(0, seq.to_sparql())];
        assert!(plan.out.contains(&(0, "http://ex/a".into())));
        assert!(plan.out.contains(&(after_a, "http://ex/b".into())));
        assert!(plan.out.contains(&(values, RDF_TYPE.into())));
        assert!(plan.out.contains(&(0, RDF_TYPE.into())), "the class target");
        assert_eq!(
            plan.classes,
            ["http://ex/C", "http://ex/D"]
                .into_iter()
                .map(String::from)
                .collect()
        );
    }

    #[test]
    fn sparql_and_self_reference_have_no_plan() {
        let node =
            || Target::TargetNode(oxigraph::model::NamedNode::new_unchecked("http://ex/n").into());
        let sparql = Shape {
            constraints: vec![Constraint::SparqlConstraint {
                node: "_:c".into(),
                select: "SELECT $this WHERE {}".into(),
                message: None,
                severity: None,
                annotations: Vec::new(),
            }],
            ..shape("http://ex/S", vec![node()], vec![])
        };
        assert!(read_plan(&[sparql]).unwrap_err().contains("SPARQL"));

        let inner = shape("http://ex/S", vec![], vec![]);
        let looping = Shape {
            constraints: vec![Constraint::Node(Box::new(inner))],
            ..shape("http://ex/S", vec![node()], vec![])
        };
        assert!(read_plan(&[looping]).unwrap_err().contains("refers back"));
    }
}
