# Improvement log

Running record of the improvement programme in
[open-triplestore-painpoints.md](open-triplestore-painpoints.md): one entry per
item, verify-first. An item is either implemented (failing test, then passing,
plus docs), "already fixed at `<commit>`" (the probe passed before any change),
or "blocked" with the diagnosis. Branch `feat/improvements` off `develop`;
build and test environment: the Dockerfile's `chef` stage
(`docker build --target chef`), `cargo test --workspace --features
full,saml,test-utils,backup-encrypt,alerting,plugin-hello,plugin-accounts-dashboard --locked`.

## Phase P0 — correctness and trust (2026-09-10)

### 1. `/sparql/batch` atomicity — implemented

- **Verified first.** The audit's probe (`INSERT DATA …` then `THIS IS NOT
  SPARQL`) already applied nothing: the route parses and authorises every
  statement before calling the engine, so a syntax error was a 400 with no
  side effect. The real gap was a statement that parses but fails at
  execution (`DROP GRAPH <missing>` without `SILENT`): statement 1 stayed
  applied, the response said `partial`. `tests/sparql_batch_http.rs` pins both.
- **Fix.** `TripleStore::batch_update` parses everything, then runs the
  updates on one `Store::start_transaction()` via
  `PreparedSparqlUpdate::on_transaction` (oxigraph 0.5.9), commits once and
  rebuilds the count index once; the first failure drops the transaction.
  The engine returns `BatchStatement::{Applied, Failed(msg), RolledBack}` per
  statement. HTTP: `status: ok` unchanged; a failed batch is `status:
  rolled_back` with per-statement `results` (`error` on the failing one,
  `rolled_back` on the rest — never `ok`). The 200 on a rolled-back batch is
  kept for compatibility with the previous `partial` shape (changing it is a
  status-code change the brief reserves for the user). OpenAPI and
  `docs/api-reference.md` describe the contract; the OpenAPI entry used to
  promise a 204 the handler never sent.

### 2. Graph Store `PUT` atomicity — implemented

- **Verified first.** `tests/graph_store_put_atomicity.rs` races a reader
  (`COUNT(?s)`, which bypasses the count index) against six replaces of a
  12 000-quad graph; on the unmodified engine the reader saw 0 quads on read
  #8 (in-memory) and #31 (RocksDB): the chunked clear committed before the
  bulk load did. The parse-before-clear half was closed in #347 and stays
  pinned by `gsp_put_with_malformed_body_leaves_graph_intact`.
- **Fix.** The replace is one transaction — `clear_graph` plus every parsed
  quad inserted on the same `Transaction`, one commit — so a reader sees the
  old graph or the new one and a crash cannot leave the graph empty. Cheaper
  than the staging-graph + `MOVE` pattern of `snapshot.rs::restore` (that
  pattern clears, copies and drops inside its transaction; a direct replace
  does clear + insert). The count index is set to the payload's distinct
  quads after the commit. `clear_graph_chunked` stays for `DELETE`.
- **Cost.** Measured in-process on RocksDB (release-dev, mirror and cache
  off, idle machine, 900k-quad graph replaced by 900k quads): baseline
  17.7 s / 18.8 s, one transaction 32.3 s / 30.2 s (+70 %); a first PUT into
  an empty graph 5.6 s → 12.5 s on the transactional path. Both exceed the
  brief's +20 % budget, so the session stopped and asked. Decision (user):
  keep the atomic replace and route an empty target (first PUT, boot seed)
  through the bulk loader, where nothing can be observed half-replaced —
  so the boot-seed case stays at 5.6 s and only a large replace pays. The
  same trade the version restore already makes. `Transaction::insert` is
  ~2.4× slower per quad than the bulk loader and the clear inside the
  transaction ~20 s vs ~13 s chunked; documented in `docs/performance.md`.
- **Not testable without a fault hook:** "a failing insert leaves the old
  contents intact". Inside the transaction `Transaction::insert` cannot fail
  and `commit` failing is a storage error; there is no injection point short
  of a mock storage. The parse-failure case (the only externally triggerable
  failure) is pinned; the crash-safety argument is the single commit.

### 3. Read isolation — not observable; misleading comment removed

- **Verified first.** `tests/read_isolation.rs`: a writer flips `<urn:p>` and
  `<urn:q>` of 400 subjects together in one `DELETE/INSERT` (one transaction)
  for three seconds while a reader joins the two and asks for subjects where
  they disagree. In memory: 690 reads over 287 flips; RocksDB: 119 reads over
  43 flips; zero torn reads, on the unmodified engine.
- **Why.** Oxigraph 0.5.9 binds one storage snapshot per query
  (`PreparedSparqlQuery::on_store` → `storage().snapshot()` once, every
  iterator reads through it). The `opengraph/src/mvcc.rs` claim of a
  snapshot per iterator described oxigraph 0.4's internals at best and is
  now replaced by the actual behaviour; the test stays as the pin. No
  production code changed.

### 4. Pin the already-fixed gate/cache findings

- **4a POST merge under `sh:maxCount 1` → 422: already fixed at `1eb59e4`
  (#347)** — `validate_on_write` and `check_write_gates` stage existing graph
  + payload for a merge. The sharper probe (second `ex:name` on a node that
  already has one) passed on the first run; it is now
  `shacl_on_write_post_second_value_under_max_count_1_is_422` in
  `tests/api_protocol_conformance.rs`.
- **4b fail-closed on a malformed `sh:sparql` → 422, never 204: NOT fixed —
  implemented.** Both the dataset `shacl_on_write` path and a Studio gating
  pipeline answered 204: `constraints.rs` swallowed the query error with
  `if let Ok(..)`, and `load_shapes` dropped any shape that failed to load
  with a warning. Fix in `src/shacl`: a `sh:select` is parse-checked at load
  time exactly as it runs (`$this` bound, `sh:prefixes` prepended), a shape
  that fails to load fails the run, a runtime SPARQL error is a Violation of
  the focus node, and the legacy path maps an engine error to the same 422
  gate-evaluation report the Studio gate uses (it was a 500). Making
  `load_shapes` propagate errors exposed the failure the warning had hidden:
  blank-node shapes (inline `sh:and`/`sh:or`/`sh:node` members, which the
  shape query also returns) failed target loading on an invalid `<_:…>`
  SPARQL IRI; class/predicate targets now resolve through the quad index like
  `sh:targetNode`. The W3C SHACL Core ratchet (97/1/15) and every SHACL,
  Studio, bundle and pipeline suite are unchanged.
  Follow-up (not in scope): inline shapes loaded through `load_inline_shape`
  are still skipped with `if let Ok(..)` at six call sites, and a malformed
  `sh:target/sh:select` (SPARQL target) yields no focus nodes rather than an
  error.
  "Unreadable shapes graph → 422" was already fixed at `1eb59e4` and is
  pinned by `a_gating_pipeline_whose_shape_graph_is_missing_refuses_the_write`
  plus the `gate.rs` unit tests; a *legacy* dataset whose configured shapes
  graph is simply empty still conforms (no shapes) — a configuration state,
  not an evaluation failure, left as is.
- **4c no stale cache hit under a concurrent write: already fixed at
  `1eb59e4` (#347)** and already pinned by
  `tests/query_cache.rs::concurrent_writes_never_leave_a_stale_count_cached`
  (200 writes against a hot `COUNT(*)`, monotonic) plus the unit test
  `a_result_computed_before_a_write_is_not_cached_as_fresh` in
  `src/store/query_cache.rs`. No change.

### 5. Official W3C SPARQL 1.1 manifests — implemented

- Vendored (user-approved) the query and update sections of
  `w3c/rdf-tests` `sparql/sparql11` at commit `369a90d` (2026-08-28):
  1.8 MB, 918 files, 485 entries (225 query evaluation, 111 query syntax,
  94 update evaluation, 55 update syntax) under
  `tests/fixtures/w3c-sparql11/` with PROVENANCE.md and the W3C licence.
  Not vendored: protocol, service-description, graph-store-protocol,
  federation, result-format and entailment sections.
- Runner `tests/w3c_sparql11_manifests.rs`: manifest-driven through
  `mf:include`, every entry through `TripleStore` (mirror and cache off);
  result-set isomorphism via the DAWG result-set vocabulary + oxrdf
  canonicalisation, graph/dataset isomorphism for CONSTRUCT and updates,
  numeric literals by value. Two-way ratchet + floor 450, same policy as the
  SHACL corpus. Baseline: **475 pass / 10 known-fail / 0 skips**; all ten
  are oxigraph 0.5 evaluator behaviours, documented in
  `docs/conformance/sparql11.md` (GRAPH ?g over quad-less patterns, 1.2
  GROUP_CONCAT language-tag rule, per-query BNODE(str), outer GRAPH implied
  into MINUS, zero-length paths on absent terms).
- `scripts/conformance_table.py` now scores every manifest-driven runner
  from its baseline comment (the SHACL-only special case generalised) and
  counts the vendored rows; README and docs/standards.md regenerated.
- **Ignored tests.** `cargo test … -- --ignored --list` lists exactly one:
  `api_comprehensive_test::bulk_insert_100k` (perf stress, documented,
  nightly `--ignored` workflow runs it). The audit's "4 ignored" counted the
  same test from four feature-combination builds. Nothing to fix.

### 6. Release hygiene — reviewed, stopped for the maintainer

See "Release hygiene findings" below.

## Checkpoint (2026-09-10, HEAD d674fad + this note)

| Check | Result |
|---|---|
| `cargo test --workspace --features full,saml,test-utils,backup-encrypt,alerting,plugin-hello,plugin-accounts-dashboard --locked --no-fail-fast` | 81 test binaries + 5 doc-test runs: **2955 passed, 0 failed, 1 ignored** (`api_comprehensive_test::bulk_insert_100k`, the documented perf stress test) |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --features <same> -- -D warnings` | clean |
| `scripts/conformance_table.py --check` | regenerated (three vendored corpora), check passes |
| Graph Store PUT replace, 900k quads, release-dev in-process | empty target 5.2 s (unchanged); replace 30.4 s vs 17.7–18.8 s before (accepted) |

