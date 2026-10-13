# Draft upstream PR: GROUP_CONCAT always returns an xsd:string

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** `main`, with a backport to the `0.5` branch.

## Title

spareval: GROUP_CONCAT returns a simple literal, never a language-tagged one

## Body

When every input of `GROUP_CONCAT` carries the same language tag, spareval
returns the concatenation with that tag (`"1 2"@en`). SPARQL 1.1 §18.5.1.7
defines `GroupConcat` by concatenating the lexical forms into a simple literal,
and the SPARQL 1.2 Working Draft (§18.6.1.7) builds it with `CONCAT("", L1)`,
which returns an `xsd:string` too, so neither version keeps the tag.

The W3C tests `aggregates/manifest#agg-groupconcat-04` (`"1 2"` expected from two
`@en` inputs) and `#agg-groupconcat-06` (`"1"` from one) check this; they are on
`testsuite/tests/sparql.rs`'s ignore list ("Our GROUP_CONCAT is wrong").

### Change

`GroupConcatAccumulator` no longer tracks a shared language tag; `finish`
returns `ExpressionTerm::StringLiteral`. Inputs that are not strings still make
the aggregate unbound, as before.

### Tests

The two W3C entries pass and can leave the ignore list. `SELECT
(GROUP_CONCAT(?l) AS ?c) { VALUES ?l { "a"@en "b"@en } }` returns `"a b"`, and
`"a"@en--ltr` inputs (RDF 1.2) give a plain string as well.
