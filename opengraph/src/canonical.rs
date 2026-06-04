//! Deterministic blank-node labeling — the foundation of durable blank-node
//! identity in OpenGraph.
//!
//! Plain RDF blank nodes have no durable identity: every time a document is
//! parsed, the engine is free to invent fresh labels (`_:b0`, `_:b1`, …), so
//! re-importing or reloading the *same* data yields *different* labels. That is
//! exactly what makes anonymous nodes (SHACL shapes, RDF lists, GeoSPARQL
//! geometries) impossible to address reliably across sessions.
//!
//! This module assigns each blank node a **deterministic** label derived purely
//! from the graph structure around it — not from the arbitrary labels or
//! statement order of the input. It follows the shape of the W3C *RDF Dataset
//! Canonicalization* algorithm (RDFC-1.0 / URDNA2015):
//!
//! 1. **First-degree hash** — for each blank node, serialize every quad it
//!    appears in with the node itself rendered as `_:a` and *all other* blank
//!    nodes as `_:z`, sort those lines, and SHA-256 them. Two blank nodes with
//!    different immediate surroundings already get different hashes.
//! 2. **Iterative refinement** (colour refinement / Weisfeiler-Leman) — fold
//!    each node's hash together with the *current* hashes of its blank-node
//!    neighbours, repeatedly, until the partition stops splitting. This
//!    distinguishes nodes that are only told apart by deeper structure (e.g.
//!    their position in an RDF list of repeated values).
//! 3. **Label assignment** — order the nodes by their final hash and hand out
//!    `c14n0`, `c14n1`, … in that order.
//!
//! The result is independent of input label choice and statement order, so the
//! same logical graph always canonicalizes to the same labels.
//!
//! # Scope / limitations
//!
//! * Hashes are an **implementation-internal** canonical form: they are stable
//!   and deterministic for our own round-trips, but are *not* guaranteed to be
//!   byte-identical to other RDFC-1.0 implementations' hashes. Durability — not
//!   cross-implementation hash interop — is the goal.
//! * Genuinely *automorphic* blank nodes (structurally indistinguishable even
//!   after full refinement — rare in real ontology/SHACL/list data) are ordered
//!   by their input label as a deterministic tie-break. Full RDFC-1.0 resolves
//!   these with the "Hash N-Degree Quads" procedure; that is a future addition.
//! * RDF-star triple terms are not traversed (OpenGraph does not enable the
//!   `rdf-star` feature on `oxrdf`).

use oxrdf::{BlankNode, GraphName, NamedNode, Quad, Subject, Term};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Label prefix for canonical blank nodes (matches RDFC-1.0, which uses `c14n`).
pub const CANON_PREFIX: &str = "c14n";

/// Outcome of canonicalizing a set of quads.
#[derive(Debug, Clone)]
pub struct Canonicalized {
    /// The input quads with every blank node relabeled to its canonical id.
    pub quads: Vec<Quad>,
    /// Map from each input blank-node id to its canonical id (e.g. `"c14n0"`).
    pub mapping: BTreeMap<String, String>,
}

// ── blank-node extraction ───────────────────────────────────────────────────

fn subject_bnode(s: &Subject) -> Option<&str> {
    match s {
        Subject::BlankNode(b) => Some(b.as_str()),
        _ => None,
    }
}
fn term_bnode(t: &Term) -> Option<&str> {
    match t {
        Term::BlankNode(b) => Some(b.as_str()),
        _ => None,
    }
}
fn graph_bnode(g: &GraphName) -> Option<&str> {
    match g {
        GraphName::BlankNode(b) => Some(b.as_str()),
        _ => None,
    }
}

/// Every distinct blank-node id occurring anywhere in `quads`.
fn collect_bnodes(quads: &[Quad]) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for q in quads {
        if let Some(b) = subject_bnode(&q.subject) {
            set.insert(b.to_string());
        }
        if let Some(b) = term_bnode(&q.object) {
            set.insert(b.to_string());
        }
        if let Some(b) = graph_bnode(&q.graph_name) {
            set.insert(b.to_string());
        }
    }
    set
}

