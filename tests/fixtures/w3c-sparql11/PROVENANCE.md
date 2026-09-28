# W3C SPARQL 1.1 test suite — vendored copy

- Source: https://github.com/w3c/rdf-tests (`sparql/sparql11`)
- Commit: 369a90d1a60c021b746df2e411da0ff36258a758 (vendored 2026-09-10)
- Copyright: W3C. The suite's cover page carries "Copyright © 2010 W3C®
  (MIT, ERCIM, Keio), All Rights Reserved" in its header and "Copyright ©
  2015 W3C® (MIT, ERCIM, Keio, Beihang)" in its footer; both notices, its
  distribution statement and its disclaimer are reproduced in LICENSE.md.
- License: W3C 3-clause BSD License
  (https://www.w3.org/copyright/3-clause-bsd-license-2008/, full text in
  LICENSE.md). Upstream offers the suite under either the W3C Test Suite
  License or the W3C 3-clause BSD License, chosen per use. This copy relies on
  the BSD option because it is a subset: W3C's test-suite licence policy
  (https://www.w3.org/copyright/test-suites-licenses/) treats a subset as a
  derivative work, which the W3C Test Suite License does not permit. Not
  covered by this project's AGPL-3.0 + Commons Clause licence.
- Changes: none. Every vendored file is byte-identical to upstream at the
  commit above; the only difference is which sections were copied (Scope).
  LICENSE.md starts from upstream's root `LICENSE.md` pointer, kept verbatim,
  and adds the original notice and the BSD text; this file is ours.
- Scope: the directories included by `manifest-sparql11-query.ttl` and
  `manifest-sparql11-update.ttl` — query evaluation, query syntax, update
  evaluation and update syntax. Not vendored: the protocol, service
  description, graph-store-protocol (`graph-store-protocol/`, and its
  deprecated predecessor `http-rdf-update/`), federation (`service/`,
  `syntax-fed/`), result-format (`csv-tsv-res/`, `json-res/`) and
  entailment-regime (`entailment/`) sections, which test surfaces this
  runner does not cover; the other top-level manifests (`manifest.ttl`,
  `manifest-all.ttl`, `manifest-sparql11-fed.ttl`,
  `manifest-sparql11-results.ttl`); and the cover-page template
  `template.haml`.
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
  summarised in docs/conformance/sparql11.md. The results are development
  and regression results on this subset, not a W3C conformance claim.
