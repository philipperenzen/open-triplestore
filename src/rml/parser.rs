//! RML mapping document parser.
//!
//! Reads an RML mapping stored as triples in the triple store (or loaded
//! from a Turtle string into an in-memory store) and builds an `RmlMapping`.

use std::collections::BTreeMap;

use super::model::*;
use crate::store::engine::TripleStore;

const RR: &str = "http://www.w3.org/ns/r2rml#";
const RML: &str = "http://semweb.mmlab.be/ns/rml#";
const FNML: &str = "http://semweb.mmlab.be/ns/fnml#";
/// FnO moved host; both spellings of `fno:executes` are accepted.
const FNO_EXECUTES: [&str; 2] = [
    "https://w3id.org/function/ontology#executes",
    "http://w3id.org/function/ontology#executes",
];

/// The IRI prefix a `rml:source` carries when it names a registered
/// datasource rather than a file.
pub const DATASOURCE_PREFIX: &str = "urn:source:";

/// Parse RML mappings from Turtle text into an `RmlMapping`.
pub fn parse_rml(turtle: &str) -> Result<RmlMapping, String> {
    let store =
        TripleStore::in_memory().map_err(|e| format!("Failed to create temp store: {e}"))?;
    store
        .load_str(turtle, oxigraph::io::RdfFormat::Turtle, None)
        .map_err(|e| format!("Failed to parse RML Turtle: {e}"))?;
    parse_from_store(&store, None)
}

/// Parse RML mappings from a named graph in an existing store.
pub fn parse_from_store(store: &TripleStore, graph: Option<&str>) -> Result<RmlMapping, String> {
    let graph_clause = match graph {
        Some(g) => format!("GRAPH <{}> {{", crate::store::escape_sparql_iri(g)),
        None => String::new(),
    };
    let graph_close = if graph.is_some() { "}" } else { "" };

    // Find all TriplesMap IRIs
    let q = format!("SELECT ?tm WHERE {{ {graph_clause} ?tm a <{RR}TriplesMap> {graph_close} }}");
    let mut tm_iris = query_col(store, &q, "tm");
    // Deterministic order: a mapping's triples maps run (and their parent
    // indexes build) in a stable sequence, so two runs of the same mapping
    // produce the same blank-node labelling and the same log.
    tm_iris.sort();
    tm_iris.dedup();

    let mut triples_maps = Vec::new();
    for tm_iri in &tm_iris {
        match parse_triples_map(store, tm_iri, graph) {
            Ok(tm) => triples_maps.push(tm),
            Err(e) => return Err(format!("Error in TriplesMap <{tm_iri}>: {e}")),
        }
    }
    if triples_maps.is_empty() {
        return Err("the mapping declares no rr:TriplesMap".to_string());
    }

    let mapping = RmlMapping { triples_maps };
    validate_references(&mapping)?;
    Ok(mapping)
}

/// Every `rr:parentTriplesMap` must name a triples map that exists, and a
/// join-less reference is only legal when both sides read the same logical
/// source (R2RML §10.3). Catching it here turns a silent "no triples" into a
/// mapping error the author sees at upload.
fn validate_references(mapping: &RmlMapping) -> Result<(), String> {
    for tm in &mapping.triples_maps {
        for pom in &tm.predicate_object_maps {
            let ObjectMap::Ref(r) = &pom.object else {
                continue;
            };
            let parent = mapping.find(&r.parent_triples_map).ok_or_else(|| {
                format!(
                    "TriplesMap <{}> references parent <{}>, which the mapping does not define",
                    tm.iri, r.parent_triples_map
                )
            })?;
            if r.joins.is_empty()
                && !same_logical_source(&tm.logical_source, &parent.logical_source)
            {
                return Err(format!(
                    "TriplesMap <{}> references parent <{}> with no rr:joinCondition, but they \
                     read different logical sources",
                    tm.iri, r.parent_triples_map
                ));
            }
        }
    }
    Ok(())
}

fn same_logical_source(a: &LogicalSource, b: &LogicalSource) -> bool {
    a.source == b.source && a.query == b.query && a.table_name == b.table_name
}

