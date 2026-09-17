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

use std::collections::HashMap;

use ots_plugin_api::sources::{Row as SourceRow, SourceConnection, SourceError};

use super::model::*;
use super::terms::{eval_function, eval_term, BlankNodes, Kinds, Row};
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SqlOutcome {
    pub rows: u64,
    pub triples: u64,
}

/// Generate one row's triples for one triples map.
///
/// `resolve_ref` answers a `rr:parentTriplesMap` object: the subject terms the
/// parent map generates for the rows this row joins to. `None` means the
/// caller has no index for it (the file-based path), and the reference is
/// skipped.
pub fn row_triples(
    tm: &TriplesMap,
    row: &Row,
    kinds: Option<&Kinds>,
    bnodes: &mut BlankNodes,
    row_bnodes: &mut HashMap<String, String>,
    resolve_ref: &dyn Fn(&RefObjectMap, &Row) -> Option<Vec<String>>,
) -> Result<Vec<EmittedTriple>, String> {
    let mut out = Vec::new();

    // The TriplesMap-level graph map, evaluated once per row.
    let tm_graph: Option<String> = tm
        .graph_map
        .as_ref()
        .and_then(|gm| super::terms::eval_iri(gm, row, kinds, bnodes, row_bnodes));

    let Some(subject) = eval_term(&tm.subject_map.term_map, row, kinds, bnodes, row_bnodes) else {
        // No subject — R2RML says the row generates nothing at all.
        return Ok(out);
    };

    for class_iri in &tm.subject_map.classes {
        out.push(EmittedTriple {
            graph: tm_graph.clone(),
            text: format!(
                "{subject} <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{class_iri}> ."
            ),
        });
    }

    for pom in &tm.predicate_object_maps {
        let Some(predicate) = eval_term(&pom.predicate_map, row, kinds, bnodes, row_bnodes) else {
            continue;
        };
        let objects: Vec<String> = match &pom.object {
            ObjectMap::Term(tm_obj) => eval_term(tm_obj, row, kinds, bnodes, row_bnodes)
                .into_iter()
                .collect(),
            ObjectMap::Function(f) => eval_function(f, row, kinds)?.into_iter().collect(),
            ObjectMap::Ref(r) => resolve_ref(r, row).unwrap_or_default(),
        };

        // A POM-level graph map overrides the TriplesMap-level one.
        let graph_key: Option<String> = pom
            .graph_map
            .as_ref()
            .and_then(|gm| super::terms::eval_iri(gm, row, kinds, bnodes, row_bnodes))
            .or_else(|| tm_graph.clone());

        for object in objects {
            out.push(EmittedTriple {
                graph: graph_key.clone(),
                text: format!("{subject} {predicate} {object} ."),
            });
        }
    }

    Ok(out)
}

/// Split a connector row into the lexical values and the per-column generic
/// types the natural-datatype rule needs.
fn split_row(src: &SourceRow, row: &mut Row, kinds: &mut Kinds) {
    row.clear();
    kinds.clear();
    for (name, value) in src {
        row.insert(name.clone(), value.lexical.clone());
        kinds.insert(name.clone(), value.kind);
    }
}

/// The join key for one row: the values of `columns`, in order. `None` when
/// any of them is NULL — SQL join semantics, so a NULL never matches.
fn join_key(row: &Row, columns: &[String]) -> Option<Vec<String>> {
    columns.iter().map(|c| row.get(c).cloned()).collect()
}

type ParentIndex = HashMap<Vec<String>, Vec<String>>;

/// Columns a pushed-down join projects from the parent carry this prefix, and
/// the child subquery carries it as its alias. Reserved: a source column whose
/// name starts with it would be shadowed.
const PUSHDOWN_PREFIX: &str = "__ots_j";

