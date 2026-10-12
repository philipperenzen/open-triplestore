# Draft upstream PR: refuse nested aggregates

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** `main`, with a backport to the `0.5` branch (`spargebra` 0.4.x).

## Title

spargebra: reject an aggregate nested in another aggregate's argument

## Body

`SELECT (SUM(COUNT(?x)) AS ?s) { … }` parses today, and so do
`MAX(1 + AVG(?o))` and `HAVING (SUM(COUNT(?x)) > 1)`. SPARQL does not allow an
aggregate inside another one's argument; the SPARQL 1.2 test suite has a negative
syntax test for it (`sparql/sparql12/…#nested-aggregate-functions`, on the
`testsuite/tests/sparql.rs` ignore list of v0.5.11).

### Change

The parser already replaces every aggregate by a fresh variable and records it in
the current `SELECT` level's list (`ParserState::new_aggregation`). An inner
aggregate is parsed, and so recorded, before the outer one, whose argument then
mentions the inner aggregate's fresh variable. `new_aggregation` now refuses an
aggregate whose argument mentions one of the current level's aggregate variables,
with "Aggregate functions cannot be nested". Those variables have generated names,
so nothing else can mention them; aggregates side by side
(`SUM(?x) / COUNT(?x)`), in `HAVING` or `ORDER BY`, and over a sub-select's
aggregate (a deeper level) are unaffected.

### Tests

The W3C entry `nested-aggregate-functions` passes and can leave the ignore list.
Unit cases: the three queries above fail to parse; `SUM(?x) / COUNT(?x)`,
`HAVING (COUNT(?x) > 1)` and `SELECT (SUM(?c) AS ?s) { { SELECT (COUNT(?x) AS ?c) … GROUP BY ?p } }`
parse.
