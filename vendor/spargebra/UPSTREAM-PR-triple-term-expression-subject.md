# Draft upstream request: refuse literal and triple-term subjects in triple-term expressions (0.5)

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** the `0.5` maintenance branch (`spargebra` 0.4.x). `main` does not
need it: its rewritten parser (`lib/spargebra/src/parser.rs` at `de0e18f5`,
2026-10-01) already implements `[138] ExprTripleTermSubject ::= iri | Var`.

## Title

spargebra 0.4: ExprTripleTermSubject is `iri | Var`

## Body

In 0.4.7, `ExprTripleTermSubject` is defined as `ExprTripleTermObject`, so the
subject of a triple-term expression may be a literal or another triple term:

```sparql
SELECT * { BIND(<<( "literal" :q :z )>> AS ?X) }
SELECT * { BIND(<<( <<( :s :p :o )>> :q :z )>> AS ?X) }
```

both parse. The SPARQL 1.2 grammar restricts the subject to `iri | Var` (the
questions in w3c/sparql-query#282 and #283 were settled that way; both closed
on 2025-12-26). The SPARQL 1.2 test suite has a negative syntax test for each:
`syntax-triple-terms-negative/manifest#tripleterm-subject-03` and
`#tripleterm-subject-06`. The `VALUES` variants (`#tripleterm-subject-01`,
`-02`, `-04`, `-05`) already fail to parse, because `TripleTermData` checks its
subject.

### Change

```rust
rule ExprTripleTermSubject() -> Expression =
    i:iri() { i.into() } /
    v:Var() { v.into() }
```

A variable subject is still allowed; if it is bound to a literal at evaluation
time, `TRIPLE()` gives an unbound result, as today.

### Tests

The two W3C entries pass. `BIND(<<( :s :p "o" )>> AS ?X)`,
`BIND(<<( ?s :p <<( :a :b :c )>> )>> AS ?X)` and the other positive
`syntax-triple-terms-positive` entries still parse.
