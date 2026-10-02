# Vendored crates

## `spareval/` and `sparopt/` — Oxigraph's SPARQL evaluator and optimizer with SPARQL 1.1 fixes

`vendor/spareval/` and `vendor/sparopt/` are the `spareval` 0.2.7 and `sparopt`
0.3.7 crates exactly as published on crates.io (upstream repository
<https://github.com/oxigraph/oxigraph>, paths `lib/spareval` and `lib/sparopt`,
commit `0e81a29d27575ab69e7f85ea9c7f1058b277fcd0`, recorded in each crate's
`.cargo_vcs_info.json`), plus the local changes listed below. They are the
versions `oxigraph` 0.5.11 pins with `=`, so the workspace `Cargo.toml` swaps them
in for the crates.io packages:

```toml
[patch.crates-io]
spareval = { path = "vendor/spareval" }
sparopt = { path = "vendor/sparopt" }
```

and lists them under `[workspace] exclude`, so they are not linted, formatted or
tested as part of this project. `spargebra` (the parser and algebra) and the rest
of the Oxigraph family still come from crates.io, and `Cargo.lock` only loses the
`source`/`checksum` lines of the two entries: their dependencies resolve to the
same versions as before. A path patch is used rather than a git one because
`deny.toml` denies unknown git sources.

### Why

Every SPARQL 1.1 Query entry of the vendored W3C test-suite subset that this
project failed (`tests/w3c_sparql11_manifests.rs`, `docs/conformance/sparql11.md`)
failed inside these two crates, and so did the duplicate rows a multi-`FROM`
query returned. Oxigraph has fixed some of them on its main branch, for its next
major release, and none in a 0.5.x release. Carrying the fixes here lets the
project follow the specification now; each one is written so it can be proposed
upstream unchanged (`UPSTREAM-PR.md` in the crate it touches).

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

### Verifying the fork

The fork diff is every commit after the one that added this directory. To see it
against the published crates:

```sh
for c in spareval-0.2.7 sparopt-0.3.7; do
  diff -ru ~/.cargo/registry/src/index.crates.io-*/$c vendor/${c%-*} \
    -x .cargo-ok -x LICENSE-MIT -x LICENSE-APACHE -x UPSTREAM-PR.md
done
```

`LICENSE-MIT` and `LICENSE-APACHE` are the Oxigraph project's licence texts at the
commit above (the crates.io packages do not ship them).

### Rebasing onto a new upstream release

1. When `oxigraph` is bumped, check which `spareval` / `sparopt` versions it pins
   and run `cargo fetch` with the patch section commented out, so the new crates
   land in `~/.cargo/registry/src/`.
2. Replace `vendor/spareval/` and `vendor/sparopt/` with the new crates, keeping
   `LICENSE-MIT`, `LICENSE-APACHE` and the `UPSTREAM-PR*.md` files; drop
   `.cargo-ok`. Commit that alone.
3. Re-apply each change below that upstream has not released, one commit each.
   Drop the ones upstream has released; `tests/w3c_sparql11_manifests.rs` is a
   two-way ratchet, so a released fix shows up there as an unexpected pass.
4. If upstream has released all of them, delete both directories, the
   `[patch.crates-io]` entries, the `exclude` entries and (once no vendored crate
   is left) the `COPY vendor/` lines in the `Dockerfile`.
5. Run `tests/w3c_sparql11_manifests.rs`, `tests/w3c_sparql11_conformance.rs`, the
   parallel/columnar parity tests and the perf gate.
