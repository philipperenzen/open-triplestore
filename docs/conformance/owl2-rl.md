# OWL 2 RL — test corpora and known gaps

Two kinds of evidence back the OWL 2 RL grade in [standards.md](../standards.md) (footnote 14):

1. **A differential test** (`engine_agrees_with_the_reference_evaluator_on_random_graphs` in
   `tests/owl2_rl_conformance.rs`). A test-only evaluator, `tests/support/rl_reference.rs`,
   applies the 78 RL/RDF rules of OWL 2 Profiles §4.3 naively over *generalized* triples, so
   `dt-type2`, `dt-eq` and `dt-diff` run as written, with literal subjects. It shares no code
   with the engine. For seeded random graphs (classes, object and data properties, restrictions,
   lists, keys, negative assertions, literals of several datatypes) the engine must agree with it
   on consistency and, when consistent, write exactly the RDF-representable part of its closure.
   Reflexive `owl:sameAs` triples are left out of the comparison: they are `eq-ref`'s, which is
   opt-in (decision D2). `OTS_RL_DIFF_SEEDS=<n>` runs more graphs.
2. **The approved W3C OWL 2 test cases of the RL profile**, from
   [`tests/fixtures/w3c-owl2/`](../../tests/fixtures/w3c-owl2/PROVENANCE.md), run by
   `tests/w3c_owl2_rl_manifests.rs`, and the OWL 2 RDF-Based Semantics cases of the SPARQL 1.1
   entailment-regime tests (see [entailment.md](entailment.md)).

No score is published for either corpus: W3C's
[test-suite licence policy](https://www.w3.org/copyright/test-suites-licenses/) allows no
performance claims for a subset or partial run. The runners are two-way ratchets: every case not
listed below must pass, and every listed case must still fail.

## How the OWL 2 cases run

Each approved case marked `test:profile test:RL` with an RDF/XML premise is loaded (with its
imported ontologies) into a fresh store and materialized with the RL engine. A consistency /
inconsistency case checks the run's outcome; a positive / negative entailment case matches the
conclusion graph, blank nodes read as variables, against the asserted and derived triples. The
conclusion's ontology header is not part of what is matched. `OTS_TEST_W3C_OWL2_RL_EXPLAIN=1`
prints the conclusion triples a failing case does not match.

## Known gaps

<!-- filled from the runner's KNOWN_FAILURES -->
