//! SHACL-AF node expressions (SHACL Advanced Features §6).
//!
//! A node expression is declared in the shapes graph and computes a set of
//! RDF terms for a focus node. The seven kinds of the 2017 Note:
//!
//! | kind | syntax | `Eval(expr, $this)` |
//! |---|---|---|
//! | focus node | `sh:this` | `{ $this }` |
//! | constant term | any other IRI or literal | `{ expr }` |
//! | path | `[ sh:path P ; sh:nodes N ]` | values of `P` from each node of `N` (default `sh:this`) |
//! | filter shape | `[ sh:filterShape S ; sh:nodes N ]` | the nodes of `N` that conform to `S` |
//! | intersection | `[ sh:intersection ( E1 E2 … ) ]` | nodes in every `Ei` |
//! | union | `[ sh:union ( E1 E2 … ) ]` | nodes in any `Ei` |
//! | function | `[ f ( E1 E2 … ) ]` | `f(a1, a2, …)` for every combination of the `Ei`'s nodes |
//!
//! They are used by expression constraints (`sh:expression`, §7) and triple
//! rules (`sh:subject` / `sh:predicate` / `sh:object`, §8.5). The shapes graph
//! is parsed into [`NodeExpr`] once per run (`engine::load_node_expr`); this
//! module evaluates it against the run's [`DataView`], so a path reads the run's
//! snapshot and data graphs and a function call is one query of the run — its
//! body, for a `sh:SPARQLFunction`, reads the same data.

use super::constraints::{validate_inline_shape, value_nodes};
use super::report::Severity;
use super::shapes::{PropertyPath, Shape};
use super::view::DataView;
use opengraph::spargebra::Query as SpargebraQuery;
use oxigraph::model::Term;
use oxigraph::sparql::{QueryResults, Variable};

/// How many argument combinations one function expression may call its
/// function with, for one focus node, before the evaluation fails.
pub(crate) const MAX_CALLS: usize = 10_000;

#[derive(Debug, Clone)]
pub enum NodeExpr {
    /// `sh:this`.
    This,
    /// Any other IRI, or a literal.
    Constant(Term),
    Path {
        path: PropertyPath,
        /// `sh:nodes`; the focus node when absent.
        nodes: Option<Box<NodeExpr>>,
    },
    Filter {
        shape: Box<Shape>,
        nodes: Box<NodeExpr>,
    },
    Intersection(Vec<NodeExpr>),
    Union(Vec<NodeExpr>),
    Function(Box<FunctionCall>),
}

impl NodeExpr {
    /// Every shape and property path the expression uses, nested ones
    /// included — for the run's class sets and adjacency index.
    pub(crate) fn visit(
        &self,
        on_shape: &mut impl FnMut(&Shape),
        on_path: &mut impl FnMut(&PropertyPath),
    ) {
        match self {
            NodeExpr::This | NodeExpr::Constant(_) => {}
            NodeExpr::Path { path, nodes } => {
                on_path(path);
                if let Some(n) = nodes {
                    n.visit(on_shape, on_path);
                }
            }
            NodeExpr::Filter { shape, nodes } => {
                on_shape(shape);
                nodes.visit(on_shape, on_path);
            }
            NodeExpr::Intersection(members) | NodeExpr::Union(members) => {
                for m in members {
                    m.visit(on_shape, on_path);
                }
            }
            NodeExpr::Function(call) => {
                for a in &call.args {
                    a.visit(on_shape, on_path);
                }
            }
        }
    }
}

/// A function expression: the call `iri(a0, a1, …)`, prepared as
/// `SELECT ?result ?a0 ?a1 … WHERE { BIND (<iri>(?a0, ?a1, …) AS ?result) }`
/// so the arguments are bound as terms, never pasted into query text.
#[derive(Debug, Clone)]
pub struct FunctionCall {
    pub iri: String,
    pub args: Vec<NodeExpr>,
    /// Per argument: whether the function may be called without it
    /// (`sh:optional` on the declared parameter). Trailing optional
    /// arguments with no value are left out of the call.
    pub optional: Vec<bool>,
    /// The call with every argument, parsed once.
    full: (SpargebraQuery, Vec<Variable>),
}

impl FunctionCall {
    pub(crate) fn new(
        iri: String,
        args: Vec<NodeExpr>,
        optional: Vec<bool>,
    ) -> Result<Self, String> {
        let full = Self::prepare(&iri, args.len())?;
        Ok(Self {
            iri,
            args,
            optional,
            full,
        })
    }

    /// The prepared call with `arity` arguments.
    fn query(&self, arity: usize) -> Result<(SpargebraQuery, Vec<Variable>), String> {
        if arity == self.args.len() {
            Ok(self.full.clone())
        } else {
            Self::prepare(&self.iri, arity)
        }
    }

