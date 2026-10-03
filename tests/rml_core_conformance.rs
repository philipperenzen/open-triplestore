//! The RML-Core test cases of the W3C Knowledge Graph Construction Community
//! Group, vendored in `tests/fixtures/rml-core/` (CC BY-SA 4.0; see its
//! `LICENSE.md` and `PROVENANCE.md`).
//!
//! Every case is a JSON source, a mapping in the RML-Core vocabulary
//! (`http://w3id.org/rml/`) and the expected output dataset, or the
//! expectation of an error. The runner is manifest-driven, runs each case
//! through the file executor with the case's default base IRI, compares
//! output datasets by isomorphism, and holds the suite to `KNOWN_FAILURES` as
//! a two-way ratchet: an unlisted failure fails the test, and so does a listed
//! case that passes.

#[path = "rml_corpus/mod.rs"]
mod rml_corpus;

use open_triplestore::rml::checks::OnDataError;
use rml_corpus::*;

/// Empirical baseline: 75 pass / 1 known-fail / 0 aux skips
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("RMLTC0027b-JSON", "rml:UnsafeIRI writes template values unencoded, and the expected output holds <http://example.com/Person/Emily Smith> — an IRI with a space, which RDF does not allow; the store refuses to make it and the run reports a data error"),
];

/// Cases the runner does not run.
const SKIPS: &[(&str, &str)] = &[];

#[test]
fn rml_core_test_cases() {
    let cases = manifest_cases(&fixture("rml-core"), "http://example.com/");
    assert!(
        cases.len() >= 70,
        "the manifest lists {} cases",
        cases.len()
    );
    let tally = run_suite(
        "RML-Core",
        &cases,
        KNOWN_FAILURES,
        SKIPS,
        OnDataError::Abort,
    );
    assert_baseline(include_str!("rml_core_conformance.rs"), &tally);
    // A floor as well as the ratchet: a regression that made every case
    // unreadable would otherwise only move cases between the lists.
    // `scripts/conformance_table.py` publishes it (CORPUS_RUNNERS).
    assert!(
        tally.pass >= 70,
        "only {} cases passed (floor 70)",
        tally.pass
    );
}

#[test]
fn every_known_failure_gives_a_reason() {
    for (id, why) in KNOWN_FAILURES.iter().chain(SKIPS) {
        assert!(id.starts_with("RMLTC"), "{id}");
        assert!(!why.trim().is_empty(), "{id} gives no reason");
    }
}
