//! Waalbrug reference-example conformance oracle (R0).
//!
//! Encodes the §6 fixture→constraint→expected-outcome matrix from the implementation
//! brief against the fixtures in `tests/fixtures/waalbrug/`. This is the authoritative
//! acceptance gate for the GeoSPARQL + SHACL (Core/SPARQL/AF) work: as each engine gap
//! closes, the corresponding `#[ignore]` is removed.
//!
//! Known gaps blocking cases (see docs/notes/recon.md §8), confirmed empirically by the
//! first R0 run (4 active pass, 8 ignored pending the listed milestone):
//!   G1  — sh:prefixes not injected into SHACL-SPARQL bodies → prefixed queries silently skip (R1)
//!   G2  — complex property paths (sequence/inverse/sh:alternativePath) not parsed from RDF (R2)
//!   G3  — sh:expression node expressions unimplemented (R5)
//!   G5  — geo:gmlLiteral not parsed (WKT-only) (R3)
//!   G10 — inline blank-node sh:qualifiedValueShape not resolved (looked up in the top-level
//!         shape list, where an inline `[ sh:class … ]` never appears) → constraint skipped.
//!         NEW, discovered by this oracle; sh:not/and/or use load_inline_shape but qvs does not.
//!
//! Convention mirrors tests/shacl_conformance.rs: shapes → `urn:shapes`, data → `urn:data`,
//! then `validate(store, "urn:shapes", &["urn:data"])`.

use open_triplestore::shacl::report::{Severity, ValidationReport};
use open_triplestore::shacl::{infer, validate};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;

const VOCAB: &str = include_str!("fixtures/waalbrug/vocab.ttl");
const SHAPES_CORE: &str = include_str!("fixtures/waalbrug/shapes-core.ttl");
const SHAPES_SPARQL: &str = include_str!("fixtures/waalbrug/shapes-sparql.ttl");
const SHAPES_AF: &str = include_str!("fixtures/waalbrug/shapes-af.ttl");

// ── harness ──────────────────────────────────────────────────────────────────

/// Build a store: `vocab` + each `shapes` file into `urn:shapes`, `data` into `urn:data`,
/// then validate. `data_graph` selects whether validation scopes to `urn:data`.
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
// SHACL Core (§4) — expected to work today.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn pass_boog_noord_guid_conforms() {
    let r = validate_case(&[SHAPES_CORE], include_str!("fixtures/waalbrug/pass/boog-noord-guid.ttl"));
    assert_eq!(focus_violations(&r, "Boog-Noord"), 0, "valid 22-char GUID conforms: {:?}", r.results);
}

#[test]
fn fail_foutbrug_core_violations() {
    let r = validate_case(&[SHAPES_CORE], include_str!("fixtures/waalbrug/fail/foutbrug.ttl"));
    assert!(focus_violations(&r, "FoutBrug") >= 4, "expected ≥4 violations, got {:?}", r.results);
    assert!(has_constraint(&r, "datatype"), "datatype (gYear/boolean) violation");
    assert!(has_constraint(&r, "minInclusive"), "minInclusive(1) violation");
    assert!(has_constraint(&r, "minCount"), "missing geometry / clearance minCount violation");
}

#[test]
fn fail_boog_fout_pattern() {
    let r = validate_case(&[SHAPES_CORE], include_str!("fixtures/waalbrug/fail/boog-fout.ttl"));
    assert_eq!(focus_violations(&r, "Boog-Fout"), 1, "exactly one pattern violation: {:?}", r.results);
    assert!(has_constraint(&r, "pattern"));
}

#[test]
fn fail_label_unique_lang() {
    let r = validate_case(&[SHAPES_CORE], include_str!("fixtures/waalbrug/fail/label-dup.ttl"));
    assert!(focus_violations(&r, "Waalbrug") >= 1, "uniqueLang violation: {:?}", r.results);
    assert!(has_constraint(&r, "uniqueLang"));
}

#[test]
#[ignore = "G10: inline blank-node sh:qualifiedValueShape not resolved (looked up in top-level shape list); fix in R2/R3 follow-up"]
fn fail_duiker_qualified_min_count() {
    let r = validate_case(&[SHAPES_CORE], include_str!("fixtures/waalbrug/fail/duiker-als-brug.ttl"));
    assert!(focus_violations(&r, "DuikerAlsBrug") >= 1, "qualifiedMinCount violation: {:?}", r.results);
    assert!(has_constraint(&r, "qualifiedMinCount") || has_constraint(&r, "qualified"));
}