fn parse_triples_map(
    store: &TripleStore,
    tm_iri: &str,
    graph: Option<&str>,
) -> Result<TriplesMap, String> {
    let g = |pred: &str| -> Vec<String> { get_objects(store, tm_iri, pred, graph) };

    // Logical source (rml:logicalSource, or R2RML's rr:logicalTable)
    let ls_iri = g(&format!("{RML}logicalSource"))
        .into_iter()
        .next()
        .or_else(|| g(&format!("{RR}logicalTable")).into_iter().next())
        .ok_or("Missing rml:logicalSource")?;
    let logical_source = parse_logical_source(store, &ls_iri, graph)?;

    let subject_map = parse_subject_map(store, tm_iri, graph)?;

    let pom_nodes = g(&format!("{RR}predicateObjectMap"));
    let mut predicate_object_maps = Vec::new();
    for pom_node in &pom_nodes {
        predicate_object_maps.push(parse_pom(store, pom_node, graph)?);
    }

    let graph_map = parse_optional_term_map(store, tm_iri, &format!("{RR}graphMap"), graph);

    Ok(TriplesMap {
        iri: tm_iri.to_string(),
        logical_source,
        subject_map,
        predicate_object_maps,
        graph_map,
    })
}

fn parse_logical_source(
    store: &TripleStore,
    ls_iri: &str,
    graph: Option<&str>,
) -> Result<LogicalSource, String> {
    let get = |pred: &str| get_objects(store, ls_iri, pred, graph);

    // `rr:tableName` / `rml:query` / `rr:sqlQuery` describe a relational source.
    let table_name = get(&format!("{RR}tableName")).into_iter().next();
    let query = get(&format!("{RML}query"))
        .into_iter()
        .next()
        .or_else(|| get(&format!("{RR}sqlQuery")).into_iter().next());

    // rml:source — a datasource IRI, a file path/URL, or inline data.
    let source_terms = get_objects_typed(store, ls_iri, &format!("{RML}source"), graph);
    let source = match source_terms.into_iter().next() {
        Some((value, true)) if value.starts_with(DATASOURCE_PREFIX) => SourceRef::Datasource(value),
        Some((value, _)) => SourceRef::File(value),
        // R2RML's `rr:logicalTable` names no source: the datasource is the
        // one the run supplies. Only legal with a table or query.
        None if table_name.is_some() || query.is_some() => SourceRef::Datasource(String::new()),
        None => return Err("Missing rml:source".to_string()),
    };

    let mut formulation = get(&format!("{RML}referenceFormulation"))
        .into_iter()
        .next()
        .map(|iri| ReferenceFormulation::from_iri(&iri))
        .unwrap_or(ReferenceFormulation::Csv);
    // A datasource source IS relational, whatever (if anything) the document
    // declared: the reference formulation is advisory, the source is not.
    if matches!(source, SourceRef::Datasource(_)) {
        formulation = ReferenceFormulation::Sql;
    }
    if formulation == ReferenceFormulation::Sql && query.is_none() && table_name.is_none() {
        return Err(
            "a relational logical source needs rr:tableName or rml:query / rr:sqlQuery".to_string(),
        );
    }

    let iterator = get(&format!("{RML}iterator")).into_iter().next();

    Ok(LogicalSource {
        source,
        reference_formulation: formulation,
        iterator,
        query,
        table_name,
    })
}

fn parse_subject_map(
    store: &TripleStore,
    tm_iri: &str,
    graph: Option<&str>,
) -> Result<SubjectMap, String> {
    let sm_node = get_objects(store, tm_iri, &format!("{RR}subjectMap"), graph)
        .into_iter()
        .next();
    let term_map = if let Some(ref sm) = sm_node {
        parse_term_map(store, sm, graph, TermType::IRI)?
    } else {
        let subjects = get_objects(store, tm_iri, &format!("{RR}subject"), graph);
        let val = subjects
            .into_iter()
            .next()
            .ok_or("Missing rr:subjectMap or rr:subject")?;
        TermMap {
            kind: TermMapKind::Constant(val),
            term_type: TermType::IRI,
            datatype: None,
            language: None,
        }
    };

    // rr:class assertions. R2RML places rr:class on the subjectMap; also accept it on
    // the TriplesMap as a convenience.
    let mut classes = Vec::new();
    if let Some(ref sm) = sm_node {
        classes.extend(get_objects(store, sm, &format!("{RR}class"), graph));
    }
    classes.extend(get_objects(store, tm_iri, &format!("{RR}class"), graph));
    classes.sort();
    classes.dedup();

    Ok(SubjectMap { term_map, classes })
}

