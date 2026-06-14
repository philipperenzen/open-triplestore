//! Write-gating: before a write to a named graph lands, evaluate every
//! `gate_writes` pipeline whose scope covers that graph against the *incoming*
//! data (in a throwaway store) and reject the write if any fails its severity
//! threshold. This generalises the legacy per-dataset `shacl_on_write` gate to
//! reusable, composable pipelines.

use std::collections::BTreeSet;

use oxigraph::io::RdfFormat;
use oxigraph::model::{GraphName, NamedNode, Quad};

use crate::auth::db::AuthDb;
use crate::shacl::report::ValidationReport;
use crate::store::TripleStore;

use super::bindings;
use super::models::{SeverityThreshold, TargetKind, ValidationPipeline};
// Only the test `pipe()` builder constructs these write-target fields.
#[cfg(test)]
use super::models::{ResultsTarget, WriteTarget};
use super::store::ShaclStudioStore;

/// Returns `Err(report)` with the first failing gate's report when the incoming
/// data would violate a gating pipeline **or** a validation-layer binding that
/// applies to this write; `Ok(())` otherwise.
///
/// Two independent gate sources are evaluated:
/// 1. `gate_writes` pipelines whose scope covers the graph (at each pipeline's
///    own severity threshold);
/// 2. validation-layer bindings — shapes attached directly to the written graph
///    or to its owning dataset. A binding gates on its own (no pipeline needed)
///    at the default `Violation` threshold, so graph-attached shapes travel
///    with the graph and are enforced wherever it is mounted.
pub fn check_write_gates(
    main_store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    graph_iri: &str,
    data: &str,
    format: RdfFormat,
) -> Result<(), ValidationReport> {
    // The legacy per-dataset `shacl_on_write` gate is handled separately by
    // `validate_on_write` on this path, so it is excluded here.
    let gates = discover_gates(main_store, auth_db, studio, base_url, graph_iri, false);
    if gates.is_empty() {
        return Ok(());
    }
    let needed_graphs = needed_shape_graphs(studio, &gates);
    if needed_graphs.is_empty() {
        return Ok(());
    }

    // Build a temp store: incoming data + the shapes (copied from the live store).
    let temp = match TripleStore::in_memory() {
        Ok(t) => t,
        Err(_) => return Ok(()), // never block a write on our own infra hiccup
    };
    if temp.load_str(data, format, Some(graph_iri)).is_err() {
        // Malformed data — let the normal write path surface the parse error.
        return Ok(());
    }
    copy_shape_graphs(main_store, &temp, &needed_graphs);

    evaluate_gates(&temp, studio, &gates, graph_iri)
}

/// Cheap pre-check for bulk import: does *any* gate apply to writes into
/// `graph_iri`? Considers `gate_writes` pipelines, validation-layer bindings
/// (graph- and dataset-level) and the owning dataset's legacy `shacl_on_write`
/// shapes graph. Metadata lookups only — no quad scans, no temp store — so
/// large imports with no gates configured (the common case) pay near-nothing.
pub fn import_gates_apply(
    main_store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    graph_iri: &str,
) -> bool {
    !discover_gates(main_store, auth_db, studio, base_url, graph_iri, true).is_empty()
}

/// Quad-based write gate for bulk import: validates `quads` (re-homed into
/// `graph_iri` in a throwaway store) against every gate that applies —
/// `gate_writes` pipelines, validation-layer bindings, and the owning dataset's
/// legacy `shacl_on_write` shapes graph (which Graph Store writes enforce in
/// `validate_on_write` but bulk import must enforce itself). `Err` carries the
/// first failing gate's report.
pub fn check_import_gates(
    main_store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    graph_iri: &str,
    quads: &[Quad],
) -> Result<(), ValidationReport> {
    let gates = discover_gates(main_store, auth_db, studio, base_url, graph_iri, true);
    if gates.is_empty() {
        return Ok(());
    }
    let needed_graphs = needed_shape_graphs(studio, &gates);
    if needed_graphs.is_empty() {
        return Ok(());
    }

    let temp = match TripleStore::in_memory() {
        Ok(t) => t,
        Err(_) => return Ok(()), // never block a write on our own infra hiccup
    };
    let graph = match NamedNode::new(graph_iri) {
        Ok(g) => GraphName::NamedNode(g),
        Err(_) => return Ok(()), // unaddressable graph — nothing to gate against
    };
    // Insert the parsed quads directly (no serialise/re-parse round trip, which
    // would also relabel blank nodes), re-homed under the target graph.
    let rehomed: Vec<Quad> = quads
        .iter()
        .map(|q| {
            Quad::new(
                q.subject.clone(),
                q.predicate.clone(),
                q.object.clone(),
                graph.clone(),
            )
        })
        .collect();
    if temp
        .bulk_insert_quads(rehomed, &[graph_iri.to_string()])
        .is_err()
    {
        return Ok(());
    }
    copy_shape_graphs(main_store, &temp, &needed_graphs);

    evaluate_gates(&temp, studio, &gates, graph_iri)
}

