# Draft upstream PR: optimize the pattern of EXISTS like any other pattern

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** `main`, with a backport to the `0.5` branch.

## Title

sparopt: reorder joins and push filters inside EXISTS

## Body

`Optimizer::optimize_graph_pattern` runs `normalize_pattern`, `push_graph`,
`reorder_joins` and `push_filters`. Only the first of them looks into
expressions (`normalize_expression` normalizes the pattern of an `EXISTS`); the
other three stop at a `Filter`'s, `LeftJoin`'s or `Extend`'s expression. So the
pattern of every `EXISTS` keeps the join order it was written in, every `Join`
in it keeps the `JoinAlgorithm` the conversion from `spargebra` gave it — a
`HashBuildLeftProbeRight` with no keys, i.e. a cartesian product filtered for
compatibility — and its filters stay at the top.

spareval evaluates `EXISTS { P }` once per outer solution with that solution as
input. For

```sparql
SELECT (COUNT(*) AS ?n) WHERE {
  ?offer bsbm:product ?product
  FILTER NOT EXISTS { ?review bsbm:reviewFor ?product ; bsbm:rating1 ?r FILTER (?r >= 9) }
}
```

each outer row evaluates both triple patterns of `P` in full — the second one
does not mention `?product`, so it reads every `bsbm:rating1` triple — and joins
them as a cartesian product. On the BSBM data with 1,000 products (20k offers,
10k reviews) Oxigraph 0.5.11 took 112 s; with 10,000 products it did not finish
in 300 s.

### Change

A last pass, `optimize_exists`, walks the optimized pattern and, for every
`EXISTS` in a `Filter`, `LeftJoin`, `Extend` or `OrderBy` expression, runs
`reorder_joins` and `push_filters` on its pattern with the variable types of the
pattern the expression is evaluated on as input types. The variables the outer
pattern binds are therefore known to be bound: the part of `P` that uses them is
chosen first, and with `sep-0006` the rest becomes a lateral (for-loop) join
driven by it. In the query above `P` becomes
`Filter(Lateral(?review bsbm:reviewFor ?product, ?review bsbm:rating1 ?r), ?r >= 9)`:
one index lookup from `?product`, one per review found.

It also adds `Optimizer::optimize_for_standalone_evaluation`, the same two
passes with no input types, for an evaluator that wants to evaluate an `EXISTS`
pattern once for all outer solutions (see the companion spareval draft,
`UPSTREAM-PR-exists-semi-join.md`).

### Tests

The W3C negation and exists tests are unchanged. On the query above (BSBM 1k)
the evaluation time drops from 112 s to well under a second; the companion
spareval change brings it to tens of milliseconds.
