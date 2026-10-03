# W3C RDF 1.2 and RDF 1.1 syntax test suites — vendored copy

- Source: https://github.com/w3c/rdf-tests (`rdf/rdf12` and `rdf/rdf11`),
  kept in that layout (`rdf12/…`, `rdf11/…`) so the RDF 1.2 manifests'
  `../../rdf11/…` includes resolve unchanged
- Commit: 5e5da96bb06c551ae3ac57d30fec8116f7161b9b (2026-10-02; vendored
  2026-10-03)
- Copyright: W3C. The suites' cover pages carry "Copyright © 2004-2026 World
  Wide Web Consortium"; the notice, the distribution statement and the
  disclaimer are reproduced in LICENSE.md. Some test files carry their own
  W3C copyright comment, kept unchanged. The RDF 1.2 manifests name the W3C
  RDF & SPARQL Working Group as creator.
- License: W3C 3-clause BSD License
  (https://www.w3.org/copyright/3-clause-bsd-license-2008/, full text in
  LICENSE.md). Upstream offers the suites under either the W3C Test Suite
  License or the W3C 3-clause BSD License, chosen per use. This copy relies on
  the BSD option because it is a subset: W3C's test-suite licence policy
  (https://www.w3.org/copyright/test-suites-licenses/) treats a subset as a
  derivative work, which the W3C Test Suite License does not permit. Not
  covered by this project's AGPL-3.0 + Commons Clause licence.
- Changes: none. Every vendored file (1901 files) is byte-identical to
  upstream at the commit above; the only difference is which files were
  copied (Scope). LICENSE.md and this file are ours.
- Scope: `rdf12/manifest.ttl` and, for each of N-Triples, N-Quads, Turtle,
  TriG and RDF/XML, the RDF 1.2 suite (`rdf12/rdf-n-triples`,
  `rdf12/rdf-n-quads`, `rdf12/rdf-turtle`, `rdf12/rdf-trig`,
  `rdf12/rdf-xml`: syntax, evaluation and N-Triples / N-Quads
  canonical-form tests) and the RDF 1.1 suite of the same format it includes
  (`rdf11/rdf-n-triples`, `rdf11/rdf-n-quads`, `rdf11/rdf-turtle`,
  `rdf11/rdf-trig`, `rdf11/rdf-xml`). Not vendored: the RDF 1.2 Semantics
  entailment tests (`rdf12/rdf-semantics`) and the RDF 1.1 Semantics tests
  they include (`rdf11/rdf-mt`), which test an entailment regime rather than
  a syntax; the other top-level manifests (`rdf11/manifest.ttl`); the
  implementation reports (`reports/`), cover-page templates
  (`template.haml`), archives (`TESTS.zip`, `TESTS.tar.gz`) and the
  `rdf11/rdf-xml/convert-manifest.rb` script.
- Runner: `tests/w3c_rdf12_manifests.rs` — walks `rdf12/manifest.ttl`
  through `mf:include` (the semantics include is recorded as not run), loads
  every entry through `TripleStore::load_str_with_base` (the upload path)
  into a fresh store, and checks syntax entries load or fail, evaluation
  entries against `mf:result` by isomorphism of the stored data, and
  canonical-form entries against the store's N-Triples / N-Quads export.
  Known gaps are tracked in the runner's KNOWN_FAILURES list (two-way
  ratchet) and in docs/conformance/rdf12.md. The results are development and
  regression results on this subset, not a W3C conformance claim, and no
  score is published for them.
