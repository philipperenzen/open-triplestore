# Draft upstream PR: reuse of a SELECT-expression variable in an aggregating query

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** `main`, with a backport to the `0.5` branch (`spargebra` 0.4.x).
Checked against `main` at `de0e18f5` (2026-10-01): `AlgebraBuilder` in
`lib/spargebra/src/algebra_builder.rs` validates SELECT expressions against the
same fixed `visible` set.

## Title

spargebra: a SELECT expression may use an earlier SELECT expression's variable in an aggregating query

## Body

```sparql
SELECT (COUNT(?v) AS ?count) (?count + 1 AS ?countPlusOne) WHERE {
  VALUES ?v { 0 1 2 3 }
}
```

is refused with "The SELECT contains an expression with a variable that is
unbound". With an aggregate, the parser checks that every variable of a SELECT
expression is in scope after the grouping, but it computes that set once, before
the SELECT clause, so the variable that `(COUNT(?v) AS ?count)` binds is never in
it.

SPARQL 1.2 allows this (w3c/sparql-query PR #380, closed 2026-05-28): the SELECT
expressions are evaluated left to right, each one extending the solutions of the
previous one, so `?count` is bound when `?count + 1` is evaluated. The W3C test
`grouping/manifest#select-variable-reuse` expects `?count = 4`,
`?countPlusOne = 5`.

### Change

`build_select` keeps a second set, the in-scope variables plus the variable of
every SELECT expression already processed, and validates each expression of an
aggregating query against it. The parser already emits one `Extend` per SELECT
expression in order, so the evaluator needs no change. The other checks are
untouched: a SELECT expression still cannot rebind a variable in scope from the
pattern or the grouping, and a variable still cannot appear twice in the SELECT
clause; a variable that is neither grouped, aggregated nor bound by an earlier
SELECT expression is still refused.

### Tests

The W3C entry passes. `SELECT (COUNT(?v) AS ?c) (?c * 2 AS ?d) (?d + ?c AS ?e)`
gives `4 8 12` over four values; `SELECT (?x + 1 AS ?y) (COUNT(?v) AS ?x)` (the
use before the binding) and `SELECT (COUNT(?v) AS ?c) (?w AS ?d)` with `?w`
ungrouped are still refused.
