# RML-IO source test cases — vendored copy

- Source: https://github.com/kg-construct/rml-io (`test-cases/`)
- Commit: c0c5902bc547f7867afe746625ed18d418b06cdb (2026-08-19), vendored 2026-10-03
- Authors: the W3C Knowledge Graph Construction Community Group.
- Licence: see `LICENSE.md` (treated as CC BY-SA 4.0).
- Content: the 32 `RMLSTC*` (source) case directories — each its
  `mapping.ttl`, its input files and its expected `default.nq` / `output.nq` —
  and the suite's `manifest.ttl`. Byte-identical to upstream. Not taken: the
  `RMLTTC*` (logical target) cases, which test writing output files, each
  case's `README.md`, and the suite's HTML documentation, spreadsheets and
  scripts.
- Runner: `tests/rml_io_conformance.rs`. Two cases need a service rather than a
  file — `RMLSTC0003` a SPARQL endpoint and `RMLSTC0006a` a database described
  with D2RQ — and are runner-side skips; this engine maps both through
  registered datasources instead.