// ── serialization for hashing ───────────────────────────────────────────────

fn ser_named(n: &NamedNode) -> String {
    format!("<{}>", n.as_str())
}

/// Serialize a quad to a single N-Quad-ish line, rendering every blank node via
/// `bn` (which maps a blank-node id to its placeholder/coloured form). The exact
/// syntax only needs to be deterministic and injective for our own hashing.
fn ser_quad(q: &Quad, bn: &dyn Fn(&str) -> String) -> String {
    let s = match &q.subject {
        Subject::NamedNode(n) => ser_named(n),
        Subject::BlankNode(b) => bn(b.as_str()),
        // rdf-star triple subjects (feature off by default) — opaque placeholder.
        _ => "<<triple>>".to_string(),
    };
    let p = ser_named(&q.predicate);
    let o = match &q.object {
        Term::NamedNode(n) => ser_named(n),
        Term::BlankNode(b) => bn(b.as_str()),
        Term::Literal(l) => l.to_string(),
        _ => "<<triple>>".to_string(),
    };
    match &q.graph_name {
        GraphName::DefaultGraph => format!("{s} {p} {o} ."),
        GraphName::NamedNode(n) => format!("{s} {p} {o} {} .", ser_named(n)),
        GraphName::BlankNode(b) => format!("{s} {p} {o} {} .", bn(b.as_str())),
    }
}

fn sha_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// RDFC-1.0 "hash first degree quads" for blank node `target`.
fn first_degree(quads: &[Quad], idxs: &[usize], target: &str) -> String {
    let bn = |id: &str| {
        if id == target {
            "_:a".to_string()
        } else {
            "_:z".to_string()
        }
    };
    let mut lines: Vec<String> = idxs.iter().map(|&i| ser_quad(&quads[i], &bn)).collect();
    lines.sort();
    sha_hex(lines.join("\n").as_bytes())
}

/// One colour-refinement round: fold each node's hash with the current hashes of
/// its neighbours (other blank nodes are rendered as `_:<their current hash>`,
/// the node itself as `_:_`).
fn refine(
    quads: &[Quad],
    by_bnode: &BTreeMap<String, Vec<usize>>,
    colors: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut next = BTreeMap::new();
    for (b, idxs) in by_bnode {
        let bn = |id: &str| {
            if id == b.as_str() {
                "_:_".to_string()
            } else {
                format!("_:{}", colors[id])
            }
        };
        let mut sigs: Vec<String> = idxs.iter().map(|&i| ser_quad(&quads[i], &bn)).collect();
        sigs.sort();
        let payload = format!("{}|{}", colors[b], sigs.join("|"));
        next.insert(b.clone(), sha_hex(payload.as_bytes()));
    }
    next
}

fn distinct(colors: &BTreeMap<String, String>) -> usize {
    colors.values().collect::<BTreeSet<_>>().len()
}

/// Build the index `blank-node id → indices of quads that mention it`.
fn index_by_bnode(quads: &[Quad], bnodes: &BTreeSet<String>) -> BTreeMap<String, Vec<usize>> {
    let mut by_bnode: BTreeMap<String, Vec<usize>> =
        bnodes.iter().map(|b| (b.clone(), Vec::new())).collect();
    for (i, q) in quads.iter().enumerate() {
        let mut touched = BTreeSet::new();
        if let Some(b) = subject_bnode(&q.subject) {
            touched.insert(b.to_string());
        }
        if let Some(b) = term_bnode(&q.object) {
            touched.insert(b.to_string());
        }
        if let Some(b) = graph_bnode(&q.graph_name) {
            touched.insert(b.to_string());
        }
        for b in touched {
            by_bnode.get_mut(&b).unwrap().push(i);
        }
    }
    by_bnode
}

