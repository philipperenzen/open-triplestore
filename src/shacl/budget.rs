//! One memory budget for what a SHACL validation run holds (issue #532).
//!
//! Besides the data it reads, a whole-dataset run holds four things that grow
//! with the data: the class closures and instance sets it prepares, the run
//! index ([`super::view`], adjacency for the shape predicates), the memo of
//! property-path closures, and the results. Each used to have its own bound or
//! none — the index a quad count derived as `limit / 8 / 300` bytes (about
//! 600 bytes a quad in fact), the memo 2 M terms, the rest nothing — so
//! together they reached the container's limit: BSBM 10k in 4 GiB peaked at
//! 3.4 GiB of anonymous memory, 1.1 GiB of it the index.
//!
//! Now all of them count, in estimated bytes, against one pool sized from the
//! memory limit like the query accelerator's cap ([`default_run_memory_bytes`],
//! `OTS_SHACL_RUN_MEMORY_BYTES`). The pool is process-wide, so runs that
//! overlap share it rather than each taking a full share. What a part cannot
//! reserve it does without, and the run stays correct:
//!
//! * the class sets are charged, not refused (the targets are read from them),
//!   before the index is built, so the index gets what they leave;
//! * the run index may fill the pool to [`INDEX_SHARE_PERCENT`]; it drops the
//!   (graph, predicate) pairs that do not fit and answers them from the store
//!   ([`super::view`]);
//! * the closure memo stops keeping closures and recomputes them;
//! * the results are never dropped: they are charged, so the memo of the same
//!   run and the index of an overlapping one make room for them, and a run
//!   whose results take the pool past its size says so in its log line.
//!
//! The estimates follow the allocations the structures make (a `Term` inline
//! plus its strings rounded to the allocator's chunk size), checked against
//! the process's anonymous memory on BSBM 10k (docs/perf-data, issue #532).
//! A run that held more than [`TRIM_AFTER_BYTES`] gives the freed memory back
//! to the system when it ends ([`release_freed_memory`]): glibc keeps freed
//! chunks in its arenas, and without that every whole-dataset run started
//! from the high-water mark of the one before.

use super::report::ValidationResult;
use super::shapes::PropertyPath;
use oxigraph::model::Term;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Share of the memory limit the pool takes (1/`MEMORY_DIVISOR`): the same
/// quarter the query accelerator's two copies may take, which never coexist
/// with a large run's index (a run on the accelerator's copy builds none).
const MEMORY_DIVISOR: u64 = 4;
/// The pool where no memory limit can be detected.
const FALLBACK_BYTES: u64 = 1 << 30;
/// The smallest derived pool: a small container still indexes small runs.
const MIN_BYTES: u64 = 64 << 20;
/// The largest derived pool: past it a run's index costs more memory than the
/// seeks it saves are worth (the old quad cap stopped at 8 M quads, about
/// this much).
const MAX_BYTES: u64 = 8 << 30;
/// How full the run index may make the pool, in percent.
pub(crate) const INDEX_SHARE_PERCENT: usize = 75;
/// A run that held at least this much gives the freed memory back when it
/// ends.
pub(crate) const TRIM_AFTER_BYTES: usize = 64 << 20;

/// The memory-derived default of `OTS_SHACL_RUN_MEMORY_BYTES`: a quarter of
/// the memory limit (the cgroup limit in a container), 1 GiB when none can be
/// detected, never below 64 MiB or above 8 GiB.
pub(crate) fn default_run_memory_bytes() -> u64 {
    crate::store::parallel_mirror::detect_memory_limit_bytes()
        .map(|b| b / MEMORY_DIVISOR)
        .unwrap_or(FALLBACK_BYTES)
        .clamp(MIN_BYTES, MAX_BYTES)
}

/// What draws from a run's budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Classes,
    Index,
    Memo,
    Results,
}

impl Part {
    const ALL: [Part; 4] = [Part::Classes, Part::Index, Part::Memo, Part::Results];

    fn slot(self) -> usize {
        match self {
            Part::Classes => 0,
            Part::Index => 1,
            Part::Memo => 2,
            Part::Results => 3,
        }
    }
}

/// The bytes every run of the process holds at the moment.
#[derive(Default)]
struct Pool {
    used: AtomicUsize,
}

fn process_pool() -> Arc<Pool> {
    static POOL: std::sync::OnceLock<Arc<Pool>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| Arc::new(Pool::default())).clone()
}

