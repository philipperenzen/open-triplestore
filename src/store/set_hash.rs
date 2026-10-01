//! An order-independent hash of a set of quads.
//!
//! Each quad is printed canonically — `<graph> s p o`, every term in its
//! N-Triples form (triple terms included, literals escaped, nothing
//! truncated) — hashed with SHA-256, and the digests are summed modulo
//! 2^256. Because the sum does not depend on order, the hash of a set can be
//! moved by a delta without reading the set: adding a quad adds its digest,
//! removing one subtracts it. That is what lets a delta version carry a
//! content hash (`ver:contentHash`) computed from its base and its chain,
//! without being materialised (delta note §3.5), and lets a patch name the
//! state it applies to (`H base-digest`).
//!
//! It is an integrity check — it detects a chain that drifted from the data
//! it describes — not a cryptographic commitment: an additive hash can be
//! steered by someone who chooses the quads.

use oxigraph::model::{GraphName, Quad, Term};
use sha2::{Digest, Sha256};

/// The running hash of a set: four little-endian 64-bit limbs of a 256-bit
/// integer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetHash([u64; 4]);

impl SetHash {
    /// The hash of the empty set.
    pub fn new() -> Self {
        Self::default()
    }

    fn digest(element: &str) -> [u64; 4] {
        let bytes = Sha256::digest(element.as_bytes());
        let mut limbs = [0u64; 4];
        for (i, limb) in limbs.iter_mut().enumerate() {
            let mut b = [0u8; 8];
            b.copy_from_slice(&bytes[i * 8..i * 8 + 8]);
            *limb = u64::from_le_bytes(b);
        }
        limbs
    }

    /// The canonical element for a quad held under `graph` — the graph it
    /// belongs to in the version's own terms (a snapshot's *source* graph),
    /// whatever graph the quad sits in physically.
    pub fn element(graph: &str, subject: &str, predicate: &str, object: &str) -> String {
        format!("<{graph}> {subject} {predicate} {object}")
    }

    /// The element of `quad` under `graph` (`None`: the quad's own graph).
    pub fn quad_element(quad: &Quad, graph: Option<&str>) -> String {
        let g = match graph {
            Some(g) => g.to_string(),
            None => match &quad.graph_name {
                GraphName::NamedNode(n) => n.as_str().to_string(),
                _ => String::new(),
            },
        };
        let o: &Term = &quad.object;
        Self::element(
            &g,
            &quad.subject.to_string(),
            &quad.predicate.to_string(),
            &o.to_string(),
        )
    }

    pub fn add_element(&mut self, element: &str) {
        let d = Self::digest(element);
        let mut carry = 0u64;
        for (limb, x) in self.0.iter_mut().zip(d) {
            let (s1, c1) = limb.overflowing_add(x);
            let (s2, c2) = s1.overflowing_add(carry);
            *limb = s2;
            carry = (c1 as u64) + (c2 as u64);
        }
    }

    pub fn remove_element(&mut self, element: &str) {
        let d = Self::digest(element);
        let mut borrow = 0u64;
        for (limb, x) in self.0.iter_mut().zip(d) {
            let (s1, b1) = limb.overflowing_sub(x);
            let (s2, b2) = s1.overflowing_sub(borrow);
            *limb = s2;
            borrow = (b1 as u64) + (b2 as u64);
        }
    }

    /// The hash of the union of two disjoint sets: the sum of their hashes.
    pub fn merge(&mut self, other: &SetHash) {
        let mut carry = 0u64;
        for (limb, x) in self.0.iter_mut().zip(other.0) {
            let (s1, c1) = limb.overflowing_add(x);
            let (s2, c2) = s1.overflowing_add(carry);
            *limb = s2;
            carry = (c1 as u64) + (c2 as u64);
        }
    }

    pub fn add_quad(&mut self, quad: &Quad, graph: Option<&str>) {
        self.add_element(&Self::quad_element(quad, graph));
    }

    pub fn remove_quad(&mut self, quad: &Quad, graph: Option<&str>) {
        self.remove_element(&Self::quad_element(quad, graph));
    }

    /// The hash of every quad of `graphs` as they are now, each under its
    /// own graph. A full scan of each graph.
    pub fn of_graphs(store: &crate::store::TripleStore, graphs: &[String]) -> Self {
        let mut h = Self::new();
        for g in graphs {
            let Ok(name) = oxigraph::model::NamedNodeRef::new(g) else {
                continue;
            };
            for q in store
                .store()
                .quads_for_pattern(
                    None,
                    None,
                    None,
                    Some(oxigraph::model::GraphNameRef::NamedNode(name)),
                )
                .flatten()
            {
                h.add_quad(&q, Some(g));
            }
        }
        h
    }

    /// 64 lower-case hex digits, most significant first.
    pub fn to_hex(&self) -> String {
        self.0.iter().rev().map(|l| format!("{l:016x}")).collect()
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let mut limbs = [0u64; 4];
        for (i, limb) in limbs.iter_mut().rev().enumerate() {
            *limb = u64::from_str_radix(&s[i * 16..i * 16 + 16], 16).ok()?;
        }
        Some(Self(limbs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::model::{Literal, NamedNode};

    fn q(n: u32) -> Quad {
        Quad::new(
            NamedNode::new(format!("http://e/s{n}")).unwrap(),
            NamedNode::new("http://e/p").unwrap(),
            Literal::new_simple_literal(format!("v{n}")),
            GraphName::NamedNode(NamedNode::new("http://e/snap").unwrap()),
        )
    }

    #[test]
    fn order_does_not_matter_and_a_removal_undoes_an_addition() {
        let mut a = SetHash::new();
        a.add_quad(&q(1), Some("http://e/src"));
        a.add_quad(&q(2), Some("http://e/src"));
        let mut b = SetHash::new();
        b.add_quad(&q(2), Some("http://e/src"));
        b.add_quad(&q(1), Some("http://e/src"));
        assert_eq!(a, b);
        b.add_quad(&q(3), Some("http://e/src"));
        assert_ne!(a, b);
        b.remove_quad(&q(3), Some("http://e/src"));
        assert_eq!(a, b);
        // Removing from the empty set and adding back wraps around to zero.
        let mut z = SetHash::new();
        z.remove_quad(&q(9), None);
        z.add_quad(&q(9), None);
        assert_eq!(z, SetHash::new());
    }

    #[test]
    fn the_graph_a_quad_is_hashed_under_is_its_source_graph() {
        let mut physical = SetHash::new();
        physical.add_quad(&q(1), None);
        let mut source = SetHash::new();
        source.add_quad(&q(1), Some("http://e/src"));
        assert_ne!(physical, source, "the snapshot IRI is not the content");
    }

    #[test]
    fn hex_round_trips() {
        let mut h = SetHash::new();
        h.add_quad(&q(7), None);
        let hex = h.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(SetHash::from_hex(&hex), Some(h));
        assert_eq!(SetHash::from_hex("xyz"), None);
    }
}
