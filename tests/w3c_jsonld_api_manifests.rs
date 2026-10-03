//! W3C JSON-LD 1.1 API test suite — the `toRdf` and `fromRdf` sections,
//! vendored unmodified in `tests/fixtures/w3c-jsonld-api/` and driven by
//! their manifests, through the JSON-LD processor every RDF parse and
//! serialisation of this server uses (oxigraph's, behind `RdfParser` /
//! `RdfSerializer` with `RdfFormat::JsonLd`).
//!
//! This is a development and regression ratchet. The two sections are a
//! subset of a W3C test suite, so no score is published for them (W3C's
//! test-suite policy allows no claims of performance on a subset,
//! https://www.w3.org/copyright/test-suites-licenses/), and the runner keeps
//! no pass count either. What it asserts:
//!
//! * every evaluated entry not in [`KNOWN_FAILURES`] passes, and every listed
//!   entry still fails (so the list can only shrink);
//! * at least [`PASS_FLOOR`] entries pass.
//!
//! How an entry is evaluated:
//!
//! * `toRdf`: the input is parsed with the base IRI the manifest gives it
//!   (`baseIri` + input path, or the entry's `base` option), remote documents
//!   (contexts the tests name by IRI) resolved from the vendored files by a
//!   document loader. A `PositiveEvaluationTest` passes when the dataset is
//!   isomorphic to the expected N-Quads; a `NegativeEvaluationTest` when the
//!   parse fails (the error code itself is not compared); a
//!   `PositiveSyntaxTest` when the parse succeeds.
//! * `fromRdf`: the input N-Quads are serialised as JSON-LD and parsed back,
//!   and the result must be isomorphic to the expected document parsed by the
//!   same processor. The comparison is at the RDF level, not JSON-LD object
//!   equality: the serialiser writes a compacted form, the expected outputs
//!   are expanded.
//!
//! Entries are skipped by design (and counted, never silently) when they test
//! an option this processor does not offer: JSON-LD 1.0-only behaviour
//! (`specVersion` / `processingMode` `json-ld-1.0`), generalized RDF,
//! `rdfDirection`, `expandContext`, and the `useNativeTypes` / `useRdfType`
//! serialisation options. A manifest input that is missing from the vendored
//! files fails the run instead of counting as a parse error.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oxigraph::io::{JsonLdProfileSet, LoadedDocument, RdfFormat, RdfParser, RdfSerializer};
use oxigraph::model::dataset::CanonicalizationAlgorithm;
use oxigraph::model::Dataset;
use serde_json::Value;

const ROOT: &str = "tests/fixtures/w3c-jsonld-api";
const BASE: &str = "https://w3c.github.io/json-ld-api/tests/";

/// Entries this processor gets wrong, `(section#id, reason)`. Every one is a
/// processor (oxjsonld 0.2.6) deviation, not a runner limitation.
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("toRdf#tdi02", "built with rdf-12, @direction becomes an RDF 1.2 directional language string; JSON-LD 1.1 drops it unless rdfDirection is set"),
    ("toRdf#tdi04", "built with rdf-12, @direction becomes an RDF 1.2 directional language string; JSON-LD 1.1 drops it unless rdfDirection is set"),
    ("toRdf#tdi05", "built with rdf-12, @direction becomes an RDF 1.2 directional language string; JSON-LD 1.1 drops it unless rdfDirection is set"),
    ("toRdf#tdi06", "built with rdf-12, @direction becomes an RDF 1.2 directional language string; JSON-LD 1.1 drops it unless rdfDirection is set"),
    (
        "fromRdf#t0016",
        "a list whose nodes are typed rdf:List is written as nodes that read back as a different dataset",
    ),
    (
        "fromRdf#tjs08",
        "an invalid rdf:JSON literal is serialised instead of refused",
    ),
    (
        "fromRdf#tjs09",
        "an invalid rdf:JSON literal is serialised instead of refused",
    ),
];

/// Fewest passing entries across both sections.
const PASS_FLOOR: usize = 470;

fn json_ld() -> RdfFormat {
    RdfFormat::JsonLd {
        profile: JsonLdProfileSet::empty(),
    }
}

/// Serve `https://w3c.github.io/json-ld-api/tests/<path>` from the vendored
/// files; everything else is not dereferenceable, as for any processor run
/// offline.
fn loader(url: &str) -> Result<LoadedDocument, Box<dyn std::error::Error + Send + Sync>> {
    let url = url.split('#').next().unwrap_or(url);
    let rel = url
        .strip_prefix(BASE)
        .ok_or_else(|| format!("<{url}> is not part of the vendored suite"))?;
    let path = Path::new(ROOT).join(rel);
    let content = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(LoadedDocument {
        url: url.to_string(),
        content,
        format: json_ld(),
    })
}