thread_local! {
    /// Test hook: runs started on this thread get a private pool of this
    /// size instead of the process pool (see [`set_budget_override`]).
    static BUDGET_OVERRIDE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Give validations started on the current thread a private pool of `bytes`
/// (`None` restores the process pool and the configured size). Test hook.
#[cfg(test)]
pub(crate) fn set_budget_override(bytes: Option<usize>) {
    BUDGET_OVERRIDE.with(|c| c.set(bytes));
}

/// One run's draw on the pool. Everything it holds goes back when it drops.
pub(crate) struct RunBudget {
    pool: Arc<Pool>,
    capacity: usize,
    held: [AtomicUsize; 4],
    /// Bytes a part asked for and did not get (diagnostics).
    refused: [AtomicUsize; 4],
}

impl RunBudget {
    /// The budget of a run starting now: the process pool, sized by
    /// `OTS_SHACL_RUN_MEMORY_BYTES` (or a test's private pool).
    pub(crate) fn for_run() -> Self {
        match BUDGET_OVERRIDE.with(|c| c.get()) {
            Some(bytes) => Self::private(bytes),
            None => Self::new(
                process_pool(),
                usize::try_from(crate::config::SHACL_RUN_MEMORY_BYTES.get()).unwrap_or(usize::MAX),
            ),
        }
    }

    /// A budget of `capacity` bytes that shares nothing with other runs.
    pub(crate) fn private(capacity: usize) -> Self {
        Self::new(Arc::new(Pool::default()), capacity)
    }

    fn new(pool: Arc<Pool>, capacity: usize) -> Self {
        Self {
            pool,
            capacity,
            held: Default::default(),
            refused: Default::default(),
        }
    }

    /// The pool's size.
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    /// Reserve `bytes` for `part` if the pool has room; `false` (and nothing
    /// reserved) if not. The run index, built before any result exists, may
    /// fill the pool to [`INDEX_SHARE_PERCENT`] only, so the results and the
    /// memo of the run it serves find room left.
    pub(crate) fn try_reserve(&self, part: Part, bytes: usize) -> bool {
        let ceiling = match part {
            Part::Index => self.capacity / 100 * INDEX_SHARE_PERCENT,
            Part::Classes | Part::Memo | Part::Results => self.capacity,
        };
        let mut used = self.pool.used.load(Ordering::Relaxed);
        loop {
            let next = used.saturating_add(bytes);
            if next > ceiling {
                self.refused[part.slot()].fetch_add(bytes, Ordering::Relaxed);
                return false;
            }
            match self.pool.used.compare_exchange_weak(
                used,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.held[part.slot()].fetch_add(bytes, Ordering::Relaxed);
                    return true;
                }
                Err(now) => used = now,
            }
        }
    }

    /// Count `bytes` against the pool whether or not it has room (what a run
    /// cannot do without: its results).
    pub(crate) fn charge(&self, part: Part, bytes: usize) {
        self.pool.used.fetch_add(bytes, Ordering::AcqRel);
        self.held[part.slot()].fetch_add(bytes, Ordering::Relaxed);
    }

    /// Give back `bytes` `part` reserved earlier.
    pub(crate) fn release(&self, part: Part, bytes: usize) {
        let held = &self.held[part.slot()];
        let bytes = bytes.min(held.load(Ordering::Relaxed));
        held.fetch_sub(bytes, Ordering::Relaxed);
        self.pool.used.fetch_sub(bytes, Ordering::AcqRel);
    }

    /// What `part` holds.
    pub(crate) fn held(&self, part: Part) -> usize {
        self.held[part.slot()].load(Ordering::Relaxed)
    }

    /// What `part` was refused.
    pub(crate) fn refused(&self, part: Part) -> usize {
        self.refused[part.slot()].load(Ordering::Relaxed)
    }

    /// Whether the pool is past its size (the results took it there).
    pub(crate) fn over(&self) -> bool {
        self.pool.used.load(Ordering::Relaxed) > self.capacity
    }

    /// Everything this run holds.
    pub(crate) fn total(&self) -> usize {
        Part::ALL.iter().map(|p| self.held(*p)).sum()
    }
}

impl Drop for RunBudget {
    fn drop(&mut self) {
        for part in Part::ALL {
            let held = self.held[part.slot()].swap(0, Ordering::Relaxed);
            self.pool.used.fetch_sub(held, Ordering::AcqRel);
        }
    }
}

