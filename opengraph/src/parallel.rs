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
//! * `ASK` over a shard-local pattern (logical OR of the partials);
//! * a mergeable `GROUP BY` over a shard-local pattern — `COUNT` (sum the per-group
//!   partials) and, via a rewrite-and-re-merge through the engine itself,
//!   `SUM`/`MIN`/`MAX`/`AVG` (`AVG` → per-shard `SUM`+`COUNT`). This is exact for
//!   `xsd:integer`/`decimal`; `SUM`/`AVG` over `xsd:double`/`float` is declined at
//!   runtime (IEEE-754 is non-associative) — see the grouped-aggregate section below.
//!
//! Anything that could join *across* subjects (object→subject joins, property
//! paths, `SERVICE`, `OPTIONAL`/`UNION`/`MINUS`, `ORDER BY`/`LIMIT`) is **not**
//! decomposed: [`ParallelStore::query`] returns `Ok(None)` so the caller can fall
//! back to single-store evaluation. The classifier is deliberately conservative —
//! it never trades correctness for parallelism.
//!
//! This is increment 1 of the parallel-execution roadmap: a self-contained,
//! tested, benchmarked capability. Wiring it into `TripleStore`'s live query
//! path (so the server shards its storage) is the next step.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use oxigraph::sparql::{QueryOptions, QueryResults, QuerySolution};
use oxigraph::store::Store;
use oxrdf::{BlankNode, GraphName, NamedNode, Quad, Term, Variable};
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
    /// Mergeable `GROUP BY` of pure non-distinct `COUNT`s: each shard produces
    /// grouped rows; merge them by the group key, summing the (integer) `COUNT`
    /// columns in a single pass. This is the lightweight path for `COUNT`-only
    /// grouped queries; grouped `SUM`/`MIN`/`MAX`/`AVG` (and mixes with `COUNT`) take
    /// the more general [`plan_group_aggregate`] path instead, which rewrites the
    /// per-shard query and re-merges through the engine.
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
        // A mergeable grouped SUM/MIN/MAX/AVG needs a *rewritten* per-shard query
        // (partials) and an engine-driven merge, so try it before the strategies
        // that run the identical query on every shard.
        if let Some(gplan) = plan_group_aggregate(&query) {
            return self.run_group_aggregate(&query, &gplan, &options);
        }
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

    /// Evaluate a mergeable grouped aggregate (`SUM`/`MIN`/`MAX`/`AVG`, possibly
    /// alongside `COUNT`s) by running the per-shard *partial* query concurrently and
    /// merging the partials **through the engine itself** — see [`merge_group_agg`].
    ///
    /// Returns `Ok(None)` to **decline** when the partials sum `xsd:double`/`float`
    /// (IEEE-754 addition is non-associative, so a cross-shard sum is not bit-exact):
    /// the caller then evaluates on the unsharded copy, keeping the answer identical
    /// to single-store. Decomposition is exact for `xsd:integer`/`decimal` (addition
    /// is associative) and for `MIN`/`MAX` of any type (order-independent).
    fn run_group_aggregate(
        &self,
        orig: &Query,
        plan: &GroupAggPlan,
        options: &QueryOptions,
    ) -> Result<Option<ParAnswer>, String> {
        let Some(partial_sparql) = build_partial_query(orig, plan) else {
            return Ok(None);
        };
        // Per-shard partials, concurrently. Custom functions in the WHERE apply
        // identically per shard via the caller-supplied options.
        let per_shard: Vec<Vec<Vec<Option<Term>>>> = self
            .shards
            .par_iter()
            .map(|s| run_partial_shard(s, &partial_sparql, &plan.partial_proj, options))
            .collect::<Result<_, _>>()?;
        let rows: Vec<Vec<Option<Term>>> = per_shard.into_iter().flatten().collect();
        // Fidelity guard: a summed double/float cannot be merged bit-exactly.
        if rows_have_double(&rows, plan) {
            return Ok(None);
        }
        Ok(Some(merge_group_agg(&rows, plan)?))
    }
}

