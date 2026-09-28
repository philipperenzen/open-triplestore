//! The evaluator over the columnar copy: SPARQL algebra on id rows, with
//! terms decoded only where an expression, an order key or the final
//! projection needs them. What it does not implement is declined *before*
//! evaluation ([`accepts`]), so a declined query costs a parse and a walk
//! and the engine answers it — never a different answer.

use std::collections::{HashMap, HashSet};

use oxrdf::vocab::xsd;
use oxrdf::{Literal, NamedNode, Term, Triple, Variable};
use spargebra::algebra::{
    AggregateExpression, AggregateFunction, Expression, Function, GraphPattern, OrderExpression,
    QueryDataset,
};
use spargebra::term::{GroundTerm, NamedNodePattern, TermPattern, TriplePattern};
use spargebra::Query;

use super::index::{Columnar, DEFAULT_GRAPH};
use super::value::{order_cmp, Decline, EvalError, Numeric, Value};
use crate::parallel::ParAnswer;

// ─── Acceptance ─────────────────────────────────────────────────────────────

/// Whether the evaluator implements every construct of `query`.
pub fn accepts(query: &Query) -> Result<(), Decline> {
    match query {
        Query::Select { pattern, .. } | Query::Ask { pattern, .. } => accepts_pattern(pattern),
        Query::Construct {
            template, pattern, ..
        } => {
            for t in template {
                if matches!(t.subject, TermPattern::BlankNode(_))
                    || matches!(t.object, TermPattern::BlankNode(_))
                {
                    return Err(Decline("CONSTRUCT template with blank nodes"));
                }
            }
            accepts_pattern(pattern)
        }
        Query::Describe { .. } => Err(Decline("DESCRIBE")),
    }
}

fn accepts_pattern(p: &GraphPattern) -> Result<(), Decline> {
    match p {
        GraphPattern::Bgp { patterns } => {
            for t in patterns {
                accepts_triple(t)?;
            }
            Ok(())
        }
        // A `Path` node is what the parser emits for a path it could not
        // reduce to plain triple patterns — an alternative, or a sequence with
        // an alternative on one side. It carries a fresh blank node as the
        // join key between sibling nodes, and this evaluator drops blank-node
        // columns at the end of each basic graph pattern, so the join would
        // become a cross product. A simple sequence folds into one `Bgp` and
        // never reaches here.
        GraphPattern::Path { .. } => Err(Decline("property path")),
        GraphPattern::Join { left, right }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right } => {
            accepts_pattern(left)?;
            accepts_pattern(right)
        }
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => {
            accepts_pattern(left)?;
            accepts_pattern(right)?;
            if let Some(e) = expression {
                accepts_expr(e)?;
            }
            Ok(())
        }
        GraphPattern::Lateral { .. } => Err(Decline("LATERAL")),
        GraphPattern::Filter { expr, inner } => {
            accepts_expr(expr)?;
            accepts_pattern(inner)
        }
        GraphPattern::Graph { name, inner } => {
            if let NamedNodePattern::Variable(v) = name {
                // `GRAPH ?g { }` — or any body that might bind no triple —
                // must produce one solution per named graph; this evaluator
                // seeds `?g` unbound and produces one. And a `?g` used inside
                // the body would have to constrain the graph it ranges over.
                if !surely_binds_a_triple(inner) {
                    return Err(Decline("GRAPH ?g over a body that binds no triple"));
                }
                if pattern_uses_variable(inner, v) {
                    return Err(Decline("GRAPH ?g with ?g used inside"));
                }
            }
            accepts_pattern(inner)
        }
        GraphPattern::Extend {
            inner, expression, ..
        } => {
            accepts_expr(expression)?;
            accepts_pattern(inner)
        }
        GraphPattern::Values { bindings, .. } => {
            for row in bindings {
                for t in row.iter().flatten() {
                    match t {
                        GroundTerm::NamedNode(_) | GroundTerm::Literal(_) => {}
                        #[allow(unreachable_patterns)]
                        _ => return Err(Decline("quoted triple in VALUES")),
                    }
                }
            }
            Ok(())
        }
        GraphPattern::OrderBy { inner, expression } => {
            for e in expression {
                match e {
                    OrderExpression::Asc(e) | OrderExpression::Desc(e) => accepts_expr(e)?,
                }
            }
            accepts_pattern(inner)
        }
        GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. } => accepts_pattern(inner),
        GraphPattern::Group {
            inner, aggregates, ..
        } => {
            for (_, a) in aggregates {
                match a {
                    AggregateExpression::CountSolutions { .. } => {}
                    AggregateExpression::FunctionCall { name, expr, .. } => {
                        if matches!(name, AggregateFunction::Custom(_)) {
                            return Err(Decline("custom aggregate"));
                        }
                        accepts_expr(expr)?;
                    }
                }
            }
            accepts_pattern(inner)
        }
        GraphPattern::Service { .. } => Err(Decline("SERVICE")),
    }
}

fn accepts_triple(t: &TriplePattern) -> Result<(), Decline> {
    accepts_term(&t.subject)?;
    accepts_term(&t.object)
}

fn accepts_term(t: &TermPattern) -> Result<(), Decline> {
    match t {
        TermPattern::NamedNode(_)
        | TermPattern::BlankNode(_)
        | TermPattern::Literal(_)
        | TermPattern::Variable(_) => Ok(()),
        // A SPARQL-star build: quoted triple patterns are declined.
        #[allow(unreachable_patterns)]
        _ => Err(Decline("quoted triple pattern")),
    }
}

/// Whether every solution of `p` comes from at least one triple pattern, so
/// a `GRAPH ?g` above it binds `?g` for each solution. A group with no triple
/// pattern (`{ }`, a lone `FILTER`, a `VALUES`) produces a solution that names
/// no graph, which is where this evaluator and the engine part company.
fn surely_binds_a_triple(p: &GraphPattern) -> bool {
    match p {
        GraphPattern::Bgp { patterns } => !patterns.is_empty(),
        GraphPattern::Path { .. } => true,
        // Both branches of a union must bind one; for a join or an optional
        // the required side is enough.
        GraphPattern::Union { left, right } => {
            surely_binds_a_triple(left) && surely_binds_a_triple(right)
        }
        GraphPattern::Join { left, right } => {
            surely_binds_a_triple(left) || surely_binds_a_triple(right)
        }
        GraphPattern::LeftJoin { left, .. } | GraphPattern::Minus { left, .. } => {
            surely_binds_a_triple(left)
        }
        GraphPattern::Filter { inner, .. }
        | GraphPattern::Graph { inner, .. }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Group { inner, .. }
        | GraphPattern::Lateral { right: inner, .. } => surely_binds_a_triple(inner),
        _ => false,
    }
}

/// Whether `v` appears anywhere in `p` — in a triple position, an expression,
/// a `VALUES` header or a nested graph name.
fn pattern_uses_variable(p: &GraphPattern, v: &Variable) -> bool {
    fn in_term(t: &TermPattern, v: &Variable) -> bool {
        matches!(t, TermPattern::Variable(x) if x == v)
    }
    fn in_named(n: &NamedNodePattern, v: &Variable) -> bool {
        matches!(n, NamedNodePattern::Variable(x) if x == v)
    }
    match p {
        GraphPattern::Bgp { patterns } => patterns
            .iter()
            .any(|t| in_term(&t.subject, v) || in_named(&t.predicate, v) || in_term(&t.object, v)),
        GraphPattern::Path {
            subject, object, ..
        } => in_term(subject, v) || in_term(object, v),
        GraphPattern::Join { left, right }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right }
        | GraphPattern::Lateral { left, right } => {
            pattern_uses_variable(left, v) || pattern_uses_variable(right, v)
        }
        GraphPattern::LeftJoin { left, right, .. } => {
            pattern_uses_variable(left, v) || pattern_uses_variable(right, v)
        }
        GraphPattern::Graph { name, inner } => in_named(name, v) || pattern_uses_variable(inner, v),
        GraphPattern::Extend {
            inner, variable, ..
        } => variable == v || pattern_uses_variable(inner, v),
        GraphPattern::Values { variables, .. } => variables.iter().any(|x| x == v),
        GraphPattern::Group {
            inner,
            variables,
            aggregates,
        } => {
            variables.iter().any(|x| x == v)
                || aggregates.iter().any(|(x, _)| x == v)
                || pattern_uses_variable(inner, v)
        }
        GraphPattern::Project { inner, variables } => {
            variables.iter().any(|x| x == v) || pattern_uses_variable(inner, v)
        }
        GraphPattern::Filter { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. } => pattern_uses_variable(inner, v),
        _ => true, // Unknown shape: assume it does, and decline.
    }
}