// ---------------------------------------------------------------------------
// Size estimates
// ---------------------------------------------------------------------------

/// Bytes a heap allocation of `len` bytes takes: glibc's 8-byte chunk header,
/// rounded up to its 16-byte alignment, 32 bytes at least.
pub(crate) const fn alloc_bytes(len: usize) -> usize {
    if len == 0 {
        0
    } else {
        let n = (len + 8 + 15) & !15;
        if n < 32 {
            32
        } else {
            n
        }
    }
}

/// Heap bytes behind a term (its strings), without the inline `Term` itself.
pub(crate) fn term_heap_bytes(term: &Term) -> usize {
    match term {
        Term::NamedNode(n) => alloc_bytes(n.as_str().len()),
        // A named blank node keeps its label on the heap; an anonymous one
        // (`[]`, generated ids) is inline. Counting the label either way errs
        // high by one small allocation.
        Term::BlankNode(b) => alloc_bytes(b.as_str().len()),
        Term::Literal(l) => {
            let extra = match l.language() {
                Some(lang) => alloc_bytes(lang.len()),
                None if l.datatype().as_str() == XSD_STRING => 0,
                None => alloc_bytes(l.datatype().as_str().len()),
            };
            alloc_bytes(l.value().len()) + extra
        }
        #[allow(unreachable_patterns)]
        other => alloc_bytes(other.to_string().len()) + std::mem::size_of::<Term>() * 3,
    }
}

/// A term held by value: inline plus its strings.
pub(crate) fn term_bytes(term: &Term) -> usize {
    std::mem::size_of::<Term>() + term_heap_bytes(term)
}

/// Heap bytes behind a property path.
pub(crate) fn path_heap_bytes(path: &PropertyPath) -> usize {
    let boxed = alloc_bytes(std::mem::size_of::<PropertyPath>());
    match path {
        PropertyPath::Predicate(p) => alloc_bytes(p.len()),
        PropertyPath::Inverse(p)
        | PropertyPath::ZeroOrMore(p)
        | PropertyPath::OneOrMore(p)
        | PropertyPath::ZeroOrOne(p) => boxed + path_heap_bytes(p),
        PropertyPath::Sequence(v) | PropertyPath::Alternative(v) => {
            alloc_bytes(v.len() * std::mem::size_of::<PropertyPath>())
                + v.iter().map(path_heap_bytes).sum::<usize>()
        }
    }
}

/// Estimated bytes of one validation result: the struct, its display
/// strings and the typed terms behind them. A result about a long literal
/// holds it twice (the display `value` and the `Term`).
pub(crate) fn result_bytes(r: &ValidationResult) -> usize {
    let s = |v: &str| alloc_bytes(v.len());
    let o = |v: &Option<String>| v.as_deref().map_or(0, s);
    let t = |v: &Option<Term>| v.as_ref().map_or(0, term_heap_bytes);
    std::mem::size_of::<ValidationResult>()
        + s(&r.focus_node)
        + o(&r.path)
        + o(&r.value)
        + s(&r.source_shape)
        + s(&r.source_constraint)
        + s(&r.source_constraint_component)
        + s(&r.message)
        + r.annotations
            .iter()
            .map(|a| alloc_bytes(std::mem::size_of_val(a)) + s(&a.property) + s(&a.value))
            .sum::<usize>()
        + t(&r.terms.focus_node)
        + t(&r.terms.value)
        + r.terms.path.as_ref().map_or(0, |p| {
            alloc_bytes(std::mem::size_of::<PropertyPath>()) + path_heap_bytes(p)
        })
        + t(&r.terms.source_shape)
        + t(&r.terms.source_constraint)
        + o(&r.terms.severity)
        + r.terms
            .annotations
            .iter()
            .map(|(p, v)| s(p.as_str()) + term_bytes(v))
            .sum::<usize>()
}

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// Estimated bytes of a set of terms: each slot (a `Term` and a control
/// byte) at the table's average occupancy, about two thirds, plus the
/// terms' strings.
pub(crate) fn term_set_bytes<'a>(terms: impl ExactSizeIterator<Item = &'a Term>) -> usize {
    let slots = terms.len() * (std::mem::size_of::<Term>() + 1) * 3 / 2;
    slots + terms.map(term_heap_bytes).sum::<usize>()
}

