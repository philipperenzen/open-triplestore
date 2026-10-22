//! The write gate's focused sandbox (issue #534; delta note §8.3, §9.5).
//!
//! A gate validates the graph as it will be after the write. For a merge
//! (Graph Store `POST`) or a delta (RDF Patch, a repair apply) it used to copy
//! the whole live graph into its sandbox first, so a 1 000-quad `POST` into a
//! 9M-quad graph serialised and re-parsed all nine million. A write can only
//! change the results of the focus nodes whose evaluation reads one of the
//! triples it adds or removes, so this module finds those nodes, and the
//! triples their evaluation reads, from the shapes' [`ReadPlan`]:
//!
//! 1. **Focus.** The subjects of the changed triples, plus every node from
//!    which a read of the plan reaches a changed triple: for a triple
//!    `(s, p, o)`, each site that reads `p` (or every predicate) outgoing is
//!    walked backwards from `s`, and each site that reads `p` incoming from
//!    `o`. That covers new and lost targets (`sh:targetClass` reads
//!    `rdf:type`, `sh:targetSubjectsOf` and `sh:targetObjectsOf` their
//!    predicate), inverse and sequence paths, `sh:class` on value nodes, and
//!    nested `sh:node` and `sh:qualifiedValueShape` shapes.
//! 2. **Neighbourhood.** From each focus node, every site of the plan walked
//!    forwards in the future graph, copying the triples read there, plus the
//!    `rdfs:subClassOf` chains of the plan's classes. The sandbox holds the
//!    write's additions and this neighbourhood, never a removed triple, so it
//!    is part of the future graph that holds everything the focus nodes'
//!    evaluation reads: validating them there gives what validating them in
//!    the whole future graph gives.
//!
//! Both walks read the live graph through `FutureGraph`, the live graph less
//! the write's removals plus its additions, and stop at a read budget the
//! size of the graph. A write whose reach cannot be bounded is staged whole,
//! as before ([`Plan::Whole`] says why): shapes without a plan (SPARQL
//! constraints and targets, SHACL-AF components and expressions, shapes that
//! refer back to themselves), a write that changes `rdfs:subClassOf` when a
//! shape reads class membership, and a reach past the budget.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};

use oxigraph::model::{
    GraphNameRef, NamedNode, NamedNodeRef, NamedOrBlankNodeRef, Quad, Term, TermRef,
};
use oxigraph::store::Store;

use crate::shacl::reach::{self, PrefixId, ReadPlan};
use crate::shacl::shapes::{PropertyPath, Shape};
use crate::store::TripleStore;

const RDFS_SUBCLASS: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";

/// The fewest live quads the focused path may read before it stages the
/// whole graph instead; a larger graph allows as many reads as it has quads.
const MIN_READ_BUDGET: usize = 100_000;

/// How to stage a write's future graph.
pub(crate) enum Plan {
    /// Validate `focus` in a sandbox holding the write's additions and
    /// `neighbourhood` (triples of the future graph).
    Focused {
        focus: HashSet<Term>,
        neighbourhood: Vec<Quad>,
    },
    /// Stage the whole graph, for the reason given.
    Whole(String),
}

/// How to stage the future of `graph` after a write that adds `adds` and
/// removes `removes` (disjoint, all in `graph`), validated against `shapes`
/// (every gate's, as the engine loads them). `Err` when the live graph cannot
/// be read.
pub(crate) fn plan(
    store: &TripleStore,
    graph: &NamedNode,
    shapes: &[Shape],
    adds: &[Quad],
    removes: &[Quad],
) -> Result<Plan, String> {
    let read_plan = match reach::read_plan(shapes) {
        Ok(p) => p,
        Err(why) => return Ok(Plan::Whole(why)),
    };
    if !read_plan.classes.is_empty()
        && adds
            .iter()
            .chain(removes)
            .any(|q| q.predicate.as_str() == RDFS_SUBCLASS)
    {
        return Ok(Plan::Whole(
            "the write changes rdfs:subClassOf, which decides class membership across the graph"
                .to_string(),
        ));
    }
    let budget = store
        .graph_count_cached(Some(graph.as_str()))
        .unwrap_or(0)
        .max(MIN_READ_BUDGET);
    let future = FutureGraph::new(
        store.store(),
        GraphNameRef::NamedNode(graph.as_ref()),
        adds,
        removes,
        budget,
    );
    match derive(&future, &read_plan, adds, removes) {
        Ok((focus, neighbourhood)) => Ok(Plan::Focused {
            focus,
            neighbourhood,
        }),
        Err(Stop::Budget) => Ok(Plan::Whole(format!(
            "what the write can affect takes more than {budget} reads of the graph"
        ))),
        Err(Stop::Read(e)) => Err(e),
    }
}

