//! One consistent data source for a whole SHACL run.
//!
//! The engine used to reach the data through a full SPARQL round trip per
//! (focus node, property path) — three parses, a fresh evaluator with forty
//! custom-function registrations and a store-wide scan for `sh:SPARQLFunction`
//! definitions, a plan compile, a result-cache mutex that never hit — plus a
//! RocksDB snapshot per raw probe, taken and released under RocksDB's global
//! mutex. Profiled on 100k focus nodes that was 73% of the worker CPU in the
//! query pipeline and 25% waiting for locks, with the data read itself at 4%.
//!
//! A [`DataView`] is created once per `validate()` call and shared by every
//! rayon worker. It reads from exactly one source for the whole run:
//!
//! * the query accelerator's clean in-memory copy of the store, when one is
//!   published (a peek — it never triggers a rebuild);
//! * otherwise, on a persistent store, one RocksDB readable transaction — a
//!   single snapshot with no locks, so every probe is a prefix seek and writers
//!   are never blocked;
//! * otherwise (the in-memory backend, whose transaction would hold the
//!   exclusive write lock) the live store, where a snapshot is an `Arc` clone.
//!
//! Value nodes are then resolved natively from the quad index for every path
//! form SHACL has, targets and `sh:class` checks come from per-run class sets,
//! and no SPARQL is evaluated on the per-focus-node path at all. SPARQL-based
//! constraints (`sh:sparql`) and SPARQL targets keep reading the live store.

use super::shapes::{Constraint, Shape, Target};
use crate::store::TripleStore;
use oxigraph::model::{
    GraphName, GraphNameRef, NamedNode, NamedNodeRef, NamedOrBlankNodeRef, Term, TermRef,
};
use oxigraph::store::{QuadIter, Store, Transaction};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

pub(crate) const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
pub(crate) const RDFS_SUBCLASS: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";

/// Which of the run's data graphs a lookup spans.
///
/// SPARQL property paths keep every hop inside one graph (`GRAPH <g> { … }`
/// per data graph, UNIONed), which is what IRI focus nodes evaluate with:
/// [`GraphSel::One`] per graph, results merged. Blank-node and literal focus
/// nodes keep the engine's historical native walk, where each hop unions over
/// every data graph ([`GraphSel::All`]); so do `sh:class` membership checks.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum GraphSel {
    All,
    One(usize),
}

/// The run's single data source (see the module docs).
pub(crate) enum RawSource<'a> {
    Mirror(Arc<Store>),
    Snapshot(Transaction<'a>),
    Live(&'a Store),
}

impl RawSource<'_> {
    pub(crate) fn quads_for_pattern(
        &self,
        subject: Option<NamedOrBlankNodeRef<'_>>,
        predicate: Option<NamedNodeRef<'_>>,
        object: Option<TermRef<'_>>,
        graph_name: Option<GraphNameRef<'_>>,
    ) -> QuadIter<'_> {
        match self {
            RawSource::Mirror(store) => {
                store.quads_for_pattern(subject, predicate, object, graph_name)
            }
            RawSource::Snapshot(tx) => tx.quads_for_pattern(subject, predicate, object, graph_name),
            RawSource::Live(store) => {
                store.quads_for_pattern(subject, predicate, object, graph_name)
            }
        }
    }

    /// Human-readable kind, for the run's log line.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            RawSource::Mirror(_) => "mirror",
            RawSource::Snapshot(_) => "snapshot",
            RawSource::Live(_) => "live",
        }
    }
}

/// Per-class run state: the `rdfs:subClassOf*` closure reaching the class, and
/// (for classes that are validation targets) the set of their SHACL instances.
struct ClassInfo {
    closure: Arc<HashSet<Term>>,
    instances: Option<Arc<HashSet<Term>>>,
}

pub(crate) struct DataView<'a> {
    pub(crate) store: &'a TripleStore,
    pub(crate) data_graphs: &'a [String],
    raw: RawSource<'a>,
    /// The data graphs as parsed graph names: the default graph when
    /// `data_graphs` is empty; an entry that is not a valid IRI is skipped (it
    /// can hold no quads), so the other graphs are still validated.
    graphs: Vec<GraphName>,
    classes: HashMap<(String, GraphSel), ClassInfo>,
}