/// Compute a stable, structure-derived hash for every blank node in `quads`.
///
/// The returned map is keyed by input blank-node id; the same logical graph
/// always produces the same hashes regardless of input labels or quad order.
/// Used both for canonical labeling and for minting durable Skolem IRIs.
pub fn canonical_hashes(quads: &[Quad]) -> BTreeMap<String, String> {
    let bnodes = collect_bnodes(quads);
    if bnodes.is_empty() {
        return BTreeMap::new();
    }
    let by_bnode = index_by_bnode(quads, &bnodes);

    // First-degree colours.
    let mut colors: BTreeMap<String, String> = by_bnode
        .iter()
        .map(|(b, idxs)| (b.clone(), first_degree(quads, idxs, b)))
        .collect();

    // Refine until the partition stabilises (colour refinement reaches a
    // fixpoint) or every node is uniquely coloured. Bounded by the node count.
    let total = bnodes.len();
    let mut prev = distinct(&colors);
    for _ in 0..=total {
        if prev == total {
            break; // already fully discriminated
        }
        let next = refine(quads, &by_bnode, &colors);
        let d = distinct(&next);
        colors = next;
        if d == prev {
            break; // no further splitting — fixpoint
        }
        prev = d;
    }
    colors
}

fn relabel_quad(q: &Quad, m: &BTreeMap<String, String>) -> Quad {
    let subject = match &q.subject {
        Subject::BlankNode(b) => {
            Subject::BlankNode(BlankNode::new_unchecked(m[b.as_str()].clone()))
        }
        other => other.clone(),
    };
    let object = match &q.object {
        Term::BlankNode(b) => Term::BlankNode(BlankNode::new_unchecked(m[b.as_str()].clone())),
        other => other.clone(),
    };
    let graph_name = match &q.graph_name {
        GraphName::BlankNode(b) => {
            GraphName::BlankNode(BlankNode::new_unchecked(m[b.as_str()].clone()))
        }
        other => other.clone(),
    };
    Quad::new(subject, q.predicate.clone(), object, graph_name)
}

/// Canonicalize a set of quads: relabel every blank node to a deterministic
/// `c14nN` id and return the rewritten quads plus the id mapping.
///
/// Quads without blank nodes are returned unchanged.
pub fn canonicalize(quads: &[Quad]) -> Canonicalized {
    let colors = canonical_hashes(quads);
    if colors.is_empty() {
        return Canonicalized {
            quads: quads.to_vec(),
            mapping: BTreeMap::new(),
        };
    }
    // Order by final hash, then by input id (deterministic tie-break for the
    // rare automorphic case). Assign c14n0, c14n1, … in that order.
    let mut ids: Vec<&String> = colors.keys().collect();
    ids.sort_by(|a, b| {
        colors[a.as_str()]
            .cmp(&colors[b.as_str()])
            .then_with(|| a.cmp(b))
    });
    let mut mapping = BTreeMap::new();
    for (i, id) in ids.iter().enumerate() {
        mapping.insert((*id).clone(), format!("{CANON_PREFIX}{i}"));
    }
    let quads = quads.iter().map(|q| relabel_quad(q, &mapping)).collect();
    Canonicalized { quads, mapping }
}

/// Label prefix for content-hash blank-node ids produced by [`stable_relabel`]
/// (a leading letter keeps the id a valid blank-node label).
pub const STABLE_PREFIX: &str = "g";