Commits on `feat/improvements` (off origin/develop 619d3a5):
`02d460a` item 3, `69eb01f` item 4, `3eb1596` item 1, `5e8e102` item 2,
`d674fad` item 5, then this note. Nothing pushed, nothing tagged.

Follow-ups surfaced, not in P0's scope: `load_inline_shape` results are still
taken with `if let Ok(..)` at six call sites and SPARQL targets are not
parse-checked (same fail-open class as item 4b); a rolled-back batch still
answers 200 (status-code change reserved for the maintainer); the
`insert_quads_and_reindex_into(.., target_is_empty)` parameter now only
serves the empty-target PUT path; the count index is updated after the
replace commits, so a reader can see the old count for a moment (never 0).

**Handoff:** the next session runs phase P1 starting with item 1 (identity
policy: per-dataset `identity` setting, linksets out of reasoning sources,
`prov:specializationOf`/`alternateOf` never trigger `eq-rep-*`), on this
branch, with the container loop described at the top of this file.

## Release hygiene findings (item 6)

- **`v0.6.0` was never tagged.** `CHANGELOG.md` carries `## [0.6.0] —
  2026-07-31` (commit `b826db0 release: 0.6.0`, Cargo.toml 0.6.0), `origin/main`
  is at `f20a87b` (2026-08-12) and *includes* 21 post-release develop commits,
  yet the latest tag is `v0.5.0`. `main` was fast-forwarded to develop rather
  than merged through a PR titled with `major`/`minor`/`patch`, so
  `auto-tag.yml` never fired and no manual annotated tag was pushed:
  no GitHub Release, no GHCR `0.6.0` image, the CHANGELOG compare links for
  `[0.6.0]` and `[Unreleased]` dangle, and the next auto-tag (from `v0.5.0`)
  would stamp `v0.6.0` on whatever `main` holds at that moment, including
  post-0.6.0 work. The maintainer decides: tag `b826db0` as `v0.6.0`
  retroactively (release-process.md's manual path), or fold everything into a
  0.7.0 release.
- **`## [0.6.0]` section vs its range (`8b25cea..b826db0`, the content-equivalent
  of `v0.5.0..b826db0` — `v0.5.0` is not an ancestor of `main`, its history was
  replaced):** #233's 3-D sidebar grouping, Spark grounding rounds, guest chat
  rate limit, public dataset assets, dataset/organisation page rebuild and the
  demo-asset licence swap are missing; #257's `OTS_OIDC_SESSION_POLICY` /
  `OTS_OIDC_WRITE_SCOPES` and the closed token-escalation are missing; #227's
  MSRV bump 1.88 → 1.94.1 is missing; the prefix-seeding Added entry
  contradicts #258; the perf-gate "+10 %" and "jsdom 30 held / Node 20"
  entries were superseded within the range (#260: 1.15; #261: Node 24, jsdom
  30 landed).
- **`## [Unreleased]` vs `b826db0..619d3a5`:** #348 (RP-initiated logout,
  `prompt=none`) is absent; `### Security` says `None.` although #347 closed
  at least ten security findings (LDP PATCH without ACL, SWRL endpoint
  injection/auth, validate-and-commit cross-tenant graph, Studio unscoped
  introspection, Graph Store ACL/anonymous default graph, triple-label
  matching and fail-open, endpoint ACL API-wide, LLM guard bypass, remote
  allowlist prefix matching, private graphs in container/LDES export) and
  #293 scoped text-search hits; `### Removed` says `None.` although `f20a87b`
  removed the invented vocabularies; the encrypted-backup change (startup
  fails without a recipient) is unlisted; #321, #265, #267, #288, #295,
  #293, #290, #264, #262, #296 are missing or only partly covered; Changed
  still says `sd:BasicFederatedQuery` is not advertised while Added says
  `SERVICE` is allowlisted and advertised.
