//! RML mapping document parser.
//!
//! Reads an RML mapping stored as triples in the triple store (or loaded
//! from a Turtle string into an in-memory store) and builds an `RmlMapping`.

use super::model::*;
use crate::store::engine::TripleStore;

const RR: &str = "http://www.w3.org/ns/r2rml#";
const RML: &str = "http://semweb.mmlab.be/ns/rml#";

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
        Some(g) => format!("GRAPH <{g}> {{"),
        None => String::new(),
    };
    let graph_close = if graph.is_some() { "}" } else { "" };

    // Find all TriplesMap IRIs
    let q = format!("SELECT ?tm WHERE {{ {graph_clause} ?tm a <{RR}TriplesMap> {graph_close} }}");
    let tm_iris = query_col(store, &q, "tm");

    let mut triples_maps = Vec::new();
    for tm_iri in &tm_iris {
        match parse_triples_map(store, tm_iri, graph) {
            Ok(tm) => triples_maps.push(tm),
            Err(e) => return Err(format!("Error in TriplesMap <{tm_iri}>: {e}")),
        }
    }

    Ok(RmlMapping { triples_maps })
}

fn parse_triples_map(
    store: &TripleStore,
    tm_iri: &str,
    graph: Option<&str>,
) -> Result<TriplesMap, String> {
    let g = |pred: &str| -> Vec<String> { get_objects(store, tm_iri, pred, graph) };

    // Logical source (rml:logicalSource)
    let ls_nodes = g(&format!("{RML}logicalSource"));
    let ls_iri = ls_nodes
        .into_iter()
        .next()
        .ok_or("Missing rml:logicalSource")?;
    let logical_source = parse_logical_source(store, &ls_iri, graph)?;

    // Subject map (rr:subjectMap or rr:subject shortcut)
    let subject_map = parse_subject_map(store, tm_iri, graph)?;

    // Predicate-object maps
    let pom_nodes = g(&format!("{RR}predicateObjectMap"));
    let mut predicate_object_maps = Vec::new();
    for pom_node in &pom_nodes {
        predicate_object_maps.push(parse_pom(store, pom_node, graph)?);
    }

    // Optional graph map on the TriplesMap itself
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

    // rml:source — either a URI (file/URL) or a literal (inline data)
    let source_vals = get(&format!("{RML}source"));
    let source_val = source_vals.into_iter().next().ok_or("Missing rml:source")?;
    // If the value is a plain string literal (quoted by SPARQL serialisation),
    // treat it as inline CSV/JSON data; otherwise treat it as a file path/URL.
    let source = if source_val.starts_with('"') {
        // Strip SPARQL literal quotes and optional datatype/lang tag
        let inner = source_val.trim_start_matches('"');
        let inner = inner.rsplit_once('"').map(|(s, _)| s).unwrap_or(inner);
        SourceRef::Inline(inner.to_string())
    } else {
        SourceRef::File(source_val)
    };

    // rml:referenceFormulation
    let rf_vals = get(&format!("{RML}referenceFormulation"));
    let formulation = rf_vals
        .into_iter()
        .next()
        .map(|iri| ReferenceFormulation::from_iri(&iri))
        .unwrap_or(ReferenceFormulation::Csv);

    // rml:iterator (optional)
    let iterator = get(&format!("{RML}iterator")).into_iter().next();

    Ok(LogicalSource {
        source,
        reference_formulation: formulation,
        iterator,
    })
}

fn parse_subject_map(
    store: &TripleStore,
    tm_iri: &str,
    graph: Option<&str>,
) -> Result<SubjectMap, String> {
    // Try rr:subjectMap first (a blank/named node), then rr:subject (shortcut constant)
    let sm_nodes = get_objects(store, tm_iri, &format!("{RR}subjectMap"), graph);
    let term_map = if let Some(sm_node) = sm_nodes.into_iter().next() {
        parse_term_map(store, &sm_node, graph, TermType::IRI)?
    } else {
        // rr:subject shortcut
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

    // rr:class assertions
    let classes = get_objects(store, tm_iri, &format!("{RR}class"), graph);

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

    // Object map
    let om_nodes = get_objects(store, pom_node, &format!("{RR}objectMap"), graph);
    let object_map = if let Some(om) = om_nodes.into_iter().next() {
        parse_term_map(store, &om, graph, TermType::Literal)?
    } else {
        let obj = get_objects(store, pom_node, &format!("{RR}object"), graph)
            .into_iter()
            .next()
            .ok_or("Missing rr:objectMap or rr:object")?;
        TermMap {
            kind: TermMapKind::Constant(obj),
            term_type: TermType::Literal,
            datatype: None,
            language: None,
        }
    };

    // Optional graph map
    let graph_map = parse_optional_term_map(store, pom_node, &format!("{RR}graphMap"), graph);

    Ok(PredicateObjectMap {
        predicate_map,
        object_map,
        graph_map,
    })
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
    let term_type = term_type_iris
        .into_iter()
        .next()
        .map(|iri| match iri.as_str() {
            i if i.ends_with("IRI") => TermType::IRI,
            i if i.ends_with("BlankNode") => TermType::BlankNode,
            i if i.ends_with("Literal") => TermType::Literal,
            _ => default_type.clone(),
        })
        .unwrap_or(default_type);

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
fn get_objects(
    store: &TripleStore,
    subject: &str,
    predicate: &str,
    graph: Option<&str>,
) -> Vec<String> {
    let (subj_pattern, graph_clause, graph_close) = if subject.starts_with("_:") {
        // blank node subject — use a different format
        let gc = graph.map(|g| format!("GRAPH <{g}> {{")).unwrap_or_default();
        let gclose = if graph.is_some() { "}" } else { "" };
        (subject.to_string(), gc, gclose.to_string())
    } else {
        let gc = graph.map(|g| format!("GRAPH <{g}> {{")).unwrap_or_default();
        let gclose = if graph.is_some() { "}" } else { "" };
        (format!("<{subject}>"), gc, gclose.to_string())
    };

    let q = format!(
        "SELECT ?o WHERE {{ {graph_clause} {subj_pattern} <{predicate}> ?o {graph_close} }}"
    );
    query_col(store, &q, "o")
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
