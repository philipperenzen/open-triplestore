# buildingSMART IDS 1.0 — test-corpus ratchet and known gaps

The [buildingSMART IDS repository](https://github.com/buildingSMART/IDS) ships a
test corpus for Information Delivery Specification 1.0 implementations:
334 pairs of an IDS file and an IFC file in nine folders (`attribute`,
`classification`, `entity`, `ids`, `material`, `partof`, `property`,
`restriction`, `tolerance`), each named `pass-`, `fail-` or `invalid-` after the
outcome every implementation must reach. The corpus documentation says all
valid implementations must behave identically on it.

[`tests/buildingsmart_ids_conformance.rs`](../../tests/buildingsmart_ids_conformance.rs)
runs the whole corpus in CI as a development and regression ratchet. This page
describes how, and tracks the gaps it finds. It publishes no score: the results
are this project's development results, **not a buildingSMART certification**,
and buildingSMART has not reviewed them.

## Fetched, not vendored

The corpus is © buildingSMART International Ltd. under CC BY-ND 4.0, and its
IFC files came from IfcOpenShell's ifctester work, so the repository does not
carry a copy. [`tests/fixtures/buildingsmart-ids/`](../../tests/fixtures/buildingsmart-ids/PROVENANCE.md)
holds only `MANIFEST.sha256` — the path and SHA-256 of each of the 668 files at
the pinned commit (870f9c4e, 2026-09-25) — and its provenance note.

The runner downloads each file from that commit on first run, caches it under
`target/buildingsmart-ids/<commit>/` (override with `OTS_IDS_CORPUS_DIR`), and
checks every file against the manifest before it runs anything; a mismatch is a
failure, never a skip. Without network access the runner skips with a message.
CI sets `OTS_TEST_IDS_CORPUS_REQUIRED=1`, which turns a failed download into a
failure.

## What runs

Each case takes the path a user takes:

1. the IFC file through the built-in IFC lift (`ifc::convert`, the same code
   the IFC import endpoint runs), into a fresh in-memory store;
2. the IDS file through the IDS importer (`POST /api/shacl/import/ids`);
3. the resulting shapes through the SHACL validator over the lifted graph.

A case is satisfied when

| prefix | satisfied when |
|---|---|
| `pass-` | the importer accepts the IDS and the model conforms |
| `fail-` | the importer rejects the IDS, or the model does not conform |
| `invalid-` | the same as `fail-`: an invalid IDS "could not be satisfied, regardless of IFC contents", so refusing it at import counts |

**Gap policy:** a two-way ratchet, as for the W3C SHACL corpus. Every case not
in the runner's `KNOWN_FAILURES` list must be satisfied, and every listed case
must still be unsatisfied, so silent regressions and silent fixes both turn CI
red. `OTS_IDS_PRINT_FAILURES=1` prints the current list.

## Known gaps

The list was measured first against the converter and lift as they were
(develop @ 570a7a3), before any IDS work, to set the baseline:

- **The building-topology layer is the wrong data for IDS.** It lifts spatial
  elements and the elements contained in or aggregated into them, so a lone
  `IFCWALL` — most corpus models — is not in the graph at all. Nothing is
  targeted, every specification conforms vacuously, and every `fail-` and
  `invalid-` case is unsatisfied. The `pass-` cases mostly "pass" the same
  vacuous way.
- **No IDS audit.** An invalid IDS is accepted.
- **Bounds untyped.** A `0.` bound became invalid Turtle (four `pass-` cases).

The cards that follow this baseline fix the converter's value, tolerance and
cardinality mapping, and then add an IDS-oriented projection to the IFC lift
beside the building-topology output, with IDS document validation; the list in
the runner records what is still open after each step.
