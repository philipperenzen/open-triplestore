//! The graph a validation reads.
//!
//! [`Data`] is all the engine asks of its input: a node's arcs out and in,
//! and the triple patterns a ShapeMap's node selectors use. [`StoreData`]
//! reads a [`TripleStore`] through a [`GraphScope`] — the named graphs the
//! caller may read, merged — and nothing else; [`GraphData`] reads an
//! in-memory graph (the conformance runner, and ShExR imports).

use std::collections::HashSet;

use oxigraph::model::{
    Graph, GraphNameRef, NamedNode, NamedNodeRef, NamedOrBlankNodeRef, Quad, Term, TermRef, Triple,
};

use crate::store::TripleStore;

/// The graphs a validation reads.
#[derive(Debug, Clone)]
pub enum GraphScope {
    /// Every graph, the default graph included: an admin's view of the store.
    All,
    /// These named graphs, merged, and nothing else.
    Graphs(Vec<NamedNode>),
}

impl GraphScope {
    /// The named graphs among `iris`, in a stable order. A name that is not an
    /// IRI can hold no quads and is dropped.
    pub fn named(iris: impl IntoIterator<Item = String>) -> Self {
        let mut graphs: Vec<NamedNode> = iris
            .into_iter()
            .filter_map(|g| NamedNode::new(g).ok())
            .collect();
        graphs.sort();
        graphs.dedup();
        GraphScope::Graphs(graphs)
    }

    /// Whether the scope reads the named graph `g`.
    pub fn includes(&self, g: &str) -> bool {
        match self {
            GraphScope::All => true,
            GraphScope::Graphs(gs) => gs.iter().any(|n| n.as_str() == g),
        }
    }
}

/// What the validator reads.
pub trait Data {
    /// `(predicate, object)` of every triple with subject `n`.
    fn arcs_out(&self, n: &Term) -> Vec<(NamedNode, Term)>;
    /// `(subject, predicate)` of every triple with object `n`.
    fn arcs_in(&self, n: &Term) -> Vec<(Term, NamedNode)>;
    /// Subjects of triples `?s p o` (`o` = `None` matches any object).
    fn subjects(&self, p: &NamedNode, o: Option<&Term>) -> Vec<Term>;
    /// Objects of triples `s p ?o` (`s` = `None` matches any subject).
    fn objects(&self, s: Option<&Term>, p: &NamedNode) -> Vec<Term>;
}

fn subject_ref(t: &Term) -> Option<NamedOrBlankNodeRef<'_>> {
    match t {
        Term::NamedNode(n) => Some(n.as_ref().into()),
        Term::BlankNode(b) => Some(b.as_ref().into()),
        _ => None,
    }
}

fn dedup<T: Clone + Eq + std::hash::Hash>(v: impl Iterator<Item = T>) -> Vec<T> {
    let mut seen = HashSet::new();
    v.filter(|x| seen.insert(x.clone())).collect()
}

/// A store read through a [`GraphScope`]. A triple held by two graphs of the
/// scope is one triple of the merged data.
pub struct StoreData<'a> {
    pub store: &'a TripleStore,
    pub scope: &'a GraphScope,
}

impl StoreData<'_> {
    fn triples(
        &self,
        s: Option<NamedOrBlankNodeRef<'_>>,
        p: Option<NamedNodeRef<'_>>,
        o: Option<TermRef<'_>>,
    ) -> Vec<Triple> {
        let quads: Box<dyn Iterator<Item = Quad> + '_> = match self.scope {
            GraphScope::All => Box::new(
                self.store
                    .store()
                    .quads_for_pattern(s, p, o, None)
                    .flatten(),
            ),
            GraphScope::Graphs(graphs) => Box::new(graphs.iter().flat_map(move |g| {
                self.store
                    .store()
                    .quads_for_pattern(s, p, o, Some(GraphNameRef::NamedNode(g.as_ref())))
                    .flatten()
            })),
        };
        dedup(quads.map(Triple::from))
    }
}

impl Data for StoreData<'_> {
    fn arcs_out(&self, n: &Term) -> Vec<(NamedNode, Term)> {
        let Some(s) = subject_ref(n) else {
            return Vec::new();
        };
        self.triples(Some(s), None, None)
            .into_iter()
            .map(|t| (t.predicate, t.object))
            .collect()
    }

    fn arcs_in(&self, n: &Term) -> Vec<(Term, NamedNode)> {
        self.triples(None, None, Some(n.as_ref()))
            .into_iter()
            .map(|t| (t.subject.into(), t.predicate))
            .collect()
    }

    fn subjects(&self, p: &NamedNode, o: Option<&Term>) -> Vec<Term> {
        dedup(
            self.triples(None, Some(p.as_ref()), o.map(Term::as_ref))
                .into_iter()
                .map(|t| t.subject.into()),
        )
    }

    fn objects(&self, s: Option<&Term>, p: &NamedNode) -> Vec<Term> {
        let s = match s {
            Some(t) => match subject_ref(t) {
                Some(r) => Some(r),
                None => return Vec::new(),
            },
            None => None,
        };
        dedup(
            self.triples(s, Some(p.as_ref()), None)
                .into_iter()
                .map(|t| t.object),
        )
    }
}

/// An in-memory graph.
pub struct GraphData<'a>(pub &'a Graph);

impl Data for GraphData<'_> {
    fn arcs_out(&self, n: &Term) -> Vec<(NamedNode, Term)> {
        let Some(s) = subject_ref(n) else {
            return Vec::new();
        };
        self.0
            .triples_for_subject(s)
            .map(|t| (t.predicate.into_owned(), t.object.into_owned()))
            .collect()
    }

    fn arcs_in(&self, n: &Term) -> Vec<(Term, NamedNode)> {
        self.0
            .triples_for_object(n.as_ref())
            .map(|t| (t.subject.into_owned().into(), t.predicate.into_owned()))
            .collect()
    }

    fn subjects(&self, p: &NamedNode, o: Option<&Term>) -> Vec<Term> {
        dedup(
            self.0
                .triples_for_predicate(p.as_ref())
                .filter(|t| o.is_none_or(|o| t.object == o.as_ref()))
                .map(|t| t.subject.into_owned().into()),
        )
    }

    fn objects(&self, s: Option<&Term>, p: &NamedNode) -> Vec<Term> {
        dedup(
            self.0
                .triples_for_predicate(p.as_ref())
                .filter(|t| s.is_none_or(|s| Term::from(t.subject.into_owned()) == *s))
                .map(|t| t.object.into_owned()),
        )
    }
}
