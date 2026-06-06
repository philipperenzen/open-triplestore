//! Subject-partitioned **parallel query execution** — "divide a query over a
//! dataset split".
//!
//! A single SPARQL query in Oxigraph runs on one thread, so a large scan /
//! aggregation uses a single core regardless of how many are available. This
//! module adds *data-parallel* execution at the OpenGraph layer (no engine fork):
//! the dataset is split into `N` shards by a **stable hash of each triple's
//! subject**, and a query is evaluated on every shard concurrently (Rayon), with
//! the partial results merged.
//!
//! ## Why partition by subject?
//!
//! Hashing on the subject co-locates every triple that shares a subject in the
//! same shard. That makes a large, useful class of queries **shard-local** —
//! correct to run independently per shard and concatenate:
//!
//! * a single triple pattern (`{ ?s ?p ?o }`) — each triple lives in exactly one
//!   shard, so the union is the full, duplicate-free result;
//! * a *subject star* (every pattern shares the same subject variable, e.g.
//!   `{ ?s :name ?n ; :age ?a }`) — the join key is the partition key, so the
//!   join never crosses a shard boundary;
//! * row-local `FILTER`, `DISTINCT` and projection over the above;
//! * a global, non-distinct `COUNT` over a shard-local pattern (sum the partials);
//! * `ASK` over a shard-local pattern (logical OR of the partials).
//!
//! Anything that could join *across* subjects (object→subject joins, property
//! paths, `SERVICE`, `OPTIONAL`/`UNION`/`MINUS`, grouped/`AVG`-style aggregates,
//! `ORDER BY`/`LIMIT`) is **not** decomposed: [`ParallelStore::query`] returns
//! `Ok(None)` so the caller can fall back to single-store evaluation. The
//! classifier is deliberately conservative — it never trades correctness for
//! parallelism.
//!
//! This is increment 1 of the parallel-execution roadmap: a self-contained,
//! tested, benchmarked capability. Wiring it into `TripleStore`'s live query
//! path (so the server shards its storage) is the next step.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use oxigraph::sparql::{QueryOptions, QueryResults, QuerySolution};
use oxigraph::store::Store;
use oxrdf::{Quad, Term, Variable};
use rayon::prelude::*;
use spargebra::algebra::{AggregateExpression, AggregateFunction, Expression, GraphPattern};
use spargebra::term::{TermPattern, TriplePattern};
use spargebra::Query;

/// How a query's per-shard partial results combine into the global answer.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Merge {
    /// Concatenate solution rows from every shard (optionally global-dedup).
    Concat { distinct: bool },
    /// Sum a single global non-distinct `COUNT` across shards.
    SumCount,
    /// Logical-OR of `ASK` booleans.
    OrAsk,
    /// Mergeable `GROUP BY`: each shard produces grouped rows; merge them by the
    /// group key, summing the (integer) non-distinct `COUNT` columns. Only `COUNT`
    /// is mergeable this way without re-deriving SPARQL's numeric-promotion rules,
    /// so `SUM`/`AVG`/`MIN`/`MAX` grouped aggregates are deliberately *not*
    /// classified here (they fall back to single-store evaluation).
    GroupCount {
        /// Projected group-key columns to merge on.
        key_vars: Vec<Variable>,
        /// Projected non-distinct `COUNT` columns to sum.
        count_vars: Vec<Variable>,
    },
}

/// A merged answer produced by [`ParallelStore::query`].
#[derive(Clone, Debug)]
pub enum ParAnswer {
    /// SELECT-style result: the variable header plus one entry per solution row.
    Solutions {
        variables: Vec<Variable>,
        rows: Vec<Vec<Option<Term>>>,
    },
    /// ASK-style boolean result.
    Boolean(bool),
}

