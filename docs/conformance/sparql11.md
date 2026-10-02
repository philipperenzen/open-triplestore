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
result-format (`csv-tsv-res/`, `json-res/`) and entailment-regime
(`entailment/`) sections — surfaces these runners do not cover. The Graph
Store and SPARQL Protocol behaviour is pinned by
`tests/api_protocol_conformance.rs`. The federation sections are vendored and
run by their own runner (see [Federation](#federation) below).

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

## Federation

The federation sections — `service/` (query evaluation) and `syntax-fed/`
(positive syntax), through `manifest-sparql11-fed.ttl` — are vendored from the
same commit and run by
[`tests/w3c_sparql11_federation.rs`](../../tests/w3c_sparql11_federation.rs),
under the same licence terms: development and regression only, no score.

**What runs:** the suite names its remote endpoints by fixed IRIs
(`http://example.org/sparql`, `http://example1.org/sparql`, …) and gives each
one's data in a `qt:serviceData` block. The runner starts one local listener
per endpoint IRI, a SPARQL endpoint over a `TripleStore` loaded with that
block's data, and replaces the IRIs by the listeners' URLs in memory: in the
query, in the local and remote data and in the expected results. The files on
disk stay byte-identical to upstream. Only the listeners are on
`OTS_REMOTE_ALLOWLIST`, so the endpoint the suite expects to fail
(`http://invalid.endpoint.org/sparql`, under `SERVICE SILENT`) is refused by
the allowlist, never contacted. The listeners evaluate through `TripleStore`
as well, so a `SERVICE` nested inside a remote pattern is federated again
from the listener.

**Comparison level:** solution multisets by result-set isomorphism, as for the
query sections; syntax entries assert that the query parses.

**Known failures:** none. `service5` — `SERVICE ?service` with the endpoint
taken from the local data — runs since `SERVICE ?var` is evaluated as a
lateral join (see [federation.md](../federation.md#service-var-endpoints-named-by-the-data)).
