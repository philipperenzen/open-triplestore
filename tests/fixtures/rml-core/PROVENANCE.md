# RML-Core test cases — vendored copy

- Source: https://github.com/kg-construct/rml-core (`test-cases/`)
- Commit: 82ab28d46803ba66a83c133f1db371a60116f84d (2026-04-20), vendored 2026-10-03
- Authors: the W3C Knowledge Graph Construction Community Group; the
  repository's `CITATION.cff` names Pano Maria, Anastasia Dimou, Ben De
  Meester, Ana Iglesias-Molina, David Chaves-Fraga and others.
- Licence: see `LICENSE.md` (treated as CC BY-SA 4.0).
- Content: the 76 `RMLTC*-JSON` case directories — each its `mapping.ttl`,
  its input files and its expected `output.nq` — and the suite's
  `manifest.ttl`. Byte-identical to upstream. Not taken: each case's
  `README.md` (a rendering of the same files), the suite's HTML documentation,
  metadata spreadsheets and generator scripts.
- Runner: `tests/rml_core_conformance.rs` — manifest-driven; a case passes when
  its output dataset is isomorphic to the expected one, or, for a case with
  `rmltest:hasError true`, when the mapping is refused or the run fails. Known
  failures are listed, with the reason, in the runner's `KNOWN_FAILURES`.
