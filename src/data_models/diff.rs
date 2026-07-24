//! data-model versions.

use super::models::{ChangedTriple, DiffResult, DiffSummary, TripleView};
use crate::store::{escape_sparql_iri, TripleStore};
use oxigraph::sparql::QueryResults;

/// Compute added, removed, and changed triples between two sets of sub-graphs.
///
/// Matching sub-graphs (by suffix) are compared. Any graph present only in
/// `to_graphs` contributes to additions; only in `from_graphs` to removals.
pub fn compute_diff(
    store: &TripleStore,
    from_graphs: &[String],
    to_graphs: &[String],
    filter_suffix: Option<&str>,
) -> DiffResult {
    let to_compare: Vec<(String, String)> = if let Some(suffix) = filter_suffix {
        // Only compare the specific sub-graph pair
        let from = from_graphs
            .iter()
            .find(|g| g.ends_with(suffix))
            .cloned()
            .or_else(|| from_graphs.iter().find(|g| !g.contains('/')).cloned());
        let to = to_graphs
            .iter()
            .find(|g| g.ends_with(suffix))
            .cloned()
            .or_else(|| to_graphs.iter().find(|g| !g.contains('/')).cloned());
        match (from, to) {
            (Some(f), Some(t)) => vec![(f, t)],
            _ => vec![],
        }
    } else {
        // Match sub-graphs by their suffix (last segment)
        let mut pairs: Vec<(String, String)> = Vec::new();
        for to_g in to_graphs {
            let to_suffix = to_g.rsplit('/').next().unwrap_or(to_g.as_str());
            if let Some(from_g) = from_graphs
                .iter()
                .find(|fg| fg.rsplit('/').next().unwrap_or(fg.as_str()) == to_suffix)
            {
                pairs.push((from_g.clone(), to_g.clone()));
            } else {
                // No corresponding from-graph → all triples are additions
                pairs.push((String::new(), to_g.clone()));
            }
        }
        // Any from-graph without a matching to-graph → all triples are removals
        for from_g in from_graphs {
            let from_suffix = from_g.rsplit('/').next().unwrap_or(from_g.as_str());
            if !to_graphs
                .iter()
                .any(|tg| tg.rsplit('/').next().unwrap_or(tg.as_str()) == from_suffix)
            {
                pairs.push((from_g.clone(), String::new()));
            }
        }
        pairs
    };

    let mut added: Vec<TripleView> = Vec::new();
    let mut removed: Vec<TripleView> = Vec::new();
    let mut changed: Vec<ChangedTriple> = Vec::new();

    for (from_g, to_g) in &to_compare {
        let graph_label = if to_g.is_empty() {
            to_suffix_label(from_g)
        } else {
            to_suffix_label(to_g)
        };

        if from_g.is_empty() {
            // Everything in to_g is an addition
            let triples = graph_triples(store, to_g);
            for t in triples {
                added.push(TripleView {
                    s: t.0,
                    p: t.1,
                    o: t.2,
                    graph: Some(graph_label.clone()),
                });
            }
        } else if to_g.is_empty() {
            // Everything in from_g is a removal
            let triples = graph_triples(store, from_g);
            for t in triples {
                removed.push(TripleView {
                    s: t.0,
                    p: t.1,
                    o: t.2,
                    graph: Some(graph_label.clone()),
                });
            }
        } else {
            // Compare both graphs
            let additions = additions_between(store, from_g, to_g);
            let removals = additions_between(store, to_g, from_g);

            for t in &additions {
                added.push(TripleView {
                    s: t.0.clone(),
                    p: t.1.clone(),
                    o: t.2.clone(),
                    graph: Some(graph_label.clone()),
                });
            }
            for t in &removals {
                removed.push(TripleView {
                    s: t.0.clone(),
                    p: t.1.clone(),
                    o: t.2.clone(),
                    graph: Some(graph_label.clone()),
                });
            }

            // Detect changed (same subject+predicate, different object)
            // A triple is "changed" if it was removed AND a triple with the same s+p was added
            for del in &removals {
                if let Some(add) = additions.iter().find(|a| a.0 == del.0 && a.1 == del.1) {
                    // Remove both from added/removed and put in changed
                    // (We'll handle deduplication after all pairs are processed)
                    changed.push(ChangedTriple {
                        s: del.0.clone(),
                        p: del.1.clone(),
                        before: del.2.clone(),
                        after: add.2.clone(),
                        graph: Some(graph_label.clone()),
                    });
                }
            }

            // Remove the "changed" triples from added/removed lists
            let changed_subjects: std::collections::HashSet<(&str, &str, &str)> = changed
                .iter()
                .map(|c| (c.s.as_str(), c.p.as_str(), c.graph.as_deref().unwrap_or("")))
                .collect();
            added.retain(|t| {
                !changed_subjects.contains(&(
                    t.s.as_str(),
                    t.p.as_str(),
                    t.graph.as_deref().unwrap_or(""),
                ))
            });
            removed.retain(|t| {
                !changed_subjects.contains(&(
                    t.s.as_str(),
                    t.p.as_str(),
                    t.graph.as_deref().unwrap_or(""),
                ))
            });
        }
    }

    let summary = DiffSummary {
        added: added.len(),
        removed: removed.len(),
        changed: changed.len(),
    };

    DiffResult {
        added,
        removed,
        changed,
        summary,
    }
}

