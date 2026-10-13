//! Schema requirements (ShEx 2.1 §5.6–§5.7): imports are resolved and merged,
//! every reference resolves, no label refers to itself without passing
//! through a shape's triple constraints, and no cycle of references passes a
//! negation (`NOT`, or a triple constraint whose predicate is `EXTRA`).
//!
//! The result, [`ResolvedSchema`], also records the strongly connected
//! components of the label dependency graph: the validator evaluates one
//! component's recursion as a greatest fixpoint and treats references into
//! other components (always lower strata) as already decided.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use super::ast::*;

/// Where `IMPORT`ed schemas come from. The server's resolver reads them from
/// the store and nowhere else (never the network).
pub trait ImportResolver {
    /// The schema at `iri`.
    fn resolve(&self, iri: &str) -> Result<Schema, String>;
}

/// A resolver for schemas with no imports: any `IMPORT` is an error.
pub struct NoImports;

impl ImportResolver for NoImports {
    fn resolve(&self, iri: &str) -> Result<Schema, String> {
        Err(format!("IMPORT <{iri}>: imports are not available here"))
    }
}

/// A schema with its imports merged and its requirements checked.
#[derive(Debug, Clone)]
pub struct ResolvedSchema {
    pub shapes: HashMap<Label, ShapeExpr>,
    /// Declaration order: the main schema's labels, then each import's.
    pub order: Vec<Label>,
    /// Labelled triple expressions (targets of `&label` inclusions).
    pub triple_exprs: HashMap<Label, TripleExpr>,
    pub start: Option<ShapeExpr>,
    pub start_acts: Vec<SemAct>,
    /// The strongly connected component of each shape label.
    pub component: HashMap<Label, usize>,
    pub prefixes: Vec<(String, String)>,
    pub base: Option<String>,
}

impl ResolvedSchema {
    pub fn shape(&self, l: &Label) -> Option<&ShapeExpr> {
        self.shapes.get(l)
    }
}

/// Merge `main`'s imports (transitively, each IRI once; `self_iri` names
/// `main` itself so a circular import of it is recognised) and check the
/// schema requirements.
pub fn resolve(
    main: Schema,
    self_iri: Option<&str>,
    resolver: &dyn ImportResolver,
) -> Result<ResolvedSchema, String> {
    let mut rs = ResolvedSchema {
        shapes: HashMap::new(),
        order: Vec::new(),
        triple_exprs: HashMap::new(),
        start: main.start.clone(),
        start_acts: main.start_acts.clone(),
        component: HashMap::new(),
        prefixes: main.prefixes.clone(),
        base: main.base.clone(),
    };
    let mut seen: HashSet<String> = HashSet::new();
    if let Some(s) = self_iri {
        seen.insert(s.to_string());
    }
    let mut queue: VecDeque<String> = main.imports.iter().cloned().collect();
    add_schema(&mut rs, main, true)?;
    while let Some(iri) = queue.pop_front() {
        if !seen.insert(iri.clone()) {
            continue;
        }
        let imported = resolver
            .resolve(&iri)
            .map_err(|e| format!("IMPORT <{iri}>: {e}"))?;
        if !imported.start_acts.is_empty() {
            return Err(format!(
                "IMPORT <{iri}>: an imported schema may not have start actions"
            ));
        }
        queue.extend(imported.imports.iter().cloned());
        add_schema(&mut rs, imported, false).map_err(|e| format!("IMPORT <{iri}>: {e}"))?;
    }
    check_references(&rs)?;
    check_ref_closures(&rs)?;
    check_triple_expr_closures(&rs)?;
    rs.component = stratify(&rs)?;
    Ok(rs)
}

fn add_schema(rs: &mut ResolvedSchema, s: Schema, _main: bool) -> Result<(), String> {
    for d in s.shapes {
        if rs.shapes.contains_key(&d.label) || rs.triple_exprs.contains_key(&d.label) {
            return Err(format!("shape label {} is defined twice", d.label));
        }
        let mut tes = Vec::new();
        collect_labelled_tes(&d.expr, &mut tes);
        for (l, te) in tes {
            if rs.shapes.contains_key(&l) || rs.triple_exprs.contains_key(&l) || l == d.label {
                return Err(format!("label {l} is defined twice"));
            }
            rs.triple_exprs.insert(l, te);
        }
        rs.order.push(d.label.clone());
        rs.shapes.insert(d.label, d.expr);
    }
    if let Some(st) = &s.start {
        let mut tes = Vec::new();
        collect_labelled_tes(st, &mut tes);
        for (l, te) in tes {
            if rs.shapes.contains_key(&l) || rs.triple_exprs.contains_key(&l) {
                return Err(format!("label {l} is defined twice"));
            }
            rs.triple_exprs.insert(l, te);
        }
    }
    Ok(())
}

fn collect_labelled_tes(se: &ShapeExpr, out: &mut Vec<(Label, TripleExpr)>) {
    match se {
        ShapeExpr::Or(v) | ShapeExpr::And(v) => v.iter().for_each(|e| collect_labelled_tes(e, out)),
        ShapeExpr::Not(e) => collect_labelled_tes(e, out),
        ShapeExpr::Shape(s) => {
            if let Some(te) = &s.expression {
                collect_te(te, out);
            }
        }
        _ => {}
    }
}

