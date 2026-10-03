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
request, and compare the whole resulting dataset. Every query-evaluation entry
then runs a second time on a store with the in-memory mirror built (the
parallel shards, the columnar copy and the full copy) and must end the same
way; a floor on the number of entries that ran with the mirror built keeps that
check from passing vacuously.

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

None is open, on the engine path or through the in-memory mirror.

The last ten failures were all in Oxigraph 0.5's SPARQL evaluator and optimizer
(spareval 0.2.7, sparopt 0.3.7), which this project now carries as a vendored,
patched copy ([`vendor/README.md`](../../vendor/README.md), one commit per fix,
each with a draft upstream PR in `vendor/spareval/UPSTREAM-PR-*.md`). They are
listed here so the history is not re-derived:

| Entry | Gap | Fix |
|---|---|---|
| `aggregates#agg-empty-group-count-graph`, `bindings#graph`, `negation#graph-minus` | `GRAPH ?g { … }` pushed `?g` into the patterns inside it, so around a pattern that binds no quads (an aggregate sub-select, a `VALUES` with `UNDEF`) the named graphs were not enumerated, and both sides of an inner `MINUS` shared `?g`. | Backport of upstream commit `fdc32b5` ([oxigraph#1905](https://github.com/oxigraph/oxigraph/issues/1905)), on oxigraph's main branch only. |
| `aggregates#agg-groupconcat-04`, `#agg-groupconcat-06` | `GROUP_CONCAT` kept a language tag shared by every input (`"1 2"@en`). An evaluator defect, not a spec-version divergence: SPARQL 1.1 §18.5.1.7 and the SPARQL 1.2 draft (§18.6.1.7, `CONCAT("", L1)`) both return an `xsd:string`. It was on oxigraph's own known-failures list. | `GroupConcatAccumulator` returns a simple literal. |
| `functions#bnode01` | `BNODE(str)` returned one blank node per string for the whole query (and across requests), and none for a string that is not a legal blank-node label; SPARQL 1.1 §17.4.2.9 requires a fresh node per solution. Not on oxigraph's own known-failures list. | A keyed hash of the string and the solution, with keys drawn per evaluation. |
| `property-path#zero_or_more_set_start/end`, `#zero_or_one_set_start/end` | A zero-length path whose constant start or end is absent from the dataset yielded no solution; the spec's zero-length path matches a term endpoint whatever the graph holds. | The path evaluator skips the graph-membership check for a constant endpoint. |

The same fork also makes a default graph built from several `FROM` graphs their
RDF merge, as SPARQL 1.1 §13.2 defines it: before, a triple held in two of them
matched twice ([oxigraph#1919](https://github.com/oxigraph/oxigraph/issues/1919),
backport of [#1920](https://github.com/oxigraph/oxigraph/pull/1920)). No entry of
the vendored sections exercises that; `tests/w3c_sparql11_conformance.rs`
(`dataset_from_graphs_merge_as_a_set`) pins it.
