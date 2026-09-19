//! The columnar copy: a dictionary of terms and three sorted permutations of
//! the quads, graph-first, as flat arrays of `u32` ids. Around 48 bytes per
//! quad plus the dictionary, against roughly a kilobyte per quad in an
//! in-memory oxigraph store — and a triple pattern is a binary search, not
//! a key-encoded B-tree walk.

use std::collections::HashMap;

use oxrdf::{GraphName, Quad, Term};

/// The id of the default graph in the graph column.
pub const DEFAULT_GRAPH: u32 = 0;

/// Terms interned to dense ids. Id `0` is the default graph's marker and
/// never a term.
#[derive(Default)]
pub struct Dictionary {
    terms: Vec<Term>,
    index: HashMap<Term, u32>,
}

impl Dictionary {
    pub fn new() -> Self {
        let mut d = Self::default();
        // Slot 0: the default graph. Never looked up as a term.
        d.terms
            .push(Term::NamedNode(oxrdf::NamedNode::new_unchecked(
                "urn:opengraph:default-graph",
            )));
        d
    }

    pub fn intern(&mut self, term: &Term) -> u32 {
        if let Some(id) = self.index.get(term) {
            return *id;
        }
        let id = self.terms.len() as u32;
        self.terms.push(term.clone());
        self.index.insert(term.clone(), id);
        id
    }

    pub fn lookup(&self, term: &Term) -> Option<u32> {
        self.index.get(term).copied()
    }

    pub fn get(&self, id: u32) -> Option<&Term> {
        self.terms.get(id as usize)
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.len() <= 1
    }
}

/// A quad as ids: `[g, s, p, o]`.
pub type Row = [u32; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Perm {
    /// graph, subject, predicate, object
    Gspo,
    /// graph, predicate, object, subject
    Gpos,
    /// graph, object, subject, predicate
    Gosp,
}

/// The copy. Built once from a quad iterator; immutable afterwards.
pub struct Columnar {
    dict: Dictionary,
    /// `[g, s, p, o]` sorted lexicographically.
    gspo: Vec<Row>,
    /// `[g, p, o, s]` sorted.
    gpos: Vec<Row>,
    /// `[g, o, s, p]` sorted.
    gosp: Vec<Row>,
    /// Every graph id present (the default graph only when it holds quads).
    graphs: Vec<u32>,
    quads: usize,
}

impl Columnar {
    /// Build from quads. Duplicates collapse.
    pub fn from_quads<I: IntoIterator<Item = Quad>>(quads: I) -> Self {
        let mut dict = Dictionary::new();
        let mut gspo: Vec<Row> = Vec::new();
        for q in quads {
            let g = match &q.graph_name {
                GraphName::DefaultGraph => DEFAULT_GRAPH,
                GraphName::NamedNode(n) => dict.intern(&Term::NamedNode(n.clone())),
                GraphName::BlankNode(b) => dict.intern(&Term::BlankNode(b.clone())),
            };
            let s = dict.intern(&Term::from(q.subject));
            let p = dict.intern(&Term::NamedNode(q.predicate));
            let o = dict.intern(&q.object);
            gspo.push([g, s, p, o]);
        }
        Self::from_rows(dict, gspo)
    }

    fn from_rows(dict: Dictionary, mut gspo: Vec<Row>) -> Self {
        use rayon::prelude::*;
        gspo.par_sort_unstable();
        gspo.dedup();
        let mut gpos: Vec<Row> = gspo.par_iter().map(|r| [r[0], r[2], r[3], r[1]]).collect();
        gpos.par_sort_unstable();
        let mut gosp: Vec<Row> = gspo.par_iter().map(|r| [r[0], r[3], r[1], r[2]]).collect();
        gosp.par_sort_unstable();
        let mut graphs: Vec<u32> = gspo.iter().map(|r| r[0]).collect();
        graphs.dedup();
        let quads = gspo.len();
        Self {
            dict,
            gspo,
            gpos,
            gosp,
            graphs,
            quads,
        }
    }

    pub fn len(&self) -> usize {
        self.quads
    }

    pub fn is_empty(&self) -> bool {
        self.quads == 0
    }

    pub fn dict(&self) -> &Dictionary {
        &self.dict
    }

    /// Every graph that holds a quad, the default graph included as `0`.
    pub fn graphs(&self) -> &[u32] {
        &self.graphs
    }

    /// Named graphs only.
    pub fn named_graphs(&self) -> impl Iterator<Item = u32> + '_ {
        self.graphs.iter().copied().filter(|g| *g != DEFAULT_GRAPH)
    }

    /// Rough memory use, for the cap arithmetic.
    pub fn bytes(&self) -> usize {
        self.quads * 3 * std::mem::size_of::<Row>() + self.dict.len() * 96
    }

    /// The rows of `perm` whose leading columns equal `prefix` (1 to 3
    /// bound leading columns), as `[g, s, p, o]` regardless of the
    /// permutation. The graph must be bound: callers iterate graphs.
    pub fn scan(&self, perm: Perm, prefix: &[u32]) -> impl Iterator<Item = Row> + '_ {
        let table: &[Row] = match perm {
            Perm::Gspo => &self.gspo,
            Perm::Gpos => &self.gpos,
            Perm::Gosp => &self.gosp,
        };
        let (lo, hi) = range_of(table, prefix);
        table[lo..hi].iter().map(move |r| match perm {
            Perm::Gspo => *r,
            Perm::Gpos => [r[0], r[3], r[1], r[2]],
            Perm::Gosp => [r[0], r[2], r[3], r[1]],
        })
    }

    /// How many rows `scan` would yield (a binary search, no iteration).
    pub fn count(&self, perm: Perm, prefix: &[u32]) -> usize {
        let table: &[Row] = match perm {
            Perm::Gspo => &self.gspo,
            Perm::Gpos => &self.gpos,
            Perm::Gosp => &self.gosp,
        };
        let (lo, hi) = range_of(table, prefix);
        hi - lo
    }

    /// Whether the quad `[g, s, p, o]` exists.
    pub fn contains(&self, row: Row) -> bool {
        self.gspo.binary_search(&row).is_ok()
    }

    /// The permutation and prefix for a pattern with the given bound columns
    /// (the graph always bound). Returns the permutation and the prefix in
    /// that permutation's column order.
    pub fn plan(g: u32, s: Option<u32>, p: Option<u32>, o: Option<u32>) -> (Perm, Vec<u32>) {
        match (s, p, o) {
            (Some(s), Some(p), Some(o)) => (Perm::Gspo, vec![g, s, p, o]),
            (Some(s), Some(p), None) => (Perm::Gspo, vec![g, s, p]),
            (Some(s), None, Some(o)) => (Perm::Gosp, vec![g, o, s]),
            (Some(s), None, None) => (Perm::Gspo, vec![g, s]),
            (None, Some(p), Some(o)) => (Perm::Gpos, vec![g, p, o]),
            (None, Some(p), None) => (Perm::Gpos, vec![g, p]),
            (None, None, Some(o)) => (Perm::Gosp, vec![g, o]),
            (None, None, None) => (Perm::Gspo, vec![g]),
        }
    }
}