- **Licence presentation** is consistent (LICENSE, TERMS.md, NOTICE, README,
  CONTRIBUTING, Cargo.toml `license-file`): source-available AGPL-3.0 with
  the Commons Clause; `docs/overview.md` says nothing about the licence.
- **README claims stronger than `docs/standards.md` grades:** README:36 "full
  SPARQL 1.2 (RDF-star), GeoSPARQL 1.1, OWL 2" (standards grades them
  Partial); README calls the engine "RDF-star" where standards.md documents
  RDF 1.2 semantics; README:85/674 "full W3C DCAT 2" vs standards "DCAT 3 /
  DCAT-AP — Partial". The generated conformance table is the only counted
  claims surface and is regenerated in this branch (three vendored corpora).
- **No changelog restructuring was committed**: the brief's "draft the 0.6.0
  section" is moot (it exists); what is needed is the maintainer's decision
  on the tag plus the corrections above, which touch security claims.

## Phase P1 — semantics (2026-09-10)

### 1. Identity policy for `owl:sameAs` — implemented (`61da6a2`)

Probe first: the RL engine ran the Table 4 equality rules (`eq-sym`, `eq-trans`,
`eq-rep-s/p/o`) unconditionally, over every reasoning source including
`linkset`-role graphs, and the DL bridge's RL phase ran **unscoped** (it built
its own `Owl2RlReasoner` without `with_sources`, so it read the whole store).
A cross-dataset `owl:sameAs` in a linkset therefore merged both resources'
properties into the entailment graph.

