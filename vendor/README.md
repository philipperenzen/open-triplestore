# Vendored crates

## `oxigraph/` — Oxigraph 0.5.11 with lexical-form-preserving literal storage

`vendor/oxigraph/` is the `oxigraph` 0.5.11 crate exactly as published on
crates.io (upstream repository <https://github.com/oxigraph/oxigraph>, path
`lib/oxigraph`, commit `df37a5c98e2497135cdd4cfce01a049b78ca6740`, recorded in
`.cargo_vcs_info.json`), plus one local change. The workspace `Cargo.toml` swaps it
in for the crates.io package:

```toml
[patch.crates-io]
oxigraph = { path = "vendor/oxigraph" }
```

and lists it under `[workspace] exclude`, so it is not linted, formatted or tested
as part of this project. Every other crate of the Oxigraph family (`oxrdf`,
`oxrdfio`, `oxsdatatypes`, `spargebra`, `sparopt`, `spareval`, `sparesults`,
`oxrocksdb-sys`) still comes from crates.io at the exact versions oxigraph 0.5.11
pins, so `Cargo.lock` only loses the `source`/`checksum` lines of the `oxigraph`
entry. A path patch is used rather than a git one because `deny.toml` denies
unknown git sources.

### Why

Oxigraph stores some typed literals as native values: `xsd:boolean`, the numeric
types, the date/time types and the durations. Before this change it did so for any
well-formed lexical form, and also for all twelve types derived from
`xsd:integer` (`xsd:int`, `xsd:long`, `xsd:nonNegativeInteger`, `xsd:byte`, ...)
and for `xsd:dateTimeStamp`. Reading such a literal back printed the canonical
form of the value under the primitive type, so the store did not return the term
it was given:

| written | read back before | read back now |
| --- | --- | --- |
| `"1"^^xsd:boolean` | `"true"^^xsd:boolean` | `"1"^^xsd:boolean` |
| `"05"^^xsd:integer` | `"5"^^xsd:integer` | `"05"^^xsd:integer` |
| `"5"^^xsd:nonNegativeInteger` | `"5"^^xsd:integer` | `"5"^^xsd:nonNegativeInteger` |
| `"2020-01-01T00:00:00+00:00"^^xsd:dateTimeStamp` | `"2020-01-01T00:00:00Z"^^xsd:dateTime` | unchanged |
| `"5"^^xsd:integer`, `"true"^^xsd:boolean` | unchanged | unchanged (native encoding) |

RDF 1.1 defines a literal by its lexical form and datatype IRI, so this was a
change of the data, not only of its spelling. It broke SHACL `sh:datatype` on
derived types (every stored `"5"^^xsd:nonNegativeInteger` violated
`sh:datatype xsd:nonNegativeInteger`), SHACL activation flags written as
`"1"^^xsd:boolean` (W3C test `core/property/uniqueLang-002`), `sh:hasValue` /
`sh:in` / `sh:value` fidelity, and the OWL 2 RL datatype rules, which cannot see a
type that storage has dropped.

### The change

`src/storage/numeric_encoder.rs`, `impl From<LiteralRef<'_>> for EncodedTerm`:
a typed literal gets a native encoding only when its datatype IRI is exactly the
native type (`xsd:boolean`, `xsd:integer`, `xsd:decimal`, `xsd:float`,
`xsd:double`, `xsd:dateTime`, `xsd:time`, `xsd:date`, the five `xsd:g*` types,
`xsd:duration`, `xsd:yearMonthDuration`, `xsd:dayTimeDuration`) **and** the value
prints back to exactly the given lexical form. Everything else (derived types,
`xsd:dateTimeStamp`, non-canonical lexical forms) goes to the
`SmallTypedLiteral` / `BigTypedLiteral` variants that already held ill-typed
literals and custom datatypes. The canonical check writes the value's `Display`
form into a comparator, so it allocates nothing.

What does not change:

- The on-disk format. No new `EncodedTerm` variant, no new type byte. Data written
  by an unpatched Oxigraph reads exactly as before (it was canonical anyway), and a
  store written by this fork opens in an unpatched 0.5.11, which then reads the kept
  lexical forms back too (the typed-literal variants decode verbatim).
- Value semantics in SPARQL expressions. `spareval` parses typed literals into
  values when it evaluates an expression (`spareval/src/dataset.rs`,
  `parse_typed_literal`), so `FILTER (?x = 5)`, `ORDER BY`, arithmetic and
  aggregates treat `"05"^^xsd:integer`, `"5"^^xsd:int` and `5` as the same number.

What changes (RDF-correct, and intended): term identity. Basic graph pattern
matching, joins, `DISTINCT`, `GROUP BY` keys, `sameTerm` and `DELETE DATA` compare
terms, so `"1"^^xsd:boolean` no longer matches `true`, and `"5"^^xsd:int` no longer
joins with `5`. Existing canonical data keeps matching canonical patterns.

### Verifying the fork

The fork diff is the commit after the one that added this directory. To see it
against the published crate:

```sh
diff -ru ~/.cargo/registry/src/index.crates.io-*/oxigraph-0.5.11 vendor/oxigraph \
  -x .cargo-ok -x LICENSE-MIT -x LICENSE-APACHE -x UPSTREAM-PR.md
```

`LICENSE-MIT` and `LICENSE-APACHE` are the Oxigraph project's licence texts
(identical to the files at the root of the upstream repository; the crates.io
package of `oxigraph` does not ship them). `UPSTREAM-PR.md` is the description of
the change as proposed to upstream.

### Rebasing onto a new upstream release

1. Bump `oxigraph` in the workspace `Cargo.toml` (and `opengraph/Cargo.toml`) and
   run `cargo fetch` with the patch section commented out, so the new crate lands in
   `~/.cargo/registry/src/`.
2. Replace `vendor/oxigraph/` with the new crate, keeping `LICENSE-MIT`,
   `LICENSE-APACHE` and `UPSTREAM-PR.md`; drop `.cargo-ok`. Commit that alone.
3. Re-apply the change (`git show <fork commit> -- vendor/oxigraph/src` and
   apply it, or redo it by hand: it is one function and a helper) in a second
   commit, and update the commit hash above.
4. If upstream has merged an equivalent change, delete `vendor/oxigraph/`, the
   `[patch.crates-io]` section, the `exclude` entry and the `COPY vendor/` lines in
   the `Dockerfile` instead.
5. Run the lexical-form tests (`tests/literal_lexical_forms.rs`), the SHACL, SPARQL
   and RDF conformance runners, and the parallel/columnar parity tests.
