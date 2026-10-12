# Draft upstream request: backport "Deduplicate merged SPARQL default graphs" to 0.5

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** the `0.5` maintenance branch (`spareval` 0.2.x).
**Upstream change:** PR #1920 (merge commit `7ce152a1d910d5662027a5bcbe7c32cee0a4e059`
on `main`), closing #1919; released only with the next major version.

## Title

Backport #1920 (deduplicate merged default graphs, #1919) to the 0.5 branch

## Body

On 0.5.11, a default graph built from several graphs (`FROM <a> FROM <b>`, `USING`
twice, or `set_default_graph` with more than one graph, and the union default
graph) is the concatenation of the graphs, not their RDF merge (SPARQL 1.1 Query
§13.2, §18.3). A triple held in two of the graphs matches twice, which inflates
row counts, `COUNT` and `SUM`, and `INSERT … USING` persists the inflated totals.

#1920 fixed this on `main` with a defaulted `QueryableDataset::internal_triples_for_pattern`
that hash-deduplicates the triples of a merged scan, plus a RocksDB-specific
override in `oxigraph`. The `spareval` half applies to the 0.5 branch unchanged
apart from two import lines, and is enough on its own: an implementation that does
not override the new method gets the deduplicating default, so `oxigraph` 0.5.x
gives correct answers with no change, and the RocksDB fast path can follow
separately (or stay on `main`).

Adaptations for 0.5:

- `spareval/src/dataset.rs`, `spareval/src/eval.rs`: import lines only (0.5 has no
  `oxstr`).
- `spareval/src/lib.rs`: the new unit test `merged_default_graph_propagates_read_errors`
  depends on a `FailingDataset` test helper that only exists on `main`, so it is
  left out; `merged_default_graph_streams_and_honors_cancellation` and the
  documentation examples are kept.

The only API change is additive: the `InternalTriple` type and the defaulted trait
method. Merged scans keep a hash set of the unique matching triples for the
iterator's lifetime; single-graph queries keep the old fast path.

### Testing

The `#1920` regressions (`merged_default_graph*` in `testsuite/oxigraph-tests/sparql`)
pass with the backport. We carry it in a vendored `spareval` 0.2.7 and check it
with a query (`COUNT = 3`, `SUM = 40` over four quads forming three triples), a
self-join and an `INSERT … USING` case, through both `oxigraph::store::Store` and
our own column-store copy of the data.
