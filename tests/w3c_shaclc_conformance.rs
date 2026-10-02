//! Runner for the test cases of the SHACL Compact Syntax Community Group
//! report, vendored under `tests/fixtures/w3c-shaclc/valid` (see PROVENANCE.md
//! and LICENSE.md there). The report calls them non-normative; these are
//! development and regression results, not a W3C conformance claim.
//!
//! Each case is a `<name>.shaclc` / `<name>.ttl` pair. A case passes when
//!
//!   1. parsing the `.shaclc` gives a graph isomorphic to the `.ttl` (the
//!      report's criterion), and
//!   2. the serializer writes the `.ttl` graph back as SHACL-C losslessly and
//!      that document parses to an isomorphic graph again (the round trip).
//!
//! A `.shaclc` without `BASE` is parsed with the initial base IRI
//! `urn:x-base:default`, which `empty.ttl` expects; the report leaves the
//! parser's "optional base URI" to the caller.
//!
//! Gap policy (two-way ratchet): every case NOT in `KNOWN_FAILURES` must pass,
//! and every listed case must still fail — so silent regressions *and* silent
//! fixes both turn the suite red, keeping the list honest.

use open_triplestore::shaclc::{parse_document, serialize_graph};
use oxigraph::io::{RdfFormat, RdfParser};
use oxrdf::dataset::CanonicalizationAlgorithm;
use oxrdf::Graph;
use std::path::{Path, PathBuf};

const FIXTURES: &str = "tests/fixtures/w3c-shaclc/valid";
const DEFAULT_BASE: &str = "urn:x-base:default";

/// Cases that currently fail, with the gap they sit behind. Keep sorted.
/// Removing an entry requires the case to actually pass (the ratchet asserts
/// both directions). Keys are file stems.
///
/// Empirical baseline: 32 pass / 0 known-fail / 0 aux skips
const KNOWN_FAILURES: &[(&str, &str)] = &[];

fn cases() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(FIXTURES)
        .expect("vendored SHACL-C cases present")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "shaclc"))
        .collect();
    v.sort();
    v
}

fn turtle_graph(path: &Path) -> Result<Graph, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut g = Graph::new();
    for q in RdfParser::from_format(RdfFormat::Turtle).for_slice(&bytes) {
        g.insert(q.map_err(|e| e.to_string())?.as_ref());
    }
    Ok(g)
}

fn isomorphic(a: &Graph, b: &Graph) -> bool {
    let (mut a, mut b) = (a.clone(), b.clone());
    a.canonicalize(CanonicalizationAlgorithm::Unstable);
    b.canonicalize(CanonicalizationAlgorithm::Unstable);
    a == b
}

fn diff(actual: &Graph, expected: &Graph) -> String {
    let show = |g: &Graph| {
        let mut v: Vec<String> = g.iter().map(|t| t.to_string()).collect();
        v.sort();
        v.join("\n    ")
    };
    format!(
        "\n  actual:\n    {}\n  expected:\n    {}",
        show(actual),
        show(expected)
    )
}

fn run_one(shaclc: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(shaclc).map_err(|e| e.to_string())?;
    let expected = turtle_graph(&shaclc.with_extension("ttl"))?;

    // 1. Parse.
    let parsed = parse_document(&text, Some(DEFAULT_BASE))
        .map_err(|e| format!("parse: {e}"))?
        .graph;
    if !isomorphic(&parsed, &expected) {
        return Err(format!(
            "parse: not isomorphic to the .ttl{}",
            diff(&parsed, &expected)
        ));
    }

    // 2. Round trip through the serializer.
    let written = serialize_graph(&expected, |_| None).map_err(|e| match e {
        open_triplestore::shaclc::SerializeError::Losses(l) => {
            format!("round trip: serializer reports losses: {l:?}")
        }
        other => format!("round trip: {other}"),
    })?;
    let reparsed = parse_document(&written, None)
        .map_err(|e| format!("round trip: re-parse: {e}\n{written}"))?
        .graph;
    if !isomorphic(&reparsed, &expected) {
        return Err(format!(
            "round trip: not isomorphic\n{written}{}",
            diff(&reparsed, &expected)
        ));
    }
    Ok(())
}