/// True iff `sparql` is provably shard-decomposable — a cheap parse + classify
/// with no store access. The live query path uses this to skip building (or
/// consulting) the subject-sharded mirror for queries it cannot accelerate.
pub fn is_decomposable(sparql: &str) -> bool {
    Query::parse(sparql, None)
        .ok()
        .map(|q| plan(&q).is_some() || plan_group_aggregate(&q).is_some())
        .unwrap_or(false)
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
    // A mergeable grouped non-COUNT aggregate is an order-insensitive set result.
    if plan_group_aggregate(&query).is_some() {
        return Some(ParClass::Aggregate);
    }
    Some(match plan(&query)? {
        Merge::Concat { .. } => ParClass::Rows,
        Merge::SumCount | Merge::OrAsk | Merge::GroupCount { .. } => ParClass::Aggregate,
    })
}

/// True iff `sparql` contains a `SUM` or `AVG` aggregate anywhere — the *order-
/// sensitive* aggregates, since IEEE-754 addition is non-associative for
/// `xsd:double`/`xsd:float` (summing the same values in a different order can differ
/// in the last bit). A second in-memory copy of the data iterates quads in a
/// different order than the persistent store, so a `SUM`/`AVG` over doubles evaluated
/// on the copy can differ from the persistent store's own result in the last ULP.
///
/// The mirror uses this to keep its **byte-identical** guarantee: such queries are
/// declined by the full in-memory copy so the persistent store answers them itself.
/// `MIN`/`MAX`/`COUNT` are order-independent and unaffected; grouped int/decimal
/// `SUM`/`AVG` are served exactly (and in parallel) by the subject shards *before* the
/// full copy is consulted, so this only defers the `SUM`/`AVG` shapes the shards do
/// not decompose (global, computed-expression, or otherwise complex ones).
pub fn has_sum_or_avg(sparql: &str) -> bool {
    let Ok(query) = Query::parse(sparql, None) else {
        return false;
    };
    let pattern = match &query {
        Query::Select { pattern, .. }
        | Query::Construct { pattern, .. }
        | Query::Describe { pattern, .. }
        | Query::Ask { pattern, .. } => pattern,
    };
    pattern_has_sum_or_avg(pattern)
}

