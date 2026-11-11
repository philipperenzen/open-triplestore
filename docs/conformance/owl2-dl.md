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
writes each premise, as sent, to `<dir>/<identifier>.nt` (and as the
functional syntax the Konclude bridge writes for it, to `<identifier>.ofn`), and
`OTS_TEST_W3C_OWL2_TIMEOUT_SECS=<n>` sets the per-check budget (default 60).

## Known failures

Each is classified under the Full rubric of [standards.md](../standards.md) (2026-10-06) as
(a) a defect in the test suite, with its issue.

| Test case | Why | Class |
|---|---|---|
| `New-Feature-Rational-002`, `New-Feature-Rational-003` | The premise's RDF list ends in `rdf:` (the namespace IRI) instead of `rdf:nil`. It is not a well-formed OWL 2 document, so the server refuses it as outside OWL 2 DL (422). | (a) [#505](https://github.com/philipperenzen/open-triplestore/issues/505) |
| `WebOnt-I5.26-001` | The premise holds a class expression that no axiom uses. The OWL 2 RDF mapping leaves its triples unparsed (Mapping to RDF Graphs §3.2.5: the graph must be empty at the end), so the server refuses the input as outside OWL 2 DL (422). | (a) [#505](https://github.com/philipperenzen/open-triplestore/issues/505) |

## WebOnt-description-logic-208 and -209

These two cases (k_poly, an ABox test from the DL98 systems comparison) were
known failures until [#506](https://github.com/philipperenzen/open-triplestore/issues/506):
both are consistency tests, 208 also a positive and 209 a negative entailment
test, and HermiT 1.4.5.519 did not decide the consistency of their premise in
60 s, nor in 900 s. They pass now, with each of their four checks taking
5.2–6.0 s. What was tried, on 2026-10-15, on the development host (x86_64, 24
logical CPUs, Docker Desktop), with the premises as the runner sends them
(`OTS_TEST_W3C_OWL2_DUMP`):

| Backend and configuration | Consistency of the premise |
|---|---|
| HermiT with its defaults: blocking strategy `OPTIMAL`, direct blocking `OPTIMAL`, blocking signature cache, existential strategy `CREATION_ORDER`, disjunction learning | no answer in 120 s |
| HermiT, blocking strategy `ANYWHERE`, `ANCESTOR`, `SIMPLE_CORE` or `COMPLEX_CORE` | no answer in 120 s |
| HermiT, direct blocking `SINGLE` or `PAIR_WISE`; without the blocking signature cache | no answer in 120 s |
| HermiT, existential strategy `INDIVIDUAL_REUSE` (also with `ANCESTOR` blocking) or `EL` | no answer in 120 s |
| HermiT **without disjunction learning** | consistent, 208 in 0.6 s and 209 in 0.3 s (the expected answer) |
| Konclude v0.7.0-1138 (`OTS_DL_BACKEND=konclude`), through the server | consistent, and the entailment checks right too: all checks of 208 in 0.8 s, of 209 in 0.2 s |

Disjunction learning makes HermiT try first the disjuncts of a disjunction
that led to fewer clashes so far. It changes only the order of the search, so
HermiT gives the same answers without it, but it cannot simply be switched
off: without it, the classification of the WebOnt-description-logic-905
premise did not finish in 20 s (0.5 s with it), and classifying and realising
the premises of the other cases took about 70 % longer in all. The real
ontologies tried (GALEN, LUBM, GoodRelations, BOT, DCAT and IMBOR) classify as
fast either way. The sidecar therefore keeps HermiT's defaults and, when they
have not decided consistency after 5 s, tries once without disjunction
learning (see [owl2-dl.md](../owl2-dl.md#what-the-bundled-sidecar-does)); the
checks of 208 and 209 are decided in that second try. A materialisation
(`/v1/reason`) of the k_poly premise, whose classification and realisation
then also run without disjunction learning, takes 35 s, within the default
limit of 300 s.

Konclude is not needed for these cases any more and is not run in CI, which
installs no Konclude binary. The Konclude Linux release binary also needs
`libpcre.so.3` (PCRE 1) at run time.

## Through Konclude

`w3c_owl2_dl_suite_against_konclude` runs the same cases through the
Konclude bridge (`OTS_DL_BACKEND=konclude`) when `OTS_TEST_KONCLUDE_BIN`
points at a binary; CI installs none. Konclude is an optional backend with
known engine limits, so the bar there is "never wrong": every check answers
as expected or `unknown`, never the opposite, and never fails outright; a
check that runs out of time (Konclude hangs now and then) counts as `unknown`.

The first such run, on 2026-10-23 with Konclude v0.7.0-1138, found wrong
answers. None was the mapping losing an axiom: each was reproduced on the
input's functional syntax with Konclude alone. They were:

| Cases | What Konclude did | Now |
|---|---|---|
| `New-Feature-AsymmetricProperty-001`, `New-Feature-IrreflexiveProperty-001`, and the WebOnt-description-logic cases whose individual has an inverse predecessor bounded by a maximum cardinality (`-008`, `-012`, `-013`, `-015`, `-017`, `-105`, `-612`, `-613`, `-615`, `-617`) | "consistent" for an inconsistent premise: two ABox optimisations lose the clash | right: the bridge turns those optimisations off |
| `New-Feature-DisjointDataProperties-*`, `New-Feature-Keys-*` | "consistent": disjoint data properties sharing a value, and keys, are not reasoned with | `unknown` |
| `one=two`, `WebOnt-description-logic-035` | "consistent": nominals with inverse properties and counting | `unknown` |
| `Datatype-Float-Discrete-001` | "consistent": floats taken as dense | `unknown` |
| `WebOnt-miscellaneous-203`, `-204` | "consistent": two different `rdf:XMLLiteral` values taken as one | `unknown` |
| `Datatype-DataComplementOf-001`, `WebOnt-I5.8-007` | a clash that is not there: the complement of a number datatype read as holding no number | `unknown` |
| `WebOnt-description-logic-661`, `-665` | crashed (SIGSEGV), or hung, at random on an entailment check that carries several knowledge bases | `unknown` (a hang: `unknown` once the time budget runs out); trying again usually answers right |

[owl2-dl.md](../owl2-dl.md#konclude-ots_dl_backendkonclude) has the
details, and `dl_konclude_live_still_misses_these_clashes` in
`tests/owl2_dl_conformance.rs` watches the engine for each kind.

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
- HermiT's disjunction learning kept it from deciding the k_poly cases
  (WebOnt-description-logic-208 and -209); the sidecar now retries a
  consistency test that HermiT's defaults have not decided after 5 s without
  it (above).
