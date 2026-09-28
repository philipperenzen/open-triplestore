# SPARQL 1.1 — rdf-tests regression ratchet and known gaps

The query and update sections of the W3C SPARQL 1.1 test suite, from the
[w3c/rdf-tests](https://github.com/w3c/rdf-tests) repository
(`sparql/sparql11`), are vendored unmodified under
[`tests/fixtures/w3c-sparql11/`](../../tests/fixtures/w3c-sparql11/PROVENANCE.md)
and run in CI via
[`tests/w3c_sparql11_manifests.rs`](../../tests/w3c_sparql11_manifests.rs).
It complements the hand-written, spec-derived suite in
`tests/w3c_sparql11_conformance.rs`, which stays as the platform's own
regression corpus (including the cx01–cx15 high-complexity cases).

This page describes how that run works and tracks the gaps it has found. It
publishes no score, on purpose. The vendored sections are a subset of a W3C
test suite, redistributed under the W3C 3-clause BSD License (see
[`LICENSE.md`](../../tests/fixtures/w3c-sparql11/LICENSE.md) there). W3C's
[test-suite licence policy](https://www.w3.org/copyright/test-suites-licenses/)
offers that licence for "software development, bug tracking, and other
applications that do not require assertions of performance to the public", and
a subset of a W3C test suite does not allow claims of performance or the use of
the name W3C without a special licence from W3C. So the run is used for
development and regression only, and no pass count, pass rate or conformance
claim is published for it.

The run covers the query evaluation, query syntax (positive and negative),
update evaluation and update syntax (positive and negative) entries of the two
top-level manifests.

**What runs:** every entry goes through `TripleStore` — the same evaluation
path `/sparql` uses — against a fresh in-memory store with the result cache
and the parallel mirror off. `qt:data` loads into the default graph, each
`qt:graphData` into the named graph of its resolved IRI; the query runs with
`BASE <query IRI>`. Update tests load `ut:data`/`ut:graphData`, run the
request, and compare the whole resulting dataset.

**Comparison level:** ASK by boolean; SELECT by result-set isomorphism (both
sides are encoded as an RDF graph in the DAWG result-set vocabulary and
canonicalised, so blank nodes are matched structurally; order is asserted only
when the query has an outer `ORDER BY`); CONSTRUCT/DESCRIBE by graph
isomorphism; updates by dataset isomorphism. Numeric literals are compared by
value (`"2.0"^^xsd:decimal` equals `"2"^^xsd:decimal`), because the engine
stores numerics natively and writes them back in canonical form; every other
literal by lexical form. Syntax tests assert parse success or failure only.

**Not vendored:** the protocol, service-description, graph-store-protocol,
federation (`service/`, `syntax-fed/`), result-format (`csv-tsv-res/`,
`json-res/`) and entailment-regime (`entailment/`) sections — surfaces this
runner does not cover. The Graph Store and SPARQL Protocol behaviour is
pinned by `tests/api_protocol_conformance.rs`.

**Gap policy:** a two-way ratchet. Every entry not listed in
`KNOWN_FAILURES` must pass, and every listed entry must still fail — silent
regressions *and* silent fixes both turn CI red, so the list cannot go stale.
A pass floor in the runner guards against a loader regression turning passes
into skips.

## Known failures (bug tracking)

Each entry below is a behaviour of the oxigraph 0.5 evaluator; none is in the
platform layer. They are listed here so the engine question can be revisited
with evidence rather than re-derived.

| Entry | Gap |
|---|---|
| `aggregates#agg-empty-group-count-graph`, `bindings#graph` | `GRAPH ?g { … }` around a pattern that binds no quads (an aggregate sub-select, a `VALUES` with `UNDEF`) does not enumerate the named graphs, so `?g` stays unbound. |
| `aggregates#agg-groupconcat-04`, `#agg-groupconcat-06` | `GROUP_CONCAT` keeps a language tag shared by every input (`"1 2"@en`), the SPARQL 1.2 rule; the 1.1 suite expects the plain literal. A spec-version divergence, not a defect. |
| `functions#bnode01` | `BNODE(str)` returns one blank node per string for the whole query; SPARQL 1.1 §17.4.2.9 requires a fresh node per solution. |
| `negation#graph-minus` | The outer `GRAPH ?g` variable is implied on both sides of an inner `MINUS`, so the sides share `?g` and are not disjoint. |
| `property-path#zero_or_more_set_start/end`, `#zero_or_one_set_start/end` | A zero-length path whose constant start or end is absent from the dataset yields no solution; the spec's zero-length path matches any term. |