/// Compact human-readable summary of a failed validation report, for error
/// messages with no structured-report channel (bulk-import rejection).
pub fn summarize_report(report: &ValidationReport, max_results: usize) -> String {
    let mut parts: Vec<String> = report
        .results
        .iter()
        .take(max_results)
        .map(|r| {
            let mut s = format!("{:?} at <{}>", r.severity, r.focus_node);
            if let Some(p) = &r.path {
                s.push_str(&format!(" path <{p}>"));
            }
            if !r.message.is_empty() {
                s.push_str(&format!(": {}", r.message));
            }
            s
        })
        .collect();
    if report.results_count > max_results {
        parts.push(format!("… and {} more", report.results_count - max_results));
    }
    format!(
        "{} validation result(s) — {}",
        report.results_count,
        parts.join("; ")
    )
}

/// Everything that gates a write to one graph: covering `gate_writes`
/// pipelines, validation-layer binding shape graphs, and (import path only)
/// the owning dataset's legacy `shacl_on_write` shapes graph.
struct GateSet {
    pipelines: Vec<ValidationPipeline>,
    binding_graphs: BTreeSet<String>,
    legacy_shapes_graph: Option<String>,
}

impl GateSet {
    fn is_empty(&self) -> bool {
        self.pipelines.is_empty()
            && self.binding_graphs.is_empty()
            && self.legacy_shapes_graph.is_none()
    }
}

fn discover_gates(
    main_store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    graph_iri: &str,
    include_legacy_dataset_gate: bool,
) -> GateSet {
    // Which dataset (if any) owns the graph being written — needed both for
    // pipelines scoped by dataset and for dataset-level bindings.
    let owning_dataset = auth_db.find_dataset_by_graph_iri(graph_iri).ok().flatten();

    // (a) Gating pipelines whose scope covers this graph.
    let pipelines: Vec<ValidationPipeline> = studio
        .list_gating_pipelines()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| {
            pipeline_covers_graph(p, graph_iri, owning_dataset.as_ref().map(|d| d.id.as_str()))
        })
        .collect();

    // (b) Bindings that apply to this write: shapes on the written graph itself
    // plus dataset-level shapes on its owner.
    let mut binding_graphs: BTreeSet<String> = bindings::bindings_for_target(main_store, graph_iri)
        .into_iter()
        .collect();
    if let Some(ds) = &owning_dataset {
        let ds_iri = bindings::dataset_target_iri(base_url, &ds.id);
        binding_graphs.extend(bindings::bindings_for_target(main_store, &ds_iri));
    }

    // (c) Legacy per-dataset gate (`shacl_on_write` + `shapes_graph_iri`).
    let legacy_shapes_graph = if include_legacy_dataset_gate {
        owning_dataset
            .as_ref()
            .filter(|ds| ds.shacl_on_write)
            .and_then(|ds| ds.shapes_graph_iri.clone())
            .filter(|iri| !iri.is_empty())
    } else {
        None
    };

    GateSet {
        pipelines,
        binding_graphs,
        legacy_shapes_graph,
    }
}

/// Union of shape-graph graphs needed by every gate source, resolved once.
fn needed_shape_graphs(studio: &ShaclStudioStore, gates: &GateSet) -> BTreeSet<String> {
    let mut needed: BTreeSet<String> = gates.binding_graphs.clone();
    for p in &gates.pipelines {
        for set_id in &p.shape_graph_ids {
            if let Ok(Some(set)) = studio.get_shape_graph(set_id) {
                needed.insert(set.graph_iri);
            }
        }
    }
    if let Some(g) = &gates.legacy_shapes_graph {
        needed.insert(g.clone());
    }
    needed
}

