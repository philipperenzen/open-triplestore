# RDF/RDFS and SPARQL entailment — test corpora and known gaps

Two sections of [w3c/rdf-tests](https://github.com/w3c/rdf-tests) are vendored unmodified at
commit `369a90d1`:

- the RDF 1.1 Semantics test cases (`rdf/rdf11/rdf-mt`), in
  [`tests/fixtures/w3c-rdf-mt/`](../../tests/fixtures/w3c-rdf-mt/PROVENANCE.md), run by
  `tests/w3c_rdf_mt_manifests.rs`;
- the SPARQL 1.1 entailment-regime tests (`sparql/sparql11/entailment`), in
  [`tests/fixtures/w3c-sparql11/entailment/`](../../tests/fixtures/w3c-sparql11/PROVENANCE.md),
  run by `tests/w3c_sparql11_entailment_manifests.rs`.

Both are subsets of a W3C test suite, used under the W3C 3-clause BSD License. W3C's
[test-suite licence policy](https://www.w3.org/copyright/test-suites-licenses/) allows no
performance claims for a subset, so no pass count or rate is published. Each runner is a two-way
ratchet: every case not in its `KNOWN_FAILURES` list must pass, every listed case must still
fail, and a pass floor catches loader regressions.

## RDF 1.1 Semantics cases

Each entry's action is loaded into a fresh store and materialized with the RDFS engine for the
`RDF` and `RDFS` regimes (`simple` runs no rules), recognizing exactly the datatypes the entry
lists in `mf:recognizedDatatypes` (plus `xsd:string` and `rdf:langString`, which every RDF 1.1
interpretation recognizes) and with `rdfD1` on. A positive / negative entailment case passes
when the result graph, blank nodes read as variables, does / does not match the asserted and
derived triples, its literals of recognized datatypes matched by value as the server's regimes
match them ([rdfs-entailment.md](../rdfs-entailment.md#equal-values-written-differently-d-entailment));
a result of `false` stands for an inconsistent action, which the engine must report as a
datatype clash. `OTS_TEST_W3C_RDF_MT_EXPLAIN=1` prints the result triples a failing positive
case does not match.

**No known failures** since 2026-10-06. The fourteen entries listed before passed once the
engine took the entry's datatypes as `D` (`datatypes-non-well-formed-literal-1`), applied
`rdfD1` (`literal-type`, `pfps-10-non-well-formed-literal-1`, `xmlsch-02-whitespace-facet-3`),
matched equal values written differently (`datatypes-semantic-equivalence-*`,
`double-infinity`, `double-round-same`, `float-infinity`, `float-round-same`), checked the
lexical forms of `rdf:XMLLiteral` (`rdfs-entailment-test001`) and stopped normalizing
whitespace before reading a lexical form (`xmlsch-02-whitespace-facet-2`, `-4`).

## SPARQL 1.1 entailment-regime cases

A case runs with the RDFS engine when its regimes include `RDFS`, `RDF` or `D`, with the OWL 2 RL
engine when they include only the OWL 2 RDF-Based Semantics, and is skipped when they name only
the OWL 2 Direct Semantics or RIF. The query runs over the data and the derived triples; its
solutions are compared with the expected ones as multisets, blank nodes compared as "some blank
node". Materialization is not the regimes' answer semantics, which restrict answers to terms of
the queried graph. `OTS_TEST_W3C_ENTAILMENT_EXPLAIN=1` prints the differing rows.

| Cases | Why they fail |
|---|---|
| `sparqldl-10`, `sparqldl-11`, `sparqldl-12` | outside the OWL 2 RL profile: each case's `sd:EntailmentProfile` names DL and Full (and EL for `sparqldl-10`), not RL. The runner answers OWL-RDF-Based cases with the RL engine, whose completeness the profile bounds (OWL 2 Profiles §4.3, Theorem PR1), so these are out of the RL grade's scope, not failures of it. In `sparqldl-12` an answer also binds a blank-node class (a restriction), where the regime answers only with terms that name things in the queried graph |
