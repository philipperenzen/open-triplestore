//! The sampling executor behind a dry-run: a few rows of each triples map,
//! plus every row those rows reach through a join, materialised into a
//! scratch graph.
//!
//! **Why the pull-in.** A dry-run validates its sample with the same shapes a
//! run is gated by. Sample one row of a child table on its own and every
//! `rr:parentTriplesMap` reference in it points at a parent that was never
//! materialised — so a shape's `sh:class` on that property reports a
//! violation that is an artefact of sampling, not of the mapping. The
//! referenced parent rows are therefore fetched by key and mapped too, and a
//! parent's own references likewise, until the sample is closed under its
//! joins.
//!
//! **What it shares with a run.** The plan is the run's plan
//! ([`super::sql::plan_triples_map`]): the same pushed-down joins, the same
//! index fallback, the same term evaluation. A dry-run that resolved a join
//! differently from the run it previews would preview something else.
//!
//! **How rows are bounded.** No `LIMIT` is written into SQL — dialects spell
//! it differently and the connector takes a statement, not parameters. A map
//! is sampled by streaming its plan in one batch of the sample size and
//! stopping the stream after it. Parent rows are fetched with a `WHERE … IN`
//! over the child keys when every key is a plain value, and by streaming and
//! filtering otherwise, so a key that would need escaping never reaches SQL.

use std::collections::{HashMap, HashSet};

use ots_plugin_api::sources::{Row as SourceRow, SourceConnection, SourceError};
use serde::Serialize;

use super::model::*;
use super::sql::{
    collect_unique_keys, eval_subject, flush, index_key, join_key, plan_triples_map,
    pushdown_subject, row_triples, sanitise_label, split_row, JoinStrategy, ParentIndex, RefKey,
    TmPlan,
};
use super::terms::{BlankNodes, Kinds, Row};
use crate::store::engine::TripleStore;

/// Most rows one triples map contributes directly. Parents pulled in by key
/// come on top, bounded by the number of distinct keys the sample carries.
pub const MAX_SAMPLE_ROWS: usize = 1_000;

/// What to sample.
#[derive(Debug, Clone, Default)]
pub struct SampleSpec {
    /// Rows per sampled triples map.
    pub limit: usize,
    /// Restrict the directly sampled maps to those reading these tables.
    pub tables: Vec<String>,
    /// …or to these triples maps, by IRI. Both empty means every map.
    pub triples_maps: Vec<String>,
}

impl SampleSpec {
    fn selects(&self, tm: &TriplesMap) -> bool {
        if self.tables.is_empty() && self.triples_maps.is_empty() {
            return true;
        }
        let by_table = tm
            .logical_source
            .table_name
            .as_deref()
            .is_some_and(|t| self.tables.iter().any(|w| w == t));
        by_table || self.triples_maps.contains(&tm.iri)
    }
}

/// What one triples map contributed to the sample.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SampledMap {
    pub triples_map: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// Rows taken from the head of the map's own query.
    pub sampled_rows: u64,
    /// Rows fetched because a sampled row referenced them through a join.
    pub pulled_in_rows: u64,
    pub triples: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SampleOutcome {
    pub rows: u64,
    pub triples: u64,
    pub maps: Vec<SampledMap>,
}

/// A row with the column types it came with.
type Typed = (Row, Kinds);