/// Why a walk stopped.
enum Stop {
    /// The read budget ran out.
    Budget,
    /// The live store failed.
    Read(String),
}

/// The focus nodes and the neighbourhood (see the module docs).
fn derive(
    future: &FutureGraph<'_>,
    plan: &ReadPlan,
    adds: &[Quad],
    removes: &[Quad],
) -> Result<(HashSet<Term>, Vec<Quad>), Stop> {
    // Sites by predicate, for matching changed triples.
    let mut out_by_p: HashMap<&str, Vec<PrefixId>> = HashMap::new();
    for (at, p) in &plan.out {
        out_by_p.entry(p.as_str()).or_default().push(*at);
    }
    let mut inv_by_p: HashMap<&str, Vec<PrefixId>> = HashMap::new();
    for (at, p) in &plan.inv {
        inv_by_p.entry(p.as_str()).or_default().push(*at);
    }

    // 1. Every node whose evaluation reads a changed triple.
    let mut focus: HashSet<Term> = HashSet::new();
    let mut walked: HashSet<(PrefixId, Term)> = HashSet::new();
    let mut reach_back = |at: PrefixId, x: &Term, focus: &mut HashSet<Term>| {
        if walked.insert((at, x.clone())) {
            focus.extend(future.walk_back(plan, at, x)?);
        }
        Ok::<(), Stop>(())
    };
    for q in adds.iter().chain(removes) {
        let s: Term = q.subject.clone().into();
        focus.insert(s.clone());
        let p = q.predicate.as_str();
        for &at in out_by_p.get(p).into_iter().flatten().chain(&plan.all_out) {
            reach_back(at, &s, &mut focus)?;
        }
        for &at in inv_by_p.get(p).into_iter().flatten() {
            reach_back(at, &q.object, &mut focus)?;
        }
    }

    // 2. What their evaluation reads.
    let needed = needed_prefixes(plan);
    let mut quads: HashSet<Quad> = HashSet::new();
    for f in &focus {
        let walks = future.walks(plan, &needed, f)?;
        for (id, p) in &plan.out {
            for x in walks[*id].iter().flatten() {
                quads.extend(future.outgoing(x, Some(p))?);
            }
        }
        for id in &plan.all_out {
            for x in walks[*id].iter().flatten() {
                quads.extend(future.outgoing(x, None)?);
            }
        }
        for (id, p) in &plan.inv {
            for x in walks[*id].iter().flatten() {
                quads.extend(future.incoming(x, p)?);
            }
        }
    }
    // The `rdfs:subClassOf*` closures the run computes, walked down from
    // each class as `DataView::compute_closure` walks them.
    for class in &plan.classes {
        let Ok(class) = NamedNode::new(class.as_str()) else {
            continue;
        };
        let mut seen: HashSet<Term> = HashSet::from([Term::NamedNode(class)]);
        let mut queue: VecDeque<Term> = seen.iter().cloned().collect();
        while let Some(c) = queue.pop_front() {
            for q in future.incoming(&c, RDFS_SUBCLASS)? {
                let sub: Term = q.subject.clone().into();
                if seen.insert(sub.clone()) {
                    queue.push_back(sub);
                }
                quads.insert(q);
            }
        }
    }
    Ok((focus, quads.into_iter().collect()))
}

/// The prefixes a forward walk must compute: those with a site, and their
/// ancestors.
fn needed_prefixes(plan: &ReadPlan) -> Vec<bool> {
    let mut needed = vec![false; plan.prefix_count()];
    for (id, _) in plan.out.iter().chain(&plan.inv) {
        needed[*id] = true;
    }
    for id in &plan.all_out {
        needed[*id] = true;
    }
    // Children come after their parents, so one pass from the end marks
    // every ancestor.
    for id in (1..needed.len()).rev() {
        if needed[id] {
            needed[plan.split(id).0] = true;
        }
    }
    needed
}

