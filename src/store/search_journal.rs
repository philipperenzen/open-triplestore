//! What each store write touched, for the indexes derived from the store
//! that are kept outside it — today the full-text index.
//!
//! The text index used to be maintained by the callers that wrote: SPARQL
//! UPDATE, the Graph Store Protocol, imports, version restores and seeds
//! refreshed it or marked it dirty, and every other writer (LDP, RDF Patch on
//! some paths, RML runs, SHACL rule output, entailment materialisation,
//! replication, LDES sync, repair) left it stale until an unrelated write
//! forced a rebuild. This module moves the bookkeeping into the store: every
//! mutating primitive brackets itself with `TripleStore::begin_write`, and
//! while that bracket is open the primitive records what it is about to
//! change — the exact quads when it holds them, otherwise the graphs, and
//! "everything" when it cannot bound itself. When the outermost bracket on a
//! thread closes, the record is published to the store's [`SearchJournal`],
//! which the text index drains before it answers a search.
//!
//! A writer that keeps the index up to date itself (the "writer pays" paths)
//! runs its write inside a [`SearchClaim`]: the record then goes to the claim
//! instead of the journal, and the writer marks the claim handled once the
//! index reflects the write. A claim dropped without being handled — a
//! timeout, an early return, a panic — publishes its record after all, so an
//! abandoned writer leaves the index repairable rather than stale.
//!
//! Records are taken *before* the data changes (like the change log's intent
//! rows): a write that fails half-way has still announced every graph it may
//! have touched, and refreshing an untouched graph is harmless.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use oxigraph::model::{GraphName, Quad};

/// A single write's quads above this are recorded as their graphs instead.
pub const MAX_WRITE_QUADS: usize = 10_000;
/// Quads the journal holds before it falls back to graphs.
const MAX_JOURNAL_QUADS: usize = 50_000;
/// Graphs the journal holds before it falls back to "everything".
const MAX_JOURNAL_GRAPHS: usize = 10_000;

/// What one or more writes touched. `None` in `graphs` is the default graph.
#[derive(Debug, Default, Clone)]
pub struct Touched {
    /// Nothing narrower is known: the consumer rebuilds from the whole store.
    pub all: bool,
    /// Graphs whose content may have changed in any way.
    pub graphs: BTreeSet<Option<String>>,
    /// Quads that may have been inserted or deleted. The consumer reconciles
    /// each against the store's current state, so order and duplicates do
    /// not matter.
    pub quads: Vec<Quad>,
}

impl Touched {
    pub fn is_empty(&self) -> bool {
        !self.all && self.graphs.is_empty() && self.quads.is_empty()
    }

    fn everything() -> Self {
        Touched {
            all: true,
            ..Touched::default()
        }
    }

    fn merge(&mut self, other: Touched) {
        if self.all {
            return;
        }
        if other.all {
            *self = Touched::everything();
            return;
        }
        self.graphs.extend(other.graphs);
        self.quads.extend(other.quads);
    }

    /// Keep the record bounded: too many quads become their graphs, too many
    /// graphs become "everything".
    fn cap(&mut self, max_quads: usize) {
        if self.all {
            return;
        }
        if self.quads.len() > max_quads {
            let quads = std::mem::take(&mut self.quads);
            self.graphs.extend(quads.iter().map(graph_key));
        }
        if self.graphs.len() > MAX_JOURNAL_GRAPHS {
            *self = Touched::everything();
        }
    }
}

/// The graph key of `quad`: `None` for the default graph. A blank-node graph
/// name (which no write path produces) is keyed by its label.
pub fn graph_key(quad: &Quad) -> Option<String> {
    match &quad.graph_name {
        GraphName::NamedNode(n) => Some(n.as_str().to_string()),
        GraphName::BlankNode(b) => Some(format!("_:{}", b.as_str())),
        GraphName::DefaultGraph => None,
    }
}

/// The writes a derived index has not yet seen. Shared by a store's clones.
#[derive(Debug, Default)]
pub struct SearchJournal {
    /// Off until a consumer exists: a store without a text index pays nothing.
    enabled: AtomicBool,
    /// Cheap "anything pending" probe for the query path.
    pending: AtomicBool,
    state: Mutex<Touched>,
}

impl SearchJournal {
    /// Start recording. Idempotent; writes made before this are not recorded
    /// (the text index boots dirty and rebuilds from the whole store once).
    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Whether a write has been published since the last [`Self::take`].
    pub fn has_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }

    /// Everything published since the last call, leaving the journal empty.
    pub fn take(&self) -> Touched {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.pending.store(false, Ordering::Release);
        std::mem::take(&mut *state)
    }

    pub(crate) fn publish(&self, touched: Touched) {
        if touched.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.merge(touched);
        state.cap(MAX_JOURNAL_QUADS);
        self.pending.store(true, Ordering::Release);
    }
}

/// One store's write bracket open on this thread.
struct Open {
    journal: Arc<SearchJournal>,
    depth: usize,
    touched: Touched,
}