impl ParAnswer {
    /// Number of solution rows (1 for a boolean).
    pub fn len(&self) -> usize {
        match self {
            ParAnswer::Solutions { rows, .. } => rows.len(),
            ParAnswer::Boolean(_) => 1,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An in-memory triple store split into `N` subject-partitioned shards.
pub struct ParallelStore {
    shards: Vec<Store>,
}

impl ParallelStore {
    /// Create a store with `n` shards (clamped to at least 1).
    pub fn new(n: usize) -> Self {
        let n = n.max(1);
        let shards = (0..n)
            .map(|_| Store::new().expect("in-memory oxigraph store"))
            .collect();
        Self { shards }
    }

    /// Number of shards.
    pub fn shards(&self) -> usize {
        self.shards.len()
    }

    /// Shard index for a subject string via a stable hash.
    fn shard_for_subject(&self, subject: &str) -> usize {
        let mut h = DefaultHasher::new();
        subject.hash(&mut h);
        (h.finish() % self.shards.len() as u64) as usize
    }

    /// Bulk-load quads, routing each to the shard owning its subject. Triples
    /// that share a subject always land in the same shard.
    pub fn load_quads<I: IntoIterator<Item = Quad>>(&self, quads: I) -> Result<(), String> {
        let n = self.shards.len();
        let mut buckets: Vec<Vec<Quad>> = (0..n).map(|_| Vec::new()).collect();
        for q in quads {
            let idx = self.shard_for_subject(&q.subject.to_string());
            buckets[idx].push(q);
        }
        buckets
            .into_par_iter()
            .zip(self.shards.par_iter())
            .try_for_each(|(quads, shard)| {
                shard
                    .bulk_loader()
                    .load_quads(quads)
                    .map_err(|e| e.to_string())
            })
    }

    /// Total quad count across shards (computed in parallel).
    pub fn len(&self) -> usize {
        self.shards.par_iter().map(|s| s.len().unwrap_or(0)).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Evaluate `sparql` in parallel across shards **iff** it is provably
    /// shard-decomposable. Returns `Ok(None)` for anything that is not (the
    /// caller should fall back to single-store evaluation); `Err` only on a real
    /// evaluation error inside a shard.
    pub fn query(&self, sparql: &str) -> Result<Option<ParAnswer>, String> {
        self.query_with_options(sparql, QueryOptions::default())
    }

    /// Like [`Self::query`], but evaluates each shard with the caller-supplied
    /// [`QueryOptions`] — so custom functions (GeoSPARQL, RDF 1.2, …) registered
    /// on the live store apply identically per shard, keeping the parallel result
    /// bit-for-bit equal to the single-store one. `options` is cloned per shard.
    pub fn query_with_options(
        &self,
        sparql: &str,
        options: QueryOptions,
    ) -> Result<Option<ParAnswer>, String> {
        let query = match Query::parse(sparql, None) {
            Ok(q) => q,
            Err(_) => return Ok(None),
        };
        let Some(merge) = plan(&query) else {
            return Ok(None);
        };

        // Run the identical query on every shard, concurrently.
        let partials: Vec<ShardPartial> = self
            .shards
            .par_iter()
            .map(|s| run_shard(s, sparql, &merge, &options))
            .collect::<Result<_, _>>()?;

        Ok(Some(combine(partials, &merge)))
    }
}

/// True iff `sparql` is provably shard-decomposable — a cheap parse + classify
/// with no store access. The live query path uses this to skip building (or
/// consulting) the subject-sharded mirror for queries it cannot accelerate.
pub fn is_decomposable(sparql: &str) -> bool {
    Query::parse(sparql, None)
        .ok()
        .and_then(|q| plan(&q))
        .is_some()
}

/// Coarse classification of *how* a decomposable query merges, for callers that
/// want to accelerate only **order-insensitive** shapes. A row-returning SELECT
/// (`Rows`) is concatenated shard-by-shard, so its order differs from the
/// single-store order — fine for SPARQL (unordered without `ORDER BY`) but a
/// reason a server may choose to keep it single-core. Aggregates and `ASK`
/// (`Aggregate`) return a scalar/boolean/grouped set where merge order is
/// irrelevant, so they are always safe to parallelize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParClass {
    /// Global or grouped aggregate, or `ASK`: result is a scalar/set.
    Aggregate,
    /// Row-returning SELECT: concat-merged, so shard order is observable.
    Rows,
}

/// Classify a query's parallel shape, or `None` if it is not decomposable.
pub fn classify(sparql: &str) -> Option<ParClass> {
    let query = Query::parse(sparql, None).ok()?;
    Some(match plan(&query)? {
        Merge::Concat { .. } => ParClass::Rows,
        Merge::SumCount | Merge::OrAsk | Merge::GroupCount { .. } => ParClass::Aggregate,
    })
}

/// Per-shard partial result, shaped by the merge strategy.
enum ShardPartial {
    Rows {
        variables: Vec<Variable>,
        rows: Vec<Vec<Option<Term>>>,
    },
    Count {
        /// The query's projection variable (e.g. `?n` in `COUNT(*) AS ?n`), kept so
        /// the merged answer carries the caller's actual name, not a fixed one.
        var: Option<Variable>,
        value: i128,
    },
    Bool(bool),
}

fn run_shard(
    store: &Store,
    sparql: &str,
    merge: &Merge,
    options: &QueryOptions,
) -> Result<ShardPartial, String> {
    let results = store
        .query_opt(sparql, options.clone())
        .map_err(|e| e.to_string())?;
    let mismatch = || "parallel: result shape did not match the planned merge".to_string();
    match results {
        QueryResults::Boolean(b) if matches!(merge, Merge::OrAsk) => Ok(ShardPartial::Bool(b)),
        QueryResults::Solutions(sols) => match merge {
            Merge::SumCount => {
                let vars: Vec<Variable> = sols.variables().to_vec();
                let var = vars.first().cloned();
                let mut total: i128 = 0;
                for sol in sols {
                    let sol = sol.map_err(|e| e.to_string())?;
                    if let Some(v) = vars.first() {
                        if let Some(Term::Literal(lit)) = sol.get(v) {
                            total += lit.value().parse::<i128>().unwrap_or(0);
                        }
                    }
                }
                Ok(ShardPartial::Count { var, value: total })
            }
            // `GroupCount` collects the per-shard grouped rows exactly like
            // `Concat`; the per-group sum happens in `combine`.
            Merge::Concat { .. } | Merge::GroupCount { .. } => {
                let vars: Vec<Variable> = sols.variables().to_vec();
                let rows = collect_rows(sols, &vars)?;
                Ok(ShardPartial::Rows {
                    variables: vars,
                    rows,
                })
            }
            _ => Err(mismatch()),
        },
        // Shape/strategy mismatch should never happen given the classifier, but
        // surface it rather than guess.
        _ => Err(mismatch()),
    }
}

fn collect_rows(
    sols: oxigraph::sparql::QuerySolutionIter,
    vars: &[Variable],
) -> Result<Vec<Vec<Option<Term>>>, String> {
    let mut rows = Vec::new();
    for sol in sols {
        let sol: QuerySolution = sol.map_err(|e| e.to_string())?;
        rows.push(vars.iter().map(|v| sol.get(v).cloned()).collect());
    }
    Ok(rows)
}

fn combine(partials: Vec<ShardPartial>, merge: &Merge) -> ParAnswer {
    match merge {
        Merge::OrAsk => {
            let any = partials
                .iter()
                .any(|p| matches!(p, ShardPartial::Bool(true)));
            ParAnswer::Boolean(any)
        }
        Merge::SumCount => {
            let total: i128 = partials
                .iter()
                .map(|p| match p {
                    ShardPartial::Count { value, .. } => *value,
                    _ => 0,
                })
                .sum();
            // Preserve the query's real projection variable (e.g. `?n`), not a
            // fixed `?c`, so the caller reads the count by the name it asked for.
            let var = partials
                .iter()
                .find_map(|p| match p {
                    ShardPartial::Count { var: Some(v), .. } => Some(v.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| Variable::new_unchecked("c"));
            let lit =
                oxrdf::Literal::new_typed_literal(total.to_string(), oxrdf::vocab::xsd::INTEGER);
            ParAnswer::Solutions {
                variables: vec![var],
                rows: vec![vec![Some(Term::Literal(lit))]],
            }
        }
        Merge::Concat { distinct } => {
            let mut variables: Vec<Variable> = Vec::new();
            let mut rows: Vec<Vec<Option<Term>>> = Vec::new();
            for p in partials {
                if let ShardPartial::Rows {
                    variables: v,
                    rows: r,
                } = p
                {
                    if variables.is_empty() {
                        variables = v;
                    }
                    rows.extend(r);
                }
            }
            if *distinct {
                let mut seen = std::collections::HashSet::new();
                rows.retain(|row| seen.insert(row_key(row)));
            }
            ParAnswer::Solutions { variables, rows }
        }
        Merge::GroupCount {
            key_vars,
            count_vars,
        } => combine_group_count(partials, key_vars, count_vars),
    }
}

/// Merge per-shard grouped rows: group by the key columns and sum the (integer)
/// `COUNT` columns. The output header and column order are the projection's, taken
/// from the shards' result variables. First-seen group order is preserved (SPARQL
/// is unordered without `ORDER BY`, so any deterministic order is conformant).
fn combine_group_count(
    partials: Vec<ShardPartial>,
    key_vars: &[Variable],
    count_vars: &[Variable],
) -> ParAnswer {
    use std::collections::HashMap;

    let header: Vec<Variable> = partials
        .iter()
        .find_map(|p| match p {
            ShardPartial::Rows { variables, .. } => Some(variables.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let key_idx: Vec<usize> = header
        .iter()
        .enumerate()
        .filter(|(_, v)| key_vars.contains(v))
        .map(|(i, _)| i)
        .collect();
    let count_idx: Vec<usize> = header
        .iter()
        .enumerate()
        .filter(|(_, v)| count_vars.contains(v))
        .map(|(i, _)| i)
        .collect();

    // key string -> (representative row for the key columns, per-count running sum)
    let mut groups: HashMap<String, (Vec<Option<Term>>, Vec<i128>)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for p in &partials {
        if let ShardPartial::Rows { rows, .. } = p {
            for row in rows {
                let mut key = String::new();
                for &i in &key_idx {
                    match row.get(i).and_then(|c| c.as_ref()) {
                        Some(t) => key.push_str(&t.to_string()),
                        None => key.push('\u{1}'),
                    }
                    key.push('\u{2}');
                }
                let entry = groups.entry(key.clone()).or_insert_with(|| {
                    order.push(key.clone());
                    (row.clone(), vec![0i128; count_idx.len()])
                });
                for (j, &i) in count_idx.iter().enumerate() {
                    if let Some(Some(Term::Literal(lit))) = row.get(i) {
                        entry.1[j] += lit.value().parse::<i128>().unwrap_or(0);
                    }
                }
            }
        }
    }

    let mut out_rows: Vec<Vec<Option<Term>>> = Vec::with_capacity(order.len());
    for key in &order {
        let (template, sums) = &groups[key];
        let mut row: Vec<Option<Term>> = vec![None; header.len()];
        for &i in &key_idx {
            row[i] = template.get(i).cloned().flatten();
        }
        for (j, &i) in count_idx.iter().enumerate() {
            let lit =
                oxrdf::Literal::new_typed_literal(sums[j].to_string(), oxrdf::vocab::xsd::INTEGER);
            row[i] = Some(Term::Literal(lit));
        }
        out_rows.push(row);
    }

    ParAnswer::Solutions {
        variables: header,
        rows: out_rows,
    }
}

/// Stable string key for a solution row (for global DISTINCT dedup).
fn row_key(row: &[Option<Term>]) -> String {
    let mut s = String::new();
    for cell in row {
        match cell {
            Some(t) => s.push_str(&t.to_string()),
            None => s.push('\u{1}'), // unbound sentinel
        }
        s.push('\u{2}');
    }
    s
}

// ─── Decomposition classifier ──────────────────────────────────────────────────

/// Decide how (and whether) a query decomposes across subject shards.
fn plan(query: &Query) -> Option<Merge> {
    match query {
        Query::Ask { pattern, .. } => {
            if shard_local_rows(pattern) {
                Some(Merge::OrAsk)
            } else {
                None
            }
        }
        Query::Select { pattern, .. } => plan_select(pattern, false),
        _ => None,
    }
}

fn plan_select(pattern: &GraphPattern, distinct: bool) -> Option<Merge> {
    match pattern {
        GraphPattern::Project { inner, variables } => {
            // A mergeable GROUP BY (keys + non-distinct COUNTs) is the one shape
            // where the projection list matters, so handle it before recursing.
            if let Some(merge) = plan_group_count(variables, inner) {
                return Some(merge);
            }
            plan_select(inner, distinct)
        }
        GraphPattern::Distinct { inner } => plan_select(inner, true),
        GraphPattern::Reduced { inner } => plan_select(inner, distinct),
        // `(COUNT(*) AS ?c)` parses to Extend(Group, ?c, Variable(internal)). A
        // pure variable alias over a global COUNT is sum-safe; a *computed*
        // expression over an aggregate (e.g. COUNT(*)+1) is not, so reject it.
        // Over a plain row stream, Extend (BIND) is row-local → still concat-safe.
        GraphPattern::Extend {
            inner, expression, ..
        } => match plan_select(inner, distinct) {
            Some(Merge::SumCount) if !matches!(expression, Expression::Variable(_)) => None,
            other => other,
        },
        // Global, non-distinct COUNT over a shard-local pattern → sum partials.
        GraphPattern::Group {
            inner,
            variables,
            aggregates,
        } if variables.is_empty()
            && aggregates.len() == 1
            && is_nondistinct_count(&aggregates[0].1)
            && shard_local_rows(inner) =>
        {
            Some(Merge::SumCount)
        }
        // Plain row stream (BGP / FILTER) over a shard-local pattern → concat.
        GraphPattern::Bgp { .. } | GraphPattern::Filter { .. } if shard_local_rows(pattern) => {
            Some(Merge::Concat { distinct })
        }
        _ => None,
    }
}

/// Classify a mergeable `GROUP BY`: `Project { keys + COUNT aliases }` over
/// `Extend*(Group { keys, [(internal, COUNT)…] })` — the shape SPARQL parses
/// `SELECT ?k (COUNT(*) AS ?c) … GROUP BY ?k` into. Every projected column must be
/// a group key or alias a non-distinct `COUNT` (so per-group summing is exact),
/// every key must be projected (so the merge can group on it), and the grouped
/// pattern must be shard-local. Returns `None` for anything else — including
/// `SUM`/`AVG`/`MIN`/`MAX` grouped aggregates, which need SPARQL's numeric-promotion
/// rules to merge correctly and so stay single-store.
fn plan_group_count(proj_vars: &[Variable], inner: &GraphPattern) -> Option<Merge> {
    use std::collections::{HashMap, HashSet};

    // Walk the Extend chain that aliases internal aggregate vars to user names.
    let mut alias: HashMap<String, String> = HashMap::new(); // user name -> internal name
    let mut node = inner;
    let (keys, aggregates, g_inner) = loop {
        match node {
            GraphPattern::Extend {
                inner,
                variable,
                expression,
            } => {
                match expression {
                    Expression::Variable(v) => {
                        alias.insert(variable.as_str().to_string(), v.as_str().to_string());
                    }
                    // A computed expression over an aggregate isn't sum-mergeable.
                    _ => return None,
                }
                node = inner;
            }
            GraphPattern::Group {
                inner,
                variables,
                aggregates,
            } => break (variables, aggregates, inner),
            _ => return None,
        }
    };

    if keys.is_empty() {
        return None; // empty keys = the global SumCount case, handled elsewhere
    }
    if !shard_local_rows(g_inner) {
        return None;
    }
    // Every aggregate must be a non-distinct COUNT (binding an internal var).
    let mut count_internal: HashSet<String> = HashSet::new();
    for (var, agg) in aggregates {
        if !is_nondistinct_count(agg) {
            return None;
        }
        count_internal.insert(var.as_str().to_string());
    }

    // Classify every projected column as a key or a COUNT alias.
    let key_set: HashSet<&str> = keys.iter().map(|v| v.as_str()).collect();
    let mut key_vars: Vec<Variable> = Vec::new();
    let mut count_vars: Vec<Variable> = Vec::new();
    for pv in proj_vars {
        if key_set.contains(pv.as_str()) {
            key_vars.push(pv.clone());
        } else if let Some(internal) = alias.get(pv.as_str()) {
            if count_internal.contains(internal) {
                count_vars.push(pv.clone());
            } else {
                return None; // aliases a non-COUNT aggregate
            }
        } else {
            return None; // a projected column that is neither key nor COUNT
        }
    }
    // All keys must be projected (so the merge can group on them), and there must
    // be at least one COUNT column (else this is just a plain projection).
    if key_vars.len() != keys.len() || count_vars.is_empty() {
        return None;
    }
    Some(Merge::GroupCount {
        key_vars,
        count_vars,
    })
}

fn is_nondistinct_count(agg: &AggregateExpression) -> bool {
    matches!(
        agg,
        AggregateExpression::CountSolutions { distinct: false }
            | AggregateExpression::FunctionCall {
                name: AggregateFunction::Count,
                distinct: false,
                ..
            }
    )
}

/// True iff `pattern` produces a shard-local row stream: only BGP / FILTER /
/// PROJECT / DISTINCT / REDUCED nodes, and every triple pattern shares one
/// subject variable (or there is a single pattern).
fn shard_local_rows(pattern: &GraphPattern) -> bool {
    let mut pats: Vec<TriplePattern> = Vec::new();
    collect_rowable(pattern, &mut pats) && subject_local(&pats)
}

fn collect_rowable(pattern: &GraphPattern, out: &mut Vec<TriplePattern>) -> bool {
    match pattern {
        GraphPattern::Bgp { patterns } => {
            out.extend(patterns.iter().cloned());
            true
        }
        GraphPattern::Filter { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner } => collect_rowable(inner, out),
        _ => false,
    }
}

/// All patterns must share a single subject *variable*. A single pattern of any
/// shape is always shard-safe (it is a partitioned scan). Bound/mixed subjects
/// across multiple patterns are rejected (they could form a cross-subject
/// cartesian product that does not decompose).
fn subject_local(patterns: &[TriplePattern]) -> bool {
    if patterns.is_empty() {
        return false; // empty BGP: let the engine handle it
    }
    if patterns.len() == 1 {
        return true;
    }
    let mut subject: Option<&str> = None;
    for tp in patterns {
        match &tp.subject {
            TermPattern::Variable(v) => match subject {
                None => subject = Some(v.as_str()),
                Some(s) if s == v.as_str() => {}
                _ => return false,
            },
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{GraphName, Literal, NamedNode, Quad, Subject, Term};

    fn iri(s: &str) -> NamedNode {
        NamedNode::new(s).unwrap()
    }

    /// Generate the same person workload the criterion suite uses.
    fn persons(n: usize) -> Vec<Quad> {
        let ex = "http://example.org/";
        let mut quads = Vec::with_capacity(n * 3);
        for i in 0..n {
            let s = Subject::NamedNode(iri(&format!("{ex}p{i}")));
            quads.push(Quad::new(
                s.clone(),
                iri(&format!("{ex}name")),
                Term::Literal(Literal::new_simple_literal(format!("Person {i}"))),
                GraphName::DefaultGraph,
            ));
            quads.push(Quad::new(
                s.clone(),
                iri(&format!("{ex}age")),
                Term::Literal(Literal::new_typed_literal(
                    (18 + i % 65).to_string(),
                    oxrdf::vocab::xsd::INTEGER,
                )),
                GraphName::DefaultGraph,
            ));
            quads.push(Quad::new(
                s,
                iri(&format!("{ex}type")),
                Term::NamedNode(iri(&format!("{ex}Type{}", i % 10))),
                GraphName::DefaultGraph,
            ));
        }
        quads
    }

    /// Reference: run a query on a single oxigraph store, normalised to a sorted
    /// multiset of stringified rows (or "ASK:<bool>").
    fn single_store_answer(quads: &[Quad], sparql: &str) -> Vec<String> {
        let store = Store::new().unwrap();
        store
            .bulk_loader()
            .load_quads(quads.iter().cloned())
            .unwrap();
        normalise(store.query(sparql).unwrap())
    }

    fn normalise(results: QueryResults) -> Vec<String> {
        match results {
            QueryResults::Boolean(b) => vec![format!("ASK:{b}")],
            QueryResults::Solutions(sols) => {
                let vars: Vec<Variable> = sols.variables().to_vec();
                let mut rows: Vec<String> = sols
                    .map(|s| {
                        let s = s.unwrap();
                        row_key(&vars.iter().map(|v| s.get(v).cloned()).collect::<Vec<_>>())
                    })
                    .collect();
                rows.sort();
                rows
            }
            QueryResults::Graph(_) => vec!["<graph>".into()],
        }
    }

    fn par_answer_sorted(a: &ParAnswer) -> Vec<String> {
        match a {
            ParAnswer::Boolean(b) => vec![format!("ASK:{b}")],
            ParAnswer::Solutions { rows, .. } => {
                let mut r: Vec<String> = rows.iter().map(|row| row_key(row)).collect();
                r.sort();
                r
            }
        }
    }

    /// A parallel answer must equal the single-store answer for every
    /// decomposable query, across several shard counts.
    fn assert_matches(quads: &[Quad], sparql: &str) {
        let expected = single_store_answer(quads, sparql);
        for n in [1usize, 2, 4, 8] {
            let ps = ParallelStore::new(n);
            ps.load_quads(quads.iter().cloned()).unwrap();
            let got = ps
                .query(sparql)
                .unwrap()
                .unwrap_or_else(|| panic!("query should be decomposable: {sparql}"));
            assert_eq!(
                par_answer_sorted(&got),
                expected,
                "mismatch at {n} shards for: {sparql}"
            );
        }
    }

    #[test]
    fn count_star_sums_across_shards() {
        let q = persons(500);
        assert_matches(&q, "SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }");
    }

    #[test]
    fn subject_star_join_count() {
        let q = persons(500);
        assert_matches(
            &q,
            "SELECT (COUNT(*) AS ?c) WHERE { ?s <http://example.org/name> ?n . ?s <http://example.org/age> ?a }",
        );
    }

    #[test]
    fn plain_select_concatenates() {
        let q = persons(200);
        assert_matches(&q, "SELECT ?s ?n WHERE { ?s <http://example.org/name> ?n }");
    }

    #[test]
    fn subject_star_select() {
        let q = persons(200);
        assert_matches(
            &q,
            "SELECT ?s ?n ?a WHERE { ?s <http://example.org/name> ?n . ?s <http://example.org/age> ?a }",
        );
    }

    #[test]
    fn filter_then_count() {
        let q = persons(500);
        assert_matches(
            &q,
            "SELECT (COUNT(*) AS ?c) WHERE { ?s <http://example.org/age> ?a FILTER(?a >= 40 && ?a < 60) }",
        );
    }

    #[test]
    fn distinct_select_dedups_globally() {
        let q = persons(300);
        assert_matches(
            &q,
            "SELECT DISTINCT ?t WHERE { ?s <http://example.org/type> ?t }",
        );
    }

    #[test]
    fn ask_ors_across_shards() {
        let q = persons(300);
        assert_matches(&q, "ASK { ?s <http://example.org/name> \"Person 7\" }");
        assert_matches(&q, "ASK { ?s <http://example.org/name> \"Nobody\" }");
    }

    #[test]
    fn group_by_count_decomposes() {
        // COUNT per group is mergeable: each shard counts its subjects per ?t and
        // the per-group counts sum (the same type spans shards, but its count is a
        // sum of per-shard counts).
        let q = persons(500);
        assert_matches(
            &q,
            "SELECT ?t (COUNT(*) AS ?c) WHERE { ?s <http://example.org/type> ?t } GROUP BY ?t",
        );
    }

    #[test]
    fn group_by_count_subject_star_decomposes() {
        let q = persons(500);
        assert_matches(
            &q,
            "SELECT ?t (COUNT(?n) AS ?c) WHERE { ?s <http://example.org/type> ?t . ?s <http://example.org/name> ?n } GROUP BY ?t",
        );
    }

    #[test]
    fn group_by_count_filtered_decomposes() {
        let q = persons(500);
        assert_matches(
            &q,
            "SELECT ?t (COUNT(*) AS ?c) WHERE { ?s <http://example.org/type> ?t . ?s <http://example.org/age> ?a FILTER(?a >= 40) } GROUP BY ?t",
        );
    }

    // ── Negative cases: must be classified NON-decomposable (query → None) ──

    fn assert_not_decomposable(sparql: &str) {
        let ps = ParallelStore::new(4);
        ps.load_quads(persons(50).into_iter()).unwrap();
        assert!(
            ps.query(sparql).unwrap().is_none(),
            "must NOT be decomposed (could be wrong across subject shards): {sparql}"
        );
    }

    #[test]
    fn cross_subject_join_is_rejected() {
        // ?mid is object in one pattern, subject in another → crosses shards.
        assert_not_decomposable(
            "SELECT ?a ?c WHERE { ?a <http://example.org/knows> ?mid . ?mid <http://example.org/knows> ?c }",
        );
    }

    #[test]
    fn grouped_aggregate_is_rejected() {
        // GROUP BY + AVG cannot be merged by summing (avg of avgs ≠ overall avg);
        // only non-distinct COUNT is mergeable, so AVG stays single-store.
        assert_not_decomposable(
            "SELECT ?t (AVG(?a) AS ?avg) WHERE { ?s <http://example.org/type> ?t . ?s <http://example.org/age> ?a } GROUP BY ?t",
        );
    }

    #[test]
    fn grouped_sum_and_distinct_count_are_rejected() {
        // SUM/MIN/MAX would need SPARQL's numeric-promotion rules to merge, and a
        // grouped COUNT(DISTINCT) is not sum-safe across shards — all rejected.
        assert_not_decomposable(
            "SELECT ?t (SUM(?a) AS ?s) WHERE { ?s <http://example.org/type> ?t . ?s <http://example.org/age> ?a } GROUP BY ?t",
        );
        assert_not_decomposable(
            "SELECT ?t (COUNT(DISTINCT ?n) AS ?c) WHERE { ?s <http://example.org/type> ?t . ?s <http://example.org/name> ?n } GROUP BY ?t",
        );
    }

    #[test]
    fn distinct_count_is_rejected() {
        // COUNT(DISTINCT ?t) is not sum-safe (a type spans shards).
        assert_not_decomposable(
            "SELECT (COUNT(DISTINCT ?t) AS ?c) WHERE { ?s <http://example.org/type> ?t }",
        );
    }

    #[test]
    fn property_path_is_rejected() {
        assert_not_decomposable("SELECT ?a ?b WHERE { ?a <http://example.org/knows>+ ?b }");
    }

    #[test]
    fn order_by_limit_is_rejected() {
        // Per-shard LIMIT/ORDER does not equal a global ordered slice.
        assert_not_decomposable(
            "SELECT ?s ?n WHERE { ?s <http://example.org/name> ?n } ORDER BY ?n LIMIT 5",
        );
    }
}
