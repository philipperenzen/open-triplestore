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
approval. Same branch, same container loop. *(Run: the SHACL→SQL note was held
by the maintainer and answered inside the analytical-mirror note — see below.)*

## P0 addendum — item 6 completed, follow-ups closed (2026-09-11)

### 6. Release hygiene — the CHANGELOG corrections (`b7f688b`)

P0 item 6 listed the defects of both sections and stopped, because the
corrections touch security claims. They are now written, commit by commit
from `git log 8b25cea..b826db0` (19 commits, `[0.6.0]`) and `b826db0..HEAD`
(37 commits, `[Unreleased]`), with every kept entry spliced verbatim.

- `[Unreleased] ### Security` said `None.`; it now carries eleven entries for
  the findings #347 closed (LDP `PATCH` without ACL, SWRL unauthenticated and
  injectable, `validate-and-commit` cross-tenant graph replace, Studio
  unscoped introspection reading the default graph, Graph Store reads ignoring
  `graph_acl` and dumping the default graph to anonymous, triple labels never
  matching and failing open, endpoint ACL on six routes and never for
  anonymous, the Spark guard exempting client-labelled assistant turns, remote
  allowlist raw-prefix matching, private graphs in container export and LDES)
  plus #293's scoped text-search hits. The last three concern features new in
  the range, and say so.
- `### Removed` said `None.`; `f20a87b` removed the invented vocabularies.
- Missing entries added: #348 (RP-initiated logout, `prompt=none`), the
  encrypted-backup startup refusal, #321, #265, #267, #288, #295, #290, #264,
  #262, #296; `[0.6.0]` gained #233's six items, #257's
  `OTS_OIDC_SESSION_POLICY` / `OTS_OIDC_WRITE_SCOPES` and closed token
  exchange, and #227's MSRV 1.88 → 1.94.1 with the builder images.