/// Give the memory the allocator holds free back to the system (glibc's
/// `malloc_trim`, which also returns the free pages inside its arenas, not
/// just the top of the heap). A no-op off Linux with glibc.
pub(crate) fn release_freed_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only walks the allocator's own free lists; it takes
    // no pointer and is safe to call from any thread at any time.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Bytes of the process's anonymous memory (`RssAnon`), for the run's log
/// line; `None` off Linux.
pub(crate) fn process_anon_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("RssAnon:"))?;
    let kb: u64 = line
        .trim_start_matches("RssAnon:")
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kb * 1024)
}

/// `bytes` in MiB, for log lines.
pub(crate) fn mib(bytes: u64) -> u64 {
    bytes >> 20
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::model::{Literal, NamedNode};

    #[test]
    fn reservations_share_one_pool_and_come_back_on_drop() {
        let pool = Arc::new(Pool::default());
        let a = RunBudget::new(pool.clone(), 1000);
        let b = RunBudget::new(pool.clone(), 1000);
        assert!(a.try_reserve(Part::Index, 600));
        // The other run sees what the first holds.
        assert!(!b.try_reserve(Part::Index, 500));
        assert_eq!(b.refused(Part::Index), 500);
        assert!(b.try_reserve(Part::Memo, 400));
        assert!(!a.try_reserve(Part::Memo, 1));
        a.release(Part::Index, 100);
        assert!(b.try_reserve(Part::Memo, 100));
        // Results are charged even past the size, and the pool says so.
        b.charge(Part::Results, 5000);
        assert!(b.over());
        assert_eq!(b.held(Part::Results), 5000);
        drop(b);
        assert_eq!(pool.used.load(Ordering::Relaxed), 500);
        drop(a);
        assert_eq!(pool.used.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn the_index_leaves_a_quarter_for_the_results_and_the_memo() {
        let run = RunBudget::private(1000);
        assert!(!run.try_reserve(Part::Index, 751));
        assert!(run.try_reserve(Part::Index, 750));
        assert!(!run.try_reserve(Part::Index, 1));
        assert!(run.try_reserve(Part::Memo, 250));
        assert_eq!(run.held(Part::Index) + run.held(Part::Memo), 1000);
    }

    #[test]
    fn a_release_never_gives_back_more_than_was_held() {
        let pool = Arc::new(Pool::default());
        let a = RunBudget::new(pool.clone(), 1000);
        let b = RunBudget::new(pool.clone(), 1000);
        assert!(b.try_reserve(Part::Index, 300));
        assert!(a.try_reserve(Part::Index, 100));
        a.release(Part::Index, 1000);
        assert_eq!(a.held(Part::Index), 0);
        assert_eq!(
            pool.used.load(Ordering::Relaxed),
            300,
            "b's bytes stay held"
        );
    }

    #[test]
    fn term_estimates_follow_the_strings() {
        let iri = Term::NamedNode(NamedNode::new_unchecked("http://example.org/a"));
        assert_eq!(term_heap_bytes(&iri), alloc_bytes(20));
        let plain = Term::Literal(Literal::new_simple_literal("x".repeat(1000)));
        assert_eq!(term_heap_bytes(&plain), alloc_bytes(1000));
        let typed = Term::Literal(Literal::new_typed_literal(
            "1",
            NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#integer"),
        ));
        assert_eq!(term_heap_bytes(&typed), alloc_bytes(1) + alloc_bytes(40));
        let tagged = Term::Literal(Literal::new_language_tagged_literal_unchecked("chat", "fr"));
        assert_eq!(term_heap_bytes(&tagged), alloc_bytes(4) + alloc_bytes(2));
        assert_eq!(alloc_bytes(0), 0);
        assert_eq!(alloc_bytes(1), 32);
        assert_eq!(alloc_bytes(24), 32);
        assert_eq!(alloc_bytes(25), 48);
    }

    #[test]
    fn the_default_is_a_quarter_of_the_limit_within_bounds() {
        let d = default_run_memory_bytes();
        assert!(d >= MIN_BYTES);
        if let Some(limit) = crate::store::parallel_mirror::detect_memory_limit_bytes() {
            assert_eq!(d, (limit / MEMORY_DIVISOR).clamp(MIN_BYTES, MAX_BYTES));
        }
    }
}
