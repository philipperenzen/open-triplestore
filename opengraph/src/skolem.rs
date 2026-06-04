//! Opt-in Skolemization — turning blank nodes into durable IRIs.
//!
//! Stable canonical labels ([`crate::canonical`]) make blank nodes *reproducible*,
//! but they are still blank nodes: their identity is local to the dataset and not
//! directly referenceable in a SPARQL query. RDF 1.1 (§3.5, "Replacing Blank
//! Nodes with IRIs") sanctions going one step further — replacing a blank node
//! with a fresh IRI in the `/.well-known/genid/` space (RFC 6694). Such *Skolem
//! IRIs* are the furthest the standard lets you push blank-node durability: they
//! are real IRIs, globally referenceable, and survive any store round-trip.
//!
//! OpenGraph mints **content-derived** Skolem IRIs: the local id is the node's
//! canonical hash ([`crate::canonical::canonical_hashes`]), so the *same* logical
//! blank node always maps to the *same* IRI — across re-imports, reloads and
//! exports. This is what makes anonymous structures (SHACL shapes, RDF lists,
//! GeoSPARQL geometries) durably addressable.
//!
//! [`skolemize`] is the forward transform; [`deskolemize`] restores blank nodes
//! for standards-compliant blank-node output.

use crate::canonical;
use oxrdf::{BlankNode, GraphName, NamedNode, Quad, Subject, Term};
use std::collections::BTreeMap;

/// Default base IRI under which Skolem IRIs are minted when the caller does not
/// supply one.
pub const DEFAULT_SKOLEM_BASE: &str = "https://opengraph.local";

/// The well-known path component reserved for Skolem IRIs (RFC 6694 / RDF 1.1).
pub const GENID_PATH: &str = "/.well-known/genid/";

fn skolem_iri(base: &str, id: &str) -> String {
    format!("{}{GENID_PATH}{id}", base.trim_end_matches('/'))
}

fn named(iri: &str) -> NamedNode {
    NamedNode::new_unchecked(iri)
}

/// Replace every blank node in `quads` with a durable, content-derived Skolem
/// IRI under `base`. Returns the rewritten quads plus the mapping from each
/// input blank-node id to its minted IRI.
///
/// Because the local id is the node's canonical hash, the same logical blank
/// node always yields the same IRI — the property that makes it *durable*.
/// Quads without blank nodes are returned unchanged.
pub fn skolemize(quads: &[Quad], base: &str) -> (Vec<Quad>, BTreeMap<String, String>) {
    let hashes = canonical::canonical_hashes(quads);
    if hashes.is_empty() {
        return (quads.to_vec(), BTreeMap::new());
    }
    let map: BTreeMap<String, String> = hashes
        .into_iter()
        .map(|(bid, hash)| (bid, skolem_iri(base, &hash)))
        .collect();
    let out = quads.iter().map(|q| skolemize_quad(q, &map)).collect();
    (out, map)
}

fn skolemize_quad(q: &Quad, m: &BTreeMap<String, String>) -> Quad {
    let subject = match &q.subject {
        Subject::BlankNode(b) => Subject::NamedNode(named(&m[b.as_str()])),
        other => other.clone(),
    };
    let object = match &q.object {
        Term::BlankNode(b) => Term::NamedNode(named(&m[b.as_str()])),
        other => other.clone(),
    };
    let graph_name = match &q.graph_name {
        GraphName::BlankNode(b) => GraphName::NamedNode(named(&m[b.as_str()])),
        other => other.clone(),
    };
    Quad::new(subject, q.predicate.clone(), object, graph_name)
}

/// True if `iri` is a Skolem IRI minted under `base`.
pub fn is_skolem_iri(iri: &str, base: &str) -> bool {
    iri.starts_with(&format!("{}{GENID_PATH}", base.trim_end_matches('/')))
}

/// Inverse of [`skolemize`]: turn Skolem IRIs minted under `base` back into
/// blank nodes (the local id after the genid path becomes the blank-node label),
/// for standards-compliant blank-node serialization. IRIs outside the genid
/// space are left untouched.
pub fn deskolemize(quads: &[Quad], base: &str) -> Vec<Quad> {
    let prefix = format!("{}{GENID_PATH}", base.trim_end_matches('/'));
    quads.iter().map(|q| deskolemize_quad(q, &prefix)).collect()
}

fn de_iri(iri: &str, prefix: &str) -> Option<BlankNode> {
    iri.strip_prefix(prefix).map(BlankNode::new_unchecked)
}

