//! The RML-IO source test cases (`RMLSTC*`) of the W3C Knowledge Graph
//! Construction Community Group, vendored in `tests/fixtures/rml-io/`
//! (CC BY-SA 4.0; see its `LICENSE.md` and `PROVENANCE.md`): encodings,
//! compression, NULL values, reference formulations, XML namespaces, several
//! sources, quoted CSV columns, invalid sources and nested JSON and XML.
//!
//! Manifest-driven, compared by isomorphism, held to `KNOWN_FAILURES` as a
//! two-way ratchet. The two cases that need a service rather than a file are
//! runner-side skips (`SKIPS`).

#[path = "rml_corpus/mod.rs"]
mod rml_corpus;

use open_triplestore::rml::checks::OnDataError;
use rml_corpus::*;

/// Empirical baseline: 29 pass / 1 known-fail / 2 aux skips
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("RMLSTC0009a", "the manifest says hasError true, but the suite's descriptions.csv says no error is expected and the case ships the output of reading the quoted header \"id\",\"name\",\"age\" as RFC 4180 does — the output this engine produces"),
];

/// Cases the runner does not run: they need a service, not a file.
const SKIPS: &[(&str, &str)] = &[
    (
        "RMLSTC0003",
        "a SPARQL endpoint as the source; this engine maps endpoints through registered \
         datasources",
    ),
    (
        "RMLSTC0006a",
        "a database described with D2RQ in the mapping; this engine maps databases through \
         registered datasources",
    ),
];

#[test]
fn rml_io_source_test_cases() {
    let cases: Vec<Case> = manifest_cases(&fixture("rml-io"), "http://example.com/")
        .into_iter()
        .filter(|c| c.id.starts_with("RMLSTC"))
        .collect();
    assert!(
        cases.len() >= 30,
        "the manifest lists {} source cases",
        cases.len()
    );
    let tally = run_suite("RML-IO", &cases, KNOWN_FAILURES, SKIPS, OnDataError::Abort);
    assert_baseline(include_str!("rml_io_conformance.rs"), &tally);
    // A floor as well as the ratchet: a regression that made every case
    // unreadable would otherwise only move cases between the lists.
    // `scripts/conformance_table.py` publishes it (CORPUS_RUNNERS).
    assert!(
        tally.pass >= 25,
        "only {} cases passed (floor 25)",
        tally.pass
    );
}

#[test]
fn every_known_failure_and_skip_gives_a_reason() {
    for (id, why) in KNOWN_FAILURES.iter().chain(SKIPS) {
        assert!(id.starts_with("RMLSTC"), "{id}");
        assert!(!why.trim().is_empty(), "{id} gives no reason");
    }
}
