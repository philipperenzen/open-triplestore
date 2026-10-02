# Draft upstream pull request (not posted)

Target: `oxigraph/oxigraph`, branch `main`, files `lib/oxigraph/src/storage/numeric_encoder.rs`
and `lib/spareval/src/eval.rs` (two commits; the second is needed once the first lands).

---

**Title:** Storage: keep the lexical form and datatype of non-canonical and derived-type literals

**Problem**

`EncodedTerm::from(LiteralRef)` stores a typed literal as a native value whenever its
lexical form parses, and also does so for the twelve datatypes derived from
`xsd:integer` and for `xsd:dateTimeStamp`. Decoding prints the value under the
primitive type, so the store does not give back the term it was given:

```text
insert "1"^^xsd:boolean             -> read "true"^^xsd:boolean
insert "05"^^xsd:integer            -> read "5"^^xsd:integer
insert "5"^^xsd:nonNegativeInteger  -> read "5"^^xsd:integer
insert "..."^^xsd:dateTimeStamp     -> read "..."^^xsd:dateTime
```

In RDF 1.1 these are different literals (a literal is its lexical form plus datatype
IRI; RDF 1.1 Concepts §3.3 and §5.1 only allow *literal value* equality to be
looser than term equality). Consequences we hit downstream:

- SHACL `sh:datatype xsd:nonNegativeInteger` reports every stored
  `"5"^^xsd:nonNegativeInteger` as a violation, because the shape engine sees
  `xsd:integer`.
- SHACL activation flags written `"1"^^xsd:boolean` read back as `true` (W3C SHACL
  test `core/property/uniqueLang-002` expects a non-canonical `true` to be ignored).
- Round trips (load, then Graph Store GET or CONSTRUCT) change the data, which breaks
  digests and "is this graph unchanged?" checks against the source file.
- OWL 2 RL datatype rules cannot see the original datatype.

**Change**

Use a native encoding only when the datatype IRI is exactly the native type and the
value's `Display` output (what decoding produces) equals the lexical form. Everything
else is stored with the existing `SmallTypedLiteral` / `BigTypedLiteral` variants, which
already hold ill-typed literals and unknown datatypes. The check writes `Display` into
a comparing `fmt::Write` sink, so it does not allocate.

- `DatasetView::internal_term_effective_boolean_value` had a fast path for the native
  variants only and answered "no boolean value" for every typed-literal variant. With
  the change those can hold well-formed booleans and numbers (`"1"^^xsd:boolean`,
  `"8.0e0"^^xsd:double`, `"5"^^xsd:int`), so they now fall back to the parsed value.
- No on-disk format change: no new variant or type byte. Existing databases decode
  exactly as before (their literals were canonical), and a database written with the
  change opens with older versions, which decode the kept forms verbatim.
- SPARQL expression semantics are unchanged: `spareval` parses typed literals into
  values when evaluating (`parse_typed_literal`), so `FILTER(?x = 5)`, `ORDER BY`,
  arithmetic and aggregates still treat `"05"^^xsd:integer`, `"5"^^xsd:int` and `5`
  as equal numbers.
- Term identity becomes exact, as RDF requires: a BGP with `true` no longer matches a
  stored `"1"^^xsd:boolean`, `"5"^^xsd:int` no longer joins with `5`, and `DISTINCT`
  keeps `"05"^^xsd:integer` and `5` apart. This is a visible behaviour change for
  users who relied on canonicalisation, worth a changelog line.
- Data loaded before the upgrade stays in canonical form; re-loading the same source
  afterwards (append, not replace) can add the lexical-form variant beside it.
- Cost: one `Display` pass per native literal on write.

**Second commit: spareval rebuilds triple terms through `Term`**

When evaluation matches, binds or builds a triple term (a constant `<<( s p o )>>` in
a pattern, a triple pattern with bound variables, `VALUES`), `spareval` goes through
`ExpressionTerm` (`externalize_expression_term` per component, `ExpressionTriple::new`,
`internalize_expression_term`). An expression term holds the *value* of a typed
literal, so with the storage change a stored `<<( :a :age "41"^^xsd:int )>>` comes
back as `<<( :a :age 41 )>>`: the stored triple term can no longer be found by its own
constant, and `?s :age ?age {| :source ?src |}` returns rows or not depending on join
order. The commit uses `externalize_term` / `internalize_term` instead
(`internalize_triple_term` in `from_ground_term_pattern` and `get_pattern_value`,
`encode_triple`, `put_pattern_value`). Expression functions (`TRIPLE()`, `OBJECT()`,
`=` on triple terms) are untouched and keep value semantics; a triple-term constant
inside an expression is still built from values, so `sameTerm(?t, <<( :a :b "05"^^xsd:integer )>>)`
is false against a stored triple term with that literal (left for a follow-up: it
needs a way to build an internal triple term from internal components in
`ExpressionEvaluatorContext`).

**Tests**

Unit tests in `numeric_encoder.rs` cover canonical literals keeping the native
encoding and non-canonical / derived / `dateTimeStamp` / ill-typed literals
round-tripping verbatim. Downstream (open-triplestore) runs the W3C SHACL core
suite, the SPARQL 1.1 query/update suites and the RDF 1.1 / SPARQL 1.2 conformance
tests against the patched crate.

**Possible follow-ups (not in this PR)**

- Native encodings for the derived integer types would keep the fast path for them;
  that needs new `EncodedTerm` variants and a storage version bump.
