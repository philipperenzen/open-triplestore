# Legacy RML test cases (CSV, JSON, XML) — vendored copy

- Source: https://github.com/kg-construct/rml-test-cases (`test-cases/`;
  archived upstream on 2026-03-11)
- Commit: 803dd3ec6b7185801cf19ebecaa18513baf78613, vendored 2026-10-03
- Authors: RML.io, IDLab, Ghent University – imec.
- Licence: CC BY 4.0 — see `LICENSE.md`.
- Content: the 118 case directories for file sources (`*-CSV`, `*-JSON`,
  `*-XML`) — each its `mapping.ttl`, input files and expected `output.nq` —
  and the suite's `metadata.csv`, which says per case whether an error is
  expected. Byte-identical to upstream. `metadata.csv` lists 117 of the 118
  directories; `RMLTC0002g-JSON` has no row (so no stated expectation) and
  the runner does not run it. Not taken: the MySQL, PostgreSQL,
  SQL Server and SPARQL cases, the HTML documentation and the RDF metadata.
- These cases are written in the legacy RML vocabulary
  (`http://semweb.mmlab.be/ns/rml#`), which this engine still reads, and
  expect the legacy processor's behaviour: JSON values without natural
  datatypes and data errors left out of the output rather than failing the
  run.
- Runner: `tests/rml_legacy_conformance.rs`, with its known failures and their
  reasons in `KNOWN_FAILURES`.