fn collect_te(te: &TripleExpr, out: &mut Vec<(Label, TripleExpr)>) {
    if let Some(id) = te.id() {
        out.push((id.clone(), te.clone()));
    }
    match te {
        TripleExpr::EachOf(g) | TripleExpr::OneOf(g) => {
            g.expressions.iter().for_each(|e| collect_te(e, out))
        }
        TripleExpr::TripleConstraint(tc) => {
            if let Some(v) = &tc.value_expr {
                collect_labelled_tes(v, out);
            }
        }
        TripleExpr::Ref(_) => {}
    }
}

// ── references ──────────────────────────────────────────────────────────

fn for_each_se_ref<'a>(
    se: &'a ShapeExpr,
    f: &mut dyn FnMut(&'a Label),
    g: &mut dyn FnMut(&'a Label),
) {
    match se {
        ShapeExpr::Or(v) | ShapeExpr::And(v) => v.iter().for_each(|e| for_each_se_ref(e, f, g)),
        ShapeExpr::Not(e) => for_each_se_ref(e, f, g),
        ShapeExpr::Ref(l) => f(l),
        ShapeExpr::Shape(s) => {
            if let Some(te) = &s.expression {
                for_each_te_ref(te, f, g);
            }
        }
        ShapeExpr::NodeConstraint(_) | ShapeExpr::External => {}
    }
}

fn for_each_te_ref<'a>(
    te: &'a TripleExpr,
    f: &mut dyn FnMut(&'a Label),
    g: &mut dyn FnMut(&'a Label),
) {
    match te {
        TripleExpr::EachOf(gr) | TripleExpr::OneOf(gr) => {
            gr.expressions.iter().for_each(|e| for_each_te_ref(e, f, g))
        }
        TripleExpr::TripleConstraint(tc) => {
            if let Some(v) = &tc.value_expr {
                for_each_se_ref(v, f, g);
            }
        }
        TripleExpr::Ref(l) => g(l),
    }
}

fn check_references(rs: &ResolvedSchema) -> Result<(), String> {
    let mut shape_refs: Vec<&Label> = Vec::new();
    let mut te_refs: Vec<&Label> = Vec::new();
    let exprs = rs
        .order
        .iter()
        .map(|l| &rs.shapes[l])
        .chain(rs.start.as_ref());
    for se in exprs {
        for_each_se_ref(se, &mut |l| shape_refs.push(l), &mut |l| te_refs.push(l));
    }
    for l in shape_refs {
        if !rs.shapes.contains_key(l) {
            return Err(if rs.triple_exprs.contains_key(l) {
                format!("{l} names a triple expression, not a shape expression")
            } else {
                format!("reference to undefined shape {l}")
            });
        }
    }
    for l in te_refs {
        if !rs.triple_exprs.contains_key(l) {
            return Err(if rs.shapes.contains_key(l) {
                format!("&{l} includes a shape expression, not a triple expression")
            } else {
                format!("inclusion of undefined triple expression {l}")
            });
        }
    }
    Ok(())
}

/// Refs at shape-expression level, not passing into a shape's triple
/// expression: the `shapeExprRef` closure (§5.7.2).
fn direct_refs<'a>(se: &'a ShapeExpr, out: &mut Vec<&'a Label>) {
    match se {
        ShapeExpr::Or(v) | ShapeExpr::And(v) => v.iter().for_each(|e| direct_refs(e, out)),
        ShapeExpr::Not(e) => direct_refs(e, out),
        ShapeExpr::Ref(l) => out.push(l),
        _ => {}
    }
}

fn check_ref_closures(rs: &ResolvedSchema) -> Result<(), String> {
    for start in &rs.order {
        let mut stack = vec![start];
        let mut seen = HashSet::new();
        while let Some(l) = stack.pop() {
            let mut refs = Vec::new();
            direct_refs(&rs.shapes[l], &mut refs);
            for r in refs {
                if r == start {
                    return Err(format!(
                        "{start} refers to itself without passing through a shape"
                    ));
                }
                if seen.insert(r) {
                    stack.push(r);
                }
            }
        }
    }
    Ok(())
}

fn te_inclusions<'a>(te: &'a TripleExpr, out: &mut Vec<&'a Label>) {
    match te {
        TripleExpr::EachOf(g) | TripleExpr::OneOf(g) => {
            g.expressions.iter().for_each(|e| te_inclusions(e, out))
        }
        TripleExpr::Ref(l) => out.push(l),
        TripleExpr::TripleConstraint(_) => {}
    }
}

fn check_triple_expr_closures(rs: &ResolvedSchema) -> Result<(), String> {
    for start in rs.triple_exprs.keys() {
        let mut stack = vec![start];
        let mut seen = HashSet::new();
        while let Some(l) = stack.pop() {
            let mut refs = Vec::new();
            te_inclusions(&rs.triple_exprs[l], &mut refs);
            for r in refs {
                if r == start {
                    return Err(format!("triple expression {start} includes itself"));
                }
                if seen.insert(r) {
                    stack.push(r);
                }
            }
        }
    }
    Ok(())
}

