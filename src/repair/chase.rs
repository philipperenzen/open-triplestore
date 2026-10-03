//! The restricted chase over the sandbox (`docs/notes/repair-layer-design.md`
//! §4.1–§4.7).
//!
//! Bodies run as `SELECT` over the sandbox through the store's own evaluator
//! (`query_options()`, so GeoSPARQL, `ADJUST` and the `sh:SPARQLFunction`s
//! of the copied shape graphs are available); heads are applied in Rust. A
//! trigger is active only when the rule's guard says its head is not already
//! satisfied (§4.2), and a head quad is added only when the sandbox does not
//! already hold it, so a second run over an applied proposal is a no-op.
//!
//! Strata run in order; within one, every rule is evaluated against the state
//! at the start of the round, its triggers in sorted order, and the round's
//! additions and deletions are applied together at its end. Rounds repeat
//! until one changes nothing or the budget runs out. A null merged into
//! another term (an EGD) is substituted everywhere at the end of its round,
//! and when a pass over the strata substituted anything, the strata run
//! again from the first (§4.4: merged pairs re-feed the strata). Policy
//! deletions run last. Determinism comes from a total rule order, sorted
//! solutions, content-derived nulls, a total representative order and
//! insertion-ordered bookkeeping (§4.7).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Instant;

use oxigraph::model::{
    GraphName, Literal, NamedNode, NamedOrBlankNode, NamedOrBlankNodeRef, Quad, Term,
};
use oxigraph::sparql::QueryResults;
use serde::Serialize;
use sha2::{Digest, Sha256};
use spargebra::algebra::GraphPattern;
use spargebra::term::{GroundTerm, NamedNodePattern, TermPattern, TriplePattern, Variable};

use super::rules::{ground, Kind, MergeMode, Native, Placement, Policy, PreparedRule, OWL_SAME_AS};
use crate::store::TripleStore;

/// The well-known path nulls are minted under (RFC 6694 / RDF 1.1 §3.5).
pub const GENID: &str = "/.well-known/genid/";

/// The budgets of one run (§4.5).
#[derive(Debug, Clone)]
pub struct Budget {
    pub rounds: u32,
    pub nulls: usize,
    pub ops: usize,
    pub deadline: Instant,
}

/// What the chase reads and where it may write.
pub struct ChaseInput<'a> {
    pub sandbox: &'a TripleStore,
    pub rules: &'a [PreparedRule],
    /// The strata, the policy deletions and the report rules.
    pub strata: &'a super::stratify::Strata,
    /// The graphs rule bodies read (the premises, §5.2).
    pub premises: &'a [String],
    /// The graphs a head may land in (§4.6).
    pub targets: &'a BTreeSet<String>,
    /// The base nulls are minted under (`{base}/.well-known/genid/…`).
    pub null_base: &'a str,
    /// IRI prefixes that mark a term as a null (this run's base and the
    /// Skolem default).
    pub null_prefixes: &'a [String],
    /// `scope.focus`: the focus nodes rules binding `?this` are limited to.
    pub focus: Option<&'a [Term]>,
    pub budget: Budget,
    /// `owl:sameAs` pairs already asserted, when the dataset's identity
    /// policy propagates them: they seed the equality structure.
    pub same_as_seeds: &'a [(Term, Term)],
    /// Evaluate rounds after the first from the previous round's additions
    /// (semi-naive); `false` re-evaluates every body in full.
    pub semi_naive: bool,
}

/// Why a quad is in the proposal.
#[derive(Debug, Clone, Serialize)]
pub struct Derivation {
    #[serde(skip)]
    pub rule: usize,
    pub stratum: usize,
    pub round: u32,
    /// Hex digest of the rule and its sorted bindings.
    pub trigger: String,
    /// The bindings of the trigger (graph and blank-node helper variables
    /// left out), as N-Triples terms.
    pub bindings: BTreeMap<String, String>,
    /// The quads the body matched, as N-Quads statements.
    pub premises: Vec<String>,
    pub kind: DerivationKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DerivationKind {
    Added,
    Deleted,
    /// Rewritten because a null (or, under `Rewrite`, a live IRI) was merged.
    Substituted {
        from: String,
        to: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Merge {
    pub loser: String,
    pub winner: String,
    pub rule_iri: String,
    pub mode: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Conflict {
    #[serde(skip)]
    pub rule: usize,
    pub rule_iri: String,
    pub kind: &'static str,
    pub detail: String,
}

/// A trigger that fired but produced nothing (unplaceable, unexpressible).
#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub rule_iri: String,
    pub trigger: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RuleStats {
    pub rule: String,
    pub stratum: Option<usize>,
    pub triggers: usize,
    pub adds: usize,
    pub deletes: usize,
    pub merges: usize,
}

/// What a run produced.
#[derive(Debug, Default)]
pub struct ChaseOutcome {
    /// Quads the proposal adds, in derivation order, with their reasons.
    pub added: Vec<(Quad, Derivation)>,
    /// Seeded quads the proposal deletes.
    pub deleted: Vec<(Quad, Derivation)>,
    pub merges: Vec<Merge>,
    pub conflicts: Vec<Conflict>,
    pub unplaceable: Vec<Skipped>,
    pub unexpressible: Vec<Skipped>,
    pub per_rule: Vec<RuleStats>,
    /// Distinct triggers of each `ots:Report` rule, by rule index.
    pub report_triggers: Vec<(usize, usize)>,
    pub rounds: u32,
    pub nulls: usize,
    pub exhausted: Option<&'static str>,
    /// Body evaluations run (diagnostics: semi-naive runs fewer rows).
    pub evaluations: usize,
    pub rows_read: usize,
}

/// A union-find over terms with the representative order of §4.4.
#[derive(Debug, Default)]
pub struct UnionFind {
    parent: HashMap<Term, Term>,
    members: HashMap<Term, Vec<Term>>,
}

impl UnionFind {
    pub fn find(&mut self, t: &Term) -> Term {
        let mut root = t.clone();
        while let Some(p) = self.parent.get(&root) {
            if p == &root {
                break;
            }
            root = p.clone();
        }
        // Path compression.
        let mut cur = t.clone();
        while let Some(p) = self.parent.get(&cur).cloned() {
            if p == root {
                break;
            }
            self.parent.insert(cur.clone(), root.clone());
            cur = p;
        }
        root
    }

    /// Every term known to be equal to `root` (itself included).
    pub fn class(&self, root: &Term) -> Vec<Term> {
        self.members
            .get(root)
            .cloned()
            .unwrap_or_else(|| vec![root.clone()])
    }

    /// Make `win` the representative of `lose`'s class.
    pub fn union(&mut self, lose: &Term, win: &Term) {
        let mut moved = self
            .members
            .remove(lose)
            .unwrap_or_else(|| vec![lose.clone()]);
        self.parent.insert(lose.clone(), win.clone());
        self.parent
            .entry(win.clone())
            .or_insert_with(|| win.clone());
        let entry = self
            .members
            .entry(win.clone())
            .or_insert_with(|| vec![win.clone()]);
        entry.append(&mut moved);
    }
}

fn nt(t: &Term) -> String {
    t.to_string()
}

fn sha_hex(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update(b"\n");
        }
        h.update(p.as_bytes());
    }
    hex::encode(h.finalize())
}

/// The IRI of a labelled null: content-derived from the rule, its frontier
/// bindings and the null's name (§4.3), so the same state and rules mint the
/// same IRIs.
pub fn mint_null(base: &str, rule_iri: &str, frontier: &str, null_name: &str) -> String {
    let digest = sha_hex(&[rule_iri, frontier, null_name]);
    format!("{}{GENID}{}", base.trim_end_matches('/'), &digest[..32])
}

/// A solution, aligned with the query's projected variables.
type Row = Vec<Option<Term>>;

struct Solutions {
    vars: Vec<Variable>,
    index: HashMap<String, usize>,
    rows: Vec<Row>,
}

impl Solutions {
    fn get<'r>(&self, row: &'r Row, var: &str) -> Option<&'r Term> {
        self.index.get(var).and_then(|i| row[*i].as_ref())
    }
}

