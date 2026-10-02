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
//! | REF  | `r` reflexive                               | `X →r X`, `ran(r) ∈ S(X)` |
//!
//! `B'` in CR3 is `B` itself when no range applies to `r`, else a fresh
//! context `B ⊓ ran(r)` for every range of `r` and its super-roles (the
//! range-restriction transformation of "… Further"). Ranges are never added
//! to a shared filler context: `A ⊑ ∃r.B` with `ran(r) = C` says the
//! successor is a `B ⊓ C`, not that every `B` is a `C`. Transitivity is the
//! chain `r ∘ r ⊑ r`; longer chains are binarized with fresh roles by the
//! loader. CR6 (nominals) and the self restriction are not rules here yet.

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
}

impl Axioms {
    pub fn new() -> Self {
        Axioms {
            n_concepts: 2,
            ..Default::default()
        }
    }

    pub fn n_concepts(&self) -> u32 {
        self.n_concepts
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
}

#[derive(Clone, Copy)]
enum Event {
    Sub(Cid, Cid),
    Link(Cid, Rid, Cid),
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
        Saturation {
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
        self.ax.nominal.contains(&c)
    }

    /// Make sure `c` has a context (`c` and `⊤` in `S(c)`, reflexive roles
    /// looped). Idempotent.
    pub fn init(&mut self, c: Cid) {
        let i = c as usize;
        if self.contexts.len() <= i {
            self.contexts.resize_with(i + 1, || None);
        }
        if self.contexts[i].is_some() {
            return;
        }
        self.contexts[i] = Some(Box::default());
        self.todo.push(Event::Sub(c, c));
        self.todo.push(Event::Sub(c, TOP));
        for k in 0..self.reflexive.len() {
            let r = self.reflexive[k];
            self.todo.push(Event::Link(c, r, c));
            // x r x for every x puts every x in the range of r.
            for s in self.sup_star[r as usize].clone() {
                for &rng in &self.ranges[s as usize] {
                    self.todo.push(Event::Sub(c, rng));
                }
            }
        }
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
        self.ax.sub.insert(f, supers);
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
            }
        }
    }

    fn on_sub(&mut self, x: Cid, a: Cid) {
        if !ctx_mut(&mut self.contexts, x).subs.insert(a) {
            return;
        }
        let todo = &mut self.todo;
        let ctx = ctx(&self.contexts, x);
        // CR1
        if let Some(bs) = self.ax.sub.get(&a) {
            todo.extend(bs.iter().map(|&b| Event::Sub(x, b)));
        }
        // CR2
        if let Some(entries) = self.ax.conj.get(&a) {
            for &(a2, b) in entries {
                if ctx.subs.contains(&a2) {
                    todo.push(Event::Sub(x, b));
                }
            }
        }
        // CR4, with X as the filler side: W →r X, ∃r.A ⊑ B.
        if let Some(entries) = self.ax.ex_lhs.get(&a) {
            for &(r, b) in entries {
                if let Some(ws) = ctx.pred.get(&r) {
                    todo.extend(ws.iter().map(|&w| Event::Sub(w, b)));
                }
            }
        }
        // CR5
        if a == BOTTOM {
            for ws in ctx.pred.values() {
                todo.extend(ws.iter().map(|&w| Event::Sub(w, BOTTOM)));
            }
        }
        // CR3
        if let Some(entries) = self.ax.ex_rhs.get(&a).cloned() {
            for (r, b) in entries {
                let y = self.filler(r, b);
                self.init(y);
                self.todo.push(Event::Link(x, r, y));
            }
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
                    if let Some(entries) = self.ax.ex_lhs.get(a) {
                        for &(r2, b) in entries {
                            if r2 == s {
                                todo.push(Event::Sub(x, b));
                            }
                        }
                    }
                }
            }
            // CR5
            if ycx.subs.contains(&BOTTOM) {
                todo.push(Event::Sub(x, BOTTOM));
            }
            // RNG: an edge to an individual puts it in the range.
            if self.ax.nominal.contains(&y) {
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
    fn long_conjunction_is_binarized() {
        let mut ax = Axioms::new();
        let [a, b, c, d, x] = [0; 5].map(|_| ax.fresh_concept());
        ax.add_conj_n(&[a, b, c], d);
        ax.add_sub(x, a);
        ax.add_sub(x, b);
        let sat = saturate(ax.clone(), &[x]);
        assert!(!sat.subsumes(x, d), "two of three operands are not enough");
        let mut ax = ax;
        ax.add_sub(x, c);
        let sat = saturate(ax, &[x]);
        assert!(sat.subsumes(x, d));
    }
}
