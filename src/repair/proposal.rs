//! The proposal artefact: an RDF Patch and a structured report
//! (`docs/notes/repair-layer-design.md` §7.1, §7.2).
//!
//! Every patch header is a function of the snapshot and the rules — the id
//! is a content hash of the dataset, the base marker, the rule set's digest
//! and the patch body — so two runs over one snapshot with one rule set give
//! byte-identical patch text. The volatile facts (`write_generation`,
//! `elapsed_ms`, the PROV activity's start time) live only in the JSON
//! report. `D` lines precede `A` lines per graph, both sorted, as
//! `rdf_patch::generate` writes them; a `#` comment before each line names
//! the rule and trigger, inert for every RDF Patch reader.

use std::collections::{BTreeMap, HashMap, HashSet};

use oxigraph::model::{GraphName, Quad};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::chase::{ChaseOutcome, Derivation};
use super::compile::ReportOnly;
use super::rules::{Confidence, PreparedRule, RuleSpec};
use super::stratify::Strata;
use crate::shacl::report::{Severity, ValidationReport, ValidationResult};

/// The engine name and version in `H engine`.
pub const ENGINE: &str = "ots-chase/0.1";
/// Actions carried inline in a report; the rest are paged on the stored
/// proposal (§7.2).
pub const MAX_INLINE_ACTIONS: usize = 10_000;
/// Residual validation results carried in a report.
pub const MAX_RESIDUAL: usize = 1_000;

/// The base marker a proposal was computed from (§7.1, §1.3).
#[derive(Debug, Clone, Serialize)]
pub struct Base {
    /// The dataset graphs the run read: the commit and the sequence below
    /// are about these, and so are the staleness check and the apply's
    /// precondition.
    pub graphs: Vec<String>,
    /// The newest commit touching those graphs.
    pub commit: Option<String>,
    /// The change log's sequence before the copy was taken (change capture
    /// on); the apply precondition `if-base-sequence` compares against it.
    pub sequence: Option<i64>,
    pub epoch: Option<String>,
    /// Process-local; informational.
    pub write_generation: u64,
    /// `false` when the in-memory store moved while it was copied.
    pub consistent: bool,
    pub entailment: &'static str,
    /// Where the copy was read from: `mirror`, `snapshot` or `live`.
    pub source: &'static str,
    pub quads: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeverityCounts {
    pub violation: usize,
    pub warning: usize,
    pub info: usize,
}

impl SeverityCounts {
    pub fn of(report: &ValidationReport) -> Self {
        let mut c = SeverityCounts {
            violation: 0,
            warning: 0,
            info: 0,
        };
        for r in &report.results {
            match r.severity {
                Severity::Violation => c.violation += 1,
                Severity::Warning => c.warning += 1,
                Severity::Info => c.info += 1,
            }
        }
        c
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Validation {
    pub before: SeverityCounts,
    pub after: SeverityCounts,
    pub residual: Vec<ValidationResult>,
    pub residual_total: usize,
}

/// One proposed line and why.
#[derive(Debug, Clone, Serialize)]
pub struct Action {
    pub op: &'static str,
    pub graph: String,
    pub quad: String,
    pub rule: String,
    pub stratum: Option<usize>,
    pub confidence: Confidence,
    pub destructive: bool,
    pub trigger: Trigger,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub violation: Option<ViolationView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub kind: super::chase::DerivationKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct Trigger {
    pub id: String,
    pub round: u32,
    pub bindings: BTreeMap<String, String>,
    pub premises: Vec<String>,
}

/// The validation result an action answers: the compiled rule's constraint
/// at the trigger's focus node.
#[derive(Debug, Clone, Serialize)]
pub struct ViolationView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_node: Option<String>,
    pub source_shape: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub source_constraint: String,
    pub source_constraint_component: String,
}

/// Everything the report needs that is not in the chase outcome.
pub struct ReportInput<'a> {
    pub dataset_id: &'a str,
    pub base_url: &'a str,
    pub actor_iri: Option<&'a str>,
    pub base: Base,
    pub rules: &'a [PreparedRule],
    pub strata: &'a Strata,
    pub outcome: &'a ChaseOutcome,
    pub report_only: Vec<ReportOnly>,
    pub skipped: Vec<String>,
    pub rule_graphs: &'a [String],
    pub validation: Option<Validation>,
    pub elapsed_ms: u64,
    pub partial: bool,
    pub withheld_graphs: usize,
    pub withheld_shapes: usize,
    pub started_at: String,
    /// Heuristic rules dropped because they exhausted their budget.
    pub dropped: Vec<String>,
}

/// A proposal: its id, its patch text and its report.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub id: String,
    pub patch: String,
    pub report: serde_json::Value,
    /// The actions beyond the inline cap (all of them, in order), for the
    /// stored proposal's paging.
    pub actions: Vec<serde_json::Value>,
}

fn sha(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update(b"\x1f");
        }
        h.update(p.as_bytes());
    }
    hex::encode(h.finalize())
}

