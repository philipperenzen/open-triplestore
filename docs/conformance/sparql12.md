# SPARQL 1.2 — rdf-tests regression ratchet and known gaps

The W3C SPARQL 1.2 test suite, from the
[w3c/rdf-tests](https://github.com/w3c/rdf-tests) repository
(`sparql/sparql12`), is vendored unmodified under
[`tests/fixtures/w3c-sparql12/`](../../tests/fixtures/w3c-sparql12/PROVENANCE.md)
and runs in CI via
[`tests/w3c_sparql12_manifests.rs`](../../tests/w3c_sparql12_manifests.rs).
It complements the hand-written pins in `tests/sparql12_conformance.rs`.
The reference text is the
[SPARQL 1.2 Query Language Working Draft of 1 October 2026](https://www.w3.org/TR/sparql12-query/);
the suite is maintained by the W3C RDF & SPARQL Working Group and changes
with the drafts, so the vendored copy is pinned to one commit (see
`PROVENANCE.md`).

This page describes how that run works and tracks the gaps it has found. It
publishes no score, on purpose. The vendored files are a subset of a W3C test
suite (the implementation reports and the cover-page template are left out),
redistributed under the W3C 3-clause BSD License (see
[`LICENSE.md`](../../tests/fixtures/w3c-sparql12/LICENSE.md) there). W3C's
[test-suite licence policy](https://www.w3.org/copyright/test-suites-licenses/)
offers that licence for "software development, bug tracking, and other
applications that do not require assertions of performance to the public", and
a subset of a W3C test suite does not allow claims of performance or the use of
the name W3C without a special licence from W3C. So the run is used for
development and regression only, and no pass count, pass rate or conformance
claim is published for it.

The run covers every section the suite's `manifest.ttl` includes: triple-term
syntax (positive and negative) and evaluation, reified triples and
annotations, base direction and the `LANGDIR` family, `VERSION`, codepoint
escapes, grouping, expressions, and the RDF 1.1-model checks in `rdf11/`.

**What runs:** every entry goes through `TripleStore` — the same evaluation
path `/sparql` uses — against a fresh in-memory store with the result cache
off. `qt:data` loads into the default graph (a TriG file keeps its named
graphs), each `qt:graphData` into the named graph of its resolved IRI; the
query runs with `BASE <query IRI>`. Every query-evaluation entry runs twice:
on the engine alone, and through the in-memory mirror (four subject shards,
the columnar copy and the full copy, built with no debounce) that answers the
server's reads by default. Both runs must end the same way. Update tests load
`ut:data`/`ut:graphData`, run the request, and compare the whole resulting
dataset.

**Comparison level:** ASK by boolean; SELECT by result-set isomorphism (both
sides are encoded as an RDF graph in the DAWG result-set vocabulary and
canonicalised, so blank nodes are matched structurally, also inside triple
terms; order is asserted only when the query has an outer `ORDER BY`);
CONSTRUCT/DESCRIBE by graph isomorphism; updates by dataset isomorphism.
Numeric literals are compared by value (`"2.0"^^xsd:decimal` equals
`"2"^^xsd:decimal`), at any depth inside triple terms, because the engine
stores numerics natively and writes them back in canonical form; an entry that
declares `mf:requires mf:NoCanonicalizationOfNumerics` is compared exactly.
Every other literal is compared by lexical form, language tag, base direction
and datatype. Syntax tests assert parse success or failure only.

**Gap policy:** a two-way ratchet. Every entry not listed in
`KNOWN_FAILURES` must pass, and every listed entry must still fail — silent
regressions *and* silent fixes both turn CI red, so the list cannot go stale.
No entry may be skipped, and a pass floor in the runner guards against a
loader regression turning passes into failures.

## Known failures (bug tracking)

Checked against the W3C issue trackers on 2026-10-03. No entry below is
blocked by an *open* Working Group issue: the grammar questions behind the
triple-term entries were settled when w3c/sparql-query#282 and #283 closed
(2025-12-26), and the rule behind `select-variable-reuse` came with
w3c/sparql-query PR #380 (closed 2026-05-28). Each is a behaviour of the
oxigraph 0.5.11 parser (spargebra 0.4.7) or of its storage; none is in the
platform layer. They keep the SPARQL 1.2 grade at Partial
([Standards](../standards.md) note 1).

| Entry | Gap |
|---|---|
| `syntax-triple-terms-negative#tripleterm-subject-03`, `#tripleterm-subject-06` | The parser accepts a triple term or a literal as the subject of a triple-term expression (`BIND(<<( "literal" :q :z )>> AS ?X)`). The 1.2 grammar forbids both since #282/#283 closed. |
| `syntax#nested-aggregate-functions` | The parser accepts an aggregate inside an aggregate's argument (`COUNT(COUNT(*))`), which SPARQL 1.1 erratum query-5 and SPARQL 1.2 forbid. |
| `grouping#select-variable-reuse` | The parser rejects a SELECT expression that uses the variable an earlier SELECT expression of the same aggregating query binds (`(COUNT(?v) AS ?count) (?count + 1 AS ?countPlusOne)`), which SPARQL 1.2 allows. |
| `grouping#group01` | The entry declares `mf:requires mf:NoCanonicalizationOfNumerics`. Storage keeps numerics as values, so `"001"^^xsd:integer` reads back as `"1"^^xsd:integer` and groups with it. The lexical-form storage change fixes it. |

One defect the suite does not reach is pinned by a flip-when-fixed test in
`tests/sparql12_conformance.rs`: `=` between two literals that both carry a
base direction (`"abc"@en--ltr = "abc"@en--rtl`) panics inside the evaluator
(spareval 0.2.7 has no equality arm for directional strings), so such a query
fails with a server error instead of answering `false`.

## Hand-written pins

`tests/sparql12_conformance.rs` runs every pin on the engine and on the
in-memory mirror. Beyond the triple-term cases it pins `VERSION`, the
`LANGDIR` family and base direction as part of the term, `~ reifier` and
`{| |}` annotations in `INSERT DATA` and in query patterns, and the rejection
of duplicate `VALUES` variables; and, for the two SEP extensions oxigraph
compiles in (not part of the Working Draft), `LATERAL` with a per-row
`LIMIT 1` and `ADJUST` with an `xsd:dayTimeDuration`.