Decision record. The brief asked for a per-dataset `identity ∈ {sameas-off,
sameas-narrow, sameas-full}`. Asked which default a dataset should get, the
maintainer first clicked `sameas-off` by accident and withdrew it ("do not do
sameas-off yet"); on re-presentation the instruction was: *configurable per
dataset or organisation, in settings, with a simple description and example
for users; disagree if you like*. Implemented as instructed: an
organisation-level setting inherited by the datasets it owns, a dataset-level
override, and a built-in fallback of **`sameas-narrow`** (equality rules run,
only over the dataset's own graphs) — the choice that keeps a single dataset's
reasoning intact while never letting a linkset merge resources unasked; it is a
one-line change (`IdentityPolicy::default`) to `sameas-full` if the
maintainer prefers. The Settings *UI* is `frontend/`, outside this phase's
edit scope — follow-up: two selects (organisation page, dataset page) over
`GET/PUT/DELETE /api/organisations/:id/identity` and
`/api/datasets/:id/identity`; the `GET` answers already carry
`identity_options` with the user-facing descriptions.

Mechanics: `src/reasoning/identity.rs` (`IdentityPolicy`, the
`CORRESPONDENCE_PREDICATES` that never feed equality: `prov:specializationOf`,
`prov:alternateOf`, the SKOS match properties, `rdfs:seeAlso`); the Table 4
block runs only when `propagates_same_as()`; `entailment.rs` keeps the setting
in its own lazily created SQLite table (`identity_policy(scope,id)`), computes
the effective policy (dataset → organisation → default) and filters
`GraphKind::Linkset` graphs out of the reasoning sources unless the policy is
`sameas-full`; `GET …/entailment` reports `identity`, `identity_source`,
`identity_options` and the effective `reasoning_sources`. The DL bridge's RL
phase is now scoped and carries the policy. Since correspondence predicates are
not `owl:sameAs`, no rule ever fires on them; the test pins that under every
policy. Docs: `docs/reasoning.md` (policy table, bridge/BIM example, curl),
`docs/named-graphs.md` (linkset role), the modelling styleguide (IRI rule
narrowed; `owl:sameAs` vs `prov:specializationOf` rows). Tests:
`tests/reasoning_http.rs::identity_policy` (5). No route, status code or JSON
field changed; the identity endpoints are additions (OpenAPI mounted).

### 2. `owl:hasKey`, OWL 2 RL Table 8, rule inventory, incremental materialisation — implemented (`1a101c9`)

- `prp-key` matched `rdf:first ?p ; rdf:rest rdf:nil` — single-property keys
  only; a composite key produced no `owl:sameAs` at all (probe:
  `prp_key_composite_merges_only_when_every_key_property_matches` failed).
  Keys are now read from the quad index (class + property list per
  `owl:hasKey`, list walk capped at 64 cells) and one INSERT per key joins
  every key property.
- Table 8: `dt-type1` declares the 32 RL datatypes as `rdfs:Datatype`
  once per run — which is why `dl_empty_store_ok` now expects 32 derived
  triples on an empty store; `dt-not-type` makes a literal outside its
  datatype's lexical space (`"abc"^^xsd:integer`) an `Inconsistency`, using
  the same lexical rules `sh:datatype` uses (`constraints::xsd_lexical_valid`,
  now `pub(crate)`). `dt-type2`, `dt-eq`, `dt-diff` need literal subjects and
  are documented as not representable in an RDF store.
- The module claimed "the complete OWL 2 RL rule set". It runs 63 of the 78
  RL/RDF rules; `IMPLEMENTED_RULES` / `UNIMPLEMENTED_RULES` (each with its
  reason) are pinned against the specification's list by
  `rule_inventory_is_the_whole_rl_rule_set`, and `docs/owl2-rl.md` lists both.
- Incremental per-dataset materialisation, scoped to the one write that only
  adds quads: a Graph Store `POST` calls `entailment::after_additive_write`,
  which extends the entailment graph (rules re-run to fixed point on top of
  the existing consequences) instead of clearing and rebuilding; every other
  write still rebuilds. Runs record the store write generation, so a write
  that changed nothing triggers no run, and an extension is only used after a
  successful full run.

Tests: `tests/owl2_rl_conformance.rs` (+7; the `ask_tg` helper gained
`xsd:`/`ex:` prefixes), `tests/owl2_dl_conformance.rs` (expectation updated).

### 3. NEN 2660-2 relation seed bundle — implemented (`40bedc3`)

`examples/seed-bundles/nen2660-relations/`: `relations-profile.ttl` gives
the seven relations the characteristics OWL can carry — `nen2660:hasPart`
transitive, `hasFunctionalPart`/`hasTechnicalPart` sub-properties of it (so
proper parthood is transitive through the hierarchy), `contains`,
`consistsOf`, `connectsObject`, `connectsPort` plain object properties (Keet,
Fernández-Reyes & Morales-González: containment, constitution and connection
are not transitive) — and `shapes.ttl` the ones it cannot, as SHACL-SPARQL:
acyclic decomposition (`(hasPart|hasFunctionalPart|hasTechnicalPart)+` back
to `$this`), irreflexive containment/connection, a part's geometry
`geof:sfWithin` its whole's and an RCC8 proper part of it
(`rcc8tpp`/`rcc8ntpp`/`rcc8eq`), a contained object within its region. The
sample decomposes a bridge and plants a cycle, an escaped part, an outside
pump and a self-containment; `tests/nen2660_relations_bundle.rs` asserts
exactly those six (shape, focus) results and that `hasPart` is transitive
under OWL 2 RL while `contains` is not. The NEN 2660-2 RDFS file is fetched
(`fetch.sh`, maintainer approved the download) and git-ignored; the bundle
end-to-end test skips green without it. `docs/plugins.md` lists the bundle;
the styleguide gains "Part-whole, containment and connection".

### 4. SHACL-AF completion and strict SHACLC — implemented (`ccf164a`)

Vendored the W3C SHACL test suite's `sparql/` section (maintainer approved;
28 files, w3c/data-shapes `9c86396`; PROVENANCE.md updated). First run of the
new material — components, pre-binding, rule modifiers, strict SHACLC — was
15/23 on `sparql/`; what the suite found:

- the per-thread constraint-component cache was keyed on (shapes graph,
  write generation) and served one store's declarations to the next store
  with the same graph name and generation (every test store). `TripleStore`
  now has a process-unique `instance_id` (shared by clones) and the key
  includes it;
- `bound($this)` → `true` is not a SPARQL `Constraint`; it is `(true)`;
- `$PATH` in a `sh:sparql` on a property shape was left as a variable and
  matched every predicate (`sparql/property/sparql-001`);
- `sh:prefixes` did not follow `owl:imports` (`sparql/node/prefixes-001`);
- a triple rule's literal object lost its datatype (`sh:object true` was
  inserted as the string `"true"`) — a pre-existing bug the new
  `sh:order`/`sh:condition` tests tripped over.

