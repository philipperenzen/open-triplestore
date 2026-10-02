# TopQuadrant SHACL API — SHACL-AF tests, vendored copy

- Source: https://github.com/TopQuadrant/shacl
  (`src/test/resources/sh/tests/{expression,function,rules,target}`)
- Commit: 6687b48bd2c81eda369f224598061f87dce0d425 (2026-06-30; vendored 2026-10-02)
- Licence: Apache License 2.0 — see `LICENSE.md`. Not covered by this project's
  AGPL-3.0 + Commons Clause licence.
- Content: the four directories that test SHACL Advanced Features (SHACL-AF
  Note, 2017): expression constraints (`expression/`), `sh:SPARQLFunction`
  (`function/`), `sh:SPARQLRule` and `sh:TripleRule` with node expressions
  (`rules/sparql/`, `rules/triple/`) and `sh:SPARQLTarget` (`target/`) —
  10 `*.test.ttl` cases plus `rules/triple/person.ttl`, which two of them
  `owl:import`. The repository's other test directories (`core/`, `sparql/`,
  `shapedefs/`, `js/` …) cover SHACL Core and SHACL-SPARQL, which the vendored
  W3C suite (`tests/fixtures/w3c-shacl/`) already covers, and were not copied.
- Changes: none. Every file is byte-identical to upstream at the commit above,
  at the same path relative to `src/test/resources/sh/tests/`.
- Upstream `LICENSE` is `LICENSE-Apache-2.0.txt` here and upstream `NOTICE` is
  `NOTICE`, both byte-identical.
- Runner: `tests/shacl_af_corpus.rs` (comparison levels in its header and in
  `docs/conformance/shacl.md`). Its results are development and regression
  results on these files, not a conformance claim.