// Every rayon worker shares one view. The RocksDB readable transaction is
// `Sync` (a snapshot plus an unused write batch); the other sources are plain
// shared references. This fails to compile, rather than at runtime, if that
// ever changes.
const _: () = {
    fn assert_sync<T: Sync>() {}
    fn check() {
        assert_sync::<DataView<'static>>();
    }
    let _ = check;
};

impl<'a> DataView<'a> {
    /// Open the run's data source (see the module docs for the choice).
    pub(crate) fn new(store: &'a TripleStore, data_graphs: &'a [String]) -> Self {
        let raw = if let Some(full) = store.mirror_full_copy() {
            RawSource::Mirror(full)
        } else if store.is_persistent() {
            match store.store().start_transaction() {
                Ok(tx) => RawSource::Snapshot(tx),
                Err(_) => RawSource::Live(store.store()),
            }
        } else {
            RawSource::Live(store.store())
        };
        let graphs: Vec<GraphName> = if data_graphs.is_empty() {
            vec![GraphName::DefaultGraph]
        } else {
            data_graphs
                .iter()
                .filter_map(|g| NamedNode::new(g).ok().map(GraphName::NamedNode))
                .collect()
        };
        Self {
            store,
            data_graphs,
            raw,
            graphs,
            classes: HashMap::new(),
        }
    }

