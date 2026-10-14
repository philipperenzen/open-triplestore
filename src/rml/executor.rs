//! RML executor: runs a mapping against source data and produces RDF quads.
//!
//! Core flow:
//! ```text
//! LogicalSource → bytes (decompressed, decoded) → logical iterations
//!   → TriplesMap: for each iteration:
//!       SubjectMap.eval(it) → subject IRIs / blank nodes
//!       for each PredicateObjectMap:
//!         PredicateMap.eval(it) → predicate IRIs
//!         ObjectMap.eval(it) → objects (IRI/Literal/BNode)
//!       → Quad(subject, predicate, object, graph) for every combination
//! ```
//!
//! This is the **file-based** path (CSV / JSON / XML). Relational sources
//! stream through [`super::sql`](super::sql) instead; both share the
//! term-map evaluation in [`super::terms`] and the join rules: a referencing
//! object map with join conditions resolves through an index of the parent's
//! iterations ([`ParentIndexBuilder`]), built once per parent and join, and
//! one without resolves from the child's own iteration (R2RML §8).
//!
//! **Sources.** A logical source names its file by `rml:source "name"`, by an
//! RML-IO `rml:RelativePathSource` / `rml:FilePath` (`rml:path`), or by a
//! CSVW `csvw:Table` (`csvw:url`). The name is looked up among the files the
//! run was given: as written, then without a leading `./`, then by its last
//! path segment. A remote URL is never fetched: a run that needs one is given
//! it as a file of that name.

use super::checks::{check_columns, needed_columns, DataErrors, OnDataError};
use super::model::*;
use super::sources::ReadAs;
use super::sql::{index_key, lookup, same_row_subject, ParentIndex, ParentIndexBuilder, RefKey};
use super::terms::{Iteration, TermGen};
use crate::store::engine::TripleStore;
use std::cell::RefCell;
use std::collections::HashMap;

/// What a file-mapping run wrote, and the rows it skipped terms from.
#[derive(Debug, Clone, Default)]
pub struct FileOutcome {
    pub triples: usize,
    /// Empty unless the run skipped data errors ([`OnDataError::Skip`]).
    pub data_errors: DataErrors,
}

/// Execute an RML mapping, writing generated triples into `target_graph` in `store`.
///
/// `source_data` maps a logical source's file name to its bytes. `authorize`
/// gates **every effective target graph** before any write. A graph map (on a
/// subject map or a predicate-object map) sends triples to a graph other than
/// `target_graph`, so a caller-supplied mapping can name an arbitrary
/// destination graph; the dataset-scoped HTTP path passes an `authorize` that
/// keeps those targets inside the dataset's own graph boundary (preventing a
/// cross-tenant write). Authorization runs over the full resolved set *before*
/// the first insert, so a rejected mapping writes nothing.
///
/// A column the mapping names that a CSV header lacks is an error before the
/// first row. A row value that cannot become its term (R2RML §4.3) aborts
/// the run with the offending rows named, unless `on_data_error` is
/// [`OnDataError::Skip`]; either way an aborted run writes nothing.
pub fn execute_with<S, A>(
    mapping: &RmlMapping,
    source_data: &HashMap<String, S>,
    store: &TripleStore,
    target_graph: Option<&str>,
    on_data_error: OnDataError,
    authorize: A,
) -> Result<FileOutcome, String>
where
    S: AsRef<[u8]>,
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
    let sources = Sources::new(mapping, source_data)?;

    // Triples keyed by their target named graph (None = default/target_graph).
    let mut triples_by_graph: HashMap<Option<String>, Vec<String>> = HashMap::new();
    let mut gen = TermGen::new(mapping.semantics, "b");
    let mut data_errors = DataErrors::default();
    let indexes = build_indexes(mapping, &sources, &mut gen)?;

    for tm in &mapping.triples_maps {
        execute_triples_map(
            mapping,
            tm,
            &sources,
            &indexes,
            &mut triples_by_graph,
            &mut gen,
            on_data_error,
            &mut data_errors,
        )?;
        if on_data_error == OnDataError::Abort && !data_errors.is_empty() {
            return Err(data_errors.abort_message());
        }
    }

    if triples_by_graph.is_empty() {
        return Ok(FileOutcome {
            triples: 0,
            data_errors,
        });
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

    Ok(FileOutcome {
        triples: total,
        data_errors,
    })
}

