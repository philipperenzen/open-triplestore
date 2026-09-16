//! The relational RML executor: a registered datasource as a logical source.
//!
//! Rows are streamed in batches and never materialised whole. Triples are
//! accumulated as N-Triples lines and flushed into the run's graph per batch,
//! so peak memory is one batch of rows plus one buffer of text.
//!
//! **Joins.** `rr:parentTriplesMap` is resolved with a hash join: the parent
//! triples map's logical source is streamed once, and each row's join-key
//! values are indexed to the subject term that map generates. The child then
//! streams and looks up. The index is bounded (`OTS_SOURCES_JOIN_MAX_ROWS`,
//! default 1 000 000 keys) and a mapping that would exceed it is refused with
//! a message naming the parent, rather than exhausting memory. Pushing the
//! join into the source query instead is a worthwhile optimisation for wide
//! parents; it is not implemented here, and the strategy is one function
//! (`build_parent_index`) so it stays a local change.

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
    let mut failure: Option<String> = None;

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
        overflow.clone().unwrap_or_else(|| {
            failure = Some(e.to_string());
            format!("reading join parent <{}>: {e}", parent.iri)
        })
    })?;

    Ok(index)
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
    let batch_size = batch_size.clamp(1, 100_000);
    // Blank-node labels carry the run id, so two runs' graphs never share a
    // node and a batch-by-batch load never merges rows.
    let mut bnodes = BlankNodes::new(format!("r{}_", sanitise_label(run_id)));
    let mut outcome = SqlOutcome::default();

    // Index every parent referenced from anywhere in the mapping, once.
    let mut indexes: HashMap<(String, Vec<(String, String)>), ParentIndex> = HashMap::new();
    for tm in &mapping.triples_maps {
        for pom in &tm.predicate_object_maps {
            let ObjectMap::Ref(r) = &pom.object else {
                continue;
            };
            let key = index_key(r);
            if indexes.contains_key(&key) {
                continue;
            }
            let parent = mapping
                .find(&r.parent_triples_map)
                .ok_or_else(|| format!("unknown parent TriplesMap <{}>", r.parent_triples_map))?;
            let index = build_parent_index(parent, &r.joins, conn, quote, &mut bnodes)?;
            indexes.insert(key, index);
        }
    }

    let mut buffer = String::with_capacity(FLUSH_BYTES / 4);
    let mut row: Row = HashMap::new();
    let mut kinds: Kinds = HashMap::new();

    for tm in &mapping.triples_maps {
        let Some(sql) = tm.logical_source.sql(quote) else {
            return Err(format!(
                "TriplesMap <{}> has no relational logical source",
                tm.iri
            ));
        };

        let mut emit_error: Option<String> = None;
        let mut triples: u64 = 0;
        let rows = conn
            .stream(&sql, batch_size, &mut |batch| {
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
                            let index = indexes.get(&index_key(r))?;
                            let child_columns: Vec<String> =
                                r.joins.iter().map(|j| j.child.clone()).collect();
                            if child_columns.is_empty() {
                                // A join-less reference over the same logical
                                // source: every parent row matches.
                                return Some(index.values().flatten().cloned().collect());
                            }
                            let key = join_key(child_row, &child_columns)?;
                            index.get(&key).cloned()
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

fn index_key(r: &RefObjectMap) -> (String, Vec<(String, String)>) {
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
             INSERT INTO supplier VALUES (1,'Acme'), (2,'Globex');
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
        let c = crate::sources::sqlite::SqliteConnector.connect(&params).unwrap();
        (dir, c)
    }

    fn quote(t: &str) -> String {
        format!("\"{}\"", t.replace('"', "\"\""))
    }

    /// `OTS_SOURCES_JOIN_MAX_ROWS` is process-wide, so the test that lowers it
    /// must not overlap a test that relies on the default. Every test that
    /// executes a mapping takes this lock.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
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
        assert!(ask(&store, "<http://example.org/p10> ex:supplier <http://example.org/s1> ."));
        assert!(ask(&store, "<http://example.org/p11> ex:supplier <http://example.org/s1> ."));
        assert!(ask(&store, "<http://example.org/s1> a ex:Supplier ; ex:label \"Acme\" ."));
        // A NULL foreign key joins to nothing, so no triple is emitted.
        assert!(!ask(&store, "<http://example.org/p12> ex:supplier ?o ."));
    }

    #[test]
    fn a_null_column_produces_no_triple_and_integers_are_typed_naturally() {
        let (store, _) = run(JOINED, 100);
        assert!(ask(
            &store,
            "<http://example.org/p10> ex:qty \"5\"^^<http://www.w3.org/2001/XMLSchema#integer> ."
        ));
        assert!(!ask(&store, "<http://example.org/p11> ex:qty ?q ."));
        assert!(ask(&store, "<http://example.org/p10> ex:name \"Bolt\" ."), "TEXT stays plain");
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
        assert!(ask(&store, "<http://example.org/p10> ex:status <http://example.org/Active> ."));
        assert!(
            ask(&store, "<http://example.org/p11> ex:status <http://example.org/Active> ."),
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
        let err = execute_relational(&mapping, conn.as_mut(), &quote, &store, "urn:run:t", 10, "r")
            .unwrap_err();
        assert!(err.contains("fn:mapping"), "the mapping error survives the stream: {err}");
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
        let _guard = env_guard();
        std::env::set_var(JOIN_MAX_ROWS_ENV, "1");
        let mapping = parse_rml(&format!("{PFX}{JOINED}")).unwrap();
        let (_dir, mut conn) = db();
        let store = TripleStore::in_memory().unwrap();
        let err = execute_relational(&mapping, conn.as_mut(), &quote, &store, "urn:run:t", 10, "r")
            .unwrap_err();
        std::env::remove_var(JOIN_MAX_ROWS_ENV);
        assert!(err.contains("Supplier") && err.contains(JOIN_MAX_ROWS_ENV), "{err}");
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
        assert_eq!(join_key(&row, &["a".to_string()]), Some(vec!["1".to_string()]));
        assert_eq!(join_key(&row, &["a".to_string(), "b".to_string()]), None);
    }
}