fn accepts_expr(e: &Expression) -> Result<(), Decline> {
    match e {
        Expression::NamedNode(_) | Expression::Literal(_) | Expression::Variable(_) => Ok(()),
        Expression::Or(a, b)
        | Expression::And(a, b)
        | Expression::Equal(a, b)
        | Expression::SameTerm(a, b)
        | Expression::Greater(a, b)
        | Expression::GreaterOrEqual(a, b)
        | Expression::Less(a, b)
        | Expression::LessOrEqual(a, b)
        | Expression::Add(a, b)
        | Expression::Subtract(a, b)
        | Expression::Multiply(a, b)
        | Expression::Divide(a, b) => {
            accepts_expr(a)?;
            accepts_expr(b)
        }
        Expression::In(a, list) => {
            accepts_expr(a)?;
            for x in list {
                accepts_expr(x)?;
            }
            Ok(())
        }
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => {
            accepts_expr(a)
        }
        Expression::Exists(_) => Err(Decline("EXISTS")),
        Expression::Bound(_) => Ok(()),
        Expression::If(a, b, c) => {
            accepts_expr(a)?;
            accepts_expr(b)?;
            accepts_expr(c)
        }
        Expression::Coalesce(list) => {
            for x in list {
                accepts_expr(x)?;
            }
            Ok(())
        }
        Expression::FunctionCall(f, args) => {
            // Regular expressions are declined outright. The work is string
            // matching over decoded terms, which the engine does against its
            // own storage; this copy has no index to bring to it and pays a
            // dictionary round trip per solution, measured at 3x the engine
            // on `query/regex_filter` even with the pattern compiled once.
            if matches!(f, Function::Regex | Function::Replace) {
                return Err(Decline("REGEX/REPLACE"));
            }
            match f {
                Function::Str
                | Function::Lang
                | Function::LangMatches
                | Function::Datatype
                | Function::Iri
                | Function::Abs
                | Function::Ceil
                | Function::Floor
                | Function::Round
                | Function::Concat
                | Function::StrLen
                | Function::UCase
                | Function::LCase
                | Function::EncodeForUri
                | Function::Contains
                | Function::StrStarts
                | Function::StrEnds
                | Function::StrBefore
                | Function::StrAfter
                | Function::IsIri
                | Function::IsBlank
                | Function::IsLiteral
                | Function::IsNumeric => {}
                _ => return Err(Decline("function")),
            }
            for a in args {
                accepts_expr(a)?;
            }
            Ok(())
        }
    }
}

// ─── Tables ─────────────────────────────────────────────────────────────────

type Id = u32;
type IdRow = Vec<Option<Id>>;

/// Solutions with a shared variable header.
#[derive(Clone, Debug, Default)]
struct Table {
    vars: Vec<Variable>,
    rows: Vec<IdRow>,
}

impl Table {
    fn unit() -> Self {
        Self {
            vars: Vec::new(),
            rows: vec![Vec::new()],
        }
    }

    fn empty() -> Self {
        Self::default()
    }

    fn col(&self, v: &Variable) -> Option<usize> {
        self.vars.iter().position(|x| x == v)
    }
}

/// Which graphs a triple pattern reads.
#[derive(Clone, Debug)]
enum Scope {
    /// The default graph(s) of the dataset; more than one means the union.
    Default,
    /// One named graph.
    Named(Id),
    /// `GRAPH ?g`: every named graph of the dataset, binding the variable.
    Var(Variable),
}

struct Ctx<'a> {
    idx: &'a Columnar,
    /// Terms produced by expressions that the dictionary does not hold.
    extra: Vec<Term>,
    extra_index: HashMap<Term, Id>,
    /// Decoded values, by id, on demand.
    values: HashMap<Id, Value>,
    default_graphs: Vec<Id>,
    named_graphs: Vec<Id>,
}

impl<'a> Ctx<'a> {
    fn new(idx: &'a Columnar, dataset: Option<&QueryDataset>) -> Result<Self, Decline> {
        let (default_graphs, named_graphs) = match dataset {
            None => (vec![DEFAULT_GRAPH], idx.named_graphs().collect()),
            Some(ds) => {
                let lookup = |n: &NamedNode| idx.dict().lookup(&Term::NamedNode(n.clone()));
                let default: Vec<Id> = ds.default.iter().filter_map(lookup).collect();
                let named: Vec<Id> = match &ds.named {
                    Some(list) => list.iter().filter_map(lookup).collect(),
                    None => idx.named_graphs().collect(),
                };
                (default, named)
            }
        };
        Ok(Self {
            idx,
            extra: Vec::new(),
            extra_index: HashMap::new(),
            values: HashMap::new(),
            default_graphs,
            named_graphs,
        })
    }

    fn base(&self) -> Id {
        self.idx.dict().len() as Id
    }

    fn term(&self, id: Id) -> Option<&Term> {
        if id < self.base() {
            self.idx.dict().get(id)
        } else {
            self.extra.get((id - self.base()) as usize)
        }
    }

    fn value(&mut self, id: Id) -> Result<Value, EvalError> {
        if let Some(v) = self.values.get(&id) {
            return Ok(v.clone());
        }
        let term = self.term(id).ok_or(EvalError)?.clone();
        let v = Value::from_term(&term).map_err(|_| EvalError)?;
        self.values.insert(id, v.clone());
        Ok(v)
    }

    fn intern(&mut self, term: &Term) -> Id {
        if let Some(id) = self.idx.dict().lookup(term) {
            return id;
        }
        if let Some(id) = self.extra_index.get(term) {
            return *id;
        }
        let id = self.base() + self.extra.len() as Id;
        self.extra.push(term.clone());
        self.extra_index.insert(term.clone(), id);
        id
    }

    fn intern_value(&mut self, v: &Value) -> Id {
        let term = v.to_term();
        let id = self.intern(&term);
        self.values.entry(id).or_insert_with(|| v.clone());
        id
    }
}

// ─── Entry points ───────────────────────────────────────────────────────────

/// Evaluate a parsed query. `Ok(None)` is a decline; `Err` a real
/// evaluation failure.
/// Above this many rows, a query that is a single unconstrained triple
/// pattern is left to the engine: there is no join for the index to
/// accelerate, and this copy would only materialise the answer a second
/// time. Measured on `query/simple_lookup`: at ten thousand rows the copy
/// is 50 % faster than the engine, at a hundred thousand it is slower.
const BULK_SCAN_ROWS: usize = 50_000;

/// Triple patterns anywhere under `p` — what a row budget would have to stop
/// early *through*, not just at.
fn triple_patterns(p: &GraphPattern) -> usize {
    match p {
        GraphPattern::Bgp { patterns } => patterns.len(),
        GraphPattern::Path { .. } => 1,
        GraphPattern::Join { left, right }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right }
        | GraphPattern::Lateral { left, right } => triple_patterns(left) + triple_patterns(right),
        GraphPattern::LeftJoin { left, right, .. } => {
            triple_patterns(left) + triple_patterns(right)
        }
        GraphPattern::Filter { inner, .. }
        | GraphPattern::Graph { inner, .. }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Group { inner, .. } => triple_patterns(inner),
        _ => 0,
    }
}

/// Whether a `Group` sits anywhere below `p` — which makes a filter above it
/// a `HAVING`, testing one row per group rather than one per solution.
fn contains_group(p: &GraphPattern) -> bool {
    match p {
        GraphPattern::Group { .. } => true,
        GraphPattern::Join { left, right }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right }
        | GraphPattern::Lateral { left, right } => contains_group(left) || contains_group(right),
        GraphPattern::LeftJoin { left, right, .. } => contains_group(left) || contains_group(right),
        GraphPattern::Filter { inner, .. }
        | GraphPattern::Graph { inner, .. }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. } => contains_group(inner),
        _ => false,
    }
}

/// Whether a `FILTER` in `p` tests *solutions* rather than groups. A `HAVING`
/// — a filter above a `GROUP BY` — runs once per group and costs nothing worth
/// declining for; a filter on raw solutions decodes every candidate row.
fn has_row_filter(p: &GraphPattern) -> bool {
    match p {
        GraphPattern::Filter { inner, .. } => !contains_group(inner) || has_row_filter(inner),
        GraphPattern::Join { left, right }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right }
        | GraphPattern::Lateral { left, right } => has_row_filter(left) || has_row_filter(right),
        GraphPattern::LeftJoin { left, right, .. } => has_row_filter(left) || has_row_filter(right),
        GraphPattern::Graph { inner, .. }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Group { inner, .. } => has_row_filter(inner),
        _ => false,
    }
}

/// A `LIMIT` over more than one triple pattern. The budget reaches only the
/// last pattern of a basic graph pattern — an earlier one's rows can still be
/// dropped by a later one — so every stage but the last is materialised in
/// full, while the engine stops early throughout. Measured on the perf gate's
/// `concurrent/reads` (a two-pattern join, `LIMIT 100`, 50 000 triples): 377 us
/// for the engine against 605 us here. Declined for speed, not fidelity.
fn limited_over_several_patterns(p: &GraphPattern) -> bool {
    match p {
        GraphPattern::Slice {
            inner,
            length: Some(_),
            ..
        } => triple_patterns(inner) > 1 || limited_over_several_patterns(inner),
        GraphPattern::Join { left, right }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right }
        | GraphPattern::Lateral { left, right } => {
            limited_over_several_patterns(left) || limited_over_several_patterns(right)
        }
        GraphPattern::LeftJoin { left, right, .. } => {
            limited_over_several_patterns(left) || limited_over_several_patterns(right)
        }
        GraphPattern::Filter { inner, .. }
        | GraphPattern::Graph { inner, .. }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Group { inner, .. } => limited_over_several_patterns(inner),
        _ => false,
    }
}