Final: `core` 97 / 1 known-fail / 15 aux skips (unchanged), `sparql` 22 / 1
/ 0; runner floor 110, skip ceiling 20, `sht:Failure` cases pass when
validation returns an error. The remaining `sparql` gap is
`pre-binding/shapesGraph-001` (`$shapesGraph`/`$currentShape`); a constraint
that uses them fails the shapes graph at load — the honest behaviour for a
processor that does not expose the shapes graph — rather than passing with
the variables unbound. The §5.3.2 restrictions (`MINUS`, `VALUES`, `SERVICE`,
nested `SELECT` not projecting `$this`, `AS $this`) are detected at load time
by a small lexer (`prebinding_violation`), so an ill-formed shapes graph fails
to load and the write gate built on it fails closed. SHACLC: `parse` is strict
(position-naming error), `parse_lenient` keeps the old behaviour,
`?lenient=true|1` on `PUT …/shapes` and `POST /api/shaclc/parse` selects it —
a 400 on input that previously produced an *empty* shapes graph and a 200; no
existing status code or field changed (`docs/api-reference.md`, OpenAPI).

Tests: `tests/shacl_conformance.rs` (+4), `tests/shacl_rules_conformance.rs`
(+3), `tests/shaclc_conformance.rs` (lenient test replaced by strict + lenient
+ HTTP `?lenient`), `tests/w3c_shacl_conformance.rs` (two sections).

