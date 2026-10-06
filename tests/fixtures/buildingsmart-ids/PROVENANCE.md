# buildingSMART IDS 1.0 test corpus — fetched at test time, not vendored

- Source: https://github.com/buildingSMART/IDS
  (`Documentation/ImplementersDocumentation/TestCases`)
- Commit: 870f9c4e6e8f414e737b4d84ca1aa9b46fc6c8f3 (pinned 2026-10-03)
- Content: 334 test cases in nine folders (`attribute`, `classification`,
  `entity`, `ids`, `material`, `partof`, `property`, `restriction`,
  `tolerance`), each an IDS file and the IFC file it is checked against,
  named `pass-`, `fail-` or `invalid-` after the outcome every IDS
  implementation must reach.
- Runner: `tests/buildingsmart_ids_conformance.rs` — see its header and
  docs/conformance/ids.md.

## What is in this directory

Only `MANIFEST.sha256`: the path (relative to the `TestCases` folder) and the
SHA-256 digest of each of the 668 corpus files at the commit above, written by
this project. The corpus itself is not in this repository and is not shipped
with any build: the runner downloads each file from
`https://raw.githubusercontent.com/buildingSMART/IDS/<commit>/Documentation/ImplementersDocumentation/TestCases/<path>`
on first run, keeps it under `target/buildingsmart-ids/<commit>/` (or
`$OTS_IDS_CORPUS_DIR`), and checks every file against the manifest before it
runs a single case. A file that does not match is a test failure, never a
skip. Offline, the runner skips with a message; `OTS_TEST_IDS_CORPUS_REQUIRED=1`
(set in CI) makes a failed download fail instead.

## Why fetched rather than vendored

- The corpus is © buildingSMART International Ltd. and licensed under the
  Creative Commons Attribution-NoDerivatives 4.0 International licence
  (https://creativecommons.org/licenses/by-nd/4.0/; the repository's `LICENSE`
  file). Copies may be redistributed verbatim with attribution, but not in
  modified form; keeping the files out of this repository removes any question
  of line-ending normalisation or other changes on checkout.
- The corpus's own documentation (`TestCases/scripts.md`) says its IFC files
  "have been imported from the work previously done in the IfcOpenShell
  repository" (ifctester) "and adepted where appropriate". IfcOpenShell is
  licensed LGPL-3.0; the provenance of each IFC file was not traced further,
  so they are fetched under the IDS repository's licence and not redistributed.

## Results

The results are this project's development and regression results on the
corpus. They are not a buildingSMART certification or compliance statement,
and buildingSMART has not reviewed or endorsed them. No score is published.