// ═══════════════════════════════════════════════════════════════════════════
// SHACL-SPARQL (§5) — blocked by G1 (sh:prefixes injection) until R1.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn fail_boog_zuid_ifc_requires_guid() {
    let r = validate_case(&[SHAPES_SPARQL], include_str!("fixtures/waalbrug/fail/boog-zuid-no-guid.ttl"));
    assert!(focus_violations(&r, "Boog-Zuid") >= 1, "IFC⇒ifcGuid SPARQL violation: {:?}", r.results);
}

#[test]
fn fail_beweegbaar_without_operating_object() {
    let r = validate_case(&[SHAPES_SPARQL], include_str!("fixtures/waalbrug/fail/beweegbaar-no-bediening.ttl"));
    assert!(focus_violations(&r, "DraaiBrug") >= 1, "cross-property SPARQL violation: {:?}", r.results);
}

#[test]
fn fail_span_sum_mismatch_warning() {
    let r = validate_case(&[SHAPES_SPARQL], include_str!("fixtures/waalbrug/fail/span-sum-mismatch.ttl"));
    assert!(focus_violations(&r, "SpanBrug") >= 1, "aggregate SPARQL result: {:?}", r.results);
    assert!(has_severity(&r, Severity::Warning), "span-sum mismatch is a Warning");
}

#[test]
fn fail_onderdeel_off_trace_geosparql() {
    let r = validate_case(&[SHAPES_SPARQL], include_str!("fixtures/waalbrug/fail/onderdeel-off-trace.ttl"));
    assert!(focus_violations(&r, "OffBrug") >= 1, "geof:distance >25m SPARQL violation: {:?}", r.results);
}

#[test]
fn fail_bogen_too_close_geosparql() {
    let r = validate_case(&[SHAPES_SPARQL], include_str!("fixtures/waalbrug/fail/bogen-too-close.ttl"));
    assert!(focus_violations(&r, "DubbelBrug") >= 1, "geof:distance <10m SPARQL violation: {:?}", r.results);
}

// ═══════════════════════════════════════════════════════════════════════════
// SHACL-AF (§6) — sh:expression (G3) + SPARQLRule/target (G1) until R5/R1.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "G3: sh:expression node expressions unimplemented; unblocked by R5"]
fn fail_doorvaarthoogte_expression() {
    let r = validate_case(&[SHAPES_AF], include_str!("fixtures/waalbrug/fail/doorvaarthoogte-laag.ttl"));
    assert!(focus_violations(&r, "LageBrug") >= 1, "sh:expression minExclusive violation: {:?}", r.results);
}

/// §6.3 rule fires on a 5/6 condition (positive) and not on ≤4 (negative).
/// Rule bodies + custom target use prefixed names → blocked by G1 until R1.
fn infer_priority(data: &str) -> bool {
    let store = TripleStore::in_memory().unwrap();
    store.load_str(VOCAB, RdfFormat::Turtle, Some("urn:shapes")).unwrap();
    store.load_str(SHAPES_SPARQL, RdfFormat::Turtle, Some("urn:shapes")).unwrap();
    store.load_str(SHAPES_AF, RdfFormat::Turtle, Some("urn:shapes")).unwrap();
    // Rule WHERE/CONSTRUCT run against the default graph, so load data there.
    store.load_str(data, RdfFormat::Turtle, None).unwrap();
    infer(&store, "urn:shapes", &[]).unwrap();
    matches!(
        store.query("ASK { ?b <https://data.example.nl/def/inspectieprioriteit> \"hoog\" }"),
        Ok(oxigraph::sparql::QueryResults::Boolean(true))
    )
}

#[test]
fn rule_fires_on_poor_condition() {
    assert!(infer_priority(include_str!("fixtures/waalbrug/pass/conditie-hoog.ttl")),
        "cs5 part must infer da:inspectieprioriteit hoog");
}

// NOTE: currently passes vacuously — G1 blocks the rule from firing at all. Once R1 lands,
// this must keep passing for the right reason (cs3 ≤ 4 ⇒ no inference). Kept active as the
// negative half of the rule oracle and a regression guard against over-firing.
#[test]
fn rule_does_not_fire_on_good_condition() {
    assert!(!infer_priority(include_str!("fixtures/waalbrug/pass/conditie-laag.ttl")),
        "cs3 part must NOT infer a priority");
}
