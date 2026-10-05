# Vendored crates

Three crates of the Oxigraph family are vendored. Two are changed in one place each
so that the store keeps typed literals exactly as written; the third, the JSON-LD
processor, carries fixes for defects the W3C JSON-LD 1.1 API test suite exposes
(see [oxjsonld](#oxjsonld) below):

| Directory | Crate | Upstream (repository `oxigraph/oxigraph`) | Change |
| --- | --- | --- | --- |
| `oxigraph/` | `oxigraph` 0.5.11 | `lib/oxigraph` at `df37a5c98e2497135cdd4cfce01a049b78ca6740` | the literal encoder keeps lexical forms and datatypes |
| `spareval/` | `spareval` 0.2.7 | `lib/spareval` at `0e81a29d27575ab69e7f85ea9c7f1058b277fcd0` | triple terms are rebuilt without reading their literals as values |
| `oxjsonld/` | `oxjsonld` 0.2.6 | `lib/oxjsonld` at `0e81a29d27575ab69e7f85ea9c7f1058b277fcd0` | JSON-LD 1.1 API fixes (see [oxjsonld](#oxjsonld)) |

Each directory is the crate exactly as published on crates.io (the upstream commit
is recorded in its `.cargo_vcs_info.json`), plus the change. The workspace
`Cargo.toml` swaps them in for the crates.io packages:

```toml
[patch.crates-io]
oxigraph = { path = "vendor/oxigraph" }
oxjsonld = { path = "vendor/oxjsonld" }
spareval = { path = "vendor/spareval" }
```

and lists them under `[workspace] exclude`, so they are not linted, formatted or
tested as part of this project. Every other crate of the family (`oxrdf`, `oxrdfio`,
`oxsdatatypes`, `spargebra`, `sparopt`, `sparesults`, `oxrdfio`, `oxrocksdb-sys`) still comes
from crates.io at the exact versions oxigraph 0.5.11 pins, and `Cargo.lock` only
loses the `source`/`checksum` lines of the three entries. Path patches are used rather
than git ones because `deny.toml` denies unknown git sources.

## Why

Oxigraph stores some typed literals as native values: `xsd:boolean`, the numeric
types, the date/time types and the durations. Before this change it did so for any
well-formed lexical form, and also for all twelve types derived from `xsd:integer`
(`xsd:int`, `xsd:long`, `xsd:nonNegativeInteger`, `xsd:byte`, ...) and for
`xsd:dateTimeStamp`. Reading such a literal back printed the canonical form of the
value under the primitive type, so the store did not return the term it was given:

| written | read back before | read back now |
| --- | --- | --- |
| `"1"^^xsd:boolean` | `"true"^^xsd:boolean` | `"1"^^xsd:boolean` |
| `"05"^^xsd:integer` | `"5"^^xsd:integer` | `"05"^^xsd:integer` |
| `"5"^^xsd:nonNegativeInteger` | `"5"^^xsd:integer` | `"5"^^xsd:nonNegativeInteger` |
| `"2020-01-01T00:00:00+00:00"^^xsd:dateTimeStamp` | `"2020-01-01T00:00:00Z"^^xsd:dateTime` | unchanged |
| `"5"^^xsd:integer`, `"true"^^xsd:boolean` | unchanged | unchanged (native encoding) |

RDF 1.1 defines a literal by its lexical form and datatype IRI, so this changed the
data, not only its spelling. It broke SHACL `sh:datatype` on derived types (every
stored `"5"^^xsd:nonNegativeInteger` violated `sh:datatype xsd:nonNegativeInteger`),
SHACL activation flags written as `"1"^^xsd:boolean` (W3C test
`core/property/uniqueLang-002`), `sh:hasValue` / `sh:in` / `sh:value` fidelity, the
OWL 2 RL datatype rules (which cannot see a type that storage has dropped), and every
"is the stored copy still the file?" check.

## The changes

**`oxigraph/src/storage/numeric_encoder.rs`**, `impl From<LiteralRef<'_>> for
EncodedTerm`: a typed literal gets a native encoding only when its datatype IRI is
exactly the native type (`xsd:boolean`, `xsd:integer`, `xsd:decimal`, `xsd:float`,
`xsd:double`, `xsd:dateTime`, `xsd:time`, `xsd:date`, the five `xsd:g*` types,
`xsd:duration`, `xsd:yearMonthDuration`, `xsd:dayTimeDuration`) **and** the value
prints back to exactly the given lexical form. Everything else (derived types,
`xsd:dateTimeStamp`, non-canonical lexical forms) goes to the `SmallTypedLiteral` /
`BigTypedLiteral` variants that already held ill-typed literals and custom
datatypes. The check writes the value's `Display` form into a comparator, so it
allocates nothing. `oxigraph/src/sparql/dataset.rs` follows: its fast path for the
effective boolean value (`FILTER (?x)`) knew only the native variants, so it now
reads the value of a typed literal kept as written (`"1"^^xsd:boolean`,
`"8.0e0"^^xsd:double`, `"5"^^xsd:int`).

**`spareval/src/eval.rs`**: when evaluation matches, binds or builds a triple term
(a constant `<<( s p o )>>` in a pattern, a triple pattern with bound variables,
`VALUES`), it used to go through `ExpressionTerm`, which holds the *value* of a typed
literal. With literals kept as written that turned a stored
`<<( :a :age "41"^^xsd:int )>>` into `<<( :a :age 41 )>>` on the way, so the stored
triple term could not be found by its own constant, and a join between a reifier and
its asserted triple depended on join order. It now goes through `Term`, which keeps
the literal (`internalize_triple_term`, `encode_triple`, `put_pattern_value`).

What does not change:

- The on-disk format. No new `EncodedTerm` variant, no new type byte. Data written by
  an unpatched Oxigraph reads exactly as before (it was canonical anyway;
  `tests/fixtures/oxigraph-0.5.11-store` is such a store), and a store written by
  this fork opens in an unpatched 0.5.11, which reads the kept lexical forms back
  too (the typed-literal variants decode verbatim).
- Value semantics in SPARQL expressions. `spareval` parses typed literals into values
  when it evaluates an expression (`spareval/src/dataset.rs`, `parse_typed_literal`),
  so `FILTER (?x = 5)`, `ORDER BY`, arithmetic, aggregates and `STR()` of a cast
  treat `"05"^^xsd:integer`, `"5"^^xsd:int` and `5` as the same number. That
  includes expressions over triple terms: `OBJECT()` returns the value, and `=`
  compares triple terms component by component, by value.

What changes (RDF-correct, and intended): term identity. Basic graph pattern
matching, joins, `DISTINCT`, `GROUP BY` keys, `sameTerm` and `DELETE DATA` compare
terms, so `"1"^^xsd:boolean` no longer matches `true`, and `"5"^^xsd:int` no longer
joins with `5`. Existing canonical data keeps matching canonical patterns. One
residual: a triple term written as a constant *inside an expression*
(`sameTerm(?t, <<( :a :b "05"^^xsd:integer )>>)`) is still built from values by
spareval's expression evaluator, so `sameTerm` against a stored triple term with a
non-canonical literal is false. Graph patterns are not affected.

## Verifying the fork

Each fork diff is its own commit, after the commit that vendored the crate
unmodified. To see them against the published crates:

```sh
for c in oxigraph-0.5.11:oxigraph spareval-0.2.7:spareval oxjsonld-0.2.6:oxjsonld; do
  diff -ru ~/.cargo/registry/src/index.crates.io-*/"${c%%:*}" "vendor/${c##*:}" \
    -x .cargo-ok -x LICENSE-MIT -x LICENSE-APACHE -x 'UPSTREAM-PR*.md'
done
```

`LICENSE-MIT` and `LICENSE-APACHE` are the Oxigraph project's licence texts
(identical to the files at the root of the upstream repository; the crates.io
packages do not ship them). `oxigraph/UPSTREAM-PR.md` is the description of the
changes as proposed to upstream; `oxjsonld/UPSTREAM-PR-*.md` hold one draft per
JSON-LD fix. None of them has been posted upstream.

The `oxjsonld` crate was checked against `Cargo.lock` when it was vendored: the
published `oxjsonld-0.2.6.crate` has the SHA-256 the lock recorded
(`86a3e89e005662e60327027f45ec7cefd0472404e01831b5d83a3ac522cfabe0`), and the
directory is that archive unpacked, `.cargo-ok` aside.

## Rebasing onto a new upstream release

1. Bump `oxigraph` in the workspace `Cargo.toml` (and `opengraph/Cargo.toml`) and run
   `cargo fetch` with the patch section commented out, so the new crates land in
   `~/.cargo/registry/src/` (`spareval` at the version the new `oxigraph` pins).
2. Replace `vendor/oxigraph/`, `vendor/spareval/` and `vendor/oxjsonld/` with the
   new crates (`oxjsonld` at the version the new `oxrdfio` pins), keeping
   `LICENSE-MIT`, `LICENSE-APACHE` and the `UPSTREAM-PR*.md` files; drop `.cargo-ok`. Commit
   that alone. `cargo metadata` re-resolves the patched crates' dependencies; if the
   lock entries of their dependencies move to other versions than the registry
   crates had, set them back by hand and check with `cargo metadata --locked`.
3. Re-apply each change (`git show <fork commit> -- vendor/<crate>/src` and apply it,
   or redo it by hand: one function and a helper in each crate; the `oxjsonld`
   fixes are listed below) in its own commit, and update the commit hashes above.
4. If upstream has merged an equivalent change, delete the directory, its
   `[patch.crates-io]` line, its `exclude` entry and (when both are gone) the
   `COPY vendor/` lines in the `Dockerfile` and the `vendor/**` path filters in
   `.github/workflows/perf.yml` and `e2e.yml`.
5. Run the lexical-form tests (`rdf11_typed_literals_*`, `rdf11_graph_patterns_*`,
   `rdf11_expressions_*`, `rdf11_a_store_written_before_*` in
   `tests/rdf11_conformance.rs`, the `star_*literal*` tests in
   `tests/sparql12_conformance.rs`), the SHACL, SPARQL and RDF conformance runners,
   and the parallel/columnar parity tests. For `oxjsonld`: `tests/w3c_jsonld_api_manifests.rs`
   (its known-failure list is exact, so a fix upstream shows up as an unexpected
   pass), `tests/jsonld_document_loader_http.rs` and the lib `jsonld::` tests.

## oxjsonld

`oxjsonld` is the JSON-LD 1.1 parser and serializer behind `oxrdfio`, which every
JSON-LD upload, download and remote-context fetch in the store goes through. It
was vendored unmodified first, then each fix below went in as its own commit with
an `UPSTREAM-PR-*.md` draft beside the crate.

1. **IRI resolution removes the dot segments of the whole target path**
   (`src/iri.rs`, used by `context.rs` and `expansion.rs`;
   `UPSTREAM-PR-1-iri-dot-segments.md`). `oxiri::Iri::resolve` keeps the `.` and
   `..` segments of the base path it merges with and of a network-path reference
   (`//host/../x`); RFC 3986 §5.2.2 removes them. W3C json-ld-api toRdf `0122`,
   `0123`, `e062`, `e091`.

2. **An `@base` in absolute form is the base even when it is not a valid IRI**
   (`src/context.rs`, context processing step 5.7;
   `UPSTREAM-PR-2-invalid-base.md`). It used to refuse the document; IRIs
   resolved against such a base are not well-formed and are left out of the
   quads, as the JSON-LD to RDF algorithm says. A relative `@base` that does not
   resolve and a non-string `@base` still raise `invalid base IRI`. W3C
   json-ld-api toRdf `li12`.

3. **A type map applies the type's scoped context to the map context**
   (`src/expansion.rs`, `IndexContainer`, expansion steps 13.8.3.2 and
   13.8.3.7.4; `UPSTREAM-PR-3-type-map-scoped-context.md`). The context
   propagates into nested nodes, and the node gets the type already expanded, so
   it does not apply the context a second time as its own non-propagating
   type-scoped context. W3C json-ld-api toRdf `c013`.

4. **The `rdfDirection` option** (`src/to_rdf.rs`, `JsonLdRdfDirection` and
   `JsonLdParser::with_rdf_direction`, exported from `src/lib.rs`;
   `UPSTREAM-PR-4-rdf-direction.md`): `Ignore` (JSON-LD 1.1's `null`, the
   direction is dropped), `I18nDatatype`, `CompoundLiteral`, and
   `DirectionalLanguageTaggedString` (RDF 1.2 `"abc"@ar--rtl`). The default is
   unchanged — the RDF 1.2 directional string in the `rdf-12` build this server
   uses, so every upload, import and Graph Store parse keeps the direction as
   before. `oxrdfio`'s `RdfParser` has no setting for it; the W3C runner calls
   `JsonLdParser` directly with the option each test names (absent = `null`).
   W3C json-ld-api toRdf `di02`, `di04`–`di06`, and the non-normative
   `di09`–`di12` that the runner used to skip.
