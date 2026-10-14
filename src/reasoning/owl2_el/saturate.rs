//! EL++ saturation: the completion rules of Baader, Brandt and Lutz
//! ("Pushing the EL Envelope", IJCAI 2005; "… Further", OWLED 2008) over
//! normalized axioms, run to a fixed point with a worklist.
//!
//! Every concept that needs its subsumers gets a *context*: the set `S(X)` of
//! concepts known to subsume `X`, and its links `X →r Y` ("every `X` has an
//! `r`-successor in `Y`"). Contexts are shared: the filler of an existential
//! is one context however many concepts point at it, which is what keeps the
//! closure polynomial. An individual is a context of its own (its nominal),
//! and its asserted property edges are links between individual contexts.
//!
//! | Rule | Premises                                    | Conclusion        |
//! |------|---------------------------------------------|-------------------|
//! | CR1  | `A ∈ S(X)`, `A ⊑ B`                         | `B ∈ S(X)`        |
//! | CR2  | `A₁, A₂ ∈ S(X)`, `A₁ ⊓ A₂ ⊑ B`              | `B ∈ S(X)`        |
//! | CR3  | `A ∈ S(X)`, `A ⊑ ∃r.B`                      | `X →r B'`         |
//! | CR4  | `X →r Y`, `A ∈ S(Y)`, `∃r.A ⊑ B`            | `B ∈ S(X)`        |
//! | CR5  | `X →r Y`, `⊥ ∈ S(Y)`                        | `⊥ ∈ S(X)`        |
//! | CR10 | `X →r Y`, `r ⊑ s`                           | `X →s Y`          |
//! | CR11 | `X →r Y`, `Y →s Z`, `r ∘ s ⊑ t`             | `X →t Z`          |
//! | RNG  | `X →r Y`, `Y` an individual, `ran(r) = C`   | `C ∈ S(Y)`        |
//! | REF  | `r` reflexive                               | a self loop `X →r X` |
//! | SELF | a self loop `X →r X`                        | `ran(r) ∈ S(X)`; `B ∈ S(X)` for `∃r.Self ⊑ B` |
//!
//! `B'` in CR3 is `B` itself when no range applies to `r`, else a fresh
//! context `B ⊓ ran(r)` for every range of `r` and its super-roles (the
//! range-restriction transformation of "… Further"). Ranges are never added
//! to a shared filler context: `A ⊑ ∃r.B` with `ran(r) = C` says the
//! successor is a `B ⊓ C`, not that every `B` is a `C`. Transitivity is the
//! chain `r ∘ r ⊑ r`; longer chains are binarized with fresh roles by the
//! loader.
//!
//! # Nominals, self restrictions and data
//!
//! An individual or a data value is a *nominal*: a context that stands for
//! exactly one element. Two rules make nominals behave as equality:
//!
//! - **NOM-in** (sound everywhere): `{a} ∈ S(X)` — every `X` *is* `a` — so `X`
//!   gets all of `S({a})` and `a`'s links.
//! - **NOM-out** (CR6 of "Pushing the EL Envelope"): when `X` is known to be
//!   non-empty — it is `⊤`, a nominal, or reachable from one by links — and
//!   `{a} ∈ S(X)`, then `a` is in `X`: `X ∈ S({a})`.
//!
//! Together they are complete for the consequences about individuals.
//! For a class `C` they can miss a subsumption that holds only because two
//! contexts reachable from `C` are forced to be the same nominal once `C`
//! has an instance (ELK leaves this case out). [`Saturation::needs_hypothesis`]
//! detects it and [`Saturation::hypothetical_subsumers`] decides it the
//! textbook way: `O ⊨ C ⊑ D` iff `O ∪ {h : C} ⊨ h : D` for a fresh individual
//! `h`, saturated on a copy of the state.
//!
//! `∃r.Self` is a separate kind of link, since `X →r X` between shared
//! contexts only says an `X` has an `r`-successor that is an `X`. Self loops
//! close under the role hierarchy and binary chains, and an edge between two
//! nominals that are the same element is one.
//!
//! Literals are nominals whose told superclasses are the EL datatypes that
//! contain their value; two different values, or a value and a datatype that
//! does not contain it, in one context give `⊥`. A functional data property
//! merges the successors of a context into one fresh context.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// FxHash: a multiplicative hasher for the integer keys the saturation uses
/// everywhere. Not DoS-resistant, which does not matter for ids we assign.
#[derive(Default, Clone, Copy)]
pub(crate) struct FxHasher(u64);

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }
    fn write_u8(&mut self, n: u8) {
        self.add(n as u64)
    }
    fn write_u32(&mut self, n: u32) {
        self.add(n as u64)
    }
    fn write_u64(&mut self, n: u64) {
        self.add(n)
    }
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64)
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type FxBuild = BuildHasherDefault<FxHasher>;
pub(crate) type FxMap<K, V> = HashMap<K, V, FxBuild>;
pub(crate) type FxSet<K> = HashSet<K, FxBuild>;

/// A concept id. `TOP` and `BOTTOM` are fixed; the loader assigns the rest.
pub(crate) type Cid = u32;
/// A role (object or data property) id.
pub(crate) type Rid = u32;

pub(crate) const TOP: Cid = 0;
pub(crate) const BOTTOM: Cid = 1;