/// The digest of a rule set: every rule's definition, by IRI.
pub fn rules_digest(rules: &[RuleSpec]) -> String {
    let mut defs: Vec<String> = rules
        .iter()
        .map(|r| serde_json::to_string(r).unwrap_or_default())
        .collect();
    defs.sort();
    format!(
        "urn:ots:rules:{}",
        &sha(&defs.iter().map(String::as_str).collect::<Vec<_>>())[..32]
    )
}

fn graph_of(q: &Quad) -> String {
    match &q.graph_name {
        GraphName::NamedNode(n) => n.as_str().to_string(),
        _ => String::new(),
    }
}

fn statement(q: &Quad) -> String {
    format!("{} {} {}", q.subject, q.predicate, q.object)
}

/// Build the patch and the report.
/// RDF Patch text from statement sets: `(target, from, to)` per graph, each
/// set holding `"s p o"` statements in N-Triples form. No store is read and
/// no id is minted: the headers are written as given, in order, so the same
/// input always gives the same bytes (the id is a content hash, §7.1). `D`
/// lines come before `A` lines per graph, both sorted, as
/// `rdf_patch::generate` writes them; `comment(code, graph, statement)` may
/// put `#` lines before a line, which every RDF Patch reader skips.
fn patch_text(
    headers: &[(&str, &str)],
    mappings: &[(String, &HashSet<String>, &HashSet<String>)],
    comment: &dyn Fn(char, &str, &str) -> Option<String>,
) -> String {
    let mut out = String::new();
    for (k, v) in headers {
        if v.starts_with('<') || v.starts_with('"') {
            out.push_str(&format!("H {k} {v} .\n"));
        } else if v.contains("://") || v.starts_with("urn:") {
            out.push_str(&format!("H {k} <{v}> .\n"));
        } else {
            out.push_str(&format!("H {k} \"{}\" .\n", v.replace('"', "\\\"")));
        }
    }
    out.push_str("TX .\n");
    for (target, from, to) in mappings {
        let g = format!("<{}>", crate::store::escape_sparql_iri(target));
        let mut dels: Vec<&String> = from.difference(to).collect();
        let mut adds: Vec<&String> = to.difference(from).collect();
        dels.sort();
        adds.sort();
        for (code, lines) in [('D', dels), ('A', adds)] {
            for t in lines {
                if let Some(c) = comment(code, target, t) {
                    for line in c.lines() {
                        out.push_str(&format!("# {line}\n"));
                    }
                }
                out.push_str(&format!("{code} {t} {g} .\n"));
            }
        }
    }
    out.push_str("TC .\n");
    out
}

