//! Core pipeline execution: validate a data scope against one or more shape
//! sets, merge the reports, and apply the severity threshold. Shared by the
//! manual run endpoint, the scheduler, and the write-gate.

use std::collections::HashSet;

use oxigraph::model::{GraphNameRef, NamedNode, Quad, Term};
use oxigraph::sparql::QueryResults;

use crate::shacl::report::{Severity, ValidationReport};
use crate::store::TripleStore;

use super::models::SeverityThreshold;

const SH: &str = "http://www.w3.org/ns/shacl#";

/// Outcome of running a pipeline's validation: the merged report plus the
/// threshold-based pass/fail and per-severity counts.
pub struct RunOutcome {
    pub report: ValidationReport,
    /// True when no result meets or exceeds the configured severity threshold.
    pub passes: bool,
    pub violation_count: i64,
    pub warning_count: i64,
    pub info_count: i64,
}

/// Validate `data_graphs` against every graph in `shape_graph_graphs`, merging
/// the per-set reports into one. When `run_inference` is set, SHACL-AF rules
/// from each set are materialised into `store` first (callers must only enable
/// this against the live store for manual/scheduled runs — never for the
/// write-gate, which validates a throwaway temp store).
pub fn run_validation(
    store: &TripleStore,
    shape_graph_graphs: &[String],
    data_graphs: &[String],
    threshold: SeverityThreshold,
    run_inference: bool,
) -> Result<RunOutcome, String> {
    let mut results = Vec::new();
    for shapes_graph in shape_graph_graphs {
        if run_inference {
            let _ = crate::shacl::infer(store, shapes_graph, data_graphs)?;
        }
        let report = crate::shacl::validate(store, shapes_graph, data_graphs)?;
        results.extend(report.results);
    }
    Ok(summarise(results, threshold))
}

/// Snapshot the union of all quads currently in `data_graphs` (used to diff the
/// store before/after SHACL-AF inference so the newly-derived triples can be
/// routed to a destination graph/version).
fn collect_graph_quads(store: &TripleStore, data_graphs: &[String]) -> HashSet<Quad> {
    let mut set = HashSet::new();
    for g in data_graphs {
        if let Ok(nn) = NamedNode::new(g) {
            if let Ok(quads) = store.quads_for_graph(GraphNameRef::NamedNode(nn.as_ref())) {
                set.extend(quads);
            }
        }
    }
    set
}

/// Like [`run_validation`], but additionally returns the triples that SHACL-AF
/// inference materialised this run (empty when `run_inference` is false). The
/// delta is computed by diffing the data graphs before/after — `crate::shacl::infer`
/// materialises in place and only returns a count, so this recovers *which*
/// triples it added. The returned quads still carry their source graph name;
/// callers re-home them when writing to a dedicated graph.
pub fn run_validation_capturing(
    store: &TripleStore,
    shape_graph_graphs: &[String],
    data_graphs: &[String],
    threshold: SeverityThreshold,
    run_inference: bool,
) -> Result<(RunOutcome, Vec<Quad>), String> {
    let before = if run_inference { collect_graph_quads(store, data_graphs) } else { HashSet::new() };
    let outcome = run_validation(store, shape_graph_graphs, data_graphs, threshold, run_inference)?;
    let inferred = if run_inference {
        collect_graph_quads(store, data_graphs)
            .into_iter()
            .filter(|q| !before.contains(q))
            .collect()
    } else {
        Vec::new()
    };
    Ok((outcome, inferred))
}

fn severity_rank(s: &Severity) -> u8 {
    match s {
        Severity::Info => 1,
        Severity::Warning => 2,
        Severity::Violation => 3,
    }
}

