# Vendored crates

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
multi-`FROM` query returned; `spargebra` accepted nested aggregates, which SPARQL
does not allow. Oxigraph has fixed some of them on its main branch, for its next
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
   Drop the ones upstream has released; `tests/w3c_sparql11_manifests.rs` is a
   two-way ratchet, so a released fix shows up there as an unexpected pass.
4. If upstream has released all of them, delete the three directories, the
   `[patch.crates-io]` entries, the `exclude` entries and (once no vendored crate
   is left) the `COPY vendor/` lines in the `Dockerfile`.
5. Run `tests/w3c_sparql11_manifests.rs`, `tests/w3c_sparql11_conformance.rs`, the
   parallel/columnar parity tests and the perf gate.