/// Sort key of a row: its terms in N-Triples form.
fn row_key(row: &Row) -> Vec<String> {
    row.iter()
        .map(|t| t.as_ref().map(nt).unwrap_or_default())
        .collect()
}

fn is_helper_var(name: &str) -> bool {
    name.starts_with("__")
}

/// The run's mutable state.
struct Run<'a> {
    input: &'a ChaseInput<'a>,
    default_graphs: Vec<GraphName>,
    named_graphs: Vec<NamedOrBlankNode>,
    added: Vec<(Quad, Derivation)>,
    alive: Vec<bool>,
    added_idx: HashMap<Quad, usize>,
    deleted: Vec<(Quad, Derivation)>,
    deleted_idx: HashMap<Quad, usize>,
    nulls: HashSet<String>,
    uf: UnionFind,
    out: ChaseOutcome,
    /// This round's changes, applied at its end.
    new: Vec<Quad>,
    gone: Vec<Quad>,
    substitutions: Vec<(Term, Term)>,
    /// Frontier keys of null-minting triggers seen this round, per rule.
    seen_frontier: HashSet<(usize, String)>,
    /// What the previous round added, for semi-naive evaluation.
    delta: Vec<Quad>,
    delta_complete: bool,
    stop: Option<&'static str>,
    stratum_of: HashMap<usize, usize>,
    /// Conflicts already recorded (rule, kind, the pair in order).
    conflicts_seen: HashSet<(usize, &'static str, String)>,
}

/// Run the chase.
pub fn chase(input: &ChaseInput<'_>) -> Result<ChaseOutcome, String> {
    let mut run = Run::new(input)?;
    run.seed_same_as();
    run.run_strata()?;
    run.count_report_rules()?;
    Ok(run.finish())
}

impl<'a> Run<'a> {
    fn new(input: &'a ChaseInput<'a>) -> Result<Self, String> {
        let mut default_graphs = Vec::new();
        let mut named_graphs = Vec::new();
        for g in input.premises {
            let n = NamedNode::new(g).map_err(|e| format!("premise graph <{g}>: {e}"))?;
            default_graphs.push(GraphName::NamedNode(n.clone()));
            named_graphs.push(NamedOrBlankNode::NamedNode(n));
        }
        let mut stratum_of = HashMap::new();
        for r in 0..input.rules.len() {
            if let Some(s) = input.strata.stratum_of(r) {
                stratum_of.insert(r, s);
            }
        }
        let per_rule = input
            .rules
            .iter()
            .enumerate()
            .map(|(i, r)| RuleStats {
                rule: r.spec.iri.clone(),
                stratum: stratum_of.get(&i).copied(),
                ..RuleStats::default()
            })
            .collect();
        Ok(Self {
            input,
            default_graphs,
            named_graphs,
            added: Vec::new(),
            alive: Vec::new(),
            added_idx: HashMap::new(),
            deleted: Vec::new(),
            deleted_idx: HashMap::new(),
            nulls: HashSet::new(),
            uf: UnionFind::default(),
            out: ChaseOutcome {
                per_rule,
                ..ChaseOutcome::default()
            },
            new: Vec::new(),
            gone: Vec::new(),
            substitutions: Vec::new(),
            seen_frontier: HashSet::new(),
            delta: Vec::new(),
            delta_complete: false,
            stop: None,
            stratum_of,
            conflicts_seen: HashSet::new(),
        })
    }

    fn is_null(&self, t: &Term) -> bool {
        matches!(t, Term::NamedNode(n)
            if self.input.null_prefixes.iter().any(|p| n.as_str().starts_with(p.as_str())))
    }

    /// Representative order (§4.4): a live IRI beats a null; then the
    /// smallest N-Triples form.
    fn rank(&self, t: &Term) -> (bool, String) {
        (self.is_null(t), nt(t))
    }

    fn seed_same_as(&mut self) {
        let mut pairs: Vec<(Term, Term)> = self.input.same_as_seeds.to_vec();
        pairs.sort_by_key(|(a, b)| (nt(a), nt(b)));
        for (a, b) in pairs {
            let (ra, rb) = (self.uf.find(&a), self.uf.find(&b));
            if ra == rb {
                continue;
            }
            let (win, lose) = if self.rank(&ra) <= self.rank(&rb) {
                (ra, rb)
            } else {
                (rb, ra)
            };
            self.uf.union(&lose, &win);
        }
    }

    fn deadline_passed(&self) -> bool {
        Instant::now() >= self.input.budget.deadline
    }

    fn ops(&self) -> usize {
        self.alive.iter().filter(|a| **a).count() + self.deleted.len()
    }

    fn check_budget(&mut self) {
        if self.stop.is_some() {
            return;
        }
        if self.deadline_passed() {
            self.stop = Some("time");
        } else if self.nulls.len() > self.input.budget.nulls {
            self.stop = Some("nulls");
        } else if self.ops() > self.input.budget.ops {
            self.stop = Some("ops");
        }
    }

    // ── evaluation ──────────────────────────────────────────────────────────

    fn evaluate(
        &mut self,
        rule: &PreparedRule,
        extra: Option<GraphPattern>,
    ) -> Result<Solutions, String> {
        let (query, vars) = rule.select(extra);
        let mut prepared = self.input.sandbox.query_options().for_query(query);
        {
            let ds = prepared.dataset_mut();
            ds.set_default_graph(self.default_graphs.clone());
            ds.set_available_named_graphs(self.named_graphs.clone());
        }
        let results = prepared
            .on_store(self.input.sandbox.store())
            .execute()
            .map_err(|e| format!("rule <{}> could not be evaluated: {e}", rule.spec.iri))?;
        let QueryResults::Solutions(solutions) = results else {
            return Err(format!(
                "rule <{}>: the body is not a SELECT",
                rule.spec.iri
            ));
        };
        let mut rows: Vec<Row> = Vec::new();
        for s in solutions {
            let s =
                s.map_err(|e| format!("rule <{}> could not be evaluated: {e}", rule.spec.iri))?;
            rows.push(vars.iter().map(|v| s.get(v.as_str()).cloned()).collect());
        }
        self.out.evaluations += 1;
        self.out.rows_read += rows.len();
        let index = vars
            .iter()
            .enumerate()
            .map(|(i, v)| (v.as_str().to_string(), i))
            .collect();
        Ok(Solutions { vars, index, rows })
    }

    /// `VALUES ?this { focus }` when the run is focus-scoped.
    fn focus_values(&self) -> Option<GraphPattern> {
        self.input
            .focus
            .map(|f| super::rules::values(&Variable::new_unchecked("this"), f))
    }