/// A claim active on this thread: the journal it claims writes of, and
/// where those writes' records go instead.
type ActiveClaim = (Arc<SearchJournal>, Arc<Mutex<Touched>>);

thread_local! {
    /// The write brackets open on this thread, one per store (a write to the
    /// main store can validate in a scratch store before it commits).
    static OPEN: RefCell<Vec<Open>> = const { RefCell::new(Vec::new()) };
    /// The claims active on this thread (see [`SearchClaim::run`]).
    static CLAIMS: RefCell<Vec<ActiveClaim>> =
        const { RefCell::new(Vec::new()) };
}

/// Open (or nest) a write bracket for `journal` on this thread. Returns
/// whether the bracket is tracked, which the caller hands back to [`leave`].
pub(crate) fn enter(journal: &Arc<SearchJournal>) -> bool {
    if !journal.enabled() {
        return false;
    }
    OPEN.with(|open| {
        let mut open = open.borrow_mut();
        if let Some(o) = open.iter_mut().find(|o| Arc::ptr_eq(&o.journal, journal)) {
            o.depth += 1;
        } else {
            open.push(Open {
                journal: journal.clone(),
                depth: 1,
                touched: Touched::default(),
            });
        }
    });
    true
}

/// Close a bracket opened by [`enter`]; the outermost one publishes the
/// write's record to the active claim for this journal, or to the journal.
pub(crate) fn leave(journal: &Arc<SearchJournal>) {
    let done = OPEN.with(|open| {
        let mut open = open.borrow_mut();
        let i = open.iter().position(|o| Arc::ptr_eq(&o.journal, journal))?;
        open[i].depth -= 1;
        (open[i].depth == 0).then(|| open.remove(i).touched)
    });
    if let Some(touched) = done {
        deliver(journal, touched);
    }
}

fn deliver(journal: &Arc<SearchJournal>, touched: Touched) {
    if touched.is_empty() {
        return;
    }
    let claim = CLAIMS.with(|c| {
        c.borrow()
            .iter()
            .rev()
            .find(|(j, _)| Arc::ptr_eq(j, journal))
            .map(|(_, buf)| buf.clone())
    });
    match claim {
        Some(buf) => {
            let mut buf = buf.lock().unwrap_or_else(|p| p.into_inner());
            buf.merge(touched);
            buf.cap(MAX_JOURNAL_QUADS);
        }
        None => journal.publish(touched),
    }
}

/// Record that the write open on this thread touches `touched`. Outside a
/// bracket (the journal was enabled mid-write) it is published directly.
pub(crate) fn record(journal: &Arc<SearchJournal>, mut touched: Touched) {
    if !journal.enabled() {
        return;
    }
    touched.cap(MAX_WRITE_QUADS);
    let rest = OPEN.with(|open| {
        let mut open = open.borrow_mut();
        match open.iter_mut().find(|o| Arc::ptr_eq(&o.journal, journal)) {
            Some(o) => {
                o.touched.merge(touched);
                None
            }
            None => Some(touched),
        }
    });
    if let Some(touched) = rest {
        deliver(journal, touched);
    }
}

/// A writer's promise to keep the derived index in step with the writes it
/// makes inside [`SearchClaim::run`] itself. Mark it [`handled`](Self::handled)
/// once it has; dropped unhandled, it publishes what those writes touched.
pub struct SearchClaim {
    journal: Arc<SearchJournal>,
    buf: Arc<Mutex<Touched>>,
    handled: bool,
}

impl SearchClaim {
    pub(crate) fn new(journal: Arc<SearchJournal>) -> Self {
        SearchClaim {
            journal,
            buf: Arc::new(Mutex::new(Touched::default())),
            handled: false,
        }
    }

    /// Run `f` with this claim active on the current thread: the store
    /// writes `f` makes are recorded here instead of in the journal.
    pub fn run<R>(&self, f: impl FnOnce() -> R) -> R {
        struct Pop;
        impl Drop for Pop {
            fn drop(&mut self) {
                CLAIMS.with(|c| {
                    c.borrow_mut().pop();
                });
            }
        }
        CLAIMS.with(|c| {
            c.borrow_mut()
                .push((self.journal.clone(), self.buf.clone()))
        });
        let _pop = Pop;
        f()
    }

    /// The writer has brought the index up to date with this claim's writes.
    pub fn handled(mut self) {
        self.handled = true;
    }
}