- Corrected: the perf gate is 1.15, not "+10 %" (#260); jsdom 30 landed on
  Node 24 (#261), not "held on Node 20"; basemap suppression had the primary
  path and the fallback the wrong way round; two entries described states that
  never shipped (the prefix-seeding fix, and `sd:BasicFederatedQuery` not being
  advertised — it is advertised when an allowlist is configured) and were
  dropped.
- **Not decided here:** `v0.6.0` was never tagged and the compare links dangle.
  That is the maintainer's call (tag `b826db0` retroactively, or fold into
  0.7.0); the links are untouched. Review the Security entries first.

### Fail-open follow-ups closed (`5954c88`)

The three the P0 log carried forward, each closed the way item 4b closed the
top-level case — a check that cannot be evaluated is a failure, never a pass.

- The six `if let Ok(load_inline_shape)` call sites propagate: an unloadable
  `sh:node`, `sh:not`, `sh:and`, `sh:or`, `sh:xone` or qualified-value member
  fails the shapes graph, which the write gate turns into a 422.
- A SPARQL target is parse-checked at load and must project `?this`
  (SHACL-AF §2.1.2); either failure fails the shapes graph. It used to be
  stored unchecked and its query error ignored at evaluation, so the shape
  passed over nothing.
- The Graph Store `GET` label gate refuses the read with 503 when the label
  table cannot be read, instead of `unwrap_or(false)` serving the graph
  unfiltered. `docs/security.md` says so.

Two fixtures the change exposed, both fixed rather than worked around:
`tests/waalbrug_conformance.rs` loaded `shapes-af.ttl` alone although that
file's own header says its rule, target and function bodies need the
`ex:prefixes` declaration from `shapes-sparql.ttl` — so its SPARQL target had
never parsed and that shape validated nothing; and a SPARQL target is
evaluated against the raw store with no `FROM` prologue for the run's data
graphs, so a target over a named graph matches nothing unless it names the
graph itself. The second is pre-existing and a behaviour change to close —
**follow-up, not done here.**

Tests: `shacl_conformance` (+2), `api_protocol_conformance` (+1, which drops
the label table underneath the handler). W3C SHACL ratchet unchanged
(core 97/1/15, sparql 22/1/0).

### Still open after P0 and P1

- **Maintainer decisions (stop conditions):** endpoint ACL default-open for
  unauthenticated requests (`src/auth/`); triple-label filtering on `/sparql`
  results (a design change, not a fix — `docs/security.md` states the scope
  honestly); a rolled-back `/sparql/batch` still answers 200.
- **Painpoints sub-items the phase specs did not include:** linkset SHACL
  shapes (no `owl:sameAs` between resources of different `dct:conformsTo`
  models unless whitelisted); the NEN 2660 "no object is part of two disjoint
  wholes" shape; the IFC relation mapping to the NEN relation family (that one
  belongs to P3's IFC lift item).
- **Already closed by earlier work, contrary to the audit:** SHACLC blank-node
  property-shape serialisation (pinned in `tests/shaclc_conformance.rs`),
  SHACL Studio HTTP tests (ten across two files), `prp-trp` (in
  `IMPLEMENTED_RULES`), licence presentation.
- **Found while censusing shape graphs, queued as its own task:** the
  implicit class target ASK is scoped to the shapes graph
  (`src/shacl/engine.rs:455`), so a node shape that is also a class declared
  in a *different* graph gets no focus nodes — 27 of the 30 NEN 2660 node
  shapes validate nothing under that bundle's graph split.

---

## Phase P2 — analytical layer, design notes only (2026-09-11)

Nothing was implemented. Three notes shipped in `d9ca0b7`; the fourth was
held by the maintainer and answered inside the first.

### Evidence base

Eleven read-only subsystem maps (1 136 file:line facts), a completeness
critic, eight follow-up reads (533 facts), a four-lens adversarial panel on
the author's draft position, then two independent drafts per note, judging,
synthesis, citation verification and repair — all against `ccf164a`. Two
follow-up findings changed the designs:

- An oxigraph `Transaction` **can** carry quad mutations and a SPARQL Update
  in one commit, because `PreparedSparqlUpdate::on_transaction` does not
  commit itself. So a commit record can be atomic on the paths that already
  hold a transaction (`batch_update`, the PUT replace path) and never on the
  bulk-loader paths, which ingest SST files.
- Oxigraph exposes **no** durable sequence number. RocksDB's is bound in the
  vendored C API (`db/c.cc:2436`) but the `storage` module is private, so the
  cursor has to be allocated by the platform.

### 2. `docs/notes/shacl-to-sql-design.md` — held by the maintainer

The maintainer asked, before the note was written, whether SHACL→SQL is
genuinely a good fit and why not SPARQL→SQL; after the evidence sweep the
steer was: Studio pipelines do validate large instance datasets so it may
apply, **9M must be measured**, instance data is routinely wrong after a model
update so bulk revalidation is a real workload, and **the reliable changelog
comes first**. The recorded decision, in the analytical-mirror note's §1:

1. **Per-quad change capture with a durable cursor is the first deliverable.**
   It is the prerequisite for any persisted substrate, for delta versions, for
   exact count-index maintenance and for changed-node scoping of validation.
2. **SHACL→SQL is deferred pending measurement, not rejected.** The gating
   experiment is specified: a post-rebuild whole-dataset Studio pipeline run at
   9M quads, plus the gate-sandbox cost, against thresholds written down now.
3. **The columnar substrate and `/sql` are not built now.** Adding a DuckDB or
   DataFusion dependency is outside the programme's edit scope, and a `/sql`
   endpoint needs `src/auth/middleware.rs`, which is too.

Four claims of the draft position the panel refuted, corrected in the note:
the write gate is **not** a per-delivery check of one payload (a gated `POST`
dumps and re-parses the whole target graph and validates every focus node of
the merge, so it *is* bulk many-targets Core validation on the hottest write
path; a gated `PUT` validates the whole payload); the engine's graph semantics
are **not** test-pinned (three disagreeing regimes coexist in one run and no
test names any of them, so there is no written contract for a second validator
to be equivalent to — which makes the liability argument stronger, not weaker);
SHACL Core is **not** a corollary of BGP + FILTER + GROUP BY (`sh:class` and
`sh:targetClass` need `rdfs:subClassOf*` closures, `zeroOrMorePath` is a
closure, `sh:node` recursion is bounded unfolding, `sh:closed` enumerates
predicates); and the builder already compiles RocksDB and GEOS from C++, so a
bundled C++ engine is an unmeasured cost rather than a new class of problem.

**The operational fact that dominates every number in this area:** the shipped
`docker-compose.yml` limit of 4 GiB makes the accelerator cap
`(4 GiB / 4) / 1024 = 1 048 576`, clamped up to the 2 000 000 floor — so a
default install never accelerates a store above 2M triples, and at 9M the
accelerator is **off**, where the same group-by takes about 11 s rather than
0.52 s. The 0.52 s figure was measured on a 54.9 GiB host. Fixing or
documenting that is the cheapest win in the whole analysis.

### 1, 3, 4. The three notes (`d9ca0b7`)

- **`analytical-mirror-design.md`** — the decision record above; what exists
  today to build on (the `query_uncached` chain and its insertion point, the
  publish/stale protocol, `opengraph::parallel::classify`, the count index,
  the result cache, the memory budget); substrate options with a
  recommendation; schema derivation and its typing problems; why "fed from the
  commit log" cannot be built as written; the SPARQL-subset→SQL router with
  the SPARQL semantics SQL gets wrong by default and a decline-rather-than-
  differ fidelity policy; the `/sql` endpoint with the readable-graph predicate
  reproduced as a row filter; publish/stale carry-over for a third copy; the
  telemetry phase, its thresholds, and how SHACL Core would ride on the
  translator instead of a separate compiler.
- **`repair-layer-design.md`** — TGD/EGD rule format with worked examples,
  chase semantics (restricted chase, labelled nulls, idempotent
  head-satisfaction, termination budget), where the chase runs and what it
  sees, the constraint-by-constraint repairability analysis, the proposal
  artefact, and `POST /api/datasets/:id/repair` proposing an RDF Patch that is
  never auto-applied — including the gate the existing patch route lacks.
- **`delta-versioning-design.md`** — the change-capture primitive per mutation
  primitive (which need a before-image scan and what it costs), where it is
  persisted without touching the RocksDB layout, the `row`/`seq` split with
  `seq` assigned inside the commit critical section, versions as checkpoint
  plus patch chain with lazy materialisation, concurrency, the RDF Patch
  extensions required, and RDF-star statement provenance with its cost and its
  downstream breakage budgeted.

## Checkpoint (2026-09-11, HEAD `5954c88` + this note)

| Check | Result |
|---|---|
| `cargo test` over the thirteen binaries the P0 follow-ups touch (shacl_conformance, api_protocol_conformance, w3c_shacl_conformance, shacl_studio_http, shacl_pipeline_integration, shacl_rules_conformance, shaclc_conformance, security_shacl_studio, ogc_geosparql_shacl_roundtrip, standards_conformance, standards_demo_e2e, nen2660_relations_bundle, waalbrug_conformance) | **139 passed, 0 failed, 0 ignored**; W3C SHACL 119 pass / 2 known-fail / 15 skips, unchanged |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --features <same> -- -D warnings` | clean |
| `scripts/conformance_table.py --check` | regenerated (SHACL Core 13→15, Protocol 16→17), check passes |

The full workspace suite was last run green at P1's checkpoint (2 984 passed);
this phase's code change is confined to the SHACL loader and one Graph Store
read path, and the thirteen binaries above cover both.

Commits after P1's `7986ab1`: `b7f688b` the CHANGELOG corrections (P0 item 6),
`d9ca0b7` the three P2 design notes, `5954c88` the fail-open follow-ups, then
this note. Nothing pushed, nothing tagged.

**Handoff.** Implementing any P2 item needs explicit approval. If approved, the
order the notes argue for is: (1) the query and validation telemetry
(§1.4 of the mirror note — in scope, small, and the only way the go/no-go
becomes mechanical); (2) per-quad change capture with a durable cursor (phase 1
of the delta-versioning note); (3) the 9M SHACL measurement; then decide P2-1
and P2-2 against the thresholds. Otherwise the programme's next phase is P3
(connectors and standards surface: R2RML SQL sources and joins, Ontop as an
allow-listed `SERVICE`, IFC lift depth, IDS export, LDES retention, DQV
shapes). Two things are waiting on the maintainer either way: the `v0.6.0`
tagging decision, and the three stop-condition items listed above.

---

## SHACL graph reach — scope fixed, incoherence measured (2026-09-11)

Raised by the maintainer from the P2 notes' remark that the engine has
"disagreeing graph-reach regimes with no test pinning any of them". Settled
with a five-topic investigation (W3C spec, blast radius, performance, peer
validators, baseline design) and a three-lens adversarial pass, which
overturned two of the author's starting claims. Commits `58099d9`, `c8280e8`,
`6ed571d`.

### What the engine actually does

When a run spans several data graphs, the constructs do not read the same
graphs. `sh:path` is evaluated inside each data graph in turn for an IRI focus
node and merged for a blank-node or literal one; `sh:sparql`, `sh:class`,
`sh:targetSubjectsOf`, `sh:targetObjectsOf` and `sh:closed` read the graphs
merged; `sh:targetClass` took its type triples per graph and, until this
change, its subclass chain per graph too. So one rule written two ways gave
opposite answers in a single run, and two constraints on one property shape
reached into different graph sets.

### What the specification settles

Validation is defined against **one** data graph, fixed before the descent into
shapes, focus nodes and constraints (§3.4). "Any RDF graph can be a data graph"
(§3.2) is the whole normative definition; the word *merge* does not appear in
the Recommendation, and no SHACL term takes a data-graph IRI, so per-graph
confinement is not expressible at all — only as N separate runs, each with its
own `sh:conforms`. `sh:targetClass` (§2.1.3.2) and `sh:class` (§4.1.1) are the
same relation, "SHACL instance of C in the data graph", evaluated at two
moments: giving them different reach implements one construct two ways. No peer
validator splits scope per construct — pySHACL merges unconditionally, RDF4J
merges by default, Jena and TopBraid take a single graph.

### Fixed

- **`58099d9` — SHACL-AF SPARQL targets were unscoped.** `sh:target
  [ sh:select … ]` ran against the bare store: any caller who could write a
  shapes graph selected focus nodes from every graph in the store, other
  tenants' included, and `sh:value` carried their terms back in the report.
  Targets now get the same `FROM <g>` prologue a `sh:sparql` constraint gets.
  This was the other half of `5954c88`, which made an unparseable target fail
  the shapes graph but left its scope open.
- **`c8280e8` — the conformed model was not in scope, and the subclass chain
  was read per graph.** Model graphs live in the model registry, never in
  `dataset_graphs`, so a dataset was validated without the model it declares
  `dct:conformsTo` — `sh:targetClass` on a superclass targeted nothing and
  `sh:class` against a model term failed, silently. The declared version's
  graphs now join the data graphs of `POST …/validate` (filtered by the
  registry's visibility rule), and the `rdfs:subClassOf*` chain is read across
  every data graph so targets and `sh:class` agree. The spec asks for exactly
  this: §2.1.3.2 and §3.2 both say the ontology axioms have to be in the data
  graph. Nothing in the repository separated a subclass axiom from its type
  triples, which is why this was never observed.
- **`c8280e8` — a regression from `5954c88`, caught by the lib suite.** Making
  every inline-member load error fail the shapes graph turned the loader's own
  recursion bound into a hard failure, so a legitimate recursive `sh:node`
  cycle stopped validating — and through the write gate refused *every* write
  to a dataset using one. The bound is now distinguishable and drops only the
  member being loaded; every other load error still fails the shapes graph.

### Measured, not changed

`6ed571d` adds `OTS_SHACL_REACH_PROBE=1` (off by default): each run counts the
value-node lookups that found nothing per graph and would have found values
over the merge, and logs the totals. It changes no answer — the extra
evaluation runs only where the per-graph union came back empty, and the result
is counted and dropped. It is reported through `tracing` and nowhere else,
because `conforms` is `results.is_empty()` and an informational result would
flip conformance and break the W3C ratchet. `docs/shacl.md` now states which
graphs a run reads and carries the reach table, so the inconsistency is
disclosed rather than latent.

**Why path reach was not flipped**, against the author's initial
recommendation. Three independent objections, each verified:

1. `shacl::infer` materialises **in place**, `WriteTarget::InPlace` is the
   default, and the Studio scheduler fires due pipelines every 60 seconds with
   no human in the loop. A reach change alters which SHACL-AF rules fire, so it
   changes what is **written into user data**, unattended.
2. Today the write gate is a sound predictor of the dataset result for path
   constraints: every gate path passes exactly one data graph, so dataset
   validation is literally the union of N gate-shaped evaluations. Merging
   destroys that property.
3. It cannot be sized. All 121 vendored W3C cases carry exactly one
   `sht:dataGraph` and the suite has no TriG or N-Quads file; all three gated
   SHACL benchmarks pass one data graph; and of the four multi-graph corpora in
   the tree **not one has a shape whose path or target crosses a graph
   boundary**. A differential baseline over today's corpus would return an
   all-"same" table and be read as a safety signal. Hence the probe, which
   answers the question the baseline cannot, from real data.

A per-dataset setting was considered and rejected: no setting value can express
the status quo, because the split runs *through a single property shape* —
`sh:path` per graph and `sh:class` merged, one line apart — so shipping a knob
would mean picking a correct default anyway and then also shipping the position
just judged wrong.

### Corrections to the author's starting position

- "The layered convention depends on reaching across, which is why `sh:class`
  already merges" — **false**. Model, vocabulary and domain-value graphs are
  declared under `[[data_models.graphs]]` and registered to the model registry,
  not the dataset, so merging across *dataset* graphs never reached them. That
  is what `c8280e8` fixes; it did not support the merge argument.
- "The engine has test-pinned semantics a second validator would have to
  reproduce" — **false**, and the truth is worse: only `sh:datatype` lexical
  validity is pinned, by the W3C suite. Per-graph hop confinement is named by
  no test, so there is no written contract to be equivalent to.
- "Role-filter the data graphs so provenance and linksets are not merged in" —
  **withdrawn** on the maintainer's challenge that all graphs are data and
  metadata deserves validating too. The graphs that genuinely should not be
  validated (entailment output, snapshots, reports) are already excluded, and
  the real defect was the opposite one: a missing graph, not a surplus.

### The four follow-ups, closed (2026-09-11)

Raised as out-of-scope follow-ups at the end of the graph-reach work and then
brought into scope by the maintainer. Commits `912fcea`, `0bcbf39`, `6b3ba5d`,
`0d8bc06`. One of the four turned out not to be a defect, and the investigation
found a defect in the probe shipped an hour earlier.

**`sh:closed` is not a sixth reach regime.** It enumerates the focus node's
outgoing quads in a *single hop*, and every matching quad lives in exactly one
graph, so "enumerate per graph and union" and "enumerate over the merge" return
the same set by construction. The same argument retires `sh:targetSubjectsOf`
and `sh:targetObjectsOf`. Nothing to fix; three tests now assert the
equivalence rather than argue it. The real divergence is confined to paths with
an **intermediate node** — a sequence, `zeroOrMorePath` or `oneOrMorePath` —
which is a much narrower statement than "five regimes" and is now what
`docs/shacl.md` says.

**The probe shipped in `6ed571d` measured the wrong thing.** It fired only when
the per-graph evaluation came back *empty*, so it was blind to every lookup
where the merge merely *adds* values to a non-empty result — precisely the case
that changes a `sh:maxCount`, `sh:uniqueLang` or `sh:qualifiedMaxCount` answer.
A zero reading from it was not evidence, which defeats the reason for shipping
it. It now compares the two result sets, reports a denominator, and runs only
for paths that can actually cross a boundary. Caught by the follow-up
investigation, not by a test — there was none to catch it.

**`sh:sparql` and friends now read the run's own source.** A run reads one
instant (the accelerator copy, else one RocksDB readable transaction), but
`sh:sparql` constraints, custom-component validators and SPARQL targets went
through `TripleStore::query` — the live store — so a write landing mid-run was
visible to half a shapes graph and invisible to the other half. `DataView`
gained a `query` method that dispatches on its own source, and the four
data-reading call sites use it. The `sh:pattern` REGEX fallback is deliberately
left on the store: it is an `ASK` over a `FILTER` that reads no data. The
evaluator is built once per run from `TripleStore::query_options` and cloned per
query, because building it scans the store for `sh:SPARQLFunction` definitions —
the per-probe cost the `DataView` refactor existed to remove.
`ogc_geosparql_shacl_roundtrip` is the guard: it calls `geof:distance` from
inside a `sh:sparql` constraint and fails immediately if the options are not
threaded through. The cost is the result cache and the shard routing for these
queries, which is the trade every native probe already makes.

**Studio pipelines read the conformed model, and not their own report graph.**
The model-graph resolution is extracted to
`conformance::model_graphs_for_dataset`, which takes the store, the auth db and
the base URL rather than `AppState` — `resolve` only ever used those three — so
the scheduler can call it, and both validation entry points go through it and
cannot drift. The pipeline gained `resolve_read_graphs` *beside*
`resolve_data_graphs` rather than widening it, because that function is the
pipeline's write surface: it authorises targets, gates inference and decides
where derived graphs are attached. The split is not cosmetic. `shacl::infer`
materialises into a named graph only when handed exactly **one** data graph, so
slipping a model graph into that slice would silently relocate every
single-graph pipeline's in-place inference into the store's default graph, where
the run cannot read it back. `run_validation_scoped` and
`run_validation_capturing` therefore take the read scope and the inference scope
separately; `run_validation` keeps its signature, so the write-gate call sites
are untouched.

**A multi-graph SHACL benchmark exists.** Every other SHACL benchmark passes one
data graph, where the two readings are the same code path, so the gate had zero
coverage of any of this. `shacl/validate_multigraph` adds `within/1`,
`within/4` and `crossing/4` over 1 000 assets, with the subclass axiom alone in
a model graph and the shapes using a sequence path and a `zeroOrMorePath`. Each
id asserts its violation count inside `b.iter`, because the gate only fails on
slowdowns — a scoping regression that dropped focus nodes would otherwise be
reported as an improvement. No CI change: the filter already covers the `shacl`
group and the `shacl_validate_` prefix already carries a tolerance. The first PR
produces WARN rows only, since the merge base has no such benchmark.

Tests added across the four: `shacl_conformance` +3 (the `sh:closed` and
single-hop equivalences, and the IRI-versus-blank-node asymmetry of a composite
path, which is a genuine divergence nobody had noticed), a `DataView`-level
snapshot parity unit test that writes from the test thread with no sleeps, and
`conformance_http` +2 — one driving the whole route to show a superclass target
conforming vacuously before conformance is declared and reporting after it, the
other pinning the Studio read/write split. Nothing in the repository previously
created a Dataset-target pipeline, so that branch of `resolve_data_graphs` had
never been executed by CI at all.

Still open, and now the only ones: property-path reach still disagrees with
`sh:sparql` for multi-hop paths (pinned by test, awaiting probe data from a real
deployment); and `sh:closed`'s expression remains `GraphSel::All` where a
per-graph loop would read more consistently, which is cosmetic and deliberately
left.

### Follow-ups, not done

- Property-path reach still disagrees with `sh:sparql`; pinned by
  `a_path_and_an_equivalent_sparql_constraint_disagree_across_graphs` so any
  change is deliberate. Decide on probe data.
- ~~Studio pipelines do not get the model graphs~~, ~~`sh:closed` is a sixth
  regime~~, ~~`sh:sparql` reads the live store~~, ~~no multi-graph benchmark~~ —
  all four taken into scope by the maintainer and closed; see below.
