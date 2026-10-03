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
    // shapes-sparql.ttl. Loading it alone leaves the SPARQL target's query
    // without its prefixes — which used to be silent (the target produced no
    // focus nodes and the shape validated nothing) and is now a load error.
    let r = validate_case(
        &[SHAPES_SPARQL, SHAPES_AF],
        include_str!("fixtures/example-bridge/fail/clearance-too-low.ttl"),
    );
    assert!(
        focus_violations(&r, "LowBridge") >= 1,
        "sh:expression violation (8.50 m is below 9.10 m): {:?}",
        r.results
    );
    assert!(
        r.results
            .iter()
            .any(|x| x.message.contains("at least 9.10 m")),
        "the expression's sh:message is reported: {:?}",
        r.results
    );
}

/// The positive half of the expression oracle: `ex:atLeast` over the clearance
/// path is exactly `{ true }` for a clearance of 9.50 m.
#[test]
fn pass_clearance_expression() {
    let r = validate_case(
        &[SHAPES_SPARQL, SHAPES_AF],
        r#"@prefix def:  <https://example.org/def/> .
           @prefix eb:   <https://example.org/id/example-bridge/> .
           @prefix qudt: <http://qudt.org/schema/qudt/> .
           eb:HighBridge a def:NavigableBridge ;
               def:clearanceHeight [ a qudt:QuantityValue ; qudt:numericValue 9.50 ] ."#,
    );
    assert_eq!(
        focus_violations(&r, "HighBridge"),
        0,
        "9.50 m is at least 9.10 m: {:?}",
        r.results
    );
}

/// The user-defined sh:SPARQLFunction ex:distanceMetres is callable from the shapes
/// graph that declares it and returns the same value as the raw geof:distance it
/// wraps. The points are in RD New (the reference example's CRS), where `uom:metre`
/// is the plane's own metres: 3-4-5 is 5 m. (The unprefixed CRS84 points POINT(0 0)
/// and POINT(3 4) would be a geodesic distance in metres, ~554 km.) A plain query
/// does not see it: a function belongs to its shapes graph's runs.
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
    let a =
        r#""<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(186000 427000)"^^geo:wktLiteral"#;
    let b =
        r#""<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(186003 427004)"^^geo:wktLiteral"#;
    // One row, whose value is ex:distanceMetres, exactly when it equals
    // geof:distance: an unbound function yields no row at all.
    let probe = format!(
        r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix ex: <https://example.org/shape/def#> .
        ex:DistanceProbe a sh:NodeShape ; sh:targetNode ex:probe ;
          sh:sparql [ sh:prefixes ex:prefixes ; sh:select """
            SELECT $this ?value WHERE {{
              BIND(ex:distanceMetres({a}, {b}) AS ?value)
              BIND(geof:distance({a}, {b}, uom:metre) AS ?ref)
              FILTER(?value = ?ref)
            }}""" ] .
        "#
    );
    store
        .load_str(&probe, RdfFormat::Turtle, Some("urn:shapes"))
        .unwrap();
    let r = validate(&store, "urn:shapes", &["urn:data".to_string()]).unwrap();
    let probe_rows: Vec<_> = r
        .results
        .iter()
        .filter(|x| x.focus_node.ends_with("probe"))
        .collect();
    assert_eq!(
        probe_rows.len(),
        1,
        "ex:distanceMetres must equal geof:distance: {:?}",
        r.results
    );
    let d = probe_rows[0].value.clone().unwrap_or_default();
    assert!(
        d.contains('5'),
        "ex:distanceMetres should return 5, got {d:?}"
    );

    let q = format!(
        r#"PREFIX ex: <https://example.org/shape/def#>
        PREFIX geo: <http://www.opengis.net/ont/geosparql#>
        SELECT (ex:distanceMetres({a}, {b}) AS ?d) WHERE {{}}"#
    );
    let leaked = match store.query(&q) {
        Ok(oxigraph::sparql::QueryResults::Solutions(sols)) => {
            sols.flatten().any(|sol| sol.get("d").is_some())
        }
        _ => false,
    };
    assert!(!leaked, "a shapes graph's function reached a plain query");
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