    /// The triggers of `rule` this round: in full, or from the previous
    /// round's additions (one variant per atom, each binding that atom to a
    /// new quad) when semi-naive evaluation applies.
    fn triggers(&mut self, rule: &PreparedRule, round: u32) -> Result<Solutions, String> {
        if self.input.focus.is_some() && !rule.binds_this {
            // A focus-scoped run only fires rules with a focus.
            return Ok(Solutions {
                vars: Vec::new(),
                index: HashMap::new(),
                rows: Vec::new(),
            });
        }
        let focus = self.focus_values();
        let semi = self.input.semi_naive
            && round > 1
            && self.delta_complete
            && !rule.atoms.is_empty()
            && rule.spec.native.is_none()
            && rule.spec.conditions.is_empty();
        let mut sols = if semi {
            let mut merged: Option<Solutions> = None;
            let mut seen: HashSet<Vec<String>> = HashSet::new();
            for k in 0..rule.atoms.len() {
                let Some(delta) = delta_values(&rule.atoms[k], &self.delta) else {
                    continue;
                };
                let extra = match &focus {
                    Some(f) => GraphPattern::Join {
                        left: Box::new(f.clone()),
                        right: Box::new(delta),
                    },
                    None => delta,
                };
                let s = self.evaluate(rule, Some(extra))?;
                match &mut merged {
                    None => {
                        for r in &s.rows {
                            seen.insert(row_key(r));
                        }
                        merged = Some(s);
                    }
                    Some(m) => {
                        // Every variant projects the same variables (the
                        // VALUES only binds body variables), in name order.
                        for r in s.rows {
                            if seen.insert(row_key(&r)) {
                                m.rows.push(r);
                            }
                        }
                    }
                }
            }
            merged.unwrap_or(Solutions {
                vars: Vec::new(),
                index: HashMap::new(),
                rows: Vec::new(),
            })
        } else {
            self.evaluate(rule, focus)?
        };
        let mut keyed: Vec<(Vec<String>, Row)> =
            sols.rows.drain(..).map(|r| (row_key(&r), r)).collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        keyed.dedup_by(|a, b| a.0 == b.0);
        sols.rows = keyed.into_iter().map(|(_, r)| r).collect();
        Ok(sols)
    }

    // ── the strata ──────────────────────────────────────────────────────────

    fn run_strata(&mut self) -> Result<(), String> {
        let input = self.input;
        let main = &input.strata.main;
        // Passes over the main strata: a pass that substituted a null
        // re-feeds the strata (§4.4).
        let mut passes = 0u32;
        loop {
            passes += 1;
            let mut substituted = false;
            for (s, stratum) in main.iter().enumerate() {
                substituted |= self.run_stratum(s, stratum)?;
                if self.stop.is_some() {
                    break;
                }
            }
            if self.stop.is_some() || !substituted || passes >= input.budget.rounds {
                break;
            }
        }
        // Policy deletions last, after every addition (§6).
        if !input.strata.last.is_empty() && self.stop.is_none() {
            self.run_stratum(main.len(), &input.strata.last)?;
        }
        if let Some(kind) = self.stop {
            self.out.exhausted = Some(kind);
        }
        Ok(())
    }

    /// Run one stratum to its fixed point. `true` when a null was
    /// substituted.
    fn run_stratum(&mut self, s: usize, stratum: &[usize]) -> Result<bool, String> {
        let input = self.input;
        let mut order: Vec<usize> = stratum.to_vec();
        order.sort_by(|a, b| {
            let (ra, rb) = (&input.rules[*a].spec, &input.rules[*b].spec);
            ra.priority
                .total_cmp(&rb.priority)
                .then_with(|| ra.iri.cmp(&rb.iri))
        });
        let mut substituted = false;
        self.delta_complete = false;
        for round in 1..=input.budget.rounds {
            self.out.rounds += 1;
            self.seen_frontier.clear();
            for &r in &order {
                if self.stop.is_some() {
                    break;
                }
                let rule = &input.rules[r];
                let sols = self.triggers(rule, round)?;
                for row in &sols.rows {
                    self.out.per_rule[r].triggers += 1;
                    self.fire(r, s, round, &sols, row)?;
                    self.check_budget();
                    if self.stop.is_some() {
                        break;
                    }
                }
                if self.deadline_passed() {
                    self.stop.get_or_insert("time");
                }
            }
            let changed =
                !self.new.is_empty() || !self.gone.is_empty() || !self.substitutions.is_empty();
            substituted |= !self.substitutions.is_empty();
            let delta_ok = self.gone.is_empty() && self.substitutions.is_empty();
            self.apply_round()?;
            self.delta_complete = delta_ok;
            if !changed || self.stop.is_some() {
                break;
            }
            if round == input.budget.rounds {
                self.stop = Some("rounds");
            }
        }
        Ok(substituted)
    }

    /// Apply the round's additions and deletions to the sandbox, then its
    /// substitutions.
    fn apply_round(&mut self) -> Result<(), String> {
        let store = self.input.sandbox.store();
        let new = std::mem::take(&mut self.new);
        let gone = std::mem::take(&mut self.gone);
        {
            let mut tx = store.start_transaction().map_err(|e| e.to_string())?;
            for q in &gone {
                tx.remove(q);
            }
            for q in &new {
                tx.insert(q);
            }
            tx.commit().map_err(|e| e.to_string())?;
        }
        let mut delta = new;
        // Substitutions: every quad mentioning a merged null is rewritten.
        let subs = std::mem::take(&mut self.substitutions);
        if !subs.is_empty() {
            let mut removed = Vec::new();
            let mut inserted = Vec::new();
            for (null, win) in &subs {
                let win = self.uf.find(win);
                for q in quads_mentioning(store, null) {
                    let image = substitute(&q, null, &win);
                    let Some(image) = image else {
                        // The image would be a literal subject: nothing to write.
                        continue;
                    };
                    let from = nt(null);
                    let to = nt(&win);
                    match self.added_idx.get(&q).copied() {
                        Some(i) if self.alive[i] => {
                            self.alive[i] = false;
                            self.added_idx.remove(&q);
                            let mut d = self.added[i].1.clone();
                            d.kind = DerivationKind::Substituted {
                                from: from.clone(),
                                to: to.clone(),
                            };
                            self.push_added(image.clone(), d);
                        }
                        _ => {
                            if let Some(g) = graph_iri(&q) {
                                if !self.input.targets.contains(&g) {
                                    self.out.unplaceable.push(Skipped {
                                        rule_iri: String::new(),
                                        trigger: String::new(),
                                        reason: format!(
                                            "substituting {from} by {to} would change <{g}>, which is not a writable graph"
                                        ),
                                    });
                                    continue;
                                }
                            }
                            let d = Derivation {
                                rule: usize::MAX,
                                stratum: 0,
                                round: 0,
                                trigger: String::new(),
                                bindings: BTreeMap::new(),
                                premises: Vec::new(),
                                kind: DerivationKind::Substituted {
                                    from: from.clone(),
                                    to: to.clone(),
                                },
                                explanation: Some(format!("{from} was merged into {to}")),
                            };
                            self.push_deleted(q.clone(), d.clone());
                            if !store.contains(&image).unwrap_or(false) {
                                self.push_added(image.clone(), d);
                            }
                        }
                    }
                    removed.push(q);
                    inserted.push(image);
                }
            }
            let mut tx = store.start_transaction().map_err(|e| e.to_string())?;
            for q in &removed {
                tx.remove(q);
            }
            for q in &inserted {
                tx.insert(q);
            }
            tx.commit().map_err(|e| e.to_string())?;
            delta.extend(inserted);
            // The bookkeeping above queued what was just written; it is in
            // the sandbox already.
            self.new.clear();
            self.gone.clear();
        }
        self.delta = delta;
        Ok(())
    }