/// The exact row count when `pattern` is one triple pattern with nothing
/// above it that bounds or reduces the rows — two binary searches, no scan.
fn bulk_scan_rows(idx: &Columnar, pattern: &GraphPattern) -> Option<usize> {
    let mut p = pattern;
    loop {
        match p {
            // Neither drops nor bounds rows.
            GraphPattern::Project { inner, .. } | GraphPattern::Extend { inner, .. } => p = inner,
            GraphPattern::Bgp { patterns } if patterns.len() == 1 => {
                let t = &patterns[0];
                let lookup = |x: &TermPattern| match x {
                    TermPattern::Variable(_) | TermPattern::BlankNode(_) => Some(None),
                    TermPattern::NamedNode(n) => {
                        idx.dict().lookup(&Term::NamedNode(n.clone())).map(Some)
                    }
                    TermPattern::Literal(l) => {
                        idx.dict().lookup(&Term::Literal(l.clone())).map(Some)
                    }
                    #[allow(unreachable_patterns)]
                    _ => None,
                };
                let s = lookup(&t.subject)?;
                let o = lookup(&t.object)?;
                let pr = match &t.predicate {
                    NamedNodePattern::Variable(_) => None,
                    NamedNodePattern::NamedNode(n) => {
                        Some(idx.dict().lookup(&Term::NamedNode(n.clone()))?)
                    }
                };
                let (perm, prefix) = Columnar::plan(DEFAULT_GRAPH, s, pr, o);
                return Some(idx.count(perm, &prefix));
            }
            _ => return None,
        }
    }
}