/// Materialise a sample of `mapping` into `target_graph`.
pub fn execute_sample(
    mapping: &RmlMapping,
    conn: &mut dyn SourceConnection,
    quote: &dyn Fn(&str) -> String,
    store: &TripleStore,
    target_graph: &str,
    run_id: &str,
    spec: &SampleSpec,
) -> Result<SampleOutcome, String> {
    let limit = spec.limit.clamp(1, MAX_SAMPLE_ROWS);
    let unique_keys = collect_unique_keys(mapping, conn)?;
    let mut plans: HashMap<String, TmPlan> = HashMap::new();
    for tm in &mapping.triples_maps {
        plans.insert(
            tm.iri.clone(),
            plan_triples_map(tm, mapping, &unique_keys, quote, None)?,
        );
    }

    let sampled: Vec<&TriplesMap> = mapping
        .triples_maps
        .iter()
        .filter(|tm| spec.selects(tm))
        .collect();
    if sampled.is_empty() {
        let wanted: Vec<&str> = spec
            .tables
            .iter()
            .chain(spec.triples_maps.iter())
            .map(String::as_str)
            .collect();
        return Err(format!(
            "no triples map reads {}; the mapping reads {}",
            wanted.join(", "),
            mapping
                .triples_maps
                .iter()
                .map(|tm| tm
                    .logical_source
                    .table_name
                    .clone()
                    .unwrap_or_else(|| format!("<{}>", tm.iri)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // ── 1. The head of every selected map ──
    let mut rows_of: HashMap<String, Vec<Typed>> = HashMap::new();
    let mut seen: HashMap<String, HashSet<String>> = HashMap::new();
    let mut direct: HashMap<String, u64> = HashMap::new();
    let mut pulled: HashMap<String, u64> = HashMap::new();
    for tm in &sampled {
        let rows = take(conn, &plans[&tm.iri].sql, limit)
            .map_err(|e| format!("sampling <{}>: {e}", tm.iri))?;
        for src in rows {
            if insert_row(&mut rows_of, &mut seen, &tm.iri, &src) {
                *direct.entry(tm.iri.clone()).or_default() += 1;
            }
        }
    }

    // ── 2. Close the sample under its joins ──
    // Each reference remembers the keys already asked for, so a parent is
    // fetched once per key however many children point at it, and a cycle of
    // references terminates when no new key appears.
    let mut requested: HashMap<RefKey, HashSet<Vec<String>>> = HashMap::new();
    let mut worklist: Vec<String> = sampled.iter().map(|tm| tm.iri.clone()).collect();
    while let Some(tm_iri) = worklist.pop() {
        let tm = mapping
            .find(&tm_iri)
            .ok_or_else(|| format!("unknown TriplesMap <{tm_iri}>"))?;
        for pom in &tm.predicate_object_maps {
            let ObjectMap::Ref(r) = &pom.object else {
                continue;
            };
            // A join-less reference over the same logical source resolves to
            // the parent map's own sample; there is no key to pull in by.
            if r.joins.is_empty() {
                continue;
            }
            let parent = mapping
                .find(&r.parent_triples_map)
                .ok_or_else(|| format!("unknown parent TriplesMap <{}>", r.parent_triples_map))?;
            let child_cols: Vec<String> = r.joins.iter().map(|j| j.child.clone()).collect();
            let parent_cols: Vec<String> = r.joins.iter().map(|j| j.parent.clone()).collect();

            let wanted: HashSet<Vec<String>> = rows_of
                .get(&tm_iri)
                .map(|rows| {
                    rows.iter()
                        .filter_map(|(row, _)| join_key(row, &child_cols))
                        .collect()
                })
                .unwrap_or_default();
            let already = requested.entry(index_key(r)).or_default();
            let new_keys: Vec<Vec<String>> = wanted
                .into_iter()
                .filter(|k| !already.contains(k))
                .collect();
            if new_keys.is_empty() {
                continue;
            }
            already.extend(new_keys.iter().cloned());

            let fetched = fetch_by_keys(
                conn,
                quote,
                &plans[&parent.iri].sql,
                &parent_cols,
                &new_keys,
            )
            .map_err(|e| format!("pulling in rows of <{}>: {e}", parent.iri))?;
            let mut added = false;
            for src in fetched {
                if insert_row(&mut rows_of, &mut seen, &parent.iri, &src) {
                    *pulled.entry(parent.iri.clone()).or_default() += 1;
                    added = true;
                }
            }
            if added {
                worklist.push(parent.iri.clone());
            }
        }
    }

    // ── 3. Indexes for the references the plan did not push down ──
    // Built from the rows in hand, never from the database: the sample is
    // closed under its joins, so every parent a child row can reach is here.
    let mut bnodes = BlankNodes::new(format!("d{}_", sanitise_label(run_id)));
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
            let parent_cols: Vec<String> = r.joins.iter().map(|j| j.parent.clone()).collect();
            let mut index = ParentIndex::new();
            for (row, kinds) in rows_of.get(&parent.iri).map(Vec::as_slice).unwrap_or(&[]) {
                let Some(k) = join_key(row, &parent_cols) else {
                    continue;
                };
                let mut row_bnodes = HashMap::new();
                if let Some(subject) = eval_subject(
                    &parent.subject_map,
                    row,
                    Some(kinds),
                    &mut bnodes,
                    &mut row_bnodes,
                )? {
                    let entry = index.entry(k).or_default();
                    if !entry.contains(&subject) {
                        entry.push(subject);
                    }
                }
            }
            indexes.insert(key, index);
        }
    }

    // ── 4. Emit, in mapping order ──
    let mut buffer = String::new();
    let mut outcome = SampleOutcome::default();
    for tm in &mapping.triples_maps {
        let Some(rows) = rows_of.get(&tm.iri) else {
            continue;
        };
        let plan = &plans[&tm.iri];
        let mut triples = 0u64;
        for (row, kinds) in rows {
            let mut row_bnodes = HashMap::new();
            let generated = row_triples(
                tm,
                row,
                Some(kinds),
                &mut bnodes,
                &mut row_bnodes,
                &|r, child_row| match plan.strategies.get(&index_key(r)) {
                    Some(JoinStrategy::Pushdown { alias, witness }) => {
                        let parent = mapping.find(&r.parent_triples_map)?;
                        pushdown_subject(parent, alias, witness, child_row).map(|s| vec![s])
                    }
                    _ => {
                        let index = indexes.get(&index_key(r))?;
                        let child_cols: Vec<String> =
                            r.joins.iter().map(|j| j.child.clone()).collect();
                        if child_cols.is_empty() {
                            return Some(index.values().flatten().cloned().collect());
                        }
                        index.get(&join_key(child_row, &child_cols)?).cloned()
                    }
                },
            )?;
            for t in generated {
                buffer.push_str(&t.text);
                buffer.push('\n');
                triples += 1;
            }
        }
        outcome.rows += rows.len() as u64;
        outcome.triples += triples;
        outcome.maps.push(SampledMap {
            triples_map: tm.iri.clone(),
            table: tm.logical_source.table_name.clone(),
            sampled_rows: direct.get(&tm.iri).copied().unwrap_or(0),
            pulled_in_rows: pulled.get(&tm.iri).copied().unwrap_or(0),
            triples,
        });
    }
    flush(store, &mut buffer, target_graph)?;
    Ok(outcome)
}

/// Keep `src` for `tm` unless an identical row is already held.
///
/// A parent's own sample and its pulled-in rows overlap; mapping a row twice
/// would double its blank nodes and its triple count.
fn insert_row(
    rows_of: &mut HashMap<String, Vec<Typed>>,
    seen: &mut HashMap<String, HashSet<String>>,
    tm: &str,
    src: &SourceRow,
) -> bool {
    let mut row = Row::with_capacity(src.len());
    let mut kinds = Kinds::with_capacity(src.len());
    split_row(src, &mut row, &mut kinds);
    let mut cells: Vec<(&String, &String)> = row.iter().collect();
    cells.sort();
    let fingerprint = cells
        .iter()
        .map(|(k, v)| format!("{k}\u{1}{v}"))
        .collect::<Vec<_>>()
        .join("\u{2}");
    if !seen.entry(tm.to_string()).or_default().insert(fingerprint) {
        return false;
    }
    rows_of
        .entry(tm.to_string())
        .or_default()
        .push((row, kinds));
    true
}

/// The first `limit` rows of `sql`: one batch, then the stream is stopped.
fn take(
    conn: &mut dyn SourceConnection,
    sql: &str,
    limit: usize,
) -> Result<Vec<SourceRow>, String> {
    let mut out: Vec<SourceRow> = Vec::with_capacity(limit);
    let mut stopped = false;
    let result = conn.stream(sql, limit, &mut |batch| {
        out.extend(batch.into_iter().take(limit - out.len().min(limit)));
        if out.len() >= limit {
            stopped = true;
            return Err(SourceError::Query("sample complete".into()));
        }
        Ok(())
    });
    match result {
        Ok(_) => Ok(out),
        Err(_) if stopped => Ok(out),
        Err(e) => Err(e.to_string()),
    }
}

/// The alias a key-restricted parent query carries.
const KEY_ALIAS: &str = "__ots_k";

/// A value that can be written as a SQL string literal without escaping.
fn is_plain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | ':' | '.' | ' ' | '+' | '_' | '/' | '@')
        })
}