    /// Record a conflict once per rule, kind and detail.
    fn conflict(&mut self, c: Conflict) {
        if self
            .conflicts_seen
            .insert((c.rule, c.kind, c.detail.clone()))
        {
            self.out.conflicts.push(c);
        }
    }

    fn push_added(&mut self, q: Quad, d: Derivation) {
        if let Some(&i) = self.added_idx.get(&q) {
            if self.alive[i] {
                return;
            }
        }
        // A seeded quad deleted earlier and derived again: the two cancel.
        if let Some(i) = self.deleted_idx.remove(&q) {
            self.deleted.remove(i);
            self.deleted_idx = self
                .deleted
                .iter()
                .enumerate()
                .map(|(i, (q, _))| (q.clone(), i))
                .collect();
            self.new.push(q);
            return;
        }
        self.added_idx.insert(q.clone(), self.added.len());
        self.added.push((q.clone(), d));
        self.alive.push(true);
        self.new.push(q);
    }

    fn push_deleted(&mut self, q: Quad, d: Derivation) {
        if self.deleted_idx.contains_key(&q) {
            return;
        }
        self.deleted_idx.insert(q.clone(), self.deleted.len());
        self.deleted.push((q.clone(), d));
        self.gone.push(q);
    }

    // ── firing ──────────────────────────────────────────────────────────────

    fn bindings_of(&self, sols: &Solutions, row: &Row) -> BTreeMap<String, String> {
        sols.vars
            .iter()
            .zip(row.iter())
            .filter(|(v, _)| !is_helper_var(v.as_str()))
            .filter_map(|(v, t)| t.as_ref().map(|t| (v.as_str().to_string(), nt(t))))
            .collect()
    }

    fn trigger_id(&self, rule: &PreparedRule, bindings: &BTreeMap<String, String>) -> String {
        let flat: Vec<String> = bindings.iter().map(|(k, v)| format!("{k}={v}")).collect();
        sha_hex(&[&rule.spec.iri, &flat.join("\n")])[..16].to_string()
    }

    fn premises_of(&self, rule: &PreparedRule, sols: &Solutions, row: &Row) -> Vec<String> {
        rule.atoms
            .iter()
            .filter_map(|a| {
                let s = inst(&a.pattern.subject, sols, row, &HashMap::new())?;
                let p = inst_pred(&a.pattern.predicate, sols, row)?;
                let o = inst(&a.pattern.object, sols, row, &HashMap::new())?;
                let g = match &a.graph {
                    NamedNodePattern::NamedNode(n) => Term::NamedNode(n.clone()),
                    NamedNodePattern::Variable(v) => sols.get(row, v.as_str())?.clone(),
                };
                Some(format!("{s} {p} {o} {g}"))
            })
            .collect()
    }

    fn explain(
        &self,
        rule: &PreparedRule,
        sols: &Solutions,
        row: &Row,
        extra: &HashMap<String, Term>,
    ) -> Option<String> {
        let tpl = rule.spec.message.as_ref()?;
        let mut out = tpl.clone();
        for (v, t) in sols.vars.iter().zip(row.iter()) {
            if let Some(t) = t {
                let shown = crate::shacl::constraints::display_term(t);
                out = out
                    .replace(&format!("{{?{}}}", v.as_str()), &shown)
                    .replace(&format!("{{${}}}", v.as_str()), &shown);
            }
        }
        for (v, t) in extra {
            let shown = crate::shacl::constraints::display_term(t);
            out = out
                .replace(&format!("{{?{v}}}"), &shown)
                .replace(&format!("{{${v}}}"), &shown);
        }
        Some(out)
    }

    /// The single writable graph, used when a placement is undetermined.
    fn only_target(&self) -> Option<String> {
        (self.input.targets.len() == 1).then(|| self.input.targets.iter().next().cloned())?
    }

    fn resolve_placement(
        &self,
        placement: &Placement,
        resolved: &[Option<String>],
        rule: &PreparedRule,
        sols: &Solutions,
        row: &Row,
    ) -> Option<String> {
        match placement {
            Placement::Target(g) => Some(g.clone()),
            Placement::Atom(k) => match &rule.atoms[*k].graph {
                NamedNodePattern::NamedNode(n) => Some(n.as_str().to_string()),
                NamedNodePattern::Variable(v) => match sols.get(row, v.as_str()) {
                    Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
                    _ => None,
                },
            },
            Placement::Head(j) => resolved.get(*j).cloned().flatten(),
            Placement::Unknown => self.only_target(),
        }
    }

    fn fire(
        &mut self,
        r: usize,
        stratum: usize,
        round: u32,
        sols: &Solutions,
        row: &Row,
    ) -> Result<(), String> {
        let input = self.input;
        let rule = &input.rules[r];
        if rule.spec.policy == Policy::Report {
            return Ok(());
        }
        // sh:condition of an imported SHACL-AF rule: the focus must conform.
        if !rule.spec.conditions.is_empty() {
            let Some(focus) = sols.get(row, "this").cloned() else {
                return Ok(());
            };
            if !self.conforms_to_conditions(rule, &focus) {
                return Ok(());
            }
        }
        match &rule.kind {
            Kind::Egd { a, b } => self.fire_egd(r, stratum, round, sols, row, a.clone(), b.clone()),
            Kind::Tgd => match rule.spec.native.clone() {
                Some(native) => self.fire_native(r, stratum, round, sols, row, &native),
                None => self.fire_tgd(r, stratum, round, sols, row),
            },
        }
    }

