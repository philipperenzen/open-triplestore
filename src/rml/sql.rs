//! The relational RML executor: a registered datasource as a logical source.
//!
//! Rows are streamed in batches and never materialised whole. Triples are
//! accumulated as N-Triples lines and flushed into the run's graph per batch,
//! so peak memory is one batch of rows plus one buffer of text.
//!
//! **Joins.** `rr:parentTriplesMap` is resolved one of two ways, decided per
//! reference by [`plan_triples_map`]:
//!
//! * **Pushdown** — the parent's join columns cover a unique key of the parent
//!   table, so a `LEFT JOIN` matches at most one parent row and cannot
//!   duplicate a child row. The parent's subject columns are projected onto the
//!   child's own query and the object is built straight from the child row.
//!   No second scan of the parent, and no memory held for it.
//! * **Hash index** — everything else: the parent's logical source is streamed
//!   once and each row's join-key values are indexed to the subject term that
//!   map generates; the child then streams and looks up. The index is bounded
//!   (`OTS_SOURCES_JOIN_MAX_ROWS`, default 1 000 000 keys) and a mapping that
//!   would exceed it is refused by name rather than exhausting memory.
//!
//! The unique-key condition is what makes the two interchangeable. Pushing a
//! join down on a NON-unique parent key would multiply the child row, which
//! changes the row count, inflates the triple count, and re-emits every one of
//! the child's own predicate-object maps per match. The index has no such
//! effect, so it stays the fallback rather than a legacy path.
//!
//! The index is built by [`ParentIndexBuilder`], which takes rows from any
//! source — the file executor joins CSV, JSON and XML sources with it too.
//!
//! **No join condition.** A referencing object map without one is legal only
//! when the child and the parent read the same logical source, and then it
//! joins each row to *itself* (R2RML §8: the joint query is
//! `SELECT * FROM ({child-query}) AS tmp`): the object is the subject the
//! parent map generates for the child's own row ([`same_row_subject`]). It is
//! not a cross join over every parent row.

use std::collections::HashMap;

use ots_plugin_api::sources::{Row as SourceRow, SourceConnection, SourceError};

use super::checks::{check_columns, DataErrors, OnDataError};
use super::model::*;
use super::parser::DEFAULT_GRAPH;
use super::terms::{
    eval_function, eval_iri, eval_parent_subject, eval_term, position_independent, At, Kinds, Row,
    TermGen,
};
use crate::store::engine::TripleStore;

/// Flush the N-Triples buffer once it exceeds this many bytes.
const FLUSH_BYTES: usize = 4 * 1024 * 1024;
const JOIN_MAX_ROWS_ENV: &str = "OTS_SOURCES_JOIN_MAX_ROWS";

fn join_max_rows() -> usize {
    std::env::var(JOIN_MAX_ROWS_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1_000_000)
}

/// One generated triple, with the named graph it belongs in (`None` = the
/// caller's target graph).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmittedTriple {
    pub graph: Option<String>,
    pub text: String,
}

/// How many rows were read and how many triples they produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SqlOutcome {
    pub rows: u64,
    pub triples: u64,
    /// Rows the run skipped terms from; empty unless it ran with
    /// [`OnDataError::Skip`] (an aborting run fails instead).
    pub data_errors: DataErrors,
}

/// Generate one row's triples for one triples map (R2RML §11.1).
///
/// `resolve_ref` answers a `rr:parentTriplesMap` object: the subject terms the
/// parent map generates for the rows this row joins to, given where the
/// triple lands (a blank node is scoped to its graph). `None` means the
/// caller has no index for it, and the reference is skipped. A parent's
/// subject reached through an index is rendered for the default graph.
///
/// **Graphs.** A triple goes to every graph its subject map names, *and* every
/// graph its predicate-object map names — the union, not an override — and to
/// the default graph when neither names one. `rr:class` triples go to the
/// subject's graphs. A graph map that generates `rr:defaultGraph` names the
/// default graph. In the result, `None` is the default graph: the caller's
/// target graph.
pub fn row_triples(
    tm: &TriplesMap,
    row: &Row,
    kinds: Option<&Kinds>,
    gen: &mut TermGen,
    base: Option<&str>,
    resolve_ref: &dyn Fn(&RefObjectMap, &Row, At<'_>) -> Option<Vec<String>>,
) -> Result<Vec<EmittedTriple>, String> {
    let mut out = Vec::new();
    // The subject, once per graph it lands in: a blank node is scoped to its
    // graph (§9.1), so the same row can yield a different node in each.
    let mut subjects: Vec<(Option<String>, String)> = Vec::new();
    let mut subject_in =
        |graph: &Option<String>, gen: &mut TermGen| -> Result<Option<String>, String> {
            if let Some((_, s)) = subjects.iter().find(|(g, _)| g == graph) {
                return Ok(Some(s.clone()));
            }
            let at = At {
                base,
                graph: graph.as_deref(),
            };
            let s = eval_subject(&tm.subject_map, row, kinds, gen, at)?;
            if let Some(s) = &s {
                subjects.push((graph.clone(), s.clone()));
            }
            Ok(s)
        };

    if subject_in(&None, gen)?.is_none() {
        // No subject — R2RML says the row generates nothing at all.
        return Ok(out);
    }

    let subject_graphs = eval_graphs(&tm.subject_map.graph_maps, row, kinds, gen, base);
    let class_graphs = if tm.subject_map.graph_maps.is_empty() {
        vec![None]
    } else {
        subject_graphs.clone()
    };
    for graph in &class_graphs {
        let Some(subject) = subject_in(graph, gen)? else {
            continue;
        };
        for class_iri in &tm.subject_map.classes {
            out.push(EmittedTriple {
                graph: graph.clone(),
                text: format!(
                    "{subject} <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{class_iri}> ."
                ),
            });
        }
    }

    for pom in &tm.predicate_object_maps {
        // Every predicate × every object (R2RML §11.1). A predicate map that
        // generates no term for the row contributes no triples.
        let predicates: Vec<String> = pom
            .predicate_maps
            .iter()
            .filter_map(|pm| eval_term(pm, row, kinds, gen, At { base, graph: None }))
            .collect();
        if predicates.is_empty() {
            continue;
        }
        let graphs = if tm.subject_map.graph_maps.is_empty() && pom.graph_maps.is_empty() {
            vec![None]
        } else {
            let mut union = subject_graphs.clone();
            for g in eval_graphs(&pom.graph_maps, row, kinds, gen, base) {
                if !union.contains(&g) {
                    union.push(g);
                }
            }
            union
        };
        for graph in &graphs {
            let Some(subject) = subject_in(graph, gen)? else {
                continue;
            };
            let at = At {
                base,
                graph: graph.as_deref(),
            };
            let mut objects: Vec<String> = Vec::new();
            for object_map in &pom.object_maps {
                match object_map {
                    ObjectMap::Term(tm_obj) => {
                        objects.extend(eval_term(tm_obj, row, kinds, gen, at))
                    }
                    ObjectMap::Function(f) => objects.extend(eval_function(f, row, kinds, gen)?),
                    ObjectMap::Ref(r) => {
                        objects.extend(resolve_ref(r, row, at).unwrap_or_default())
                    }
                }
            }
            for predicate in &predicates {
                for object in &objects {
                    out.push(EmittedTriple {
                        graph: graph.clone(),
                        text: format!("{subject} {predicate} {object} ."),
                    });
                }
            }
        }
    }

    Ok(out)
}

/// The graphs a list of graph maps names for a row, without repeats. `None`
/// is the default graph (`rr:defaultGraph`). A graph map that generates no
/// term for the row contributes no graph.
fn eval_graphs(
    maps: &[TermMap],
    row: &Row,
    kinds: Option<&Kinds>,
    gen: &mut TermGen,
    base: Option<&str>,
) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = Vec::new();
    for gm in maps {
        let Some(iri) = eval_iri(gm, row, kinds, gen, base) else {
            continue;
        };
        let graph = (iri != DEFAULT_GRAPH).then_some(iri);
        if !out.contains(&graph) {
            out.push(graph);
        }
    }
    out
}

