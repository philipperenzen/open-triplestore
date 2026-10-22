# Draft upstream PR: leave graphs without the pattern's predicate out of a union scan

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** `main` (on top of #1920), with the #1920 RocksDB path itself as a
backport to the 0.5 branch.

## Title

oxigraph: skip the graphs of a merged default graph that do not hold the pattern's predicate

## Body

#1920 evaluates a pattern over a default graph made of several graphs
(`DatasetView::internal_triples_for_pattern`, then
`quads_for_pattern_in_union`) by opening a scan on every graph and merging
them. A lookup the evaluator makes once per row of a for-loop join therefore
opens one RocksDB iterator per graph of the dataset. With a data graph merged
with a small side graph (shapes, a vocabulary) every such lookup costs about
twice what it costs over the data graph alone, although the side graph never
holds the lookup's predicate.

### Change

`DatasetView` keeps, for the lifetime of the evaluation, whether each graph of
the merge holds a quad with a given predicate (one probe of the graph's
predicate-first index per graph and predicate; at most 1,024 answers kept).
From the second scan with a predicate on, a pattern with that predicate is
scanned over the graphs that hold it only: a single graph left is streamed as
a single-graph scan, and none left is empty. The first scan opens every graph
as before, so a predicate a query looks up once pays no probe. The reader is a
snapshot, so an answer stays true for the evaluation. Patterns without a bound
predicate are unchanged.

### Tests

The `merged_default_graph*` tests (#1920) pass unchanged; they cover a graph
with no match, a predicate in one graph only and a triple held by two graphs.
In Open Triplestore's store comparison (BSBM 10k, 3.5M triples, a dataset
service whose default graph is the data graph plus its shapes graph), BSBM
explore Q5 took 1.6 times as long as over the data graph alone before the
change.