fn parse_pom(
    store: &TripleStore,
    pom_node: &str,
    graph: Option<&str>,
) -> Result<PredicateObjectMap, String> {
    // Predicate map
    let pm_nodes = get_objects(store, pom_node, &format!("{RR}predicateMap"), graph);
    let predicate_map = if let Some(pm) = pm_nodes.into_iter().next() {
        parse_term_map(store, &pm, graph, TermType::IRI)?
    } else {
        let pred = get_objects(store, pom_node, &format!("{RR}predicate"), graph)
            .into_iter()
            .next()
            .ok_or("Missing rr:predicateMap or rr:predicate")?;
        TermMap {
            kind: TermMapKind::Constant(pred),
            term_type: TermType::IRI,
            datatype: None,
            language: None,
        }
    };

    // Object map: a referencing map, a function, or a plain term.
    let om_nodes = get_objects(store, pom_node, &format!("{RR}objectMap"), graph);
    let object = if let Some(om) = om_nodes.into_iter().next() {
        parse_object_map(store, &om, graph)?
    } else {
        let obj = get_objects(store, pom_node, &format!("{RR}object"), graph)
            .into_iter()
            .next()
            .ok_or("Missing rr:objectMap or rr:object")?;
        ObjectMap::Term(TermMap {
            kind: TermMapKind::Constant(obj),
            term_type: TermType::Literal,
            datatype: None,
            language: None,
        })
    };

    let graph_map = parse_optional_term_map(store, pom_node, &format!("{RR}graphMap"), graph);

    Ok(PredicateObjectMap {
        predicate_map,
        object,
        graph_map,
    })
}

fn parse_object_map(
    store: &TripleStore,
    om: &str,
    graph: Option<&str>,
) -> Result<ObjectMap, String> {
    if let Some(parent) = get_objects(store, om, &format!("{RR}parentTriplesMap"), graph)
        .into_iter()
        .next()
    {
        let mut joins = Vec::new();
        for jc in get_objects(store, om, &format!("{RR}joinCondition"), graph) {
            let child = get_objects(store, &jc, &format!("{RR}child"), graph)
                .into_iter()
                .next()
                .ok_or("rr:joinCondition is missing rr:child")?;
            let parent_col = get_objects(store, &jc, &format!("{RR}parent"), graph)
                .into_iter()
                .next()
                .ok_or("rr:joinCondition is missing rr:parent")?;
            joins.push(JoinCondition {
                child,
                parent: parent_col,
            });
        }
        joins.sort_by(|a, b| (&a.child, &a.parent).cmp(&(&b.child, &b.parent)));
        return Ok(ObjectMap::Ref(RefObjectMap {
            parent_triples_map: parent,
            joins,
        }));
    }

    if let Some(fv) = get_objects(store, om, &format!("{FNML}functionValue"), graph)
        .into_iter()
        .next()
    {
        return parse_function_map(store, om, &fv, graph);
    }

    parse_term_map(store, om, graph, TermType::Literal).map(ObjectMap::Term)
}