/// The files a run was given, looked up by the names its logical sources use.
struct Sources<'a> {
    mapping: &'a RmlMapping,
    files: HashMap<&'a str, &'a [u8]>,
    /// Each triples map's iterations, read once and shared by the map's own
    /// run and every join index that needs them.
    cache: RefCell<HashMap<String, std::rc::Rc<Vec<Iteration>>>>,
}

impl<'a> Sources<'a> {
    fn new<S: AsRef<[u8]>>(
        mapping: &'a RmlMapping,
        data: &'a HashMap<String, S>,
    ) -> Result<Self, String> {
        Ok(Self {
            mapping,
            files: data.iter().map(|(k, v)| (k.as_str(), v.as_ref())).collect(),
            cache: RefCell::new(HashMap::new()),
        })
    }

    /// The bytes of the file a logical source names.
    fn file(&self, name: &str) -> Result<&'a [u8], String> {
        let candidates = [
            Some(name),
            name.strip_prefix("./"),
            name.rsplit('/').next().filter(|b| !b.is_empty()),
        ];
        for c in candidates.into_iter().flatten() {
            if let Some(d) = self.files.get(c) {
                return Ok(*d);
            }
        }
        if name.starts_with("http://") || name.starts_with("https://") {
            return Err(format!(
                "the logical source names the remote file <{name}>; this engine does not fetch \
                 sources over the network — supply it as a part named \"{name}\""
            ));
        }
        let mut names: Vec<&&str> = self.files.keys().collect();
        names.sort();
        Err(format!(
            "Source data not found for key: {name} (the run was given {})",
            if names.is_empty() {
                "no files".to_string()
            } else {
                names
                    .iter()
                    .map(|n| format!("\"{n}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ))
    }

    /// The logical iterations of `tm`, after checking the columns the
    /// mapping names against the source's own (a CSV header).
    fn iterations(&self, tm: &TriplesMap) -> Result<std::rc::Rc<Vec<Iteration>>, String> {
        if let Some(cached) = self.cache.borrow().get(&tm.iri) {
            return Ok(cached.clone());
        }
        let mapping = self.mapping;
        let name = match &tm.logical_source.source {
            SourceRef::File(path) => path.as_str(),
            SourceRef::Datasource(_) => unreachable!("guarded by has_sql_source"),
        };
        let data = self.file(name)?;
        let references = iteration_references(mapping, tm);
        let loaded = super::sources::load(
            data,
            name,
            &tm.logical_source,
            &references,
            ReadAs {
                rml_core: mapping.rml_core,
                empty_is_value: mapping.semantics == Semantics::R2rml,
            },
        )?;
        // A CSV header is the source's column list: a column the mapping
        // names that it lacks is a mapping error, not a run of empty rows.
        // JSON and XML records carry no fixed set, and a missing key there is
        // a NULL.
        if let Some(columns) = &loaded.columns {
            check_columns(mapping, tm, columns)?;
        }
        let its = std::rc::Rc::new(loaded.iterations);
        self.cache.borrow_mut().insert(tm.iri.clone(), its.clone());
        Ok(its)
    }
}

/// Every reference expression an iteration of `tm` is read for: what its
/// own term maps, functions and joins read, and the subject references of
/// each parent it reaches without a join condition — that subject is built
/// from `tm`'s own iteration.
fn iteration_references(mapping: &RmlMapping, tm: &TriplesMap) -> Vec<String> {
    let mut out: Vec<String> = needed_columns(mapping, tm)
        .into_iter()
        .map(|n| n.column)
        .collect();
    let mut extra: Vec<String> = Vec::new();
    for r in tm.refs().filter(|r| r.joins.is_empty()) {
        let Some(parent) = mapping.find(&r.parent_triples_map) else {
            continue;
        };
        extra.extend(parent.subject_map.term_map.referenced_columns());
        if let Some(f) = &parent.subject_map.function {
            for a in f.params.values().flatten() {
                if let FunctionArg::Reference(c) = a {
                    extra.push(c.clone());
                }
            }
        }
    }
    // A `{x_slug}` placeholder of `otsfn:mintIri` reads `x`.
    let slugs: Vec<String> = out
        .iter()
        .chain(extra.iter())
        .filter_map(|c| c.strip_suffix("_slug").map(str::to_string))
        .collect();
    extra.extend(slugs);
    for c in extra {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn execute_triples_map(
    mapping: &RmlMapping,
    tm: &TriplesMap,
    sources: &Sources<'_>,
    indexes: &HashMap<RefKey, ParentIndex>,
    out: &mut HashMap<Option<String>, Vec<String>>,
    gen: &mut TermGen,
    on_data_error: OnDataError,
    data_errors: &mut DataErrors,
) -> Result<(), String> {
    let iterations = sources.iterations(tm)?;
    let base = mapping.base_for(tm);
    // A join-less parent's subject is computed from the child's iteration
    // with a generator of its own, as the relational executor does.
    let parent_gen = RefCell::new(TermGen::new(mapping.semantics, "b"));
    for (i, it) in iterations.iter().enumerate() {
        let mut it = it.clone();
        gen.apply_nulls_to(&tm.logical_source, &mut it);
        execute_iteration(mapping, tm, &it, out, gen, &parent_gen, indexes, base)?;
        data_errors.record(&tm.iri, i as u64 + 1, gen.take_errors());
        // An aborting run reads on only to name a few more offending rows.
        if on_data_error == OnDataError::Abort && data_errors.sample_full() {
            break;
        }
    }

    Ok(())
}

/// One index per distinct (parent, join conditions) a referencing object map
/// with join conditions names: the parent's iterations read once, keyed by
/// the parent side of the join. Built before the first triple, so an index
/// over the `OTS_SOURCES_JOIN_MAX_ROWS` cap fails the run before anything is
/// generated.
fn build_indexes(
    mapping: &RmlMapping,
    sources: &Sources<'_>,
    gen: &mut TermGen,
) -> Result<HashMap<RefKey, ParentIndex>, String> {
    let mut indexes: HashMap<RefKey, ParentIndex> = HashMap::new();
    for tm in &mapping.triples_maps {
        for r in tm.refs().filter(|r| !r.joins.is_empty()) {
            let key = index_key(r);
            if indexes.contains_key(&key) {
                continue;
            }
            let parent = mapping
                .find(&r.parent_triples_map)
                .ok_or_else(|| format!("unknown parent TriplesMap <{}>", r.parent_triples_map))?;
            let mut builder = ParentIndexBuilder::new(parent, &r.joins, mapping.base_for(parent));
            for it in sources.iterations(parent)?.iter() {
                let mut it = it.clone();
                gen.apply_nulls_to(&parent.logical_source, &mut it);
                builder.push_record(&it, gen)?;
            }
            indexes.insert(key, builder.finish());
        }
    }
    // A parent row's data error is reported when the parent's own rows run.
    gen.take_errors();
    Ok(indexes)
}

#[allow(clippy::too_many_arguments)]
fn execute_iteration(
    mapping: &RmlMapping,
    tm: &TriplesMap,
    it: &Iteration,
    out: &mut HashMap<Option<String>, Vec<String>>,
    gen: &mut TermGen,
    parent_gen: &RefCell<TermGen>,
    indexes: &HashMap<RefKey, ParentIndex>,
    base: Option<&str>,
) -> Result<(), String> {
    // Two term maps yielding the same blank-node value in this iteration
    // denote the SAME node. Under R2RML so does the same value in another
    // iteration of the same graph; a legacy mapping mints afresh per row,
    // which this resets.
    gen.start_row();

    let triples = super::sql::row_triples(tm, it, gen, base, &|r, child: &Iteration, at| {
        if r.joins.is_empty() {
            same_row_subject(mapping, r, child, &mut parent_gen.borrow_mut(), at)
        } else {
            lookup(indexes.get(&index_key(r))?, r, child)
        }
    })?;
    parent_gen.borrow_mut().take_errors();
    for triple in triples {
        out.entry(triple.graph).or_default().push(triple.text);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rml::parser::parse_rml;
    use oxigraph::sparql::QueryResults;

    fn execute(
        mapping: &RmlMapping,
        source_data: &HashMap<String, String>,
        store: &TripleStore,
        target_graph: Option<&str>,
    ) -> Result<usize, String> {
        execute_with(
            mapping,
            source_data,
            store,
            target_graph,
            OnDataError::Abort,
            |_| Ok(()),
        )
        .map(|o| o.triples)
    }

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

    #[test]
    fn a_parent_triples_map_on_file_sources_resolves_the_link() {
        // Resolving the reference to nothing used to write the person, drop
        // the link to the organisation, and report success; then the file
        // executor refused the mapping. Now the parent's rows are indexed.
        let mapping = parse_rml(
            r#"
            @prefix rr:  <http://www.w3.org/ns/r2rml#> .
            @prefix rml: <http://semweb.mmlab.be/ns/rml#> .
            @prefix ql:  <http://semweb.mmlab.be/ns/ql#> .
            @prefix ex:  <http://example.org/> .

            ex:PersonMap a rr:TriplesMap ;
                rml:logicalSource [ rml:source "people.csv" ; rml:referenceFormulation ql:CSV ] ;
                rr:subjectMap [ rr:template "http://example.org/person/{id}" ] ;
                rr:predicateObjectMap [
                    rr:predicate ex:worksFor ;
                    rr:objectMap [
                        rr:parentTriplesMap ex:OrgMap ;
                        rr:joinCondition [ rr:child "org" ; rr:parent "id" ]
                    ]
                ] .
            ex:OrgMap a rr:TriplesMap ;
                rml:logicalSource [ rml:source "orgs.csv" ; rml:referenceFormulation ql:CSV ] ;
                rr:subjectMap [ rr:template "http://example.org/org/{id}" ] .
        "#,
        )
        .expect("mapping parses");

        let mut sources = HashMap::new();
        sources.insert(
            "people.csv".to_string(),
            "id,org\n1,7\n2,8\n3,7\n".to_string(),
        );
        sources.insert("orgs.csv".to_string(), "id\n7\n9\n".to_string());

        let store = TripleStore::in_memory().unwrap();
        let inserted = execute(&mapping, &sources, &store, None).unwrap();
        assert_eq!(
            inserted, 2,
            "persons 1 and 3 work for org 7; org 8 is absent"
        );
        assert_eq!(
            count(
                &store,
                "SELECT ?p WHERE { ?p <http://example.org/worksFor> <http://example.org/org/7> }"
            ),
            2
        );
        assert_eq!(
            count(
                &store,
                "SELECT * WHERE { <http://example.org/person/2> ?p ?o }"
            ),
            0,
            "a key with no parent row joins to nothing"
        );
    }

    #[test]
    fn a_join_less_reference_takes_the_parent_subject_of_the_same_row() {
        // R2RML §8: no join condition joins each row to itself — not every
        // parent row to every child row.
        let mapping = parse_rml(
            r#"
            @prefix rr:  <http://www.w3.org/ns/r2rml#> .
            @prefix rml: <http://semweb.mmlab.be/ns/rml#> .
            @prefix ql:  <http://semweb.mmlab.be/ns/ql#> .
            @prefix ex:  <http://example.org/> .

            ex:Person a rr:TriplesMap ;
                rml:logicalSource [ rml:source "p.csv" ; rml:referenceFormulation ql:CSV ] ;
                rr:subjectMap [ rr:template "http://example.org/person/{id}" ] ;
                rr:predicateObjectMap [
                    rr:predicate ex:address ;
                    rr:objectMap [ rr:parentTriplesMap ex:Address ]
                ] .
            ex:Address a rr:TriplesMap ;
                rml:logicalSource [ rml:source "p.csv" ; rml:referenceFormulation ql:CSV ] ;
                rr:subjectMap [ rr:template "http://example.org/address/{city}" ] .
        "#,
        )
        .expect("mapping parses");
        let mut sources = HashMap::new();
        sources.insert(
            "p.csv".to_string(),
            "id,city\n1,Ghent\n2,Delft\n".to_string(),
        );
        let store = TripleStore::in_memory().unwrap();
        assert_eq!(execute(&mapping, &sources, &store, None).unwrap(), 2);
        assert_eq!(
            count(
                &store,
                "SELECT * WHERE { <http://example.org/person/1> <http://example.org/address> \
                 <http://example.org/address/Ghent> }"
            ),
            1
        );
        assert_eq!(
            count(
                &store,
                "SELECT * WHERE { <http://example.org/person/1> <http://example.org/address> \
                 <http://example.org/address/Delft> }"
            ),
            0,
            "person 1's row is not joined to person 2's"
        );
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