    fn conforms_to_conditions(&self, rule: &PreparedRule, focus: &Term) -> bool {
        let graphs: Vec<String> = self.input.premises.to_vec();
        let mut view = crate::shacl::view::DataView::new(
            self.input.sandbox,
            &graphs,
            self.input.sandbox.query_options(),
        );
        view.prepare(&rule.spec.conditions);
        rule.spec.conditions.iter().all(|c| {
            crate::shacl::constraints::validate_inline_shape(
                &view,
                &rule.spec.conditions,
                focus,
                c,
                &crate::shacl::report::Severity::Violation,
            )
            .is_empty()
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn derivation(
        &self,
        r: usize,
        stratum: usize,
        round: u32,
        sols: &Solutions,
        row: &Row,
        kind: DerivationKind,
        extra: &HashMap<String, Term>,
    ) -> Derivation {
        let rule = &self.input.rules[r];
        let bindings = self.bindings_of(sols, row);
        Derivation {
            rule: r,
            stratum,
            round,
            trigger: self.trigger_id(rule, &bindings),
            premises: self.premises_of(rule, sols, row),
            explanation: self.explain(rule, sols, row, extra),
            bindings,
            kind,
        }
    }

    fn skip(&mut self, r: usize, sols: &Solutions, row: &Row, unplaceable: bool, reason: String) {
        let rule = &self.input.rules[r];
        let bindings = self.bindings_of(sols, row);
        let s = Skipped {
            rule_iri: rule.spec.iri.clone(),
            trigger: self.trigger_id(rule, &bindings),
            reason,
        };
        if unplaceable {
            self.out.unplaceable.push(s);
        } else {
            self.out.unexpressible.push(s);
        }
    }

    fn fire_tgd(
        &mut self,
        r: usize,
        stratum: usize,
        round: u32,
        sols: &Solutions,
        row: &Row,
    ) -> Result<(), String> {
        let input = self.input;
        let rule = &input.rules[r];
        // Nulls: content-derived from the frontier (§4.3); one trigger per
        // frontier per round.
        let mut minted: HashMap<String, Term> = HashMap::new();
        if !rule.nulls.is_empty() {
            let mut frontier: Vec<String> = Vec::new();
            for v in &rule.frontier {
                let t = sols.get(row, v.as_str()).map(nt).unwrap_or_default();
                frontier.push(format!("{}={t}", v.as_str()));
            }
            let key = frontier.join("\n");
            if !self.seen_frontier.insert((r, key.clone())) {
                return Ok(());
            }
            for n in &rule.nulls {
                let scope = rule.spec.null_key.as_deref().unwrap_or(&rule.spec.iri);
                let iri = mint_null(input.null_base, scope, &key, n.as_str());
                minted.insert(
                    n.as_str().to_string(),
                    Term::NamedNode(NamedNode::new_unchecked(iri)),
                );
            }
        }
        // Instantiate and place every head and retract triple; a trigger is
        // all or nothing.
        let mut heads: Vec<Quad> = Vec::new();
        let mut resolved: Vec<Option<String>> = Vec::new();
        for (j, tp) in rule.head.iter().enumerate() {
            let graph = self.resolve_placement(&rule.head_placement[j], &resolved, rule, sols, row);
            resolved.push(graph.clone());
            match self.instantiate(tp, sols, row, &minted) {
                Ok(Some((s, p, o))) => {
                    let Some(g) = graph.filter(|g| self.input.targets.contains(g)) else {
                        self.skip(r, sols, row, true, "the head's graph is undetermined or not a writable graph of the dataset".into());
                        return Ok(());
                    };
                    heads.push(Quad::new(
                        s,
                        p,
                        o,
                        GraphName::NamedNode(NamedNode::new_unchecked(g)),
                    ));
                }
                Ok(None) => return Ok(()),
                Err(reason) => {
                    self.skip(r, sols, row, false, reason);
                    return Ok(());
                }
            }
        }
        let mut retracts: Vec<Quad> = Vec::new();
        for (j, tp) in rule.retract.iter().enumerate() {
            let placement = &rule.retract_placement[j];
            let (s, p, o) = match self.instantiate(tp, sols, row, &HashMap::new()) {
                Ok(Some(t)) => t,
                Ok(None) => return Ok(()),
                Err(reason) => {
                    self.skip(r, sols, row, false, reason);
                    return Ok(());
                }
            };
            let graphs: Vec<String> = match placement {
                Placement::Unknown => self
                    .input
                    .targets
                    .iter()
                    .filter(|g| {
                        let q = Quad::new(
                            s.clone(),
                            p.clone(),
                            o.clone(),
                            GraphName::NamedNode(NamedNode::new_unchecked(g.as_str())),
                        );
                        self.input.sandbox.store().contains(&q).unwrap_or(false)
                    })
                    .cloned()
                    .collect(),
                other => self
                    .resolve_placement(other, &[], rule, sols, row)
                    .into_iter()
                    .collect(),
            };
            for g in graphs {
                if !self.input.targets.contains(&g) {
                    self.skip(
                        r,
                        sols,
                        row,
                        true,
                        format!("the retract would change <{g}>, which is not a writable graph"),
                    );
                    return Ok(());
                }
                retracts.push(Quad::new(
                    s.clone(),
                    p.clone(),
                    o.clone(),
                    GraphName::NamedNode(NamedNode::new_unchecked(g)),
                ));
            }
        }
        // Retracting a quad the run derived is a rule conflict (§6).
        for q in &retracts {
            if self.added_idx.get(q).is_some_and(|i| self.alive[*i]) {
                self.conflict(Conflict {
                    rule: r,
                    rule_iri: rule.spec.iri.clone(),
                    kind: "retract-derived",
                    detail: format!(
                        "the retract would delete {} which another rule derived",
                        quad_text(q)
                    ),
                });
                return Ok(());
            }
        }
        let newly: usize = minted
            .values()
            .filter(|t| !self.nulls.contains(&nt(t)))
            .count();
        let store = self.input.sandbox.store();
        let mut adds = 0;
        for q in heads {
            if store.contains(&q).unwrap_or(false)
                || self.added_idx.get(&q).is_some_and(|i| self.alive[*i])
            {
                continue;
            }
            let d = self.derivation(r, stratum, round, sols, row, DerivationKind::Added, &minted);
            self.push_added(q, d);
            adds += 1;
        }
        let mut dels = 0;
        for q in retracts {
            if !store.contains(&q).unwrap_or(false) || self.deleted_idx.contains_key(&q) {
                continue;
            }
            let d = self.derivation(
                r,
                stratum,
                round,
                sols,
                row,
                DerivationKind::Deleted,
                &minted,
            );
            self.push_deleted(q, d);
            dels += 1;
        }
        if adds > 0 && newly > 0 {
            for t in minted.values() {
                self.nulls.insert(nt(t));
            }
        }
        self.out.per_rule[r].adds += adds;
        self.out.per_rule[r].deletes += dels;
        Ok(())
    }

    /// Instantiate one template triple. `Ok(None)` when a variable is
    /// unbound (the trigger has no such triple); `Err` when the triple cannot
    /// be written (a literal subject, a blank node or triple term).
    fn instantiate(
        &mut self,
        tp: &TriplePattern,
        sols: &Solutions,
        row: &Row,
        minted: &HashMap<String, Term>,
    ) -> Result<Option<(NamedOrBlankNode, NamedNode, Term)>, String> {
        let Some(s) = inst(&tp.subject, sols, row, minted) else {
            return Ok(None);
        };
        let Some(p) = inst_pred(&tp.predicate, sols, row) else {
            return Ok(None);
        };
        let Some(o) = inst(&tp.object, sols, row, minted) else {
            return Ok(None);
        };
        // Terms merged earlier in the run are written as their representative.
        let s = self.canonical(s);
        let o = self.canonical(o);
        let subject = match s {
            Term::NamedNode(n) => NamedOrBlankNode::NamedNode(n),
            Term::BlankNode(_) => {
                return Err(
                    "the head's subject is a blank node, which no RDF Patch line can name".into(),
                )
            }
            other => return Err(format!("the head's subject {other} is not a resource")),
        };
        match &o {
            Term::NamedNode(_) | Term::Literal(_) => {}
            Term::BlankNode(_) => {
                return Err(
                    "the head's object is a blank node, which no RDF Patch line can name".into(),
                )
            }
            #[allow(unreachable_patterns)]
            _ => {
                return Err(
                    "the head's object is a triple term, which no RDF Patch line can carry".into(),
                )
            }
        }
        Ok(Some((subject, p, o)))
    }

    fn canonical(&mut self, t: Term) -> Term {
        if self.uf.parent.contains_key(&t) {
            self.uf.find(&t)
        } else {
            t
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fire_egd(
        &mut self,
        r: usize,
        stratum: usize,
        round: u32,
        sols: &Solutions,
        row: &Row,
        a: Variable,
        b: Variable,
    ) -> Result<(), String> {
        let input = self.input;
        let rule = &input.rules[r];
        let (Some(ta), Some(tb)) = (
            sols.get(row, a.as_str()).cloned(),
            sols.get(row, b.as_str()).cloned(),
        ) else {
            return Ok(());
        };
        let (ra, rb) = (self.uf.find(&ta), self.uf.find(&tb));
        if ra == rb {
            return Ok(());
        }
        let conflict = |kind: &'static str, detail: String| Conflict {
            rule: r,
            rule_iri: rule.spec.iri.clone(),
            kind,
            detail,
        };
        if matches!(ra, Term::Literal(_)) || matches!(rb, Term::Literal(_)) {
            let (x, y) = if nt(&ra) <= nt(&rb) {
                (nt(&ra), nt(&rb))
            } else {
                (nt(&rb), nt(&ra))
            };
            self.conflict(conflict(
                "literal",
                format!("{x} and {y} cannot be equated: a literal is never merged"),
            ));
            return Ok(());
        }
        if !matches!(ra, Term::NamedNode(_)) || !matches!(rb, Term::NamedNode(_)) {
            self.skip(
                r,
                sols,
                row,
                false,
                format!(
                    "{} and {} would be merged, but a blank node cannot be named in a patch",
                    nt(&ra),
                    nt(&rb)
                ),
            );
            return Ok(());
        }
        if let Some(why) = self.asserted_distinct(&ra, &rb)? {
            let (x, y) = if nt(&ra) <= nt(&rb) {
                (nt(&ra), nt(&rb))
            } else {
                (nt(&rb), nt(&ra))
            };
            self.conflict(conflict(
                "distinct",
                format!("{x} and {y} are asserted distinct ({why})"),
            ));
            return Ok(());
        }
        let (win, lose) = if self.rank(&ra) <= self.rank(&rb) {
            (ra, rb)
        } else {
            (rb, ra)
        };
        self.uf.union(&lose, &win);
        let lose_is_null = self.is_null(&lose);
        self.out.merges.push(Merge {
            loser: nt(&lose),
            winner: nt(&win),
            rule_iri: rule.spec.iri.clone(),
            mode: if lose_is_null {
                "substitution"
            } else if rule.spec.merge_mode == MergeMode::Rewrite {
                "rewrite"
            } else {
                "same-as-only"
            },
            reason: if lose_is_null { "null" } else { "lexmin" },
        });
        self.out.per_rule[r].merges += 1;
        if lose_is_null {
            self.substitutions.push((lose, win));
            return Ok(());
        }
        let store = self.input.sandbox.store();
        match rule.spec.merge_mode {
            MergeMode::SameAsOnly => {
                let placement = match &rule.egd_placement {
                    Some((pa, pb)) => {
                        if lose == ta {
                            pa.clone()
                        } else if lose == tb {
                            pb.clone()
                        } else {
                            Placement::Unknown
                        }
                    }
                    None => Placement::Unknown,
                };
                let graph = self
                    .resolve_placement(&placement, &[], rule, sols, row)
                    .or_else(|| self.graph_of_subject(&lose))
                    .filter(|g| self.input.targets.contains(g));
                let Some(g) = graph else {
                    self.skip(
                        r,
                        sols,
                        row,
                        true,
                        format!(
                            "no writable graph holds {}, so the owl:sameAs link has nowhere to go",
                            nt(&lose)
                        ),
                    );
                    return Ok(());
                };
                let Term::NamedNode(lose_n) = &lose else {
                    return Ok(());
                };
                let q = Quad::new(
                    NamedOrBlankNode::NamedNode(lose_n.clone()),
                    NamedNode::new_unchecked(OWL_SAME_AS),
                    win.clone(),
                    GraphName::NamedNode(NamedNode::new_unchecked(g)),
                );
                if !store.contains(&q).unwrap_or(false) {
                    let d = self.derivation(
                        r,
                        stratum,
                        round,
                        sols,
                        row,
                        DerivationKind::Added,
                        &HashMap::new(),
                    );
                    self.push_added(q, d);
                    self.out.per_rule[r].adds += 1;
                }
            }
            MergeMode::Rewrite => {
                for q in quads_mentioning(store, &lose) {
                    let Some(g) = graph_iri(&q) else { continue };
                    if !self.input.targets.contains(&g) {
                        self.skip(
                            r,
                            sols,
                            row,
                            true,
                            format!(
                                "rewriting {} would change <{g}>, which is not a writable graph",
                                nt(&lose)
                            ),
                        );
                        continue;
                    }
                    if self.added_idx.get(&q).is_some_and(|i| self.alive[*i]) {
                        self.conflict(conflict(
                            "rewrite-derived",
                            format!(
                                "the rewrite would delete {} which another rule derived",
                                quad_text(&q)
                            ),
                        ));
                        continue;
                    }
                    let Some(image) = substitute(&q, &lose, &win) else {
                        continue;
                    };
                    let kind = DerivationKind::Substituted {
                        from: nt(&lose),
                        to: nt(&win),
                    };
                    let d = self.derivation(r, stratum, round, sols, row, kind, &HashMap::new());
                    self.push_deleted(q, d.clone());
                    self.out.per_rule[r].deletes += 1;
                    if !store.contains(&image).unwrap_or(false) {
                        self.push_added(image, d);
                        self.out.per_rule[r].adds += 1;
                    }
                }
            }
        }
        Ok(())
    }

    /// The first writable graph (in name order) holding `t` as a subject.
    fn graph_of_subject(&self, t: &Term) -> Option<String> {
        let Term::NamedNode(n) = t else { return None };
        let mut graphs: BTreeSet<String> = BTreeSet::new();
        for q in self
            .input
            .sandbox
            .store()
            .quads_for_pattern(
                Some(NamedOrBlankNodeRef::NamedNode(n.as_ref())),
                None,
                None,
                None,
            )
            .flatten()
        {
            if let Some(g) = graph_iri(&q) {
                if self.input.targets.contains(&g) {
                    graphs.insert(g);
                }
            }
        }
        graphs.into_iter().next()
    }

    /// Whether two terms (and the classes merged into them) are asserted
    /// distinct (§4.4 step 1): `owl:differentFrom`, one `owl:AllDifferent`,
    /// or disjoint asserted classes.
    fn asserted_distinct(&mut self, a: &Term, b: &Term) -> Result<Option<&'static str>, String> {
        const Q: &str = "PREFIX owl: <http://www.w3.org/2002/07/owl#> \
            PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> \
            SELECT ?a ?b ?why WHERE { \
              { { ?a owl:differentFrom ?b } UNION { ?b owl:differentFrom ?a } BIND(\"owl:differentFrom\" AS ?why) } \
              UNION { ?d a owl:AllDifferent . { ?d owl:distinctMembers ?l } UNION { ?d owl:members ?l } \
                      ?l rdf:rest*/rdf:first ?a . ?l rdf:rest*/rdf:first ?b . BIND(\"owl:AllDifferent\" AS ?why) } \
              UNION { ?a a ?c1 . ?b a ?c2 . { ?c1 owl:disjointWith ?c2 } UNION { ?c2 owl:disjointWith ?c1 } BIND(\"owl:disjointWith\" AS ?why) } \
              UNION { ?a a ?c1 . ?b a ?c2 . FILTER(?c1 != ?c2) ?x a owl:AllDisjointClasses ; owl:members ?l2 . \
                      ?l2 rdf:rest*/rdf:first ?c1 . ?l2 rdf:rest*/rdf:first ?c2 . BIND(\"owl:AllDisjointClasses\" AS ?why) } \
            } LIMIT 1";
        let ca = self.uf.class(a);
        let cb = self.uf.class(b);
        let mut checked = 0;
        for x in &ca {
            for y in &cb {
                checked += 1;
                if checked > 64 {
                    return Ok(None);
                }
                let mut prepared = self
                    .input
                    .sandbox
                    .query_options()
                    .parse_query(Q)
                    .map_err(|e| e.to_string())?
                    .substitute_variable(Variable::new_unchecked("a"), x.clone())
                    .substitute_variable(Variable::new_unchecked("b"), y.clone());
                {
                    let ds = prepared.dataset_mut();
                    ds.set_default_graph(self.default_graphs.clone());
                    ds.set_available_named_graphs(self.named_graphs.clone());
                }
                if let QueryResults::Solutions(mut s) = prepared
                    .on_store(self.input.sandbox.store())
                    .execute()
                    .map_err(|e| e.to_string())?
                {
                    if let Some(Ok(row)) = s.next() {
                        let why = match row.get("why") {
                            Some(Term::Literal(l)) if l.value() == "owl:differentFrom" => {
                                "owl:differentFrom"
                            }
                            Some(Term::Literal(l)) if l.value() == "owl:AllDifferent" => {
                                "owl:AllDifferent"
                            }
                            Some(Term::Literal(l)) if l.value() == "owl:disjointWith" => {
                                "owl:disjointWith"
                            }
                            _ => "owl:AllDisjointClasses",
                        };
                        return Ok(Some(why));
                    }
                }
            }
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    fn fire_native(
        &mut self,
        r: usize,
        stratum: usize,
        round: u32,
        sols: &Solutions,
        row: &Row,
        native: &Native,
    ) -> Result<(), String> {
        let input = self.input;
        let rule = &input.rules[r];
        let store = input.sandbox.store();
        match native {
            Native::DatatypeRelabel {
                datatype,
                predicate,
            } => {
                let (Some(Term::NamedNode(this)), Some(Term::Literal(v))) =
                    (sols.get(row, "this").cloned(), sols.get(row, "v").cloned())
                else {
                    return Ok(());
                };
                if v.language().is_some() {
                    self.skip(
                        r,
                        sols,
                        row,
                        false,
                        format!("{v} carries a language tag; relabelling it would drop the tag"),
                    );
                    return Ok(());
                }
                let relabelled = Literal::new_typed_literal(
                    v.value(),
                    NamedNode::new_unchecked(datatype.as_str()),
                );
                if !crate::shacl::constraints::xsd_lexical_valid(&relabelled) {
                    self.skip(
                        r,
                        sols,
                        row,
                        false,
                        format!(
                            "\"{}\" is not in the lexical space of <{datatype}>",
                            v.value()
                        ),
                    );
                    return Ok(());
                }
                // The value atom says where the literal sits.
                let Some(k) = rule.atoms.iter().position(
                    |a| matches!(&a.pattern.object, TermPattern::Variable(x) if x.as_str() == "v"),
                ) else {
                    return Ok(());
                };
                let Some(g) = self.resolve_placement(&Placement::Atom(k), &[], rule, sols, row)
                else {
                    return Ok(());
                };
                if !self.input.targets.contains(&g) {
                    self.skip(
                        r,
                        sols,
                        row,
                        true,
                        format!("{v} sits in <{g}>, which is not a writable graph"),
                    );
                    return Ok(());
                }
                let gn = GraphName::NamedNode(NamedNode::new_unchecked(g));
                let p = NamedNode::new_unchecked(predicate.as_str());
                let old = Quad::new(
                    this.clone(),
                    p.clone(),
                    Term::Literal(v.clone()),
                    gn.clone(),
                );
                let new = Quad::new(this, p, Term::Literal(relabelled.clone()), gn);
                if self.added_idx.get(&old).is_some_and(|i| self.alive[*i]) {
                    return Ok(());
                }
                let mut extra = HashMap::new();
                extra.insert("relabelled".to_string(), Term::Literal(relabelled));
                if store.contains(&old).unwrap_or(false) && !self.deleted_idx.contains_key(&old) {
                    let d = self.derivation(
                        r,
                        stratum,
                        round,
                        sols,
                        row,
                        DerivationKind::Deleted,
                        &extra,
                    );
                    self.push_deleted(old, d);
                    self.out.per_rule[r].deletes += 1;
                }
                if !store.contains(&new).unwrap_or(false) {
                    let d = self.derivation(
                        r,
                        stratum,
                        round,
                        sols,
                        row,
                        DerivationKind::Added,
                        &extra,
                    );
                    self.push_added(new, d);
                    self.out.per_rule[r].adds += 1;
                }
            }
            Native::MaxCountKeepLexmin {
                max,
                predicate,
                inverse,
            } => {
                let Some(this) = sols.get(row, "this").cloned() else {
                    return Ok(());
                };
                let p = NamedNode::new_unchecked(predicate.as_str());
                // The focus node's values over every premise graph, as the
                // validator counts them.
                let mut values: BTreeMap<String, Vec<Quad>> = BTreeMap::new();
                let quads: Vec<Quad> = if *inverse {
                    store
                        .quads_for_pattern(None, Some(p.as_ref()), Some(this.as_ref()), None)
                        .flatten()
                        .collect()
                } else {
                    match &this {
                        Term::NamedNode(n) => store
                            .quads_for_pattern(
                                Some(NamedOrBlankNodeRef::NamedNode(n.as_ref())),
                                Some(p.as_ref()),
                                None,
                                None,
                            )
                            .flatten()
                            .collect(),
                        _ => Vec::new(),
                    }
                };
                for q in quads {
                    let Some(g) = graph_iri(&q) else { continue };
                    if !self.input.premises.contains(&g) {
                        continue;
                    }
                    let value = if *inverse {
                        Term::from(q.subject.clone())
                    } else {
                        q.object.clone()
                    };
                    values.entry(nt(&value)).or_default().push(q);
                }
                if values.len() <= *max {
                    return Ok(());
                }
                let surplus: Vec<(String, Vec<Quad>)> = values.into_iter().skip(*max).collect();
                for (value, quads) in surplus {
                    for q in quads {
                        let g = graph_iri(&q).unwrap_or_default();
                        if !self.input.targets.contains(&g) {
                            self.skip(
                                r,
                                sols,
                                row,
                                true,
                                format!("{value} sits in <{g}>, which is not a writable graph"),
                            );
                            continue;
                        }
                        if self.added_idx.get(&q).is_some_and(|i| self.alive[*i]) {
                            self.conflict(Conflict {
                                rule: r,
                                rule_iri: rule.spec.iri.clone(),
                                kind: "delete-derived",
                                detail: format!("keeping the {max} smallest values would delete {} which another rule derived", quad_text(&q)),
                            });
                            continue;
                        }
                        if self.deleted_idx.contains_key(&q) {
                            continue;
                        }
                        let mut extra = HashMap::new();
                        extra.insert(
                            "surplus".to_string(),
                            if *inverse {
                                Term::from(q.subject.clone())
                            } else {
                                q.object.clone()
                            },
                        );
                        let d = self.derivation(
                            r,
                            stratum,
                            round,
                            sols,
                            row,
                            DerivationKind::Deleted,
                            &extra,
                        );
                        self.push_deleted(q, d);
                        self.out.per_rule[r].deletes += 1;
                    }
                }
            }
        }
        Ok(())
    }

    /// Count each `ots:Report` rule's distinct triggers on the final state.
    fn count_report_rules(&mut self) -> Result<(), String> {
        let input = self.input;
        for &r in &input.strata.report {
            if self.deadline_passed() {
                self.stop.get_or_insert("time");
                break;
            }
            let rule = &input.rules[r];
            let sols = self.triggers(rule, 1)?;
            self.out.per_rule[r].triggers += sols.rows.len();
            self.out.report_triggers.push((r, sols.rows.len()));
        }
        if self.out.exhausted.is_none() {
            self.out.exhausted = self.stop;
        }
        Ok(())
    }

    fn finish(mut self) -> ChaseOutcome {
        let added: Vec<(Quad, Derivation)> = self
            .added
            .into_iter()
            .zip(self.alive)
            .filter(|(_, alive)| *alive)
            .map(|(a, _)| a)
            .collect();
        self.out.added = added;
        self.out.deleted = self.deleted;
        self.out.nulls = self.nulls.len();
        // Keep the stratum the last pass saw for every rule.
        for (r, s) in &self.stratum_of {
            self.out.per_rule[*r].stratum = Some(*s);
        }
        self.out
    }
}

fn inst(
    t: &TermPattern,
    sols: &Solutions,
    row: &Row,
    minted: &HashMap<String, Term>,
) -> Option<Term> {
    match t {
        TermPattern::NamedNode(n) => Some(Term::NamedNode(n.clone())),
        TermPattern::Literal(l) => Some(Term::Literal(l.clone())),
        TermPattern::Variable(v) => minted
            .get(v.as_str())
            .cloned()
            .or_else(|| sols.get(row, v.as_str()).cloned()),
        _ => None,
    }
}

fn inst_pred(p: &NamedNodePattern, sols: &Solutions, row: &Row) -> Option<NamedNode> {
    match p {
        NamedNodePattern::NamedNode(n) => Some(n.clone()),
        NamedNodePattern::Variable(v) => match sols.get(row, v.as_str()) {
            Some(Term::NamedNode(n)) => Some(n.clone()),
            _ => None,
        },
    }
}

fn graph_iri(q: &Quad) -> Option<String> {
    match &q.graph_name {
        GraphName::NamedNode(n) => Some(n.as_str().to_string()),
        _ => None,
    }
}

/// `s p o g` in N-Quads form, without the final dot.
pub fn quad_text(q: &Quad) -> String {
    match &q.graph_name {
        GraphName::DefaultGraph => format!("{} {} {}", q.subject, q.predicate, q.object),
        g => format!("{} {} {} {g}", q.subject, q.predicate, q.object),
    }
}

/// Every quad of the store with `t` as subject or object.
fn quads_mentioning(store: &oxigraph::store::Store, t: &Term) -> Vec<Quad> {
    let mut out: Vec<Quad> = Vec::new();
    if let Term::NamedNode(n) = t {
        out.extend(
            store
                .quads_for_pattern(
                    Some(NamedOrBlankNodeRef::NamedNode(n.as_ref())),
                    None,
                    None,
                    None,
                )
                .flatten(),
        );
    }
    out.extend(
        store
            .quads_for_pattern(None, None, Some(t.as_ref()), None)
            .flatten(),
    );
    out.sort_by_key(quad_text);
    out.dedup();
    out
}

/// `q` with `from` replaced by `to`; `None` when that would make a literal
/// the subject.
fn substitute(q: &Quad, from: &Term, to: &Term) -> Option<Quad> {
    let subject = if &Term::from(q.subject.clone()) == from {
        match to {
            Term::NamedNode(n) => NamedOrBlankNode::NamedNode(n.clone()),
            Term::BlankNode(b) => NamedOrBlankNode::BlankNode(b.clone()),
            _ => return None,
        }
    } else {
        q.subject.clone()
    };
    let object = if &q.object == from {
        to.clone()
    } else {
        q.object.clone()
    };
    Some(Quad::new(
        subject,
        q.predicate.clone(),
        object,
        q.graph_name.clone(),
    ))
}

/// `VALUES` binding an atom's variables to the delta quads it can match;
/// `None` when no delta quad matches it.
fn delta_values(atom: &super::rules::Atom, delta: &[Quad]) -> Option<GraphPattern> {
    let mut vars: Vec<Variable> = Vec::new();
    let mut push = |v: &Variable| {
        if !vars.contains(v) {
            vars.push(v.clone());
        }
    };
    let tp = &atom.pattern;
    if let TermPattern::Variable(v) = &tp.subject {
        push(v);
    }
    if let NamedNodePattern::Variable(v) = &tp.predicate {
        push(v);
    }
    if let TermPattern::Variable(v) = &tp.object {
        push(v);
    }
    if let NamedNodePattern::Variable(v) = &atom.graph {
        push(v);
    }
    let mut bindings: Vec<Vec<Option<GroundTerm>>> = Vec::new();
    'quads: for q in delta {
        let mut cell: HashMap<String, Term> = HashMap::new();
        let mut bind = |pat: Option<&Variable>, constant: Option<Term>, value: Term| -> bool {
            match (pat, constant) {
                (Some(v), _) => match cell.get(v.as_str()) {
                    Some(prev) => prev == &value,
                    None => {
                        cell.insert(v.as_str().to_string(), value);
                        true
                    }
                },
                (None, Some(c)) => c == value,
                (None, None) => false,
            }
        };
        let (sv, sc) = match &tp.subject {
            TermPattern::Variable(v) => (Some(v), None),
            TermPattern::NamedNode(n) => (None, Some(Term::NamedNode(n.clone()))),
            TermPattern::Literal(l) => (None, Some(Term::Literal(l.clone()))),
            _ => continue 'quads,
        };
        if !bind(sv, sc, Term::from(q.subject.clone())) {
            continue;
        }
        let (pv, pc) = match &tp.predicate {
            NamedNodePattern::Variable(v) => (Some(v), None),
            NamedNodePattern::NamedNode(n) => (None, Some(Term::NamedNode(n.clone()))),
        };
        if !bind(pv, pc, Term::NamedNode(q.predicate.clone())) {
            continue;
        }
        let (ov, oc) = match &tp.object {
            TermPattern::Variable(v) => (Some(v), None),
            TermPattern::NamedNode(n) => (None, Some(Term::NamedNode(n.clone()))),
            TermPattern::Literal(l) => (None, Some(Term::Literal(l.clone()))),
            _ => continue 'quads,
        };
        if !bind(ov, oc, q.object.clone()) {
            continue;
        }
        let GraphName::NamedNode(g) = &q.graph_name else {
            continue;
        };
        let (gv, gc) = match &atom.graph {
            NamedNodePattern::Variable(v) => (Some(v), None),
            NamedNodePattern::NamedNode(n) => (None, Some(Term::NamedNode(n.clone()))),
        };
        if !bind(gv, gc, Term::NamedNode(g.clone())) {
            continue;
        }
        let mut row = Vec::with_capacity(vars.len());
        for v in &vars {
            match cell.get(v.as_str()).and_then(ground) {
                Some(g) => row.push(Some(g)),
                None => continue 'quads,
            }
        }
        bindings.push(row);
    }
    if bindings.is_empty() {
        return None;
    }
    Some(GraphPattern::Values {
        variables: vars,
        bindings,
    })
}
