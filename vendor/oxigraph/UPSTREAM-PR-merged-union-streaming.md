# Draft upstream PR: stream the merged default graph without keys once one scan is left

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** `main` (on top of #1920), with the #1920 RocksDB path itself as a
backport to the 0.5 branch.

## Title

oxigraph: skip the merge keys of a union scan when only one graph is left

## Body

#1920 evaluates a pattern over a default graph made of several graphs
(`RocksDbStorageReader::quads_for_pattern_in_union`) as a k-way merge of the
per-graph scans, which share a triple order, deduplicating by key. Every quad
pays the encoding of its key, even when every other graph has no match — the
common shape of a data graph merged with a small side graph (shapes,
vocabulary) that the pattern's predicate is not in.

### Change

`RocksDbMergedDecodingQuadIterator` records which scans are exhausted. Once only
one is left and its pending quad is emitted, the rest of that scan is streamed
as it is: it holds no repeat, and nothing smaller than what was already
emitted. A single-graph union streams from the start. The memory backend's
union drops the graphs with no match (one probe each) and streams a single
remaining graph without its hash set.

### Tests

The `merged_default_graph*` tests (#1920) pass unchanged. In Open
Triplestore's store comparison (BSBM 10k, a dataset service whose default
graph is the data graph plus its shapes graph), `SELECT (COUNT(*) AS ?n)
WHERE { ?s ?p ?o }` and the patterns of a correlated `NOT EXISTS` stream
without keys.
