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
`RDF` and `RDFS` regimes (`simple` runs no rules). A positive / negative entailment case passes
when the result graph, blank nodes read as variables, does / does not match the asserted and
derived triples; a result of `false` stands for an inconsistent action, which the engine must
report as a datatype clash. `OTS_TEST_W3C_RDF_MT_EXPLAIN=1` prints the result triples a failing
positive case does not match.

| Cases | Why they fail |
|---|---|
| `literal-type`, `pfps-10-non-well-formed-literal-1`, `xmlsch-02-whitespace-facet-3` | `rdfD1`, exempt by decision D11: the result has a blank node standing for a typed literal's value, which is not materialized |
| `datatypes-semantic-equivalence-between-datatypes` | equal values of different datatypes (an integer and a decimal) are not made interchangeable by the RDFS engine (the OWL 2 RL engine does this, `dt-eq`) |
| `datatypes-non-well-formed-literal-1` | the case recognizes no datatypes; this store always recognizes its datatype map, so the ill-typed literal is an inconsistency here |
| `rdfs-entailment-test001` | the lexical forms of `rdf:XMLLiteral` are not checked |
| `xmlsch-02-whitespace-facet-2`, `xmlsch-02-whitespace-facet-4` | an `xsd:int` lexical form with surrounding whitespace is ill-typed but not reported: integer-derived types are stored as `xsd:integer` values (decision D4) |


## SPARQL 1.1 entailment-regime cases

A case runs with the RDFS engine when its regimes include `RDFS`, `RDF` or `D`, with the OWL 2 RL
engine when they include only the OWL 2 RDF-Based Semantics, and is skipped when they name only
the OWL 2 Direct Semantics or RIF. The query runs over the RDF merge of the data and the derived triples (a set: a triple both
asserted and derived is one triple, as `/sparql?entailment=` evaluates it); its
solutions are compared with the expected ones as multisets, blank nodes compared as "some blank
node". Materialization is not the regimes' answer semantics, which restrict answers to terms of
the queried graph. `OTS_TEST_W3C_ENTAILMENT_EXPLAIN=1` prints the differing rows.

| Cases | Why they fail |
|---|---|
| `sparqldl-10`, `sparqldl-11` | the expected answers need OWL reasoning beyond the RL/RDF rules |
| `sparqldl-12` | an answer binds a blank-node class (a restriction); the regime answers only with terms that name things in the queried graph |