/// The future graph: the live graph less `removes`, plus `adds`, read
/// through the live store's indexes under a read budget.
struct FutureGraph<'a> {
    live: &'a Store,
    graph: GraphNameRef<'a>,
    adds: &'a [Quad],
    adds_out: HashMap<Term, Vec<usize>>,
    adds_in: HashMap<&'a Term, Vec<usize>>,
    removes: HashSet<&'a Quad>,
    reads: Cell<usize>,
    budget: usize,
}

impl<'a> FutureGraph<'a> {
    fn new(
        live: &'a Store,
        graph: GraphNameRef<'a>,
        adds: &'a [Quad],
        removes: &'a [Quad],
        budget: usize,
    ) -> Self {
        let mut adds_out: HashMap<Term, Vec<usize>> = HashMap::new();
        let mut adds_in: HashMap<&Term, Vec<usize>> = HashMap::new();
        for (i, q) in adds.iter().enumerate() {
            adds_out
                .entry(q.subject.clone().into())
                .or_default()
                .push(i);
            adds_in.entry(&q.object).or_default().push(i);
        }
        FutureGraph {
            live,
            graph,
            adds,
            adds_out,
            adds_in,
            removes: removes.iter().collect(),
            reads: Cell::new(0),
            budget,
        }
    }

    fn charge(&self, n: usize) -> Result<(), Stop> {
        let reads = self.reads.get() + n;
        self.reads.set(reads);
        if reads > self.budget {
            return Err(Stop::Budget);
        }
        Ok(())
    }

    /// The live quads matching the pattern, less the removed ones.
    fn live_quads(
        &self,
        subject: Option<NamedOrBlankNodeRef<'_>>,
        predicate: Option<NamedNodeRef<'_>>,
        object: Option<TermRef<'_>>,
    ) -> Result<Vec<Quad>, Stop> {
        self.charge(1)?;
        let mut out = Vec::new();
        for q in self
            .live
            .quads_for_pattern(subject, predicate, object, Some(self.graph))
        {
            let q = q.map_err(|e| Stop::Read(format!("reading the written graph: {e}")))?;
            self.charge(1)?;
            if !self.removes.contains(&q) {
                out.push(q);
            }
        }
        Ok(out)
    }

    /// Every triple `(s, p, ?)` of the future graph — every `(s, ?, ?)` when
    /// `p` is `None`. Empty for a literal.
    fn outgoing(&self, s: &Term, p: Option<&str>) -> Result<Vec<Quad>, Stop> {
        let subject = match s {
            Term::NamedNode(n) => NamedOrBlankNodeRef::NamedNode(n.as_ref()),
            Term::BlankNode(b) => NamedOrBlankNodeRef::BlankNode(b.as_ref()),
            _ => return Ok(Vec::new()),
        };
        let predicate = match p {
            Some(p) => match NamedNodeRef::new(p) {
                Ok(p) => Some(p),
                // Not an IRI: it names no stored term.
                Err(_) => return Ok(Vec::new()),
            },
            None => None,
        };
        let mut out = self.live_quads(Some(subject), predicate, None)?;
        for &i in self.adds_out.get(s).into_iter().flatten() {
            let q = &self.adds[i];
            if p.is_none_or(|p| q.predicate.as_str() == p) {
                out.push(q.clone());
            }
        }
        Ok(out)
    }

    /// Every triple `(?, p, o)` of the future graph.
    fn incoming(&self, o: &Term, p: &str) -> Result<Vec<Quad>, Stop> {
        let Ok(predicate) = NamedNodeRef::new(p) else {
            return Ok(Vec::new());
        };
        let mut out = self.live_quads(None, Some(predicate), Some(o.as_ref()))?;
        for &i in self.adds_in.get(o).into_iter().flatten() {
            let q = &self.adds[i];
            if q.predicate.as_str() == p {
                out.push(q.clone());
            }
        }
        Ok(out)
    }