    fn prepare(iri: &str, arity: usize) -> Result<(SpargebraQuery, Vec<Variable>), String> {
        let vars: Vec<String> = (0..arity).map(|i| format!("a{i}")).collect();
        let text = format!(
            "SELECT ?result {} WHERE {{ BIND (<{}>({}) AS ?result) }}",
            vars.iter()
                .map(|v| format!("?{v}"))
                .collect::<Vec<_>>()
                .join(" "),
            iri,
            vars.iter()
                .map(|v| format!("?{v}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let query = crate::sparql::parser()
            .parse_query(&text)
            .map_err(|e| format!("function expression <{iri}>: {e}"))?;
        let vars = vars
            .iter()
            .map(|v| Variable::new_unchecked(v.as_str()))
            .collect();
        Ok((query, vars))
    }
}

/// `Eval(expr, focus)`: the expression's output nodes, without duplicates, in
/// the order they were found. `Err` when a function call cannot be evaluated
/// or would be made more than [`MAX_CALLS`] times.
pub(crate) fn eval(
    view: &DataView<'_>,
    shapes: &[Shape],
    expr: &NodeExpr,
    focus: &Term,
) -> Result<Vec<Term>, String> {
    let mut out = match expr {
        NodeExpr::This => vec![focus.clone()],
        NodeExpr::Constant(term) => vec![term.clone()],
        NodeExpr::Path { path, nodes } => {
            let inputs = match nodes {
                Some(n) => eval(view, shapes, n, focus)?,
                None => vec![focus.clone()],
            };
            inputs
                .iter()
                .flat_map(|n| value_nodes(view, n, Some(path)))
                .collect()
        }
        NodeExpr::Filter { shape, nodes } => eval(view, shapes, nodes, focus)?
            .into_iter()
            .filter(|n| {
                validate_inline_shape(view, shapes, n, shape, &Severity::Violation).is_empty()
            })
            .collect(),
        NodeExpr::Union(members) => {
            let mut all = Vec::new();
            for m in members {
                all.extend(eval(view, shapes, m, focus)?);
            }
            all
        }
        NodeExpr::Intersection(members) => {
            let mut sets = Vec::with_capacity(members.len());
            for m in members {
                sets.push(eval(view, shapes, m, focus)?);
            }
            let mut sets = sets.into_iter();
            let first = sets.next().unwrap_or_default();
            let rest: Vec<Vec<Term>> = sets.collect();
            first
                .into_iter()
                .filter(|t| rest.iter().all(|s| s.contains(t)))
                .collect()
        }
        NodeExpr::Function(call) => call_function(view, shapes, call, focus)?,
    };
    dedup(&mut out);
    Ok(out)
}

/// §6.4: the function's results for every combination of its arguments'
/// output nodes. A mandatory argument with no output means no call at all.
fn call_function(
    view: &DataView<'_>,
    shapes: &[Shape],
    call: &FunctionCall,
    focus: &Term,
) -> Result<Vec<Term>, String> {
    let mut args: Vec<Vec<Term>> = Vec::with_capacity(call.args.len());
    for a in &call.args {
        args.push(eval(view, shapes, a, focus)?);
    }
    // Leave out trailing optional arguments that have no value; any other
    // empty argument is a mandatory one, or one before a value (which a
    // positional call cannot skip): no result.
    let mut arity = args.len();
    while arity > 0 && args[arity - 1].is_empty() && call.optional.get(arity - 1) == Some(&true) {
        arity -= 1;
    }
    if args[..arity].iter().any(Vec::is_empty) {
        return Ok(Vec::new());
    }
    let combinations = args[..arity]
        .iter()
        .try_fold(1usize, |n, a| n.checked_mul(a.len()))
        .filter(|n| *n <= MAX_CALLS)
        .ok_or_else(|| {
            format!(
                "function expression <{}> would be called more than {MAX_CALLS} times for one \
                 focus node",
                call.iri
            )
        })?;
    let (query, vars) = call.query(arity)?;
    let mut out = Vec::new();
    for i in 0..combinations {
        // The i-th combination, mixed-radix over the argument sets.
        let mut rest = i;
        let mut bindings = Vec::with_capacity(arity);
        for (var, values) in vars.iter().zip(&args[..arity]) {
            bindings.push((var.clone(), values[rest % values.len()].clone()));
            rest /= values.len();
        }
        match view.query_bound(&query, &bindings)? {
            QueryResults::Solutions(rows) => {
                for row in rows {
                    let row = row.map_err(|e| e.to_string())?;
                    // An unbound result is the function's error (§5.4): no node.
                    out.extend(row.get("result").cloned());
                }
            }
            _ => return Err("a function expression must be evaluated as a SELECT".to_string()),
        }
    }
    Ok(out)
}

fn dedup(terms: &mut Vec<Term>) {
    let mut seen = std::collections::HashSet::with_capacity(terms.len());
    terms.retain(|t| seen.insert(t.clone()));
}

/// The outputs of an expression constraint are exactly `{ true }` (§7).
pub(crate) fn is_exactly_true(nodes: &[Term]) -> bool {
    matches!(nodes, [Term::Literal(l)]
        if l.value() == "true"
            && l.datatype().as_str() == "http://www.w3.org/2001/XMLSchema#boolean")
}
