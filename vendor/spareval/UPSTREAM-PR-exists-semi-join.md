# Draft upstream PR: evaluate a correlated EXISTS as a hash semi-join

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead. Depends on the sparopt draft
`vendor/sparopt/UPSTREAM-PR-exists-join-order.md` (it uses
`Optimizer::optimize_for_standalone_evaluation`).

**Target:** `main`, with a backport to the `0.5` branch.

## Title

spareval: answer EXISTS from a hash table when the outer rows outnumber its cost

## Body

`build_exists` evaluates the pattern of `EXISTS { P }` once per outer solution,
with the solution as input. Even with a good plan for `P` that is one index
probe or more per outer row; with 200k outer rows on RocksDB it is seconds,
where a hash anti-join on the shared variables — evaluate `P` once, hash its
solutions on the variables it shares with the outer pattern, look each row up —
is one scan of `P` and one hash lookup per row. Jena, QLever and Virtuoso answer
the BSBM query below in 23–1700 ms at 10k products; Oxigraph did not finish in
300 s.

```sparql
SELECT (COUNT(*) AS ?n) WHERE {
  ?offer bsbm:product ?product
  FILTER NOT EXISTS { ?review bsbm:reviewFor ?product ; bsbm:rating1 ?r FILTER (?r >= 9) }
}
```

### When the table gives the same answer

Evaluating `P` with the outer solution `μ` as input yields the solutions of `P`
compatible with `μ` when every variable `μ` binds that `P` mentions is

- bound in every solution of `P` (so compatibility pins its value), and
- read by a `FILTER` or an `OPTIONAL` only where that operator's own inner
  pattern binds it (otherwise the per-row evaluation sees it bound and the
  standalone one unbound).

`exists_semi_join_slots` checks this on the plan: it accepts basic graph
patterns, property paths, joins, lateral joins, `OPTIONAL`, `FILTER`, `UNION`,
`GRAPH` and `VALUES`, refuses filters with a nested `EXISTS` or with `BNODE`,
`RAND`, `UUID` or `STRUUID` (their value depends on the whole solution or is
random), and marks each variable of `P` as usable as a key or not. A row that
binds a variable not usable as a key is evaluated on its own, as before.

### Which plan

The planner cannot know how many outer rows there will be, nor how large `P`
is. So both run: after 8 rows and 5 ms of per-row work, each per-row evaluation is followed by as much
time spent evaluating `P` standalone into the table (keyed by the bound shared
variables and the active graph); once the standalone evaluation is complete the
table answers every later row. Total work stays within about twice that of the
better plan. A table that would hold more than 2^21 keys, or whose evaluation
fails, is dropped and the `EXISTS` stays per row. The standalone plan is `P`
re-planned with no input bindings and hash joins instead of lateral joins
(`optimize_for_standalone_evaluation`), so it reads each predicate once instead of
probing the index per solution, and a join of two parts that only share an outer
variable is a hash join there, not a cartesian product. Starting it builds the
first hash-join side at once, outside the interleaving budget; the 5 ms floor
keeps that from landing on a query whose `EXISTS` is cheap per row. The clock is `std::time::Instant`; on
`wasm32-unknown-unknown`, which has none that works, the table is never used.
`QueryEvaluator::without_optimizations` turns it off too.

### Tests

The W3C negation and exists tests are unchanged (their outer patterns are
small, so they are answered per row). New tests cover the table path with
several hundred outer rows: a correlated `NOT EXISTS` and `EXISTS`, an outer
variable unbound in some rows (`OPTIONAL`), a filter inside `P` reading an
outer-only variable (refused, per row), and two shared variables across a
`UNION`. Timings on the query above are in Open Triplestore's `docs/performance-comparison.md`.