fn deskolemize_quad(q: &Quad, prefix: &str) -> Quad {
    let subject = match &q.subject {
        Subject::NamedNode(n) => match de_iri(n.as_str(), prefix) {
            Some(b) => Subject::BlankNode(b),
            None => q.subject.clone(),
        },
        other => other.clone(),
    };
    let object = match &q.object {
        Term::NamedNode(n) => match de_iri(n.as_str(), prefix) {
            Some(b) => Term::BlankNode(b),
            None => q.object.clone(),
        },
        other => other.clone(),
    };
    let graph_name = match &q.graph_name {
        GraphName::NamedNode(n) => match de_iri(n.as_str(), prefix) {
            Some(b) => GraphName::BlankNode(b),
            None => q.graph_name.clone(),
        },
        other => other.clone(),
    };
    Quad::new(subject, q.predicate.clone(), object, graph_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{Literal, NamedNode};

    fn iri(s: &str) -> NamedNode {
        NamedNode::new(s).unwrap()
    }
    fn bnode(s: &str) -> BlankNode {
        BlankNode::new_unchecked(s)
    }
    fn sample() -> Vec<Quad> {
        // <a> :p _:x . _:x :name "Alice" . _:x :knows _:y . _:y :name "Bob"
        vec![
            Quad::new(
                Subject::NamedNode(iri("http://ex/a")),
                iri("http://ex/p"),
                Term::BlankNode(bnode("x")),
                GraphName::DefaultGraph,
            ),
            Quad::new(
                Subject::BlankNode(bnode("x")),
                iri("http://ex/name"),
                Term::Literal(Literal::new_simple_literal("Alice")),
                GraphName::DefaultGraph,
            ),
            Quad::new(
                Subject::BlankNode(bnode("x")),
                iri("http://ex/knows"),
                Term::BlankNode(bnode("y")),
                GraphName::DefaultGraph,
            ),
            Quad::new(
                Subject::BlankNode(bnode("y")),
                iri("http://ex/name"),
                Term::Literal(Literal::new_simple_literal("Bob")),
                GraphName::DefaultGraph,
            ),
        ]
    }

    #[test]
    fn skolemizes_all_blank_nodes_to_genid_iris() {
        let (out, map) = skolemize(&sample(), DEFAULT_SKOLEM_BASE);
        assert_eq!(map.len(), 2);
        for iri in map.values() {
            assert!(
                iri.starts_with(&format!("{DEFAULT_SKOLEM_BASE}{GENID_PATH}")),
                "got {iri}"
            );
            assert!(is_skolem_iri(iri, DEFAULT_SKOLEM_BASE));
        }
        // No blank nodes survive in the output.
        let joined = out
            .iter()
            .map(|q| q.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !joined.contains("_:"),
            "no blank nodes should remain: {joined}"
        );
    }

    #[test]
    fn skolem_iris_are_stable_across_relabel_and_reorder() {
        let a = sample();
        // Same graph, different blank-node labels, reversed order.
        let mut b = vec![
            Quad::new(
                Subject::BlankNode(bnode("q2")),
                iri("http://ex/name"),
                Term::Literal(Literal::new_simple_literal("Bob")),
                GraphName::DefaultGraph,
            ),
            Quad::new(
                Subject::BlankNode(bnode("q1")),
                iri("http://ex/knows"),
                Term::BlankNode(bnode("q2")),
                GraphName::DefaultGraph,
            ),
            Quad::new(
                Subject::BlankNode(bnode("q1")),
                iri("http://ex/name"),
                Term::Literal(Literal::new_simple_literal("Alice")),
                GraphName::DefaultGraph,
            ),
            Quad::new(
                Subject::NamedNode(iri("http://ex/a")),
                iri("http://ex/p"),
                Term::BlankNode(bnode("q1")),
                GraphName::DefaultGraph,
            ),
        ];
        b.reverse();

        let (_, ma) = skolemize(&a, DEFAULT_SKOLEM_BASE);
        let (_, mb) = skolemize(&b, DEFAULT_SKOLEM_BASE);
        // The *set* of minted IRIs is identical — durability across serializations.
        let sa: std::collections::BTreeSet<_> = ma.values().collect();
        let sb: std::collections::BTreeSet<_> = mb.values().collect();
        assert_eq!(sa, sb);
    }

    #[test]
    fn roundtrip_skolemize_then_deskolemize_is_isomorphic() {
        let original = sample();
        let (sk, _) = skolemize(&original, DEFAULT_SKOLEM_BASE);
        let back = deskolemize(&sk, DEFAULT_SKOLEM_BASE);
        // Compare via canonicalization: the de-Skolemized graph must be the same
        // graph as the original (same structure, blank nodes restored).
        let c_orig = canonical::canonicalize(&original);
        let c_back = canonical::canonicalize(&back);
        let set = |qs: &[Quad]| {
            qs.iter()
                .map(|q| q.to_string())
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(set(&c_orig.quads), set(&c_back.quads));
    }

    #[test]
    fn deskolemize_leaves_foreign_iris_untouched() {
        let quads = vec![Quad::new(
            Subject::NamedNode(iri("http://ex/a")),
            iri("http://ex/p"),
            Term::NamedNode(iri("http://ex/b")),
            GraphName::DefaultGraph,
        )];
        let back = deskolemize(&quads, DEFAULT_SKOLEM_BASE);
        assert_eq!(back[0].to_string(), quads[0].to_string());
    }

    #[test]
    fn custom_base_is_respected_and_trailing_slash_normalized() {
        let (_, m1) = skolemize(&sample(), "https://data.example.org");
        let (_, m2) = skolemize(&sample(), "https://data.example.org/");
        for iri in m1.values() {
            assert!(iri.starts_with("https://data.example.org/.well-known/genid/"));
        }
        // Trailing slash on the base must not double up.
        assert_eq!(
            m1.values().collect::<std::collections::BTreeSet<_>>(),
            m2.values().collect::<std::collections::BTreeSet<_>>()
        );
    }
}