#[test]
fn w3c_shaclc_suite() {
    let files = cases();
    assert_eq!(
        files.len(),
        32,
        "the vendored suite holds 32 .shaclc cases (see PROVENANCE.md)"
    );
    let mut pass = 0usize;
    let mut seen_known = 0usize;
    let mut unexpected_failures = Vec::new();
    let mut unexpected_passes = Vec::new();
    for path in &files {
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let known = KNOWN_FAILURES.iter().find(|(k, _)| *k == stem);
        if known.is_some() {
            seen_known += 1;
        }
        match run_one(path) {
            Ok(()) => {
                pass += 1;
                if let Some((k, why)) = known {
                    unexpected_passes.push(format!("{k} (listed as: {why})"));
                }
            }
            Err(reason) => {
                if known.is_none() {
                    unexpected_failures.push(format!("{stem}: {reason}"));
                }
            }
        }
    }
    println!(
        "SHACL-C CG cases: {pass} passed, {} known-fail, {} files",
        KNOWN_FAILURES.len(),
        files.len()
    );
    assert_eq!(
        seen_known,
        KNOWN_FAILURES.len(),
        "every KNOWN_FAILURES key must name a vendored case (stale entries?)"
    );
    assert!(
        unexpected_failures.is_empty(),
        "cases failing that are not in KNOWN_FAILURES:\n  {}",
        unexpected_failures.join("\n  ")
    );
    assert!(
        unexpected_passes.is_empty(),
        "KNOWN_FAILURES entries now pass — remove them to ratchet forward:\n  {}",
        unexpected_passes.join("\n  ")
    );
    // A floor as well as the ratchet: every case passes today.
    assert!(pass >= 32, "only {pass} SHACL-C cases passed (floor 32)");
}

/// The report's own example (§1.2) maps to the Turtle it shows, as the
/// `complex1` case pins; the `shapeClass` variant is `complex2`. This adds the
/// constructs no CG case covers, checked against hand-written Turtle from the
/// production rules: long strings, language tags, `^^` datatypes, decimals and
/// doubles, `@<iri>` references, IRI shape names and `!` on a nested body.
#[test]
fn production_rules_beyond_the_cg_cases() {
    let shaclc = r#"
BASE <http://example.org/extra>
PREFIX ex: <http://example.org/test#>

shape ex:S -> ex:A {
	message="""two
lines"""@en severity=sh:Warning .
	!class=ex:Bad .
	ex:p minInclusive=1.5 maxInclusive=2.0E3 hasValue="x"^^ex:dt @<http://example.org/test#T> .
	^ex:q !{ ex:r [1..1] . } .
}
"#;
    let expected = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix ex: <http://example.org/test#> .
<http://example.org/extra> a owl:Ontology .
ex:S a sh:NodeShape ; sh:targetClass ex:A ;
	sh:message "two\nlines"@en ; sh:severity sh:Warning ;
	sh:not [ sh:class ex:Bad ] ;
	sh:property [ sh:path ex:p ; sh:minInclusive 1.5 ; sh:maxInclusive 2.0E3 ;
		sh:hasValue "x"^^ex:dt ; sh:node ex:T ] ;
	sh:property [ sh:path [ sh:inversePath ex:q ] ;
		sh:not [ sh:node [ sh:property [ sh:path ex:r ; sh:minCount 1 ; sh:maxCount 1 ] ] ] ] .
"#;
    let parsed = parse_document(shaclc, None).expect("parse").graph;
    let mut want = Graph::new();
    for q in RdfParser::from_format(RdfFormat::Turtle).for_slice(expected.as_bytes()) {
        want.insert(q.unwrap().as_ref());
    }
    assert!(isomorphic(&parsed, &want), "{}", diff(&parsed, &want));
    let written = serialize_graph(&parsed, |_| None).expect("lossless");
    let reparsed = parse_document(&written, None).expect("re-parse").graph;
    assert!(isomorphic(&reparsed, &want), "{written}");
}
