# Vendored crates

Five crates of the Oxigraph family are vendored. `oxigraph` and `spareval` carry the
changes that keep typed literals exactly as written (first section); `spargebra`,
`spareval` and `sparopt` carry SPARQL conformance fixes (second section), so
`spareval` has both sets of changes; `oxjsonld`, the JSON-LD processor, carries
fixes for defects the W3C JSON-LD 1.1 API test suite exposes (third section).

| Directory | Crate | Upstream (repository `oxigraph/oxigraph`) | Changes |
| --- | --- | --- | --- |
| `oxigraph/` | `oxigraph` 0.5.11 | `lib/oxigraph` at `df37a5c98e2497135cdd4cfce01a049b78ca6740` | the literal encoder keeps lexical forms and datatypes |
| `spareval/` | `spareval` 0.2.7 | `lib/spareval` at `0e81a29d27575ab69e7f85ea9c7f1058b277fcd0` | triple terms are rebuilt without reading their literals as values; SPARQL 1.1 conformance fixes 1–5 |
| `spargebra/` | `spargebra` 0.4.7 | `lib/spargebra` at `0e81a29d27575ab69e7f85ea9c7f1058b277fcd0` | SPARQL conformance fix 6 |
| `sparopt/` | `sparopt` 0.3.7 | `lib/sparopt` at `0e81a29d27575ab69e7f85ea9c7f1058b277fcd0` | SPARQL conformance fix 1 |
| `oxjsonld/` | `oxjsonld` 0.2.6 | `lib/oxjsonld` at `0e81a29d27575ab69e7f85ea9c7f1058b277fcd0` | JSON-LD 1.1 API fixes (see [oxjsonld](#oxjsonld)) |

The workspace `Cargo.toml` swaps all five in for the crates.io packages:

```toml
[patch.crates-io]
oxigraph = { path = "vendor/oxigraph" }
oxjsonld = { path = "vendor/oxjsonld" }
spargebra = { path = "vendor/spargebra" }
spareval = { path = "vendor/spareval" }
sparopt = { path = "vendor/sparopt" }
```

and lists them under `[workspace] exclude`, so they are not linted, formatted or
tested as part of this project. Every other crate of the family (`oxrdf`, `oxrdfio`,
`oxsdatatypes`, `sparesults`, `oxrocksdb-sys`) still comes from crates.io at the exact
versions oxigraph 0.5.11 pins. Path patches are used rather than git ones because
`deny.toml` denies unknown git sources.

## `oxigraph/` and `spareval/` — typed literals kept as written

### Why

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

### The changes

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

### Verifying the fork

Each fork diff is its own commit, after the commit that vendored the crate
unmodified. To see them against the published crates:

```sh
for c in oxigraph-0.5.11:oxigraph spareval-0.2.7:spareval; do
  diff -ru ~/.cargo/registry/src/index.crates.io-*/"${c%%:*}" "vendor/${c##*:}" \
    -x .cargo-ok -x LICENSE-MIT -x LICENSE-APACHE -x 'UPSTREAM-PR*.md'
done
```

The `spareval` diff also shows the conformance fixes described in the next section;
the lexical-form change there is commit `fda8aa7` (re-applied on top of them).

`LICENSE-MIT` and `LICENSE-APACHE` are the Oxigraph project's licence texts
(identical to the files at the root of the upstream repository; the crates.io
packages do not ship them). `oxigraph/UPSTREAM-PR.md` is the description of the
changes as proposed to upstream.

### Rebasing onto a new upstream release

1. Bump `oxigraph` in the workspace `Cargo.toml` (and `opengraph/Cargo.toml`) and run
   `cargo fetch` with the patch section commented out, so the new crates land in
   `~/.cargo/registry/src/` (`spareval` at the version the new `oxigraph` pins).
2. Replace `vendor/oxigraph/` and `vendor/spareval/` with the new crates, keeping
   `LICENSE-MIT`, `LICENSE-APACHE` and `UPSTREAM-PR.md`; drop `.cargo-ok`. Commit
   that alone. `cargo metadata` re-resolves the patched crates' dependencies; if the
   lock entries of their dependencies move to other versions than the registry
   crates had, set them back by hand and check with `cargo metadata --locked`.
3. Re-apply each change (`git show <fork commit> -- vendor/<crate>/src` and apply it,
   or redo it by hand: one function and a helper in each crate) in its own commit,
   and update the commit hashes above.
4. If upstream has merged an equivalent change, delete the directory, its
   `[patch.crates-io]` line, its `exclude` entry and (when both are gone) the
   `COPY vendor/` lines in the `Dockerfile` and the `vendor/**` path filters in
   `.github/workflows/perf.yml` and `e2e.yml`.
5. Run the lexical-form tests (`rdf11_typed_literals_*`, `rdf11_graph_patterns_*`,
   `rdf11_expressions_*`, `rdf11_a_store_written_before_*` in
   `tests/rdf11_conformance.rs`, the `star_*literal*` tests in
   `tests/sparql12_conformance.rs`), the SHACL, SPARQL and RDF conformance runners,
   and the parallel/columnar parity tests.

## `spargebra/`, `spareval/` and `sparopt/` — Oxigraph's SPARQL parser, evaluator and optimizer with conformance fixes

`vendor/spargebra/`, `vendor/spareval/` and `vendor/sparopt/` are the `spargebra`
0.4.7, `spareval` 0.2.7 and `sparopt` 0.3.7 crates exactly as published on
crates.io (upstream repository <https://github.com/oxigraph/oxigraph>, paths
`lib/spargebra`, `lib/spareval` and `lib/sparopt`, commit
`0e81a29d27575ab69e7f85ea9c7f1058b277fcd0`, recorded in each crate's
`.cargo_vcs_info.json`), plus the local changes listed below. They are the
versions `oxigraph` 0.5.11 pins with `=`, so the workspace `Cargo.toml` swaps them
in for the crates.io packages:

```toml
[patch.crates-io]
spargebra = { path = "vendor/spargebra" }
spareval = { path = "vendor/spareval" }
sparopt = { path = "vendor/sparopt" }
```

and lists them under `[workspace] exclude`, so they are not linted, formatted or
tested as part of this project. The rest of the Oxigraph family still comes from
crates.io, and `Cargo.lock` only loses the `source`/`checksum` lines of the three
entries: their dependencies resolve to the same versions as before. A path patch
is used rather than a git one because `deny.toml` denies unknown git sources.
`spargebra` was added after the others (change 6); it was vendored unmodified in
its own commit first, like them.

### Why

Every SPARQL 1.1 Query entry of the vendored W3C test-suite subset that this
project failed (`tests/w3c_sparql11_manifests.rs`, `docs/conformance/sparql11.md`)
failed inside `spareval` and `sparopt`, and so did the duplicate rows a
multi-`FROM` query returned. In the SPARQL 1.2 suite
(`tests/w3c_sparql12_manifests.rs`, `docs/conformance/sparql12.md`) every entry
the project failed, except one that needs numeric lexical forms kept in storage,
failed inside `spargebra`; and `spareval` panicked on `=` between two
directional literals. Oxigraph has fixed some of them on its main branch, for its next
major release, and none in a 0.5.x release. Carrying the fixes here lets the
project follow the specification now; each one is written so it can be proposed
upstream unchanged (`UPSTREAM-PR-*.md` in the crate it touches).

### The changes

One commit each, in this order:

1. **`GRAPH ?g` scoping** (`sparopt` algebra and optimizer, `spareval` evaluator).
   Backport of upstream commit `fdc32b5` (issue #1905, on `main` only):
   `Graph { graph_name, inner }` stays a node of the optimizer algebra and is
   evaluated per named graph, so `?g` is not in scope inside the inner pattern
   (SPARQL 1.1 §18.6, `Graph(var, P)`). Before, `?g` was pushed into every quad
   pattern inside, which gave wrong answers around `VALUES`, sub-selects and
   `MINUS`. Fixes `bindings#graph`, `aggregates#agg-empty-group-count-graph` and
   `negation#graph-minus`. Draft: `spareval/UPSTREAM-PR-graph-scoping.md`.
2. **A multi-graph default graph is an RDF merge** (`spareval`). Backport of the
   `spareval` half of upstream PR #1920 (issue #1919, on `main` only): a default
   graph made of several graphs (several `FROM` or `USING`, or the union default
   graph) is scanned through the new defaulted
   `QueryableDataset::internal_triples_for_pattern`, which hash-deduplicates
   whole triples, so a triple held in two of the graphs matches once. The
   `oxigraph` crate needs no change (it gets the default method; upstream's
   RocksDB fast path is not taken). The columnar copy in `opengraph` deduplicates
   the same way. Draft: `spareval/UPSTREAM-PR-merged-default-graph.md`.
3. **Zero-length paths with a constant endpoint** (`spareval` path evaluator).
   `:s :p* ?o`, `?s :p? :o` and `ASK { :x :p* :x }` matched the zero-length
   path only when the constant was a node of the active graph. SPARQL 1.1 §18.6
   evaluates the zero-length path against a term endpoint without looking at the
   graph; only a variable endpoint ranges over `nodes(G)`. The path evaluator now
   knows which endpoints are constants and skips the membership check for them
   (through `^`, `|`, `?` and the first step of `+`, not past the middle of `/`).
   Fixes `property-path#zero_or_more_set_start/end` and
   `#zero_or_one_set_start/end`. Draft: `spareval/UPSTREAM-PR-zero-length-paths.md`.
4. **`GROUP_CONCAT` returns an `xsd:string`** (`spareval` aggregate). It kept a
   language tag shared by every input (`"1 2"@en`); SPARQL 1.1 §18.5.1.7, and the
   SPARQL 1.2 draft, return a simple literal. Fixes
   `aggregates#agg-groupconcat-04` and `-06`. Draft:
   `spareval/UPSTREAM-PR-group-concat.md`.
5. **`BNODE(str)` is fresh per solution** (`spareval` expressions). It returned
   `BlankNode::new(str)`: one node per string for the whole query and every later
   request, and nothing for a string that is not a blank-node label. SPARQL 1.1
   §17.4.2.9 asks for the same node within one solution and distinct nodes across
   solutions. The node is now a keyed 128-bit hash of the string and the
   solution's bindings (leaving out variables that `BIND` / `SELECT` expressions
   assign), with keys drawn at random per evaluation. Fixes `functions#bnode01`.
   Draft: `spareval/UPSTREAM-PR-bnode-label.md`.
6. **Nested aggregates are refused** (`spargebra` parser). `SUM(COUNT(?x))` used to
   parse; SPARQL does not allow an aggregate inside another one's argument (the
   SPARQL 1.2 test `nested-aggregate-functions`). The parser refuses an aggregate
   whose argument mentions an aggregate it already replaced by a variable at the
   same `SELECT` level. Draft: `spargebra/UPSTREAM-PR-nested-aggregates.md`.
7. **`=` between directional literals** (`spareval` term equality). With
   `sparql-12`, `impl PartialEq for ExpressionTerm` had no arm for two
   `DirLangStringLiteral`s and reached `unreachable!()`, so `=`, `!=` or `IN`
   between two literals that both carry a base direction panicked (a 500 over
   HTTP). The arm compares lexical form, language tag and direction (RDF 1.2
   term equality), as the `Hash` impl already did. No W3C entry reaches it;
   `directional_literal_equality` in `tests/sparql12_conformance.rs` pins it.
   Draft: `spareval/UPSTREAM-PR-dir-lang-string-equality.md`.
8. **Triple-term expression subjects** (`spargebra` parser).
   `ExprTripleTermSubject` was `ExprTripleTermObject`, so a literal or a triple
   term could be the subject of a triple-term expression
   (`BIND(<<( "x" :q :z )>> AS ?X)`). The SPARQL 1.2 grammar has
   `ExprTripleTermSubject ::= iri | Var`; oxigraph's `main` already parses it so
   in its rewritten parser. Fixes `tripleterm-subject-03` and `-06` of the
   SPARQL 1.2 suite. Draft: `spargebra/UPSTREAM-PR-triple-term-expression-subject.md`.
9. **SELECT-expression variable reuse** (`spargebra` parser). In an
   aggregating query, `build_select` checked each SELECT expression against the
   variables in scope after the grouping only, so
   `(COUNT(?v) AS ?count) (?count + 1 AS ?countPlusOne)` was refused. SPARQL 1.2
   allows it (w3c/sparql-query PR #380); each expression is now also allowed the
   variables of the SELECT expressions before it, which the `Extend` chain
   already binds. Fixes `grouping#select-variable-reuse` of the SPARQL 1.2
   suite. Draft: `spargebra/UPSTREAM-PR-select-variable-reuse.md`.
10. **Braces in `REGEX` patterns** (`spareval` `compile_pattern`). SPARQL's
    `REGEX` takes XPath regular expressions, which are XML Schema 1.0 ones:
    `{` and `}` are ordinary characters wherever they do not make a quantifier
    `{n}`, `{n,}` or `{n,m}`. The `regex` crate refuses an unescaped `{` that
    starts no repetition, so `REGEX(?s, "^({)(.*)(})$")` was an error (the
    filter dropped every row). Those braces are now escaped before the pattern
    is compiled. No W3C entry reaches it; the OGC GeoSPARQL validator's S18
    shape does (through `sh:pattern`, `tests/ogc_geosparql_shacl_roundtrip.rs`),
    and `regex_braces_are_ordinary_characters` in
    `tests/sparql_functions_conformance.rs` pins it. Draft:
    `spareval/UPSTREAM-PR-regex-braces.md`.

### Verifying the fork

The fork diff is every commit after the one that added this directory. To see it
against the published crates:

```sh
for c in spargebra-0.4.7 spareval-0.2.7 sparopt-0.3.7; do
  diff -ru ~/.cargo/registry/src/index.crates.io-*/$c vendor/${c%-*} \
    -x .cargo-ok -x LICENSE-MIT -x LICENSE-APACHE -x 'UPSTREAM-PR-*.md'
done
```

`LICENSE-MIT` and `LICENSE-APACHE` are the Oxigraph project's licence texts at the
commit above (the crates.io packages do not ship them).

### Rebasing onto a new upstream release

1. When `oxigraph` is bumped, check which `spargebra` / `spareval` / `sparopt`
   versions it pins
   and run `cargo fetch` with the patch section commented out, so the new crates
   land in `~/.cargo/registry/src/`.
2. Replace the three directories with the new crates, keeping
   `LICENSE-MIT`, `LICENSE-APACHE` and the `UPSTREAM-PR*.md` files; drop
   `.cargo-ok`. Commit that alone.
3. Re-apply each change below that upstream has not released, one commit each.
   Drop the ones upstream has released; `tests/w3c_sparql11_manifests.rs` and
   `tests/w3c_sparql12_manifests.rs` are two-way ratchets, so a released fix
   shows up there as an unexpected pass (and a dropped fix that upstream has not
   released, as a new failure).
4. If upstream has released all of them, delete the three directories, the
   `[patch.crates-io]` entries, the `exclude` entries and (once no vendored crate
   is left) the `COPY vendor/` lines in the `Dockerfile`.
5. Run `tests/w3c_sparql11_manifests.rs`, `tests/w3c_sparql11_conformance.rs`,
   `tests/w3c_sparql12_manifests.rs`, `tests/sparql12_conformance.rs`, the
   parallel/columnar parity tests and the perf gate.

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