/// Normalized axioms. Every class expression the loader reads has a concept
/// id of its own, so every axiom here relates atomic ids:
/// `A ⊑ B`, `A₁ ⊓ A₂ ⊑ B`, `A ⊑ ∃r.B`, `∃r.A ⊑ B`, role inclusions,
/// binary chains, reflexivity and ranges.
#[derive(Clone, Default)]
pub(crate) struct Axioms {
    n_concepts: u32,
    n_roles: u32,
    /// `A ⊑ B`, keyed by `A`.
    pub sub: FxMap<Cid, Vec<Cid>>,
    /// `A₁ ⊓ A₂ ⊑ B`, keyed by both `A₁` (→ `(A₂, B)`) and `A₂` (→ `(A₁, B)`).
    pub conj: FxMap<Cid, Vec<(Cid, Cid)>>,
    /// `A ⊑ ∃r.B`, keyed by `A` → `(r, B)`.
    pub ex_rhs: FxMap<Cid, Vec<(Rid, Cid)>>,
    /// `∃r.A ⊑ B`, keyed by the filler `A` → `(r, B)`.
    pub ex_lhs: FxMap<Cid, Vec<(Rid, Cid)>>,
    /// `r ⊑ s`.
    pub role_sub: Vec<(Rid, Rid)>,
    /// `r ∘ s ⊑ t`.
    pub chains: Vec<(Rid, Rid, Rid)>,
    /// Reflexive roles.
    pub reflexive: Vec<Rid>,
    /// `ran(r) ⊑ C`.
    pub ranges: Vec<(Rid, Cid)>,
    /// Contexts that stand for exactly one element: individuals and literals.
    pub nominal: FxSet<Cid>,
    /// The nominals that are data values.
    pub literal: FxSet<Cid>,
    /// The datatype concepts.
    pub datatype: Vec<Cid>,
    /// `A ⊑ ∃r.Self`, keyed by `A`.
    pub self_rhs: FxMap<Cid, Vec<Rid>>,
    /// `∃r.Self ⊑ B`, keyed by `r`.
    pub self_lhs: FxMap<Rid, Vec<Cid>>,
    /// Functional (data) properties.
    pub functional: Vec<Rid>,
}

impl Axioms {
    pub fn new() -> Self {
        Axioms {
            n_concepts: 2,
            ..Default::default()
        }
    }

    pub fn n_roles(&self) -> u32 {
        self.n_roles
    }

    pub fn fresh_concept(&mut self) -> Cid {
        let c = self.n_concepts;
        self.n_concepts += 1;
        c
    }

    pub fn fresh_role(&mut self) -> Rid {
        let r = self.n_roles;
        self.n_roles += 1;
        r
    }

    pub fn add_sub(&mut self, a: Cid, b: Cid) {
        if a != b {
            self.sub.entry(a).or_default().push(b);
        }
    }

    /// `A₁ ⊓ A₂ ⊑ B`.
    pub fn add_conj(&mut self, a1: Cid, a2: Cid, b: Cid) {
        self.conj.entry(a1).or_default().push((a2, b));
        if a1 != a2 {
            self.conj.entry(a2).or_default().push((a1, b));
        }
    }

    /// `A₁ ⊓ … ⊓ Aₙ ⊑ B`, binarized with fresh names for the prefixes
    /// (`A₁ ⊓ A₂ ⊑ M₁`, `M₁ ⊓ A₃ ⊑ M₂`, …). A fresh name is only ever a
    /// superclass of its prefix, so it adds no consequence about the input.
    pub fn add_conj_n(&mut self, members: &[Cid], b: Cid) {
        match members {
            [] => self.add_sub(TOP, b),
            [a] => self.add_sub(*a, b),
            [a1, a2] => self.add_conj(*a1, *a2, b),
            [first, rest @ ..] => {
                let mut acc = *first;
                for (i, a) in rest.iter().enumerate() {
                    let out = if i + 1 == rest.len() {
                        b
                    } else {
                        self.fresh_concept()
                    };
                    self.add_conj(acc, *a, out);
                    acc = out;
                }
            }
        }
    }

    /// `A ⊑ ∃r.B`.
    pub fn add_exists_rhs(&mut self, a: Cid, r: Rid, filler: Cid) {
        self.ex_rhs.entry(a).or_default().push((r, filler));
    }

    /// `∃r.A ⊑ B`.
    pub fn add_exists_lhs(&mut self, r: Rid, filler: Cid, b: Cid) {
        self.ex_lhs.entry(filler).or_default().push((r, b));
    }

