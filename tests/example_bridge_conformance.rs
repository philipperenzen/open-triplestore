//! Reference-example conformance oracle: a fictional arch bridge.
//!
//! Encodes a fixture → constraint → expected-outcome matrix against the fixtures in
//! `tests/fixtures/example-bridge/`, covering SHACL Core, SHACL-SPARQL (with
//! `sh:prefixes`, aggregates and GeoSPARQL functions) and SHACL Advanced Features
//! (`sh:SPARQLFunction`, `sh:SPARQLTarget`, `sh:SPARQLRule`, `sh:expression`), plus a
//! complex property path, an inline qualified value shape and a GML geometry.
//! Every case in `pass/` must conform and every case in `fail/` must be reported.
//!
//! Convention mirrors tests/shacl_conformance.rs: shapes → `urn:shapes`, data → `urn:data`,
//! then `validate(store, "urn:shapes", &["urn:data"])`.

use open_triplestore::shacl::report::{Severity, ValidationReport};
use open_triplestore::shacl::{infer, validate};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;

const VOCAB: &str = include_str!("fixtures/example-bridge/vocab.ttl");
const SHAPES_CORE: &str = include_str!("fixtures/example-bridge/shapes-core.ttl");
const SHAPES_SPARQL: &str = include_str!("fixtures/example-bridge/shapes-sparql.ttl");
const SHAPES_AF: &str = include_str!("fixtures/example-bridge/shapes-af.ttl");

// ── harness ──────────────────────────────────────────────────────────────────

/// Build a store: `vocab` + each `shapes` file into `urn:shapes`, `data` into `urn:data`,
/// then validate.
fn validate_case(shapes: &[&str], data: &str) -> ValidationReport {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(VOCAB, RdfFormat::Turtle, Some("urn:shapes"))
        .unwrap();
    for s in shapes {
        store
            .load_str(s, RdfFormat::Turtle, Some("urn:shapes"))
            .unwrap();
    }
    store
        .load_str(data, RdfFormat::Turtle, Some("urn:data"))
        .unwrap();
    validate(&store, "urn:shapes", &["urn:data".to_string()]).unwrap()
}

fn focus_violations(r: &ValidationReport, suffix: &str) -> usize {
    r.results
        .iter()
        .filter(|v| v.focus_node.contains(suffix))
        .count()
}

fn has_constraint(r: &ValidationReport, needle: &str) -> bool {
    r.results
        .iter()
        .any(|v| v.source_constraint.contains(needle))
}

fn has_severity(r: &ValidationReport, sev: Severity) -> bool {
    r.results.iter().any(|v| v.severity == sev)
}

// ═══════════════════════════════════════════════════════════════════════════
// SHACL Core
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn pass_arch_north_guid_conforms() {
    let r = validate_case(
        &[SHAPES_CORE],
        include_str!("fixtures/example-bridge/pass/arch-north-guid.ttl"),
    );
    assert_eq!(
        focus_violations(&r, "Arch-North"),
        0,
        "valid 22-char GUID conforms: {:?}",
        r.results
    );
}

#[test]
fn fail_bad_bridge_core_violations() {
    let r = validate_case(
        &[SHAPES_CORE],
        include_str!("fixtures/example-bridge/fail/bad-bridge.ttl"),
    );
    assert!(
        focus_violations(&r, "BadBridge") >= 4,
        "expected ≥4 violations, got {:?}",
        r.results
    );
    assert!(
        has_constraint(&r, "datatype"),
        "datatype (gYear/boolean) violation"
    );
    assert!(
        has_constraint(&r, "minInclusive"),
        "minInclusive(1) violation"
    );
    assert!(
        has_constraint(&r, "minCount"),
        "missing geometry / clearance minCount violation"
    );
}

#[test]
fn fail_arch_bad_guid_pattern() {
    let r = validate_case(
        &[SHAPES_CORE],
        include_str!("fixtures/example-bridge/fail/arch-bad-guid.ttl"),
    );
    assert_eq!(
        focus_violations(&r, "Arch-Bad"),
        1,
        "exactly one pattern violation: {:?}",
        r.results
    );
    assert!(has_constraint(&r, "pattern"));
}

#[test]
fn fail_label_unique_lang() {
    let r = validate_case(
        &[SHAPES_CORE],
        include_str!("fixtures/example-bridge/fail/label-dup.ttl"),
    );
    assert!(
        focus_violations(&r, "ExampleBridge") >= 1,
        "uniqueLang violation: {:?}",
        r.results
    );
    assert!(has_constraint(&r, "uniqueLang"));
}

#[test]
fn fail_culvert_qualified_min_count() {
    let r = validate_case(
        &[SHAPES_CORE],
        include_str!("fixtures/example-bridge/fail/culvert-as-bridge.ttl"),
    );
    assert!(
        focus_violations(&r, "CulvertAsBridge") >= 1,
        "qualifiedMinCount violation: {:?}",
        r.results
    );
    assert!(has_constraint(&r, "qualifiedMinCount") || has_constraint(&r, "qualified"));
}

// ═══════════════════════════════════════════════════════════════════════════
// SHACL-SPARQL — every constraint reaches its prefixes through `sh:prefixes`.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn fail_arch_south_ifc_requires_guid() {
    let r = validate_case(
        &[SHAPES_SPARQL],
        include_str!("fixtures/example-bridge/fail/arch-south-no-guid.ttl"),
    );
    assert!(
        focus_violations(&r, "Arch-South") >= 1,
        "IFC⇒ifcGuid SPARQL violation: {:?}",
        r.results
    );
}

