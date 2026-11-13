//! data-model versions.

use super::models::{ChangedTriple, DiffResult, DiffSummary, TripleView};
use super::registry;
use crate::store::{escape_sparql_iri, TripleStore};
use oxigraph::sparql::QueryResults;

/// Compute added, removed, and changed triples between two versions, given
/// each one's base graph and the graphs holding its content.
///
/// Graphs are compared by their place in their version (see
/// [`registry::version_graph_name`]): base graph with base graph, sub-graph
/// `shapes` with sub-graph `shapes`. A graph only `to` has contributes
/// additions, one only `from` has removals. Each triple is labelled with that
/// place — `""` for the base graph, which is how the write endpoints name it
/// too — so the label of a graph is the same on both sides, whatever the
/// versions are called.
///
/// A triple only one graph of a pair holds is an addition or a removal, and
/// a removed and an added triple with the same subject and predicate are one
/// changed value (see [`pair_values`]). Every differing triple is reported
/// once, so the three lists together are the difference of the two versions
/// and the summary counts it.
///
/// `filter` narrows the comparison to the sub-graphs of that name, read as
/// the download and write endpoints read one (see
/// [`registry::sub_graph_answers_to`]: a leading `/` is ignored, and a nested
/// sub-graph also answers to its trailing segments).
pub fn compute_diff(
    store: &TripleStore,
    from_base: &str,
    from_graphs: &[String],
    to_base: &str,
    to_graphs: &[String],
    filter: Option<&str>,
) -> DiffResult {
    let place = |base: &str, graphs: &[String]| -> Vec<(String, String)> {
        graphs
            .iter()
            .map(|g| (registry::version_graph_name(base, g).to_string(), g.clone()))
            .collect()
    };
    let from_placed = place(from_base, from_graphs);
    let to_placed = place(to_base, to_graphs);

    // (place, from graph, to graph); an empty graph IRI means that side has
    // no graph there.
    let mut to_compare: Vec<(String, String, String)> = Vec::new();
    for (to_place, to_g) in &to_placed {
        let from_g = from_placed
            .iter()
            .find(|(p, _)| p == to_place)
            .map(|(_, g)| g.clone())
            .unwrap_or_default();
        to_compare.push((to_place.clone(), from_g, to_g.clone()));
    }
    for (from_place, from_g) in &from_placed {
        if !to_placed.iter().any(|(p, _)| p == from_place) {
            to_compare.push((from_place.clone(), from_g.clone(), String::new()));
        }
    }
    if let Some(wanted) = filter.filter(|f| !f.trim_start_matches('/').is_empty()) {
        to_compare.retain(|(place, _, _)| registry::sub_graph_answers_to(place, wanted));
    }

    let mut added: Vec<TripleView> = Vec::new();
    let mut removed: Vec<TripleView> = Vec::new();
    let mut changed: Vec<ChangedTriple> = Vec::new();

    for (graph_label, from_g, to_g) in &to_compare {
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
            let removals = additions_between(store, to_g, from_g);
            let additions = additions_between(store, from_g, to_g);
            let paired = pair_values(removals, additions);

            for (s, p, o) in paired.added {
                added.push(TripleView {
                    s,
                    p,
                    o,
                    graph: Some(graph_label.clone()),
                });
            }
            for (s, p, o) in paired.removed {
                removed.push(TripleView {
                    s,
                    p,
                    o,
                    graph: Some(graph_label.clone()),
                });
            }
            for ((s, p, before), after) in paired.changed {
                changed.push(ChangedTriple {
                    s,
                    p,
                    before,
                    after,
                    graph: Some(graph_label.clone()),
                });
            }
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

/// A triple as its three N-Triples terms: subject, predicate, object.
type Spo = (String, String, String);

/// What differs between two graphs, each differing triple in one list.
#[derive(Default)]
struct PairedValues {
    /// Triples only the newer graph holds.
    added: Vec<Spo>,
    /// Triples only the older graph holds.
    removed: Vec<Spo>,
    /// A triple only the older graph holds, with the object that replaced
    /// its own in the newer one.
    changed: Vec<(Spo, String)>,
}

/// Sort what differs between two graphs — `removals`, the triples only the
/// older one holds, and `additions`, those only the newer one does — into
/// changed values, additions and removals.
///
/// A removed and an added triple with the same subject and predicate are one
/// changed value, and each triple is used once. Where a subject has several
/// removed or added values for a predicate, they pair off in the order of
/// their terms and the ones left over stay removals or additions: `a` giving
/// way to `b` and `c` is the change `a` to `b` and the addition `c`. So every
/// differing triple is in exactly one of the three lists.
///
/// The lists come out in subject, predicate, object order, and the same two
/// graphs always pair the same way.
fn pair_values(mut removals: Vec<Spo>, mut additions: Vec<Spo>) -> PairedValues {
    use std::cmp::Ordering;
    removals.sort_unstable();
    additions.sort_unstable();
    let mut removals = removals.into_iter().peekable();
    let mut additions = additions.into_iter().peekable();
    let mut out = PairedValues::default();
    loop {
        // Which side's next triple comes first, by subject and predicate.
        let next = match (removals.peek(), additions.peek()) {
            (Some(del), Some(add)) => (&del.0, &del.1).cmp(&(&add.0, &add.1)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => break,
        };
        match next {
            Ordering::Less => out.removed.extend(removals.next()),
            Ordering::Greater => out.added.extend(additions.next()),
            Ordering::Equal => {
                if let (Some(del), Some(add)) = (removals.next(), additions.next()) {
                    out.changed.push((del, add.2));
                }
            }
        }
    }
    out
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
/// Unlike [`compute_diff`] (which pairs graphs by their place in the version),
/// this flattens every triple across each version's graphs into a set and
/// compares the sets: a triple that moved to another sub-graph does not count.
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

/// N-Triples rendering of a term: IRIs in `<>`, escaped literals with their
/// language tag, RDF 1.2 base direction (`"x"@ar--rtl`) or datatype, and
/// triple terms as `<<( s p o )>>` with their components rendered the same
/// way, so two different terms never render alike.
fn term_str(t: Option<&oxigraph::model::Term>) -> Option<String> {
    t.map(|t| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::term_str;
    use oxigraph::model::{Literal, NamedNode, Term};

    /// Quotes and newlines are escaped, so a literal cannot be mistaken for
    /// another term and two different literals never render alike.
    #[test]
    fn literals_are_escaped() {
        let t = Term::Literal(Literal::new_simple_literal("say \"hi\"\nnow"));
        assert_eq!(term_str(Some(&t)).unwrap(), r#""say \"hi\"\nnow""#);
    }

    #[cfg(feature = "rdf-12")]
    #[test]
    fn triple_terms_and_direction_are_kept() {
        use oxigraph::model::{BaseDirection, Triple};
        let dir = Literal::new_directional_language_tagged_literal("x", "ar", BaseDirection::Rtl)
            .unwrap();
        assert_eq!(
            term_str(Some(&Term::Literal(dir.clone()))).unwrap(),
            r#""x"@ar--rtl"#
        );
        let a = Term::Triple(Box::new(Triple::new(
            NamedNode::new_unchecked("http://ex/s"),
            NamedNode::new_unchecked("http://ex/p"),
            dir,
        )));
        let b = Term::Triple(Box::new(Triple::new(
            NamedNode::new_unchecked("http://ex/s"),
            NamedNode::new_unchecked("http://ex/p"),
            NamedNode::new_unchecked("http://ex/o"),
        )));
        assert_eq!(
            term_str(Some(&a)).unwrap(),
            r#"<<( <http://ex/s> <http://ex/p> "x"@ar--rtl )>>"#
        );
        assert_ne!(term_str(Some(&a)), term_str(Some(&b)));
    }
}