impl Drop for SearchClaim {
    fn drop(&mut self) {
        if self.handled {
            return;
        }
        let touched = std::mem::take(&mut *self.buf.lock().unwrap_or_else(|p| p.into_inner()));
        self.journal.publish(touched);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::model::{Literal, NamedNode};

    fn quad(g: &str) -> Quad {
        Quad::new(
            NamedNode::new_unchecked("http://example.org/s"),
            NamedNode::new_unchecked("http://example.org/p"),
            Literal::new_simple_literal("o"),
            NamedNode::new_unchecked(g),
        )
    }

    fn graphs(g: &[&str]) -> Touched {
        Touched {
            graphs: g.iter().map(|g| Some(g.to_string())).collect(),
            ..Touched::default()
        }
    }

    fn enabled() -> Arc<SearchJournal> {
        let j = Arc::new(SearchJournal::default());
        j.enable();
        j
    }

    /// Every store primitive that brackets a write records what it touches:
    /// one that does not leaves the text index stale, which is the bug this
    /// module exists to end. A new primitive has to say what it changes (or
    /// `touch_all`).
    #[test]
    fn every_mutating_store_primitive_records_what_it_touches() {
        let src = include_str!("engine.rs");
        let body_start = src.find("impl TripleStore").expect("impl block");
        let mut missing = Vec::new();
        let mut current: Option<(&str, bool, bool)> = None;
        for line in src[body_start..].lines() {
            let t = line.trim_start();
            let is_fn = line.starts_with("    pub fn ")
                || line.starts_with("    pub(crate) fn ")
                || line.starts_with("    fn ");
            if is_fn {
                if let Some((name, writes, touches)) = current.take() {
                    if writes && !touches {
                        missing.push(name.to_string());
                    }
                }
                let name = t.split('(').next().unwrap_or(t);
                current = Some((name, false, false));
            }
            if let Some((_, writes, touches)) = current.as_mut() {
                *writes |= t.contains("self.begin_write()");
                *touches |= t.contains("self.touch_");
            }
        }
        if let Some((name, writes, touches)) = current {
            if writes && !touches {
                missing.push(name.to_string());
            }
        }
        // Rebuilds a derived index; the data does not change.
        missing.retain(|n| !n.ends_with("fn rebuild_graph_index"));
        assert!(
            missing.is_empty(),
            "store primitives that write without recording it: {missing:?}"
        );
    }

    #[test]
    fn a_disabled_journal_records_nothing() {
        let j = Arc::new(SearchJournal::default());
        assert!(!enter(&j));
        record(&j, graphs(&["http://example.org/g"]));
        assert!(!j.has_pending());
    }

    #[test]
    fn the_outermost_bracket_publishes() {
        let j = enabled();
        assert!(enter(&j));
        assert!(enter(&j));
        record(&j, graphs(&["http://example.org/a"]));
        leave(&j);
        assert!(!j.has_pending(), "a nested bracket must not publish");
        record(&j, graphs(&["http://example.org/b"]));
        leave(&j);
        let t = j.take();
        assert_eq!(t.graphs.len(), 2);
        assert!(!j.has_pending());
    }

    #[test]
    fn brackets_of_two_stores_stay_apart() {
        let (main, scratch) = (enabled(), enabled());
        enter(&main);
        enter(&scratch);
        record(&scratch, graphs(&["http://example.org/scratch"]));
        leave(&scratch);
        record(&main, graphs(&["http://example.org/main"]));
        leave(&main);
        assert_eq!(
            main.take().graphs.into_iter().collect::<Vec<_>>(),
            vec![Some("http://example.org/main".to_string())]
        );
        assert_eq!(scratch.take().graphs.len(), 1);
    }

    #[test]
    fn a_handled_claim_keeps_its_writes_out_of_the_journal() {
        let j = enabled();
        let claim = SearchClaim::new(j.clone());
        claim.run(|| {
            enter(&j);
            record(&j, graphs(&["http://example.org/g"]));
            leave(&j);
        });
        assert!(!j.has_pending());
        claim.handled();
        assert!(!j.has_pending());
    }

    #[test]
    fn an_abandoned_claim_publishes_its_writes() {
        let j = enabled();
        let claim = SearchClaim::new(j.clone());
        claim.run(|| {
            enter(&j);
            record(&j, graphs(&["http://example.org/g"]));
            leave(&j);
        });
        // Moved to another thread and dropped there, as a timed-out
        // request's claim would be.
        std::thread::spawn(move || drop(claim)).join().unwrap();
        assert!(j.has_pending());
        assert_eq!(j.take().graphs.len(), 1);
    }

    #[test]
    fn writes_after_the_claim_ran_are_not_claimed() {
        let j = enabled();
        let claim = SearchClaim::new(j.clone());
        claim.run(|| ());
        enter(&j);
        record(&j, graphs(&["http://example.org/g"]));
        leave(&j);
        assert!(j.has_pending());
        claim.handled();
    }

    #[test]
    fn oversized_records_degrade_instead_of_growing() {
        let j = enabled();
        enter(&j);
        record(
            &j,
            Touched {
                quads: (0..MAX_WRITE_QUADS + 1)
                    .map(|_| quad("http://example.org/big"))
                    .collect(),
                ..Touched::default()
            },
        );
        leave(&j);
        let t = j.take();
        assert!(t.quads.is_empty());
        assert_eq!(t.graphs.len(), 1);

        let mut many = Touched::default();
        for i in 0..=MAX_JOURNAL_GRAPHS {
            many.graphs.insert(Some(format!("http://example.org/g{i}")));
        }
        j.publish(many);
        assert!(j.take().all);
    }
}