/// Whether an `ORDER BY` sits under the `LIMIT`, in which case nothing can
/// stop early on either side and the budget costs nothing.
fn orders_before_limiting(p: &GraphPattern) -> bool {
    match p {
        GraphPattern::OrderBy { .. } => true,
        GraphPattern::Slice { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::Group { inner, .. } => orders_before_limiting(inner),
        _ => false,
    }
}

pub fn evaluate(idx: &Columnar, query: &Query) -> Result<Option<ParAnswer>, String> {
    if declines_on_cost(idx, query) {
        return Ok(None);
    }
    evaluate_semantics(idx, query)
}

/// The cost-based declines, separately from the semantic ones.
///
/// These say nothing about whether this evaluator *can* answer a query — it
/// can, identically — only that the engine answers it faster, which the
/// benchmarks measured shape by shape. They are a routing policy, so
/// [`evaluate_semantics`] skips them and the parity suite holds the evaluator
/// to the engine's answer for these shapes too.
fn declines_on_cost(idx: &Columnar, query: &Query) -> bool {
    let Query::Select {
        pattern, dataset, ..
    } = query
    else {
        return false;
    };
    {
        if dataset.is_none() && bulk_scan_rows(idx, pattern).is_some_and(|n| n > BULK_SCAN_ROWS) {
            return true;
        }
        // An `ORDER BY` has to see every row anyway, so a limit above one costs
        // the engine the same full evaluation and this copy keeps the shape.
        if limited_over_several_patterns(pattern) && !orders_before_limiting(pattern) {
            return true;
        }
        // A filter over a single triple pattern. Finding rows is this copy's
        // advantage — a binary search on a sorted permutation of ids — and
        // testing them is its disadvantage, because each candidate has to go
        // back through the dictionary to become a term the filter can read. A
        // join pays for that decode many times over; one pattern has nothing
        // to pay it with, and the gate measured `query/filter` slower on three
        // consecutive runs (+19 %, +15 %, +40 %). Declined for speed, not
        // fidelity: a filtered *join* keeps the shape, and so does an
        // unfiltered single pattern.
        if has_row_filter(pattern) && triple_patterns(pattern) <= 1 {
            return true;
        }
    }
    false
}

/// Evaluate without the cost-based declines: what this evaluator answers when
/// it is asked, which is what the parity suite checks against the engine.
pub fn evaluate_semantics(idx: &Columnar, query: &Query) -> Result<Option<ParAnswer>, String> {
    if accepts(query).is_err() {
        return Ok(None);
    }
    match query {
        Query::Select {
            pattern, dataset, ..
        } => {
            let mut ctx = match Ctx::new(idx, dataset.as_ref()) {
                Ok(c) => c,
                Err(_) => return Ok(None),
            };
            let table = match eval(pattern, &mut ctx, &Scope::Default, None) {
                Ok(t) => t,
                Err(_) => return Ok(None),
            };
            let rows: Vec<Vec<Option<Term>>> = table
                .rows
                .iter()
                .map(|r| {
                    r.iter()
                        .map(|id| id.and_then(|id| ctx.term(id).cloned()))
                        .collect()
                })
                .collect();
            Ok(Some(ParAnswer::Solutions {
                variables: table.vars,
                rows,
            }))
        }
        Query::Ask {
            pattern, dataset, ..
        } => {
            let mut ctx = match Ctx::new(idx, dataset.as_ref()) {
                Ok(c) => c,
                Err(_) => return Ok(None),
            };
            // An ASK is answered by the first solution; one row is enough.
            match eval(pattern, &mut ctx, &Scope::Default, Some(1)) {
                Ok(t) => Ok(Some(ParAnswer::Boolean(!t.rows.is_empty()))),
                Err(_) => Ok(None),
            }
        }
        Query::Construct {
            template,
            pattern,
            dataset,
            ..
        } => {
            let mut ctx = match Ctx::new(idx, dataset.as_ref()) {
                Ok(c) => c,
                Err(_) => return Ok(None),
            };
            let table = match eval(pattern, &mut ctx, &Scope::Default, None) {
                Ok(t) => t,
                Err(_) => return Ok(None),
            };
            let mut out: Vec<Triple> = Vec::new();
            let mut seen: HashSet<Triple> = HashSet::new();
            for row in &table.rows {
                for t in template {
                    let s = match instantiate(&t.subject, row, &table, &ctx) {
                        Some(Term::NamedNode(n)) => oxrdf::NamedOrBlankNode::from(n),
                        Some(Term::BlankNode(b)) => oxrdf::NamedOrBlankNode::from(b),
                        _ => continue,
                    };
                    let p = match &t.predicate {
                        NamedNodePattern::NamedNode(n) => n.clone(),
                        NamedNodePattern::Variable(v) => match table
                            .col(v)
                            .and_then(|c| row[c])
                            .and_then(|id| ctx.term(id).cloned())
                        {
                            Some(Term::NamedNode(n)) => n,
                            _ => continue,
                        },
                    };
                    let Some(o) = instantiate(&t.object, row, &table, &ctx) else {
                        continue;
                    };
                    let triple = Triple::new(s, p, o);
                    if seen.insert(triple.clone()) {
                        out.push(triple);
                    }
                }
            }
            Ok(Some(ParAnswer::Graph(out)))
        }
        Query::Describe { .. } => Ok(None),
    }
}

fn instantiate(t: &TermPattern, row: &IdRow, table: &Table, ctx: &Ctx<'_>) -> Option<Term> {
    match t {
        TermPattern::NamedNode(n) => Some(Term::NamedNode(n.clone())),
        TermPattern::Literal(l) => Some(Term::Literal(l.clone())),
        TermPattern::Variable(v) => table
            .col(v)
            .and_then(|c| row[c])
            .and_then(|id| ctx.term(id).cloned()),
        _ => None,
    }
}

// ─── Patterns ───────────────────────────────────────────────────────────────

/// Evaluate one algebra node.
///
/// `cap` is the caller's row budget: it will use at most that many rows, in
/// the order this call produces them, so producing more is waste and stopping
/// there changes nothing. It is forwarded only where an operator is 1:1 and
/// order-preserving, or where the budget can be widened arithmetically to
/// cover what the operator drops; everything else bounds its own output.
fn eval(
    p: &GraphPattern,
    ctx: &mut Ctx<'_>,
    scope: &Scope,
    cap: Option<usize>,
) -> Result<Table, EvalError> {
    match p {
        GraphPattern::Bgp { patterns } => eval_bgp(patterns, ctx, scope, cap),
        // Declined by `accepts_pattern`: a `Path` node carries a fresh
        // blank node as its join key to a sibling node, which this
        // evaluator does not carry across the join.
        GraphPattern::Path { .. } => Err(EvalError),
        GraphPattern::Join { left, right } => {
            // Neither side may be capped: the row a budget would drop can be
            // the one that joins. Only the join's own output is bounded.
            let l = eval(left, ctx, scope, None)?;
            let r = eval(right, ctx, scope, None)?;
            Ok(cap_rows(join(&l, &r), cap))
        }
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => {
            let l = eval(left, ctx, scope, None)?;
            let r = eval(right, ctx, scope, None)?;
            left_join(l, r, expression.as_ref(), ctx).map(|t| cap_rows(t, cap))
        }
        GraphPattern::Filter { expr, inner } => {
            // The filter drops rows, so the inner pattern gets no budget — but
            // the loop stops as soon as enough have survived.
            let t = eval(inner, ctx, scope, None)?;
            let budget = cap.unwrap_or(usize::MAX);
            let mut rows = Vec::with_capacity(t.rows.len().min(budget));
            for row in t.rows {
                if rows.len() >= budget {
                    break;
                }
                if matches!(
                    eval_expr(expr, &row, &t.vars, ctx).and_then(|v| v.ebv()),
                    Ok(true)
                ) {
                    rows.push(row);
                }
            }
            Ok(Table { vars: t.vars, rows })
        }
        GraphPattern::Union { left, right } => {
            // The union is the left rows followed by the right ones, so a
            // budget splits across the two.
            let l = eval(left, ctx, scope, cap)?;
            let right_cap = cap.map(|n| n.saturating_sub(l.rows.len()));
            let r = eval(right, ctx, scope, right_cap)?;
            Ok(cap_rows(union(l, r), cap))
        }
        GraphPattern::Graph { name, inner } => match name {
            NamedNodePattern::NamedNode(n) => {
                let Some(id) = ctx.idx.dict().lookup(&Term::NamedNode(n.clone())) else {
                    return Ok(Table::empty());
                };
                if !ctx.named_graphs.contains(&id) {
                    return Ok(Table::empty());
                }
                eval(inner, ctx, &Scope::Named(id), cap)
            }
            NamedNodePattern::Variable(v) => eval(inner, ctx, &Scope::Var(v.clone()), cap),
        },
        GraphPattern::Extend {
            inner,
            variable,
            expression,
        } => {
            // BIND is one row in, one row out: the budget passes straight
            // through.
            let t = eval(inner, ctx, scope, cap)?;
            let mut vars = t.vars.clone();
            let existing = t.col(variable);
            if existing.is_none() {
                vars.push(variable.clone());
            }
            let mut rows = Vec::with_capacity(t.rows.len());
            for mut row in t.rows {
                let v = eval_expr(expression, &row, &t.vars, ctx)
                    .ok()
                    .map(|v| ctx.intern_value(&v));
                match existing {
                    Some(c) => row[c] = v,
                    None => row.push(v),
                }
                rows.push(row);
            }
            Ok(Table { vars, rows })
        }
        GraphPattern::Minus { left, right } => {
            let l = eval(left, ctx, scope, None)?;
            let r = eval(right, ctx, scope, None)?;
            Ok(cap_rows(minus(l, &r), cap))
        }
        GraphPattern::Values {
            variables,
            bindings,
        } => {
            let mut rows = Vec::with_capacity(bindings.len());
            for b in bindings {
                let row: IdRow = b
                    .iter()
                    .map(|t| {
                        t.as_ref().map(|g| {
                            let term = match g {
                                GroundTerm::NamedNode(n) => Term::NamedNode(n.clone()),
                                GroundTerm::Literal(l) => Term::Literal(l.clone()),
                                #[allow(unreachable_patterns)]
                                _ => Term::Literal(Literal::new_simple_literal("")),
                            };
                            ctx.intern(&term)
                        })
                    })
                    .collect();
                rows.push(row);
            }
            Ok(cap_rows(
                Table {
                    vars: variables.clone(),
                    rows,
                },
                cap,
            ))
        }
        GraphPattern::OrderBy { inner, expression } => {
            // Ordering has to see every row before it can say which come
            // first, so the inner pattern gets no budget.
            let t = eval(inner, ctx, scope, None)?;
            let mut keyed: Vec<(Vec<Option<Value>>, IdRow)> = Vec::with_capacity(t.rows.len());
            for row in t.rows {
                let keys = expression
                    .iter()
                    .map(|e| {
                        let (e, _) = match e {
                            OrderExpression::Asc(e) => (e, true),
                            OrderExpression::Desc(e) => (e, false),
                        };
                        eval_expr(e, &row, &t.vars, ctx).ok()
                    })
                    .collect();
                keyed.push((keys, row));
            }
            let descs: Vec<bool> = expression
                .iter()
                .map(|e| matches!(e, OrderExpression::Desc(_)))
                .collect();
            keyed.sort_by(|a, b| {
                for (i, desc) in descs.iter().enumerate() {
                    let o = order_cmp(a.0[i].as_ref(), b.0[i].as_ref());
                    let o = if *desc { o.reverse() } else { o };
                    if o != std::cmp::Ordering::Equal {
                        return o;
                    }
                }
                std::cmp::Ordering::Equal
            });
            Ok(cap_rows(
                Table {
                    vars: t.vars,
                    rows: keyed.into_iter().map(|(_, r)| r).collect(),
                },
                cap,
            ))
        }
        GraphPattern::Project { inner, variables } => {
            // Projection drops columns, never rows.
            let t = eval(inner, ctx, scope, cap)?;
            let cols: Vec<Option<usize>> = variables.iter().map(|v| t.col(v)).collect();
            let rows = t
                .rows
                .iter()
                .map(|r| cols.iter().map(|c| c.and_then(|c| r[c])).collect())
                .collect();
            Ok(Table {
                vars: variables.clone(),
                rows,
            })
        }
        GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } => {
            // Deduplication drops rows, so the inner pattern gets no budget;
            // collecting stops once enough distinct rows have been seen.
            let t = eval(inner, ctx, scope, None)?;
            let budget = cap.unwrap_or(usize::MAX);
            let mut seen: HashSet<IdRow> = HashSet::with_capacity(t.rows.len());
            let mut rows: Vec<IdRow> = Vec::new();
            for r in t.rows {
                if rows.len() >= budget {
                    break;
                }
                if seen.insert(r.clone()) {
                    rows.push(r);
                }
            }
            Ok(Table { vars: t.vars, rows })
        }
        GraphPattern::Slice {
            inner,
            start,
            length,
        } => {
            // OFFSET plus LIMIT is the whole budget the inner pattern has to
            // meet. This is what keeps a LIMIT from walking the store.
            let inner_cap = match (*length, cap) {
                (Some(len), Some(c)) => Some(start.saturating_add(len.min(c))),
                (Some(len), None) => Some(start.saturating_add(len)),
                (None, Some(c)) => Some(start.saturating_add(c)),
                (None, None) => None,
            };
            let t = eval(inner, ctx, scope, inner_cap)?;
            let rows = t
                .rows
                .into_iter()
                .skip(*start)
                .take(length.unwrap_or(usize::MAX))
                .collect();
            Ok(Table { vars: t.vars, rows })
        }
        GraphPattern::Group {
            inner,
            variables,
            aggregates,
        } => {
            // Aggregation reads every row before it produces any.
            let t = eval(inner, ctx, scope, None)?;
            group(t, variables, aggregates, ctx).map(|t| cap_rows(t, cap))
        }
        GraphPattern::Lateral { .. } | GraphPattern::Service { .. } => Err(EvalError),
    }
}

/// Truncate a table to the caller's row budget.
///
/// Safe wherever the caller consumes rows in the order they were produced:
/// the kept rows are the same rows, in the same order, that an uncapped
/// evaluation would have put first.
fn cap_rows(mut t: Table, cap: Option<usize>) -> Table {
    if let Some(n) = cap {
        if t.rows.len() > n {
            t.rows.truncate(n);
        }
    }
    t
}

fn project_cols(t: Table, keep: &[usize]) -> Table {
    Table {
        vars: keep.iter().map(|i| t.vars[*i].clone()).collect(),
        rows: t
            .rows
            .into_iter()
            .map(|r| keep.iter().map(|i| r[*i]).collect())
            .collect(),
    }
}

// ─── Basic graph patterns ───────────────────────────────────────────────────

/// A triple pattern with its terms resolved to ids or variables. A constant
/// the dictionary does not hold matches nothing.
enum Slot {
    Const(Id),
    Var(Variable),
    Nothing,
}

fn slot(t: &TermPattern, ctx: &Ctx<'_>) -> Slot {
    match t {
        TermPattern::Variable(v) => Slot::Var(v.clone()),
        TermPattern::NamedNode(n) => ctx
            .idx
            .dict()
            .lookup(&Term::NamedNode(n.clone()))
            .map(Slot::Const)
            .unwrap_or(Slot::Nothing),
        TermPattern::Literal(l) => ctx
            .idx
            .dict()
            .lookup(&Term::Literal(l.clone()))
            .map(Slot::Const)
            .unwrap_or(Slot::Nothing),
        // A blank node in a pattern is a variable that is not projected.
        TermPattern::BlankNode(b) => Slot::Var(Variable::new_unchecked(format!(
            "__og_bnode_{}",
            b.as_str()
        ))),
        #[allow(unreachable_patterns)]
        _ => Slot::Nothing,
    }
}