/// The half-open index range of rows whose leading columns equal `prefix`.
fn range_of(table: &[Row], prefix: &[u32]) -> (usize, usize) {
    let n = prefix.len().min(4);
    if n == 0 {
        return (0, table.len());
    }
    let lo = table.partition_point(|r| r[..n] < prefix[..n]);
    let hi = table.partition_point(|r| r[..n] <= prefix[..n]);
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{Literal, NamedNode};

    fn q(s: u32, p: &str, o: u32, g: Option<&str>) -> Quad {
        Quad::new(
            NamedNode::new_unchecked(format!("http://x/s{s}")),
            NamedNode::new_unchecked(format!("http://x/{p}")),
            Literal::from(o as i64),
            match g {
                Some(g) => GraphName::NamedNode(NamedNode::new_unchecked(g)),
                None => GraphName::DefaultGraph,
            },
        )
    }

    #[test]
    fn scans_answer_every_bound_pattern_and_dedup_holds() {
        let c = Columnar::from_quads(vec![
            q(1, "p", 1, None),
            q(1, "p", 2, None),
            q(2, "p", 1, None),
            q(2, "q", 3, Some("http://g/a")),
            q(2, "q", 3, Some("http://g/a")),
        ]);
        assert_eq!(c.len(), 4);
        assert_eq!(c.graphs().len(), 2);
        let d = c.dict();
        let s1 = d
            .lookup(&Term::NamedNode(NamedNode::new_unchecked("http://x/s1")))
            .unwrap();
        let p = d
            .lookup(&Term::NamedNode(NamedNode::new_unchecked("http://x/p")))
            .unwrap();
        let one = d.lookup(&Term::Literal(Literal::from(1i64))).unwrap();
        let (perm, prefix) = Columnar::plan(DEFAULT_GRAPH, Some(s1), None, None);
        assert_eq!(c.scan(perm, &prefix).count(), 2);
        let (perm, prefix) = Columnar::plan(DEFAULT_GRAPH, None, Some(p), Some(one));
        let rows: Vec<Row> = c.scan(perm, &prefix).collect();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r[2] == p && r[3] == one));
        let (perm, prefix) = Columnar::plan(DEFAULT_GRAPH, None, None, Some(one));
        assert_eq!(c.count(perm, &prefix), 2);
        assert!(c.contains([DEFAULT_GRAPH, s1, p, one]));
        let ga = d
            .lookup(&Term::NamedNode(NamedNode::new_unchecked("http://g/a")))
            .unwrap();
        let (perm, prefix) = Columnar::plan(ga, None, None, None);
        assert_eq!(c.scan(perm, &prefix).count(), 1);
    }
}
