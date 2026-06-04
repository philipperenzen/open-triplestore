//! SHACL RDF → SHACLC compact syntax serializer.
//!
//! Reads NodeShape and PropertyShape triples from a named graph in the store
//! and emits the equivalent SHACL Compact Syntax text.

use crate::store::engine::TripleStore;

const SH: &str = "http://www.w3.org/ns/shacl#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// Serialize all NodeShapes in `shapes_graph` from `store` as SHACLC text.
pub fn serialize(store: &TripleStore, shapes_graph: &str) -> Result<String, String> {
    let mut out = String::with_capacity(1024);

    // Collect prefixes from shapes graph by examining all IRIs used
    let used_namespaces = collect_namespaces(store, shapes_graph);
    let prefixes = build_prefix_map(&used_namespaces);

    // Write PREFIX declarations
    for (prefix, ns) in &prefixes {
        out.push_str(&format!("PREFIX {prefix}: <{ns}>\n"));
    }
    if !prefixes.is_empty() {
        out.push('\n');
    }

    // Find all NodeShapes
    let node_shapes = query_objects(store,
        &format!("SELECT ?s WHERE {{ GRAPH <{shapes_graph}> {{ ?s a <{SH}NodeShape> }} }}"));

    if node_shapes.is_empty() {
        return Ok(out);
    }

    for shape_iri in &node_shapes {
        emit_shape(store, shapes_graph, shape_iri, &prefixes, &mut out)?;
        out.push('\n');
    }

    Ok(out)
}

fn emit_shape(
    store: &TripleStore,
    graph: &str,
    shape_iri: &str,
    prefixes: &[(String, String)],
    out: &mut String,
) -> Result<(), String> {
    let shape_ref = compact_iri(shape_iri, prefixes);

    // sh:targetClass
    let target_classes = query_objects(store,
        &format!("SELECT ?o WHERE {{ GRAPH <{graph}> {{ <{shape_iri}> <{SH}targetClass> ?o }} }}"));
    let target = target_classes.first().map(|tc| compact_iri(tc, prefixes));

    // sh:closed
    let closed_vals = query_objects(store,
        &format!("SELECT ?o WHERE {{ GRAPH <{graph}> {{ <{shape_iri}> <{SH}closed> ?o }} }}"));
    let is_closed = closed_vals.iter().any(|v| v == "true");

    out.push_str(&format!("shape {shape_ref}"));
    if let Some(ref tc) = target {
        out.push_str(&format!(" -> {tc}"));
    }
    if is_closed {
        out.push_str(" closed");
    }
    out.push_str(" {\n");

    // sh:property
    let props = query_objects(store,
        &format!("SELECT ?o WHERE {{ GRAPH <{graph}> {{ <{shape_iri}> <{SH}property> ?o }} }}"));

    for prop_node in &props {
        emit_property(store, graph, prop_node, prefixes, out)?;
    }

    out.push_str("}\n");
    Ok(())
}