    /// `r₁ ∘ … ∘ rₙ ⊑ t`, binarized from the left with fresh roles.
    /// A one-element chain is a role inclusion; an empty one is ignored
    /// (the loader reports it).
    pub fn add_chain(&mut self, chain: &[Rid], t: Rid) {
        match chain {
            [] => {}
            [r] => self.role_sub.push((*r, t)),
            [first, rest @ ..] => {
                let mut acc = *first;
                for (i, r) in rest.iter().enumerate() {
                    let out = if i + 1 == rest.len() {
                        t
                    } else {
                        self.fresh_role()
                    };
                    self.chains.push((acc, *r, out));
                    acc = out;
                }
            }
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Context {
    /// `S(X)`: every concept known to subsume `X`, `X` and `⊤` included.
    pub subs: FxSet<Cid>,
    /// `X →r Y`, by role (closed under the role hierarchy).
    pub succ: FxMap<Rid, Vec<Cid>>,
    /// `W →r X`, by role.
    pub pred: FxMap<Rid, Vec<Cid>>,
    /// The nominals in `S(X)`.
    pub noms: Vec<Cid>,
    /// Roles with a self loop on `X` (`X ⊑ ∃r.Self`).
    pub selfs: Vec<Rid>,
}

const NOMINAL: u8 = 1;
const LITERAL: u8 = 2;
const DATATYPE: u8 = 4;

/// The row of a dense table, empty past its end (fresh concepts).
fn row<T>(t: &[Vec<T>], c: Cid) -> &[T] {
    t.get(c as usize).map_or(&[], |v| v.as_slice())
}

#[derive(Clone, Copy)]
enum Event {
    Sub(Cid, Cid),
    Link(Cid, Rid, Cid),
    SelfLink(Cid, Rid),
}

/// The saturation state. Build it from [`Axioms`], add contexts with
/// [`init`](Self::init), then [`run`](Self::run); more contexts and facts can
/// be added and `run` called again (saturation is monotone).
#[derive(Clone)]
pub(crate) struct Saturation {
    ax: Axioms,
    contexts: Vec<Option<Box<Context>>>,
    links: FxSet<(Cid, Rid, Cid)>,
    todo: Vec<Event>,
    /// Reflexive-transitive closure of the role hierarchy, per role.
    sup_star: Vec<Vec<Rid>>,
    /// Told ranges per role.
    ranges: Vec<Vec<Cid>>,
    /// Ranges of a role and all its super-roles.
    ranges_star: Vec<Vec<Cid>>,
    /// `r ∘ s ⊑ t`, by `r` → `(s, t)`.
    chain_first: Vec<Vec<(Rid, Rid)>>,
    /// `r ∘ s ⊑ t`, by `s` → `(r, t)`.
    chain_second: Vec<Vec<(Rid, Rid)>>,
    /// `∃r.A ⊑ B`, by `r` → `(A, B)`.
    ex_lhs_by_role: Vec<Vec<(Cid, Cid)>>,
    /// Reflexive roles (told; their super-roles follow through CR10).
    reflexive: Vec<Rid>,
    /// `B ⊓ ran(r)` fillers made so far, by `(B, r)`.
    range_fillers: FxMap<(Cid, Rid), Cid>,
    /// Self loops made so far.
    self_links: FxSet<(Cid, Rid)>,
    /// For a nominal `a`, the other contexts `X` with `{a} ∈ S(X)`.
    members: Vec<Vec<Cid>>,
    /// The told axioms by left-hand concept, indexed by id: the rules look
    /// these up for every conclusion, so they are vectors, not hash maps.
    sub_t: Vec<Vec<Cid>>,
    conj_t: Vec<Vec<(Cid, Cid)>>,
    ex_rhs_t: Vec<Vec<(Rid, Cid)>>,
    ex_lhs_t: Vec<Vec<(Rid, Cid)>>,
    /// Per concept: `NOMINAL`, `LITERAL`, `DATATYPE`.
    flags: Vec<u8>,
    /// Contexts known to have an element in every model.
    nonempty: Vec<bool>,
    /// Functional roles.
    functional: FxSet<Rid>,
    /// Merged successor contexts of functional roles, by their parts.
    merged: FxMap<Vec<Cid>, Cid>,
    /// The parts of each merged context.
    parts: FxMap<Cid, Vec<Cid>>,
    /// Number of rule conclusions processed (new or not), for the log.
    pub steps: u64,
}

impl Saturation {
    pub fn new(ax: Axioms) -> Self {
        let n_roles = ax.n_roles() as usize;
        // Role hierarchy closure by a search from every role.
        let mut direct: Vec<Vec<Rid>> = vec![Vec::new(); n_roles];
        for &(r, s) in &ax.role_sub {
            direct[r as usize].push(s);
        }
        let mut sup_star: Vec<Vec<Rid>> = Vec::with_capacity(n_roles);
        for r in 0..n_roles {
            let mut seen: FxSet<Rid> = FxSet::default();
            let mut stack = vec![r as Rid];
            let mut out = Vec::new();
            while let Some(x) = stack.pop() {
                if seen.insert(x) {
                    out.push(x);
                    stack.extend(direct[x as usize].iter().copied());
                }
            }
            sup_star.push(out);
        }
        let mut ranges: Vec<Vec<Cid>> = vec![Vec::new(); n_roles];
        for &(r, c) in &ax.ranges {
            if !ranges[r as usize].contains(&c) {
                ranges[r as usize].push(c);
            }
        }
        let ranges_star: Vec<Vec<Cid>> = (0..n_roles)
            .map(|r| {
                let mut out: Vec<Cid> = Vec::new();
                for s in &sup_star[r] {
                    for c in &ranges[*s as usize] {
                        if !out.contains(c) {
                            out.push(*c);
                        }
                    }
                }
                out.sort_unstable();
                out
            })
            .collect();
        let mut chain_first: Vec<Vec<(Rid, Rid)>> = vec![Vec::new(); n_roles];
        let mut chain_second: Vec<Vec<(Rid, Rid)>> = vec![Vec::new(); n_roles];
        for &(r, s, t) in &ax.chains {
            chain_first[r as usize].push((s, t));
            chain_second[s as usize].push((r, t));
        }
        let mut ex_lhs_by_role: Vec<Vec<(Cid, Cid)>> = vec![Vec::new(); n_roles];
        for (&filler, entries) in &ax.ex_lhs {
            for &(r, b) in entries {
                ex_lhs_by_role[r as usize].push((filler, b));
            }
        }
        let reflexive = ax.reflexive.clone();
        let functional = ax.functional.iter().copied().collect();
        let n = ax.n_concepts as usize;
        fn dense<T: Clone>(n: usize, m: &FxMap<Cid, Vec<T>>) -> Vec<Vec<T>> {
            let mut t = vec![Vec::new(); n];
            for (&c, v) in m {
                t[c as usize] = v.clone();
            }
            t
        }
        let mut flags = vec![0u8; n];
        for &c in &ax.nominal {
            flags[c as usize] |= NOMINAL;
        }
        for &c in &ax.literal {
            flags[c as usize] |= LITERAL;
        }
        for &c in &ax.datatype {
            flags[c as usize] |= DATATYPE;
        }
        Saturation {
            sub_t: dense(n, &ax.sub),
            conj_t: dense(n, &ax.conj),
            ex_rhs_t: dense(n, &ax.ex_rhs),
            ex_lhs_t: dense(n, &ax.ex_lhs),
            flags,
            self_links: FxSet::default(),
            members: Vec::new(),
            nonempty: Vec::new(),
            functional,
            merged: FxMap::default(),
            parts: FxMap::default(),
            contexts: Vec::new(),
            links: FxSet::default(),
            todo: Vec::new(),
            sup_star,
            ranges,
            ranges_star,
            chain_first,
            chain_second,
            ex_lhs_by_role,
            reflexive,
            range_fillers: FxMap::default(),
            steps: 0,
            ax,
        }
    }

    pub fn axioms(&self) -> &Axioms {
        &self.ax
    }

    /// `r` and every role it is a sub-role of.
    pub fn sup_star(&self, r: Rid) -> &[Rid] {
        &self.sup_star[r as usize]
    }

    pub fn context(&self, c: Cid) -> Option<&Context> {
        self.contexts.get(c as usize).and_then(|c| c.as_deref())
    }

    /// The contexts made so far, by concept id.
    pub fn contexts(&self) -> impl Iterator<Item = (Cid, &Context)> {
        self.contexts
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.as_deref().map(|c| (i as Cid, c)))
    }

    pub fn subsumes(&self, x: Cid, a: Cid) -> bool {
        self.context(x).is_some_and(|c| c.subs.contains(&a))
    }

    pub fn is_nominal(&self, c: Cid) -> bool {
        self.flag(c, NOMINAL)
    }

    fn flag(&self, c: Cid, f: u8) -> bool {
        self.flags.get(c as usize).is_some_and(|x| x & f != 0)
    }

    /// Give a concept made during saturation its told superclasses.
    fn set_sub(&mut self, c: Cid, supers: Vec<Cid>) {
        let i = c as usize;
        if self.sub_t.len() <= i {
            self.sub_t.resize(i + 1, Vec::new());
        }
        self.sub_t[i] = supers.clone();
        self.ax.sub.insert(c, supers);
    }

    /// Make sure `c` has a context (`c` and `⊤` in `S(c)`, reflexive roles
    /// looped). Idempotent.
    pub fn init(&mut self, c: Cid) {
        let i = c as usize;
        if self.contexts.len() <= i {
            self.contexts.resize_with(i + 1, || None);
            self.nonempty.resize(i + 1, false);
            self.members.resize(i + 1, Vec::new());
        }
        if self.contexts[i].is_some() {
            return;
        }
        self.contexts[i] = Some(Box::default());
        // ⊤ has an element (the domain is not empty), and so has a nominal.
        if c == TOP || self.flag(c, NOMINAL) {
            self.nonempty[i] = true;
        }
        self.todo.push(Event::Sub(c, c));
        self.todo.push(Event::Sub(c, TOP));
        self.todo
            .extend(self.reflexive.iter().map(|&r| Event::SelfLink(c, r)));
    }

    /// A fresh context with an element in every model: a hypothetical
    /// individual. Give it its classes with [`add_sub`](Self::add_sub).
    pub fn add_hypothetical(&mut self) -> Cid {
        let h = self.ax.fresh_concept();
        self.init(h);
        self.nonempty[h as usize] = true;
        h
    }

    pub fn is_nonempty(&self, c: Cid) -> bool {
        self.nonempty.get(c as usize).copied().unwrap_or(false)
    }

    /// Whether the subsumers of class `c` may depend on two contexts
    /// reachable from it being the same nominal once `c` has an instance —
    /// the case NOM-in and NOM-out cannot settle for a class (see the module
    /// documentation). Cheap when no context below a nominal is unreached.
    pub fn needs_hypothesis(&self, c: Cid) -> bool {
        let mut seen: FxSet<Cid> = FxSet::default();
        let mut stack = vec![c];
        // nominal → (contexts in it reached, one of them possibly empty)
        let mut count: FxMap<Cid, (u32, bool)> = FxMap::default();
        while let Some(x) = stack.pop() {
            if !seen.insert(x) {
                continue;
            }
            let Some(cx) = self.context(x) else { continue };
            for &n in &cx.noms {
                let e = count.entry(n).or_insert((0, false));
                e.0 += 1;
                e.1 |= !self.is_nonempty(x);
                if e.0 >= 2 && e.1 {
                    return true;
                }
            }
            for ys in cx.succ.values() {
                stack.extend(ys.iter().copied());
            }
        }
        false
    }

    /// Whether some context below a nominal may be empty — i.e. whether any
    /// class can need [`needs_hypothesis`](Self::needs_hypothesis) at all.
    pub fn has_unreached_nominal_members(&self) -> bool {
        self.contexts()
            .any(|(x, cx)| !self.is_nonempty(x) && cx.noms.iter().any(|&n| n != x))
    }

    /// The subsumers of `c` in `O ∪ {h : c}`: the class's subsumers when
    /// `c` is satisfiable. `⊥` is among them when adding an instance of `c`
    /// makes the ontology inconsistent. Leaves `self` untouched.
    pub fn hypothetical_subsumers(&self, c: Cid) -> FxSet<Cid> {
        let mut copy = self.clone();
        let h = copy.add_hypothetical();
        copy.add_sub(h, c);
        copy.run();
        let clash = copy.ax.nominal.iter().any(|&n| copy.subsumes(n, BOTTOM));
        let mut subs = copy
            .context(h)
            .map(|cx| cx.subs.clone())
            .unwrap_or_default();
        subs.remove(&h);
        subs.insert(c);
        if clash {
            subs.insert(BOTTOM);
        }
        subs
    }

    /// Add `a ∈ S(x)` (made a context if needed); takes effect on [`run`](Self::run).
    pub fn add_sub(&mut self, x: Cid, a: Cid) {
        self.init(x);
        self.todo.push(Event::Sub(x, a));
    }

    /// Add the link `x →r y`; takes effect on [`run`](Self::run).
    pub fn add_link(&mut self, x: Cid, r: Rid, y: Cid) {
        self.init(x);
        self.init(y);
        self.todo.push(Event::Link(x, r, y));
    }

    /// The filler context for `A ⊑ ∃r.B`: `B`, or `B ⊓ ran(r)` when `r` (or a
    /// super-role) has a range.
    fn filler(&mut self, r: Rid, b: Cid) -> Cid {
        let rng = &self.ranges_star[r as usize];
        if rng.is_empty() || rng.iter().all(|c| *c == b || *c == TOP) {
            return b;
        }
        if let Some(&f) = self.range_fillers.get(&(b, r)) {
            return f;
        }
        let f = self.ax.fresh_concept();
        let mut supers = vec![b];
        supers.extend(rng.iter().copied());
        self.set_sub(f, supers);
        self.range_fillers.insert((b, r), f);
        f
    }

    /// Apply the rules until nothing new follows.
    pub fn run(&mut self) {
        while let Some(ev) = self.todo.pop() {
            self.steps += 1;
            match ev {
                Event::Sub(x, a) => self.on_sub(x, a),
                Event::Link(x, r, y) => self.on_link(x, r, y),
                Event::SelfLink(x, r) => self.on_self(x, r),
            }
        }
    }

    fn on_sub(&mut self, x: Cid, a: Cid) {
        if !ctx_mut(&mut self.contexts, x).subs.insert(a) {
            return;
        }
        let todo = &mut self.todo;
        let cx = ctx(&self.contexts, x);
        // CR1
        todo.extend(row(&self.sub_t, a).iter().map(|&b| Event::Sub(x, b)));
        // CR2
        for &(a2, b) in row(&self.conj_t, a) {
            if cx.subs.contains(&a2) {
                todo.push(Event::Sub(x, b));
            }
        }
        // CR4, with X as the filler side: W →r X, ∃r.A ⊑ B.
        if !cx.pred.is_empty() {
            for &(r, b) in row(&self.ex_lhs_t, a) {
                if let Some(ws) = cx.pred.get(&r) {
                    todo.extend(ws.iter().map(|&w| Event::Sub(w, b)));
                }
            }
        }
        // CR5
        if a == BOTTOM {
            for ws in cx.pred.values() {
                todo.extend(ws.iter().map(|&w| Event::Sub(w, BOTTOM)));
            }
        }
        // CR3
        for k in 0..row(&self.ex_rhs_t, a).len() {
            let (r, b) = self.ex_rhs_t[a as usize][k];
            let y = self.filler(r, b);
            self.init(y);
            self.todo.push(Event::Link(x, r, y));
        }
        // A ⊑ ∃r.Self
        if let Some(rs) = self.ax.self_rhs.get(&a) {
            self.todo.extend(rs.iter().map(|&r| Event::SelfLink(x, r)));
        }
        // NOM-in: what holds of nominal x holds of its members.
        self.todo
            .extend(row(&self.members, x).iter().map(|&m| Event::Sub(m, a)));
        if self.flag(a, NOMINAL) {
            self.on_nominal(x, a);
        } else if self.flag(a, DATATYPE) {
            // A value and a datatype that does not contain it.
            let cx = ctx(&self.contexts, x);
            if cx
                .noms
                .iter()
                .any(|&v| self.flag(v, LITERAL) && !row(&self.sub_t, v).contains(&a))
            {
                self.todo.push(Event::Sub(x, BOTTOM));
            }
        }
    }

    /// `{n} ∈ S(x)` is new.
    fn on_nominal(&mut self, x: Cid, n: Cid) {
        self.init(n);
        let lit = self.flag(n, LITERAL);
        // Two different data values, a value and an individual, or a value
        // outside a datatype of the context: no element is all of them.
        let clash = {
            let cx = ctx(&self.contexts, x);
            cx.noms
                .iter()
                .any(|&m| m != n && (lit || self.flag(m, LITERAL)))
                || (lit
                    && self
                        .ax
                        .datatype
                        .iter()
                        .any(|d| cx.subs.contains(d) && !row(&self.sub_t, n).contains(d)))
        };
        if clash {
            self.todo.push(Event::Sub(x, BOTTOM));
        }
        ctx_mut(&mut self.contexts, x).noms.push(n);
        // An edge to the nominal x already is: a self loop.
        {
            let cx = ctx(&self.contexts, x);
            for (&r, ys) in &cx.succ {
                if ys.contains(&n) {
                    self.todo.push(Event::SelfLink(x, r));
                }
            }
        }
        if n == x {
            return;
        }
        // NOM-in: x is n.
        self.members[n as usize].push(x);
        let ncx = ctx(&self.contexts, n);
        self.todo.extend(ncx.subs.iter().map(|&b| Event::Sub(x, b)));
        for (&r, ys) in &ncx.succ {
            self.todo.extend(ys.iter().map(|&y| Event::Link(x, r, y)));
        }
        self.todo
            .extend(ncx.selfs.iter().map(|&r| Event::SelfLink(x, r)));
        // NOM-out: x has an element, so n is in x.
        if self.is_nonempty(x) {
            self.todo.push(Event::Sub(n, x));
        }
    }

    /// `x` has an element in every model; so does everything it links to.
    fn mark_nonempty(&mut self, x: Cid) {
        let mut stack = vec![x];
        while let Some(c) = stack.pop() {
            if self.nonempty[c as usize] {
                continue;
            }
            self.nonempty[c as usize] = true;
            let cx = ctx(&self.contexts, c);
            for &n in &cx.noms {
                if n != c {
                    self.todo.push(Event::Sub(n, c));
                }
            }
            for ys in cx.succ.values() {
                stack.extend(ys.iter().copied().filter(|&y| !self.nonempty[y as usize]));
            }
        }
    }

    /// The context for "one value that is in both `a` and `b`".
    fn merge(&mut self, a: Cid, b: Cid) -> Cid {
        let mut set: Vec<Cid> = Vec::new();
        for c in [a, b] {
            match self.parts.get(&c) {
                Some(p) => set.extend(p.iter().copied()),
                None => set.push(c),
            }
        }
        set.sort_unstable();
        set.dedup();
        if set.len() == 1 {
            return set[0];
        }
        if let Some(&m) = self.merged.get(&set) {
            return m;
        }
        let m = self.ax.fresh_concept();
        self.set_sub(m, set.clone());
        self.parts.insert(m, set.clone());
        self.merged.insert(set, m);
        self.init(m);
        m
    }

    fn on_self(&mut self, x: Cid, r: Rid) {
        for k in 0..self.sup_star[r as usize].len() {
            let s = self.sup_star[r as usize][k];
            if !self.self_links.insert((x, s)) {
                continue;
            }
            ctx_mut(&mut self.contexts, x).selfs.push(s);
            // A loop is also a link, and puts x in the range of s.
            self.todo.push(Event::Link(x, s, x));
            for &c in &self.ranges[s as usize] {
                self.todo.push(Event::Sub(x, c));
            }
            if let Some(bs) = self.ax.self_lhs.get(&s) {
                self.todo.extend(bs.iter().map(|&b| Event::Sub(x, b)));
            }
            for &(t, u) in &self.chain_first[s as usize] {
                if self.self_links.contains(&(x, t)) {
                    self.todo.push(Event::SelfLink(x, u));
                }
            }
            for &(q, u) in &self.chain_second[s as usize] {
                if self.self_links.contains(&(x, q)) {
                    self.todo.push(Event::SelfLink(x, u));
                }
            }
            self.todo
                .extend(row(&self.members, x).iter().map(|&m| Event::SelfLink(m, s)));
        }
    }

    fn on_link(&mut self, x: Cid, r: Rid, y: Cid) {
        // CR10: the link holds for every super-role at once.
        for k in 0..self.sup_star[r as usize].len() {
            let s = self.sup_star[r as usize][k];
            if !self.links.insert((x, s, y)) {
                continue;
            }
            ctx_mut(&mut self.contexts, x)
                .succ
                .entry(s)
                .or_default()
                .push(y);
            ctx_mut(&mut self.contexts, y)
                .pred
                .entry(s)
                .or_default()
                .push(x);
            let todo = &mut self.todo;
            let ycx = ctx(&self.contexts, y);
            // CR4: ∃s.A ⊑ B with A ∈ S(y); walk whichever side is smaller.
            let by_role = &self.ex_lhs_by_role[s as usize];
            if by_role.len() <= ycx.subs.len() {
                for &(a, b) in by_role {
                    if ycx.subs.contains(&a) {
                        todo.push(Event::Sub(x, b));
                    }
                }
            } else {
                for a in &ycx.subs {
                    for &(r2, b) in row(&self.ex_lhs_t, *a) {
                        if r2 == s {
                            todo.push(Event::Sub(x, b));
                        }
                    }
                }
            }
            // CR5
            if ycx.subs.contains(&BOTTOM) {
                todo.push(Event::Sub(x, BOTTOM));
            }
            // RNG: an edge to an individual puts it in the range.
            if self.flags.get(y as usize).is_some_and(|f| f & NOMINAL != 0) {
                todo.extend(self.ranges[s as usize].iter().map(|&c| Event::Sub(y, c)));
            }
            // CR11, x →s y as the first link: s ∘ t ⊑ u, y →t z.
            for &(t, u) in &self.chain_first[s as usize] {
                if let Some(zs) = ycx.succ.get(&t) {
                    todo.extend(zs.iter().map(|&z| Event::Link(x, u, z)));
                }
            }
            // CR11, x →s y as the second link: q ∘ s ⊑ u, w →q x.
            let xcx = ctx(&self.contexts, x);
            for &(q, u) in &self.chain_second[s as usize] {
                if let Some(ws) = xcx.pred.get(&q) {
                    todo.extend(ws.iter().map(|&w| Event::Link(w, u, y)));
                }
            }
            // NOM-in: the members of nominal x share its edges.
            todo.extend(row(&self.members, x).iter().map(|&m| Event::Link(m, s, y)));
            // x ⊑ {y} and x →s y: x is y, the edge is a loop.
            if self.flags.get(y as usize).is_some_and(|f| f & NOMINAL != 0) && xcx.subs.contains(&y)
            {
                todo.push(Event::SelfLink(x, s));
            }
            // Functional: every s-successor of x is one value.
            if self.functional.contains(&s) {
                let others: Vec<Cid> = xcx.succ[&s].iter().copied().filter(|&o| o != y).collect();
                for o in others {
                    let m = self.merge(y, o);
                    if m != y && m != o {
                        self.todo.push(Event::Link(x, s, m));
                    }
                }
            }
            if self.nonempty[x as usize] && !self.nonempty[y as usize] {
                self.mark_nonempty(y);
            }
        }
    }
}

fn ctx(contexts: &[Option<Box<Context>>], c: Cid) -> &Context {
    contexts[c as usize]
        .as_deref()
        .expect("context initialised before use")
}

fn ctx_mut(contexts: &mut [Option<Box<Context>>], c: Cid) -> &mut Context {
    contexts[c as usize]
        .as_deref_mut()
        .expect("context initialised before use")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saturate `ax` with contexts for `0..n` and return the state.
    fn saturate(ax: Axioms, contexts: &[Cid]) -> Saturation {
        let mut s = Saturation::new(ax);
        s.init(TOP);
        for &c in contexts {
            s.init(c);
        }
        s.run();
        s
    }

    #[test]
    fn cr4_through_filler_subsumption_and_roles() {
        // A ⊑ ∃r.B, B ⊑ C, r ⊑ s, ∃s.C ⊑ D  ⊨  A ⊑ D
        let mut ax = Axioms::new();
        let [a, b, c, d] = [0; 4].map(|_| ax.fresh_concept());
        let [r, s] = [0; 2].map(|_| ax.fresh_role());
        ax.add_exists_rhs(a, r, b);
        ax.add_sub(b, c);
        ax.role_sub.push((r, s));
        ax.add_exists_lhs(s, c, d);
        let sat = saturate(ax, &[a, d]);
        assert!(sat.subsumes(a, d));
        assert!(!sat.subsumes(d, a));
    }

    #[test]
    fn bottom_through_existential() {
        let mut ax = Axioms::new();
        let [a, b] = [0; 2].map(|_| ax.fresh_concept());
        let r = ax.fresh_role();
        ax.add_exists_rhs(a, r, b);
        ax.add_sub(b, BOTTOM);
        let sat = saturate(ax, &[a]);
        assert!(sat.subsumes(a, BOTTOM));
    }

    #[test]
    fn tbox_chain_composition() {
        // A ⊑ ∃r.B, B ⊑ ∃s.C, r ∘ s ⊑ t, ∃t.C ⊑ D  ⊨  A ⊑ D
        let mut ax = Axioms::new();
        let [a, b, c, d] = [0; 4].map(|_| ax.fresh_concept());
        let [r, s, t] = [0; 3].map(|_| ax.fresh_role());
        ax.add_exists_rhs(a, r, b);
        ax.add_exists_rhs(b, s, c);
        ax.add_chain(&[r, s], t);
        ax.add_exists_lhs(t, c, d);
        let sat = saturate(ax, &[a]);
        assert!(sat.subsumes(a, d));
    }

    #[test]
    fn range_does_not_leak_into_shared_filler() {
        // A ⊑ ∃r.B, ran(r) = C: the successor is a B ⊓ C, B is not a C.
        // ∃r.(B ⊓ C) ⊑ D, written as ∃r.E ⊑ D, B ⊓ C ⊑ E  ⊨  A ⊑ D.
        let mut ax = Axioms::new();
        let [a, b, c, d, e] = [0; 5].map(|_| ax.fresh_concept());
        let r = ax.fresh_role();
        ax.add_exists_rhs(a, r, b);
        ax.ranges.push((r, c));
        ax.add_conj(b, c, e);
        ax.add_exists_lhs(r, e, d);
        let sat = saturate(ax, &[a, b]);
        assert!(sat.subsumes(a, d));
        assert!(!sat.subsumes(b, c));
    }

    #[test]
    fn nominal_in_and_out() {
        // a : B; X ⊑ {a} ⊨ X ⊑ B. b →r Y, Y ⊑ {a}, Y ⊑ C ⊨ a : C (NOM-out:
        // Y has an element, so it is a). Without b, a is not a C.
        let mut ax = Axioms::new();
        let [a, b, bb, c, x, y] = [0; 6].map(|_| ax.fresh_concept());
        let r = ax.fresh_role();
        ax.nominal.extend([a, b]);
        ax.add_sub(a, bb);
        ax.add_sub(x, a);
        ax.add_sub(y, a);
        ax.add_sub(y, c);
        let sat = saturate(ax.clone(), &[a, x, y]);
        assert!(sat.subsumes(x, bb));
        assert!(!sat.subsumes(a, c), "y may be empty");
        let mut sat = Saturation::new(ax);
        sat.add_link(b, r, y);
        sat.init(x);
        sat.run();
        assert!(sat.subsumes(a, c));
        assert!(sat.subsumes(x, c), "x is a, and a is now a C");
    }

    #[test]
    fn hypothetical_instance_settles_shared_nominals() {
        // C ⊑ ∃r.D, C ⊑ ∃s.E, D ⊑ {a} ⊓ X, E ⊑ {a} ⊓ Y, X ⊓ Y ⊑ Z,
        // ∃r.Z ⊑ G ⊨ C ⊑ G (a is in D and E once C has an instance).
        let mut ax = Axioms::new();
        let [a, c, d, e, x, y, z, g] = [0; 8].map(|_| ax.fresh_concept());
        let [r, s] = [0; 2].map(|_| ax.fresh_role());
        ax.nominal.insert(a);
        ax.add_exists_rhs(c, r, d);
        ax.add_exists_rhs(c, s, e);
        for (k, v) in [(d, a), (d, x), (e, a), (e, y)] {
            ax.add_sub(k, v);
        }
        ax.add_conj(x, y, z);
        ax.add_exists_lhs(r, z, g);
        let sat = saturate(ax, &[a, c]);
        assert!(!sat.subsumes(c, g), "the shared contexts alone miss it");
        assert!(!sat.subsumes(a, x), "nothing makes a an X");
        assert!(sat.has_unreached_nominal_members());
        assert!(sat.needs_hypothesis(c));
        assert!(sat.hypothetical_subsumers(c).contains(&g));
        assert!(!sat.subsumes(a, x), "the hypothesis leaves the state alone");
    }

    #[test]
    fn self_loops_are_not_shared_links() {
        // A ⊑ ∃r.A is no loop; A ⊑ ∃r.Self is, and puts A in ran(r).
        let mut ax = Axioms::new();
        let [a, b, n, rng] = [0; 4].map(|_| ax.fresh_concept());
        let r = ax.fresh_role();
        ax.add_exists_rhs(a, r, a);
        ax.self_lhs.entry(r).or_default().push(n);
        ax.self_rhs.entry(b).or_default().push(r);
        ax.ranges.push((r, rng));
        let sat = saturate(ax, &[a, b]);
        assert!(!sat.subsumes(a, n));
        assert!(sat.subsumes(b, n));
        assert!(sat.subsumes(b, rng));
        assert!(!sat.subsumes(a, rng));
    }

    #[test]
    fn functional_merges_successors() {
        // f functional, X ⊑ ∃f.D₁ ⊓ ∃f.D₂, D₁ ⊓ D₂ ⊑ ⊥ ⊨ X ⊑ ⊥; two distinct
        // values on an individual clash, a value outside a datatype too.
        let mut ax = Axioms::new();
        let [x, d1, d2, i, v1, v2, dt] = [0; 7].map(|_| ax.fresh_concept());
        let f = ax.fresh_role();
        ax.functional.push(f);
        ax.add_exists_rhs(x, f, d1);
        ax.add_exists_rhs(x, f, d2);
        ax.add_conj(d1, d2, BOTTOM);
        ax.nominal.extend([i, v1, v2]);
        ax.literal.extend([v1, v2]);
        ax.datatype.push(dt);
        ax.add_sub(v2, dt);
        let mut sat = Saturation::new(ax.clone());
        sat.init(x);
        sat.add_link(i, f, v2);
        sat.run();
        assert!(sat.subsumes(x, BOTTOM));
        assert!(!sat.subsumes(i, BOTTOM), "one value is fine");
        let mut both = Saturation::new(ax.clone());
        both.add_link(i, f, v1);
        both.add_link(i, f, v2);
        both.run();
        assert!(both.subsumes(i, BOTTOM));
        let mut typed = Saturation::new(ax);
        typed.add_link(i, f, v1);
        typed.add_sub(v1, dt);
        typed.run();
        assert!(typed.subsumes(v1, BOTTOM), "v1 is not in dt");
    }

    #[test]
    fn long_conjunction_is_binarized() {
        let mut ax = Axioms::new();
        let [a, b, c, d, x] = [0; 5].map(|_| ax.fresh_concept());
        ax.add_conj_n(&[a, b, c], d);
        ax.add_sub(x, a);
        ax.add_sub(x, b);
        let sat = saturate(ax.clone(), &[x]);
        assert!(!sat.subsumes(x, d), "two of three operands are not enough");
        ax.add_sub(x, c);
        let sat = saturate(ax, &[x]);
        assert!(sat.subsumes(x, d));
    }
}