fn slot_pred(p: &NamedNodePattern, ctx: &Ctx<'_>) -> Slot {
    match p {
        NamedNodePattern::Variable(v) => Slot::Var(v.clone()),
        NamedNodePattern::NamedNode(n) => ctx
            .idx
            .dict()
            .lookup(&Term::NamedNode(n.clone()))
            .map(Slot::Const)
            .unwrap_or(Slot::Nothing),
    }
}

struct Resolved {
    s: Slot,
    p: Slot,
    o: Slot,
}

fn eval_bgp(
    patterns: &[TriplePattern],
    ctx: &mut Ctx<'_>,
    scope: &Scope,
    cap: Option<usize>,
) -> Result<Table, EvalError> {
    let mut resolved: Vec<Resolved> = Vec::with_capacity(patterns.len());
    for t in patterns {
        let r = Resolved {
            s: slot(&t.subject, ctx),
            p: slot_pred(&t.predicate, ctx),
            o: slot(&t.object, ctx),
        };
        if matches!(r.s, Slot::Nothing)
            || matches!(r.p, Slot::Nothing)
            || matches!(r.o, Slot::Nothing)
        {
            return Ok(Table::empty());
        }
        resolved.push(r);
    }
    let mut table = Table::unit();
    if let Scope::Var(v) = scope {
        // The graph variable may already be bound by an outer pattern; if
        // not, it joins the header now.
        if table.col(v).is_none() {
            table.vars.push(v.clone());
            table.rows = vec![vec![None]];
        }
    }
    let mut remaining: Vec<Resolved> = resolved;
    while !remaining.is_empty() {
        // The most selective next pattern: the one with the most bound
        // positions given the header, then the smallest estimated scan.
        let best = pick(&remaining, &table, ctx, scope);
        let r = remaining.swap_remove(best);
        // Only the last pattern may stop early: a row an earlier pattern
        // produced can still be dropped by a later one, so truncating it
        // would lose solutions the query asked for.
        let step = if remaining.is_empty() { cap } else { None };
        table = extend(table, &r, ctx, scope, step)?;
        if table.rows.is_empty() {
            break;
        }
    }
    // Blank-node pattern variables are not in scope.
    let keep: Vec<usize> = table
        .vars
        .iter()
        .enumerate()
        .filter(|(_, v)| !v.as_str().starts_with("__og_bnode_"))
        .map(|(i, _)| i)
        .collect();
    if keep.len() != table.vars.len() {
        table = project_cols(table, &keep);
    }
    Ok(cap_rows(table, cap))
}

fn bound_in(slot: &Slot, table: &Table) -> bool {
    match slot {
        Slot::Const(_) => true,
        Slot::Var(v) => table.col(v).is_some(),
        Slot::Nothing => true,
    }
}

fn pick(remaining: &[Resolved], table: &Table, ctx: &Ctx<'_>, scope: &Scope) -> usize {
    let mut best = 0usize;
    let mut best_key = (0usize, usize::MAX);
    for (i, r) in remaining.iter().enumerate() {
        let bound = [&r.s, &r.p, &r.o]
            .iter()
            .filter(|s| bound_in(s, table))
            .count();
        // Estimate the scan for the first row's bindings (cheap: one binary search).
        let est = estimate(r, table, ctx, scope);
        let key = (bound, est);
        if best_key == (0, usize::MAX)
            || key.0 > best_key.0
            || (key.0 == best_key.0 && key.1 < best_key.1)
        {
            best = i;
            best_key = key;
        }
    }
    best
}

fn estimate(r: &Resolved, table: &Table, ctx: &Ctx<'_>, scope: &Scope) -> usize {
    let row = table.rows.first();
    let val = |s: &Slot| -> Option<Id> {
        match s {
            Slot::Const(id) => Some(*id),
            Slot::Var(v) => table.col(v).and_then(|c| row.and_then(|r| r[c])),
            Slot::Nothing => None,
        }
    };
    let (s, p, o) = (val(&r.s), val(&r.p), val(&r.o));
    let graphs: Vec<Id> = match scope {
        Scope::Default => ctx.default_graphs.clone(),
        Scope::Named(g) => vec![*g],
        Scope::Var(_) => ctx.named_graphs.clone(),
    };
    graphs
        .iter()
        .map(|g| {
            let (perm, prefix) = Columnar::plan(*g, s, p, o);
            ctx.idx.count(perm, &prefix)
        })
        .sum()
}

/// Extend every row of `table` with the matches of one pattern.
fn extend(
    table: Table,
    r: &Resolved,
    ctx: &mut Ctx<'_>,
    scope: &Scope,
    cap: Option<usize>,
) -> Result<Table, EvalError> {
    let mut vars = table.vars.clone();
    // New columns this pattern introduces, in s, p, o order.
    let mut new_cols: Vec<(usize, Variable)> = Vec::new(); // (position 0..3, var)
    for (pos, s) in [&r.s, &r.p, &r.o].iter().enumerate() {
        if let Slot::Var(v) = s {
            if vars.iter().all(|x| x != v) && new_cols.iter().all(|(_, x)| x != v) {
                new_cols.push((pos, v.clone()));
                vars.push(v.clone());
            }
        }
    }
    let graph_col = match scope {
        Scope::Var(v) => table.col(v),
        _ => None,
    };
    let mut rows: Vec<IdRow> = Vec::new();
    // The caller's row budget, if it gave one: the scan stops on reaching it.
    let budget = cap.unwrap_or(usize::MAX);
    for row in &table.rows {
        if rows.len() >= budget {
            break;
        }
        let val = |s: &Slot| -> Option<Id> {
            match s {
                Slot::Const(id) => Some(*id),
                Slot::Var(v) => table.col(v).and_then(|c| row[c]),
                Slot::Nothing => None,
            }
        };
        let (bs, bp, bo) = (val(&r.s), val(&r.p), val(&r.o));
        // Positions that are the same variable twice must agree.
        let same = |a: &Slot, b: &Slot| matches!((a, b), (Slot::Var(x), Slot::Var(y)) if x == y);
        let graphs: Vec<Id> = match scope {
            Scope::Default => ctx.default_graphs.clone(),
            Scope::Named(g) => vec![*g],
            Scope::Var(_) => match graph_col.and_then(|c| row[c]) {
                Some(g) => vec![g],
                None => ctx.named_graphs.clone(),
            },
        };
        // The engine evaluates a multi-graph default (several FROM) as the
        // union *with* duplicates, not the RDF merge; parity means the same.
        let dedup = false;
        let mut seen: HashSet<[Id; 3]> = HashSet::new();
        for g in graphs {
            let (perm, prefix) = Columnar::plan(g, bs, bp, bo);
            for q in ctx.idx.scan(perm, &prefix) {
                let (qs, qp, qo) = (q[1], q[2], q[3]);
                if (same(&r.s, &r.p) && qs != qp)
                    || (same(&r.s, &r.o) && qs != qo)
                    || (same(&r.p, &r.o) && qp != qo)
                {
                    continue;
                }
                if dedup && !seen.insert([qs, qp, qo]) {
                    continue;
                }
                let mut out = row.clone();
                if let Some(c) = graph_col {
                    if out[c].is_none() {
                        out[c] = Some(g);
                    }
                }
                for (pos, _) in &new_cols {
                    out.push(Some(match pos {
                        0 => qs,
                        1 => qp,
                        _ => qo,
                    }));
                }
                rows.push(out);
                if rows.len() >= budget {
                    break;
                }
            }
            if rows.len() >= budget {
                break;
            }
        }
    }
    Ok(Table { vars, rows })
}

// ─── Joins, unions, minus ───────────────────────────────────────────────────

fn shared(l: &Table, r: &Table) -> Vec<(usize, usize)> {
    l.vars
        .iter()
        .enumerate()
        .filter_map(|(i, v)| r.col(v).map(|j| (i, j)))
        .collect()
}

fn merged_vars(l: &Table, r: &Table) -> (Vec<Variable>, Vec<usize>) {
    let mut vars = l.vars.clone();
    let mut extra_r = Vec::new();
    for (j, v) in r.vars.iter().enumerate() {
        if l.col(v).is_none() {
            vars.push(v.clone());
            extra_r.push(j);
        }
    }
    (vars, extra_r)
}

fn compatible(lr: &IdRow, rr: &IdRow, shared: &[(usize, usize)]) -> bool {
    shared.iter().all(|(i, j)| match (lr[*i], rr[*j]) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    })
}

fn merge_rows(lr: &IdRow, rr: &IdRow, shared: &[(usize, usize)], extra_r: &[usize]) -> IdRow {
    let mut out = lr.clone();
    for (i, j) in shared {
        if out[*i].is_none() {
            out[*i] = rr[*j];
        }
    }
    for j in extra_r {
        out.push(rr[*j]);
    }
    out
}

