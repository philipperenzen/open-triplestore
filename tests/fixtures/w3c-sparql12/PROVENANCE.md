# W3C SPARQL 1.2 test suite — vendored copy

- Source: https://github.com/w3c/rdf-tests (`sparql/sparql12`)
- Commit: 5e5da96bb06c551ae3ac57d30fec8116f7161b9b (2026-10-02; vendored
  2026-10-03)
- Copyright: W3C. The suite's cover page carries "Copyright © 2010 W3C®
  (MIT, ERCIM, Keio), All Rights Reserved" in its header and "Copyright ©
  2015 W3C® (MIT, ERCIM, Keio, Beihang)" in its footer; both notices, its
  distribution statement and its disclaimer are reproduced in LICENSE.md.
  The manifests name the W3C RDF & SPARQL Working Group as creator.
- License: W3C 3-clause BSD License
  (https://www.w3.org/copyright/3-clause-bsd-license-2008/, full text in
  LICENSE.md). Upstream offers the suite under either the W3C Test Suite
  License or the W3C 3-clause BSD License, chosen per use (the manifests
  declare `dct:licence` the BSD licence). This copy relies on the BSD option
  because it is a subset: W3C's test-suite licence policy
  (https://www.w3.org/copyright/test-suites-licenses/) treats a subset as a
  derivative work, which the W3C Test Suite License does not permit. Not
  covered by this project's AGPL-3.0 + Commons Clause licence.
- Changes: none. Every vendored file (360 files) is byte-identical to
  upstream at the commit above; the only difference is which files were
  copied (Scope). LICENSE.md and this file are ours.
- Scope: `manifest.ttl`, `README.md` and the ten directories the manifest
  includes (`codepoint-escapes`, `eval-triple-terms`, `expression`,
  `grouping`, `lang-basedir`, `rdf11`, `syntax`,
  `syntax-triple-terms-negative`, `syntax-triple-terms-positive`,
  `version`). Not vendored: `reports/` (implementation reports, not tests)
  and the cover-page template `template.haml`.
- Runner: `tests/w3c_sparql12_manifests.rs` — walks `manifest.ttl` through
  `mf:include`, loads each entry's data into a fresh store, runs the query or
  update through `TripleStore` (the path the HTTP endpoint uses; every
  query-evaluation entry a second time through the in-memory mirror) and
  compares against `mf:result`. Known gaps are tracked in the runner's
  KNOWN_FAILURES list (two-way ratchet) and in docs/conformance/sparql12.md.
  The results are development and regression results on this subset, not a
  W3C conformance claim, and no score is published for them.