/// Recursively search a graph pattern for a `SUM`/`AVG` aggregate. Aggregates live
/// only in `Group` nodes, reached through the standard container patterns.
fn pattern_has_sum_or_avg(p: &GraphPattern) -> bool {
    match p {
        GraphPattern::Group {
            inner, aggregates, ..
        } => {
            aggregates.iter().any(|(_, a)| {
                matches!(
                    a,
                    AggregateExpression::FunctionCall {
                        name: AggregateFunction::Sum | AggregateFunction::Avg,
                        ..
                    }
                )
            }) || pattern_has_sum_or_avg(inner)
        }
        GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::Filter { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Service { inner, .. }
        | GraphPattern::Graph { inner, .. } => pattern_has_sum_or_avg(inner),
        GraphPattern::Join { left, right }
        | GraphPattern::LeftJoin { left, right, .. }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right } => {
            pattern_has_sum_or_avg(left) || pattern_has_sum_or_avg(right)
        }
        // Bgp / Path / Values carry no aggregates.
        _ => false,
    }
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

// ─── Mergeable grouped non-COUNT aggregates (SUM / MIN / MAX / AVG) ──────────────
//
// A more general grouped path than `GroupCount`. `SUM`/`MIN`/`MAX`/`AVG` over
// subject shards are decomposable when the per-shard partials are merged exactly the
// way the engine itself would — so the merge is *done by the engine*: each shard
// computes partials (`AVG(?v)` → `SUM(?v)` + `COUNT(?v)`; the others directly), the
// partials are materialised into a throwaway in-memory store, and one final
// aggregation query re-merges them (`SUM(sum)/SUM(cnt)` for AVG, `SUM`/`MIN`/`MAX`
// for the rest). Because Oxigraph's `AVG` is byte-identical to `SUM/COUNT` and
// integer/decimal addition is exact and associative, the result is identical to
// single-store evaluation.
//
// The one exception is IEEE-754: `SUM`/`AVG` over `xsd:double`/`xsd:float` is
// non-associative — summing per-shard partials in a different order can differ in the
// last bit — so when any summed partial is double/float the decomposition is DECLINED
// at runtime (`run_group_aggregate` returns `Ok(None)`) and the caller falls back to
// the unsharded copy. `MIN`/`MAX` are order-independent (min-of-mins = global min
// under SPARQL's total order) and pass the selected term through unchanged, so they
// decompose for every type, doubles included.

/// `xsd:double`/`xsd:float`: datatypes whose summation is not associative, so a
/// cross-shard `SUM`/`AVG` merge is not bit-exact and must fall back.
const XSD_DOUBLE: &str = "http://www.w3.org/2001/XMLSchema#double";
const XSD_FLOAT: &str = "http://www.w3.org/2001/XMLSchema#float";
/// Predicate base for the materialised partial columns (throwaway merge store).
const GACOL: &str = "http://ots.invalid/gacol/";

/// How one output column of a mergeable grouped aggregate is produced from the
/// per-shard partials.
#[derive(Clone)]
enum AggCol {
    /// A group key — passes through unchanged.
    Key(Variable),
    /// `COUNT` or `SUM`: the global value is the SUM of the per-shard partials.
    SumOf { out: Variable, partial: Variable },
    /// `MIN`: the MIN of the per-shard partials.
    MinOf { out: Variable, partial: Variable },
    /// `MAX`: the MAX of the per-shard partials.
    MaxOf { out: Variable, partial: Variable },
    /// `AVG`: total SUM / total COUNT (Oxigraph's AVG ≡ SUM/COUNT, bit-for-bit).
    AvgOf {
        out: Variable,
        sum: Variable,
        cnt: Variable,
    },
}

/// A plan for decomposing a mergeable grouped aggregate across subject shards.
struct GroupAggPlan {
    /// Group-by key variables (also the leading columns of the partial query).
    keys: Vec<Variable>,
    /// The grouped pattern (the WHERE), reused verbatim per shard.
    group_inner: GraphPattern,
    /// Per-shard partial aggregates: `(binding var, aggregate)`.
    partials: Vec<(Variable, AggregateExpression)>,
    /// The partial query's projection — `keys` then every partial var, in order.
    partial_proj: Vec<Variable>,
    /// Output columns, in the original projection order.
    outputs: Vec<AggCol>,
    /// Partial vars carrying a `SUM` (a `SUM` aggregate, or an `AVG`'s sum half) —
    /// the ones whose values must not be `xsd:double`/`float` for an exact merge.
    sum_partials: Vec<Variable>,
}

/// Fresh internal variable for a partial aggregate column.
fn agg_var(n: usize) -> Variable {
    Variable::new_unchecked(format!("__pa{n}"))
}

/// Append a `(fresh var, FunctionCall)` partial and return the fresh var.
fn push_partial(
    partials: &mut Vec<(Variable, AggregateExpression)>,
    next: &mut usize,
    name: AggregateFunction,
    arg: &Expression,
) -> Variable {
    let v = agg_var(*next);
    *next += 1;
    partials.push((
        v.clone(),
        AggregateExpression::FunctionCall {
            name,
            expr: arg.clone(),
            distinct: false,
        },
    ));
    v
}

/// Classify a mergeable `GROUP BY` carrying at least one `SUM`/`MIN`/`MAX`/`AVG`
/// (alongside any `COUNT`s) — the shape `GroupCount` deliberately rejects. Returns a
/// plan, or `None` for anything not provably mergeable: a distinct aggregate,
/// `GROUP_CONCAT`/`SAMPLE`, an aggregate over a computed expression (only a plain
/// variable, or `COUNT(*)`, is taken), a non-projected key, or a non-shard-local
/// pattern. A statically-accepted plan can still be DECLINED at runtime if it sums
/// doubles (see [`rows_have_double`]).
fn plan_group_aggregate(query: &Query) -> Option<GroupAggPlan> {
    use std::collections::{HashMap, HashSet};

    let Query::Select { pattern, .. } = query else {
        return None;
    };
    // Unwrap to the projection variables and the node directly below Project.
    let (proj_vars, mut node): (&[Variable], &GraphPattern) = match pattern {
        GraphPattern::Project { inner, variables } => (variables, inner),
        GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } => {
            match inner.as_ref() {
                GraphPattern::Project { inner, variables } => (variables, inner),
                _ => return None,
            }
        }
        _ => return None,
    };
    // Walk the Extend chain (internal aggregate var → user name), reach the Group.
    let mut alias: HashMap<String, String> = HashMap::new();
    let (keys, aggregates, group_inner) = loop {
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
                    _ => return None, // computed expression over an aggregate
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
    if keys.is_empty() || !shard_local_rows(group_inner) {
        return None;
    }
    let agg_by_internal: HashMap<&str, &AggregateExpression> =
        aggregates.iter().map(|(v, a)| (v.as_str(), a)).collect();
    let key_set: HashSet<&str> = keys.iter().map(|v| v.as_str()).collect();

    let mut partials: Vec<(Variable, AggregateExpression)> = Vec::new();
    let mut outputs: Vec<AggCol> = Vec::new();
    let mut sum_partials: Vec<Variable> = Vec::new();
    let mut next = 0usize;
    let mut has_non_count = false;

    for pv in proj_vars {
        if key_set.contains(pv.as_str()) {
            outputs.push(AggCol::Key(pv.clone()));
            continue;
        }
        // A non-key column must alias an aggregate the Group binds.
        let internal = alias.get(pv.as_str())?;
        match *agg_by_internal.get(internal.as_str())? {
            AggregateExpression::CountSolutions { distinct: false } => {
                let p = agg_var(next);
                next += 1;
                partials.push((
                    p.clone(),
                    AggregateExpression::CountSolutions { distinct: false },
                ));
                outputs.push(AggCol::SumOf {
                    out: pv.clone(),
                    partial: p,
                });
            }
            AggregateExpression::FunctionCall {
                name,
                expr: Expression::Variable(arg),
                distinct: false,
            } => {
                // Only a plain-variable argument (keeps the partial query simple and
                // the aggregated value bound by the required shard-local pattern).
                let arg = Expression::Variable(arg.clone());
                match name {
                    AggregateFunction::Count => {
                        let p =
                            push_partial(&mut partials, &mut next, AggregateFunction::Count, &arg);
                        outputs.push(AggCol::SumOf {
                            out: pv.clone(),
                            partial: p,
                        });
                    }
                    AggregateFunction::Sum => {
                        let p =
                            push_partial(&mut partials, &mut next, AggregateFunction::Sum, &arg);
                        sum_partials.push(p.clone());
                        outputs.push(AggCol::SumOf {
                            out: pv.clone(),
                            partial: p,
                        });
                        has_non_count = true;
                    }
                    AggregateFunction::Min => {
                        let p =
                            push_partial(&mut partials, &mut next, AggregateFunction::Min, &arg);
                        outputs.push(AggCol::MinOf {
                            out: pv.clone(),
                            partial: p,
                        });
                        has_non_count = true;
                    }
                    AggregateFunction::Max => {
                        let p =
                            push_partial(&mut partials, &mut next, AggregateFunction::Max, &arg);
                        outputs.push(AggCol::MaxOf {
                            out: pv.clone(),
                            partial: p,
                        });
                        has_non_count = true;
                    }
                    AggregateFunction::Avg => {
                        let s =
                            push_partial(&mut partials, &mut next, AggregateFunction::Sum, &arg);
                        let c =
                            push_partial(&mut partials, &mut next, AggregateFunction::Count, &arg);
                        sum_partials.push(s.clone());
                        outputs.push(AggCol::AvgOf {
                            out: pv.clone(),
                            sum: s,
                            cnt: c,
                        });
                        has_non_count = true;
                    }
                    _ => return None, // GroupConcat, Sample
                }
            }
            _ => return None, // distinct aggregate, or non-variable argument
        }
    }

    // Every group key must be projected (so the merge can group on it), and there
    // must be a non-COUNT aggregate (pure COUNT goes through the lighter path).
    let projected_keys = outputs
        .iter()
        .filter(|o| matches!(o, AggCol::Key(_)))
        .count();
    if projected_keys != keys.len() || !has_non_count {
        return None;
    }

    let mut partial_proj = keys.clone();
    partial_proj.extend(partials.iter().map(|(v, _)| v.clone()));

    Some(GroupAggPlan {
        keys: keys.clone(),
        group_inner: group_inner.as_ref().clone(),
        partials,
        partial_proj,
        outputs,
        sum_partials,
    })
}

/// Serialise the per-shard partial query from the plan, reusing the original
/// `FROM`/base IRI. Built from the algebra (not string surgery) so the WHERE
/// round-trips exactly. spargebra serialises `Project{keys+partials, Group{…}}` as an
/// outer pass-through `SELECT` over the grouping subquery, e.g.
/// `SELECT ?t ?__pa0 ?__pa1 FROM <g> WHERE { {SELECT (SUM(?a) AS ?__pa0)
/// (COUNT(?a) AS ?__pa1) ?t WHERE { <inner> } GROUP BY ?t} }` — valid SPARQL whose
/// columns are exactly `partial_proj`.
fn build_partial_query(orig: &Query, plan: &GroupAggPlan) -> Option<String> {
    let Query::Select {
        dataset, base_iri, ..
    } = orig
    else {
        return None;
    };
    let group = GraphPattern::Group {
        inner: Box::new(plan.group_inner.clone()),
        variables: plan.keys.clone(),
        aggregates: plan.partials.clone(),
    };
    let project = GraphPattern::Project {
        inner: Box::new(group),
        variables: plan.partial_proj.clone(),
    };
    let q = Query::Select {
        dataset: dataset.clone(),
        pattern: project,
        base_iri: base_iri.clone(),
    };
    Some(q.to_string())
}

/// Run the partial query on one shard, collecting each solution's values in
/// `partial_proj` (key then partial) column order.
fn run_partial_shard(
    store: &Store,
    sparql: &str,
    proj: &[Variable],
    options: &QueryOptions,
) -> Result<Vec<Vec<Option<Term>>>, String> {
    match store
        .query_opt(sparql, options.clone())
        .map_err(|e| e.to_string())?
    {
        QueryResults::Solutions(sols) => collect_rows(sols, proj),
        _ => Err("group-aggregate partial query did not return solutions".into()),
    }
}

/// True if any summed partial (a `SUM` aggregate or an `AVG`'s sum half) is
/// `xsd:double`/`xsd:float` — values whose cross-shard summation is not bit-exact, so
/// the decomposition must be declined and the caller uses the unsharded copy.
fn rows_have_double(rows: &[Vec<Option<Term>>], plan: &GroupAggPlan) -> bool {
    let sum_idx: Vec<usize> = plan
        .sum_partials
        .iter()
        .filter_map(|sp| plan.partial_proj.iter().position(|v| v == sp))
        .collect();
    for row in rows {
        for &i in &sum_idx {
            if let Some(Term::Literal(lit)) = row.get(i).and_then(|c| c.as_ref()) {
                let dt = lit.datatype().as_str();
                if dt == XSD_DOUBLE || dt == XSD_FLOAT {
                    return true;
                }
            }
        }
    }
    false
}

/// Merge per-shard partial rows by materialising them into a throwaway in-memory
/// store and running the final aggregation ([`build_merge_query`]) over them — so the
/// engine itself does every sum/division/min/max, byte-for-byte as single-store would.
fn merge_group_agg(rows: &[Vec<Option<Term>>], plan: &GroupAggPlan) -> Result<ParAnswer, String> {
    let store = Store::new().map_err(|e| e.to_string())?;
    let mut quads: Vec<Quad> = Vec::with_capacity(rows.len() * plan.partial_proj.len());
    for (ri, row) in rows.iter().enumerate() {
        let subj = BlankNode::new_unchecked(format!("r{ri}"));
        for (ci, cell) in row.iter().enumerate() {
            if let Some(term) = cell {
                quads.push(Quad::new(
                    subj.clone(),
                    NamedNode::new_unchecked(format!("{GACOL}{ci}")),
                    term.clone(),
                    GraphName::DefaultGraph,
                ));
            }
        }
    }
    store
        .bulk_loader()
        .load_quads(quads)
        .map_err(|e| e.to_string())?;
    let merge_sparql = build_merge_query(plan);
    match store.query(&merge_sparql).map_err(|e| e.to_string())? {
        QueryResults::Solutions(sols) => {
            let variables: Vec<Variable> = sols.variables().to_vec();
            let rows = collect_rows(sols, &variables)?;
            Ok(ParAnswer::Solutions { variables, rows })
        }
        _ => Err("group-aggregate merge did not return solutions".into()),
    }
}

/// The final aggregation query over the materialised partials. Each partial row is a
/// blank node with one `<…/gacol/i>` triple per bound column; this groups by the key
/// columns and re-aggregates: `SUM` for COUNT/SUM, `MIN`/`MAX` direct,
/// `SUM(sum)/SUM(cnt)` for AVG. The SELECT uses the original output names and order.
fn build_merge_query(plan: &GroupAggPlan) -> String {
    let where_parts: Vec<String> = plan
        .partial_proj
        .iter()
        .enumerate()
        .map(|(i, v)| format!("<{GACOL}{i}> ?{}", v.as_str()))
        .collect();
    let select_items: Vec<String> = plan
        .outputs
        .iter()
        .map(|o| match o {
            AggCol::Key(v) => format!("?{}", v.as_str()),
            AggCol::SumOf { out, partial } => {
                format!("(SUM(?{}) AS ?{})", partial.as_str(), out.as_str())
            }
            AggCol::MinOf { out, partial } => {
                format!("(MIN(?{}) AS ?{})", partial.as_str(), out.as_str())
            }
            AggCol::MaxOf { out, partial } => {
                format!("(MAX(?{}) AS ?{})", partial.as_str(), out.as_str())
            }
            AggCol::AvgOf { out, sum, cnt } => format!(
                "(SUM(?{}) / SUM(?{}) AS ?{})",
                sum.as_str(),
                cnt.as_str(),
                out.as_str()
            ),
        })
        .collect();
    let group_by: Vec<String> = plan
        .keys
        .iter()
        .map(|v| format!("?{}", v.as_str()))
        .collect();
    format!(
        "SELECT {} WHERE {{ ?r {} . }} GROUP BY {}",
        select_items.join(" "),
        where_parts.join(" ; "),
        group_by.join(" ")
    )
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
        ps.load_quads(persons(50)).unwrap();
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
    fn grouped_avg_decomposes() {
        // AVG per group ≡ SUM/COUNT; the per-shard (SUM, COUNT) merge is byte-
        // identical for xsd:integer/decimal (exact, associative addition). Ages are
        // integers → the AVG is a decimal, exercising integer→decimal division.
        let q = persons(500);
        assert_matches(
            &q,
            "SELECT ?t (AVG(?a) AS ?avg) WHERE { ?s <http://example.org/type> ?t . ?s <http://example.org/age> ?a } GROUP BY ?t",
        );
    }

    #[test]
    fn grouped_sum_min_max_decompose() {
        let q = persons(500);
        for agg in ["SUM", "MIN", "MAX"] {
            assert_matches(
                &q,
                &format!(
                    "SELECT ?t ({agg}(?a) AS ?v) WHERE {{ ?s <http://example.org/type> ?t . ?s <http://example.org/age> ?a }} GROUP BY ?t"
                ),
            );
        }
    }

    #[test]
    fn grouped_mixed_aggregates_decompose() {
        // COUNT + SUM + MIN + MAX + AVG together in one grouped query, all merged.
        let q = persons(500);
        assert_matches(
            &q,
            "SELECT ?t (COUNT(*) AS ?c) (SUM(?a) AS ?s) (MIN(?a) AS ?lo) (MAX(?a) AS ?hi) (AVG(?a) AS ?avg) \
             WHERE { ?s <http://example.org/type> ?t . ?s <http://example.org/age> ?a } GROUP BY ?t",
        );
    }

    #[test]
    fn grouped_avg_sum_decimal_decompose() {
        // xsd:decimal measure: AVG/SUM over decimals must stay byte-identical (exact
        // fixed-point addition is associative, so the cross-shard merge matches).
        let ex = "http://example.org/";
        let mut quads = Vec::new();
        for i in 0..300usize {
            let s = Subject::NamedNode(iri(&format!("{ex}p{i}")));
            quads.push(Quad::new(
                s.clone(),
                iri(&format!("{ex}type")),
                Term::NamedNode(iri(&format!("{ex}T{}", i % 7))),
                GraphName::DefaultGraph,
            ));
            quads.push(Quad::new(
                s,
                iri(&format!("{ex}score")),
                Term::Literal(Literal::new_typed_literal(
                    format!("{}.{:02}", i % 50, i % 100),
                    oxrdf::vocab::xsd::DECIMAL,
                )),
                GraphName::DefaultGraph,
            ));
        }
        assert_matches(
            &quads,
            "SELECT ?t (AVG(?v) AS ?a) (SUM(?v) AS ?s) WHERE { ?x <http://example.org/type> ?t . ?x <http://example.org/score> ?v } GROUP BY ?t",
        );
    }

    #[test]
    fn grouped_min_max_strings_decompose() {
        // MIN/MAX are order-independent (min-of-mins) and pass the term through, so
        // they decompose for non-numeric types too.
        let ex = "http://example.org/";
        let mut quads = Vec::new();
        for i in 0..200usize {
            let s = Subject::NamedNode(iri(&format!("{ex}p{i}")));
            quads.push(Quad::new(
                s.clone(),
                iri(&format!("{ex}type")),
                Term::NamedNode(iri(&format!("{ex}T{}", i % 5))),
                GraphName::DefaultGraph,
            ));
            quads.push(Quad::new(
                s,
                iri(&format!("{ex}label")),
                Term::Literal(Literal::new_simple_literal(format!(
                    "label-{:04}",
                    (i * 7) % 200
                ))),
                GraphName::DefaultGraph,
            ));
        }
        assert_matches(
            &quads,
            "SELECT ?t (MIN(?l) AS ?lo) (MAX(?l) AS ?hi) WHERE { ?x <http://example.org/type> ?t . ?x <http://example.org/label> ?l } GROUP BY ?t",
        );
    }

    #[test]
    fn grouped_double_sum_avg_declined_minmax_ok() {
        // xsd:double SUM is IEEE-754 non-associative across shards, so SUM/AVG must be
        // DECLINED at runtime (→ None; the caller uses the unsharded copy). MIN/MAX
        // over doubles is order-independent and still decomposes.
        let ex = "http://example.org/";
        let mut quads = Vec::new();
        for i in 0..200usize {
            let s = Subject::NamedNode(iri(&format!("{ex}p{i}")));
            quads.push(Quad::new(
                s.clone(),
                iri(&format!("{ex}type")),
                Term::NamedNode(iri(&format!("{ex}T{}", i % 5))),
                GraphName::DefaultGraph,
            ));
            quads.push(Quad::new(
                s,
                iri(&format!("{ex}m")),
                Term::Literal(Literal::new_typed_literal(
                    format!("{}.5e0", i),
                    oxrdf::vocab::xsd::DOUBLE,
                )),
                GraphName::DefaultGraph,
            ));
        }
        let ps = ParallelStore::new(4);
        ps.load_quads(quads.iter().cloned()).unwrap();
        for agg in ["AVG", "SUM"] {
            assert!(
                ps.query(&format!(
                    "SELECT ?t ({agg}(?v) AS ?a) WHERE {{ ?x <http://example.org/type> ?t . ?x <http://example.org/m> ?v }} GROUP BY ?t"
                ))
                .unwrap()
                .is_none(),
                "{agg} over xsd:double must be declined (IEEE-754 non-associative)"
            );
        }
        // MIN/MAX over the same doubles must still decompose, exactly.
        assert_matches(
            &quads,
            "SELECT ?t (MIN(?v) AS ?lo) (MAX(?v) AS ?hi) WHERE { ?x <http://example.org/type> ?t . ?x <http://example.org/m> ?v } GROUP BY ?t",
        );
    }

    #[test]
    fn grouped_distinct_count_is_rejected() {
        // A grouped COUNT(DISTINCT) is not sum-safe across shards (a value can recur
        // in several shards), so it stays single-store.
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