/// How one `rr:parentTriplesMap` reference is resolved. See the module docs for
/// why the choice is not free.
#[derive(Debug, Clone)]
enum JoinStrategy {
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
struct TmPlan {
    sql: String,
    strategies: HashMap<RefKey, JoinStrategy>,
}

type RefKey = (String, Vec<(String, String)>);

/// Whether a reference can be pushed into the child's query.
///
/// Four conditions, all necessary:
/// * there is at least one join condition — a join-less reference is a cross
///   join, which multiplies rows by definition;
/// * the parent reads a named table, so its keys can be looked up at all (an
///   `rml:query` parent is opaque to the catalogue);
/// * the parent's join columns cover one of that table's unique keys, so the
///   join cannot duplicate a child row;
/// * the parent's subject is an IRI. A blank-node subject must be minted once
///   per PARENT row; pushed down it would be minted once per child row, so two
///   children of one parent would stop sharing a node.
fn can_push_down(
    parent: &TriplesMap,
    joins: &[JoinCondition],
    unique_keys: &HashMap<String, Vec<Vec<String>>>,
) -> bool {
    if joins.is_empty() || parent.subject_map.term_map.term_type != TermType::IRI {
        return false;
    }
    let Some(table) = parent.logical_source.table_name.as_deref() else {
        return false;
    };
    if parent.logical_source.query.is_some() {
        return false;
    }
    let parent_cols: Vec<&str> = joins.iter().map(|j| j.parent.as_str()).collect();
    unique_keys.get(table).is_some_and(|keys| {
        keys.iter()
            .any(|k| k.iter().all(|c| parent_cols.contains(&c.as_str())))
    })
}

/// Build one triples map's plan: which references push down, and the SQL that
/// carries them.
fn plan_triples_map(
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

    for pom in &tm.predicate_object_maps {
        let ObjectMap::Ref(r) = &pom.object else {
            continue;
        };
        let key = index_key(r);
        if strategies.contains_key(&key) {
            continue;
        }
        let parent = mapping
            .find(&r.parent_triples_map)
            .ok_or_else(|| format!("unknown parent TriplesMap <{}>", r.parent_triples_map))?;
        if !can_push_down(parent, &r.joins, unique_keys) {
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
fn pushdown_subject(
    parent: &TriplesMap,
    alias: &str,
    witness: &[String],
    child_row: &Row,
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
    // Kinds are irrelevant: the subject is an IRI, so no natural datatype
    // applies (`can_push_down` guarantees the term type).
    super::terms::eval_iri_term(&parent.subject_map.term_map, &parent_row, None)
}

/// Stream a parent triples map once and index its subject terms by join key.
fn build_parent_index(
    parent: &TriplesMap,
    joins: &[JoinCondition],
    conn: &mut dyn SourceConnection,
    quote: &dyn Fn(&str) -> String,
    bnodes: &mut BlankNodes,
) -> Result<ParentIndex, String> {
    let sql = parent.logical_source.sql(quote).ok_or_else(|| {
        format!(
            "TriplesMap <{}> is used as a join parent but is not a relational source",
            parent.iri
        )
    })?;
    let parent_columns: Vec<String> = joins.iter().map(|j| j.parent.clone()).collect();
    let cap = join_max_rows();

    let mut index: ParentIndex = HashMap::new();
    let mut row: Row = HashMap::new();
    let mut kinds: Kinds = HashMap::new();
    let mut overflow: Option<String> = None;

    conn.stream(&sql, 1_000, &mut |batch| {
        for src in &batch {
            split_row(src, &mut row, &mut kinds);
            let Some(key) = join_key(&row, &parent_columns) else {
                continue;
            };
            let mut row_bnodes = HashMap::new();
            let Some(subject) = eval_term(
                &parent.subject_map.term_map,
                &row,
                Some(&kinds),
                bnodes,
                &mut row_bnodes,
            ) else {
                continue;
            };
            if index.len() >= cap && !index.contains_key(&key) {
                overflow = Some(format!(
                    "the join index for parent TriplesMap <{}> exceeded {cap} distinct keys; \
                     raise {JOIN_MAX_ROWS_ENV} or narrow the parent's logical source",
                    parent.iri
                ));
                return Err(SourceError::Query("join index overflow".into()));
            }
            let entry = index.entry(key).or_default();
            if !entry.contains(&subject) {
                entry.push(subject);
            }
        }
        Ok(())
    })
    .map_err(|e| {
        overflow
            .clone()
            .unwrap_or_else(|| format!("reading join parent <{}>: {e}", parent.iri))
    })?;

    Ok(index)
}

/// Every parent table a reference joins to, with its unique keys — the input
/// the planner needs. Costs one cheap catalogue lookup per distinct parent
/// table, and nothing at all for a mapping with no joins.
fn collect_unique_keys(
    mapping: &RmlMapping,
    conn: &mut dyn SourceConnection,
) -> Result<HashMap<String, Vec<Vec<String>>>, String> {
    let mut tables: Vec<String> = Vec::new();
    for tm in &mapping.triples_maps {
        for pom in &tm.predicate_object_maps {
            let ObjectMap::Ref(r) = &pom.object else {
                continue;
            };
            let Some(parent) = mapping.find(&r.parent_triples_map) else {
                continue;
            };
            if let Some(t) = &parent.logical_source.table_name {
                if parent.logical_source.query.is_none() && !tables.contains(t) {
                    tables.push(t.clone());
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
pub fn execute_relational(
    mapping: &RmlMapping,
    conn: &mut dyn SourceConnection,
    quote: &dyn Fn(&str) -> String,
    store: &TripleStore,
    target_graph: &str,
    batch_size: usize,
    run_id: &str,
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
    )
}

/// [`execute_relational`], restricted to rows past a cursor.
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
) -> Result<SqlOutcome, String> {
    let batch_size = batch_size.clamp(1, 100_000);
    // Blank-node labels carry the run id, so two runs' graphs never share a
    // node and a batch-by-batch load never merges rows.
    let mut bnodes = BlankNodes::new(format!("r{}_", sanitise_label(run_id)));
    let mut outcome = SqlOutcome::default();

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
        for pom in &tm.predicate_object_maps {
            let ObjectMap::Ref(r) = &pom.object else {
                continue;
            };
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
            let index = build_parent_index(parent, &r.joins, conn, quote, &mut bnodes)?;
            indexes.insert(key, index);
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

    let mut buffer = String::with_capacity(FLUSH_BYTES / 4);
    let mut row: Row = HashMap::new();
    let mut kinds: Kinds = HashMap::new();

    for tm in &mapping.triples_maps {
        let plan = &plans[&tm.iri];
        let mut emit_error: Option<String> = None;
        let mut triples: u64 = 0;
        let rows = conn
            .stream(&plan.sql, batch_size, &mut |batch| {
                for src in &batch {
                    split_row(src, &mut row, &mut kinds);
                    let mut row_bnodes = HashMap::new();
                    let generated = row_triples(
                        tm,
                        &row,
                        Some(&kinds),
                        &mut bnodes,
                        &mut row_bnodes,
                        &|r, child_row| {
                            let key = index_key(r);
                            match plan.strategies.get(&key) {
                                Some(JoinStrategy::Pushdown { alias, witness }) => {
                                    let parent = mapping.find(&r.parent_triples_map)?;
                                    pushdown_subject(parent, alias, witness, child_row)
                                        .map(|s| vec![s])
                                }
                                _ => {
                                    let index = indexes.get(&key)?;
                                    let child_columns: Vec<String> =
                                        r.joins.iter().map(|j| j.child.clone()).collect();
                                    if child_columns.is_empty() {
                                        // A join-less reference over the same
                                        // logical source: every parent matches.
                                        return Some(index.values().flatten().cloned().collect());
                                    }
                                    let k = join_key(child_row, &child_columns)?;
                                    index.get(&k).cloned()
                                }
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
                    for t in generated {
                        buffer.push_str(&t.text);
                        buffer.push('\n');
                        triples += 1;
                    }
                }
                if buffer.len() >= FLUSH_BYTES {
                    if let Err(e) = flush(store, &mut buffer, target_graph) {
                        emit_error = Some(e);
                        return Err(SourceError::Query("write error".into()));
                    }
                }
                Ok(())
            })
            .map_err(|e| {
                emit_error
                    .clone()
                    .unwrap_or_else(|| format!("reading <{}>: {e}", tm.iri))
            })?;
        if let Some(e) = emit_error {
            return Err(e);
        }
        outcome.rows += rows;
        outcome.triples += triples;
    }

    flush(store, &mut buffer, target_graph)?;
    Ok(outcome)
}

fn index_key(r: &RefObjectMap) -> RefKey {
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
fn sanitise_label(run_id: &str) -> String {
    run_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(32)
        .collect()
}

fn flush(store: &TripleStore, buffer: &mut String, target_graph: &str) -> Result<(), String> {
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

        assert!(
            can_push_down(parent, &joins, &unique),
            "a primary-key join is safe"
        );
        assert!(
            !can_push_down(parent, &[], &unique),
            "a join-less reference is a cross join"
        );
        assert!(
            !can_push_down(parent, &joins, &HashMap::new()),
            "no known key means no proof of uniqueness"
        );
        assert!(
            !can_push_down(
                parent,
                &[JoinCondition {
                    child: "sid".into(),
                    parent: "label".into()
                }],
                &unique
            ),
            "joining on a non-key column is not safe"
        );

        // A blank-node parent subject must be minted per parent row, not per
        // child row, so it is never pushed down.
        let mut bnode_parent = parent.clone();
        bnode_parent.subject_map.term_map.term_type = TermType::BlankNode;
        assert!(!can_push_down(&bnode_parent, &joins, &unique));
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
            can_push_down(parent, &both, &unique),
            "both key columns are joined"
        );
        let partial = vec![JoinCondition {
            child: "sid".into(),
            parent: "sid".into(),
        }];
        assert!(
            !can_push_down(parent, &partial, &unique),
            "half a composite key does not make the match unique"
        );
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
