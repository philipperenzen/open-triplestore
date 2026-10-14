# OWL 2 DL — W3C test cases against the reasoner sidecar

The approved test cases of the W3C OWL 2 Test Case Repository, which
[OWL 2 Conformance](https://www.w3.org/TR/owl2-conformance/) §3.3 refers to,
are vendored unmodified as
[`tests/fixtures/w3c-owl2/approved/all.rdf`](../../tests/fixtures/w3c-owl2/PROVENANCE.md).
[`tests/w3c_owl2_dl_manifests.rs`](../../tests/w3c_owl2_dl_manifests.rs) runs
the OWL 2 DL / Direct Semantics ones in CI against the reasoner sidecar the
project ships (OWL API + HermiT, `sidecars/reasoner/`; see
[owl2-dl.md](../owl2-dl.md#reasoner-sidecar-ots_dl_backendsidecar)). It
complements the hand-written suite in `tests/owl2_dl_conformance.rs`, whose
`sidecar_live` tests run in the same CI step.

This page describes how the run works and lists the cases it does not pass. It
publishes no score, on purpose. The file carries no licence of its own, so it
is redistributed under the W3C Document License (see
[`LICENSE.md`](../../tests/fixtures/w3c-owl2/LICENSE.md)), and W3C allows no
performance claims based on part of a test suite. The run uses only the OWL 2
DL / Direct Semantics cases, so it is used for development and regression
only: no pass count, pass rate or conformance claim is published for it.

## What runs

The runner selects every test case with `test:status test:Approved`,
`test:species test:DL` and `test:semantics test:DIRECT`. Each one goes through
`POST /api/reasoning/check`, the endpoint a user's check takes: the server's
own RDF → OWL 2 mapping and profile check first, then the sidecar.

| Test type | Request | Must answer |
|---|---|---|
| `test:ConsistencyTest` | `task: consistency` | `true` |
| `test:InconsistencyTest` | `task: consistency` | `false` |
| `test:PositiveEntailmentTest` | `task: entailment`, the conclusion | `true` |
| `test:NegativeEntailmentTest` | `task: entailment`, the non-conclusion | `false` |

A case with several types passes when every one of them does.

**Inputs.** Each premise and conclusion is the case's RDF/XML document,
converted to N-Triples in memory (the endpoint reads Turtle). oxigraph reads
only double-quoted `<!ENTITY>` declarations, so the runner swaps apostrophes
for quotes in those declarations, in memory; the vendored file is never
changed. The server never follows `owl:imports`, so for a case that imports
another ontology the runner adds the imported documents, which the manifest
carries, to the premise itself: the imports closure.

**Skipped by the runner.** Cases whose only premise is in functional-style
syntax or OWL/XML cannot be posted to an RDF endpoint and are skipped.

**Ratchet.** Every case not listed below must pass, every listed case must
still fail, and a pass floor guards against a loader regression turning
passes into skips. Each call has a 60 s limit.

**Running it.** It needs a running sidecar:

```bash
OTS_TEST_REASONER_URL=http://127.0.0.1:8090 OTS_TEST_REASONER_TOKEN=<token> cargo test --features full,test-utils --test w3c_owl2_dl_manifests
```

Without `OTS_TEST_REASONER_URL` the run is skipped (`w3c_owl2_dl_manifest_selection`,
which checks the selection, runs everywhere). `OTS_TEST_W3C_OWL2_DUMP=<dir>`
writes each premise, as sent, to `<dir>/<identifier>.nt`, and
`OTS_TEST_W3C_OWL2_TIMEOUT_SECS=<n>` sets the per-check budget (default 60).

## Known failures

Each is classified under the Full rubric of [standards.md](../standards.md) (2026-10-06): (a) a
defect in the test suite, or (c) a limit of an external engine, with its issue.

| Test case | Why | Class |
|---|---|---|
| `New-Feature-Rational-002`, `New-Feature-Rational-003` | The premise's RDF list ends in `rdf:` (the namespace IRI) instead of `rdf:nil`. It is not a well-formed OWL 2 document, so the server refuses it as outside OWL 2 DL (422). | (a) [#505](https://github.com/philipperenzen/open-triplestore/issues/505) |
| `WebOnt-I5.26-001` | The premise holds a class expression that no axiom uses. The OWL 2 RDF mapping leaves its triples unparsed (Mapping to RDF Graphs §3.2.5: the graph must be empty at the end), so the server refuses the input as outside OWL 2 DL (422). | (a) [#505](https://github.com/philipperenzen/open-triplestore/issues/505) |
| `WebOnt-description-logic-208`, `WebOnt-description-logic-209` | HermiT does not decide consistency within the 60 s limit, so the answer is unknown (504). With a 900 s budget (`OTS_TEST_W3C_OWL2_TIMEOUT_SECS=900`, 2026-10-06) it still had no answer, so a larger CI budget would not help. | (c) HermiT, [#506](https://github.com/philipperenzen/open-triplestore/issues/506) |

## What the run found

Building the run surfaced these problems, all fixed:

- The OWL API needs `javax.xml.bind` to read a blank-node ontology header, and
  Java 11 and later no longer bundle it.
- HermiT 1.4.5.519 crashes on `owl:Thing owl:equivalentClass owl:Nothing` with
  OWL API 5.1.20; the sidecar pins OWL API 5.1.9, the version HermiT is built
  against.
- OWL API 5.1.9 reports some triples as unparsed although it read them (a
  class assertion of a blank-node class expression), and does not read
  `rdf:type owl:Nothing` on a blank node or annotations of ontology
  annotations; the sidecar accounts for each.
- OWL API 5.1.9 cannot create two ontology managers at once (a
  `ConcurrentModificationException` in its injector), which made concurrent
  requests fail now and then; the sidecar creates them one at a time.
- HermiT's own entailment check answered `false` for an entailed class
  assertion until the ABox had been realised; the sidecar now checks
  entailment by reduction to class satisfiability.
- The OWL API does not apply the OWL 1 DL compatibility tables of the RDF
  mapping (Tables 14, 15 and 18: one-member and empty `owl:intersectionOf` /
  `owl:unionOf` lists), and the server's mapping refused a named class with
  several `owl:oneOf` lists and OWL 1's `owl:DataRange`.
- The OWL API read an undeclared predicate between two IRIs as an annotation;
  the sidecar now types undeclared properties by use as the server does.
