# Draft upstream PR: custom aggregates with several arguments (spargebra)

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead. One of three (spargebra, sparopt, spareval) that
belong together.

**Target:** `main`, with a backport to the `0.5` branch.

## Title

spargebra: let a custom aggregate call take an argument list

## Body

SPARQL 1.1 allows a custom aggregate anywhere `iriOrFunction` does, and that
rule takes an `ArgList` (grammar [128] and [71]): `ex:agg(?x, 0.5)`. The
aggregate then runs over the list of expressions (§18.5.1, `Aggregation(exprlist,
func, scalarvals, group)`). spargebra parses a custom aggregate call with
exactly one expression, so a two-argument call is a parse error. Standards
define such aggregates: GeoSPARQL 1.1 §10.10 gives
`geof:aggConcaveHull (geom: ogc:geomLiteral, targetPercent: xsd:double)`.

### Change

- `AggregateExpression::FunctionCall` gets `extra_args: Vec<Expression>`, the
  arguments after `expr`. The built-in aggregates always have it empty.
- The `Aggregate` rule accepts `"," Expression` repetitions after the first
  argument of a registered custom aggregate, with or without `DISTINCT`.
- A custom aggregate call is tried before `iriOrFunction`, its name checked
  before its arguments are parsed. `iriOrFunction` used to read
  `ex:agg(DISTINCT ?x)` as the bare IRI `ex:agg` (its `ArgList` has no
  `DISTINCT`), so every `DISTINCT` custom aggregate call was a parse error
  ("expected ENCODE_FOR_URI"); it parses now. Regular function calls are still
  parsed once.
- `Display` prints them (`<iri>(DISTINCT ?x, 0.5)`), SSE prints them after the
  first argument, `lookup_used_variables` visits them, and the nested-aggregate
  check looks into them.

This is a breaking change to a public struct variant (a new field), so it fits
the next major release; a non-breaking alternative is a separate variant for
calls with more than one argument, at the cost of two shapes for one call.

### Tests

A two-argument call parses and round-trips through `Display`; `DISTINCT`
custom calls parse, with one argument or several; a regular function with
several arguments and a bare IRI that names a custom aggregate are unchanged;
an aggregate inside an aggregate is still refused.