/// The subject a subject map generates for a row: its function when it has
/// one, its term map otherwise. `Ok(None)` is a row that generates nothing.
pub(crate) fn eval_subject(
    sm: &SubjectMap,
    row: &Row,
    kinds: Option<&Kinds>,
    gen: &mut TermGen,
    at: At<'_>,
) -> Result<Option<String>, String> {
    match &sm.function {
        Some(f) => eval_function(f, row, kinds, gen),
        None => Ok(eval_term(&sm.term_map, row, kinds, gen, at)),
    }
}

/// Split a connector row into the lexical values and the per-column generic
/// types the natural-datatype rule needs.
pub(crate) fn split_row(src: &SourceRow, row: &mut Row, kinds: &mut Kinds) {
    row.clear();
    kinds.clear();
    for (name, value) in src {
        row.insert(name.clone(), value.lexical.clone());
        kinds.insert(name.clone(), value.kind);
    }
}

/// The join key for one row: the values of `columns`, in order. `None` when
/// any of them is NULL — SQL join semantics, so a NULL never matches.
pub(crate) fn join_key(row: &Row, columns: &[String]) -> Option<Vec<String>> {
    columns.iter().map(|c| row.get(c).cloned()).collect()
}

pub(crate) type ParentIndex = HashMap<Vec<String>, Vec<String>>;

/// Columns a pushed-down join projects from the parent carry this prefix, and
/// the child subquery carries it as its alias. Reserved: a source column whose
/// name starts with it would be shadowed.
const PUSHDOWN_PREFIX: &str = "__ots_j";

/// How one `rr:parentTriplesMap` reference is resolved. See the module docs for
/// why the choice is not free.
#[derive(Debug, Clone)]
pub(crate) enum JoinStrategy {
    /// Resolved from columns carried on the child row, under `alias`.
    Pushdown {
        alias: String,
        /// The parent's join columns, in order — present on the child row only
        /// when a parent row actually matched, which is how a miss is detected
        /// even for a parent whose subject is a constant.
        witness: Vec<String>,
    },
    /// Resolved through a pre-built index of the parent's subject terms.
    Index,
    /// No join condition: the parent's subject for the child's own row
    /// ([`same_row_subject`]). Needs neither an index nor a second scan.
    SameRow,
}

/// Restrict a triples map's own rows to those past a cursor.
///
/// Applied to the child source only. A join parent must stay fully visible: a
/// parent row older than the cursor is still the correct object for a child row
/// newer than it, and filtering the parent would silently drop the join.
#[derive(Debug, Clone)]
pub struct RowFilter {
    pub column: String,
    /// Exclusive lower bound, as a SQL literal value.
    pub greater_than: String,
    /// Tables known to carry the column. A triples map reading anything else is
    /// left unfiltered rather than failing on a column the table has not got.
    pub tables: std::collections::HashSet<String>,
}

impl RowFilter {
    /// Whether this filter can be applied to `source` at all.
    fn applies_to(&self, source: &LogicalSource) -> bool {
        source
            .table_name
            .as_deref()
            .is_some_and(|t| source.query.is_none() && self.tables.contains(t))
    }

    /// A SQL string literal. A cursor is a `MAX()` of a timestamp or id column,
    /// so anything outside that shape is refused rather than escaped and hoped
    /// for: the connector takes a statement, not parameters, and a value that
    /// needed real escaping would mean the cursor column was not what we think.
    fn literal(&self) -> Result<String, String> {
        let ok = |c: char| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | ':' | '.' | ' ' | '+' | '_' | '/')
        };
        if self.greater_than.is_empty() || !self.greater_than.chars().all(ok) {
            return Err(format!(
                "the watermark value is not a plain timestamp or identifier, so it cannot be \
                 used as a bound; column '{}' is not usable as a watermark",
                self.column
            ));
        }
        Ok(format!("'{}'", self.greater_than))
    }
}

/// One triples map's execution plan: the SQL to stream, and how each of its
/// references resolves.
pub(crate) struct TmPlan {
    pub(crate) sql: String,
    pub(crate) strategies: HashMap<RefKey, JoinStrategy>,
}

pub(crate) type RefKey = (String, Vec<(String, String)>);

/// Whether a reference can be pushed into the child's query.
///
/// Four conditions, all necessary:
/// * there is at least one join condition — a join-less reference is
///   resolved from the child's own row ([`JoinStrategy::SameRow`]);
/// * the parent reads a named, unqualified table, so its keys can be looked up
///   at all (an `rml:query` parent is opaque to the catalogue);
/// * the parent's join columns cover one of that table's unique keys, so the
///   join cannot duplicate a child row;
/// * the parent's subject does not depend on which row computes it: an IRI,
///   or an R2RML blank node (a function of its value). A legacy blank node is
///   minted once per PARENT row; pushed down it would be minted once per child
///   row, so two children of one parent would stop sharing a node.
fn can_push_down(
    parent: &TriplesMap,
    joins: &[JoinCondition],
    unique_keys: &HashMap<String, Vec<Vec<String>>>,
    semantics: Semantics,
) -> bool {
    if joins.is_empty()
        || parent.subject_map.function.is_some()
        || !position_independent(&parent.subject_map.term_map, semantics)
    {
        return false;
    }
    let Some(table) = parent.logical_source.catalogue_table() else {
        return false;
    };
    let parent_cols: Vec<&str> = joins.iter().map(|j| j.parent.as_str()).collect();
    unique_keys.get(&table).is_some_and(|keys| {
        keys.iter()
            .any(|k| k.iter().all(|c| parent_cols.contains(&c.as_str())))
    })
}