    pub(crate) fn source_kind(&self) -> &'static str {
        self.raw.kind()
    }

    /// Number of data graphs in the run (after dropping invalid IRIs).
    pub(crate) fn graph_count(&self) -> usize {
        self.graphs.len()
    }

    fn graph_ref(&self, i: usize) -> GraphNameRef<'_> {
        self.graphs[i].as_ref()
    }

    fn for_graphs(&self, sel: GraphSel, mut f: impl FnMut(GraphNameRef<'_>)) {
        match sel {
            GraphSel::All => {
                for g in &self.graphs {
                    f(g.as_ref());
                }
            }
            GraphSel::One(i) => {
                if let Some(g) = self.graphs.get(i) {
                    f(g.as_ref());
                }
            }
        }
    }

    /// One forward (`from p ?o`) or inverse (`?s p from`) predicate step over the
    /// quad index of the run's source, scoped by `sel`.
    pub(crate) fn step(
        &self,
        from: &Term,
        predicate: &str,
        inverse: bool,
        sel: GraphSel,
    ) -> Vec<Term> {
        let Ok(pred) = NamedNodeRef::new(predicate) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if inverse {
            let obj = from.as_ref();
            self.for_graphs(sel, |graph| {
                for q in self
                    .raw
                    .quads_for_pattern(None, Some(pred), Some(obj), Some(graph))
                    .flatten()
                {
                    out.push(match q.subject {
                        oxigraph::model::NamedOrBlankNode::NamedNode(nn) => Term::NamedNode(nn),
                        oxigraph::model::NamedOrBlankNode::BlankNode(bn) => Term::BlankNode(bn),
                    });
                }
            });
        } else {
            let subj: NamedOrBlankNodeRef<'_> = match from {
                Term::NamedNode(nn) => NamedOrBlankNodeRef::NamedNode(nn.as_ref()),
                Term::BlankNode(bn) => NamedOrBlankNodeRef::BlankNode(bn.as_ref()),
                _ => return out, // literals have no outgoing edges
            };
            self.for_graphs(sel, |graph| {
                for q in self
                    .raw
                    .quads_for_pattern(Some(subj), Some(pred), None, Some(graph))
                    .flatten()
                {
                    out.push(q.object);
                }
            });
        }
        out
    }

    /// All `(predicate, object)` pairs of `focus` in the selected graphs — used
    /// by `sh:closed`. Works for IRI and blank-node focus nodes alike.
    pub(crate) fn subject_predicate_objects(
        &self,
        focus: &Term,
        sel: GraphSel,
    ) -> Vec<(String, Term)> {
        let subj: NamedOrBlankNodeRef<'_> = match focus {
            Term::NamedNode(nn) => NamedOrBlankNodeRef::NamedNode(nn.as_ref()),
            Term::BlankNode(bn) => NamedOrBlankNodeRef::BlankNode(bn.as_ref()),
            _ => return Vec::new(),
        };
        let mut out = Vec::new();
        self.for_graphs(sel, |graph| {
            for q in self
                .raw
                .quads_for_pattern(Some(subj), None, None, Some(graph))
                .flatten()
            {
                out.push((q.predicate.as_str().to_string(), q.object));
            }
        });
        out
    }

    /// Subjects with `rdf:type class` in the selected graphs. Objects of the same
    /// predicate scan for `sh:targetObjectsOf` come from [`Self::objects_of`].
    pub(crate) fn subjects_of(&self, predicate: &str, sel: GraphSel) -> Vec<Term> {
        let Ok(pred) = NamedNodeRef::new(predicate) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        self.for_graphs(sel, |graph| {
            for q in self
                .raw
                .quads_for_pattern(None, Some(pred), None, Some(graph))
                .flatten()
            {
                out.push(match q.subject {
                    oxigraph::model::NamedOrBlankNode::NamedNode(nn) => Term::NamedNode(nn),
                    oxigraph::model::NamedOrBlankNode::BlankNode(bn) => Term::BlankNode(bn),
                });
            }
        });
        out
    }

    /// Objects of every `?s predicate ?o` quad in the selected graphs (typed
    /// literals preserved, so a literal focus node keeps its datatype).
    pub(crate) fn objects_of(&self, predicate: &str, sel: GraphSel) -> Vec<Term> {
        let Ok(pred) = NamedNodeRef::new(predicate) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        self.for_graphs(sel, |graph| {
            for q in self
                .raw
                .quads_for_pattern(None, Some(pred), None, Some(graph))
                .flatten()
            {
                out.push(q.object);
            }
        });
        out
    }

    // ------------------------------------------------------------------
    // Classes
    // ------------------------------------------------------------------

    /// The classes whose `rdfs:subClassOf*` chain reaches `class_iri` (the class
    /// itself included), walked over the selected graphs. Blank-node classes
    /// (`ex:x a [ rdfs:subClassOf ex:C ]`) are kept: SPARQL's path finds them,
    /// and SHACL's instance definition (§2.1.3.1) is the same path.
    fn compute_closure(&self, class_iri: &str, sel: GraphSel) -> HashSet<Term> {
        let mut set = HashSet::new();
        let Ok(class) = NamedNode::new(class_iri) else {
            // Not an IRI: the class can match nothing in the data, but the
            // constraint still names it; keep the bare term so `sh:class`
            // against an already-typed value behaves as it always has.
            set.insert(Term::NamedNode(NamedNode::new_unchecked(class_iri)));
            return set;
        };
        let start = Term::NamedNode(class);
        let mut queue = VecDeque::new();
        set.insert(start.clone());
        queue.push_back(start);
        while let Some(node) = queue.pop_front() {
            for sub in self.step(&node, RDFS_SUBCLASS, true, sel) {
                if set.insert(sub.clone()) {
                    queue.push_back(sub);
                }
            }
        }
        set
    }

    /// Every subject typed (directly) with any class of `closure` in graph `gi`.
    fn scan_instances(&self, closure: &HashSet<Term>, gi: usize) -> HashSet<Term> {
        let ty = NamedNodeRef::new_unchecked(RDF_TYPE);
        let graph = self.graph_ref(gi);
        let mut out = HashSet::new();
        for class in closure {
            let class_ref: TermRef<'_> = match class {
                Term::NamedNode(nn) => TermRef::NamedNode(nn.as_ref()),
                Term::BlankNode(bn) => TermRef::BlankNode(bn.as_ref()),
                _ => continue,
            };
            for q in self
                .raw
                .quads_for_pattern(None, Some(ty), Some(class_ref), Some(graph))
                .flatten()
            {
                out.insert(match q.subject {
                    oxigraph::model::NamedOrBlankNode::NamedNode(nn) => Term::NamedNode(nn),
                    oxigraph::model::NamedOrBlankNode::BlankNode(bn) => Term::BlankNode(bn),
                });
            }
        }
        out
    }

    /// Precompute the class state every shape in `shapes` needs — closures for
    /// each `sh:class`, closures plus instance sets for each `sh:targetClass`,
    /// per data graph — in parallel, before the constraint fan-out. The map is
    /// immutable for the rest of the run, so the hot path takes no lock.
    pub(crate) fn prepare(&mut self, shapes: &[Shape]) {
        let mut needed: HashSet<(String, GraphSel)> = HashSet::new();
        for shape in shapes {
            collect_classes(shape, self.graphs.len(), &mut needed);
        }
        let mut keys: Vec<(String, GraphSel)> = needed
            .into_iter()
            .filter(|k| !self.classes.contains_key(k))
            .collect();
        keys.sort_by(|a, b| a.0.cmp(&b.0).then(sel_order(a.1).cmp(&sel_order(b.1))));
        let single_graph = self.graphs.len() == 1;
        let computed: Vec<((String, GraphSel), ClassInfo)> = keys
            .par_iter()
            .map(|(class, sel)| {
                let closure = self.compute_closure(class, *sel);
                let instances = match sel {
                    GraphSel::One(i) => Some(Arc::new(self.scan_instances(&closure, *i))),
                    // With a single data graph the "all graphs" instance set is
                    // that graph's; computed here so `sh:class` on a targeted
                    // class is one set lookup per value instead of a type probe.
                    GraphSel::All if single_graph => {
                        Some(Arc::new(self.scan_instances(&closure, 0)))
                    }
                    GraphSel::All => None,
                };
                (
                    (class.clone(), *sel),
                    ClassInfo {
                        closure: Arc::new(closure),
                        instances,
                    },
                )
            })
            .collect();
        self.classes.extend(computed);
    }

    /// `rdfs:subClassOf*` closure of `class_iri` over `sel` (prepared, or
    /// computed on the spot for a class no shape declared).
    pub(crate) fn subclass_closure(&self, class_iri: &str, sel: GraphSel) -> Arc<HashSet<Term>> {
        if let Some(info) = self.classes.get(&(class_iri.to_string(), sel)) {
            return info.closure.clone();
        }
        Arc::new(self.compute_closure(class_iri, sel))
    }

    /// The prepared instance set of `class_iri` over `sel`, if one was built.
    pub(crate) fn instances_of(
        &self,
        class_iri: &str,
        sel: GraphSel,
    ) -> Option<Arc<HashSet<Term>>> {
        self.classes
            .get(&(class_iri.to_string(), sel))
            .and_then(|info| info.instances.clone())
    }

    /// SHACL instance check: `term rdf:type/rdfs:subClassOf* class` over every
    /// data graph. A prepared instance set answers it with one lookup; otherwise
    /// the node's types come from the index and are matched against the closure.
    pub(crate) fn is_instance_of(&self, term: &Term, class_iri: &str) -> bool {
        match term {
            Term::NamedNode(_) | Term::BlankNode(_) => {
                if let Some(set) = self.instances_of(class_iri, GraphSel::All) {
                    return set.contains(term);
                }
                let closure = self.subclass_closure(class_iri, GraphSel::All);
                self.step(term, RDF_TYPE, false, GraphSel::All)
                    .iter()
                    .any(|ty| closure.contains(ty))
            }
            _ => false,
        }
    }
}