fn to_suffix_label(iri: &str) -> String {
    iri.rsplit('/').next().unwrap_or(iri).to_string()
}

/// Flatten every triple across a set of graphs into a de-duplicated list,
/// ignoring graph names. Term serialization matches [`graph_triples`]
/// (`<iri>`, `"lit"`, `"lit"@lang`, `"lit"^^<dt>`, `_:bnode`).
pub fn collect_triples(store: &TripleStore, graphs: &[String]) -> Vec<(String, String, String)> {
    use std::collections::HashSet;
    let mut seen: HashSet<(String, String, String)> = HashSet::new();
    let mut out = Vec::new();
    for g in graphs {
        for t in graph_triples(store, g) {
            if seen.insert(t.clone()) {
                out.push(t);
            }
        }
    }
    out
}

/// Count triple-level ahead/behind between two versions, ignoring graph names.
///
/// Unlike [`compute_diff`] (which pairs sub-graphs by IRI suffix), this flattens
/// every triple across each version's graphs into a set and compares the sets.
/// That makes it correct for branch ahead/behind, where the graph IRIs differ by
/// version string and would never pair under suffix matching.
///
/// Returns `(ahead, behind)` = (triples in `to` not in `from`, in `from` not in `to`).
pub fn triple_delta(
    store: &TripleStore,
    from_graphs: &[String],
    to_graphs: &[String],
) -> (usize, usize) {
    use std::collections::HashSet;
    let collect = |graphs: &[String]| -> HashSet<(String, String, String)> {
        let mut set = HashSet::new();
        for g in graphs {
            for t in graph_triples(store, g) {
                set.insert(t);
            }
        }
        set
    };
    let from = collect(from_graphs);
    let to = collect(to_graphs);
    let ahead = to.difference(&from).count();
    let behind = from.difference(&to).count();
    (ahead, behind)
}

/// Compute a stable, order-independent content revision across a version's graphs.
///
/// Returns a short hex digest used as an opaque ETag / `If-Match` token for
/// optimistic concurrency control on draft edits. The same set of triples always
/// yields the same token regardless of insertion order; moving a triple between
/// sub-graphs changes it (the graph suffix is part of the hashed line).
pub fn version_revision(store: &TripleStore, graphs: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut lines: Vec<String> = Vec::new();
    for g in graphs {
        let label = to_suffix_label(g);
        for (s, p, o) in graph_triples(store, g) {
            lines.push(format!("{label}\t{s}\t{p}\t{o}"));
        }
    }
    lines.sort_unstable();
    let mut hasher = Sha256::new();
    for l in &lines {
        hasher.update(l.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// All (s, p, o) tuples in a graph.
fn graph_triples(store: &TripleStore, graph_iri: &str) -> Vec<(String, String, String)> {
    let graph_iri = escape_sparql_iri(graph_iri);
    let q = format!("SELECT ?s ?p ?o WHERE {{ GRAPH <{graph_iri}> {{ ?s ?p ?o }} }}");
    sparql_spo(store, &q)
}

/// Triples in `graph_b` that do NOT exist in `graph_a` — i.e. additions when going a→b.
fn additions_between(
    store: &TripleStore,
    graph_a: &str,
    graph_b: &str,
) -> Vec<(String, String, String)> {
    let graph_a = escape_sparql_iri(graph_a);
    let graph_b = escape_sparql_iri(graph_b);
    let q = format!(
        r#"
        SELECT ?s ?p ?o WHERE {{
          GRAPH <{graph_b}> {{ ?s ?p ?o }}
          FILTER NOT EXISTS {{ GRAPH <{graph_a}> {{ ?s ?p ?o }} }}
        }}
        "#
    );
    sparql_spo(store, &q)
}

fn sparql_spo(store: &TripleStore, q: &str) -> Vec<(String, String, String)> {
    let mut results = Vec::new();
    if let Ok(QueryResults::Solutions(solutions)) = store.query(q) {
        for row in solutions.flatten() {
            let vals: Vec<Option<oxigraph::model::Term>> = row.values().to_vec();
            let s = term_str(vals.first().and_then(|v: &Option<_>| v.as_ref()));
            let p = term_str(vals.get(1).and_then(|v: &Option<_>| v.as_ref()));
            let o = term_str(vals.get(2).and_then(|v: &Option<_>| v.as_ref()));
            if let (Some(s), Some(p), Some(o)) = (s, p, o) {
                results.push((s, p, o));
            }
        }
    }
    results
}

fn term_str(t: Option<&oxigraph::model::Term>) -> Option<String> {
    t.map(|t| match t {
        oxigraph::model::Term::NamedNode(nn) => format!("<{}>", nn.as_str()),
        oxigraph::model::Term::Literal(lit) => {
            if let Some(lang) = lit.language() {
                format!("\"{}\"@{}", lit.value(), lang)
            } else {
                let dt = lit.datatype().as_str();
                if dt == "http://www.w3.org/2001/XMLSchema#string" {
                    format!("\"{}\"", lit.value())
                } else {
                    format!("\"{}\"^^<{}>", lit.value(), dt)
                }
            }
        }
        oxigraph::model::Term::BlankNode(bn) => format!("_:{}", bn.as_str()),
        #[cfg(feature = "rdf-12")]
        oxigraph::model::Term::Triple(_) => "<< >>".to_string(),
    })
}