/// Hash join on the shared variables; a shared variable unbound on one side
/// is compatible with anything, so such rows go through a scan.
fn join(l: &Table, r: &Table) -> Table {
    let sh = shared(l, r);
    let (vars, extra_r) = merged_vars(l, r);
    let mut rows = Vec::new();
    if sh.is_empty() {
        for lr in &l.rows {
            for rr in &r.rows {
                rows.push(merge_rows(lr, rr, &sh, &extra_r));
            }
        }
        return Table { vars, rows };
    }
    // Build on the right side, keyed by the shared columns; rows with an
    // unbound shared column go to a side list scanned for every left row.
    let mut buckets: HashMap<Vec<Id>, Vec<usize>> = HashMap::new();
    let mut partial: Vec<usize> = Vec::new();
    for (idx, rr) in r.rows.iter().enumerate() {
        let key: Option<Vec<Id>> = sh.iter().map(|(_, j)| rr[*j]).collect();
        match key {
            Some(k) => buckets.entry(k).or_default().push(idx),
            None => partial.push(idx),
        }
    }
    for lr in &l.rows {
        let key: Option<Vec<Id>> = sh.iter().map(|(i, _)| lr[*i]).collect();
        match key {
            Some(k) => {
                if let Some(list) = buckets.get(&k) {
                    for idx in list {
                        rows.push(merge_rows(lr, &r.rows[*idx], &sh, &extra_r));
                    }
                }
                for idx in &partial {
                    if compatible(lr, &r.rows[*idx], &sh) {
                        rows.push(merge_rows(lr, &r.rows[*idx], &sh, &extra_r));
                    }
                }
            }
            None => {
                for rr in &r.rows {
                    if compatible(lr, rr, &sh) {
                        rows.push(merge_rows(lr, rr, &sh, &extra_r));
                    }
                }
            }
        }
    }
    Table { vars, rows }
}

fn left_join(
    l: Table,
    r: Table,
    expr: Option<&Expression>,
    ctx: &mut Ctx<'_>,
) -> Result<Table, EvalError> {
    let sh = shared(&l, &r);
    let (vars, extra_r) = merged_vars(&l, &r);
    let mut rows = Vec::new();
    let mut buckets: HashMap<Vec<Id>, Vec<usize>> = HashMap::new();
    let mut partial: Vec<usize> = Vec::new();
    for (idx, rr) in r.rows.iter().enumerate() {
        let key: Option<Vec<Id>> = sh.iter().map(|(_, j)| rr[*j]).collect();
        match key {
            Some(k) if !sh.is_empty() => buckets.entry(k).or_default().push(idx),
            _ => partial.push(idx),
        }
    }
    for lr in &l.rows {
        let mut matched = false;
        let key: Option<Vec<Id>> = sh.iter().map(|(i, _)| lr[*i]).collect();
        let candidates: Vec<usize> = match key {
            Some(k) if !sh.is_empty() => {
                let mut c = buckets.get(&k).cloned().unwrap_or_default();
                c.extend(partial.iter().copied());
                c
            }
            _ => (0..r.rows.len()).collect(),
        };
        for idx in candidates {
            let rr = &r.rows[idx];
            if !compatible(lr, rr, &sh) {
                continue;
            }
            let merged = merge_rows(lr, rr, &sh, &extra_r);
            let ok = match expr {
                None => true,
                Some(e) => matches!(
                    eval_expr(e, &merged, &vars, ctx).and_then(|v| v.ebv()),
                    Ok(true)
                ),
            };
            if ok {
                rows.push(merged);
                matched = true;
            }
        }
        if !matched {
            let mut out = lr.clone();
            out.extend(extra_r.iter().map(|_| None));
            rows.push(out);
        }
    }
    Ok(Table { vars, rows })
}

fn union(l: Table, r: Table) -> Table {
    let (vars, extra_r) = merged_vars(&l, &r);
    let mut rows = l.rows;
    let pad = extra_r.len();
    for row in &mut rows {
        row.extend(std::iter::repeat_n(None, pad));
    }
    let r_cols: Vec<usize> = vars
        .iter()
        .map(|v| r.col(v).unwrap_or(usize::MAX))
        .collect();
    for rr in r.rows {
        rows.push(
            r_cols
                .iter()
                .map(|c| if *c == usize::MAX { None } else { rr[*c] })
                .collect(),
        );
    }
    Table { vars, rows }
}

fn minus(l: Table, r: &Table) -> Table {
    let sh = shared(&l, r);
    // MINUS with no shared variable is a no-op: the left side passes through.
    if sh.is_empty() || r.rows.is_empty() {
        return l;
    }
    // A right row can only exclude a left row if the two agree on at least one
    // shared variable bound in both. Indexing the right rows by the value of
    // each shared column lets a left row probe only the rows that could match
    // it, instead of every right row — the difference between O(|L| x |R|) and
    // O(|L| + |R|) on `query/minus`, which the nested loop made 2.8x slower
    // than the engine at ten thousand rows.
    let mut index: Vec<HashMap<Id, Vec<usize>>> = vec![HashMap::new(); sh.len()];
    for (k, (_, j)) in sh.iter().enumerate() {
        for (n, rr) in r.rows.iter().enumerate() {
            if let Some(v) = rr[*j] {
                index[k].entry(v).or_default().push(n);
            }
        }
    }
    // Compatible, and sharing at least one bound variable.
    let excludes = |lr: &IdRow, rr: &IdRow| -> bool {
        let mut any_bound = false;
        for (i, j) in &sh {
            if let (Some(a), Some(b)) = (lr[*i], rr[*j]) {
                if a != b {
                    return false;
                }
                any_bound = true;
            }
        }
        any_bound
    };
    let rows = l
        .rows
        .into_iter()
        .filter(|lr| {
            for (k, (i, _)) in sh.iter().enumerate() {
                let Some(v) = lr[*i] else { continue };
                if let Some(candidates) = index[k].get(&v) {
                    if candidates.iter().any(|n| excludes(lr, &r.rows[*n])) {
                        return false;
                    }
                }
            }
            true
        })
        .collect();
    Table { vars: l.vars, rows }
}

// ─── Grouping and aggregates ────────────────────────────────────────────────

fn group(
    t: Table,
    variables: &[Variable],
    aggregates: &[(Variable, AggregateExpression)],
    ctx: &mut Ctx<'_>,
) -> Result<Table, EvalError> {
    let key_cols: Vec<Option<usize>> = variables.iter().map(|v| t.col(v)).collect();
    let mut order: Vec<IdRow> = Vec::new();
    let mut groups: HashMap<IdRow, Vec<usize>> = HashMap::new();
    for (i, row) in t.rows.iter().enumerate() {
        let key: IdRow = key_cols.iter().map(|c| c.and_then(|c| row[c])).collect();
        groups
            .entry(key.clone())
            .or_insert_with(|| {
                order.push(key);
                Vec::new()
            })
            .push(i);
    }
    // No group keys and no rows: one group over nothing (COUNT = 0).
    if variables.is_empty() && order.is_empty() {
        order.push(Vec::new());
        groups.insert(Vec::new(), Vec::new());
    }
    let mut vars = variables.to_vec();
    vars.extend(aggregates.iter().map(|(v, _)| v.clone()));
    let mut rows = Vec::with_capacity(order.len());
    for key in order {
        let members = &groups[&key];
        let mut out = key.clone();
        for (_, agg) in aggregates {
            let v = aggregate(agg, members, &t, ctx)?;
            out.push(v.map(|v| ctx.intern_value(&v)));
        }
        rows.push(out);
    }
    Ok(Table { vars, rows })
}

