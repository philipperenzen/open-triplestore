//! RML executor: runs a mapping against source data and produces RDF quads.
//!
//! Core flow:
//! ```text
//! LogicalSource → Iterator<Row>
//!   → TriplesMap: for each Row:
//!       SubjectMap.eval(row) → subject IRI/BNode
//!       for each PredicateObjectMap:
//!         PredicateMap.eval(row) → predicate IRI
//!         ObjectMap.eval(row) → object (IRI/Literal/BNode)
//!       → Quad(subject, predicate, object, graph)
//! ```
//!
//! This is the **file-based** path (CSV / JSON / XML), where every row is
//! self-contained. Relational sources stream through
//! [`super::sql`](super::sql) instead, because they can join across triples
//! maps; both share the term-map evaluation in [`super::terms`].

use super::model::*;
use super::sources::load_rows;
use super::terms::{BlankNodes, Row};
use crate::store::engine::TripleStore;
use std::collections::HashMap;

/// Execute an RML mapping, writing generated triples into `target_graph` in `store`.
///
/// `source_data` is a map from logical source identifier (file path / name) → content string.
/// Returns the number of triples inserted.
pub fn execute(
    mapping: &RmlMapping,
    source_data: &HashMap<String, String>,
    store: &TripleStore,
    target_graph: Option<&str>,
) -> Result<usize, String> {
    execute_authorized(mapping, source_data, store, target_graph, |_| Ok(()))
}

/// Like [`execute`], but `authorize` gates **every effective target graph** before
/// any write. A `rml:graphMap` (TriplesMap- or POM-level) overrides
/// `target_graph` per triple, so a caller-supplied mapping can name an arbitrary
/// destination graph; the dataset-scoped HTTP path passes an `authorize` that
/// keeps those targets inside the dataset's own graph boundary (preventing a
/// cross-tenant write). Authorization runs over the full resolved set *before*
/// the first insert, so a rejected mapping writes nothing.
pub fn execute_authorized<A>(
    mapping: &RmlMapping,
    source_data: &HashMap<String, String>,
    store: &TripleStore,
    target_graph: Option<&str>,
    authorize: A,
) -> Result<usize, String>
where
    A: Fn(&str) -> Result<(), String>,
{
    // A relational mapping needs a connection and a join planner, neither of
    // which this path has. Saying so beats emitting zero triples and calling
    // it success.
    if mapping.has_sql_source() {
        return Err(
            "this mapping reads a registered datasource; run it through \
             POST /api/sources/{id}/runs, which opens the connection and applies the write gate"
                .to_string(),
        );
    }

    // Triples keyed by their target named graph (None = default/target_graph).
    let mut triples_by_graph: HashMap<Option<String>, Vec<String>> = HashMap::new();
    let mut bnodes = BlankNodes::new("b");

    for tm in &mapping.triples_maps {
        let source_key = match &tm.logical_source.source {
            SourceRef::File(path) => path.clone(),
            SourceRef::Datasource(_) => unreachable!("guarded by has_sql_source above"),
        };

        execute_triples_map(
            tm,
            source_data,
            &source_key,
            &mut triples_by_graph,
            &mut bnodes,
        )?;
    }

    if triples_by_graph.is_empty() {
        return Ok(0);
    }

    // Authorize every effective destination graph up front, so a mapping whose
    // `rml:graphMap` targets a foreign graph is rejected before anything is
    // written (all-or-nothing).
    for (graph_key, triples) in &triples_by_graph {
        if triples.is_empty() {
            continue;
        }
        let effective_graph: Option<&str> = match graph_key {
            Some(g) => Some(g.as_str()),
            None => target_graph,
        };
        if let Some(g) = effective_graph {
            authorize(g)?;
        }
    }

    let mut total = 0;
    for (graph_key, triples) in &triples_by_graph {
        if triples.is_empty() {
            continue;
        }
        // graph_key overrides the caller-supplied target_graph when set
        let effective_graph: Option<&str> = match graph_key {
            Some(g) => Some(g.as_str()),
            None => target_graph,
        };
        let doc = triples.join("\n");
        store
            .load_str(&doc, oxigraph::io::RdfFormat::NTriples, effective_graph)
            .map_err(|e| format!("Failed to load generated triples: {e}"))?;
        total += triples.len();
    }

    Ok(total)
}

