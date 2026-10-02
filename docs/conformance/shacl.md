# SHACL — results on the W3C SHACL test suite

The `core` and `sparql` sections of the W3C Data Shapes Working Group's **SHACL test
suite** are vendored under
[`tests/fixtures/w3c-shacl/`](../../tests/fixtures/w3c-shacl/PROVENANCE.md) and run in CI
via [`tests/w3c_shacl_conformance.rs`](../../tests/w3c_shacl_conformance.rs).

The counts below are development and regression results on those sections, at the
comparison level described below (not full result-set equality). They are not a claim
of conformance to the W3C SHACL Recommendation, and W3C has not reviewed or endorsed
them. The tests are redistributed under the W3C Software and Document License — see
[`PROVENANCE.md`](../../tests/fixtures/w3c-shacl/PROVENANCE.md) there.

## Results (2026-09-10)

| | core | sparql | total |
|---|---|---|---|
| **Pass** | **97** | **22** | **119** |
| Known-fail (ratcheted) | 1 | 1 | 2 |
| Skipped (auxiliary `-data`/`-shapes` files, no test entry) | 15 | 0 | 15 |
| Total files | 113 | 23 | 136 |

*(Previous baselines: 2026-06-11, core only: 97 pass / 1 known-fail; 2026-06-10:
46 pass / 52 known-fail — see "Typed-term engine refactor" below for what closed
that gap.)*

The `sparql` section (vendored 2026-09-10) covers `sh:sparql` constraints on node
and property shapes, `sh:prefixes` (including `owl:imports`), custom constraint
components (`sh:validator` / `sh:nodeValidator` / `sh:propertyValidator`, optional
parameters) and pre-binding. Seven of its cases — `pre-binding/unsupported-sparql-*`
and `pre-binding-006` — expect the validator to *reject* the shapes graph
(`mf:result sht:Failure`); the runner passes those when validation returns an
error, and fails them when a report comes back.

**Comparison level:** `sh:conforms` plus the multiset of violation **focus nodes**
(IRIs/literals by lexical form, blank nodes by count). Full result-set equality
(constraint-component IRIs, `sh:resultPath`, `sh:value`) is a tracked refinement — the
engine currently reports the source constraint as a display string, not a component IRI.

**Gap policy:** a two-way ratchet. Every test not listed in `KNOWN_FAILURES` must pass,
and every listed test must still fail — silent regressions *and* silent fixes both turn
CI red, so the list cannot go stale.

## Remaining known failures

- **`sparql/pre-binding/shapesGraph-001.ttl`** — the constraint reads the shapes
  graph through `$shapesGraph` / `$currentShape`. SHACL §5.3.1 leaves those two
  variables to processors that expose the shapes graph to constraint queries;
  this one pre-binds `$this`, `$value`, `$PATH` and the component parameters and
  evaluates against the data graphs only, so a constraint that uses them fails
  the shapes graph (loudly, as a load error) instead of producing the report the
  test expects.
- **`core/property/uniqueLang-002.ttl`** — the test asserts that
  `sh:uniqueLang "1"^^xsd:boolean` does **not** activate the constraint (the spec
  activates it only for the literal `true`). Oxigraph's storage encodes
  `xsd:boolean` natively and reads the literal back in canonical form (`"1"` →
  `"true"`), so the distinction is unrecoverable after loading. This is a storage
  canonicalisation property, not an engine gap; fixing it would require keeping
  the original lexical form alongside every stored literal. The same storage
  property makes `sh:datatype` reject valid values of the derived integer types
  and `xsd:dateTimeStamp`, which read back as `xsd:integer` / `xsd:dateTime`; no
  suite case covers that (`datatype-ill-formed` passes because its values are
  ill-formed anyway). Both are pinned by `pinned_*` tests in
  `tests/shacl_conformance.rs`, and the dataset and SHACL Studio shapes uploads
  refuse non-canonical booleans on activation flags (see
  [shacl.md](../shacl.md#literal-forms-the-engine-cannot-see)).

## Beyond the suite: fail-open gaps (2026-10-01)

The suite has no case for several places where the engine let data through that
the shapes forbid. They are covered by `tests/shacl_conformance.rs` and
`tests/shacl_rules_conformance.rs` instead:

- **Multi-valued parameters**: only the first value of `sh:not`, `sh:hasValue`,
  `sh:pattern` and `sh:qualifiedValueShape`, and the first list of `sh:and`,
  `sh:or` and `sh:xone`, was read. Each value is now its own constraint
  (SHACL §4).
- **`sh:deactivated`** was honoured on top-level shapes only. It now applies to
  property shapes and inline shapes as well, with the spec's meaning (every
  term conforms, so `sh:not` of a deactivated shape fails).
- **Ill-formed paths** (a property shape with no `sh:path`, several, or one that
  is no well-formed path) skipped the property shape with a warning; they now
  fail the shapes graph like every other load error.
- **A SPARQL target that errors when it runs** yielded no focus nodes; it now
  fails the run.
- **Triple rules with node-expression terms** (`sh:object [ sh:path ex:p ]`)
  wrote the shapes graph's blank node into the data; they are refused at load
  until node expressions are implemented.

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

Running the W3C suite (rather than only in-house tests) immediately surfaced and
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
