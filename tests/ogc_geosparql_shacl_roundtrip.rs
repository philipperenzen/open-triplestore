//! OGC GeoSPARQL 1.1 SHACL validator, run by **our own SHACL engine**
//! (vendored under `tests/fixtures/ogc-geosparql/`, see PROVENANCE.md).
//!
//! Two layers:
//! 1. **OGC's own oracle** — every `Sxx-valid.ttl` example must conform and
//!    every `Sxx-invalid-*.ttl` must not, validated against the OGC GeoSPARQL
//!    1.1 validator shapes. This closes the loop: GeoSPARQL data validated by
//!    GeoSPARQL's own shapes, through this repo's engine.
//! 2. **Reference-example round-trip** — the canonical example-bridge dataset
//!    (`tests/fixtures/example-bridge/`) validates against the OGC GeoSPARQL
//!    1.1 validator shapes.
//!
//! These are development tests, not an OGC compliance test: only the OGC
//! authorises compliance marks for its standards.
//!
//! Same two-way ratchet as the W3C runner: non-listed cases must behave as
//! the OGC oracle says; listed cases must still deviate (so fixes surface).

use open_triplestore::shacl::validate;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use std::path::Path;

const VALIDATOR: &str = include_str!("fixtures/ogc-geosparql/validator.ttl");
const EXAMPLE_BRIDGE: &str = include_str!("fixtures/example-bridge/example-bridge.ttl");
const EXAMPLES: &str = "tests/fixtures/ogc-geosparql/examples";

/// Example files whose verdict currently deviates from the OGC oracle, with the
/// engine gap they sit behind. Keep sorted; the ratchet asserts both directions.
const KNOWN_DEVIATIONS: &[(&str, &str)] = &[
    // Empirical baseline: 46/48 match the OGC oracle — see docs/conformance/geosparql.md.
    // (S04-invalid-01/02 fixed by the typed-term SHACL engine refactor: node-level
    // datatype/lexical-form checks now see the focus literal's datatype.)
    (
        "S18-invalid.ttl",
        "validator sh:sparql constraint subtlety not caught",
    ),
    (
        "S21-invalid.ttl",
        "validator sh:sparql constraint subtlety not caught",
    ),
];

fn validate_against_ogc(data: &str, data_fmt: RdfFormat) -> Result<bool, String> {
    let store = TripleStore::in_memory().map_err(|e| e.to_string())?;
    store
        .load_str(VALIDATOR, RdfFormat::Turtle, Some("urn:ogc:shapes"))
        .map_err(|e| format!("validator load: {e}"))?;
    // The OGC example files use relative IRIs (`<feature-x>`) without a BASE.
    store
        .load_str_with_base(
            data,
            data_fmt,
            "http://example.com/geosparql-examples/",
            Some("urn:ogc:data"),
        )
        .map_err(|e| format!("data load: {e}"))?;
    let report = validate(&store, "urn:ogc:shapes", &["urn:ogc:data".to_string()])?;
    Ok(report.conforms)
}

#[test]
fn ogc_examples_match_the_ogc_validator_oracle() {
    let mut files: Vec<_> = std::fs::read_dir(Path::new(EXAMPLES))
        .expect("vendored examples present")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ttl"))
        .collect();
    files.sort();
    assert!(files.len() > 30, "examples present: {}", files.len());

    let mut pass = 0usize;
    let mut unexpected = Vec::new();
    let mut fixed = Vec::new();

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let want_conforms = !name.contains("invalid");
        let content = std::fs::read_to_string(path).unwrap();
        let got = validate_against_ogc(&content, RdfFormat::Turtle);
        let matches = matches!(&got, Ok(c) if *c == want_conforms);
        let known = KNOWN_DEVIATIONS.iter().find(|(k, _)| *k == name);
        match (matches, known) {
            (true, None) => pass += 1,
            (true, Some((k, why))) => fixed.push(format!("{k} (listed as: {why})")),
            (false, None) => unexpected.push(format!(
                "{name}: want conforms={want_conforms}, got {got:?}"
            )),
            (false, Some(_)) => {}
        }
    }

    println!(
        "OGC GeoSPARQL SHACL examples: {pass} matching, {} known-deviation, {} total",
        KNOWN_DEVIATIONS.len(),
        files.len()
    );
    assert!(
        unexpected.is_empty(),
        "examples deviating from the OGC oracle (not in KNOWN_DEVIATIONS):\n  {}",
        unexpected.join("\n  ")
    );
    assert!(
        fixed.is_empty(),
        "KNOWN_DEVIATIONS now match — remove them to ratchet forward:\n  {}",
        fixed.join("\n  ")
    );
}

/// The canonical reference example round-trips through the OGC GeoSPARQL 1.1
/// validator shapes: GeoSPARQL data, validated by GeoSPARQL's own SHACL shapes,
/// by our engine.
#[test]
fn example_bridge_conforms_to_ogc_geosparql_validator_shapes() {
    let conforms = validate_against_ogc(EXAMPLE_BRIDGE, RdfFormat::Turtle)
        .expect("validation runs without error");
    assert!(
        conforms,
        "the reference example must conform to the OGC GeoSPARQL 1.1 validator shapes"
    );
}