// ── stratification ──────────────────────────────────────────────────────

/// Edges `label → (referenced label, negated)` of the dependency graph.
fn dependency_edges(rs: &ResolvedSchema, l: &Label) -> Vec<(Label, bool)> {
    let mut out = Vec::new();
    se_edges(rs, &rs.shapes[l], false, &mut out, &mut HashSet::new());
    out
}

fn se_edges(
    rs: &ResolvedSchema,
    se: &ShapeExpr,
    neg: bool,
    out: &mut Vec<(Label, bool)>,
    including: &mut HashSet<Label>,
) {
    match se {
        ShapeExpr::Or(v) | ShapeExpr::And(v) => {
            v.iter().for_each(|e| se_edges(rs, e, neg, out, including))
        }
        ShapeExpr::Not(e) => se_edges(rs, e, !neg, out, including),
        ShapeExpr::Ref(l) => out.push((l.clone(), neg)),
        ShapeExpr::Shape(s) => {
            if let Some(te) = &s.expression {
                te_edges(rs, te, neg, &s.extra, out, including);
            }
        }
        ShapeExpr::NodeConstraint(_) | ShapeExpr::External => {}
    }
}

fn te_edges(
    rs: &ResolvedSchema,
    te: &TripleExpr,
    neg: bool,
    extra: &[String],
    out: &mut Vec<(Label, bool)>,
    including: &mut HashSet<Label>,
) {
    match te {
        TripleExpr::EachOf(g) | TripleExpr::OneOf(g) => g
            .expressions
            .iter()
            .for_each(|e| te_edges(rs, e, neg, extra, out, including)),
        TripleExpr::TripleConstraint(tc) => {
            if let Some(v) = &tc.value_expr {
                let mut inner = Vec::new();
                se_edges(rs, v, neg, &mut inner, including);
                let extra_neg = extra.contains(&tc.predicate);
                out.extend(inner.into_iter().map(|(l, n)| (l, n || extra_neg)));
            }
        }
        TripleExpr::Ref(l) => {
            if including.insert(l.clone()) {
                if let Some(t) = rs.triple_exprs.get(l) {
                    te_edges(rs, t, neg, extra, out, including);
                }
                including.remove(l);
            }
        }
    }
}

/// Tarjan's SCCs over the label graph; fails when an edge inside a
/// component is negated.
fn stratify(rs: &ResolvedSchema) -> Result<HashMap<Label, usize>, String> {
    let labels: Vec<&Label> = rs.order.iter().collect();
    let index_of: HashMap<&Label, usize> =
        labels.iter().enumerate().map(|(i, l)| (*l, i)).collect();
    let edges: Vec<Vec<(usize, bool)>> = labels
        .iter()
        .map(|l| {
            dependency_edges(rs, l)
                .into_iter()
                .filter_map(|(t, n)| index_of.get(&t).map(|&i| (i, n)))
                .collect()
        })
        .collect();

    struct T<'e> {
        edges: &'e [Vec<(usize, bool)>],
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        comp: Vec<usize>,
        ncomp: usize,
    }
    fn strong(t: &mut T<'_>, v: usize) {
        t.index[v] = Some(t.next);
        t.low[v] = t.next;
        t.next += 1;
        t.stack.push(v);
        t.on_stack[v] = true;
        let edges = t.edges;
        for &(w, _) in &edges[v] {
            match t.index[w] {
                None => {
                    strong(t, w);
                    t.low[v] = t.low[v].min(t.low[w]);
                }
                Some(iw) if t.on_stack[w] => t.low[v] = t.low[v].min(iw),
                _ => {}
            }
        }
        if Some(t.low[v]) == t.index[v] {
            loop {
                let w = t.stack.pop().unwrap();
                t.on_stack[w] = false;
                t.comp[w] = t.ncomp;
                if w == v {
                    break;
                }
            }
            t.ncomp += 1;
        }
    }
    let n = labels.len();
    let mut t = T {
        edges: &edges,
        index: vec![None; n],
        low: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        next: 0,
        comp: vec![0; n],
        ncomp: 0,
    };
    for v in 0..n {
        if t.index[v].is_none() {
            strong(&mut t, v);
        }
    }
    for (v, es) in edges.iter().enumerate() {
        for &(w, negated) in es {
            if negated && t.comp[v] == t.comp[w] {
                let cycle: BTreeSet<String> = (0..n)
                    .filter(|&i| t.comp[i] == t.comp[v])
                    .map(|i| labels[i].to_string())
                    .collect();
                return Err(format!(
                    "the negation requirement is violated: a cycle through {} passes a NOT or an EXTRA predicate",
                    cycle.into_iter().collect::<Vec<_>>().join(", ")
                ));
            }
        }
    }
    Ok(labels
        .iter()
        .enumerate()
        .map(|(i, l)| ((*l).clone(), t.comp[i]))
        .collect())
}