fn parse_function_map(
    store: &TripleStore,
    om: &str,
    fv: &str,
    graph: Option<&str>,
) -> Result<ObjectMap, String> {
    let mut params: BTreeMap<String, Vec<FunctionArg>> = BTreeMap::new();
    let mut function: Option<String> = None;

    for pom in get_objects(store, fv, &format!("{RR}predicateObjectMap"), graph) {
        let predicate = get_objects(store, &pom, &format!("{RR}predicate"), graph)
            .into_iter()
            .next()
            .or_else(|| {
                get_objects(store, &pom, &format!("{RR}predicateMap"), graph)
                    .into_iter()
                    .next()
                    .and_then(|pm| {
                        get_objects(store, &pm, &format!("{RR}constant"), graph)
                            .into_iter()
                            .next()
                    })
            })
            .ok_or("a function parameter is missing rr:predicate")?;

        // `rr:object` is a constant; `rr:objectMap [ rr:column … ]` reads the row.
        let mut args: Vec<FunctionArg> = get_objects(store, &pom, &format!("{RR}object"), graph)
            .into_iter()
            .map(FunctionArg::Constant)
            .collect();
        for om_node in get_objects(store, &pom, &format!("{RR}objectMap"), graph) {
            let tm = parse_term_map(store, &om_node, graph, TermType::Literal)?;
            args.push(match tm.kind {
                TermMapKind::Reference(c) => FunctionArg::Reference(c),
                TermMapKind::Constant(c) => FunctionArg::Constant(c),
                TermMapKind::Template(t) => FunctionArg::Constant(t),
            });
        }
        if FNO_EXECUTES.contains(&predicate.as_str()) {
            function = args
                .iter()
                .find_map(|a| match a {
                    FunctionArg::Constant(c) => Some(c.clone()),
                    FunctionArg::Reference(_) => None,
                })
                .or(function);
            continue;
        }
        params.entry(predicate).or_default().extend(args);
    }

    let function = function.ok_or("fnml:functionValue is missing fno:executes")?;
    let datatype = get_objects(store, om, &format!("{RR}datatype"), graph)
        .into_iter()
        .next();

    Ok(ObjectMap::Function(FunctionMap {
        function,
        params,
        datatype,
    }))
}

fn term_type_from_iri(iri: &str, default: TermType) -> TermType {
    match iri {
        i if i.ends_with("IRI") || i.ends_with("URI") => TermType::IRI,
        i if i.ends_with("BlankNode") => TermType::BlankNode,
        i if i.ends_with("Literal") => TermType::Literal,
        _ => default,
    }
}

fn parse_term_map(
    store: &TripleStore,
    node: &str,
    graph: Option<&str>,
    default_type: TermType,
) -> Result<TermMap, String> {
    let get = |pred: &str| get_objects(store, node, pred, graph);

    // Determine TermMapKind
    let kind = if let Some(c) = get(&format!("{RR}constant")).into_iter().next() {
        TermMapKind::Constant(c)
    } else if let Some(t) = get(&format!("{RR}template")).into_iter().next() {
        TermMapKind::Template(t)
    } else if let Some(r) = get(&format!("{RML}reference")).into_iter().next() {
        TermMapKind::Reference(r)
    } else if let Some(c) = get(&format!("{RR}column")).into_iter().next() {
        TermMapKind::Reference(c)
    } else {
        return Err(format!(
            "TermMap <{node}> has no constant, template, or reference"
        ));
    };

    // Determine TermType (default depends on context)
    let term_type_iris = get(&format!("{RR}termType"));
    let explicit_term_type = !term_type_iris.is_empty();
    let mut term_type = term_type_iris
        .into_iter()
        .next()
        .map(|iri| term_type_from_iri(&iri, default_type.clone()))
        .unwrap_or_else(|| default_type.clone());
    // R2RML: a constant's term type follows the constant's RDF term — an absolute-IRI
    // constant is an IRI even in object position (where the default is Literal).
    if !explicit_term_type {
        if let TermMapKind::Constant(ref v) = kind {
            if v.contains("://") || v.starts_with("urn:") {
                term_type = TermType::IRI;
            }
        }
    }

    let datatype = get(&format!("{RR}datatype")).into_iter().next();
    let language = get(&format!("{RR}language")).into_iter().next();

    Ok(TermMap {
        kind,
        term_type,
        datatype,
        language,
    })
}

fn parse_optional_term_map(
    store: &TripleStore,
    subject: &str,
    pred: &str,
    graph: Option<&str>,
) -> Option<TermMap> {
    let nodes = get_objects(store, subject, pred, graph);
    nodes
        .into_iter()
        .next()
        .and_then(|n| parse_term_map(store, &n, graph, TermType::IRI).ok())
}

/// Get all object values for (subject, predicate) in the given graph context.
///
/// Resolves through the raw quad index (`objects_for_subject_in_graph`) rather
/// than a SPARQL query, because `subject` may be a *stored* blank node — inline
/// term maps written with the standard idiom (`rr:subjectMap [ … ]`,
/// `rr:predicateObjectMap [ … ]`, `rr:objectMap [ … ]`). SPARQL surface syntax
/// cannot name a specific stored blank node (`_:x` in a query is a fresh
/// existential), so the old query form matched EVERY blank node carrying the
/// predicate and cross-contaminated inline mappings. This mirrors the fix the
/// SHACL shape loader already applies.
fn get_objects(
    store: &TripleStore,
    subject: &str,
    predicate: &str,
    graph: Option<&str>,
) -> Vec<String> {
    get_objects_typed(store, subject, predicate, graph)
        .into_iter()
        .map(|(value, _)| value)
        .collect()
}

