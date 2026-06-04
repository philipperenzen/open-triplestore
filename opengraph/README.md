# OpenGraph

**OpenGraph** is the RDF engine layer behind [Open Triplestore](..).
It is a maintained crate built *over* [Oxigraph](https://crates.io/crates/oxigraph) —
it depends on the upstream `oxigraph` / `oxrdf` / `spargebra` crates and adds the
capabilities we need on top, rather than forking the storage engine and SPARQL
evaluator. The backend depends on OpenGraph (not on Oxigraph directly), so the
extensions below are available everywhere.

## Headline feature: durable blank-node identity

Plain RDF blank nodes are **not durable**. Every time a document is parsed the
engine is free to invent fresh labels (`_:b0`, `_:b1`, …), so re-importing or
reloading the *same* data renames every anonymous node. This is what makes SHACL
shapes, RDF lists and GeoSPARQL geometries impossible to address reliably across
sessions — a blank node you found a moment ago may have a different label after a
reload.

OpenGraph makes blank nodes durable **as far as the W3C standards allow**, in two
composable layers:

| Layer | Module | What it does |
|---|---|---|
| Stable canonical labels | [`canonical`](src/canonical.rs) | Assigns each blank node a deterministic `c14nN` label derived from graph **structure** (RDF Dataset Canonicalization, RDFC-1.0 shape) — independent of input labels and statement order. The same logical graph always produces the same labels. |
| Durable Skolem IRIs (opt-in) | [`skolem`](src/skolem.rs) | Replaces blank nodes with real IRIs in the `/.well-known/genid/` space (RDF 1.1 §3.5), minted from each node's canonical hash. These survive any store round-trip and are directly query-able. `deskolemize` restores blank nodes for standards-compliant output. |

Skolem IRIs are the furthest the standard lets you push blank-node durability:
they are ordinary IRIs, so they are globally referenceable and immune to the
relabeling problem entirely.

### Example

```rust
use opengraph::{canonical, skolem};

// `quads: Vec<oxrdf::Quad>` parsed from some document.

// (1) Stable canonical labels — blank nodes stay blank nodes, but get
//     deterministic ids (c14n0, c14n1, …):
let canon = canonical::canonicalize(&quads);
// canon.quads      — relabeled quads
// canon.mapping    — input blank-node id -> canonical id

// (2) Durable Skolem IRIs under a chosen base — blank nodes become IRIs:
let (skolemized, map) = skolem::skolemize(&quads, "https://data.example.org");
// e.g. _:x -> <https://data.example.org/.well-known/genid/<hash>>

// (3) Round-trip back to blank nodes for blank-node serialization:
let restored = skolem::deskolemize(&skolemized, "https://data.example.org");
```

### Scope & limitations

- Canonical **hashes are implementation-internal**: deterministic and stable for
  our own round-trips, but not guaranteed byte-identical to other RDFC-1.0
  implementations. Durability — not cross-implementation hash interop — is the goal.
- Genuinely *automorphic* blank nodes (structurally indistinguishable even after
  full refinement — rare in real ontology/SHACL/list data) fall back to a
  deterministic input-label tie-break. Full RDFC-1.0 "Hash N-Degree Quads" is a
  planned addition.
- RDF-star triple terms are not traversed (the `rdf-star` feature is off).

## Query & storage optimisation (planning-stage)

| Module | Status | Purpose |
|---|---|---|
| [`optimizer`](src/optimizer.rs) | usable | Cost-based BGP triple-pattern reordering (query rewriting) |
| [`hash_join`](src/hash_join.rs) | prototype | Hash-join execution strategy |
| [`rocksdb_config`](src/rocksdb_config.rs) | prototype | RocksDB tuning / column-family layout |
| [`mvcc`](src/mvcc.rs) | prototype | Snapshot-isolation sketch |

These operate at the query-rewriting and post-processing levels. Native join
strategies will require forking `spareval` / `sparopt` once upstream exposes
pluggable execution.

## Building & testing

OpenGraph is a self-contained Cargo workspace member:

```sh
cargo test                       # unit tests (durable blank nodes, optimizer, …)
cargo bench --bench hash_join    # benchmarks
```

Downstream crates depend on it by path:

```toml
[dependencies]
opengraph = { path = "../opengraph" }
```

and can reach the re-exported engine via `opengraph::oxigraph` / `opengraph::oxrdf`
to stay version-aligned.
