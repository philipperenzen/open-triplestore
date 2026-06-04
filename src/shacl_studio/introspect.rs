//! Read-only introspection over a data scope:
//!
//! * [`model_context`] — the classes and properties actually present, so the
//!   visual builder and autocomplete can offer the *real* vocabulary instead of
//!   `ex:someProperty` placeholders.
//! * [`derive_shapes`] — induce a draft SHACL shape from existing instances
//!   (the "draft from data" path), inferring datatype/class, cardinality and
//!   node kind per property from what the data actually contains.

use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use serde_json::json;

use crate::store::TripleStore;

/// `VALUES ?g { <a> <b> }` clause restricting queries to the scope's graphs.
/// Empty scope yields an empty string (queries then run over the union graph).
fn values_clause(data_graphs: &[String]) -> String {
    if data_graphs.is_empty() {
        return String::new();
    }
    let iris: String = data_graphs.iter().map(|g| format!("<{g}> ")).collect();
    format!("VALUES ?g {{ {iris}}}")
}

fn wrap_graph(values: &str, body: &str) -> String {
    if values.is_empty() {
        body.to_string()
    } else {
        format!("{values} GRAPH ?g {{ {body} }}")
    }
}

fn lit_i64(sol: &oxigraph::sparql::QuerySolution, var: &str) -> i64 {
    match sol.get(var) {
        Some(Term::Literal(l)) => l.value().parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}
fn iri_str(sol: &oxigraph::sparql::QuerySolution, var: &str) -> Option<String> {
    match sol.get(var) {
        Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
        _ => None,
    }
}
fn label_str(sol: &oxigraph::sparql::QuerySolution, var: &str) -> Option<String> {
    match sol.get(var) {
        Some(Term::Literal(l)) => Some(l.value().to_string()),
        _ => None,
    }
}

fn local_name(iri: &str) -> String {
    iri.rsplit(['#', '/'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(iri)
        .to_string()
}

/// Classes and properties present in the scope, each with a label (if any) and
/// a usage count, ordered by frequency.
pub fn model_context(store: &TripleStore, data_graphs: &[String]) -> serde_json::Value {
    let values = values_clause(data_graphs);

    let class_q = format!(
        "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
         SELECT ?c (SAMPLE(?l) AS ?label) (COUNT(DISTINCT ?s) AS ?n) WHERE {{ {} OPTIONAL {{ ?c rdfs:label ?l }} FILTER(isIRI(?c)) }} GROUP BY ?c ORDER BY DESC(?n) LIMIT 250",
        wrap_graph(&values, "?s a ?c")
    );
    let prop_q = format!(
        "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
         PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
         SELECT ?p (SAMPLE(?l) AS ?label) (COUNT(*) AS ?n) WHERE {{ {} OPTIONAL {{ ?p rdfs:label ?l }} FILTER(?p != rdf:type) }} GROUP BY ?p ORDER BY DESC(?n) LIMIT 400",
        wrap_graph(&values, "?s ?p ?o")
    );

    let classes = collect_terms(store, &class_q, "c");
    let properties = collect_terms(store, &prop_q, "p");
    json!({ "classes": classes, "properties": properties })
}

/// Run a `?term ?label ?n` aggregate query into `[{iri,label,count}]`.
fn collect_terms(store: &TripleStore, query: &str, var: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(solutions)) = store.query(query) {
        for sol in solutions.flatten() {
            if let Some(iri) = iri_str(&sol, var) {
                let label = label_str(&sol, "label").unwrap_or_else(|| local_name(&iri));
                out.push(json!({ "iri": iri, "label": label, "count": lit_i64(&sol, "n") }));
            }
        }
    }
    out
}

/// Per-property statistics gathered while deriving a shape.
struct PropStat {
    iri: String,
    subjects_with: i64,
    total_values: i64,
    literal_values: i64,
    iri_values: i64,
    datatype: Option<String>,
    object_class: Option<String>,
}

/// Induce a draft SHACL shapes graph (Turtle) from the instances in
/// `data_graphs`. When `requested_classes` is empty, the most-populated classes
/// (up to 6) are chosen automatically. Returns `(turtle, stats_json)`.
pub fn derive_shapes(
    store: &TripleStore,
    data_graphs: &[String],
    requested_classes: &[String],
) -> (String, serde_json::Value) {
    let values = values_clause(data_graphs);

    let classes: Vec<String> = if !requested_classes.is_empty() {
        requested_classes.to_vec()
    } else {
        let q = format!(
            "SELECT ?c (COUNT(DISTINCT ?s) AS ?n) WHERE {{ {} FILTER(isIRI(?c)) }} GROUP BY ?c ORDER BY DESC(?n) LIMIT 6",
            wrap_graph(&values, "?s a ?c")
        );
        super::run::select_iris(store, &q, "c")
    };

    let mut ttl = String::new();
    ttl.push_str("# Draft SHACL shapes — induced from existing data. Review before use.\n");
    ttl.push_str("PREFIX sh: <http://www.w3.org/ns/shacl#>\n");
    ttl.push_str("PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n");
    ttl.push_str("PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n");
    ttl.push_str("PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\n");

    let mut class_stats = Vec::new();
    for class in &classes {
        let n_instances = super::run::scalar_count(
            store,
            &format!(
                "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE {{ {} }}",
                wrap_graph(&values, &format!("?s a <{class}>"))
            ),
        );
        if n_instances == 0 {
            continue;
        }
        let props = property_stats(store, &values, class);

        ttl.push_str(&format!(
            "<{class}Shape>\n  a sh:NodeShape ;\n  sh:targetClass <{class}> ;\n"
        ));
        for p in &props {
            ttl.push_str("  sh:property [\n");
            ttl.push_str(&format!("    sh:path <{}> ;\n", p.iri));
            ttl.push_str(&format!(
                "    sh:name \"{}\" ;\n",
                escape_literal(&local_name(&p.iri))
            ));
            // Value type.
            if p.iri_values > 0 && p.literal_values == 0 {
                if let Some(oc) = &p.object_class {
                    ttl.push_str(&format!("    sh:class <{oc}> ;\n"));
                } else {
                    ttl.push_str("    sh:nodeKind sh:IRI ;\n");
                }
            } else if p.literal_values > 0 && p.iri_values == 0 {
                if let Some(dt) = &p.datatype {
                    ttl.push_str(&format!("    sh:datatype <{dt}> ;\n"));
                }
            }
            // Cardinality.
            if p.subjects_with == n_instances {
                ttl.push_str("    sh:minCount 1 ;\n");
            }
            if p.total_values == p.subjects_with {
                ttl.push_str("    sh:maxCount 1 ;\n");
            }
            ttl.push_str("  ] ;\n");
        }
        // Trim trailing " ;\n" → " .\n".
        if ttl.ends_with(" ;\n") {
            ttl.truncate(ttl.len() - 3);
            ttl.push_str(" .\n\n");
        } else {
            ttl.push_str(".\n\n");
        }

        class_stats.push(json!({
            "class": class,
            "instances": n_instances,
            "properties": props.len(),
        }));
    }

    let stats = json!({ "classes": class_stats });
    (ttl, stats)
}

fn property_stats(store: &TripleStore, values: &str, class: &str) -> Vec<PropStat> {
    let props_q = format!(
        "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
         SELECT DISTINCT ?p WHERE {{ {} FILTER(?p != rdf:type) }} LIMIT 30",
        wrap_graph(values, &format!("?s a <{class}> ; ?p ?o"))
    );
    let prop_iris = super::run::select_iris(store, &props_q, "p");

    let mut out = Vec::new();
    for p in prop_iris {
        let agg_q = format!(
            "SELECT (COUNT(DISTINCT ?s) AS ?subs) (COUNT(?o) AS ?vals) \
             (SUM(IF(isLiteral(?o),1,0)) AS ?lit) (SUM(IF(isIRI(?o),1,0)) AS ?iri) \
             WHERE {{ {} }}",
            wrap_graph(values, &format!("?s a <{class}> ; <{p}> ?o"))
        );
        let (mut subs, mut vals, mut lit, mut iri) = (0i64, 0i64, 0i64, 0i64);
        if let Ok(QueryResults::Solutions(mut sols)) = store.query(&agg_q) {
            if let Some(Ok(sol)) = sols.next() {
                subs = lit_i64(&sol, "subs");
                vals = lit_i64(&sol, "vals");
                lit = lit_i64(&sol, "lit");
                iri = lit_i64(&sol, "iri");
            }
        }

        let datatype = if lit > 0 && iri == 0 {
            single_iri(
                store,
                &format!(
                    "SELECT (DATATYPE(?o) AS ?dt) (COUNT(*) AS ?n) WHERE {{ {} FILTER(isLiteral(?o)) }} GROUP BY (DATATYPE(?o)) ORDER BY DESC(?n) LIMIT 2",
                    wrap_graph(values, &format!("?s a <{class}> ; <{p}> ?o"))
                ),
                "dt",
            )
        } else {
            None
        };
        let object_class = if iri > 0 && lit == 0 {
            single_iri(
                store,
                &format!(
                    "SELECT ?oc (COUNT(*) AS ?n) WHERE {{ {} }} GROUP BY ?oc ORDER BY DESC(?n) LIMIT 2",
                    wrap_graph(values, &format!("?s a <{class}> ; <{p}> ?o . ?o a ?oc"))
                ),
                "oc",
            )
        } else {
            None
        };

        out.push(PropStat {
            iri: p,
            subjects_with: subs,
            total_values: vals,
            literal_values: lit,
            iri_values: iri,
            datatype,
            object_class,
        });
    }
    out
}

/// Return the single bound IRI if the query yields exactly one row, else `None`
/// (so a mixed/ambiguous result falls back to a looser constraint).
fn single_iri(store: &TripleStore, query: &str, var: &str) -> Option<String> {
    let rows = super::run::select_iris(store, query, var);
    if rows.len() == 1 {
        Some(rows.into_iter().next().unwrap())
    } else {
        None
    }
}

fn escape_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
