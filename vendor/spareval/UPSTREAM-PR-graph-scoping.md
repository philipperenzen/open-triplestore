# Draft upstream request: backport "SPARQL: Makes GRAPH evaluation conformant" to 0.5

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** the `0.5` maintenance branch (`spareval` 0.2.x, `sparopt` 0.3.x).
**Upstream change:** commit `fdc32b5a73326cb2358a580fe741e99a7b273c99` on `main`
(issue #1905), released only with the next major version.

## Title

Backport GRAPH scoping fix (fdc32b5, #1905) to the 0.5 branch

## Body

`GRAPH ?g { P }` is evaluated in 0.5.x by pushing `?g` into every quad pattern and
property path of `P` during the algebra conversion in `sparopt`. When `P` is not a
plain BGP this gives wrong answers, because SPARQL 1.1 §18.6 defines
`Graph(var, P)` as the union, over the named graphs `g`, of `Join(eval(D(g), P),
Ω(var = g))`: `?g` is not in scope inside `P`.

Three W3C SPARQL 1.1 query-evaluation tests fail on 0.5.11 for this reason (they are
on the 0.5 testsuite ignore list under "Our scoping of variables introduced by
GRAPH is wrong"):

- `bindings/manifest#graph` — `GRAPH ?g { VALUES (?g ?t) { (UNDEF …) } }`
- `aggregates/manifest#agg-empty-group-count-graph` — an aggregate sub-select
  that binds no quads inside `GRAPH ?g`
- `negation/manifest#graph-minus` — an inner `MINUS` sees `?g` on both sides

and so do the SPARQL 1.0 entries `graph/manifest#graph-variable-scope` and
`graph/manifest#graph-optional`.

`main` fixed this in fdc32b5 by keeping `Graph { graph_name, inner }` as a node of
the optimizer algebra and evaluating it in `spareval` as the spec does: the inner
pattern runs once per named graph (or once, for a bound or constant graph name)
with the active graph carried in the tuple, and `?g` is joined afterwards. The
optimizer pushes the `Graph` node back down onto quad patterns and paths where
that is equivalent, so plain `GRAPH ?g { ?s ?p ?o }` keeps its old plan.

The change applies to the 0.5 branch almost as is. The only adaptations are:

- `sparopt/src/optimizer.rs`, `estimate_graph_pattern_size`: 0.5 estimates sizes
  as `usize`, `main` as `u64` (`1_u64` → `1_usize`).
- `sparopt/src/algebra.rs`, the `OrderBy` arm of `from_sparql_algebra`, and
  `spareval/src/eval.rs`, the `Graph` arm of the explain formatter: context
  differences only (0.5 wraps ORDER BY keys in `Expression::Variable`).

The `sparopt` algebra change (`GraphPattern::Graph` gains `inner`,
`GraphPattern::Path` loses `graph_name`) is a public-API change of `sparopt`, but
`spareval` is its only user in the Oxigraph family, and `oxigraph` 0.5 pins both
with `=`, so a coordinated `spareval` 0.2.8 / `sparopt` 0.3.8 / `oxigraph` 0.5.12
release would carry it without breaking `oxigraph` users. If that is not
acceptable for a patch release, a `sparopt` 0.4.0 pinned by `spareval` 0.2.8 would
do the same.

### Testing

With the backport, the five W3C entries above pass, and they can be removed from
`testsuite/tests/sparql.rs`'s ignore list on the 0.5 branch, exactly as fdc32b5
does on `main`. We carry the backport in a vendored copy of `spareval` 0.2.7 and
`sparopt` 0.3.7 and run the W3C SPARQL 1.1 query and update sections through it
in CI; every other entry keeps its result.