/// The rows of `sql` whose `columns` take one of `keys`.
fn fetch_by_keys(
    conn: &mut dyn SourceConnection,
    quote: &dyn Fn(&str) -> String,
    sql: &str,
    columns: &[String],
    keys: &[Vec<String>],
) -> Result<Vec<SourceRow>, String> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let plain = keys.iter().all(|k| k.iter().all(|v| is_plain(v)));
    if plain {
        let alias = quote(KEY_ALIAS);
        let literal = |v: &str| format!("'{v}'");
        let predicate = if columns.len() == 1 {
            format!(
                "{alias}.{} IN ({})",
                quote(&columns[0]),
                keys.iter()
                    .map(|k| literal(&k[0]))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            keys.iter()
                .map(|k| {
                    format!(
                        "({})",
                        k.iter()
                            .zip(columns)
                            .map(|(v, c)| format!("{alias}.{} = {}", quote(c), literal(v)))
                            .collect::<Vec<_>>()
                            .join(" AND ")
                    )
                })
                .collect::<Vec<_>>()
                .join(" OR ")
        };
        let statement = format!("SELECT * FROM ({sql}) {alias} WHERE {predicate}");
        let mut out = Vec::new();
        conn.stream(&statement, 1_000, &mut |batch| {
            out.extend(batch);
            Ok(())
        })
        .map_err(|e| e.to_string())?;
        return Ok(out);
    }

    // A key that would need escaping is matched in memory instead: slower,
    // never wrong, and no value the database handed out goes back into SQL.
    let wanted: HashSet<&Vec<String>> = keys.iter().collect();
    let mut out = Vec::new();
    let mut row = Row::new();
    let mut kinds = Kinds::new();
    conn.stream(sql, 1_000, &mut |batch| {
        for src in batch {
            split_row(&src, &mut row, &mut kinds);
            if join_key(&row, columns).is_some_and(|k| wanted.contains(&k)) {
                out.push(src);
            }
        }
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rml::parse_rml;
    use ots_plugin_api::sources::SourceConnector;
    use oxigraph::sparql::QueryResults;

    const PFX: &str = "@prefix rr: <http://www.w3.org/ns/r2rml#> .\n\
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .\n\
@prefix ex: <http://example.org/> .\n";

    const JOINED: &str = r#"
        ex:Product a rr:TriplesMap ;
          rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "product" ] ;
          rr:subjectMap [ rr:template "http://example.org/p{pid}" ; rr:class ex:Product ] ;
          rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] ;
          rr:predicateObjectMap [ rr:predicate ex:supplier ; rr:objectMap [
             rr:parentTriplesMap ex:Supplier ;
             rr:joinCondition [ rr:child "sid" ; rr:parent "sid" ] ] ] .
        ex:Supplier a rr:TriplesMap ;
          rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName "supplier" ] ;
          rr:subjectMap [ rr:template "http://example.org/s{sid}" ; rr:class ex:Supplier ] ;
          rr:predicateObjectMap [ rr:predicate ex:label ; rr:objectMap [ rr:column "label" ] ] ;
          rr:predicateObjectMap [ rr:predicate ex:region ; rr:objectMap [
             rr:parentTriplesMap ex:Region ;
             rr:joinCondition [ rr:child "rid" ; rr:parent "rid" ] ] ] .
        ex:Region a rr:TriplesMap ;
          rml:logicalSource [ rml:source <urn:source:s> ; rml:query "SELECT rid, name AS rname FROM region" ] ;
          rr:subjectMap [ rr:template "http://example.org/r{rid}" ; rr:class ex:Region ] ;
          rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "rname" ] ] .
    "#;

    /// Suppliers 1–40 in regions 1–4; product 10 is the LAST product and
    /// points at supplier 40, so a head sample of suppliers never holds it.
    fn db() -> (tempfile::TempDir, Box<dyn SourceConnection>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE region (rid INTEGER PRIMARY KEY, name TEXT NOT NULL);
             CREATE TABLE supplier (sid INTEGER PRIMARY KEY, label TEXT NOT NULL, rid INTEGER);
             CREATE TABLE product (pid INTEGER PRIMARY KEY, name TEXT NOT NULL, sid INTEGER);
             INSERT INTO region VALUES (1,'north'),(2,'east'),(3,'south'),(4,'west');",
        )
        .unwrap();
        for s in 1..=40 {
            conn.execute_batch(&format!(
                "INSERT INTO supplier VALUES ({s}, 'supplier {s}', {});",
                (s - 1) % 4 + 1
            ))
            .unwrap();
        }
        for p in 1..=10 {
            conn.execute_batch(&format!(
                "INSERT INTO product VALUES ({p}, 'product {p}', {});",
                if p == 10 { 40 } else { p }
            ))
            .unwrap();
        }
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

    fn sample(spec: SampleSpec) -> (TripleStore, SampleOutcome) {
        let mapping = parse_rml(&format!("{PFX}{JOINED}")).expect("mapping parses");
        let (_dir, mut conn) = db();
        let store = TripleStore::in_memory().unwrap();
        let outcome = execute_sample(
            &mapping,
            conn.as_mut(),
            &quote,
            &store,
            "urn:dryrun:test",
            "dry-1",
            &spec,
        )
        .expect("sample succeeds");
        (store, outcome)
    }

    fn ask(store: &TripleStore, pattern: &str) -> bool {
        matches!(
            store.query(&format!(
                "PREFIX ex: <http://example.org/> ASK {{ GRAPH <urn:dryrun:test> {{ {pattern} }} }}"
            )),
            Ok(QueryResults::Boolean(true))
        )
    }

    fn count(store: &TripleStore, pattern: &str) -> usize {
        let Ok(QueryResults::Solutions(s)) = store.query(&format!(
            "PREFIX ex: <http://example.org/> SELECT ?s WHERE {{ GRAPH <urn:dryrun:test> {{ {pattern} }} }}"
        )) else {
            return 0;
        };
        s.count()
    }

    fn map<'a>(o: &'a SampleOutcome, iri: &str) -> &'a SampledMap {
        o.maps
            .iter()
            .find(|m| m.triples_map == format!("http://example.org/{iri}"))
            .unwrap_or_else(|| panic!("no entry for {iri}: {o:?}"))
    }

    #[test]
    fn one_row_of_a_child_pulls_in_its_parent_and_the_parents_parent() {
        // Product 10's supplier is the 40th supplier: outside any head sample
        // of the supplier table, so it can only be there by pull-in — and its
        // region, in turn, only through the supplier that was pulled in.
        let (store, outcome) = sample(SampleSpec {
            limit: 1,
            triples_maps: vec!["http://example.org/Product".into()],
            ..Default::default()
        });
        // The head of the product table is product 1, supplier 1, region 1.
        assert_eq!(map(&outcome, "Product").sampled_rows, 1);
        assert_eq!(
            map(&outcome, "Supplier").sampled_rows,
            0,
            "not sampled itself"
        );
        assert_eq!(map(&outcome, "Supplier").pulled_in_rows, 1);
        assert_eq!(map(&outcome, "Region").pulled_in_rows, 1);
        assert!(ask(
            &store,
            "<http://example.org/p1> ex:supplier <http://example.org/s1> . \
             <http://example.org/s1> a ex:Supplier ; ex:region <http://example.org/r1> . \
             <http://example.org/r1> a ex:Region ; ex:name \"north\" ."
        ));
        assert_eq!(
            count(&store, "?s a ex:Supplier"),
            1,
            "only the referenced supplier"
        );
        assert_eq!(count(&store, "?s a ex:Region"), 1);
    }

    #[test]
    fn a_table_sample_reaches_a_parent_the_parents_own_head_would_miss() {
        let (store, outcome) = sample(SampleSpec {
            limit: 10,
            tables: vec!["product".into()],
            ..Default::default()
        });
        assert_eq!(map(&outcome, "Product").sampled_rows, 10);
        // Products 1–9 reference suppliers 1–9; product 10 references 40.
        assert_eq!(map(&outcome, "Supplier").pulled_in_rows, 10);
        assert!(ask(
            &store,
            "<http://example.org/p10> ex:supplier <http://example.org/s40> . \
             <http://example.org/s40> a ex:Supplier ; ex:label \"supplier 40\" ."
        ));
        assert_eq!(count(&store, "?s a ex:Region"), 4, "every region reached");
        assert_eq!(outcome.rows, 24, "10 products + 10 suppliers + 4 regions");
    }

    #[test]
    fn a_map_sampled_itself_and_pulled_in_holds_each_row_once() {
        // Every map sampled with a head of 3: suppliers 1–3 directly, and
        // suppliers 1–3 again through products 1–3 — held once, not twice.
        let (store, outcome) = sample(SampleSpec {
            limit: 3,
            ..Default::default()
        });
        assert_eq!(map(&outcome, "Supplier").sampled_rows, 3);
        assert_eq!(
            map(&outcome, "Supplier").pulled_in_rows,
            0,
            "the referenced suppliers were already in the head"
        );
        assert_eq!(count(&store, "?s a ex:Supplier"), 3);
        assert_eq!(
            count(&store, "<http://example.org/s1> ex:label ?l"),
            1,
            "no duplicate triples"
        );
        // A query-sourced parent is indexed rather than pushed down; both
        // paths resolve, and the region's own triples are present.
        assert!(ask(
            &store,
            "<http://example.org/s2> ex:region <http://example.org/r2> . \
             <http://example.org/r2> ex:name \"east\" ."
        ));
    }

    #[test]
    fn a_selection_nothing_reads_is_an_error_that_names_the_choices() {
        let mapping = parse_rml(&format!("{PFX}{JOINED}")).unwrap();
        let (_dir, mut conn) = db();
        let store = TripleStore::in_memory().unwrap();
        let err = execute_sample(
            &mapping,
            conn.as_mut(),
            &quote,
            &store,
            "urn:dryrun:test",
            "dry-2",
            &SampleSpec {
                limit: 5,
                tables: vec!["nothing".into()],
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.contains("nothing") && err.contains("product"), "{err}");
    }

    #[test]
    fn a_key_that_needs_escaping_is_matched_in_memory() {
        assert!(is_plain("abc-123"));
        assert!(is_plain("2026-01-01 10:00:00+01:00"));
        assert!(!is_plain("o'brien"));
        assert!(!is_plain("a;b"));
        assert!(!is_plain(""));
    }

    #[test]
    fn take_stops_after_the_sample_and_reports_a_short_table_whole() {
        let (_dir, mut conn) = db();
        assert_eq!(
            take(conn.as_mut(), "SELECT * FROM region", 2)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            take(conn.as_mut(), "SELECT * FROM region", 100)
                .unwrap()
                .len(),
            4
        );
        assert!(take(conn.as_mut(), "SELECT * FROM nope", 1).is_err());
    }
}
