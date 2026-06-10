# SHACL conformance — official W3C test suite

The official **W3C SHACL test suite** (core section) is vendored under
[`tests/fixtures/w3c-shacl/`](../../tests/fixtures/w3c-shacl/PROVENANCE.md) and runs in CI
via [`tests/w3c_shacl_conformance.rs`](../../tests/w3c_shacl_conformance.rs).

## Scorecard (2026-06-10)

| | count |
|---|---|
| **Pass** | **46** |
| Known-fail (ratcheted) | 52 |
| Skipped (auxiliary `-data`/`-shapes` files, no test entry) | 15 |
| Total files | 113 |

**Comparison level:** `sh:conforms` plus the multiset of violation **focus nodes**
(IRIs/literals by lexical form, blank nodes by count). Full result-set equality
(constraint-component IRIs, `sh:resultPath`, `sh:value`) is a tracked refinement — the
engine currently reports the source constraint as a display string, not a component IRI.

**Gap policy:** a two-way ratchet. Every test not listed in `KNOWN_FAILURES` must pass,
and every listed test must still fail — silent regressions *and* silent fixes both turn
CI red, so the list cannot go stale.

## Known-failure categories

Most failures share one root cause: the engine's **string-typed focus-node model** —
target resolution flattens RDF terms to lexical strings, losing the term kind and
datatype. Concretely:

1. **Node-level value constraints** (`node/*` — datatype, min/max ranges, languageIn,
   class, closed, minLength on the focus node itself): the engine evaluates these only
   in property-shape context. At node level the focus is a string whose
   datatype/language is unknown, so e.g. `sh:datatype` on a `sh:targetNode` literal
   cannot be checked. *(A nodeKind heuristic — blank/scheme-shaped/other — was added and
   recovers the common literal/IRI cases.)*
2. **Result cardinality / typed-literal comparison details** (`property/*` comparisons,
   `uniqueLang`, `equals`): SHACL prescribes one result per offending value occurrence
   with type-aware comparison; the engine compares lexically and occasionally
   over/under-counts duplicates.
3. **Sequence-path edge cases** (`path/path-sequence-*`, `path-strange-*`).
4. **Qualified value shape sibling-disjointness** (`qualifiedValueShapesDisjoint`):
   unimplemented.
5. **Result metadata semantics** (`misc/*`): `sh:message`/`sh:severity` propagation
   detail and one `sh:deactivated` edge case.
6. **Target edge cases** (`targets/*`): implicit class targets through `rdfs:subClassOf`,
   one `targetObjectsOf` detail.

The forward path for categories 1–2 is typing the validation pipeline on RDF terms
instead of strings — a contained engine refactor that the ratchet will reward
automatically (fixed tests must be removed from the list to keep CI green).

## Engine fixes driven by this suite

Running the official suite (rather than only in-house tests) immediately surfaced and
fixed three real engine bugs:

- **Value-node semantics** for `sh:not`/`sh:and`/`sh:or`/`sh:xone`/`sh:node` in property
  context: these were evaluated against the focus node instead of each value node along
  the path (SHACL §4.6), so e.g. an `sh:or` of datatype branches over `geo:asWKT` values
  mis-fired on every geometry.
- **Node-level `sh:nodeKind`**: `sh:Literal` could never match (focus nodes are strings);
  a blank/scheme-shaped/other heuristic now classifies them.
- **Cross-store path-cache poisoning**: the per-thread SHACL property-path cache was
  keyed by `(focus, path)` only and rayon worker caches survive across validation passes,
  so two stores in one process sharing a focus IRI + path could serve each other stale
  values — nondeterministic validation results. Cache keys now include a process-unique
  per-store id.