/// Copy each shape graph from the live store into the throwaway store.
fn copy_shape_graphs(main_store: &TripleStore, temp: &TripleStore, graphs: &BTreeSet<String>) {
    for g in graphs {
        if let Ok(bytes) = main_store.dump(RdfFormat::Turtle, Some(g)) {
            if let Ok(ttl) = String::from_utf8(bytes) {
                let _ = temp.load_str(&ttl, RdfFormat::Turtle, Some(g));
            }
        }
    }
}

/// Run every gate source against the prepared temp store. Returns the first
/// failing gate's report.
fn evaluate_gates(
    temp: &TripleStore,
    studio: &ShaclStudioStore,
    gates: &GateSet,
    graph_iri: &str,
) -> Result<(), ValidationReport> {
    let data_graphs = [graph_iri.to_string()];

    // Pipeline gates (each at its own severity threshold). Inference is never
    // run here — gating must not mutate any store.
    for p in &gates.pipelines {
        let shape_graphs: Vec<String> = p
            .shape_graph_ids
            .iter()
            .filter_map(|id| {
                studio
                    .get_shape_graph(id)
                    .ok()
                    .flatten()
                    .map(|s| s.graph_iri)
            })
            .collect();
        if shape_graphs.is_empty() {
            continue;
        }
        match super::run::run_validation(
            temp,
            &shape_graphs,
            &data_graphs,
            p.severity_threshold,
            false,
        ) {
            Ok(outcome) if !outcome.passes => return Err(outcome.report),
            _ => {}
        }
    }

    // Binding gates — enforced at the default Violation threshold.
    if !gates.binding_graphs.is_empty() {
        let shape_graphs: Vec<String> = gates.binding_graphs.iter().cloned().collect();
        match super::run::run_validation(
            temp,
            &shape_graphs,
            &data_graphs,
            SeverityThreshold::Violation,
            false,
        ) {
            Ok(outcome) if !outcome.passes => return Err(outcome.report),
            _ => {}
        }
    }

    // Legacy per-dataset gate: mirrors `validate_on_write` — a direct engine
    // run against the dataset's configured shapes graph, failing on
    // `!conforms`.
    if let Some(shapes_graph) = &gates.legacy_shapes_graph {
        if let Ok(report) = crate::shacl::validate(temp, shapes_graph, &data_graphs) {
            if !report.conforms {
                return Err(report);
            }
        }
    }

    Ok(())
}