/// Build one triples map's plan: which references push down, and the SQL that
/// carries them.
pub(crate) fn plan_triples_map(
    tm: &TriplesMap,
    mapping: &RmlMapping,
    unique_keys: &HashMap<String, Vec<Vec<String>>>,
    quote: &dyn Fn(&str) -> String,
    filter: Option<&RowFilter>,
) -> Result<TmPlan, String> {
    let mut child_sql = tm
        .logical_source
        .sql(quote)
        .ok_or_else(|| format!("TriplesMap <{}> has no relational logical source", tm.iri))?;

    if let Some(f) = filter.filter(|f| f.applies_to(&tm.logical_source)) {
        child_sql = format!(
            "SELECT * FROM ({child_sql}) {alias} WHERE {alias}.{col} > {bound}",
            alias = quote(WATERMARK_ALIAS),
            col = quote(&f.column),
            bound = f.literal()?,
        );
    }

    let mut strategies: HashMap<RefKey, JoinStrategy> = HashMap::new();
    let mut projected: Vec<String> = Vec::new();
    let mut clauses: Vec<String> = Vec::new();

    for r in tm.refs() {
        let key = index_key(r);
        if strategies.contains_key(&key) {
            continue;
        }
        let parent = mapping
            .find(&r.parent_triples_map)
            .ok_or_else(|| format!("unknown parent TriplesMap <{}>", r.parent_triples_map))?;
        if r.joins.is_empty() {
            strategies.insert(key, JoinStrategy::SameRow);
            continue;
        }
        if !can_push_down(parent, &r.joins, unique_keys, mapping.semantics) {
            strategies.insert(key, JoinStrategy::Index);
            continue;
        }

        let alias = format!("{PUSHDOWN_PREFIX}{}", clauses.len());
        let witness: Vec<String> = r.joins.iter().map(|j| j.parent.clone()).collect();
        // The subject's columns build the term; the join columns prove a parent
        // row matched at all.
        let mut columns = witness.clone();
        for c in parent.subject_map.term_map.referenced_columns() {
            if !columns.contains(&c) {
                columns.push(c);
            }
        }
        for c in &columns {
            projected.push(format!(
                ", {}.{} AS {}",
                quote(&alias),
                quote(c),
                quote(&format!("{alias}_{c}"))
            ));
        }
        let parent_sql = parent
            .logical_source
            .sql(quote)
            .ok_or_else(|| format!("join parent <{}> is not a relational source", parent.iri))?;
        let on = r
            .joins
            .iter()
            .map(|j| {
                format!(
                    "{}.{} = {}.{}",
                    quote(PUSHDOWN_CHILD),
                    quote(&j.child),
                    quote(&alias),
                    quote(&j.parent)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        clauses.push(format!(
            " LEFT JOIN ({parent_sql}) {} ON {on}",
            quote(&alias)
        ));
        strategies.insert(key, JoinStrategy::Pushdown { alias, witness });
    }

    let sql = if clauses.is_empty() {
        child_sql
    } else {
        format!(
            "SELECT {}.*{} FROM ({child_sql}) {}{}",
            quote(PUSHDOWN_CHILD),
            projected.concat(),
            quote(PUSHDOWN_CHILD),
            clauses.concat()
        )
    };
    Ok(TmPlan { sql, strategies })
}

/// The alias the child's own logical source carries in a pushed-down query.
const PUSHDOWN_CHILD: &str = "__ots_c";
/// The alias the child's source carries when bounded by a cursor.
const WATERMARK_ALIAS: &str = "__ots_w";

/// Rebuild the parent's row from the columns a pushed-down join carried along,
/// then evaluate the parent's subject from it. `None` when no parent row
/// matched, which is why the join columns are always projected.
pub(crate) fn pushdown_subject(
    parent: &TriplesMap,
    alias: &str,
    witness: &[String],
    child_row: &Row,
    gen: &mut TermGen,
    base: Option<&str>,
) -> Option<String> {
    let prefix = format!("{alias}_");
    if !witness
        .iter()
        .all(|c| child_row.contains_key(&format!("{prefix}{c}")))
    {
        return None;
    }
    let mut parent_row = Row::with_capacity(witness.len() + 2);
    for (name, value) in child_row {
        if let Some(col) = name.strip_prefix(prefix.as_str()) {
            parent_row.insert(col.to_string(), value.clone());
        }
    }
    // The parent's own `rml:null` values: a join key that is NULL there
    // matches nothing, as it would through the index.
    gen.apply_nulls(&parent.logical_source, &mut parent_row);
    if !witness.iter().all(|c| parent_row.contains_key(c)) {
        return None;
    }
    // Kinds are irrelevant: the subject is an IRI or a blank node, so no
    // natural datatype applies (`can_push_down` guarantees the term type).
    eval_parent_subject(&parent.subject_map.term_map, &parent_row, None, gen, base)
}

/// Builds the index a join resolves through: each parent row's join-key
/// values, mapped to the subject terms its triples map generates for it.
///
/// It takes rows one at a time from any source — a relational stream, a
/// sample held in memory, a parsed CSV, JSON or XML file — so every executor
/// joins by the same rules: the parent's own `rml:null` values apply, a key
/// with a NULL in it matches nothing, a row that generates no subject is not
/// indexed, and the number of distinct keys is bounded by
/// `OTS_SOURCES_JOIN_MAX_ROWS` (default 1 000 000), past which the mapping is
/// refused by name rather than exhausting memory.
pub(crate) struct ParentIndexBuilder<'a> {
    parent: &'a TriplesMap,
    columns: Vec<String>,
    base: Option<&'a str>,
    cap: usize,
    index: ParentIndex,
}

impl<'a> ParentIndexBuilder<'a> {
    pub(crate) fn new(parent: &'a TriplesMap, joins: &[JoinCondition], base: Option<&'a str>) -> Self {
        Self {
            parent,
            columns: joins.iter().map(|j| j.parent.clone()).collect(),
            base,
            cap: join_max_rows(),
            index: ParentIndex::new(),
        }
    }

    /// Index one parent row. `row` is the row as the source delivered it;
    /// the parent's `rml:null` values are dropped from it here.
    pub(crate) fn push(
        &mut self,
        row: &mut Row,
        kinds: Option<&Kinds>,
        gen: &mut TermGen,
    ) -> Result<(), String> {
        gen.apply_nulls(&self.parent.logical_source, row);
        let Some(key) = join_key(row, &self.columns) else {
            return Ok(());
        };
        gen.start_row();
        let at = At {
            base: self.base,
            graph: None,
        };
        let Some(subject) = eval_subject(&self.parent.subject_map, row, kinds, gen, at)? else {
            return Ok(());
        };
        if self.index.len() >= self.cap && !self.index.contains_key(&key) {
            return Err(format!(
                "the join index for parent TriplesMap <{}> exceeded {} distinct keys; \
                 raise {JOIN_MAX_ROWS_ENV} or narrow the parent's logical source",
                self.parent.iri, self.cap
            ));
        }
        let entry = self.index.entry(key).or_default();
        if !entry.contains(&subject) {
            entry.push(subject);
        }
        Ok(())
    }

    pub(crate) fn finish(self) -> ParentIndex {
        self.index
    }
}

/// Look a child row up in a parent index: the parent subjects whose join key
/// equals the child's. `None` when a child join column is NULL.
pub(crate) fn lookup(index: &ParentIndex, r: &RefObjectMap, child_row: &Row) -> Option<Vec<String>> {
    let child_columns: Vec<String> = r.joins.iter().map(|j| j.child.clone()).collect();
    index.get(&join_key(child_row, &child_columns)?).cloned()
}

/// The object of a referencing object map with no join condition: the
/// subject the parent map generates for the child's *own* row (R2RML §8; the
/// parser has checked that both maps read the same logical source). The
/// parent's `rml:null` values are its source's, which is the child's.
///
/// `gen` must not be the child row's own generator: under R2RML a blank node
/// is a function of its value and graph, so any generator labels it the same;
/// a legacy blank node is minted per row and does not survive the hop either
/// way.
pub(crate) fn same_row_subject(
    mapping: &RmlMapping,
    r: &RefObjectMap,
    row: &Row,
    gen: &mut TermGen,
    at: At<'_>,
) -> Option<Vec<String>> {
    let parent = mapping.find(&r.parent_triples_map)?;
    let at = At {
        base: mapping.base_for(parent),
        graph: at.graph,
    };
    // Kinds are irrelevant: a subject is an IRI or a blank node.
    match eval_subject(&parent.subject_map, row, None, gen, at) {
        Ok(Some(s)) => Some(vec![s]),
        _ => None,
    }
}

/// Stream a parent triples map once and index its subject terms by join key.
fn build_parent_index(
    parent: &TriplesMap,
    joins: &[JoinCondition],
    conn: &mut dyn SourceConnection,
    quote: &dyn Fn(&str) -> String,
    gen: &mut TermGen,
    base: Option<&str>,
) -> Result<ParentIndex, String> {
    let sql = parent.logical_source.sql(quote).ok_or_else(|| {
        format!(
            "TriplesMap <{}> is used as a join parent but is not a relational source",
            parent.iri
        )
    })?;
    let mut builder = ParentIndexBuilder::new(parent, joins, base);
    let mut row: Row = HashMap::new();
    let mut kinds: Kinds = HashMap::new();
    let mut failure: Option<String> = None;

    conn.stream(&sql, 1_000, &mut |batch| {
        for src in &batch {
            split_row(src, &mut row, &mut kinds);
            if let Err(e) = builder.push(&mut row, Some(&kinds), gen) {
                failure = Some(e);
                return Err(SourceError::Query("join index".into()));
            }
        }
        Ok(())
    })
    .map_err(|e| {
        failure
            .clone()
            .unwrap_or_else(|| format!("reading join parent <{}>: {e}", parent.iri))
    })?;

    Ok(builder.finish())
}

/// Every parent table a reference joins to, with its unique keys — the input
/// the planner needs. Costs one cheap catalogue lookup per distinct parent
/// table, and nothing at all for a mapping with no joins.
pub(crate) fn collect_unique_keys(
    mapping: &RmlMapping,
    conn: &mut dyn SourceConnection,
) -> Result<HashMap<String, Vec<Vec<String>>>, String> {
    let mut tables: Vec<String> = Vec::new();
    for tm in &mapping.triples_maps {
        for r in tm.refs() {
            let Some(parent) = mapping.find(&r.parent_triples_map) else {
                continue;
            };
            if let Some(t) = parent.logical_source.catalogue_table() {
                if !tables.contains(&t) {
                    tables.push(t);
                }
            }
        }
    }
    let mut out = HashMap::new();
    for table in tables {
        // A catalogue that will not answer is not fatal: the planner simply
        // finds no unique key and falls back to the index, which is correct
        // for every mapping — just slower.
        match conn.unique_keys(&table) {
            Ok(keys) => {
                out.insert(table, keys);
            }
            Err(e) => tracing::debug!(
                table,
                "unique-key lookup failed, joins will be indexed: {e}"
            ),
        }
    }
    Ok(out)
}

/// Run a relational mapping into `target_graph`.
///
/// Every triple lands in `target_graph`: a run's graph is the unit the SHACL
/// gate validates and the role swap promotes, so it has to hold the whole
/// result. Mappings that declare `rr:graphMap` are refused at registration
/// rather than silently re-routed.
#[allow(clippy::too_many_arguments)]
pub fn execute_relational(
    mapping: &RmlMapping,
    conn: &mut dyn SourceConnection,
    quote: &dyn Fn(&str) -> String,
    store: &TripleStore,
    target_graph: &str,
    batch_size: usize,
    run_id: &str,
    on_data_error: OnDataError,
) -> Result<SqlOutcome, String> {
    execute_relational_filtered(
        mapping,
        conn,
        quote,
        store,
        target_graph,
        batch_size,
        run_id,
        None,
        on_data_error,
    )
}

/// [`execute_relational`], restricted to rows past a cursor, and with a
/// choice of what a data error does.
///
/// Before the first row, every triples map's columns are checked against
/// what its query returns (when the connector can say): a column the mapping
/// names that the source lacks is a mapping error. A row value that cannot
/// become its term (R2RML §4.3) fails the run with the offending rows named
/// — after reading on to name up to [`super::checks::DATA_ERROR_SAMPLE`] of
/// them — unless `on_data_error` is [`OnDataError::Skip`]. A failed run's
/// partial graph is the caller's to drop.
#[allow(clippy::too_many_arguments)]
pub fn execute_relational_filtered(
    mapping: &RmlMapping,
    conn: &mut dyn SourceConnection,
    quote: &dyn Fn(&str) -> String,
    store: &TripleStore,
    target_graph: &str,
    batch_size: usize,
    run_id: &str,
    filter: Option<&RowFilter>,
    on_data_error: OnDataError,
) -> Result<SqlOutcome, String> {
    run_relational(
        mapping,
        conn,
        quote,
        store,
        Target::Into(target_graph),
        batch_size,
        run_id,
        filter,
        on_data_error,
    )
}

/// Run a relational mapping into `store` with every triple in the graphs its
/// graph maps name, and a triple no graph map routes in the default graph —
/// the output dataset R2RML §11 defines. What a conformance run compares; a
/// registered mapping runs through [`execute_relational`] instead, whose run
/// graph has to hold the whole result.
pub fn execute_relational_as_mapped(
    mapping: &RmlMapping,
    conn: &mut dyn SourceConnection,
    quote: &dyn Fn(&str) -> String,
    store: &TripleStore,
    batch_size: usize,
    run_id: &str,
    on_data_error: OnDataError,
) -> Result<SqlOutcome, String> {
    run_relational(
        mapping,
        conn,
        quote,
        store,
        Target::AsMapped,
        batch_size,
        run_id,
        None,
        on_data_error,
    )
}

/// Where a relational run writes.
#[derive(Debug, Clone, Copy)]
enum Target<'a> {
    /// Every triple into this graph, whatever its graph maps say.
    Into(&'a str),
    /// Every triple into the graphs it was generated for.
    AsMapped,
}

/// Generated N-Triples lines, by the graph they go to.
#[derive(Default)]
struct Buffers {
    /// The default graph's lines — every line, for [`Target::Into`].
    default: String,
    named: HashMap<String, String>,
    bytes: usize,
}

impl Buffers {
    fn push(&mut self, target: Target<'_>, t: &EmittedTriple) {
        let buf = match (target, &t.graph) {
            (Target::AsMapped, Some(g)) => self.named.entry(g.clone()).or_default(),
            _ => &mut self.default,
        };
        buf.push_str(&t.text);
        buf.push('\n');
        self.bytes += t.text.len() + 1;
    }

    fn clear(&mut self) {
        self.default.clear();
        self.named.clear();
        self.bytes = 0;
    }

    fn flush(&mut self, store: &TripleStore, target: Target<'_>) -> Result<(), String> {
        match target {
            Target::Into(g) => flush(store, &mut self.default, g)?,
            Target::AsMapped => {
                if !self.default.is_empty() {
                    store
                        .load_str(&self.default, oxigraph::io::RdfFormat::NTriples, None)
                        .map_err(|e| format!("writing generated triples: {e}"))?;
                    self.default.clear();
                }
            }
        }
        for (graph, buf) in self.named.iter_mut() {
            flush(store, buf, graph)?;
        }
        self.bytes = 0;
        Ok(())
    }
}

/// The body of [`execute_relational_filtered`] and [`execute_relational_as_mapped`].
#[allow(clippy::too_many_arguments)]
fn run_relational(
    mapping: &RmlMapping,
    conn: &mut dyn SourceConnection,
    quote: &dyn Fn(&str) -> String,
    store: &TripleStore,
    target: Target<'_>,
    batch_size: usize,
    run_id: &str,
    filter: Option<&RowFilter>,
    on_data_error: OnDataError,
) -> Result<SqlOutcome, String> {
    let batch_size = batch_size.clamp(1, 100_000);
    // Blank-node labels carry the run id, so two runs' graphs never share a
    // node and a batch-by-batch load never merges rows.
    let mut gen = TermGen::new(mapping.semantics, format!("r{}_", sanitise_label(run_id)));
    let mut outcome = SqlOutcome::default();

    check_relational_columns(mapping, conn, quote)?;
    let unique_keys = collect_unique_keys(mapping, conn)?;
    let mut plans: HashMap<String, TmPlan> = HashMap::new();
    for tm in &mapping.triples_maps {
        plans.insert(
            tm.iri.clone(),
            plan_triples_map(tm, mapping, &unique_keys, quote, filter)?,
        );
    }

    // Index only what did not push down. A mapping whose joins all pushed down
    // scans each source exactly once.
    let mut indexes: HashMap<RefKey, ParentIndex> = HashMap::new();
    for tm in &mapping.triples_maps {
        for r in tm.refs() {
            let key = index_key(r);
            if indexes.contains_key(&key)
                || !matches!(
                    plans[&tm.iri].strategies.get(&key),
                    Some(JoinStrategy::Index)
                )
            {
                continue;
            }
            let parent = mapping
                .find(&r.parent_triples_map)
                .ok_or_else(|| format!("unknown parent TriplesMap <{}>", r.parent_triples_map))?;
            let index = build_parent_index(
                parent,
                &r.joins,
                conn,
                quote,
                &mut gen,
                mapping.base_for(parent),
            )?;
            indexes.insert(key, index);
            // A parent row's data error is reported by the parent's own scan.
            gen.take_errors();
        }
    }
    tracing::debug!(
        pushed_down = plans
            .values()
            .flat_map(|p| p.strategies.values())
            .filter(|s| matches!(s, JoinStrategy::Pushdown { .. }))
            .count(),
        indexed = indexes.len(),
        "relational join plan"
    );

    let mut buffer = Buffers::default();
    let mut row: Row = HashMap::new();
    let mut kinds: Kinds = HashMap::new();

    // A pushed-down parent subject is computed from the child row, with its
    // own generator: the term does not depend on it (`can_push_down`).
    let parent_gen = std::cell::RefCell::new(TermGen::new(
        mapping.semantics,
        format!("r{}_", sanitise_label(run_id)),
    ));
    for tm in &mapping.triples_maps {
        let plan = &plans[&tm.iri];
        let base = mapping.base_for(tm);
        let mut emit_error: Option<String> = None;
        let mut triples: u64 = 0;
        let mut seen: u64 = 0;
        let mut errors = DataErrors::default();
        let mut aborting = false;
        let rows = conn.stream(&plan.sql, batch_size, &mut |batch| {
            for src in &batch {
                split_row(src, &mut row, &mut kinds);
                apply_own_nulls(&gen, &tm.logical_source, &mut row);
                seen += 1;
                gen.start_row();
                let generated = row_triples(
                    tm,
                    &row,
                    Some(&kinds),
                    &mut gen,
                    base,
                    &|r, child_row, at| {
                        let key = index_key(r);
                        match plan.strategies.get(&key) {
                            Some(JoinStrategy::Pushdown { alias, witness }) => {
                                let parent = mapping.find(&r.parent_triples_map)?;
                                pushdown_subject(
                                    parent,
                                    alias,
                                    witness,
                                    child_row,
                                    &mut parent_gen.borrow_mut(),
                                    mapping.base_for(parent),
                                )
                                .map(|s| vec![s])
                            }
                            Some(JoinStrategy::SameRow) => same_row_subject(
                                mapping,
                                r,
                                child_row,
                                &mut parent_gen.borrow_mut(),
                                at,
                            ),
                            _ => lookup(indexes.get(&key)?, r, child_row),
                        }
                    },
                );
                let generated = match generated {
                    Ok(g) => g,
                    Err(e) => {
                        emit_error = Some(e);
                        return Err(SourceError::Query("mapping error".into()));
                    }
                };
                parent_gen.borrow_mut().take_errors();
                errors.record(&tm.iri, seen, gen.take_errors());
                if on_data_error == OnDataError::Abort && !errors.is_empty() {
                    // Nothing more is written; read on only to name a few
                    // more offending rows.
                    aborting = true;
                    buffer.clear();
                    if errors.sample_full() {
                        return Err(SourceError::Query("data error".into()));
                    }
                    continue;
                }
                for t in generated {
                    buffer.push(target, &t);
                    triples += 1;
                }
            }
            if !aborting && buffer.bytes >= FLUSH_BYTES {
                if let Err(e) = buffer.flush(store, target) {
                    emit_error = Some(e);
                    return Err(SourceError::Query("write error".into()));
                }
            }
            Ok(())
        });
        if aborting {
            return Err(errors.abort_message());
        }
        let rows = rows.map_err(|e| {
            emit_error
                .clone()
                .unwrap_or_else(|| format!("reading <{}>: {e}", tm.iri))
        })?;
        if let Some(e) = emit_error {
            return Err(e);
        }
        outcome.rows += rows;
        outcome.triples += triples;
        outcome.data_errors.merge(errors);
    }

    buffer.flush(store, target)?;
    Ok(outcome)
}

/// Drop the child row's own `rml:null` values, leaving the columns a
/// pushed-down join carried along to the parent's rules
/// ([`pushdown_subject`]).
pub(crate) fn apply_own_nulls(gen: &TermGen, source: &LogicalSource, row: &mut Row) {
    if gen.semantics() == Semantics::R2rml && !source.nulls.is_empty() {
        row.retain(|k, v| k.starts_with(PUSHDOWN_PREFIX) || !source.nulls.contains(v));
    }
}

/// Check every triples map's columns against what its own logical source
/// returns, for the connectors that can describe a query without running it.
pub(crate) fn check_relational_columns(
    mapping: &RmlMapping,
    conn: &mut dyn SourceConnection,
    quote: &dyn Fn(&str) -> String,
) -> Result<(), String> {
    for tm in &mapping.triples_maps {
        let Some(sql) = tm.logical_source.sql(quote) else {
            continue;
        };
        let columns = conn
            .columns(&sql)
            .map_err(|e| format!("reading the logical source of <{}>: {e}", tm.iri))?;
        if let Some(columns) = columns {
            check_columns(mapping, tm, &columns)?;
        }
    }
    Ok(())
}

pub(crate) fn index_key(r: &RefObjectMap) -> RefKey {
    (
        r.parent_triples_map.clone(),
        r.joins
            .iter()
            .map(|j| (j.child.clone(), j.parent.clone()))
            .collect(),
    )
}

/// Blank-node labels must be valid N-Triples names, and a run id is a UUID —
/// keep only what is safe rather than trusting it.
pub(crate) fn sanitise_label(run_id: &str) -> String {
    run_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(32)
        .collect()
}

pub(crate) fn flush(
    store: &TripleStore,
    buffer: &mut String,
    target_graph: &str,
) -> Result<(), String> {
    if buffer.is_empty() {
        return Ok(());
    }
    store
        .load_str(
            buffer,
            oxigraph::io::RdfFormat::NTriples,
            Some(target_graph),
        )
        .map_err(|e| format!("writing generated triples into <{target_graph}>: {e}"))?;
    buffer.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rml::parse_rml;
    use ots_plugin_api::sources::{SourceConnector, Value, ValueKind};
    use oxigraph::sparql::QueryResults;

    const PFX: &str = "@prefix rr: <http://www.w3.org/ns/r2rml#> .\n\
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .\n\
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
@prefix fnml: <http://semweb.mmlab.be/ns/fnml#> .\n\
@prefix fno: <https://w3id.org/function/ontology#> .\n\
@prefix fn: <https://w3id.org/open-triplestore/fn#> .\n\
@prefix ex: <http://example.org/> .\n";

    fn db() -> (tempfile::TempDir, Box<dyn SourceConnection>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE supplier (sid INTEGER PRIMARY KEY, label TEXT NOT NULL);
             CREATE TABLE product (
                pid INTEGER PRIMARY KEY, name TEXT NOT NULL, qty INTEGER,
                status TEXT, sid INTEGER REFERENCES supplier(sid));
             CREATE TABLE tag (sid INTEGER, label TEXT NOT NULL);
             INSERT INTO supplier VALUES (1,'Acme'), (2,'Globex');
             INSERT INTO tag VALUES (1,'red'), (1,'blue'), (2,'green');
             INSERT INTO product VALUES
               (10,'Bolt',5,'active',1),
               (11,'Nut',NULL,'ACTIVE ',1),
               (12,'Washer',7,'retired',NULL);",
        )
        .unwrap();
        let params = ots_plugin_api::sources::ConnectParams {
            dialect: "sqlite".into(),
            host: None,
            port: None,
            database: path.to_string_lossy().into_owned(),
            username: None,
            password: None,
            read_only: true,
            statement_timeout_ms: 5_000,
            tls: false,
            options: Default::default(),
        };
        let c = crate::sources::sqlite::SqliteConnector
            .connect(&params)
            .unwrap();
        (dir, c)
    }

    fn quote(t: &str) -> String {
        format!("\"{}\"", t.replace('"', "\"\""))
    }

    /// `OTS_SOURCES_JOIN_MAX_ROWS` is process-wide, so the test that lowers it
    /// must not overlap a test that relies on the default. Every test that
    /// executes a mapping takes this lock.
    struct EnvGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

    impl EnvGuard {
        fn new() -> Self {
            static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
            EnvGuard(LOCK.lock().unwrap_or_else(|e| e.into_inner()))
        }
        fn with_cap(cap: &str) -> Self {
            let g = Self::new();
            std::env::set_var(JOIN_MAX_ROWS_ENV, cap);
            g
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // Cleared on drop, so a failing assertion cannot leak the cap into
            // sibling tests and fail them instead of itself.
            std::env::remove_var(JOIN_MAX_ROWS_ENV);
        }
    }

    fn env_guard() -> EnvGuard {
        EnvGuard::new()
    }

    fn run(mapping_ttl: &str, batch: usize) -> (TripleStore, SqlOutcome) {
        let _guard = env_guard();
        let mapping = parse_rml(&format!("{PFX}{mapping_ttl}")).expect("mapping parses");
        let (_dir, mut conn) = db();
        let store = TripleStore::in_memory().unwrap();
        let outcome = execute_relational(
            &mapping,
            conn.as_mut(),
            &quote,
            &store,
            "urn:run:test",
            batch,
            "abc-123",
            OnDataError::Abort,
        )
        .expect("run succeeds");
        (store, outcome)
    }

    fn ask(store: &TripleStore, pattern: &str) -> bool {
        match store
            .query(&format!(
                "PREFIX ex: <http://example.org/> ASK {{ GRAPH <urn:run:test> {{ {pattern} }} }}"
            ))
            .unwrap()
        {
            QueryResults::Boolean(b) => b,
            _ => false,
        }
    }

    fn count(store: &TripleStore) -> usize {
        store.count_graph(Some("urn:run:test")).unwrap()
    }

    const JOINED: &str = r#"
        ex:Product a rr:TriplesMap ;
          rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "product" ] ;
          rr:subjectMap [ rr:template "http://example.org/p{pid}" ; rr:class ex:Product ] ;
          rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] ;
          rr:predicateObjectMap [ rr:predicate ex:qty ; rr:objectMap [ rr:column "qty" ] ] ;
          rr:predicateObjectMap [ rr:predicate ex:supplier ; rr:objectMap [
             rr:parentTriplesMap ex:Supplier ;
             rr:joinCondition [ rr:child "sid" ; rr:parent "sid" ] ] ] .
        ex:Supplier a rr:TriplesMap ;
          rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "supplier" ] ;
          rr:subjectMap [ rr:template "http://example.org/s{sid}" ; rr:class ex:Supplier ] ;
          rr:predicateObjectMap [ rr:predicate ex:label ; rr:objectMap [ rr:column "label" ] ] .
    "#;

    #[test]
    fn a_join_resolves_to_the_parents_subject() {
        let (store, outcome) = run(JOINED, 100);
        assert_eq!(outcome.rows, 5, "3 product rows + 2 supplier rows");
        assert!(ask(
            &store,
            "<http://example.org/p10> ex:supplier <http://example.org/s1> ."
        ));
        assert!(ask(
            &store,
            "<http://example.org/p11> ex:supplier <http://example.org/s1> ."
        ));
        assert!(ask(
            &store,
            "<http://example.org/s1> a ex:Supplier ; ex:label \"Acme\" ."
        ));
        // A NULL foreign key joins to nothing, so no triple is emitted.
        assert!(!ask(&store, "<http://example.org/p12> ex:supplier ?o ."));
    }

    /// The same mapping as JOINED, but the parent reads through `rml:query`.
    /// A query is opaque to the catalogue, so its keys are unknown and the
    /// planner must fall back to the index.
    const JOINED_VIA_QUERY: &str = r#"
        ex:Product a rr:TriplesMap ;
          rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "product" ] ;
          rr:subjectMap [ rr:template "http://example.org/p{pid}" ; rr:class ex:Product ] ;
          rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] ;
          rr:predicateObjectMap [ rr:predicate ex:qty ; rr:objectMap [ rr:column "qty" ] ] ;
          rr:predicateObjectMap [ rr:predicate ex:supplier ; rr:objectMap [
             rr:parentTriplesMap ex:Supplier ;
             rr:joinCondition [ rr:child "sid" ; rr:parent "sid" ] ] ] .
        ex:Supplier a rr:TriplesMap ;
          rml:logicalSource [ rml:source <urn:source:s> ; rml:query "SELECT sid, label FROM supplier" ] ;
          rr:subjectMap [ rr:template "http://example.org/s{sid}" ; rr:class ex:Supplier ] ;
          rr:predicateObjectMap [ rr:predicate ex:label ; rr:objectMap [ rr:column "label" ] ] .
    "#;

    fn all_triples(store: &TripleStore) -> Vec<String> {
        let QueryResults::Solutions(sols) = store
            .query("SELECT ?s ?p ?o WHERE { GRAPH <urn:run:test> { ?s ?p ?o } } ORDER BY ?s ?p ?o")
            .unwrap()
        else {
            panic!()
        };
        sols.map(|r| {
            let r = r.unwrap();
            format!("{:?} {:?} {:?}", r.get("s"), r.get("p"), r.get("o"))
        })
        .collect()
    }

    #[test]
    fn pushdown_and_the_index_produce_exactly_the_same_triples() {
        // supplier.sid is the primary key, so JOINED pushes down; the
        // query-sourced parent cannot, so it is indexed. The output must not
        // depend on which strategy the planner picked.
        let (pushed, a) = run(JOINED, 100);
        let (indexed, b) = run(JOINED_VIA_QUERY, 100);
        assert_eq!(
            a.rows, b.rows,
            "a unique-key join does not duplicate child rows"
        );
        assert_eq!(a.triples, b.triples);
        assert_eq!(all_triples(&pushed), all_triples(&indexed));
        assert!(ask(
            &pushed,
            "<http://example.org/p10> ex:supplier <http://example.org/s1> ."
        ));
    }

    #[test]
    fn a_non_unique_parent_key_is_not_pushed_down() {
        // tag.sid has no unique constraint and supplier 1 has two tags. Pushed
        // down, the LEFT JOIN would duplicate every product row — inflating the
        // row count and re-emitting the product's own triples. Indexed, one
        // child row yields two objects, which is what RML means.
        let (store, outcome) = run(
            r#"
            ex:Product a rr:TriplesMap ;
              rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "product" ] ;
              rr:subjectMap [ rr:template "http://example.org/p{pid}" ] ;
              rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] ;
              rr:predicateObjectMap [ rr:predicate ex:tag ; rr:objectMap [
                 rr:parentTriplesMap ex:Tag ;
                 rr:joinCondition [ rr:child "sid" ; rr:parent "sid" ] ] ] .
            ex:Tag a rr:TriplesMap ;
              rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "tag" ] ;
              rr:subjectMap [ rr:template "http://example.org/t{sid}_{label}" ] .
            "#,
            100,
        );
        assert_eq!(
            outcome.rows, 6,
            "3 products + 3 tags, with no child row duplicated"
        );
        assert!(ask(
            &store,
            "<http://example.org/p10> ex:tag <http://example.org/t1_red> ."
        ));
        assert!(ask(
            &store,
            "<http://example.org/p10> ex:tag <http://example.org/t1_blue> ."
        ));
        let QueryResults::Solutions(names) = store
            .query(
                "SELECT ?n WHERE { GRAPH <urn:run:test> { \
                 <http://example.org/p10> <http://example.org/name> ?n } }",
            )
            .unwrap()
        else {
            panic!("expected solutions")
        };
        assert_eq!(
            names.count(),
            1,
            "the child's own triples are emitted once, not once per tag"
        );
    }

    #[test]
    fn the_planner_refuses_every_join_it_cannot_prove_safe() {
        let mapping = parse_rml(&format!("{PFX}{JOINED}")).unwrap();
        let parent = mapping.find("http://example.org/Supplier").unwrap();
        let joins = vec![JoinCondition {
            child: "sid".into(),
            parent: "sid".into(),
        }];
        let unique: HashMap<String, Vec<Vec<String>>> =
            HashMap::from([("supplier".to_string(), vec![vec!["sid".to_string()]])]);

        let r2rml = Semantics::R2rml;
        assert!(
            can_push_down(parent, &joins, &unique, r2rml),
            "a primary-key join is safe"
        );
        assert!(
            !can_push_down(parent, &[], &unique, r2rml),
            "a join-less reference is a cross join"
        );
        assert!(
            !can_push_down(parent, &joins, &HashMap::new(), r2rml),
            "no known key means no proof of uniqueness"
        );
        assert!(
            !can_push_down(
                parent,
                &[JoinCondition {
                    child: "sid".into(),
                    parent: "label".into()
                }],
                &unique,
                r2rml
            ),
            "joining on a non-key column is not safe"
        );

        // A legacy blank-node parent subject is minted per parent row, not per
        // child row, so it is never pushed down. An R2RML one is a function of
        // its value, so any row computes the same node.
        let mut bnode_parent = parent.clone();
        bnode_parent.subject_map.term_map.term_type = TermType::BlankNode;
        assert!(!can_push_down(
            &bnode_parent,
            &joins,
            &unique,
            Semantics::Legacy
        ));
        assert!(can_push_down(&bnode_parent, &joins, &unique, r2rml));
        // A schema-qualified table is not looked up in the current schema.
        let mut qualified = parent.clone();
        qualified.logical_source.table_name = Some("main.supplier".into());
        assert!(!can_push_down(&qualified, &joins, &unique, r2rml));
    }

    #[test]
    fn a_composite_unique_key_is_covered_by_a_superset_of_join_columns() {
        let mapping = parse_rml(&format!("{PFX}{JOINED}")).unwrap();
        let parent = mapping.find("http://example.org/Supplier").unwrap();
        let unique: HashMap<String, Vec<Vec<String>>> = HashMap::from([(
            "supplier".to_string(),
            vec![vec!["sid".to_string(), "label".to_string()]],
        )]);
        let both = vec![
            JoinCondition {
                child: "sid".into(),
                parent: "sid".into(),
            },
            JoinCondition {
                child: "name".into(),
                parent: "label".into(),
            },
        ];
        assert!(
            can_push_down(parent, &both, &unique, Semantics::R2rml),
            "both key columns are joined"
        );
        let partial = vec![JoinCondition {
            child: "sid".into(),
            parent: "sid".into(),
        }];
        assert!(
            !can_push_down(parent, &partial, &unique, Semantics::R2rml),
            "half a composite key does not make the match unique"
        );
    }

    /// A referencing object map with no join condition over the child's own
    /// table: each row joins to itself (R2RML §8).
    const SAME_ROW: &str = r#"
        ex:Product a rr:TriplesMap ;
          rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "product" ] ;
          rr:subjectMap [ rr:template "http://example.org/p{pid}" ] ;
          rr:predicateObjectMap [ rr:predicate ex:status ;
             rr:objectMap [ rr:parentTriplesMap ex:Status ] ] .
        ex:Status a rr:TriplesMap ;
          rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "product" ] ;
          rr:subjectMap [ rr:template "http://example.org/status/{status}" ] .
    "#;

    #[test]
    fn a_join_less_reference_joins_each_row_to_itself_not_every_row() {
        let (store, outcome) = run(SAME_ROW, 1);
        assert!(ask(
            &store,
            "<http://example.org/p10> ex:status <http://example.org/status/active> ."
        ));
        assert!(ask(
            &store,
            "<http://example.org/p12> ex:status <http://example.org/status/retired> ."
        ));
        assert!(
            !ask(
                &store,
                "<http://example.org/p10> ex:status <http://example.org/status/retired> ."
            ),
            "the old cross join linked every product to every status"
        );
        assert_eq!(outcome.triples, 3, "one link per product row");
        assert_eq!(count(&store), 3);
    }

    #[test]
    fn a_join_less_reference_builds_no_index() {
        // It needs no second scan, so the index cap cannot refuse it.
        let _guard = EnvGuard::with_cap("1");
        let mapping = parse_rml(&format!("{PFX}{SAME_ROW}")).unwrap();
        let plan = plan_triples_map(
            mapping.find("http://example.org/Product").unwrap(),
            &mapping,
            &HashMap::new(),
            &quote,
            None,
        )
        .unwrap();
        assert!(plan
            .strategies
            .values()
            .all(|s| matches!(s, JoinStrategy::SameRow)));
        let (_dir, mut conn) = db();
        let store = TripleStore::in_memory().unwrap();
        execute_relational(
            &mapping,
            conn.as_mut(),
            &quote,
            &store,
            "urn:run:test",
            10,
            "r",
            OnDataError::Abort,
        )
        .expect("no index, so no cap");
    }

    #[test]
    fn as_mapped_routing_keeps_graph_maps_and_into_routing_does_not() {
        let _guard = env_guard();
        let ttl = format!(
            "{PFX}
             ex:P a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"supplier\" ] ;
               rr:subjectMap [ rr:template \"http://example.org/s{{sid}}\" ;
                               rr:graph ex:Suppliers ] ;
               rr:predicateObjectMap [ rr:predicate ex:label ; rr:objectMap [ rr:column \"label\" ] ] ;
               rr:predicateObjectMap [ rr:predicate ex:id ; rr:objectMap [ rr:column \"sid\" ] ;
                                       rr:graph rr:defaultGraph ] ."
        );
        let mapping = parse_rml(&ttl).unwrap();
        let (_dir, mut conn) = db();
        let store = TripleStore::in_memory().unwrap();
        execute_relational_as_mapped(
            &mapping,
            conn.as_mut(),
            &quote,
            &store,
            10,
            "r",
            OnDataError::Abort,
        )
        .unwrap();
        assert_eq!(
            store.count_graph(Some("http://example.org/Suppliers")).unwrap(),
            4,
            "two labels and two ids in the subject's graph"
        );
        assert_eq!(
            store.count_graph(None).unwrap(),
            2,
            "the ids also go to rr:defaultGraph"
        );
        let (into, _) = run(
            r#"
             ex:P a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "supplier" ] ;
               rr:subjectMap [ rr:template "http://example.org/s{sid}" ; rr:graph ex:Suppliers ] ;
               rr:predicateObjectMap [ rr:predicate ex:label ; rr:objectMap [ rr:column "label" ] ] .
            "#,
            10,
        );
        assert_eq!(count(&into), 2, "a run's graph holds the whole result");
    }

    #[test]
    fn a_null_column_produces_no_triple_and_integers_are_typed_naturally() {
        let (store, _) = run(JOINED, 100);
        assert!(ask(
            &store,
            "<http://example.org/p10> ex:qty \"5\"^^<http://www.w3.org/2001/XMLSchema#integer> ."
        ));
        assert!(!ask(&store, "<http://example.org/p11> ex:qty ?q ."));
        assert!(
            ask(&store, "<http://example.org/p10> ex:name \"Bolt\" ."),
            "TEXT stays plain"
        );
    }

    #[test]
    fn batching_does_not_change_the_result() {
        let (small, a) = run(JOINED, 1);
        let (large, b) = run(JOINED, 1000);
        assert_eq!(a, b);
        assert_eq!(count(&small), count(&large));
        assert_eq!(a.triples as usize, count(&large));
    }

    #[test]
    fn the_enumeration_function_maps_normalises_and_keeps_unmapped_values() {
        let (store, _) = run(
            r#"
            ex:P a rr:TriplesMap ;
              rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "product" ] ;
              rr:subjectMap [ rr:template "http://example.org/p{pid}" ] ;
              rr:predicateObjectMap [ rr:predicate ex:status ; rr:objectMap [
                fnml:functionValue [
                  rr:predicateObjectMap [ rr:predicate fno:executes ; rr:object fn:mapValue ] ;
                  rr:predicateObjectMap [ rr:predicate fn:value ; rr:objectMap [ rr:column "status" ] ] ;
                  rr:predicateObjectMap [ rr:predicate fn:normalize ; rr:object "lower_trim" ] ;
                  rr:predicateObjectMap [ rr:predicate fn:mapping ; rr:object "active=http://example.org/Active" ] ;
                  rr:predicateObjectMap [ rr:predicate fn:unmapped ; rr:object "literal" ] ] ] ] .
            "#,
            100,
        );
        assert!(ask(
            &store,
            "<http://example.org/p10> ex:status <http://example.org/Active> ."
        ));
        assert!(
            ask(
                &store,
                "<http://example.org/p11> ex:status <http://example.org/Active> ."
            ),
            "'ACTIVE ' normalises onto the same term"
        );
        assert!(
            ask(&store, "<http://example.org/p12> ex:status \"retired\" ."),
            "an unmapped value stays a literal so SHACL flags it"
        );
    }

    #[test]
    fn an_rml_query_logical_source_is_used_verbatim() {
        let (store, outcome) = run(
            r#"
            ex:P a rr:TriplesMap ;
              rml:logicalSource [ rml:source <urn:source:s> ;
                                  rml:query "SELECT pid, name FROM product WHERE qty IS NOT NULL" ] ;
              rr:subjectMap [ rr:template "http://example.org/p{pid}" ] ;
              rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] .
            "#,
            100,
        );
        assert_eq!(outcome.rows, 2, "the WHERE clause is honoured");
        assert!(ask(&store, "<http://example.org/p10> ex:name \"Bolt\" ."));
        assert!(!ask(&store, "<http://example.org/p11> ex:name ?n ."));
    }

    #[test]
    fn a_mapping_error_inside_a_batch_aborts_the_run() {
        let _guard = env_guard();
        let mapping = parse_rml(&format!(
            "{PFX}
             ex:P a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"product\" ] ;
               rr:subjectMap [ rr:template \"http://example.org/p{{pid}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:status ; rr:objectMap [
                 fnml:functionValue [
                   rr:predicateObjectMap [ rr:predicate fno:executes ; rr:object fn:mapValue ] ;
                   rr:predicateObjectMap [ rr:predicate fn:value ; rr:objectMap [ rr:column \"status\" ] ] ;
                   rr:predicateObjectMap [ rr:predicate fn:mapping ; rr:object \"broken-entry\" ] ] ] ] ."
        ))
        .unwrap();
        let (_dir, mut conn) = db();
        let store = TripleStore::in_memory().unwrap();
        let err = execute_relational(
            &mapping,
            conn.as_mut(),
            &quote,
            &store,
            "urn:run:t",
            10,
            "r",
            OnDataError::Abort,
        )
        .unwrap_err();
        assert!(
            err.contains("fn:mapping"),
            "the mapping error survives the stream: {err}"
        );
    }

    #[test]
    fn blank_node_labels_carry_the_run_id_and_never_repeat_across_rows() {
        let (store, _) = run(
            r#"
            ex:P a rr:TriplesMap ;
              rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "product" ] ;
              rr:subjectMap [ rr:template "n{pid}" ; rr:termType rr:BlankNode ] ;
              rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] .
            "#,
            1,
        );
        // One blank node per row, three rows, three distinct subjects — even
        // though each batch was loaded separately.
        let QueryResults::Solutions(sols) = store
            .query("SELECT DISTINCT ?s WHERE { GRAPH <urn:run:test> { ?s ?p ?o } }")
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(sols.count(), 3);
    }

    #[test]
    fn a_join_index_over_the_cap_is_refused_by_name() {
        // The cap bounds the INDEX, so this needs a mapping that indexes: the
        // query-sourced parent is opaque to the catalogue and cannot push down.
        let _guard = EnvGuard::with_cap("1");
        let mapping = parse_rml(&format!("{PFX}{JOINED_VIA_QUERY}")).unwrap();
        let (_dir, mut conn) = db();
        let store = TripleStore::in_memory().unwrap();
        let err = execute_relational(
            &mapping,
            conn.as_mut(),
            &quote,
            &store,
            "urn:run:t",
            10,
            "r",
            OnDataError::Abort,
        )
        .unwrap_err();
        assert!(
            err.contains("Supplier") && err.contains(JOIN_MAX_ROWS_ENV),
            "{err}"
        );
    }

    #[test]
    fn a_pushed_down_join_is_not_bounded_by_the_index_cap() {
        // The cap exists to stop an index eating memory. A pushed-down join
        // builds no index, so the cap must not apply to it — otherwise the
        // cheaper strategy would be the one that fails first.
        let _guard = EnvGuard::with_cap("1");
        let mapping = parse_rml(&format!("{PFX}{JOINED}")).unwrap();
        let (_dir, mut conn) = db();
        let store = TripleStore::in_memory().unwrap();
        let outcome = execute_relational(
            &mapping,
            conn.as_mut(),
            &quote,
            &store,
            "urn:run:test",
            10,
            "r",
            OnDataError::Abort,
        )
        .expect("a pushed-down join ignores the index cap");
        assert_eq!(outcome.rows, 5);
        assert!(ask(
            &store,
            "<http://example.org/p10> ex:supplier <http://example.org/s1> ."
        ));
    }

    #[test]
    fn split_row_separates_values_from_their_types() {
        let mut row = Row::new();
        let mut kinds = Kinds::new();
        let src: SourceRow = SourceRow::from([
            ("a".to_string(), Value::new("1", ValueKind::Integer)),
            ("b".to_string(), Value::text("x")),
        ]);
        split_row(&src, &mut row, &mut kinds);
        assert_eq!(row.get("a").map(String::as_str), Some("1"));
        assert_eq!(kinds.get("a"), Some(&ValueKind::Integer));
        assert_eq!(kinds.get("b"), Some(&ValueKind::Text));
        // Reused buffers are cleared, never appended to.
        let src2: SourceRow = SourceRow::from([("c".to_string(), Value::text("y"))]);
        split_row(&src2, &mut row, &mut kinds);
        assert_eq!(row.len(), 1);
        assert!(!row.contains_key("a"));
    }

    #[test]
    fn a_join_key_with_a_null_never_matches() {
        let row: Row = Row::from([("a".to_string(), "1".to_string())]);
        assert_eq!(
            join_key(&row, &["a".to_string()]),
            Some(vec!["1".to_string()])
        );
        assert_eq!(join_key(&row, &["a".to_string(), "b".to_string()]), None);
    }
}