/// A vendored file the manifest names; a missing one is a broken vendoring,
/// not a test result.
fn read_fixture(rel: &str) -> Vec<u8> {
    let path = Path::new(ROOT).join(rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn parse_json_ld_bytes(bytes: &[u8], base: &str) -> Result<Dataset, String> {
    let parser = RdfParser::from_format(json_ld())
        .with_base_iri(base)
        .map_err(|e| e.to_string())?;
    let mut ds = Dataset::new();
    for q in parser.for_reader(bytes).with_document_loader(loader) {
        ds.insert(&q.map_err(|e| e.to_string())?);
    }
    Ok(ds)
}

fn parse_nquads(bytes: &[u8]) -> Result<Dataset, String> {
    let mut ds = Dataset::new();
    for q in RdfParser::from_format(RdfFormat::NQuads).for_reader(bytes) {
        ds.insert(&q.map_err(|e| e.to_string())?);
    }
    Ok(ds)
}

fn isomorphic(mut a: Dataset, mut b: Dataset) -> bool {
    a.canonicalize(CanonicalizationAlgorithm::Unstable);
    b.canonicalize(CanonicalizationAlgorithm::Unstable);
    a == b
}

/// The quads of `a` that are not in `b`, as N-Quads lines (blank nodes as
/// labelled, so a blank-node-only difference shows up on both sides).
fn difference(a: &Dataset, b: &Dataset) -> Vec<String> {
    let mut out: Vec<String> = a
        .iter()
        .filter(|q| !b.contains(*q))
        .map(|q| q.to_string())
        .collect();
    out.sort();
    out.truncate(6);
    out
}

fn types(entry: &Value) -> Vec<String> {
    match &entry["@type"] {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Why `entry` is not evaluated, if it tests an option this processor does
/// not offer.
fn skip_reason(entry: &Value) -> Option<&'static str> {
    let opt = &entry["option"];
    if opt["specVersion"] == "json-ld-1.0" {
        return Some("JSON-LD 1.0-only behaviour");
    }
    if opt["processingMode"] == "json-ld-1.0" {
        return Some("processingMode json-ld-1.0");
    }
    if opt["produceGeneralizedRdf"] == true {
        return Some("generalized RDF");
    }
    if !opt["rdfDirection"].is_null() {
        return Some("rdfDirection");
    }
    if !opt["expandContext"].is_null() {
        return Some("expandContext");
    }
    if opt["useNativeTypes"] == true || opt["useRdfType"] == true {
        return Some("useNativeTypes / useRdfType");
    }
    None
}

#[derive(Default)]
struct Tally {
    pass: usize,
    skipped: HashMap<&'static str, usize>,
    failing: Vec<(String, String)>,
}

fn manifest(name: &str) -> Vec<Value> {
    let path = PathBuf::from(ROOT).join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    let v: Value = serde_json::from_str(&text).unwrap();
    v["sequence"].as_array().cloned().unwrap_or_default()
}

fn run_to_rdf(tally: &mut Tally) {
    for entry in manifest("toRdf-manifest.jsonld") {
        let id = format!("toRdf{}", entry["@id"].as_str().unwrap_or("?"));
        if let Some(why) = skip_reason(&entry) {
            *tally.skipped.entry(why).or_default() += 1;
            continue;
        }
        let input = entry["input"].as_str().unwrap();
        let base = entry["option"]["base"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("{BASE}{input}"));
        let result = parse_json_ld_bytes(&read_fixture(input), &base);
        let types = types(&entry);
        let outcome: Result<(), String> = if types.iter().any(|t| t == "jld:NegativeEvaluationTest")
        {
            match result {
                Err(_) => Ok(()),
                Ok(_) => Err(format!(
                    "parsed, but {} is expected",
                    entry["expectErrorCode"].as_str().unwrap_or("an error")
                )),
            }
        } else if types.iter().any(|t| t == "jld:PositiveSyntaxTest") {
            result.map(|_| ())
        } else {
            let expect = entry["expect"].as_str().unwrap();
            let expected = parse_nquads(&read_fixture(expect));
            match (result, expected) {
                (Ok(actual), Ok(expected)) => {
                    if isomorphic(actual.clone(), expected.clone()) {
                        Ok(())
                    } else {
                        Err(format!(
                            "dataset differs from the expected N-Quads\n  only produced: {:?}\n  only expected: {:?}",
                            difference(&actual, &expected),
                            difference(&expected, &actual)
                        ))
                    }
                }
                (Err(e), _) => Err(format!("parse failed: {e}")),
                (_, Err(e)) => Err(format!("expected output unreadable: {e}")),
            }
        };
        match outcome {
            Ok(()) => tally.pass += 1,
            Err(e) => tally.failing.push((id, e)),
        }
    }
}

fn run_from_rdf(tally: &mut Tally) {
    for entry in manifest("fromRdf-manifest.jsonld") {
        let id = format!("fromRdf{}", entry["@id"].as_str().unwrap_or("?"));
        if let Some(why) = skip_reason(&entry) {
            *tally.skipped.entry(why).or_default() += 1;
            continue;
        }
        let input = entry["input"].as_str().unwrap();
        let serialised = (|| -> Result<Vec<u8>, String> {
            let source = parse_nquads(&read_fixture(input))?;
            let mut out = RdfSerializer::from_format(json_ld()).for_writer(Vec::new());
            for q in source.iter() {
                out.serialize_quad(q).map_err(|e| e.to_string())?;
            }
            out.finish().map_err(|e| e.to_string())
        })();
        if types(&entry)
            .iter()
            .any(|t| t == "jld:NegativeEvaluationTest")
        {
            // An error while serialising, or output that does not parse back.
            let failed = match &serialised {
                Err(_) => true,
                Ok(w) => parse_json_ld_bytes(w, &format!("{BASE}{input}")).is_err(),
            };
            if failed {
                tally.pass += 1;
            } else {
                tally.failing.push((
                    id,
                    format!(
                        "serialised without error, but {} is expected",
                        entry["expectErrorCode"].as_str().unwrap_or("an error")
                    ),
                ));
            }
            continue;
        }
        let expect = entry["expect"].as_str().unwrap();
        let outcome = (|| -> Result<(), String> {
            let written = serialised?;
            let round_trip =
                parse_json_ld_bytes(&written, &format!("{BASE}{expect}")).map_err(|e| {
                    format!(
                        "the serialised JSON-LD does not parse back: {e}\n{}",
                        String::from_utf8_lossy(&written)
                    )
                })?;
            let expected = parse_json_ld_bytes(&read_fixture(expect), &format!("{BASE}{expect}"))
                .map_err(|e| format!("expected output unreadable: {e}"))?;
            if isomorphic(round_trip, expected) {
                Ok(())
            } else {
                Err(format!(
                    "serialised JSON-LD reads as a different dataset:\n{}",
                    String::from_utf8_lossy(&written)
                ))
            }
        })();
        match outcome {
            Ok(()) => tally.pass += 1,
            Err(e) => tally.failing.push((id, e)),
        }
    }
}

#[test]
fn w3c_json_ld_api_to_rdf_and_from_rdf() {
    let mut tally = Tally::default();
    run_to_rdf(&mut tally);
    run_from_rdf(&mut tally);

    let unexpected: Vec<String> = tally
        .failing
        .iter()
        .filter(|(id, _)| !KNOWN_FAILURES.iter().any(|(k, _)| k == id))
        .map(|(id, e)| format!("{id}: {e}"))
        .collect();
    let fixed: Vec<&str> = KNOWN_FAILURES
        .iter()
        .filter(|(k, _)| !tally.failing.iter().any(|(id, _)| id == k))
        .map(|(k, _)| *k)
        .collect();
    let skipped: usize = tally.skipped.values().sum();
    // Counts go to the test's own output only (W3C test-suite policy: no
    // published score for a subset).
    eprintln!(
        "json-ld-api toRdf+fromRdf: {} pass, {} failing ({} known), {skipped} skipped by design: {:?}",
        tally.pass,
        tally.failing.len(),
        KNOWN_FAILURES.len(),
        tally.skipped
    );
    assert!(
        unexpected.is_empty(),
        "{} entries failing that are not in KNOWN_FAILURES:\n  {}",
        unexpected.len(),
        unexpected.join("\n  ")
    );
    assert!(
        fixed.is_empty(),
        "KNOWN_FAILURES entries now pass — remove them to ratchet forward:\n  {}",
        fixed.join("\n  ")
    );
    assert!(
        tally.pass >= PASS_FLOOR,
        "only {} entries passed (floor {PASS_FLOOR})",
        tally.pass
    );
}
