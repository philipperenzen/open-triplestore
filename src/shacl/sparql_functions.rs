//! SHACL-AF `sh:SPARQLFunction` registration (SHACL Advanced Features §5).
//!
//! A `sh:SPARQLFunction` defines a user function whose body is a SPARQL `SELECT`
//! projecting a single variable (or `sh:ask`). Each is registered as an Oxigraph
//! custom function so it is callable from queries, SHACL-SPARQL constraints and
//! rules — e.g. the Waalbrug `ex:afstandMeter(geomA, geomB)` wrapping `geof:distance`.
//!
//! **Evaluation model.** Definitions are discovered through the raw quad index
//! (never via `store.query`, which would recurse through `query_options`). The body
//! is evaluated against a *fresh in-memory store* with the parameters textually
//! bound, which (a) avoids re-entrantly querying the store under evaluation and
//! (b) fully supports "expression" functions whose `WHERE` clause is empty. A
//! function whose body actually queries data is not supported in this form and
//! returns unbound — a documented limitation; express such logic as a
//! SHACL-SPARQL constraint instead.

use crate::store::TripleStore;
use oxigraph::model::{GraphName, NamedNodeRef, Subject, Term as OxTerm, TermRef};
use oxrdf::{NamedNode, Term};
use std::sync::Arc;

type FnHandler = Arc<dyn Fn(&[Term]) -> Option<Term> + Send + Sync>;

const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// Discover every `sh:SPARQLFunction` in `store` and return `(iri, handler)` pairs
/// ready to register via `QueryOptions::with_custom_function`. Empty (cheap) when
/// the store defines no functions.
pub fn all_functions(store: &TripleStore) -> Vec<(NamedNode, FnHandler)> {
    let ty = NamedNodeRef::new_unchecked(RDF_TYPE);
    let class_iri = format!("{SH}SPARQLFunction");
    let class = NamedNodeRef::new_unchecked(&class_iri);

    let mut out = Vec::new();
    for quad in
        store
            .store()
            .quads_for_pattern(None, Some(ty), Some(TermRef::NamedNode(class)), None)
    {
        let Ok(quad) = quad else { continue };
        let iri = match quad.subject {
            Subject::NamedNode(nn) => nn,
            _ => continue, // only named functions are callable
        };
        let graph: Option<String> = match quad.graph_name {
            GraphName::NamedNode(g) => Some(g.into_string()),
            _ => None,
        };
        if let Some(handler) = build_handler(store, iri.as_str(), graph.as_deref()) {
            out.push((iri, handler));
        }
    }
    out
}

fn build_handler(store: &TripleStore, iri: &str, graph: Option<&str>) -> Option<FnHandler> {
    let body = obj1(store, iri, &format!("{SH}select"), graph)?;
    let prologue = prefixes(store, iri, graph);
    // Ordered parameter variable names, derived from each sh:parameter's sh:path
    // local name (the body references them as `$localName`).
    let mut params: Vec<(f64, String)> = Vec::new();
    for p in objs(store, iri, &format!("{SH}parameter"), graph) {
        let Some(path) = obj1(store, &p, &format!("{SH}path"), graph) else {
            continue;
        };
        let order = obj1(store, &p, &format!("{SH}order"), graph)
            .and_then(|o| o.parse::<f64>().ok())
            .unwrap_or(0.0);
        params.push((order, local_name(&path)));
    }
    params.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let var_names: Vec<String> = params.into_iter().map(|(_, n)| n).collect();

    let full_body = format!("{prologue}{body}");
    let handler: FnHandler = Arc::new(move |args: &[Term]| {
        let mut q = full_body.clone();
        for (i, vn) in var_names.iter().enumerate() {
            let arg = args.get(i)?;
            // oxrdf Term Display is valid SPARQL term syntax (e.g. "…"^^<dt>, <iri>).
            q = q.replace(&format!("${vn}"), &arg.to_string());
        }
        // Evaluate against a fresh store so we never re-enter the caller's query.
        let tmp = TripleStore::in_memory().ok()?;
        match tmp.query(&q) {
            Ok(oxigraph::sparql::QueryResults::Solutions(sols)) => {
                for sol in sols.flatten() {
                    if let Some((_, term)) = sol.iter().next() {
                        return Some(term.clone());
                    }
                }
                None
            }
            Ok(oxigraph::sparql::QueryResults::Boolean(b)) => {
                Some(Term::Literal(oxrdf::Literal::new_typed_literal(
                    if b { "true" } else { "false" },
                    NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#boolean"),
                )))
            }
            _ => None,
        }
    });
    Some(handler)
}

fn obj1(store: &TripleStore, subj: &str, pred: &str, graph: Option<&str>) -> Option<String> {
    store
        .objects_for_subject_in_graph(subj, pred, graph)
        .into_iter()
        .next()
        .map(|t| lexical(&t))
}

fn objs(store: &TripleStore, subj: &str, pred: &str, graph: Option<&str>) -> Vec<String> {
    store
        .objects_for_subject_in_graph(subj, pred, graph)
        .iter()
        .map(lexical)
        .collect()
}

/// Build the SPARQL `PREFIX` prologue from the function node's `sh:prefixes`.
fn prefixes(store: &TripleStore, iri: &str, graph: Option<&str>) -> String {
    let mut out = String::new();
    for owner in objs(store, iri, &format!("{SH}prefixes"), graph) {
        for decl in objs(store, &owner, &format!("{SH}declare"), graph) {
            let p = obj1(store, &decl, &format!("{SH}prefix"), graph);
            let ns = obj1(store, &decl, &format!("{SH}namespace"), graph);
            if let (Some(p), Some(ns)) = (p, ns) {
                out.push_str(&format!("PREFIX {p}: <{ns}>\n"));
            }
        }
    }
    out
}

fn lexical(t: &OxTerm) -> String {
    match t {
        OxTerm::NamedNode(nn) => nn.as_str().to_string(),
        OxTerm::Literal(l) => l.value().to_string(),
        OxTerm::BlankNode(b) => format!("_:{}", b.as_str()),
        OxTerm::Triple(t) => t.to_string(),
    }
}

/// Local name of an IRI: the part after the last `#` or `/`.
fn local_name(iri: &str) -> String {
    iri.rsplit(['#', '/']).next().unwrap_or(iri).to_string()
}