fn emit_property(
    store: &TripleStore,
    graph: &str,
    prop_node: &str,
    prefixes: &[(String, String)],
    out: &mut String,
) -> Result<(), String> {
    let get = |pred: &str| -> Vec<String> {
        let q = if prop_node.starts_with("_:") {
            // blank node — use a different query approach
            format!("SELECT ?o WHERE {{ GRAPH <{graph}> {{ {prop_node} <{pred}> ?o }} }}")
        } else {
            format!("SELECT ?o WHERE {{ GRAPH <{graph}> {{ <{prop_node}> <{pred}> ?o }} }}")
        };
        query_objects(store, &q)
    };

    let paths = get(&format!("{SH}path"));
    let path = match paths.first() {
        Some(p) => compact_iri(p, prefixes),
        None => return Ok(()), // no path — skip this property shape
    };

    out.push_str(&format!("    {path}"));

    // Datatype
    if let Some(dt) = get(&format!("{SH}datatype")).into_iter().next() {
        out.push(' ');
        out.push_str(&compact_iri(&dt, prefixes));
    }

    // NodeKind
    if let Some(nk) = get(&format!("{SH}nodeKind")).into_iter().next() {
        let nk_local = nk.strip_prefix(SH).unwrap_or(&nk);
        out.push(' ');
        out.push_str(nk_local);
    }

    // sh:node (shape reference)
    if let Some(nr) = get(&format!("{SH}node")).into_iter().next() {
        out.push(' ');
        out.push_str(&compact_iri(&nr, prefixes));
    }

    // Cardinality [min..max]
    let min = get(&format!("{SH}minCount")).into_iter().next();
    let max = get(&format!("{SH}maxCount")).into_iter().next();
    if min.is_some() || max.is_some() {
        let min_s = min.as_deref().unwrap_or("0");
        let max_s = max.as_deref().map(|v| v.to_string()).unwrap_or_else(|| "*".to_string());
        out.push_str(&format!(" [{min_s}..{max_s}]"));
    }

    // Pattern
    if let Some(pat) = get(&format!("{SH}pattern")).into_iter().next() {
        out.push_str(&format!(" pattern \"{pat}\""));
    }

    // Message // "..."
    if let Some(msg) = get(&format!("{SH}message")).into_iter().next() {
        let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
        out.push_str(&format!(" // \"{escaped}\""));
    }

    out.push_str(" ;\n");
    Ok(())
}

/// Run a SELECT query and collect the first binding of each solution.
fn query_objects(store: &TripleStore, sparql: &str) -> Vec<String> {
    use oxigraph::sparql::QueryResults;
    use oxigraph::model::Term;

    match store.query(sparql) {
        Ok(QueryResults::Solutions(mut solutions)) => {
            let mut results = Vec::new();
            while let Some(Ok(sol)) = solutions.next() {
                if let Some(term) = sol.get(0) {
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

/// Collect all distinct namespace prefixes used in the shapes graph.
fn collect_namespaces(store: &TripleStore, graph: &str) -> Vec<String> {
    let q = format!(
        "SELECT DISTINCT ?iri WHERE {{
            GRAPH <{graph}> {{
                {{ ?iri ?p ?o }} UNION {{ ?s ?iri ?o }} UNION {{ ?s ?p ?iri }}
                FILTER(isIRI(?iri))
            }}
        }}"
    );
    query_objects(store, &q)
}

/// Build a prefix → namespace map from a set of IRI strings.
/// Returns well-known prefixes that appear in the IRI set.
fn build_prefix_map(iris: &[String]) -> Vec<(String, String)> {
    let well_known: &[(&str, &str)] = &[
        ("sh",     "http://www.w3.org/ns/shacl#"),
        ("rdf",    "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
        ("rdfs",   "http://www.w3.org/2000/01/rdf-schema#"),
        ("owl",    "http://www.w3.org/2002/07/owl#"),
        ("xsd",    XSD),
        ("schema", "http://schema.org/"),
        ("dct",    "http://purl.org/dc/terms/"),
        ("foaf",   "http://xmlns.com/foaf/0.1/"),
        ("skos",   "http://www.w3.org/2004/02/skos/core#"),
        ("void",   "http://rdfs.org/ns/void#"),
        ("dcat",   "http://www.w3.org/ns/dcat#"),
    ];

    let mut result = Vec::new();
    for (prefix, ns) in well_known {
        if iris.iter().any(|iri| iri.starts_with(ns)) {
            result.push((prefix.to_string(), ns.to_string()));
        }
    }
    result
}

/// Compact an IRI using the prefix map, or wrap in `<>`.
fn compact_iri(iri: &str, prefixes: &[(String, String)]) -> String {
    if iri.starts_with("_:") {
        return iri.to_string();
    }
    for (prefix, ns) in prefixes {
        if let Some(local) = iri.strip_prefix(ns.as_str()) {
            if !local.is_empty() && local.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.') {
                return format!("{prefix}:{local}");
            }
        }
    }
    format!("<{iri}>")
}