pub fn build(input: ReportInput<'_>) -> Result<Proposal, String> {
    let out = input.outcome;
    // Net per graph: a statement both deleted and added in one graph is no
    // line at all.
    let mut by_graph: BTreeMap<String, (HashSet<String>, HashSet<String>)> = BTreeMap::new();
    let mut origin: HashMap<(char, String, String), &Derivation> = HashMap::new();
    for (q, d) in &out.deleted {
        let (g, s) = (graph_of(q), statement(q));
        by_graph.entry(g.clone()).or_default().0.insert(s.clone());
        origin.insert(('D', g, s), d);
    }
    for (q, d) in &out.added {
        let (g, s) = (graph_of(q), statement(q));
        by_graph.entry(g.clone()).or_default().1.insert(s.clone());
        origin.insert(('A', g, s), d);
    }
    let mappings: Vec<(String, &HashSet<String>, &HashSet<String>)> = by_graph
        .iter()
        .map(|(g, (from, to))| (g.clone(), from, to))
        .collect();
    let rule_iri = |d: &Derivation| -> String {
        input
            .rules
            .get(d.rule)
            .map(|r| r.spec.iri.clone())
            .unwrap_or_default()
    };
    let comment = |code: char, graph: &str, st: &str| -> Option<String> {
        let d = origin.get(&(code, graph.to_string(), st.to_string()))?;
        let mut c = String::new();
        let rule = rule_iri(d);
        if !rule.is_empty() {
            c.push_str(&format!("rule=<{rule}> "));
        }
        if !d.trigger.is_empty() {
            c.push_str(&format!("trigger={}", d.trigger));
        }
        if let Some(f) = d.bindings.get("this") {
            c.push_str(&format!(" focus={f}"));
        }
        (!c.trim().is_empty()).then(|| c.trim().to_string())
    };
    let body = patch_text(&[], &mappings, &comment);

    let specs: Vec<RuleSpec> = input.rules.iter().map(|r| r.spec.clone()).collect();
    let digest = rules_digest(&specs);
    let base = &input.base;
    let seq_text = base.sequence.map(|s| s.to_string()).unwrap_or_default();
    let id_hash = sha(&[
        input.dataset_id,
        base.commit.as_deref().unwrap_or(""),
        &seq_text,
        &digest,
        &body,
    ]);
    let id = format!("urn:ots:proposal:{}", &id_hash[..32]);
    let dataset_iri = format!(
        "{}/dataset/{}",
        input.base_url.trim_end_matches('/'),
        input.dataset_id
    );
    let complete = out.exhausted.is_none();
    let complete_text = complete.to_string();
    let mut headers: Vec<(&str, &str)> =
        vec![("id", id.as_str()), ("dataset", dataset_iri.as_str())];
    if let Some(c) = &base.commit {
        headers.push(("base-commit", c.as_str()));
    }
    let seq_header = base.sequence.map(|s| format!("\"{s}\""));
    if let Some(s) = &seq_header {
        headers.push(("base-sequence", s.as_str()));
    }
    let epoch_header = base.epoch.as_ref().map(|e| format!("\"{e}\""));
    if let Some(e) = &epoch_header {
        headers.push(("base-epoch", e.as_str()));
    }
    headers.push(("rules", digest.as_str()));
    let engine = format!("\"{ENGINE}\"");
    headers.push(("engine", engine.as_str()));
    let complete_header = format!("\"{complete_text}\"");
    headers.push(("complete", complete_header.as_str()));
    let patch = patch_text(&headers, &mappings, &comment);
    // A proposal is appliable by construction: the text parses.
    let parsed = crate::rdf_patch::parse(&patch)
        .map_err(|e| format!("the proposal does not parse as RDF Patch ({e}); this is a bug"))?;

    // Actions, in patch order.
    let mut actions: Vec<serde_json::Value> = Vec::new();
    for (g, (from, to)) in &by_graph {
        let mut dels: Vec<&String> = from.difference(to).collect();
        let mut adds: Vec<&String> = to.difference(from).collect();
        dels.sort();
        adds.sort();
        for (code, lines) in [('D', dels), ('A', adds)] {
            for st in lines {
                let Some(d) = origin.get(&(code, g.clone(), st.clone())) else {
                    continue;
                };
                let rule = input.rules.get(d.rule);
                let violation =
                    rule.and_then(|r| r.spec.violation.as_ref())
                        .map(|v| ViolationView {
                            focus_node: d.bindings.get("this").map(|t| {
                                t.trim_start_matches('<').trim_end_matches('>').to_string()
                            }),
                            source_shape: v.source_shape.clone(),
                            path: v.path.clone(),
                            source_constraint: v.source_constraint.clone(),
                            source_constraint_component: v.source_constraint_component.clone(),
                        });
                let action = Action {
                    op: if code == 'A' { "A" } else { "D" },
                    graph: g.clone(),
                    quad: st.clone(),
                    rule: rule.map(|r| r.spec.iri.clone()).unwrap_or_default(),
                    stratum: input.strata.stratum_of(d.rule),
                    confidence: rule
                        .map(|r| r.spec.confidence)
                        .unwrap_or(Confidence::Certain),
                    destructive: rule.is_some_and(|r| r.spec.destructive) || code == 'D',
                    trigger: Trigger {
                        id: d.trigger.clone(),
                        round: d.round,
                        bindings: d.bindings.clone(),
                        premises: d.premises.clone(),
                    },
                    violation,
                    explanation: d.explanation.clone(),
                    kind: d.kind.clone(),
                };
                actions.push(serde_json::to_value(action).map_err(|e| e.to_string())?);
            }
        }
    }
    let actions_total = actions.len();
    let inline: Vec<serde_json::Value> = actions.iter().take(MAX_INLINE_ACTIONS).cloned().collect();

    let count = |o: Origin| input.rules.iter().filter(|r| r.spec.origin == o).count();
    use super::rules::Origin;
    let report_rules: Vec<serde_json::Value> = out
        .report_triggers
        .iter()
        .map(|(r, n)| {
            let spec = &input.rules[*r].spec;
            serde_json::json!({
                "rule": spec.iri,
                "triggers": n,
                "reason": spec.message.clone().unwrap_or_else(|| "ots:Report rule".to_string()),
            })
        })
        .chain(
            input
                .report_only
                .iter()
                .map(|r| serde_json::to_value(r).unwrap_or_default()),
        )
        .collect();
    let strata: Vec<Vec<String>> = input
        .strata
        .main
        .iter()
        .chain(std::iter::once(&input.strata.last).filter(|l| !l.is_empty()))
        .map(|s| s.iter().map(|r| input.rules[*r].spec.iri.clone()).collect())
        .collect();
    let started = &input.started_at;
    let mut prov = format!(
        "@prefix prov: <http://www.w3.org/ns/prov#> .\n@prefix ots: <https://opentriplestore.org/ns#> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n<{id}> a prov:Activity, ots:RepairProposal ;\n  prov:used <{digest}>"
    );
    if let Some(c) = &base.commit {
        prov.push_str(&format!(", <{c}>"));
    }
    prov.push_str(&format!(
        " ;\n  prov:generated <{id}#patch> ;\n  prov:startedAtTime \"{started}\"^^xsd:dateTime"
    ));
    if let Some(a) = input.actor_iri {
        prov.push_str(&format!(" ;\n  prov:wasAssociatedWith <{a}>"));
    }
    prov.push_str(" .\n");

    let report = serde_json::json!({
        "schema": 1,
        "proposal_id": id,
        "dataset_id": input.dataset_id,
        "status": "proposed",
        "partial": input.partial,
        "withheld": { "graphs": input.withheld_graphs, "shapes": input.withheld_shapes },
        "base": base,
        "rules": {
            "graphs": input.rule_graphs,
            "digest": digest,
            "compiled": count(Origin::Compiled),
            "authored": count(Origin::Authored),
            "shacl_af": count(Origin::ShaclAf),
            "heuristic": count(Origin::Heuristic),
            "report_only": report_rules.len(),
            "strata": strata,
            "definitions": specs,
            "turtle": super::vocab::to_turtle(&specs),
            "skipped": input.skipped,
            "dropped": input.dropped,
        },
        "summary": {
            "adds": parsed.adds(),
            "deletes": parsed.deletes(),
            "merges": out.merges.len(),
            "nulls": out.nulls,
            "conflicts": out.conflicts.len(),
            "unexpressible": out.unexpressible.len(),
            "unplaceable": out.unplaceable.len(),
            "rounds": out.rounds,
            "exhausted": out.exhausted,
            "evaluations": out.evaluations,
            "elapsed_ms": input.elapsed_ms,
        },
        "validation": input.validation,
        "actions": inline,
        "actions_total": actions_total,
        "actions_truncated": actions_total > MAX_INLINE_ACTIONS,
        "merges": out.merges,
        "conflicts": out.conflicts,
        "unplaceable": out.unplaceable,
        "unexpressible": out.unexpressible,
        "report_only": report_rules,
        "per_rule": out.per_rule.iter().filter(|s| s.triggers > 0).collect::<Vec<_>>(),
        "prov": prov,
        "patch": patch,
    });
    Ok(Proposal {
        id,
        patch,
        report,
        actions,
    })
}

