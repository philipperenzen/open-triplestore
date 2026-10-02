# SHACL — results on the W3C SHACL test suite and the SHACL-AF tests

The `core` and `sparql` sections of the W3C Data Shapes Working Group's **SHACL test
suite** are vendored under
[`tests/fixtures/w3c-shacl/`](../../tests/fixtures/w3c-shacl/PROVENANCE.md) and run in CI
via [`tests/w3c_shacl_conformance.rs`](../../tests/w3c_shacl_conformance.rs).

The counts below are development and regression results on those sections, at full
report equality (described below). They are not a claim of conformance to the
W3C SHACL Recommendation, and W3C has not reviewed or endorsed them. The tests are
redistributed under the W3C Software and Document License — see
[`PROVENANCE.md`](../../tests/fixtures/w3c-shacl/PROVENANCE.md) there.

## Results (2026-10-02)

| | core | sparql | total |
|---|---|---|---|
| **Pass** (full report equality) | **97** | **22** | **119** |
| Known-fail (ratcheted) | 1 | 0 | 1 |
| Optional feature, unsupported (failure reported as the spec requires) | 0 | 1 | 1 |
| Skipped (auxiliary `-data`/`-shapes` files, no test entry) | 15 | 0 | 15 |
| Total files | 113 | 23 | 136 |

*(Until 2026-10-02 the runner compared `sh:conforms` and the focus-node multiset
only; full report equality, added as a second level, matched it on every case and
replaced it. Previous baselines: 2026-09-10: 119 pass / 2 known-fail, focus nodes only, with
`shapesGraph-001` counted as a failure; 2026-06-11, core only: 97 pass / 1
known-fail; 2026-06-10: 46 pass / 52 known-fail — see "Typed-term engine refactor"
below for what closed that gap.)*

The `sparql` section (vendored 2026-09-10) covers `sh:sparql` constraints on node
and property shapes, `sh:prefixes` (including `owl:imports`), custom constraint
components (`sh:validator` / `sh:nodeValidator` / `sh:propertyValidator`, optional
parameters) and pre-binding. Seven of its cases — `pre-binding/unsupported-sparql-*`
and `pre-binding-006` — expect the validator to *reject* the shapes graph
(`mf:result sht:Failure`); the runner passes those when validation returns an
error, and fails them when a report comes back.

**Comparison.** The runner's test, `w3c_shacl_full_report_equality`, compares
`sh:conforms` and the multiset of **results**, each on `sh:focusNode`,
`sh:resultPath` (as a path structure), `sh:value`, `sh:sourceShape`,
`sh:sourceConstraintComponent`, `sh:resultSeverity`, `sh:sourceConstraint` and
any other property of the result node (since 2026-10-03) — everything except
`sh:resultMessage`, whose wording the spec leaves to the processor. Our side is the RDF report the engine writes
  (`src/shacl_studio/report_rdf.rs`), loaded back into the store, so the RDF
  serialisation is tested as well. Blank nodes of the data graph (focus nodes,
  values) match any blank node; a blank-node shape or `sh:sparql` node must be the
  very node of the shapes graph. Literals are compared with their datatype and
  language tag, after the store has read both sides back (see the storage note
  under the known failures). Because the expected report goes through the same
  storage, a canonicalisation that changes both sides alike does not show here:
  `core/property/datatype-ill-formed` passes because its ill-formed literals are
  stored as written.

**Gap policy:** a two-way ratchet. Every test not listed in `KNOWN_FAILURES` must
pass, and every listed test must still fail — silent regressions *and* silent fixes
both turn CI red, so the list cannot go stale.

**Optional features.** `OPTIONAL_UNSUPPORTED` lists tests of a feature the
specification makes optional and requires a processor without it to report as a
failure. Such a test passes when validation fails with that failure, and fails if
the processor ever produces a report for it instead.

## Optional and unsupported

- **`sparql/pre-binding/shapesGraph-001.ttl`** — the constraint reads the shapes
  graph through `$shapesGraph` / `$currentShape`. SHACL §5.3.1 makes both
  variables optional, and a processor that does not support them must report a
  failure when it meets a constraint that uses them. This processor pre-binds
  `$this`, `$value`, `$PATH` and the component parameters and evaluates against
  the data graphs only, so a constraint that uses either variable fails the shapes
  graph at load, naming the variable. The test's expected report assumes a
  processor that supports them; [w3c/data-shapes#426](https://github.com/w3c/data-shapes/issues/426)
  contests the test for that reason, and SHACL 1.2 SPARQL Extensions drops both
  variables. The runner checks that the failure is reported.

## Remaining known failures

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

## SHACL Advanced Features — TopQuadrant's tests (2026-10-03)

No W3C test suite covers the SHACL Advanced Features Note (2017), so the
SHACL-AF tests of TopQuadrant's SHACL API are vendored under
[`tests/fixtures/shacl-af-topquadrant/`](../../tests/fixtures/shacl-af-topquadrant/PROVENANCE.md)
(Apache-2.0) and run in CI via
[`tests/shacl_af_corpus.rs`](../../tests/shacl_af_corpus.rs): the
`expression/`, `function/`, `rules/` and `target/` directories at commit
`6687b48bd2c81eda369f224598061f87dce0d425`. The counts are development and
regression results on those files, not a claim of conformance to anything, and
TopQuadrant has not reviewed them.

| | expression | function | rules | target | total |
|---|---|---|---|---|---|
| **Pass** | **1** | **1** | **7** | 0 | **9** |
| Expects behaviour outside the spec (failure reported as the spec requires) | 0 | 0 | 0 | 1 | 1 |
| Known-fail (ratcheted) | 0 | 0 | 0 | 0 | 0 |
| Cases | 1 | 1 | 7 | 1 | 10 |

*(2026-10-02: 9 pass / 1 known failure, validation cases compared on
`sh:conforms` and focus nodes only, with `sparqlTarget-001` counted as a
failure.)*

The tests use TopQuadrant's `dash:` test vocabulary, one self-contained file
per case (data, shapes and expected outcome in one graph, merged with the
sibling files it `owl:imports`). Comparison levels:

- **`dash:GraphValidationTestCase`** — `sh:conforms` and the full multiset of
  results, the same comparison as for the W3C suite above
  (`tests/common/shacl_report.rs`, shared by both runners): every result
  property except `sh:resultMessage`, our side being the RDF report the engine
  writes, data blank nodes as wildcards. The expected report is the
  `dash:expectedResult` node of the test case, in the file's own graph (the W3C
  files put it under `mf:result` instead). Any result property beyond the
  standard ones — a result annotation — is compared too;
