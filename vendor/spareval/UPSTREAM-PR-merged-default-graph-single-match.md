# Draft upstream PR: skip the merge's hash set when one graph matches

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead. Applies on top of #1920 (on `main`; backported here,
see `UPSTREAM-PR-merged-default-graph.md`).

**Target:** `main`.

## Title

spareval: stream a merged default graph's matches when only one graph has any

## Body

Since #1920 the default `QueryableDataset::internal_triples_for_pattern` keeps a
hash set of every matching triple of a default graph made of several graphs, so
that a triple held in two of them matches once. That costs a clone and a hash
insert per match, and memory for the whole match set, on every triple pattern of
the query — also when only one of the graphs has a match at all, where no
duplicate is possible.

That is the common shape of a dataset whose default graph is a large data graph
plus small side graphs (a shapes graph, a vocabulary): the pattern's predicate is
in one graph only. In a store comparison over BSBM (3.5M triples) served through
such a two-graph default graph, the merge's hash set was the largest single cost
of a correlated `NOT EXISTS` scan.

### Change

The per-graph iterators are made peekable and the ones with no first item are
dropped (one index probe each). With one graph left, its triples are streamed
directly; with more, the hash set is used as before. The union default graph
(`graph_names == None`) is unchanged.

### Tests

The `merged_default_graph*` tests pass unchanged: their four quads form three
triples across two graphs that both match, so the hash set is still used. A
pattern that matches in one graph only now returns the same rows without it.