fn pipeline_covers_graph(
    p: &ValidationPipeline,
    graph_iri: &str,
    dataset_id: Option<&str>,
) -> bool {
    // Explicit graph coverage — legacy `graph_iris` or a `Graph` target.
    if p.graph_iris.iter().any(|g| g == graph_iri) {
        return true;
    }
    if p.targets
        .iter()
        .any(|t| t.kind == TargetKind::Graph && t.id == graph_iri)
    {
        return true;
    }
    // Dataset coverage — a `Dataset` target always covers its graphs; the legacy
    // `dataset_ids` only do so when no explicit `graph_iris` narrow the scope
    // (preserving the historical "empty graph_iris = all dataset graphs").
    if let Some(ds) = dataset_id {
        if p.targets
            .iter()
            .any(|t| t.kind == TargetKind::Dataset && t.id == ds)
        {
            return true;
        }
        if p.graph_iris.is_empty() && p.dataset_ids.iter().any(|d| d == ds) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::models::{OwnerType, Visibility};
    use crate::shacl_studio::models::ValidationTarget;

    fn pipe(
        targets: Vec<ValidationTarget>,
        dataset_ids: Vec<&str>,
        graph_iris: Vec<&str>,
    ) -> ValidationPipeline {
        ValidationPipeline {
            id: "p".into(),
            name: "p".into(),
            description: None,
            owner_type: OwnerType::User,
            owner_id: "u".into(),
            visibility: Visibility::Private,
            targets,
            dataset_ids: dataset_ids.into_iter().map(String::from).collect(),
            graph_iris: graph_iris.into_iter().map(String::from).collect(),
            target_classes: vec![],
            shape_graph_ids: vec![],
            severity_threshold: SeverityThreshold::Violation,
            run_inference: false,
            max_results: None,
            trigger_on_write: false,
            schedule_cron: None,
            gate_writes: true,
            retention: 50,
            inferred_target: WriteTarget::InPlace,
            inferred_target_graph: None,
            results_target: ResultsTarget::None,
            results_target_graph: None,
            last_run_at: None,
            last_conforms: None,
            created_by: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn graph_target(iri: &str) -> ValidationTarget {
        ValidationTarget {
            kind: TargetKind::Graph,
            id: iri.into(),
        }
    }
    fn dataset_target(id: &str) -> ValidationTarget {
        ValidationTarget {
            kind: TargetKind::Dataset,
            id: id.into(),
        }
    }

    #[test]
    fn legacy_graph_and_dataset_scope_still_covers() {
        let p = pipe(vec![], vec!["ds1"], vec![]);
        assert!(pipeline_covers_graph(&p, "urn:g", Some("ds1")));
        assert!(!pipeline_covers_graph(&p, "urn:g", Some("other")));

        let p = pipe(vec![], vec!["ds1"], vec!["urn:only"]);
        // Explicit graph narrows scope: the dataset no longer blanket-covers.
        assert!(pipeline_covers_graph(&p, "urn:only", Some("ds1")));
        assert!(!pipeline_covers_graph(&p, "urn:other", Some("ds1")));
    }

    #[test]
    fn graph_target_covers_that_graph() {
        let p = pipe(vec![graph_target("urn:g")], vec![], vec![]);
        assert!(pipeline_covers_graph(&p, "urn:g", None));
        assert!(!pipeline_covers_graph(&p, "urn:h", None));
    }

    #[test]
    fn dataset_target_covers_its_graphs() {
        let p = pipe(vec![dataset_target("ds1")], vec![], vec![]);
        assert!(pipeline_covers_graph(&p, "urn:any", Some("ds1")));
        assert!(!pipeline_covers_graph(&p, "urn:any", Some("ds2")));
        assert!(!pipeline_covers_graph(&p, "urn:any", None));
    }

    // ─── Import gates (quad-based bulk path) ──────────────────────────────────

    use crate::shacl::report::{Severity, ValidationResult};
    use crate::shacl_studio::models::{ShapeGraph, ShapeSource};
    use oxigraph::model::{Literal, Term};

    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    const DATA_GRAPH: &str = "urn:data:g1";
    const SHAPES_GRAPH: &str = "urn:shapes:person";
    const SHAPES_TTL: &str = r#"
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix ex: <http://example.org/> .
        ex:PersonShape a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:property [ sh:path ex:name ; sh:minCount 1 ] .
    "#;

    fn person_quads(with_name: bool) -> Vec<Quad> {
        let p = NamedNode::new("http://example.org/p1").unwrap();
        let mut quads = vec![Quad::new(
            p.clone(),
            NamedNode::new(RDF_TYPE).unwrap(),
            NamedNode::new("http://example.org/Person").unwrap(),
            GraphName::DefaultGraph,
        )];
        if with_name {
            quads.push(Quad::new(
                p,
                NamedNode::new("http://example.org/name").unwrap(),
                Term::Literal(Literal::new_simple_literal("Ada")),
                GraphName::DefaultGraph,
            ));
        }
        quads
    }

    fn studio_with_shapes(store: &TripleStore, auth: &AuthDb) -> (ShaclStudioStore, ShapeGraph) {
        let studio = ShaclStudioStore::new(auth.pool());
        let set = studio
            .create_shape_graph(
                "person",
                None,
                OwnerType::User,
                "u1",
                Visibility::Private,
                SHAPES_GRAPH,
                &[],
                ShapeSource::Manual,
                None,
            )
            .unwrap();
        store
            .load_str(SHAPES_TTL, RdfFormat::Turtle, Some(SHAPES_GRAPH))
            .unwrap();
        (studio, set)
    }

    #[test]
    fn import_gates_via_binding_reject_violations_and_pass_conforming() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let (studio, set) = studio_with_shapes(&store, &auth);
        let base = "http://x";

        // No binding yet: nothing applies, nothing is checked.
        assert!(!import_gates_apply(
            &store, &auth, &studio, base, DATA_GRAPH
        ));
        check_import_gates(
            &store,
            &auth,
            &studio,
            base,
            DATA_GRAPH,
            &person_quads(false),
        )
        .expect("no gates → no rejection");

        bindings::add_binding(&store, DATA_GRAPH, &set.graph_iri).unwrap();
        assert!(import_gates_apply(&store, &auth, &studio, base, DATA_GRAPH));

        // Missing ex:name violates sh:minCount 1 → rejected with a report.
        let report = check_import_gates(
            &store,
            &auth,
            &studio,
            base,
            DATA_GRAPH,
            &person_quads(false),
        )
        .unwrap_err();
        assert!(!report.conforms);
        let summary = summarize_report(&report, 5);
        assert!(
            summary.contains("http://example.org/p1"),
            "summary names the focus node: {summary}"
        );

        // Conforming data passes.
        check_import_gates(
            &store,
            &auth,
            &studio,
            base,
            DATA_GRAPH,
            &person_quads(true),
        )
        .expect("conforming data must pass the gate");
    }

    #[test]
    fn import_gates_honor_gating_pipeline() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let (studio, set) = studio_with_shapes(&store, &auth);
        let base = "http://x";

        let mut p = pipe(vec![graph_target(DATA_GRAPH)], vec![], vec![]);
        p.shape_graph_ids = vec![set.id.clone()];
        studio.insert_pipeline(&p).unwrap();

        assert!(import_gates_apply(&store, &auth, &studio, base, DATA_GRAPH));
        assert!(
            !import_gates_apply(&store, &auth, &studio, base, "urn:data:uncovered"),
            "pipeline scope must not leak to other graphs"
        );

        let report = check_import_gates(
            &store,
            &auth,
            &studio,
            base,
            DATA_GRAPH,
            &person_quads(false),
        )
        .unwrap_err();
        assert!(!report.conforms);
        check_import_gates(
            &store,
            &auth,
            &studio,
            base,
            DATA_GRAPH,
            &person_quads(true),
        )
        .expect("conforming data must pass the pipeline gate");
    }

    #[test]
    fn import_gates_honor_legacy_shacl_on_write() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let (studio, _set) = studio_with_shapes(&store, &auth);
        let base = "http://x";

        auth.create_dataset(
            "d1",
            "D1",
            None,
            OwnerType::User,
            "u1",
            Visibility::Private,
            None,
        )
        .unwrap();
        auth.add_dataset_graph("d1", DATA_GRAPH).unwrap();

        // shacl_on_write off → no gate.
        assert!(!import_gates_apply(
            &store, &auth, &studio, base, DATA_GRAPH
        ));

        auth.update_dataset_shacl("d1", true, Some(SHAPES_GRAPH))
            .unwrap();
        assert!(import_gates_apply(&store, &auth, &studio, base, DATA_GRAPH));

        let report = check_import_gates(
            &store,
            &auth,
            &studio,
            base,
            DATA_GRAPH,
            &person_quads(false),
        )
        .unwrap_err();
        assert!(!report.conforms);
        check_import_gates(
            &store,
            &auth,
            &studio,
            base,
            DATA_GRAPH,
            &person_quads(true),
        )
        .expect("conforming data must pass the legacy dataset gate");

        // Graph Store path (`check_write_gates`) intentionally excludes the
        // legacy gate — `validate_on_write` runs it separately there.
        check_write_gates(
            &store,
            &auth,
            &studio,
            base,
            DATA_GRAPH,
            "<http://example.org/p1> a <http://example.org/Person> .",
            RdfFormat::Turtle,
        )
        .expect("legacy gate must not double-fire on the GSP path");
    }

    #[test]
    fn summarize_report_truncates() {
        let mk = |n: usize| ValidationResult {
            severity: Severity::Violation,
            focus_node: format!("http://example.org/f{n}"),
            path: Some("http://example.org/name".to_string()),
            value: None,
            source_shape: "http://example.org/S".to_string(),
            source_constraint: "minCount".to_string(),
            message: format!("missing name {n}"),
        };
        let report = ValidationReport {
            conforms: false,
            results: (0..4).map(mk).collect(),
            results_count: 4,
        };
        let s = summarize_report(&report, 2);
        assert!(s.starts_with("4 validation result(s)"), "{s}");
        assert!(s.contains("f0") && s.contains("f1"), "{s}");
        assert!(!s.contains("f2"), "truncated: {s}");
        assert!(s.contains("and 2 more"), "{s}");
    }
}