    /// One predicate step, forwards or backwards.
    fn step(&self, from: &Term, p: &str, inverse: bool) -> Result<Vec<Term>, Stop> {
        Ok(if inverse {
            self.incoming(from, p)?
                .into_iter()
                .map(|q| q.subject.into())
                .collect()
        } else {
            self.outgoing(from, Some(p))?
                .into_iter()
                .map(|q| q.object)
                .collect()
        })
    }

    /// The nodes `path` (normalised) reaches from `from`, added to `out`, as
    /// `eval_path_native` evaluates it.
    fn eval(&self, from: &Term, path: &PropertyPath, out: &mut HashSet<Term>) -> Result<(), Stop> {
        match path {
            PropertyPath::Predicate(p) => out.extend(self.step(from, p, false)?),
            PropertyPath::Inverse(inner) => match inner.as_ref() {
                PropertyPath::Predicate(p) => out.extend(self.step(from, p, true)?),
                other => self.eval(from, &reach::invert(&reach::normal(other)), out)?,
            },
            PropertyPath::Sequence(parts) => {
                let mut frontier: HashSet<Term> = HashSet::from([from.clone()]);
                for part in parts {
                    let mut next = HashSet::new();
                    for node in &frontier {
                        self.eval(node, part, &mut next)?;
                    }
                    frontier = next;
                    if frontier.is_empty() {
                        break;
                    }
                }
                out.extend(frontier);
            }
            PropertyPath::Alternative(parts) => {
                for part in parts {
                    self.eval(from, part, out)?;
                }
            }
            PropertyPath::ZeroOrMore(inner) => {
                out.insert(from.clone());
                self.closure(from, inner, out)?;
            }
            PropertyPath::OneOrMore(inner) => self.closure(from, inner, out)?,
            PropertyPath::ZeroOrOne(inner) => {
                out.insert(from.clone());
                self.eval(from, inner, out)?;
            }
        }
        Ok(())
    }

    /// The nodes one or more `inner` steps from `from`.
    fn closure(
        &self,
        from: &Term,
        inner: &PropertyPath,
        out: &mut HashSet<Term>,
    ) -> Result<(), Stop> {
        let mut reached: HashSet<Term> = HashSet::new();
        let mut queue: VecDeque<Term> = VecDeque::from([from.clone()]);
        while let Some(node) = queue.pop_front() {
            let mut next = HashSet::new();
            self.eval(&node, inner, &mut next)?;
            for t in next {
                if reached.insert(t.clone()) {
                    queue.push_back(t);
                }
            }
        }
        out.extend(reached);
        Ok(())
    }

    /// The nodes from which prefix `at` reaches `x`: its elements, inverted,
    /// walked back from `x`.
    fn walk_back(&self, plan: &ReadPlan, at: PrefixId, x: &Term) -> Result<HashSet<Term>, Stop> {
        let mut frontier: HashSet<Term> = HashSet::from([x.clone()]);
        let mut id = at;
        while let (parent, Some(element)) = plan.split(id) {
            let back = reach::invert(element);
            let mut next = HashSet::new();
            for node in &frontier {
                self.eval(node, &back, &mut next)?;
            }
            frontier = next;
            id = parent;
            if frontier.is_empty() {
                break;
            }
        }
        Ok(frontier)
    }

    /// The nodes every needed prefix reaches from `focus` (`None` for a
    /// prefix no site needs).
    fn walks(
        &self,
        plan: &ReadPlan,
        needed: &[bool],
        focus: &Term,
    ) -> Result<Vec<Option<HashSet<Term>>>, Stop> {
        let mut walks: Vec<Option<HashSet<Term>>> = vec![None; plan.prefix_count()];
        walks[0] = Some(HashSet::from([focus.clone()]));
        for id in 1..plan.prefix_count() {
            if !needed[id] {
                continue;
            }
            let (parent, element) = plan.split(id);
            let element = element.expect("only the empty prefix has no element");
            let mut reached = HashSet::new();
            for node in walks[parent].iter().flatten() {
                self.eval(node, element, &mut reached)?;
            }
            walks[id] = Some(reached);
        }
        Ok(walks)
    }
}