## Checkpoint (2026-09-10, HEAD `ccf164a` + this note)

| Check | Result |
|---|---|
| `cargo test --workspace --features full,saml,test-utils,backup-encrypt,alerting,plugin-hello,plugin-accounts-dashboard --locked --no-fail-fast` | 82 test binaries + 5 doc-test runs: **2984 passed, 0 failed, 1 ignored** (`api_comprehensive_test::performance::bulk_insert_100k`, the documented perf stress test); ~11.5 min in the container |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --features <same> -- -D warnings` | clean |
| `scripts/conformance_table.py --check` | regenerated (test counts; W3C SHACL row 136 cases / 119 pass / 2 known), check passes |

Commits on `feat/improvements` after P0's `d674fad`/`f105ed3`: `61da6a2` item 1,
`1a101c9` item 2, `40bedc3` item 3, `ccf164a` item 4, then this note.
Nothing pushed, nothing tagged.

Notes and follow-ups (none blocking):

- `saved_queries::testing::tests::version_bump_records_ok_then_changed` failed
  once during item 2 (orders by a second-resolution timestamp) and passed on
  rerun; pre-existing, unrelated, not touched.
- `scripts/conformance_table.py` labels the W3C SHACL runner "SHACL Core" and
  prints "floor ≥90"; the runner now covers Core + SHACL-SPARQL with floor 110.
  `scripts/` is outside this phase's edit scope — a two-constant change
  (`CORPUS` label, `CORPUS_RUNNERS` floor) for the maintainer.
- Identity policy Settings UI (`frontend/`, out of scope) — see item 1.
- `docs/notes/open-triplestore-painpoints.md`, which this log links to, is not
  in the repository (the programme text lives in the session brief); either
  commit it or drop the link.
- The reasoning-source filter for linksets lives in `entailment.rs`
  (`reasoning_sources`) because `conformance::resolve` is used by
  non-reasoning callers; a caller that reaches `conformance::resolve(...)
  .reasoning_sources` directly still sees linksets.

**Handoff:** the next session runs phase P2 — *design notes only*
(`docs/notes/analytical-mirror-design.md`, `docs/notes/shacl-to-sql-design.md`,
the repair layer and delta-versioning notes); implementation needs explicit
approval. Same branch, same container loop.