fn execute_triples_map(
    tm: &TriplesMap,
    source_data: &HashMap<String, String>,
    source_key: &str,
    out: &mut HashMap<Option<String>, Vec<String>>,
    bnodes: &mut BlankNodes,
) -> Result<(), String> {
    let content = source_data
        .get(source_key)
        .ok_or_else(|| format!("Source data not found for key: {source_key}"))?;

    let rows = load_rows(
        content,
        &tm.logical_source.reference_formulation,
        tm.logical_source.iterator.as_deref(),
    )?;

    for row_result in rows {
        let row = row_result?;
        execute_row(tm, &row, out, bnodes)?;
    }

    Ok(())
}

fn execute_row(
    tm: &TriplesMap,
    row: &Row,
    out: &mut HashMap<Option<String>, Vec<String>>,
    bnodes: &mut BlankNodes,
) -> Result<(), String> {
    // Blank nodes are scoped to a single row (R2RML §generated RDF term): two term
    // maps yielding the same value in this row must denote the SAME blank node, but
    // a fresh row produces distinct nodes. Keyed by the generated value; reset here
    // per row so cross-row blank nodes never collide.
    let mut row_bnodes: HashMap<String, String> = HashMap::new();

    // No column types (a file source reports none) and no join resolver: RML
    // join conditions need a queryable source, which is the relational path.
    for triple in super::sql::row_triples(tm, row, None, bnodes, &mut row_bnodes, &|_, _| None)? {
        out.entry(triple.graph).or_default().push(triple.text);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rml::parser::parse_rml;
    use oxigraph::sparql::QueryResults;

    const MAPPING: &str = r#"
        @prefix rr:   <http://www.w3.org/ns/r2rml#> .
        @prefix rml:  <http://semweb.mmlab.be/ns/rml#> .
        @prefix ql:   <http://semweb.mmlab.be/ns/ql#> .
        @prefix ex:   <http://example.org/> .
        @prefix foaf: <http://xmlns.com/foaf/0.1/> .

        ex:PersonMap a rr:TriplesMap ;
            rml:logicalSource ex:PeopleSource ;
            rr:subjectMap ex:PersonSubject ;
            rr:predicateObjectMap ex:NamePOM .
        ex:PeopleSource rml:source "people.csv" ; rml:referenceFormulation ql:CSV .
        ex:PersonSubject rr:template "http://example.org/person/{id}" .
        ex:NamePOM rr:predicate foaf:name ; rr:objectMap ex:NameObject .
        ex:NameObject rml:reference "name" .
    "#;

    #[test]
    fn csv_template_mapping_produces_expected_triples() {
        let mapping = parse_rml(MAPPING).expect("mapping parses");

        let mut sources = HashMap::new();
        sources.insert(
            "people.csv".to_string(),
            "id,name\n1,Alice\n2,Bob\n".to_string(),
        );

        let store = TripleStore::in_memory().unwrap();
        let inserted = execute(&mapping, &sources, &store, None).unwrap();

        // One foaf:name triple per CSV row.
        assert_eq!(inserted, 2, "expected one triple per row");
        assert_eq!(store.len().unwrap(), 2);

        // The {id} template substitution and the `name` column reference both
        // resolve: person/1 → "Alice", person/2 → "Bob".
        let results = store
            .query(
                "PREFIX foaf: <http://xmlns.com/foaf/0.1/> \
                 SELECT ?name WHERE { <http://example.org/person/1> foaf:name ?name }",
            )
            .unwrap();
        let QueryResults::Solutions(mut sols) = results else {
            panic!("expected SELECT solutions");
        };
        let row = sols.next().expect("one row").unwrap();
        assert_eq!(row.get("name").unwrap().to_string(), "\"Alice\"");
    }

    fn count(store: &TripleStore, q: &str) -> usize {
        match store.query(q).unwrap() {
            QueryResults::Solutions(s) => s.count(),
            _ => 0,
        }
    }

    #[test]
    fn inline_blank_node_term_maps_parse_and_execute() {
        // The standard authoring idiom: the logical source, subject map, both
        // predicate-object maps and their object maps are all INLINE blank nodes.
        // The loader must dereference each stored blank node individually — the old
        // SPARQL form matched every blank node and crossed the two POMs' predicates.
        let mapping = r#"
            @prefix rr:   <http://www.w3.org/ns/r2rml#> .
            @prefix rml:  <http://semweb.mmlab.be/ns/rml#> .
            @prefix ql:   <http://semweb.mmlab.be/ns/ql#> .
            @prefix ex:   <http://example.org/> .
            @prefix foaf: <http://xmlns.com/foaf/0.1/> .

            ex:M a rr:TriplesMap ;
              rml:logicalSource [ rml:source "people.csv" ; rml:referenceFormulation ql:CSV ] ;
              rr:subjectMap [ rr:template "http://example.org/person/{id}" ] ;
              rr:predicateObjectMap [ rr:predicate foaf:name ; rr:objectMap [ rml:reference "name" ] ] ;
              rr:predicateObjectMap [ rr:predicate foaf:age  ; rr:objectMap [ rml:reference "age"  ] ] .
        "#;
        let mapping = parse_rml(mapping).expect("inline blank-node mapping parses");
        let mut sources = HashMap::new();
        sources.insert(
            "people.csv".to_string(),
            "id,name,age\n1,Alice,30\n2,Bob,25\n".to_string(),
        );
        let store = TripleStore::in_memory().unwrap();
        let inserted = execute(&mapping, &sources, &store, None).unwrap();

        // Exactly the four correct triples — no cross-contamination between POMs.
        assert_eq!(inserted, 4, "2 rows x 2 predicate-object maps");
        assert_eq!(store.len().unwrap(), 4);
        let foaf = "PREFIX foaf: <http://xmlns.com/foaf/0.1/> ";
        assert_eq!(
            count(
                &store,
                &format!(
                    "{foaf} SELECT * WHERE {{ <http://example.org/person/1> foaf:name \"Alice\" }}"
                )
            ),
            1,
            "person/1 keeps its own name"
        );
        assert_eq!(
            count(
                &store,
                &format!(
                    "{foaf} SELECT * WHERE {{ <http://example.org/person/2> foaf:age \"25\" }}"
                )
            ),
            1,
            "person/2 keeps its own age"
        );
    }

    #[test]
    fn blank_node_term_maps_coreference_within_row() {
        // A subject term map and an object term map generate the same value as
        // rr:BlankNode: within a row they denote the SAME node (a self-edge), and a
        // different row denotes a DISTINCT node. The old counter minted a fresh node
        // per call, so the self-edge never closed.
        let mapping = r#"
            @prefix rr:   <http://www.w3.org/ns/r2rml#> .
            @prefix rml:  <http://semweb.mmlab.be/ns/rml#> .
            @prefix ql:   <http://semweb.mmlab.be/ns/ql#> .
            @prefix ex:   <http://example.org/> .
            @prefix foaf: <http://xmlns.com/foaf/0.1/> .

            ex:M a rr:TriplesMap ;
              rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;
              rr:predicateObjectMap ex:SelfPOM, ex:NamePOM .
            ex:Src rml:source "d.csv" ; rml:referenceFormulation ql:CSV .
            ex:Subj rr:template "n{id}" ; rr:termType rr:BlankNode .
            ex:SelfPOM rr:predicate ex:self ; rr:objectMap ex:SelfObj .
            ex:SelfObj rr:template "n{id}" ; rr:termType rr:BlankNode .
            ex:NamePOM rr:predicate foaf:name ; rr:objectMap ex:NameObj .
            ex:NameObj rml:reference "name" .
        "#;
        let mapping = parse_rml(mapping).expect("blank-node mapping parses");
        let mut sources = HashMap::new();
        sources.insert("d.csv".to_string(), "id,name\n1,Alice\n2,Bob\n".to_string());
        let store = TripleStore::in_memory().unwrap();
        execute(&mapping, &sources, &store, None).unwrap();

        // One closed self-edge per row, and the two rows' nodes stay distinct (two
        // self-edges, not one merged via a colliding label).
        assert_eq!(
            count(
                &store,
                "SELECT ?s WHERE { ?s <http://example.org/self> ?s }"
            ),
            2,
            "subject and same-valued object blank node co-refer within each row"
        );
        // …and the literal hangs off that same node.
        assert_eq!(
            count(
                &store,
                "PREFIX foaf: <http://xmlns.com/foaf/0.1/> \
                 SELECT ?s WHERE { ?s <http://example.org/self> ?s ; foaf:name \"Alice\" }"
            ),
            1
        );
    }
}
