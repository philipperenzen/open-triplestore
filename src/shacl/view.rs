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

use super::shapes::{Constraint, PropertyPath, Shape, Target};
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
    /// Per-run adjacency for the shape predicates, built for the snapshot and
    /// live sources when the run is large enough to pay for it (see
    /// [`RunIndex`]). `None` on the mirror source, whose probes are RAM lookups.
    index: Option<RunIndex>,
}

/// Adjacency lists for the predicates the run's shapes traverse, one scan per
/// (data graph, predicate) over the run's snapshot: `fwd[g][p][s] = objects`
/// and `inv[g][p][o] = subjects`. Answers a probe with one hash lookup instead
/// of a RocksDB prefix seek plus four term decodes, which at 600 000 probes per
/// run is the difference between a 2.3 s and a sub-second validation. A pair
/// is present only when its scan completed within the budget, and is then
/// authoritative for that graph and predicate; anything else falls through to
/// the raw source.
/// `predicate -> node -> neighbours` for one data graph.
type Adjacency = HashMap<String, HashMap<Term, Vec<Term>>>;

struct RunIndex {
    fwd: Vec<Adjacency>,
    inv: Vec<Adjacency>,
}

/// When and how large the run index may be. Read from the environment once
/// per run: `OTS_SHACL_RUN_INDEX_MIN_PROBES` (build only when the run will
/// make at least this many probes, default 20 000) and
/// `OTS_SHACL_RUN_INDEX_MAX_QUADS` (total quads the index may hold; default
/// derived from the memory limit like the accelerator's cap, 1M when the
/// limit is unknown, never below 250k or above 8M).
#[derive(Clone, Copy, Debug)]
pub(crate) struct IndexPolicy {
    pub min_probes: usize,
    pub max_quads: usize,
}

impl IndexPolicy {
    pub(crate) fn from_env() -> Self {
        let min_probes = std::env::var("OTS_SHACL_RUN_INDEX_MIN_PROBES")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(20_000);
        let max_quads = std::env::var("OTS_SHACL_RUN_INDEX_MAX_QUADS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| {
                crate::store::parallel_mirror::detect_memory_limit_bytes()
                    .map(|b| (b / 8 / 300) as usize)
                    .unwrap_or(1_000_000)
                    .clamp(250_000, 8_000_000)
            });
        Self {
            min_probes,
            max_quads,
        }
    }
}

thread_local! {
    /// Test hook: overrides [`IndexPolicy::from_env`] for runs started on this
    /// thread, so a test can force the index on (or off) without touching the
    /// process environment.
    static INDEX_POLICY_OVERRIDE: std::cell::Cell<Option<IndexPolicy>> = const { std::cell::Cell::new(None) };
}

