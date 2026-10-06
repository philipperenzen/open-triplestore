# RDF 1.2 — rdf-tests regression ratchet and known gaps

The W3C RDF 1.2 syntax test suites, from the
[w3c/rdf-tests](https://github.com/w3c/rdf-tests) repository (`rdf/rdf12`),
together with the RDF 1.1 suites each of them includes (`rdf/rdf11`), are
vendored unmodified under
[`tests/fixtures/w3c-rdf12/`](../../tests/fixtures/w3c-rdf12/PROVENANCE.md)
and run in CI via
[`tests/w3c_rdf12_manifests.rs`](../../tests/w3c_rdf12_manifests.rs). The
reference texts are the RDF 1.2 Candidate Recommendations of 7 April 2026
([Concepts](https://www.w3.org/TR/rdf12-concepts/),
[N-Triples](https://www.w3.org/TR/rdf12-n-triples/),
[N-Quads](https://www.w3.org/TR/rdf12-n-quads/),
[Turtle](https://www.w3.org/TR/rdf12-turtle/),
[TriG](https://www.w3.org/TR/rdf12-trig/),
[RDF/XML](https://www.w3.org/TR/rdf12-xml/)).

This page describes how that run works and tracks the gaps it has found. It
publishes no score, on purpose. The vendored files are a subset of W3C test
suites, redistributed under the W3C 3-clause BSD License (see
[`LICENSE.md`](../../tests/fixtures/w3c-rdf12/LICENSE.md) there). W3C's
[test-suite licence policy](https://www.w3.org/copyright/test-suites-licenses/)
offers that licence for "software development, bug tracking, and other
applications that do not require assertions of performance to the public", and
a subset of a W3C test suite does not allow claims of performance or the use of
the name W3C without a special licence from W3C. So the run is used for
development and regression only, and no pass count, pass rate or conformance
claim is published for it.

The run covers N-Triples, N-Quads, Turtle, TriG and RDF/XML: positive and
negative syntax, evaluation, and the N-Triples / N-Quads canonical form, for
RDF 1.2 (triple terms, reifiers and annotations, base direction, `VERSION`)
and for the RDF 1.1 suites the RDF 1.2 manifests include.

**What runs:** every entry loads its input through
`TripleStore::load_str_with_base` — the path uploads and the Graph Store
protocol take — into a fresh in-memory store, with the file's own IRI as the
base (the suites' rule for relative IRI resolution). Syntax entries must load
or must fail to load. Evaluation entries compare what the store then holds
with `mf:result` by dataset isomorphism (blank nodes matched structurally,
also inside triple terms; literals compared exactly — lexical form, language
tag, base direction and datatype). Canonical-form entries compare the store's
own N-Triples / N-Quads export of the loaded data with `mf:result`, line for
line.

**Not run:** the RDF 1.2 Semantics tests (`rdf12/rdf-semantics`) and the RDF
1.1 Semantics tests they include (`rdf11/rdf-mt`). They test entailment
regimes, not syntax; they are not vendored, and the runner records the
include as not run. RDFS entailment has its own spec-derived suite
(`tests/rdfs_conformance.rs`).

**Gap policy:** a two-way ratchet. Every entry not listed in
`KNOWN_FAILURES` must pass, and every listed entry must still fail — silent
regressions *and* silent fixes both turn CI red, so the list cannot go stale.
No entry may be skipped, and a pass floor in the runner guards against a
loader regression turning passes into failures.

## Known failures (bug tracking)

Checked against the W3C issue trackers on 2026-10-03. Triple terms,
reifiers, annotations, base direction, `VERSION`, non-canonical numbers and
the canonical N-Triples and N-Quads forms all pass; an entry fails only where
its literal is an `rdf:XMLLiteral`, and those wait on an open W3C issue:

| Entries | Gap |
|---|---|
| `rdf11/rdf-xml#xml-canon-test001`, `#xml-canon-test002`; `rdf12/rdf-xml/eval#rdf12-xml-an-13`, `#rdf12-xml-an-14` | `rdf:parseType="Literal"`: oxrdfxml keeps the in-scope namespace declarations on the literal's root element (`"<br xmlns:rdf=… xmlns:eg=…></br>"`). The suite expected that form from June 2025 and went back to `"<br></br>"` on 2026-04-20 (rdf-tests d974697, and 7633586 on 2026-05-31 for the RDF 1.2 entries); the canonical form of `rdf:XMLLiteral` is open W3C issue [w3c/rdf-xml#97](https://github.com/w3c/rdf-xml/issues/97) (opened 2026-04-20). Full rubric: (b), tracked in [#504](https://github.com/philipperenzen/open-triplestore/issues/504). |

Until 2026-10-03, 22 more Turtle and TriG entries failed (`bareword_decimal`,
`bareword_double`, `double_lower_case_e`, `numeric_with_leading_0`,
`positive_numeric`, `turtle-subm-11`, `-17`, `-19`, `-20`, `-26`, the same ten
in TriG, and `rdf12/rdf-turtle/eval#turtle12-rt-09`, `#turtle12-tt-05`):
storage kept numeric literals as values, so `1.0`, `1e0`, `+1` or `01` read
back in canonical form. The store now keeps every literal as written
([`vendor/README.md`](../../vendor/README.md)), and they pass. The remaining
entries are blocked on the open W3C issue, so they do not count against the
grade ([Standards](../standards.md) note 1).
