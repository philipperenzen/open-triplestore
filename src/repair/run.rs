//! One repair run, end to end: seed the sandbox, gather the rules, stratify,
//! validate (when asked), chase, validate again, and build the proposal.
//! Blocking — the handler runs it on a blocking thread under the request's
//! deadline, which the copy and the chase both check, so a run that runs
//! out of time returns what it has rather than being cut off.

use std::time::{Duration, Instant};

use serde::Deserialize;

use super::chase::{self, Budget, ChaseInput, ChaseOutcome};
use super::compile::{self, Compiled, Policies, ReportOnly};
use super::proposal::{self, Base, Proposal, ReportInput, SeverityCounts, Validation};
use super::rules::{prepare, Origin, PreparedRule, RuleSpec};
use super::sandbox::{self, Premises, SeedError};
use super::stratify::{stratify, Strata};
use crate::store::TripleStore;

/// Defaults and caps of the budgets (§4.5).
pub const DEFAULT_ROUNDS: u32 = 100;
pub const MAX_ROUNDS: u32 = 1_000;
pub const DEFAULT_NULLS: usize = 10_000;
pub const MAX_NULLS: usize = 1_000_000;
pub const DEFAULT_OPS: usize = 50_000;
pub const MAX_OPS: usize = 500_000;
pub const MAX_TIMEOUT_SECS: u64 = 240;
/// The smaller budget a heuristic (assistant-proposed) rule set runs with (§9).
pub const HEURISTIC_ROUNDS: u32 = 10;
pub const HEURISTIC_NULLS: usize = 1_000;
pub const HEURISTIC_OPS: usize = 5_000;

/// `derive`: which rules are compiled for the run.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Derive {
    /// From the dataset's SHACL Core shapes (default on).
    pub from_shapes: bool,
    /// From the OWL axioms among the premises (default on).
    pub from_owl: bool,
    /// `rdfs:domain` / `rdfs:range` / `rdfs:subClassOf` too (default off:
    /// a materialising dataset derives them already).
    pub entailment_rules: bool,
    /// Import the shapes graphs' SHACL-AF `sh:rule`s as rules (default off).
    pub shacl_rules: bool,
}

