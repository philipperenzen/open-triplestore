# SHACL conformance — official W3C test suite

The official **W3C SHACL test suite** (core section) is vendored under
[`tests/fixtures/w3c-shacl/`](../../tests/fixtures/w3c-shacl/PROVENANCE.md) and runs in CI
via [`tests/w3c_shacl_conformance.rs`](../../tests/w3c_shacl_conformance.rs).

## Scorecard (2026-06-11)

| | count |
|---|---|
| **Pass** | **97** |
| Known-fail (ratcheted) | 1 |
| Skipped (auxiliary `-data`/`-shapes` files, no test entry) | 15 |
| Total files | 113 |

*(Previous baseline, 2026-06-10: 46 pass / 52 known-fail — see "Typed-term engine
refactor" below for what closed the gap.)*

**Comparison level:** `sh:conforms` plus the multiset of violation **focus nodes**
(IRIs/literals by lexical form, blank nodes by count). Full result-set equality
(constraint-component IRIs, `sh:resultPath`, `sh:value`) is a tracked refinement — the
engine currently reports the source constraint as a display string, not a component IRI.

**Gap policy:** a two-way ratchet. Every test not listed in `KNOWN_FAILURES` must pass,
and every listed test must still fail — silent regressions *and* silent fixes both turn
CI red, so the list cannot go stale.

## Remaining known failure

- **`property/uniqueLang-002.ttl`** — the test asserts that
  `sh:uniqueLang "1"^^xsd:boolean` does **not** activate the constraint (the spec
  activates it only for the literal `true`). Oxigraph's storage encodes
  `xsd:boolean` natively and reads the literal back in canonical form (`"1"` →
  `"true"`), so the distinction is unrecoverable after loading. This is a storage
  canonicalisation property, not an engine gap; fixing it would require keeping
  the original lexical form alongside every stored literal.

## Typed-term engine refactor (2026-06-11)

The previous engine carried focus nodes and value nodes as **lexical strings**, losing
term kind, datatype and language at target resolution — the root cause of 50 of the 52
former known-failures. The engine (`src/shacl/engine.rs`, `src/shacl/constraints.rs`,
`src/shacl/shapes.rs`) is now typed on `oxigraph::model::Term` end-to-end; report
fields are still rendered as the historical display strings (bare IRI / literal lexical
form / `_:label`), so the HTTP/JSON report shape is unchanged. What that fixed:

1. **Node-level value constraints** evaluate against the typed focus node: `sh:datatype`
   (including ill-formed-literal detection, e.g. `"aldi"^^xsd:integer` and the
   out-of-range `"300"^^xsd:byte`), numeric/temporal range constraints, `sh:languageIn`,
   `sh:minLength`/`sh:maxLength` (IRIs by IRI string, blank nodes always violate),
   `sh:class` (literals never match), `sh:nodeKind` (exact, no heuristics).
2. **Typed comparison** for range and pair constraints (`sh:lessThan*`,
   `sh:min/maxInclusive/Exclusive`): numeric promotion across the XSD numeric family,
   `xsd:dateTime`/`xsd:date` with the XSD 1.1 partial order (mixed timezoned/naive
   values are indeterminate within ±14 h and indeterminate comparisons violate),
   string and boolean comparison; incomparable or non-literal values violate per spec.
3. **Result cardinality**: value nodes form a *set* (distinct terms — duplicate SPARQL
   path bindings collapse, `path-sequence-duplicate-001`); `sh:equals` reports one
   result per value in the symmetric difference; `sh:uniqueLang` reports one result per
   duplicated language tag; `sh:closed` reports one result per offending
   `(predicate, value)` pair and now honours the shape's own property-shape paths as
   allowed properties.
4. **Paths**: top-level property shapes (`ex:S a sh:PropertyShape ; sh:path … ;
   sh:targetNode …`) evaluate their constraints along their own path; sequence /
   alternative / zeroOrMore / oneOrMore / zeroOrOne values resolve as typed terms; a
   path node carrying both list cells and a path operator is read as the sequence path
   (`path-strange-*`); blank-node and literal focus nodes are walked natively over the
   raw quad index (SPARQL cannot address stored blank nodes).
5. **Shape-based constraints**: `sh:node` loads its (possibly blank inline) shape
   eagerly with a load-depth guard; nested `sh:property` on property shapes validates
   each value node (`property/property-001`, `validation-reports/shared`);
   `sh:qualifiedValueShapesDisjoint` excludes value nodes conforming to sibling
   qualified shapes.
6. **Targets**: `sh:targetClass` (and implicit class targets) resolve SHACL instances
   via `rdf:type/rdfs:subClassOf*`; `sh:targetNode` keeps the typed term (literal
   targets work); `sh:targetObjectsOf` literals carry their datatype.
7. **Result metadata**: shape-declared `sh:message` overrides the engine default on
   that shape's results; `sh:severity` on property shapes overrides the parent's.

## Engine fixes driven by this suite

Running the official suite (rather than only in-house tests) immediately surfaced and
fixed real engine bugs:

- **Value-node semantics** for `sh:not`/`sh:and`/`sh:or`/`sh:xone`/`sh:node` in property
  context: these were evaluated against the focus node instead of each value node along
  the path (SHACL §4.6), so e.g. an `sh:or` of datatype branches over `geo:asWKT` values
  mis-fired on every geometry.
- **Node-level `sh:nodeKind`**: `sh:Literal` could never match (focus nodes were
  strings); focus nodes are now typed terms, so classification is exact.
- **Cross-store path-cache poisoning**: the per-thread SHACL property-path cache was
  keyed by `(focus, path)` only and rayon worker caches survive across validation passes,
  so two stores in one process sharing a focus IRI + path could serve each other stale
  values — nondeterministic validation results. Cache keys now include a process-unique
  per-store id.
- **String-typed focus/value nodes** (the typed-term refactor above) — 51 additional
  suite tests fixed in one contained refactor.
