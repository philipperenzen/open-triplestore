# Draft upstream PR: `=` between two directional literals panics

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** `main`, with a backport to the `0.5` branch (`spareval` 0.2.x).
Checked against `main` at `de0e18f5` (2026-10-01): `lib/spareval/src/dataset.rs`
still has the same match.

## Title

spareval: add the missing DirLangStringLiteral arm to ExpressionTerm equality

## Body

With the `sparql-12` feature, comparing two literals that both carry a base
direction panics:

```sparql
SELECT ?eq WHERE { BIND("abc"@en--ltr = "abc"@en--rtl AS ?eq) }
```

`equals` in `expression.rs` answers `Some(a == b)` for a
`DirLangStringLiteral`, which calls `impl PartialEq for ExpressionTerm` in
`dataset.rs`. That impl first compares the discriminants, then matches the two
values, but has no arm for `(DirLangStringLiteral, DirLangStringLiteral)`; the
pair falls through to `(_, _) => unreachable!()`. Any `=`, `!=` or `IN` whose
two operands are directional literals — a `FILTER(?label = "x"@en--ltr)` over
stored data as much as two constants — aborts the query. In a server this is a
500 instead of a boolean.

The `Hash` impl right below already hashes the variant as
`(value, language, direction)`.

### Change

One arm, behind `#[cfg(feature = "sparql-12")]` like the variant itself:

```rust
(
    Self::DirLangStringLiteral { value: lv, language: ll, direction: ld },
    Self::DirLangStringLiteral { value: rv, language: rl, direction: rd },
) => lv == rv && ll == rl && ld == rd,
```

This is RDF 1.2 term equality for directional language-tagged strings (RDF 1.2
Concepts §3.3: two such literals are the same term when lexical form, language
tag and base direction are all equal; `oxrdf` keeps language tags normalised to
lower case, so comparing the strings is enough). It agrees with the `Hash` impl,
and `equals` keeps its existing rule for the variant (`Some(a == b)`, the same
as for `LangStringLiteral`).

### Tests

- `"abc"@en--ltr = "abc"@en--ltr` is `true`; `= "abc"@en--rtl`, `= "abc"@en`
  and `= "abc"@fr--ltr` are `false`; `!=` with a different direction is `true`;
  `"abc"@en--rtl IN ("abc"@en--ltr, "abc"@en--rtl)` is `true`.
- `FILTER(?l = "x"@en--ltr)` over stored `"x"@en--ltr` and `"x"@en--rtl`
  values keeps exactly the `ltr` rows.

None of the W3C `sparql12` entries compares two directional literals with `=`,
so the suite does not catch this.