impl Default for Derive {
    fn default() -> Self {
        Self {
            from_shapes: true,
            from_owl: true,
            entailment_rules: false,
            shacl_rules: false,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ScopeRequest {
    /// Restrict the dataset graphs seeded (model, entailment and shape
    /// graphs are always seeded whole).
    pub graphs: Option<Vec<String>>,
    /// Restrict the focus nodes (`?this`) of every rule.
    pub focus: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BudgetRequest {
    pub rounds: Option<u32>,
    pub nulls: Option<usize>,
    pub ops: Option<usize>,
    pub timeout_secs: Option<u64>,
}

/// `POST /api/datasets/:id/repair` (§8.1).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RepairRequest {
    /// Graphs holding authored `ots:Rule`s.
    pub rules: Vec<String>,
    pub derive: Derive,
    /// Opt-in policies: `closed-delete`, `maxCount-keep-lexmin`,
    /// `datatype-relabel`.
    pub policies: Vec<String>,
    /// Use this shapes graph instead of the dataset's.
    pub shapes_graph: Option<String>,
    pub scope: ScopeRequest,
    pub budget: BudgetRequest,
    /// Validate the sandbox before and after the chase.
    pub validate: bool,
    /// Keep the proposal (`GET …/repair/proposals/:pid`, `apply`).
    pub persist: bool,
    /// `false` re-evaluates every rule body in full each round.
    pub semi_naive: Option<bool>,
    /// Rules proposed by the assistant, as Turtle `ots:Rule`s: run alone,
    /// under the heuristic guard and budget (§9).
    pub heuristic_rules: Option<String>,
}

/// Why a run did not produce a proposal.
#[derive(Debug)]
pub enum RunError {
    /// A rule that does not load, a rule set that does not stratify: 400.
    BadRequest(String),
    /// The time budget ran out before the copy was complete: 503.
    Unavailable(String),
    Internal(String),
}

/// Everything a run needs, gathered by the handler with the caller's rights.
pub struct Job {
    pub main: TripleStore,
    pub dataset_id: String,
    pub base_url: String,
    pub actor_iri: Option<String>,
    pub premises: Premises,
    /// Authored rules already read from the request's rule graphs.
    pub authored: Vec<RuleSpec>,
    pub rule_graphs: Vec<String>,
    pub heuristic: Option<Vec<RuleSpec>>,
    pub derive: Derive,
    pub policies: Policies,
    pub focus: Option<Vec<oxigraph::model::Term>>,
    pub rounds: u32,
    pub nulls: usize,
    pub ops: usize,
    pub deadline: Instant,
    pub validate: bool,
    pub semi_naive: bool,
    pub base_commit: Option<String>,
    pub partial: bool,
}

/// The base for minting nulls (§4.3, open question 1: the instance's own
/// base URL, so a null is an IRI of the store that minted it).
pub fn null_prefixes(base_url: &str) -> Vec<String> {
    vec![
        format!("{}{}", base_url.trim_end_matches('/'), chase::GENID),
        format!("{}{}", opengraph::skolem::DEFAULT_SKOLEM_BASE, chase::GENID),
    ]
}

fn merged(
    reports: Vec<crate::shacl::report::ValidationReport>,
) -> crate::shacl::report::ValidationReport {
    let mut results = Vec::new();
    let mut conforms = true;
    for r in reports {
        conforms &= r.conforms;
        results.extend(r.results);
    }
    crate::shacl::report::ValidationReport {
        conforms,
        results_count: results.len(),
        results,
        metrics: None,
    }
}

fn validate_sandbox(
    sandbox: &TripleStore,
    premises: &Premises,
) -> Result<crate::shacl::report::ValidationReport, RunError> {
    let _path = crate::store::telemetry::ValidationPathGuard::set("repair");
    let data = premises.validation_graphs();
    let mut reports = Vec::new();
    for s in &premises.shapes {
        reports.push(
            crate::shacl::validate(sandbox, s, &data)
                .map_err(|e| RunError::BadRequest(format!("validating against <{s}>: {e}")))?,
        );
    }
    Ok(merged(reports))
}

/// Run the heuristic guard on an assistant-proposed rule set (§9): no
/// destructive rule, no retract, no `Rewrite` merge, and every predicate it
/// names must occur among the premises.
fn heuristic_guard(
    rules: &[PreparedRule],
    sandbox: &TripleStore,
    premises: &[String],
) -> Result<(), String> {
    for r in rules {
        let s = &r.spec;
        if s.destructive
            || !r.retract.is_empty()
            || s.merge_mode == super::rules::MergeMode::Rewrite
        {
            return Err(format!(
                "heuristic rule <{}> may not be destructive, retract or merge by rewriting",
                s.iri
            ));
        }
        let mut preds = r.deps.reads.clone();
        preds.extend(&r.deps.produces);
        if preds.all {
            return Err(format!(
                "heuristic rule <{}> has a variable predicate",
                s.iri
            ));
        }
        for p in &preds.iris {
            if p == super::rules::RDF_TYPE || p == super::rules::OWL_SAME_AS {
                continue;
            }
            let Ok(pn) = oxigraph::model::NamedNode::new(p) else {
                continue;
            };
            let present = premises.iter().any(|g| {
                oxigraph::model::NamedNode::new(g)
                    .map(|gn| {
                        sandbox
                            .store()
                            .quads_for_pattern(
                                None,
                                Some(pn.as_ref()),
                                None,
                                Some(gn.as_ref().into()),
                            )
                            .next()
                            .is_some()
                    })
                    .unwrap_or(false)
            });
            if !present {
                return Err(format!(
                    "heuristic rule <{}> names <{p}>, which no premise graph uses",
                    s.iri
                ));
            }
        }
    }
    Ok(())
}

/// Compile and gather the rule set of a run.
/// A run's rules: the specs, the report-only entries, the skipped targets.
type Gathered = (Vec<RuleSpec>, Vec<ReportOnly>, Vec<String>);

fn gather(job: &Job, sandbox: &TripleStore) -> Result<Gathered, RunError> {
    let mut specs: Vec<RuleSpec> = Vec::new();
    let mut report_only: Vec<ReportOnly> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    if let Some(h) = &job.heuristic {
        return Ok((h.clone(), report_only, skipped));
    }
    let closure_graphs = job.premises.validation_graphs();
    let mut compiled = Compiled::default();
    for s in &job.premises.shapes {
        // ots:Rule resources kept in a shapes graph run like authored ones.
        specs.extend(
            super::vocab::load_rules(sandbox, s, Origin::Authored)
                .map_err(|e| RunError::BadRequest(e.to_string()))?,
        );
        if job.derive.from_shapes {
            compiled.extend(
                compile::compile_shapes(sandbox, s, &closure_graphs, job.policies)
                    .map_err(|e| RunError::BadRequest(format!("shapes graph <{s}>: {e}")))?,
            );
        }
        if job.derive.shacl_rules {
            compiled.extend(
                compile::import_shacl_af(sandbox, s, &closure_graphs)
                    .map_err(|e| RunError::BadRequest(format!("SHACL-AF rules of <{s}>: {e}")))?,
            );
        }
    }
    if job.derive.from_owl {
        compiled.extend(
            compile::compile_owl(sandbox, &job.premises.premises, job.derive.entailment_rules)
                .map_err(|e| RunError::BadRequest(format!("OWL axioms: {e}")))?,
        );
    }
    specs.extend(job.authored.iter().cloned());
    specs.extend(compiled.rules);
    report_only.extend(compiled.report_only);
    skipped.extend(compiled.skipped);
    // One rule per IRI: two definitions under one name is an authoring error.
    let mut seen = std::collections::HashSet::new();
    for s in &specs {
        if !seen.insert(s.iri.clone()) {
            return Err(RunError::BadRequest(format!(
                "rule <{}> is defined twice",
                s.iri
            )));
        }
    }
    Ok((specs, report_only, skipped))
}

/// Run the chase once over a fresh copy.
#[allow(clippy::too_many_arguments)]
fn chase_once(
    job: &Job,
    sandbox: &TripleStore,
    rules: &[PreparedRule],
    strata: &Strata,
    same_as: &[(oxigraph::model::Term, oxigraph::model::Term)],
    budget: Budget,
) -> Result<ChaseOutcome, RunError> {
    let prefixes = null_prefixes(&job.base_url);
    let input = ChaseInput {
        sandbox,
        rules,
        strata,
        premises: &job.premises.premises,
        targets: &job.premises.targets,
        null_base: job.base_url.trim_end_matches('/'),
        null_prefixes: &prefixes,
        focus: job.focus.as_deref(),
        budget,
        same_as_seeds: same_as,
        semi_naive: job.semi_naive,
    };
    chase::chase(&input).map_err(RunError::Internal)
}

/// Run a job.
pub fn run(job: Job) -> Result<Proposal, RunError> {
    let started = Instant::now();
    let started_at = chrono::Utc::now().to_rfc3339();
    // The base marker is read before the copy, so a write that lands while
    // the copy is taken is after it: the precondition then refuses a stale
    // apply rather than missing it (§1.3).
    let changes = job.main.changes();
    let (sequence, epoch) = if changes.enabled() {
        (Some(changes.last_seq()), Some(changes.epoch().to_string()))
    } else {
        (None, None)
    };
    let generation_before = job.main.write_generation();
    let seeded_graphs = job.premises.seeded();
    let seed = |deadline: Instant| -> Result<sandbox::Seeded, RunError> {
        sandbox::seed(&job.main, &seeded_graphs, deadline).map_err(|e| match e {
            SeedError::Deadline(m) => RunError::Unavailable(format!(
                "the repair run ran out of time copying the dataset into its sandbox: {m}"
            )),
            SeedError::Store(m) => RunError::Internal(m),
        })
    };
    let mut seeded = seed(job.deadline)?;
    let generation_after = job.main.write_generation();
    let consistent = seeded.source != crate::store::graph_snapshot::SnapshotSource::Live
        || generation_before == generation_after;

    let (specs, mut report_only, skipped) = gather(&job, &seeded.store)?;
    let mut prepared: Vec<PreparedRule> = Vec::with_capacity(specs.len());
    for s in specs {
        prepared.push(prepare(s).map_err(|e| RunError::BadRequest(e.to_string()))?);
    }
    if job.heuristic.is_some() {
        heuristic_guard(&prepared, &seeded.store, &job.premises.premises)
            .map_err(RunError::BadRequest)?;
    }
    let mut strata = stratify(&prepared).map_err(RunError::BadRequest)?;

    let same_as = if job.premises.identity.propagates_same_as() {
        compile::same_as_pairs(&seeded.store, &job.premises.premises).map_err(RunError::Internal)?
    } else {
        Vec::new()
    };
    let before = if job.validate {
        Some(validate_sandbox(&seeded.store, &job.premises)?)
    } else {
        None
    };
    let budget = |rounds, nulls, ops| Budget {
        rounds,
        nulls,
        ops,
        deadline: job.deadline,
    };
    let mut outcome;
    let mut dropped: Vec<String> = Vec::new();
    loop {
        let b = if job.heuristic.is_some() {
            budget(
                job.rounds.min(HEURISTIC_ROUNDS),
                job.nulls.min(HEURISTIC_NULLS),
                job.ops.min(HEURISTIC_OPS),
            )
        } else {
            budget(job.rounds, job.nulls, job.ops)
        };
        outcome = chase_once(&job, &seeded.store, &prepared, &strata, &same_as, b)?;
        // A heuristic rule that exhausts its budget is dropped and the run
        // repeated without it, never returned as a partial repair (§9).
        let exhausted_by_rule = outcome.exhausted.is_some_and(|k| k != "time");
        if job.heuristic.is_none() || !exhausted_by_rule || prepared.is_empty() {
            break;
        }
        let worst = outcome
            .per_rule
            .iter()
            .enumerate()
            .max_by_key(|(i, s)| (s.adds + s.deletes + s.merges, std::cmp::Reverse(*i)))
            .map(|(i, _)| i)
            .unwrap_or(0);
        dropped.push(prepared[worst].spec.iri.clone());
        prepared.remove(worst);
        strata = stratify(&prepared).map_err(RunError::BadRequest)?;
        seeded = seed(job.deadline)?;
    }
    let validation = match before {
        Some(before) => {
            let after = validate_sandbox(&seeded.store, &job.premises)?;
            proposal::attribute_residual(&mut report_only, &after);
            Some(Validation {
                before: SeverityCounts::of(&before),
                after: SeverityCounts::of(&after),
                residual_total: after.results.len(),
                residual: after
                    .results
                    .into_iter()
                    .take(proposal::MAX_RESIDUAL)
                    .collect(),
            })
        }
        None => None,
    };
    let base = Base {
        graphs: job.premises.dataset_graphs.clone(),
        commit: job.base_commit.clone(),
        sequence,
        epoch,
        write_generation: generation_before,
        consistent,
        entailment: job.premises.entailment,
        source: seeded.source.as_str(),
        quads: seeded.quads,
    };
    proposal::build(ReportInput {
        dataset_id: &job.dataset_id,
        base_url: &job.base_url,
        actor_iri: job.actor_iri.as_deref(),
        base,
        rules: &prepared,
        strata: &strata,
        outcome: &outcome,
        report_only,
        skipped,
        rule_graphs: &job.rule_graphs,
        validation,
        elapsed_ms: started.elapsed().as_millis() as u64,
        partial: job.partial,
        withheld_graphs: job.premises.withheld,
        withheld_shapes: job.premises.shapes_withheld,
        started_at,
        dropped,
    })
    .map_err(RunError::Internal)
}

/// The deadline of a request with `timeout_secs` (capped).
pub fn deadline(timeout_secs: u64) -> (Instant, Duration) {
    let secs = timeout_secs.clamp(1, MAX_TIMEOUT_SECS);
    let d = Duration::from_secs(secs);
    (Instant::now() + d, d)
}
