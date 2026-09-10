# W3C SPARQL 1.1 test suite — vendored copy

- Source: https://github.com/w3c/rdf-tests (`sparql/sparql11`)
- Commit: 369a90d1a60c021b746df2e411da0ff36258a758 (vendored 2026-09-10)
- License: see LICENSE.md (W3C test-suite dual licence: W3C Test Suite
  License / 3-clause BSD)
- Scope: the directories included by `manifest-sparql11-query.ttl` and
  `manifest-sparql11-update.ttl` — query evaluation, query syntax, update
  evaluation and update syntax. Not vendored: the protocol, service
  description, graph-store-protocol, federation (`service/`, `syntax-fed/`),
  result-format (`csv-tsv-res/`, `json-res/`) and entailment-regime
  (`entailment/`) sections, which test surfaces this runner does not cover.
- Runner: `tests/w3c_sparql11_manifests.rs` — walks the two top-level
  manifests through `mf:include`, loads each entry's `qt:data` /
  `qt:graphData` (`ut:data` / `ut:graphData` for updates) into a fresh store,
  runs the query or update through `TripleStore` (the same path the HTTP
  endpoint uses) and compares against `mf:result`: boolean for ASK,
  solution multisets (order-sensitive when the query has an outer `ORDER
  BY`) for SELECT, graph isomorphism for CONSTRUCT/DESCRIBE and per-graph
  isomorphism of the resulting dataset for updates. Syntax tests assert
  parse success/failure only. Known gaps are tracked in the runner's
  KNOWN_FAILURES list (two-way ratchet, like the SHACL corpus) and
  summarised in docs/conformance/sparql11.md.