/// Force the run-index policy for validations started on the current thread
/// (`None` restores the environment-derived policy). Test hook.
#[cfg(test)]
pub(crate) fn set_index_policy_override(policy: Option<IndexPolicy>) {
    INDEX_POLICY_OVERRIDE.with(|c| c.set(policy));
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
            index: None,
        }
    }

    /// Whether the run reads the accelerator's RAM copy.
    pub(crate) fn is_mirror(&self) -> bool {
        matches!(self.raw, RawSource::Mirror(_))
    }

    /// Whether a run index was built (diagnostics).
    pub(crate) fn has_index(&self) -> bool {
        self.index.is_some()
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
        if let Some(hit) = self.step_indexed(from, predicate, inverse, sel) {
            return hit;
        }
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

    /// Answer a step from the run index when it holds the (graph, predicate)
    /// pair(s) the selection needs; `None` means "ask the raw source".
    fn step_indexed(
        &self,
        from: &Term,
        predicate: &str,
        inverse: bool,
        sel: GraphSel,
    ) -> Option<Vec<Term>> {
        let ix = self.index.as_ref()?;
        let maps = if inverse { &ix.inv } else { &ix.fwd };
        match sel {
            GraphSel::One(i) => {
                let m = maps.get(i)?.get(predicate)?;
                Some(m.get(from).cloned().unwrap_or_default())
            }
            GraphSel::All => {
                if !maps.iter().all(|g| g.contains_key(predicate)) {
                    return None;
                }
                let mut out = Vec::new();
                for g in maps {
                    if let Some(v) = g.get(predicate).and_then(|m| m.get(from)) {
                        out.extend(v.iter().cloned());
                    }
                }
                Some(out)
            }
        }
    }

    /// Build the run index for the predicates `shapes` traverse, sized by the
    /// targets already resolved: skipped on the mirror source, below the
    /// policy's probe threshold, or (on the live memory source) when a write
    /// overlaps the scans. Each (graph, predicate, direction) pair is one scan;
    /// pairs that would push the index over its quad budget are dropped and
    /// answered from the raw source instead.
    pub(crate) fn build_index(&mut self, shapes: &[Shape], targeted: &[(&Shape, Vec<Term>)]) {
        if self.is_mirror() {
            return;
        }
        let policy = INDEX_POLICY_OVERRIDE
            .with(|c| c.get())
            .unwrap_or_else(IndexPolicy::from_env);
        let _ = shapes;
        let mut pairs: HashSet<(String, bool)> = HashSet::new();
        let mut probes: usize = 0;
        for (shape, focus) in targeted {
            let mut own: HashSet<(String, bool)> = HashSet::new();
            collect_shape_predicates(shape, &mut own);
            probes = probes.saturating_add(focus.len().saturating_mul(own.len()));
            pairs.extend(own);
        }
        if pairs.is_empty() || probes < policy.min_probes {
            return;
        }
        let gen_before = self.store.write_generation();
        if !self.consistent_source() && self.store.writes_in_flight() > 0 {
            return;
        }
        let used = std::sync::atomic::AtomicUsize::new(0);
        let budget = policy.max_quads;
        let mut pairs: Vec<(String, bool)> = pairs.into_iter().collect();
        pairs.sort();
        let graph_count = self.graphs.len();
        // (pair, graph) -> adjacency, or None when the budget ran out.
        type Built = ((String, bool), usize, Option<HashMap<Term, Vec<Term>>>);
        let built: Vec<Built> = pairs
            .par_iter()
            .flat_map_iter(|pair| (0..graph_count).map(move |gi| (pair.clone(), gi)))
            .map(|((pred, inverse), gi)| {
                let map = self.scan_pair(&pred, inverse, gi, &used, budget);
                ((pred, inverse), gi, map)
            })
            .collect();
        if !self.consistent_source()
            && (self.store.write_generation() != gen_before || self.store.writes_in_flight() > 0)
        {
            tracing::debug!("SHACL run index dropped: a write overlapped its scans");
            return;
        }
        let mut fwd: Vec<Adjacency> = (0..graph_count).map(|_| HashMap::new()).collect();
        let mut inv = fwd.clone();
        let mut dropped = Vec::new();
        for ((pred, inverse), gi, map) in built {
            match map {
                Some(m) => {
                    let target = if inverse { &mut inv[gi] } else { &mut fwd[gi] };
                    target.insert(pred, m);
                }
                None => dropped.push(format!("{}{pred}", if inverse { "^" } else { "" })),
            }
        }
        if !dropped.is_empty() {
            dropped.sort();
            dropped.dedup();
            tracing::info!(
                "SHACL run index: {} quads held; over the {budget}-quad budget, answered from the store instead: {}",
                used.load(std::sync::atomic::Ordering::Relaxed),
                dropped.join(", ")
            );
        }
        self.index = Some(RunIndex { fwd, inv });
    }

    /// Whether every read of the run sees one snapshot (mirror copy or RocksDB
    /// transaction). The live memory source snapshots per call.
    fn consistent_source(&self) -> bool {
        !matches!(self.raw, RawSource::Live(_))
    }

    /// One `?s <pred> ?o` scan of graph `gi`, into `subject -> objects`
    /// (`inverse`: `object -> subjects`). `None` once the shared quad budget is
    /// exhausted — the partial map is discarded.
    fn scan_pair(
        &self,
        predicate: &str,
        inverse: bool,
        gi: usize,
        used: &std::sync::atomic::AtomicUsize,
        budget: usize,
    ) -> Option<HashMap<Term, Vec<Term>>> {
        let Ok(pred) = NamedNodeRef::new(predicate) else {
            // Not an IRI: no quad can carry it; an empty, authoritative map.
            return Some(HashMap::new());
        };
        let graph = self.graph_ref(gi);
        let mut map: HashMap<Term, Vec<Term>> = HashMap::new();
        let mut n = 0usize;
        let mut push = |subject: Term, object: Term| -> bool {
            n += 1;
            if n.is_multiple_of(1024)
                && used.fetch_add(1024, std::sync::atomic::Ordering::Relaxed) + 1024 > budget
            {
                return false;
            }
            if inverse {
                map.entry(object).or_default().push(subject);
            } else {
                map.entry(subject).or_default().push(object);
            }
            true
        };
        if let RawSource::Snapshot(tx) = &self.raw {
            // On RocksDB a raw quad scan decodes all four terms of every quad
            // — four point lookups on the id2str column family, two of them
            // for the constant predicate and graph — and that decode, not the
            // seek, is what a snapshot-path run pays for. A projected SPARQL
            // query on the same transaction decodes only the two variables.
            let query = format!(
                "SELECT ?s ?o WHERE {{ {} }}",
                graph_scoped_pattern(graph, &format!("?s <{}> ?o", pred.as_str()))?
            );
            let solutions = self
                .store
                .query_options()
                .parse_query(&query)
                .ok()?
                .on_transaction(tx)
                .execute()
                .ok()?;
            let oxigraph::sparql::QueryResults::Solutions(rows) = solutions else {
                return None;
            };
            for row in rows.flatten() {
                let (Some(s), Some(o)) = (row.get("s"), row.get("o")) else {
                    continue;
                };
                if !push(s.clone(), o.clone()) {
                    return None;
                }
            }
        } else {
            for q in self
                .raw
                .quads_for_pattern(None, Some(pred), None, Some(graph))
                .flatten()
            {
                let subject = match q.subject {
                    oxigraph::model::NamedOrBlankNode::NamedNode(nn) => Term::NamedNode(nn),
                    oxigraph::model::NamedOrBlankNode::BlankNode(bn) => Term::BlankNode(bn),
                };
                if !push(subject, q.object) {
                    return None;
                }
            }
        }
        used.fetch_add(n % 1024, std::sync::atomic::Ordering::Relaxed);
        Some(map)
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
            if let (RawSource::Snapshot(tx), Term::NamedNode(c)) = (&self.raw, class) {
                // Same reasoning as in `scan_pair`: decode one term per row.
                let pattern =
                    graph_scoped_pattern(graph, &format!("?s <{RDF_TYPE}> <{}>", c.as_str()));
                let rows = pattern.and_then(|pattern| {
                    let query = format!("SELECT ?s WHERE {{ {pattern} }}");
                    match self
                        .store
                        .query_options()
                        .parse_query(&query)
                        .ok()?
                        .on_transaction(tx)
                        .execute()
                        .ok()?
                    {
                        oxigraph::sparql::QueryResults::Solutions(rows) => Some(rows),
                        _ => None,
                    }
                });
                if let Some(rows) = rows {
                    for row in rows.flatten() {
                        if let Some(s) = row.get("s") {
                            out.insert(s.clone());
                        }
                    }
                    continue;
                }
            }
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

/// Every (predicate, inverse) pair a shape's paths and property-pair
/// constraints step through, nested shapes included. `rdf:type` steps for
/// classes are served by the class sets, not the index.
fn collect_shape_predicates(shape: &Shape, out: &mut HashSet<(String, bool)>) {
    for c in &shape.constraints {
        collect_constraint_predicates(c, out);
    }
    for ps in &shape.property_shapes {
        collect_path_predicates(&ps.path, false, out);
        for c in &ps.constraints {
            collect_constraint_predicates(c, out);
        }
    }
}

fn collect_constraint_predicates(constraint: &Constraint, out: &mut HashSet<(String, bool)>) {
    match constraint {
        Constraint::Equals(p)
        | Constraint::Disjoint(p)
        | Constraint::LessThan(p)
        | Constraint::LessThanOrEquals(p) => {
            out.insert((p.clone(), false));
        }
        Constraint::Not(s) | Constraint::Node(s) => collect_shape_predicates(s, out),
        Constraint::And(v) | Constraint::Or(v) | Constraint::Xone(v) => {
            for s in v {
                collect_shape_predicates(s, out);
            }
        }
        Constraint::QualifiedValueShape {
            shape,
            sibling_shapes,
            ..
        } => {
            collect_shape_predicates(shape, out);
            for s in sibling_shapes {
                collect_shape_predicates(s, out);
            }
        }
        Constraint::Property(ps) => {
            collect_path_predicates(&ps.path, false, out);
            for c in &ps.constraints {
                collect_constraint_predicates(c, out);
            }
        }
        Constraint::Expression { path, checks, .. } => {
            collect_path_predicates(path, false, out);
            for c in checks {
                collect_constraint_predicates(c, out);
            }
        }
        _ => {}
    }
}

fn collect_path_predicates(path: &PropertyPath, inverse: bool, out: &mut HashSet<(String, bool)>) {
    match path {
        PropertyPath::Predicate(p) => {
            out.insert((p.clone(), inverse));
        }
        PropertyPath::Inverse(inner) => collect_path_predicates(inner, !inverse, out),
        PropertyPath::Sequence(parts) | PropertyPath::Alternative(parts) => {
            for p in parts {
                collect_path_predicates(p, inverse, out);
            }
        }
        PropertyPath::ZeroOrMore(inner)
        | PropertyPath::OneOrMore(inner)
        | PropertyPath::ZeroOrOne(inner) => collect_path_predicates(inner, inverse, out),
    }
}

/// `body` inside `GRAPH <g> { … }`, or bare for the default graph. `None` for
/// a blank-node graph name, which SPARQL cannot address (the view never
/// holds one — data graphs are IRIs or the default graph).
fn graph_scoped_pattern(graph: GraphNameRef<'_>, body: &str) -> Option<String> {
    match graph {
        GraphNameRef::NamedNode(g) => Some(format!("GRAPH <{}> {{ {body} }}", g.as_str())),
        GraphNameRef::DefaultGraph => Some(body.to_string()),
        GraphNameRef::BlankNode(_) => None,
    }
}