fn sel_order(sel: GraphSel) -> usize {
    match sel {
        GraphSel::All => usize::MAX,
        GraphSel::One(i) => i,
    }
}

/// Walk a shape and everything nested in it, recording the class keys the run
/// will ask for: `(class, One(i))` for every data graph of a target class,
/// `(class, All)` for every `sh:class`.
fn collect_classes(shape: &Shape, graph_count: usize, out: &mut HashSet<(String, GraphSel)>) {
    for target in &shape.targets {
        if let Target::TargetClass(c) = target {
            for i in 0..graph_count {
                out.insert((c.clone(), GraphSel::One(i)));
            }
        }
    }
    for c in &shape.constraints {
        collect_constraint_classes(c, graph_count, out);
    }
    for ps in &shape.property_shapes {
        for c in &ps.constraints {
            collect_constraint_classes(c, graph_count, out);
        }
    }
}

fn collect_constraint_classes(
    constraint: &Constraint,
    graph_count: usize,
    out: &mut HashSet<(String, GraphSel)>,
) {
    match constraint {
        Constraint::Class(c) => {
            out.insert((c.clone(), GraphSel::All));
        }
        Constraint::Not(s) | Constraint::Node(s) => collect_classes(s, graph_count, out),
        Constraint::And(v) | Constraint::Or(v) | Constraint::Xone(v) => {
            for s in v {
                collect_classes(s, graph_count, out);
            }
        }
        Constraint::QualifiedValueShape {
            shape,
            sibling_shapes,
            ..
        } => {
            collect_classes(shape, graph_count, out);
            for s in sibling_shapes {
                collect_classes(s, graph_count, out);
            }
        }
        Constraint::Property(ps) => {
            for c in &ps.constraints {
                collect_constraint_classes(c, graph_count, out);
            }
        }
        Constraint::Expression { checks, .. } => {
            for c in checks {
                collect_constraint_classes(c, graph_count, out);
            }
        }
        _ => {}
    }
}