/// The report minus its volatile fields (`summary.elapsed_ms`,
/// `base.write_generation`, the PROV block with its start time): what two
/// runs over one snapshot agree on byte for byte.
pub fn stable_view(report: &serde_json::Value) -> serde_json::Value {
    let mut v = report.clone();
    if let Some(s) = v.get_mut("summary").and_then(|s| s.as_object_mut()) {
        s.remove("elapsed_ms");
    }
    if let Some(b) = v.get_mut("base").and_then(|b| b.as_object_mut()) {
        b.remove("write_generation");
    }
    if let Some(o) = v.as_object_mut() {
        o.remove("prov");
    }
    v
}

/// Attribute the residual validation results to the report-only entries
/// they belong to (by shape, path and component), filling `triggers`.
pub fn attribute_residual(report_only: &mut [ReportOnly], residual: &ValidationReport) {
    for r in report_only.iter_mut() {
        let n = residual
            .results
            .iter()
            .filter(|v| {
                v.source_shape == r.source_shape
                    && v.path == r.path
                    && super::compile::constraint_component(&v.source_constraint).as_deref()
                        == Some(r.source_constraint_component.as_str())
            })
            .count();
        r.triggers = Some(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The emitter reads no store and mints no id: the same sets and headers
    /// give the same bytes, `D` before `A` per graph, both sorted, and
    /// comment lines are inert for the parser.
    #[test]
    fn patch_text_is_a_function_of_its_input() {
        let from: HashSet<String> = ["<urn:a> <urn:p> <urn:x>", "<urn:b> <urn:p> <urn:y>"]
            .into_iter()
            .map(String::from)
            .collect();
        let to: HashSet<String> = [
            "<urn:b> <urn:p> <urn:y>",
            "<urn:z> <urn:p> <urn:y>",
            "<urn:c> <urn:p> \"1\"",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let headers = [("id", "urn:ots:proposal:1"), ("engine", "ots-chase/0.1")];
        let mappings = [("urn:g".to_string(), &from, &to)];
        let none = |_: char, _: &str, _: &str| None;
        let text = patch_text(&headers, &mappings, &none);
        assert_eq!(text, patch_text(&headers, &mappings, &none));
        assert_eq!(
            text,
            "H id <urn:ots:proposal:1> .\nH engine \"ots-chase/0.1\" .\nTX .\n\
             D <urn:a> <urn:p> <urn:x> <urn:g> .\n\
             A <urn:c> <urn:p> \"1\" <urn:g> .\n\
             A <urn:z> <urn:p> <urn:y> <urn:g> .\nTC .\n"
        );
        let annotated = patch_text(&headers, &mappings, &|code, g, t| {
            (code == 'A' && t.starts_with("<urn:z>")).then(|| format!("rule=<urn:r> graph=<{g}>"))
        });
        assert!(annotated.contains("# rule=<urn:r> graph=<urn:g>\nA <urn:z>"));
        let p = crate::rdf_patch::parse(&annotated).unwrap();
        assert_eq!((p.adds(), p.deletes()), (2, 1));
        assert_eq!(p.id(), Some("urn:ots:proposal:1"));
    }
}