- **`dash:InferencingTestCase`** — the exact set of inferred triples (what the
  rules write, less what the file asserts) against `dash:expectedResult`;
- **`dash:FunctionTestCase`** — the value of the SPARQL expression in
  `dash:expression`, evaluated in a run of the file's shapes graph, against
  `dash:expectedResult` (term equality).

Same two-way ratchet as above, plus no skips allowed and a floor of 9 passes.
Moving to full report equality found one gap, now fixed: an expression
constraint's result did not name the node expression as its
`sh:sourceConstraint`, which SHACL-AF §7 requires (`expression/booleans-001`).

**Expects behaviour outside the spec.** `NON_SPEC_EXPECTATIONS` lists cases
whose expected outcome rests on TopBraid behaviour the specification does not
have, where the specification requires a failure. Such a case passes when
validation fails with that failure, and fails if a report ever comes back.

- **`target/sparqlTarget-001.test.ttl`** — the target's `sh:select` uses the
  `owl:` prefix, but the ontology its `sh:prefixes` names has no `sh:declare`
  for it. TopBraid falls back to the Turtle prefixes of the file it loaded.
  The SHACL prefix mechanism (SHACL §5.2.1, which SHACL-AF reuses for targets)
  has no such fallback: the query does not parse with the declared prefixes,
  so the shapes graph is ill-formed, and validation fails with "Prefix not found".

Every SHACL-AF feature is covered by `tests/shacl_conformance.rs` and
`tests/shacl_rules_conformance.rs` as well, including the one these tests do
not reach: result annotations (§4, `result_annotations_*`).

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
  wrote the shapes graph's blank node into the data. They were refused at load
  on 2026-10-01, and are evaluated as node expressions since 2026-10-02 (see the
  SHACL-AF section above).

## Beyond the suite: SHACL-SPARQL and multi-graph runs (2026-10-02)

Also covered by `tests/shacl_conformance.rs` and
`tests/shacl_rules_conformance.rs`, not by suite cases:

- **Blank-node focus and value nodes** were skipped by `sh:sparql` constraints
  and constraint-component validators: the pre-bound value was pasted into the
  query text, and no SPARQL syntax names a stored blank node. `$this`, `$value`
  and the parameters are now pre-bound as RDF terms the way SHACL Appendix A
  defines it: every basic graph pattern, path and `GRAPH ?g` block of the query
  is joined with a one-row table of the values (a `BIND` of a function that
  returns the term, since `VALUES` cannot hold a blank node), so they reach
  every scope and the query optimizer still sees them bound.
- **`?failure`** bound to `true` is a failure (reported, so the node does not
  conform); `?message` and `{?var}` / `{$var}` message templates are filled from
  the solution; `?path` is used only when it is an IRI (SHACL §5.3, §5.3.2).
- **`sh:deactivated`** on a SPARQL-based constraint removes its results; on a
  validator it takes the validator out.
- **Component parameters**: only the first value was read. Each value of a
  single-parameter component is now its own constraint, and several values for
  a parameter of a multi-parameter component fail the shapes graph (SHACL §4).
  A sub-select must project every pre-bound variable, parameters included
  (Appendix A).
- **Multi-graph runs**: `sh:path` from an IRI focus node was evaluated inside
  each data graph in turn, so a path crossing graphs found nothing while
  `sh:sparql` and blank-node focus nodes saw the merge. Paths now read the
  merge of the data graphs (SHACL §3.4) for every focus node.

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
- **`sh:value` of SPARQL-based results** (2026-10-02, found by the full-report
  level): when a `sh:sparql` constraint or a `sh:select` validator on a node
  shape leaves `?value` unbound, SHACL §5.3.2 makes the focus node the value;
  the engine reported none (8 `sparql` cases).
- **String-typed focus/value nodes** (the typed-term refactor above) — 51 additional
  suite tests fixed in one contained refactor.