fn aggregate(
    agg: &AggregateExpression,
    members: &[usize],
    t: &Table,
    ctx: &mut Ctx<'_>,
) -> Result<Option<Value>, EvalError> {
    match agg {
        AggregateExpression::CountSolutions { distinct } => {
            let n = if *distinct {
                members
                    .iter()
                    .map(|i| &t.rows[*i])
                    .collect::<HashSet<_>>()
                    .len()
            } else {
                members.len()
            };
            Ok(Some(Value::Num(Numeric::Int((n as i64).into()))))
        }
        AggregateExpression::FunctionCall {
            name,
            expr,
            distinct,
        } => {
            // The values of the expression over the group. An error in any
            // member makes the whole aggregate unbound — the engine's rule for
            // every aggregate, not only SUM and AVG.
            let mut values: Vec<Value> = Vec::new();
            let mut errored = false;
            let mut seen: HashSet<Term> = HashSet::new();
            for i in members {
                match eval_expr(expr, &t.rows[*i], &t.vars, ctx) {
                    Ok(v) => {
                        if *distinct && !seen.insert(v.to_term()) {
                            continue;
                        }
                        values.push(v);
                    }
                    Err(_) => errored = true,
                }
            }
            match name {
                AggregateFunction::Count => {
                    if errored {
                        return Ok(None);
                    }
                    Ok(Some(Value::Num(Numeric::Int((values.len() as i64).into()))))
                }
                AggregateFunction::Sum => {
                    if errored {
                        return Ok(None);
                    }
                    let mut acc = Numeric::Int(0.into());
                    for v in &values {
                        let Value::Num(n) = v else { return Ok(None) };
                        acc = match acc.add(n) {
                            Ok(a) => a,
                            Err(_) => return Ok(None),
                        };
                    }
                    Ok(Some(Value::Num(acc)))
                }
                AggregateFunction::Avg => {
                    if errored {
                        return Ok(None);
                    }
                    if values.is_empty() {
                        return Ok(Some(Value::Num(Numeric::Int(0.into()))));
                    }
                    let mut acc = Numeric::Int(0.into());
                    for v in &values {
                        let Value::Num(n) = v else { return Ok(None) };
                        acc = match acc.add(n) {
                            Ok(a) => a,
                            Err(_) => return Ok(None),
                        };
                    }
                    let count = Numeric::Int((values.len() as i64).into());
                    Ok(acc.div(&count).ok().map(Value::Num))
                }
                AggregateFunction::Min | AggregateFunction::Max => {
                    if errored {
                        return Ok(None);
                    }
                    let want_min = matches!(name, AggregateFunction::Min);
                    let mut best: Option<Value> = None;
                    for v in values {
                        best = Some(match best {
                            None => v,
                            Some(b) => {
                                let o = order_cmp(Some(&v), Some(&b));
                                let take = if want_min {
                                    o == std::cmp::Ordering::Less
                                } else {
                                    o == std::cmp::Ordering::Greater
                                };
                                if take {
                                    v
                                } else {
                                    b
                                }
                            }
                        });
                    }
                    Ok(best)
                }
                AggregateFunction::GroupConcat { separator } => {
                    // Every member must be a string: the engine fails the
                    // aggregate rather than stringifying an IRI or a number.
                    if errored
                        || values
                            .iter()
                            .any(|v| !matches!(v, Value::Str(_) | Value::Lang(_, _)))
                    {
                        return Ok(None);
                    }
                    let sep = separator.as_deref().unwrap_or(" ");
                    let s = values.iter().map(|v| v.str()).collect::<Vec<_>>().join(sep);
                    // The language tag survives only if every value shares it.
                    let lang = values.iter().map(|v| match v {
                        Value::Lang(_, l) => Some(l.clone()),
                        _ => None,
                    });
                    let mut common: Option<Option<String>> = None;
                    for l in lang {
                        common = match common {
                            None => Some(l),
                            Some(c) if c == l => Some(c),
                            _ => Some(None),
                        };
                    }
                    Ok(Some(match common.flatten() {
                        Some(l) => Value::Lang(s, l),
                        None => Value::Str(s),
                    }))
                }
                AggregateFunction::Sample => Ok(values.into_iter().next()),
                AggregateFunction::Custom(_) => Err(EvalError),
            }
        }
    }
}

// ─── Expressions ────────────────────────────────────────────────────────────

fn eval_expr(
    e: &Expression,
    row: &IdRow,
    vars: &[Variable],
    ctx: &mut Ctx<'_>,
) -> Result<Value, EvalError> {
    match e {
        Expression::NamedNode(n) => Ok(Value::Iri(n.clone())),
        Expression::Literal(l) => {
            Value::from_term(&Term::Literal(l.clone())).map_err(|_| EvalError)
        }
        Expression::Variable(v) => {
            let c = vars.iter().position(|x| x == v).ok_or(EvalError)?;
            let id = row[c].ok_or(EvalError)?;
            ctx.value(id)
        }
        Expression::Or(a, b) => {
            let l = eval_expr(a, row, vars, ctx).and_then(|v| v.ebv());
            let r = eval_expr(b, row, vars, ctx).and_then(|v| v.ebv());
            match (l, r) {
                (Ok(true), _) | (_, Ok(true)) => Ok(Value::Bool(true)),
                (Ok(false), Ok(false)) => Ok(Value::Bool(false)),
                _ => Err(EvalError),
            }
        }
        Expression::And(a, b) => {
            let l = eval_expr(a, row, vars, ctx).and_then(|v| v.ebv());
            let r = eval_expr(b, row, vars, ctx).and_then(|v| v.ebv());
            match (l, r) {
                (Ok(false), _) | (_, Ok(false)) => Ok(Value::Bool(false)),
                (Ok(true), Ok(true)) => Ok(Value::Bool(true)),
                _ => Err(EvalError),
            }
        }
        Expression::Equal(a, b) => {
            let l = eval_expr(a, row, vars, ctx)?;
            let r = eval_expr(b, row, vars, ctx)?;
            l.equals(&r).map(Value::Bool)
        }
        Expression::SameTerm(a, b) => {
            let l = eval_expr(a, row, vars, ctx)?;
            let r = eval_expr(b, row, vars, ctx)?;
            Ok(Value::Bool(l.same_term(&r)))
        }
        Expression::Greater(a, b) => cmp(a, b, row, vars, ctx, |o| o.is_gt()),
        Expression::GreaterOrEqual(a, b) => cmp(a, b, row, vars, ctx, |o| o.is_ge()),
        Expression::Less(a, b) => cmp(a, b, row, vars, ctx, |o| o.is_lt()),
        Expression::LessOrEqual(a, b) => cmp(a, b, row, vars, ctx, |o| o.is_le()),
        Expression::In(a, list) => {
            let l = eval_expr(a, row, vars, ctx)?;
            let mut error = false;
            for x in list {
                match eval_expr(x, row, vars, ctx).and_then(|v| l.equals(&v)) {
                    Ok(true) => return Ok(Value::Bool(true)),
                    Ok(false) => {}
                    Err(_) => error = true,
                }
            }
            if error {
                Err(EvalError)
            } else {
                Ok(Value::Bool(false))
            }
        }
        Expression::Add(a, b) => arith(a, b, row, vars, ctx, |x, y| x.add(y)),
        Expression::Subtract(a, b) => arith(a, b, row, vars, ctx, |x, y| x.sub(y)),
        Expression::Multiply(a, b) => arith(a, b, row, vars, ctx, |x, y| x.mul(y)),
        Expression::Divide(a, b) => arith(a, b, row, vars, ctx, |x, y| x.div(y)),
        Expression::UnaryPlus(a) => match eval_expr(a, row, vars, ctx)? {
            Value::Num(n) => Ok(Value::Num(n)),
            _ => Err(EvalError),
        },
        Expression::UnaryMinus(a) => match eval_expr(a, row, vars, ctx)? {
            Value::Num(n) => n.neg().map(Value::Num),
            _ => Err(EvalError),
        },
        Expression::Not(a) => eval_expr(a, row, vars, ctx)
            .and_then(|v| v.ebv())
            .map(|b| Value::Bool(!b)),
        Expression::Exists(_) => Err(EvalError),
        Expression::Bound(v) => {
            let c = vars.iter().position(|x| x == v);
            Ok(Value::Bool(c.and_then(|c| row[c]).is_some()))
        }
        Expression::If(c, a, b) => {
            if eval_expr(c, row, vars, ctx).and_then(|v| v.ebv())? {
                eval_expr(a, row, vars, ctx)
            } else {
                eval_expr(b, row, vars, ctx)
            }
        }
        Expression::Coalesce(list) => {
            for x in list {
                if let Ok(v) = eval_expr(x, row, vars, ctx) {
                    return Ok(v);
                }
            }
            Err(EvalError)
        }
        Expression::FunctionCall(f, args) => {
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(eval_expr(a, row, vars, ctx)?);
            }
            function(f, &vals)
        }
    }
}

fn cmp(
    a: &Expression,
    b: &Expression,
    row: &IdRow,
    vars: &[Variable],
    ctx: &mut Ctx<'_>,
    test: impl Fn(std::cmp::Ordering) -> bool,
) -> Result<Value, EvalError> {
    let l = eval_expr(a, row, vars, ctx)?;
    let r = eval_expr(b, row, vars, ctx)?;
    l.compare(&r).map(|o| Value::Bool(test(o)))
}

fn arith(
    a: &Expression,
    b: &Expression,
    row: &IdRow,
    vars: &[Variable],
    ctx: &mut Ctx<'_>,
    op: impl Fn(&Numeric, &Numeric) -> Result<Numeric, EvalError>,
) -> Result<Value, EvalError> {
    let l = eval_expr(a, row, vars, ctx)?;
    let r = eval_expr(b, row, vars, ctx)?;
    match (l, r) {
        (Value::Num(x), Value::Num(y)) => op(&x, &y).map(Value::Num),
        _ => Err(EvalError),
    }
}

/// A string argument: simple, `xsd:string` or language-tagged.
fn string_arg(v: &Value) -> Result<(&str, Option<&str>), EvalError> {
    match v {
        Value::Str(s) => Ok((s, None)),
        Value::Lang(s, l) => Ok((s, Some(l))),
        _ => Err(EvalError),
    }
}

/// Two string arguments must be compatible (SPARQL 17.4.3.1).
fn compatible_strings<'v>(
    a: &'v Value,
    b: &'v Value,
) -> Result<(&'v str, &'v str, Option<&'v str>), EvalError> {
    let (sa, la) = string_arg(a)?;
    let (sb, lb) = string_arg(b)?;
    match (la, lb) {
        (_, None) => Ok((sa, sb, la)),
        (Some(x), Some(y)) if x.eq_ignore_ascii_case(y) => Ok((sa, sb, la)),
        _ => Err(EvalError),
    }
}