/// Like [`get_objects`], but keeps whether each object was an IRI (`true`) or a
/// literal — the difference between `rml:source <urn:source:x>` (a registered
/// datasource) and `rml:source "x.csv"` (a file part name).
fn get_objects_typed(
    store: &TripleStore,
    subject: &str,
    predicate: &str,
    graph: Option<&str>,
) -> Vec<(String, bool)> {
    use oxigraph::model::Term;
    store
        .objects_for_subject_in_graph(subject, predicate, graph)
        .into_iter()
        .filter_map(|t| match t {
            Term::NamedNode(n) => Some((n.as_str().to_string(), true)),
            Term::BlankNode(b) => Some((format!("_:{}", b.as_str()), false)),
            Term::Literal(l) => Some((l.value().to_string(), false)),
            #[cfg(feature = "rdf-12")]
            Term::Triple(_) => None,
        })
        .collect()
}

/// Run a SELECT query and return a named column's values as strings.
fn query_col(store: &TripleStore, sparql: &str, col: &str) -> Vec<String> {
    use oxigraph::model::Term;
    use oxigraph::sparql::QueryResults;

    match store.query(sparql) {
        Ok(QueryResults::Solutions(mut solutions)) => {
            let mut results = Vec::new();
            while let Some(Ok(sol)) = solutions.next() {
                if let Some(term) = sol.get(col) {
                    let s = match term {
                        Term::NamedNode(n) => n.as_str().to_string(),
                        Term::BlankNode(b) => format!("_:{}", b.as_str()),
                        Term::Literal(l) => l.value().to_string(),
                        #[cfg(feature = "rdf-12")]
                        Term::Triple(_) => continue,
                    };
                    results.push(s);
                }
            }
            results
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PFX: &str = "@prefix rr: <http://www.w3.org/ns/r2rml#> .\n\
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .\n\
@prefix ql: <http://semweb.mmlab.be/ns/ql#> .\n\
@prefix fnml: <http://semweb.mmlab.be/ns/fnml#> .\n\
@prefix fno: <https://w3id.org/function/ontology#> .\n\
@prefix fn: <https://w3id.org/open-triplestore/fn#> .\n\
@prefix ex: <http://example.org/> .\n";

    #[test]
    fn a_datasource_logical_source_is_relational() {
        let m = parse_rml(&format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:legacy> ; rr:tableName \"products\" ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:p ; rr:objectMap [ rr:column \"name\" ] ] ."
        ))
        .expect("parses");
        let tm = &m.triples_maps[0];
        assert_eq!(
            tm.logical_source.source,
            SourceRef::Datasource("urn:source:legacy".into())
        );
        assert_eq!(
            tm.logical_source.reference_formulation,
            ReferenceFormulation::Sql
        );
        assert_eq!(tm.logical_source.table_name.as_deref(), Some("products"));
        assert!(m.has_sql_source());
        assert_eq!(m.datasources(), vec!["urn:source:legacy"]);
    }

    #[test]
    fn a_file_logical_source_still_parses_as_before() {
        let m = parse_rml(&format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source \"people.csv\" ; rml:referenceFormulation ql:CSV ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:p ; rr:objectMap [ rml:reference \"name\" ] ] ."
        ))
        .unwrap();
        let ls = &m.triples_maps[0].logical_source;
        assert_eq!(ls.source, SourceRef::File("people.csv".into()));
        assert_eq!(ls.reference_formulation, ReferenceFormulation::Csv);
        assert!(!m.has_sql_source());
    }

    #[test]
    fn a_relational_source_without_a_table_or_query_is_an_error() {
        let err = parse_rml(&format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:legacy> ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ."
        ))
        .unwrap_err();
        assert!(err.contains("rr:tableName"), "{err}");
    }

    #[test]
    fn referencing_object_maps_carry_their_join_conditions() {
        let m = parse_rml(&format!(
            "{PFX}
             ex:Child a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"child\" ] ;
               rr:subjectMap [ rr:template \"http://x/c{{cid}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:parent ; rr:objectMap [
                  rr:parentTriplesMap ex:Parent ;
                  rr:joinCondition [ rr:child \"pid\" ; rr:parent \"id\" ] ] ] .
             ex:Parent a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"parent\" ] ;
               rr:subjectMap [ rr:template \"http://x/p{{id}}\" ] ."
        ))
        .expect("parses");
        let child = m.find("http://example.org/Child").unwrap();
        let ObjectMap::Ref(r) = &child.predicate_object_maps[0].object else {
            panic!("expected a referencing object map");
        };
        assert_eq!(r.parent_triples_map, "http://example.org/Parent");
        assert_eq!(
            r.joins,
            vec![JoinCondition {
                child: "pid".into(),
                parent: "id".into()
            }]
        );
    }

    #[test]
    fn a_reference_to_a_missing_parent_is_a_mapping_error() {
        let err = parse_rml(&format!(
            "{PFX}
             ex:Child a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"child\" ] ;
               rr:subjectMap [ rr:template \"http://x/c{{cid}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:parent ; rr:objectMap [
                  rr:parentTriplesMap ex:Nope ;
                  rr:joinCondition [ rr:child \"pid\" ; rr:parent \"id\" ] ] ] ."
        ))
        .unwrap_err();
        assert!(err.contains("does not define"), "{err}");
    }

    #[test]
    fn a_joinless_reference_across_different_sources_is_refused() {
        let err = parse_rml(&format!(
            "{PFX}
             ex:Child a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"child\" ] ;
               rr:subjectMap [ rr:template \"http://x/c{{cid}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:parent ;
                  rr:objectMap [ rr:parentTriplesMap ex:Parent ] ] .
             ex:Parent a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"parent\" ] ;
               rr:subjectMap [ rr:template \"http://x/p{{id}}\" ] ."
        ))
        .unwrap_err();
        assert!(err.contains("rr:joinCondition"), "{err}");
    }

    #[test]
    fn a_function_object_map_collects_its_parameters() {
        let m = parse_rml(&format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"t\" ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:status ; rr:objectMap [
                 fnml:functionValue [
                   rr:predicateObjectMap [ rr:predicate fno:executes ; rr:object fn:mapValue ] ;
                   rr:predicateObjectMap [ rr:predicate fn:value ; rr:objectMap [ rr:column \"status\" ] ] ;
                   rr:predicateObjectMap [ rr:predicate fn:normalize ; rr:object \"lower_trim\" ] ;
                   rr:predicateObjectMap [ rr:predicate fn:mapping ; rr:object \"a=http://x/A\" ] ;
                   rr:predicateObjectMap [ rr:predicate fn:mapping ; rr:object \"b=http://x/B\" ] ;
                   rr:predicateObjectMap [ rr:predicate fn:unmapped ; rr:object \"literal\" ] ] ] ] ."
        ))
        .expect("parses");
        let ObjectMap::Function(f) = &m.triples_maps[0].predicate_object_maps[0].object else {
            panic!("expected a function object map");
        };
        assert_eq!(f.function, "https://w3id.org/open-triplestore/fn#mapValue");
        assert_eq!(
            f.first("https://w3id.org/open-triplestore/fn#value"),
            Some(&FunctionArg::Reference("status".into()))
        );
        assert_eq!(
            f.all("https://w3id.org/open-triplestore/fn#mapping").len(),
            2
        );
        assert_eq!(
            f.first("https://w3id.org/open-triplestore/fn#unmapped"),
            Some(&FunctionArg::Constant("literal".into()))
        );
    }

    #[test]
    fn a_function_without_fno_executes_is_an_error() {
        let err = parse_rml(&format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"t\" ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:s ; rr:objectMap [
                 fnml:functionValue [
                   rr:predicateObjectMap [ rr:predicate fn:value ; rr:object \"x\" ] ] ] ] ."
        ))
        .unwrap_err();
        assert!(err.contains("fno:executes"), "{err}");
    }

    #[test]
    fn a_document_with_no_triples_map_is_an_error() {
        let err = parse_rml(&format!("{PFX}ex:X a ex:NotAMapping .")).unwrap_err();
        assert!(err.contains("no rr:TriplesMap"), "{err}");
    }
}
