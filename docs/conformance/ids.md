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

1. the IFC file through the built-in IFC lift's **IDS projection**
   (`ifc::convert_layers` with `include_ids`, the graph an IFC import writes to
   `…/building/ids`), into a fresh in-memory store;
2. the IDS file through the IDS importer (`POST /api/shacl/import/ids`), which
   checks it against the IDS 1.0 XSD and the IDS audit rules first;
3. the resulting shapes through the SHACL validator over the projection.

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

None: every case of the pinned corpus is satisfied, and the runner's
`KNOWN_FAILURES` list is empty. How the list got there:

1. **Baseline** (develop @ 570a7a3): the converter wrote SHACL Core over the
   building-topology layer, which lifts only the spatial tree, so a lone
   `IFCWALL` — most corpus models — was not in the graph. Nothing was
   targeted, every specification conformed vacuously, every `fail-` and
   `invalid-` case was unsatisfied (151 entries), and an untyped `0.` bound
   was invalid Turtle.
2. **Value, tolerance and cardinality mapping** (typed values, the IDS
   tolerance, anchored patterns, prohibited facets as negation, the existence
   check, every listed `ifcVersion`): the satisfied cases stopped being
   vacuous, and the list grew to 187 entries — what the building-topology
   layer cannot carry.
3. **The IDS projection, the SHACL-SPARQL converter and the IDS audit**: all
   334 cases satisfied.

Three behaviours the corpus settles, beyond the prose of the IDS
documentation:

- **A value exactly on the tolerance bound is equal.** `tolerance.md` writes
  the range with strict inequalities, but the corpus's `pass-` tolerance cases
  sit exactly on `v ± (|v|·1e-6 + 1e-6)`; the range is closed (and widened by a
  few ulps for binary rounding), and the `fail-` cases, just outside it, still
  fail.
- **`ifcVersion` does not exclude a model of another schema.** Several `ids/`
  cases run an `IFC2X3` specification against an IFC4 model and expect it to
  be checked. The list decides which names the audit accepts; the checks
  apply to a model of any schema.
- **An invalid document is refused at import** when the XSD or audit checks
  catch it, which counts as unsatisfiable.

The runner checks behaviour, not reports: it compares the conforms/violates
verdict per case, not the violation messages. The standard property-set
templates (the audit tool's `Pset_` checks) are not part of the audit here.