/// Relabel every blank node to a stable, **content-derived** label
/// (`_:g<hash>`) instead of the sequential `c14nN` ids that [`canonicalize`]
/// produces.
///
/// Sequential canonical labels are only unique *within a single* canonicalization
/// run, so two independently-canonicalized batches both start at `c14n0` and would
/// merge unrelated blank nodes when loaded into one store. Hash-derived labels are
/// globally unique per structure, so they are safe for **incremental triple-store
/// imports**: re-importing the same data reproduces the same labels (durability)
/// without colliding across separate imports.
pub fn stable_relabel(quads: &[Quad]) -> Vec<Quad> {
    let hashes = canonical_hashes(quads);
    if hashes.is_empty() {
        return quads.to_vec();
    }
    let mapping: BTreeMap<String, String> = hashes
        .into_iter()
        .map(|(id, hash)| (id, format!("{STABLE_PREFIX}{hash}")))
        .collect();
    quads.iter().map(|q| relabel_quad(q, &mapping)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{Literal, NamedNodeRef};

    // ── tiny quad builders ──────────────────────────────────────────────────
    fn iri(s: &str) -> NamedNode {
        NamedNode::new(s).unwrap()
    }
    fn bnode(s: &str) -> BlankNode {
        BlankNode::new_unchecked(s)
    }
    /// `<s> <p> <o>` (named object)
    fn q_nn(s: Subject, p: &str, o: &str) -> Quad {
        Quad::new(s, iri(p), Term::NamedNode(iri(o)), GraphName::DefaultGraph)
    }
    /// `<s> <p> "lit"`
    fn q_lit(s: Subject, p: &str, lit: &str) -> Quad {
        Quad::new(
            s,
            iri(p),
            Term::Literal(Literal::new_simple_literal(lit)),
            GraphName::DefaultGraph,
        )
    }

    fn nquad_set(quads: &[Quad]) -> BTreeSet<String> {
        quads.iter().map(|q| q.to_string()).collect()
    }

    const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
    const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
    const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";

    #[test]
    fn no_bnodes_returns_input_unchanged() {
        let quads = vec![q_nn(
            Subject::NamedNode(iri("http://ex/a")),
            "http://ex/p",
            "http://ex/b",
        )];
        let c = canonicalize(&quads);
        assert!(c.mapping.is_empty());
        assert_eq!(nquad_set(&c.quads), nquad_set(&quads));
    }

    #[test]
    fn relabels_blank_nodes_to_c14n() {
        // <a> :p _:x . _:x :q "v"
        let quads = vec![
            Quad::new(
                Subject::NamedNode(iri("http://ex/a")),
                iri("http://ex/p"),
                Term::BlankNode(bnode("x")),
                GraphName::DefaultGraph,
            ),
            q_lit(Subject::BlankNode(bnode("x")), "http://ex/q", "v"),
        ];
        let c = canonicalize(&quads);
        assert_eq!(c.mapping.len(), 1);
        assert_eq!(c.mapping.get("x").map(String::as_str), Some("c14n0"));
        // The blank node label "x" must no longer appear; "c14n0" must.
        let joined = c
            .quads
            .iter()
            .map(|q| q.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("_:c14n0"));
        assert!(!joined.contains("_:x"));
    }

    /// The same logical graph, expressed with different blank-node labels AND a
    /// different statement order, must canonicalize to the identical quad set.
    #[test]
    fn deterministic_across_relabel_and_reorder() {
        // graph A: <a> :p _:b1 ; _:b1 :name "Alice" ; _:b1 :knows _:b2 ; _:b2 :name "Bob"
        let a = vec![
            Quad::new(
                Subject::NamedNode(iri("http://ex/a")),
                iri("http://ex/p"),
                Term::BlankNode(bnode("b1")),
                GraphName::DefaultGraph,
            ),
            q_lit(Subject::BlankNode(bnode("b1")), "http://ex/name", "Alice"),
            Quad::new(
                Subject::BlankNode(bnode("b1")),
                iri("http://ex/knows"),
                Term::BlankNode(bnode("b2")),
                GraphName::DefaultGraph,
            ),
            q_lit(Subject::BlankNode(bnode("b2")), "http://ex/name", "Bob"),
        ];
        // graph B: identical structure, different labels (zzz/aaa), reversed order
        let mut b = vec![
            q_lit(Subject::BlankNode(bnode("aaa")), "http://ex/name", "Bob"),
            Quad::new(
                Subject::BlankNode(bnode("zzz")),
                iri("http://ex/knows"),
                Term::BlankNode(bnode("aaa")),
                GraphName::DefaultGraph,
            ),
            q_lit(Subject::BlankNode(bnode("zzz")), "http://ex/name", "Alice"),
            Quad::new(
                Subject::NamedNode(iri("http://ex/a")),
                iri("http://ex/p"),
                Term::BlankNode(bnode("zzz")),
                GraphName::DefaultGraph,
            ),
        ];
        b.reverse();

        let ca = canonicalize(&a);
        let cb = canonicalize(&b);
        assert_eq!(
            nquad_set(&ca.quads),
            nquad_set(&cb.quads),
            "isomorphic graphs must yield identical canonical quads"
        );
        // Two blank nodes, distinguished by their names.
        assert_eq!(ca.mapping.len(), 2);
    }

    /// An RDF list of *repeated* values: refinement must still tell the three
    /// list cells apart (they differ only by depth from `rdf:nil`).
    #[test]
    fn rdf_list_repeated_values_are_distinguished() {
        // _:l1 first "x" ; rest _:l2 .  _:l2 first "x" ; rest _:l3 .  _:l3 first "x" ; rest nil .
        let make = |l1: &str, l2: &str, l3: &str| {
            vec![
                q_lit(Subject::BlankNode(bnode(l1)), RDF_FIRST, "x"),
                Quad::new(
                    Subject::BlankNode(bnode(l1)),
                    iri(RDF_REST),
                    Term::BlankNode(bnode(l2)),
                    GraphName::DefaultGraph,
                ),
                q_lit(Subject::BlankNode(bnode(l2)), RDF_FIRST, "x"),
                Quad::new(
                    Subject::BlankNode(bnode(l2)),
                    iri(RDF_REST),
                    Term::BlankNode(bnode(l3)),
                    GraphName::DefaultGraph,
                ),
                q_lit(Subject::BlankNode(bnode(l3)), RDF_FIRST, "x"),
                Quad::new(
                    Subject::BlankNode(bnode(l3)),
                    iri(RDF_REST),
                    Term::NamedNode(iri(RDF_NIL)),
                    GraphName::DefaultGraph,
                ),
            ]
        };
        let c = canonicalize(&make("l1", "l2", "l3"));
        let labels: BTreeSet<&String> = c.mapping.values().collect();
        assert_eq!(
            labels.len(),
            3,
            "three list cells must get three distinct labels"
        );

        // And it is order/label independent: a relabeled, reordered copy matches.
        let mut other = make("n7", "n3", "n9");
        other.reverse();
        let c2 = canonicalize(&other);
        assert_eq!(nquad_set(&c.quads), nquad_set(&c2.quads));
    }

    #[test]
    fn named_graph_blank_subject_is_handled() {
        // bnode appearing as the graph name as well as the subject
        let q = Quad::new(
            Subject::BlankNode(bnode("g")),
            iri("http://ex/p"),
            Term::Literal(Literal::new_simple_literal("v")),
            GraphName::BlankNode(bnode("g")),
        );
        let _ = NamedNodeRef::new("http://ex/p").unwrap(); // sanity that imports are used
        let c = canonicalize(&[q]);
        assert_eq!(c.mapping.len(), 1);
        let s = c.quads[0].to_string();
        assert!(s.contains("_:c14n0"));
    }

    #[test]
    fn stable_relabel_is_collision_free_and_durable() {
        // _:x :p "v"
        let make = |label: &str| vec![q_lit(Subject::BlankNode(bnode(label)), "http://ex/p", "v")];
        // Same structure, different input labels → identical stable hash labels.
        let a = stable_relabel(&make("x"));
        let b = stable_relabel(&make("totally-different-label"));
        assert_eq!(
            nquad_set(&a),
            nquad_set(&b),
            "same structure must yield same label"
        );
        // The label is a content hash with the documented prefix, not c14n0.
        let s = a[0].to_string();
        assert!(s.contains(&format!("_:{}", STABLE_PREFIX)), "got {s}");
        assert!(!s.contains("c14n"));

        // Two *different* structures must NOT collide (unlike sequential c14n0).
        let other = stable_relabel(&vec![q_lit(
            Subject::BlankNode(bnode("y")),
            "http://ex/DIFFERENT",
            "v",
        )]);
        assert_ne!(nquad_set(&a), nquad_set(&other));
    }
}
