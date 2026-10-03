//! The legacy RML test cases for file sources (CSV, JSON, XML) of RML.io /
//! IDLab, Ghent University – imec, vendored in `tests/fixtures/rml-legacy/`
//! (CC BY 4.0; see its `LICENSE.md` and `PROVENANCE.md`).
//!
//! The mappings are in the legacy RML vocabulary
//! (`http://semweb.mmlab.be/ns/rml#`), which this engine still reads. Their
//! expected outputs are those of a processor that leaves a data error out of
//! the output instead of failing the run (`RMLTC0019b`, "with data error",
//! expects no error), so the runner runs them the way this engine's opt-in
//! lenient mode does (`OnDataError::Skip`). `metadata.csv` says which cases
//! expect an error; outputs are compared by isomorphism, and the suite is held
//! to `KNOWN_FAILURES` as a two-way ratchet.

#[path = "rml_corpus/mod.rs"]
mod rml_corpus;

use open_triplestore::rml::checks::OnDataError;
use rml_corpus::*;

/// Empirical baseline: 112 pass / 5 known-fail / 0 aux skips
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("RMLTC0002c-JSON", "the reference IDs names a key no record has; the RML-IO registry makes a JSONPath reference to a missing member NULL, not an error, as this engine does"),
    ("RMLTC0002c-XML", "the reference IDs names an element no record has; the RML-IO registry makes an XPath reference that selects nothing NULL, not an error, as this engine does"),
    ("RMLTC0007h-CSV", "rr:graph \"…\" names a graph with a literal, which R2RML §7.4 and RML-Core refuse as a non-conforming mapping; the legacy suite expects it ignored"),
    ("RMLTC0007h-JSON", "rr:graph with a literal is a non-conforming mapping (R2RML §7.4); the legacy suite expects it ignored"),
    ("RMLTC0007h-XML", "rr:graph with a literal is a non-conforming mapping (R2RML §7.4); the legacy suite expects it ignored"),
];

/// Cases the runner does not run.
const SKIPS: &[(&str, &str)] = &[];

fn cases() -> Vec<Case> {
    let dir = fixture("rml-legacy");
    let mut rdr = csv::Reader::from_path(dir.join("metadata.csv")).expect("metadata.csv");
    let headers = rdr.headers().unwrap().clone();
    let col = |name: &str| headers.iter().position(|h| h == name).expect(name);
    let (id_col, error_col) = (col("RML id"), col("error expected?"));
    let mut out = Vec::new();
    for record in rdr.records() {
        let record = record.unwrap();
        let id = record[id_col].to_string();
        if !(id.ends_with("-CSV") || id.ends_with("-JSON") || id.ends_with("-XML")) {
            continue;
        }
        let case_dir = dir.join(&id);
        if !case_dir.is_dir() {
            continue;
        }
        out.push(Case {
            output: case_dir
                .join("output.nq")
                .is_file()
                .then(|| "output.nq".to_string()),
            dir: case_dir,
            mapping: "mapping.ttl".to_string(),
            error: record[error_col].trim() == "true",
            base: "http://example.com/base/".to_string(),
            id,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[test]
fn legacy_rml_test_cases() {
    let cases = cases();
    assert!(
        cases.len() >= 110,
        "metadata.csv lists {} file cases",
        cases.len()
    );
    let tally = run_suite(
        "legacy RML",
        &cases,
        KNOWN_FAILURES,
        SKIPS,
        OnDataError::Skip,
    );
    assert_baseline(include_str!("rml_legacy_conformance.rs"), &tally);
    // A floor as well as the ratchet: a regression that made every case
    // unreadable would otherwise only move cases between the lists.
    // `scripts/conformance_table.py` publishes it (CORPUS_RUNNERS).
    assert!(
        tally.pass >= 100,
        "only {} cases passed (floor 100)",
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