/// Tally results into a `RunOutcome` and apply the severity threshold. Split
/// out so it can be unit-tested without running the SHACL engine.
fn summarise(results: Vec<crate::shacl::report::ValidationResult>, threshold: SeverityThreshold) -> RunOutcome {
    let mut violation_count = 0i64;
    let mut warning_count = 0i64;
    let mut info_count = 0i64;
    let mut worst = 0u8;
    for r in &results {
        let rank = severity_rank(&r.severity);
        worst = worst.max(rank);
        match r.severity {
            Severity::Violation => violation_count += 1,
            Severity::Warning => warning_count += 1,
            Severity::Info => info_count += 1,
        }
    }
    let passes = worst < threshold.rank();
    let results_count = results.len();
    RunOutcome {
        report: ValidationReport { conforms: results.is_empty(), results, results_count },
        passes,
        violation_count,
        warning_count,
        info_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shacl::report::ValidationResult;

    fn mk(sev: Severity) -> ValidationResult {
        ValidationResult {
            severity: sev,
            focus_node: "urn:x".into(),
            path: None,
            value: None,
            source_shape: "urn:shape".into(),
            source_constraint: "sh:Test".into(),
            message: "test".into(),
        }
    }

    #[test]
    fn empty_results_always_pass() {
        for t in [SeverityThreshold::Info, SeverityThreshold::Warning, SeverityThreshold::Violation] {
            let o = summarise(vec![], t);
            assert!(o.passes);
            assert!(o.report.conforms);
            assert_eq!(o.report.results_count, 0);
        }
    }

    #[test]
    fn violation_threshold_only_fails_on_violation() {
        let o = summarise(vec![mk(Severity::Info), mk(Severity::Warning)], SeverityThreshold::Violation);
        assert!(o.passes, "warnings + infos pass with threshold=violation");
        let o = summarise(vec![mk(Severity::Violation)], SeverityThreshold::Violation);
        assert!(!o.passes);
    }

    #[test]
    fn warning_threshold_fails_on_warning_or_above() {
        let o = summarise(vec![mk(Severity::Info)], SeverityThreshold::Warning);
        assert!(o.passes, "infos pass with threshold=warning");
        let o = summarise(vec![mk(Severity::Warning)], SeverityThreshold::Warning);
        assert!(!o.passes);
        let o = summarise(vec![mk(Severity::Violation)], SeverityThreshold::Warning);
        assert!(!o.passes);
    }

    #[test]
    fn info_threshold_fails_on_anything() {
        let o = summarise(vec![mk(Severity::Info)], SeverityThreshold::Info);
        assert!(!o.passes);
    }

    #[test]
    fn counts_break_down_by_severity() {
        let o = summarise(
            vec![mk(Severity::Violation), mk(Severity::Violation), mk(Severity::Warning), mk(Severity::Info)],
            SeverityThreshold::Violation,
        );
        assert_eq!(o.violation_count, 2);
        assert_eq!(o.warning_count, 1);
        assert_eq!(o.info_count, 1);
        assert_eq!(o.report.results_count, 4);
        assert!(!o.report.conforms);
        assert!(!o.passes);
    }
}

/// Inspect a shapes graph and return `(target_classes, shape_count)` for the
/// Library's cached facets. Best-effort: a query failure yields empty/zero.
pub fn analyze_shapes_graph(store: &TripleStore, graph_iri: &str) -> (Vec<String>, i64) {
    let targets = select_iris(
        store,
        &format!(
            "PREFIX sh: <{SH}> SELECT DISTINCT ?c WHERE {{ GRAPH <{graph_iri}> {{ ?s sh:targetClass ?c }} }}"
        ),
        "c",
    );
    let count_q = format!(
        "PREFIX sh: <{SH}> SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE {{ GRAPH <{graph_iri}> {{ \
         {{ ?s a sh:NodeShape }} UNION {{ ?s a sh:PropertyShape }} UNION {{ ?s sh:property ?p }} \
         UNION {{ ?s sh:targetClass ?tc }} }} }}"
    );
    let count = scalar_count(store, &count_q);
    (targets, count)
}

/// Run a single-variable SELECT and collect the bound IRIs (named nodes only).
pub fn select_iris(store: &TripleStore, query: &str, var: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(solutions)) = store.query(query) {
        for sol in solutions.flatten() {
            if let Some(Term::NamedNode(nn)) = sol.get(var) {
                out.push(nn.as_str().to_string());
            }
        }
    }
    out
}

/// Run a `SELECT (COUNT(..) AS ?n)` query and return the integer (0 on error).
pub fn scalar_count(store: &TripleStore, query: &str) -> i64 {
    if let Ok(QueryResults::Solutions(mut solutions)) = store.query(query) {
        if let Some(Ok(sol)) = solutions.next() {
            if let Some(Term::Literal(lit)) = sol.get("n") {
                return lit.value().parse::<i64>().unwrap_or(0);
            }
        }
    }
    0
}