#[test]
fn fail_movable_without_operating_mechanism() {
    let r = validate_case(
        &[SHAPES_SPARQL],
        include_str!("fixtures/example-bridge/fail/movable-no-mechanism.ttl"),
    );
    assert!(
        focus_violations(&r, "SwingBridge") >= 1,
        "cross-property SPARQL violation: {:?}",
        r.results
    );
}

#[test]
fn fail_span_sum_mismatch_warning() {
    let r = validate_case(
        &[SHAPES_SPARQL],
        include_str!("fixtures/example-bridge/fail/span-sum-mismatch.ttl"),
    );
    assert!(
        focus_violations(&r, "SpanBridge") >= 1,
        "aggregate SPARQL result: {:?}",
        r.results
    );
    assert!(
        has_severity(&r, Severity::Warning),
        "span-sum mismatch is a Warning"
    );
}

#[test]
fn fail_component_off_alignment_geosparql() {
    let r = validate_case(
        &[SHAPES_SPARQL],
        include_str!("fixtures/example-bridge/fail/component-off-alignment.ttl"),
    );
    assert!(
        focus_violations(&r, "OffBridge") >= 1,
        "geof:distance >25m SPARQL violation: {:?}",
        r.results
    );
}

#[test]
fn fail_arches_too_close_geosparql() {
    let r = validate_case(
        &[SHAPES_SPARQL],
        include_str!("fixtures/example-bridge/fail/arches-too-close.ttl"),
    );
    assert!(
        focus_violations(&r, "DoubleBridge") >= 1,
        "geof:distance <10m SPARQL violation: {:?}",
        r.results
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// SHACL-AF — node expression, SPARQL function, SPARQL target + rule.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn fail_clearance_expression() {
    // Both files: shapes-af.ttl's own header says its rule, target and
    // function bodies use the `ex:prefixes` declaration that lives in
    // shapes-sparql.ttl, so the two are always loaded together.
    let r = validate_case(
        &[SHAPES_SPARQL, SHAPES_AF],
        include_str!("fixtures/example-bridge/fail/clearance-too-low.ttl"),
    );
    assert!(
        focus_violations(&r, "LowBridge") >= 1,
        "sh:expression minExclusive violation: {:?}",
        r.results
    );
}

/// The user-defined sh:SPARQLFunction ex:distanceMetres is callable from SPARQL and
/// returns the same value as the raw geof:distance it wraps.
#[test]
fn sparql_function_distance_metres_callable() {
    let store = TripleStore::in_memory().unwrap();
    // shapes-sparql provides ex:prefixes; shapes-af defines ex:distanceMetres.
    store
        .load_str(SHAPES_SPARQL, RdfFormat::Turtle, Some("urn:shapes"))
        .unwrap();
    store
        .load_str(SHAPES_AF, RdfFormat::Turtle, Some("urn:shapes"))
        .unwrap();
    let q = r#"
        PREFIX ex:   <https://example.org/shape/def#>
        PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
        PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
        PREFIX uom:  <http://www.opengis.net/def/uom/OGC/1.0/>
        SELECT (ex:distanceMetres("POINT(0 0)"^^geo:wktLiteral, "POINT(3 4)"^^geo:wktLiteral) AS ?d)
               (geof:distance("POINT(0 0)"^^geo:wktLiteral, "POINT(3 4)"^^geo:wktLiteral, uom:metre) AS ?ref)
        WHERE {}
    "#;
    let (mut d, mut r) = (None, None);
    if let Ok(oxigraph::sparql::QueryResults::Solutions(sols)) = store.query(q) {
        for sol in sols.flatten() {
            d = sol.get("d").map(|t| t.to_string());
            r = sol.get("ref").map(|t| t.to_string());
        }
    }
    let d = d.unwrap_or_default();
    let r = r.unwrap_or_default();
    assert!(
        d.contains('5'),
        "ex:distanceMetres should return 5, got {:?}",
        d
    );
    assert_eq!(
        d.split('"').nth(1),
        r.split('"').nth(1),
        "ex:distanceMetres must equal geof:distance (d={d:?}, ref={r:?})"
    );
}

/// The inspection-priority rule fires on a 5/6 condition (positive) and not on ≤4
/// (negative).
fn infer_priority(data: &str) -> bool {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(VOCAB, RdfFormat::Turtle, Some("urn:shapes"))
        .unwrap();
    store
        .load_str(SHAPES_SPARQL, RdfFormat::Turtle, Some("urn:shapes"))
        .unwrap();
    store
        .load_str(SHAPES_AF, RdfFormat::Turtle, Some("urn:shapes"))
        .unwrap();
    // Rule WHERE/CONSTRUCT run against the default graph, so load data there.
    store.load_str(data, RdfFormat::Turtle, None).unwrap();
    infer(&store, "urn:shapes", &[]).unwrap();
    matches!(
        store.query("ASK { ?b <https://example.org/def/inspectionPriority> \"high\" }"),
        Ok(oxigraph::sparql::QueryResults::Boolean(true))
    )
}

#[test]
fn rule_fires_on_poor_condition() {
    assert!(
        infer_priority(include_str!(
            "fixtures/example-bridge/pass/condition-poor.ttl"
        )),
        "a cs5 part must infer def:inspectionPriority high"
    );
}

/// The negative half of the rule oracle: a regression guard against over-firing.
#[test]
fn rule_does_not_fire_on_fair_condition() {
    assert!(
        !infer_priority(include_str!(
            "fixtures/example-bridge/pass/condition-fair.ttl"
        )),
        "a cs3 part must NOT infer a priority"
    );
}
