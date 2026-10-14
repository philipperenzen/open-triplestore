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

Every known failure of the OWL 2 corpus is a positive entailment case whose conclusion the
RL/RDF rules do not derive; the engine computes the rules' closure exactly (the differential
test), so these are limits of the rule set rather than of the implementation. The runner's
`KNOWN_FAILURES` list gives each case its reason; they fall into these groups:

| Group | Cases | Why the rules do not derive it |
|---|---|---|
| `owl:differentFrom` / `owl:AllDifferent` conclusions | `owl2-rl-rules-fp-differentFrom`, `owl2-rl-rules-ifp-differentFrom`, `WebOnt-differentFrom-001`, `New-Feature-DisjointObjectProperties-001/002`, `New-Feature-DisjointDataProperties-002` | no RL/RDF rule concludes `owl:differentFrom` (not even its symmetry) or `owl:AllDifferent` |
| New class expressions | `DisjointClasses-001/003`, `New-Feature-ObjectQCR-002`, `WebOnt-I5.5-005` | the conclusion uses a complement or union the premise does not contain; the rules introduce no class expressions (Theorem PR1 covers assertions) |
| Schema conclusions outside the rules | `chain2trans1` (`owl:TransitiveProperty` from a chain), `WebOnt-I5.26-010` (an `owl:minCardinality` restriction), `WebOnt-I5.8-006/008/009` (datatype ranges from datatype intersections) | no rule concludes these axioms |
| Outside the profile | `New-Feature-ReflexiveProperty-001` | `owl:ReflexiveProperty` is not OWL 2 RL |
| Annotations in the conclusion | `WebOnt-I4.6-005-Direct`, `WebOnt-equivalentClass-008-Direct` | the conclusion carries an annotation the premise lacks; annotations have no Direct Semantics meaning, but the runner matches triples |

Cases without an RDF/XML premise or conclusion are skipped by the runner.

Under the Full rubric of [standards.md](../standards.md) (checked again on 2026-10-06), none of
these is a failure of the RL grade: RL is graded against Theorem PR1, and every case above is
outside its scope (below), so each stays listed with that reason and needs no issue. The three
`sparqldl-*` cases of the SPARQL 1.1 entailment-regime runner are outside the profile too: their
`sd:EntailmentProfile` does not name RL ([entailment.md](entailment.md)).

### Theorem PR1

OWL 2 Profiles §4.3, Theorem PR1, is the completeness the RL/RDF rules promise: for an OWL 2 RL
premise (no IRI used both as an individual and as a class or property), they derive every
entailed assertion of the forms `ClassAssertion` with a named class, `ObjectPropertyAssertion`,
`DataPropertyAssertion` and `SameIndividual`. None of the known failures is inside that scope:

- the `owl:differentFrom` / `owl:AllDifferent` conclusions are `DifferentIndividuals`, which
  is not among the forms (no rule concludes it, so the theorem could not hold otherwise);
- `New-Feature-ObjectQCR-002` concludes a `ClassAssertion` whose class is a complement, not a
  named class; the other class-expression and schema cases conclude class or property axioms;
- the two annotation cases conclude annotation assertions;
- `New-Feature-ReflexiveProperty-001` concludes an `ObjectPropertyAssertion`, a form PR1
  covers, but its premise declares an `owl:ReflexiveProperty`, which the OWL 2 RL grammar
  (§4.2) does not allow, so the premise is no OWL 2 RL ontology even though the test case is
  tagged `test:profile test:RL`. It stays a known failure; supporting reflexive properties in
  the RL engine would go beyond the rule set (the OWL 2 DL backends handle them).