fn with_lang(s: String, lang: Option<&str>) -> Value {
    match lang {
        Some(l) => Value::Lang(s, l.to_string()),
        None => Value::Str(s),
    }
}

fn function(f: &Function, args: &[Value]) -> Result<Value, EvalError> {
    let a = |i: usize| args.get(i).ok_or(EvalError);
    match f {
        // STR() of a blank node is a type error, not its label.
        Function::Str => match a(0)? {
            Value::Blank(_) => Err(EvalError),
            v => Ok(Value::Str(v.str())),
        },
        Function::Lang => match a(0)? {
            Value::Lang(_, l) => Ok(Value::Str(l.clone())),
            v if v.is_literal() => Ok(Value::Str(String::new())),
            _ => Err(EvalError),
        },
        Function::LangMatches => {
            let (tag, _) = string_arg(a(0)?)?;
            let (range, _) = string_arg(a(1)?)?;
            Ok(Value::Bool(lang_matches(tag, range)))
        }
        Function::Datatype => a(0)?.datatype().map(Value::Iri).ok_or(EvalError),
        Function::Iri => match a(0)? {
            Value::Iri(n) => Ok(Value::Iri(n.clone())),
            Value::Str(s) => NamedNode::new(s).map(Value::Iri).map_err(|_| EvalError),
            _ => Err(EvalError),
        },
        Function::Abs => num(a(0)?)?.abs().map(Value::Num),
        Function::Ceil => num(a(0)?)?.ceil().map(Value::Num),
        Function::Floor => num(a(0)?)?.floor().map(Value::Num),
        Function::Round => num(a(0)?)?.round().map(Value::Num),
        Function::Concat => {
            let mut out = String::new();
            let mut lang: Option<Option<&str>> = None;
            for v in args {
                let (s, l) = string_arg(v)?;
                out.push_str(s);
                lang = Some(match lang {
                    None => l,
                    Some(prev) if prev == l => prev,
                    _ => None,
                });
            }
            Ok(with_lang(out, lang.flatten()))
        }
        Function::SubStr => {
            let (s, lang) = string_arg(a(0)?)?;
            let start = int_arg(a(1)?)?;
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let (from, to) = match args.get(2) {
                Some(l) => {
                    let l = int_arg(l)?;
                    (start, start + l)
                }
                None => (start, len + 1),
            };
            // SPARQL substr: 1-based, positions outside the string are clipped.
            let from = from.max(1);
            let to = to.min(len + 1);
            let out: String = if to > from {
                chars[(from - 1) as usize..(to - 1) as usize]
                    .iter()
                    .collect()
            } else {
                String::new()
            };
            Ok(with_lang(out, lang))
        }
        Function::StrLen => {
            let (s, _) = string_arg(a(0)?)?;
            Ok(Value::Num(Numeric::Int((s.chars().count() as i64).into())))
        }
        Function::UCase => {
            let (s, lang) = string_arg(a(0)?)?;
            Ok(with_lang(s.to_uppercase(), lang))
        }
        Function::LCase => {
            let (s, lang) = string_arg(a(0)?)?;
            Ok(with_lang(s.to_lowercase(), lang))
        }
        Function::EncodeForUri => {
            let (s, _) = string_arg(a(0)?)?;
            let mut out = String::new();
            for b in s.bytes() {
                if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                    out.push(b as char);
                } else {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
            Ok(Value::Str(out))
        }
        Function::Contains => {
            let (x, y, _) = compatible_strings(a(0)?, a(1)?)?;
            Ok(Value::Bool(x.contains(y)))
        }
        Function::StrStarts => {
            let (x, y, _) = compatible_strings(a(0)?, a(1)?)?;
            Ok(Value::Bool(x.starts_with(y)))
        }
        Function::StrEnds => {
            let (x, y, _) = compatible_strings(a(0)?, a(1)?)?;
            Ok(Value::Bool(x.ends_with(y)))
        }
        Function::StrBefore => {
            let (x, y, lang) = compatible_strings(a(0)?, a(1)?)?;
            Ok(match x.find(y) {
                Some(i) => with_lang(x[..i].to_string(), lang),
                None => Value::Str(String::new()),
            })
        }
        Function::StrAfter => {
            let (x, y, lang) = compatible_strings(a(0)?, a(1)?)?;
            Ok(match x.find(y) {
                Some(i) => with_lang(x[i + y.len()..].to_string(), lang),
                None => Value::Str(String::new()),
            })
        }
        Function::Year => date_part(a(0)?, |d| d.year(), |d| d.year()),
        Function::Month => date_part(a(0)?, |d| d.month() as i64, |d| d.month() as i64),
        Function::Day => date_part(a(0)?, |d| d.day() as i64, |d| d.day() as i64),
        Function::Hours => match a(0)? {
            Value::DateTime(d) => Ok(Value::Num(Numeric::Int((d.hour() as i64).into()))),
            Value::Time(t) => Ok(Value::Num(Numeric::Int((t.hour() as i64).into()))),
            _ => Err(EvalError),
        },
        Function::Minutes => match a(0)? {
            Value::DateTime(d) => Ok(Value::Num(Numeric::Int((d.minute() as i64).into()))),
            Value::Time(t) => Ok(Value::Num(Numeric::Int((t.minute() as i64).into()))),
            _ => Err(EvalError),
        },
        Function::Seconds => match a(0)? {
            Value::DateTime(d) => Ok(Value::Num(Numeric::Dec(d.second()))),
            Value::Time(t) => Ok(Value::Num(Numeric::Dec(t.second()))),
            _ => Err(EvalError),
        },
        Function::Tz => match a(0)? {
            Value::DateTime(d) => Ok(Value::Str(
                d.timezone_offset()
                    .map(|o| o.to_string())
                    .unwrap_or_default(),
            )),
            _ => Err(EvalError),
        },
        Function::StrLang => {
            let (s, _) = string_arg(a(0)?)?;
            let (l, _) = string_arg(a(1)?)?;
            if l.is_empty() {
                return Err(EvalError);
            }
            Ok(Value::Lang(s.to_string(), l.to_ascii_lowercase()))
        }
        Function::StrDt => {
            let (s, _) = string_arg(a(0)?)?;
            match a(1)? {
                Value::Iri(dt) => {
                    Value::from_term(&Term::Literal(Literal::new_typed_literal(s, dt.clone())))
                        .map_err(|_| EvalError)
                }
                _ => Err(EvalError),
            }
        }
        Function::IsIri => Ok(Value::Bool(matches!(a(0)?, Value::Iri(_)))),
        Function::IsBlank => Ok(Value::Bool(matches!(a(0)?, Value::Blank(_)))),
        Function::IsLiteral => Ok(Value::Bool(a(0)?.is_literal())),
        Function::IsNumeric => Ok(Value::Bool(matches!(a(0)?, Value::Num(_)))),
        _ => Err(EvalError),
    }
}

fn num(v: &Value) -> Result<&Numeric, EvalError> {
    match v {
        Value::Num(n) => Ok(n),
        _ => Err(EvalError),
    }
}

fn int_arg(v: &Value) -> Result<i64, EvalError> {
    match v {
        Value::Num(Numeric::Int(i)) => i.to_string().parse::<i64>().map_err(|_| EvalError),
        Value::Num(n) => match n.round() {
            Ok(Numeric::Dec(d)) => d
                .to_string()
                .parse::<f64>()
                .map(|f| f as i64)
                .map_err(|_| EvalError),
            Ok(Numeric::Dbl(d)) => Ok(f64::from(d) as i64),
            Ok(Numeric::Flt(f)) => Ok(f32::from(f) as i64),
            Ok(Numeric::Int(i)) => i.to_string().parse::<i64>().map_err(|_| EvalError),
            Err(e) => Err(e),
        },
        _ => Err(EvalError),
    }
}

fn date_part(
    v: &Value,
    dt: impl Fn(&oxsdatatypes::DateTime) -> i64,
    d: impl Fn(&oxsdatatypes::Date) -> i64,
) -> Result<Value, EvalError> {
    match v {
        Value::DateTime(x) => Ok(Value::Num(Numeric::Int(dt(x).into()))),
        Value::Date(x) => Ok(Value::Num(Numeric::Int(d(x).into()))),
        _ => Err(EvalError),
    }
}

/// RFC 4647 basic filtering, as `langMatches`.
fn lang_matches(tag: &str, range: &str) -> bool {
    if range == "*" {
        return !tag.is_empty();
    }
    let tag = tag.to_ascii_lowercase();
    let range = range.to_ascii_lowercase();
    tag == range || (tag.starts_with(&range) && tag[range.len()..].starts_with('-'))
}

#[allow(dead_code)]
fn _xsd_string_is_used() -> oxrdf::NamedNodeRef<'static> {
    xsd::STRING
}
