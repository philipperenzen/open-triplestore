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

---

## Phase P3 — connectors and standards surface (2026-09-15)

The maintainer scoped P3 to four items — IFC lift depth, IDS export, LDES
retention, a DQV shapes bundle — and struck R2RML SQL sources and Ontop as
an allow-listed `SERVICE` ("for P3 skip"). Decisions taken through the
question tool before any code: the IFC ontology namespace sits under the
deployment's base URL; the retention tables may go into `src/auth/db.rs`;
the DQV bundle is default-on with an opt-out env; the two IDS importer bugs
found while reading the exporter's target are fixed test-first. Instruction
for after P3: halt, explain P4 and P5, build neither.

### 1. IDS export, and two importer bugs it exposed (`cf895d3`, `ab077a6`, `f7ba9e0`)

Reading the importer to write its inverse found two defects, each verified
red against the unfixed code. The occurrence attributes were read from
`<ids:specification>`, but IDS 1.0 puts `xs:occurs` on the applicability
element, so for every conformant 1.0 document the prohibited-specification
branch was dead code and "no entity of this class may exist" was imported as
an ordinary specification that enforced nothing — unnoticed because the
importer's own sample writes the attributes in the 0.9 position. And an
`<ids:entity>` facet inside `<ids:requirements>` was dropped without a
warning: in that position it constrains the focus node, so it is now a
node-level `sh:class` (or `sh:not [ sh:class ]` when prohibited).

The exporter is honest by construction. IDS can express a facet kind, a
cardinality of required/prohibited/optional and one value restriction — most
of SHACL has no IDS form — so the JSON report with its `losses` list is the
default representation and the bare document is `?raw=true`; a shapes graph
from which nothing is expressible is a 422, not an empty schema-valid file.
It reconstructs the importer's own idioms (the `sh:or ( [ sh:not X-applies ]
X-requires )` split, the `props:`/`bot:` vocabulary), so import → export →
import is a fixpoint on the shared subset, which the tests pin. It is not a
general SHACL-to-IDS translator and the docs say so; nor do the tests claim
schema validity (that needs the IDS XSD and a validator, neither offline).
`Shape` gained `sh:name` so the specification name survives. The four routes
— importers, import, exporters, export — were never in the OpenAPI document;
they are now.

### 2. The DQV quality bundle (`6951402`)

DQV is a Working Group Note that constrains nothing: no cardinality axiom,
one shipped dimension, no category, no metric, `dqv:value` without a range
and untied to `dqv:expectedDataType`. A usable deployment needs a profile
and shapes; the bundle is both. Six shapes — three SHACL Core, two
SHACL-SPARQL (core cannot compare a literal's datatype with an IRI read from
another node, nor check that the `owl:inverseOf` pair `computedOn` /
`hasQualityMeasurement` agrees), one warning shape for DQV's "generally
expected" — over a sample with seven planted violations, one per shape, each
built to fire once. The counts were read from a run before being frozen;
the profile is validated against its own shapes; the sample minus the
planted nodes conforms. The profile is not restated in the instances: it is
the model the dataset conforms to, and since the graph-reach work
validation reads it alongside — the store-level test does the same. DQV
itself is not shipped (already vendored and seeded as `dqv`). The metric
IRIs are the ones a validation-run emitter would write; that emitter is a
separate, unapproved item.

### 3. LDES retention (`f333bd6`)

The hard part was not the DELETE. Fragments were paged with
`LIMIT/OFFSET` over the member log while full pages were served with a
year-long `immutable` cache header: one deleted row would have renumbered
every page behind that promise, and a client honouring LDES §3.2 would never
have refetched. So the enabling change comes first: a page is sealed once
full *and* the next member has arrived (so the relation out of it always has
a `tree:value`, recorded at sealing so it survives any later prune), a sealed
node is served from its id range, and the unsealed tail is paged exactly as
before — an install with no sealed nodes serves byte-identical pages until
its next write. Retention then deletes rows in place: a sealed page can only
shrink, never gains a member, never hands one to another page, never changes
its bound; an emptied sealed page is `410 Gone` (Primer §5.1) naming where
the stream continues, and `tree:view` and every relation point past it. The
tail is never 410. Members are kept five minutes past the declared window;
the sweep is debounced on the write path and forced when the policy is set,
and retention lag is the safe direction. The policy is published on the root
node as an IRI described in every page that names it (§4.4: a policy IRI
"without further statements in the current page" means the view keeps
nothing). The client treats 410 as an empty page (§3.3), keeps the
publisher's policy in its report (§3.2) and warns when its bookmark predates
the publisher's window. `xsd:duration` is hand-parsed (a year is 365 days, a
month 30; documented). Not supported: `ldes:versionKey`; the legacy classes
are read, not published.

`tests/ldes_conformance.rs` was written first as a probe of the shipped
surface, one assertion per clause of the LDES spec, its Server Primer §6 and
TREE. The plan expected `Vary: Accept` to be red; it was green (a global
`if_not_present` layer sets it), as were the root-node cardinalities, IRI
members and well-formed relations. Only `<> ldes:immutable true` was red.
There is no LDES/TREE corpus to vendor, so these are spec-derived rules —
the docs and CHANGELOG say so rather than claiming conformance. The two
tables went into `src/auth/db.rs` with the maintainer's leave; the LDES
routes joined the OpenAPI document; `docs/api-reference.md` records the new
410 and the additive fields.

### 4. IFC lift depth (`202845e`, `da98112`)

Characterisation first: three hand-authored STEP fixtures (an IFC 4.3
bridge with units and a map conversion; an IFC4 wall with every quantity
and property kind, a Uniclass chain and a material; an IFC2x3 curtain wall
with the relations) and a test that pins the flat BOT / `props:` contract
as exact triples before anything was added. Writing the unit resolver
against the first fixture found a parser bug: `()` was parsed as a list
holding one unknown byte with the closing paren consumed, so every argument
after an empty list was swallowed into it — an `IfcProject` written with
empty `RepresentationContexts` (most exporters) never had a
`UnitsInContext`, and `IfcRelConnectsPathElements` lost its connection
types. Nothing had read past those positions before. Fixed first, its own
commit, test written against the unfixed parser.

The lift is additive throughout — the BOT edges, the flat `props:` literals,
the ifcOWL class IRIs are the contract of the viewer feed, the IDS importer
and the Studio shapes, and the characterisation test keeps them. On top:

- **Namespace.** `{base_url}/ns/ifc-lift#`, the maintainer's choice. The
  emitter derives it from the graph IRI it writes into
  (`{base_url}/dataset/{id}/building/` → what precedes `/dataset/`; a
  custom graph IRI → its origin), because `src/imports/ifc.rs`, which
  builds `ConvertOptions` field by field, is outside the edit scope — a
  one-line change there would pass it explicitly. The ontology ships as the
  `ifc-lift` seed bundle; seed bundles gained a `{base_url}` placeholder
  (namespace, graph IRIs, shape bindings, payload text) so an ontology can
  be minted under the instance that serves it. A parity test checks every
  lift-namespace term the emitter produces is declared.
- **IFC 4.3.** Only the entities new in 4.3 (`names::IFC4X3_ONLY`) are typed
  under the lift namespace; everything IFC4 already had keeps its IFC4
  ifcOWL IRI, so shapes written for IFC4 — the IDS importer's output — keep
  matching 4.3 models. The plan's "route IFC4X3 to the lift namespace"
  would have broken exactly that. Facilities and their parts are
  `bot:Zone` plus the lift class, so an infrastructure model has a spatial
  spine; zone-in-zone aggregation gets `bot:containsZone` beside the feed
  edge. `IFCALIGNMENT` is no longer typed as an upper-case IRI in a
  namespace where it does not exist.
- **Units.** A QUDT 2.1 table for SI units, the prefixed combinations QUDT
  names (the prefix composes with the whole unit: KILO+GRAM is `KiloGM`,
  MILLI+SQUARE_METRE is `MilliM2`), and conversion-based units by their
  conventional names. Nothing is guessed: an unmapped unit is a label, a
  conversion factor against a mapped base when there is one, and a count in
  `IfcStats`.
- **Quantities and properties.** Every `IfcElementQuantity` quantity — the
  seven simple kinds, complex quantities recursed — becomes a node with
  `qudt:numericValue` and `qudt:hasUnit` (own unit, else the project
  default for its kind) plus the flat `props:{Qto}_{Name}` literal. Property
  values gain a node beside their flat literal with the IFC measure type and
  the unit (own, else the project default for the measure); enumerated,
  list, bounded and complex properties, all dropped before, are lifted; the
  IFC4 set form of `RelatingPropertyDefinition`, which `as_ref_id` could not
  see, is read.
- **Classification → SKOS.** A reference chain becomes concepts with
  notation, label, definition, `skos:broader` and a concept scheme for the
  `IfcClassification`; absolute `Location`s are kept as the IRIs. The
  element gets the object link and the flat `props:ifcClassification`
  literal the IDS importer had targeted for a year without anything
  emitting it — every imported IDS classification requirement failed
  vacuously. Same for `props:ifcMaterial`.
- **NEN 2660-2.** Beside every BOT edge, the relation it means: containment
  is `contains` (location), spatial aggregation `hasPart`, element
  decomposition (aggregation and nesting) `hasTechnicalPart`, path
  connections `connectsObject`, materials `consistsOf`. The P1 relation
  shapes run clean over a lifted model, and a planted cycle is caught.
- **Map conversion.** Read, never applied: origin, height, rotation
  (`atan2(XAxisOrdinate, XAxisAbscissa)`), scale and the projected CRS as
  annotations on a node under the root; when the EPSG code is one the CRS
  registry knows, the node is also a `geo:Geometry` with the origin as a
  CRS-qualified WKT, and a site without its own georeference gets its WGS84
  anchor from the origin reprojected. An unknown code gets no prefix. The
  existing bare `POINT(lon lat)` anchor is CRS84 and stays; the TrueNorth
  argument applies to the rotation verbatim.

`src/docs/mod.rs` is outside the scope, so no `docs/ifc.md`: the lift is
documented as a section of `docs/geo-3d-platform.md`, the bundle in
`docs/plugins.md`. No IFC 4.3 model exists in the repository or the boot
seed; 4.3 behaviour is verified against the fixtures only, and the docs say
so.

## Checkpoint (2026-09-15, HEAD `da98112` + this note)

| Check | Result |
|---|---|
| `cargo test` over the whole main crate (82 binaries) with the full feature list | **2 979 passed, 0 failed, 1 ignored** (the ignored test pre-dates the programme) |
| `cargo test -p opengraph` | 62 passed, 0 failed |
| `cargo fmt --all --check` | clean |
| `cargo clippy --all-targets --features <same> -- -D warnings` | clean |
| `scripts/conformance_table.py --write` | regenerated (README, docs/standards.md) |

P3 commits after `3338233`: `cf895d3` the two IDS importer fixes, `ab077a6`
the IDS exporter, `f7ba9e0` OpenAPI for the import/export routes,
`6951402` the DQV bundle, `f333bd6` LDES retention, `202845e` the STEP
parser fix, `da98112` the IFC lift, then this note. Nothing pushed, nothing
tagged. A second session (`ots-improvements-ee`) was editing this worktree
during the IFC work; its uncommitted change to `tests/federated_acl_http.rs`
is not part of any commit here and was left where it was.

**Follow-ups, not done.** The one-line change in `src/imports/ifc.rs` that
would pass the lift namespace explicitly instead of deriving it (the file is
outside this programme's scope); a `docs/ifc.md` of its own once
`src/docs/mod.rs` may be edited; the validation-run → DQV emitter the bundle
was designed for; `tree:shape` on streams (the bound shapes would have to be
wrapped so tombstones and the version-object triples do not violate them,
and the shape-graph endpoint is not anonymously readable); the endpoint-ACL
default, the `v0.6.0` tag and the 200-on-rolled-back-batch decisions still
owned by the maintainer.

**Handoff — P4 and P5 are explained, not built.** The maintainer asked to
halt before P4. The chat message accompanying this note explains both:
P4 is a replication design note choosing between RocksDB checkpoint
shipping (physical, whole-store, needs the identity DB in lockstep) and an
LDES-following read replica (logical, per dataset, reuses the P3 client and
retention work); P5 is the engine decision — fork the evaluator, a QLever
read backend, or the DuckDB-authoritative mirror — which the P2 notes
already made conditional on telemetry, change capture and the 9M
measurement, in that order. Nothing in P3 changed that order.

## Decisions taken after the P3 checkpoint (2026-09-15)

Answered by the maintainer on reading the checkpoint:

- **`v0.6.0` is cut when this branch merges.** `Cargo.toml` already says
  0.6.0 and the last tag is `v0.5.0`; the auto-tag workflow bumps from the
  last tag on a release PR whose title says `minor`, requires `Cargo.toml`
  to already read the new version, and takes the tag message from the
  `## [0.6.0]` section. So the release-time step is the CHANGELOG fold —
  turning `[Unreleased]` (P0 to P4) into the dated `[0.6.0]` section, which
  today holds the never-tagged July cut — done as the last commit before
  the release PR, not now, so later phases keep adding under `[Unreleased]`.
  Nothing is tagged or pushed by this programme.
- **Endpoint ACL: the default stays open, and it is configurable by rules.**
  A request no rule matches is allowed (role and scope middleware still
  apply); an operator who wants default-closed adds a low-priority `deny`
  for `public` (and, if wanted, `user`) on `/**` and allows above it —
  `docs/security.md` now shows that. No code change; the P0 stop-condition
  item is closed.
- **The `/sparql/batch` 200 on a rolled-back batch** is explained in the
  chat message; the choice (keep 200 for the old `partial` shape, or answer
  409/422) is still the maintainer's.
- **Order of work:** P2 (telemetry, then per-quad change capture with a
  durable cursor, then the 9M measurement), then P4 as a configurable
  replication temperature — cold / warm / hot, all or some datasets — on the
  change log P2 produces.

## Phase P2 — build (2026-09-15)

The P2 notes made the analytical-layer decision conditional on three
measurements, in order: workload telemetry, per-quad change capture with a
durable cursor, the 9M SHACL run. This section logs them as they ship, one
commit each.

### 1. Workload telemetry

**Verified first.** Nothing recorded which exit answered a query (result
cache, count index, mirror shards, full copy, engine); a validation run did
not know its data source, its scope or its duration; a write did not know
how long since the previous one. `docs/notes/analytical-mirror-design.md`
§1.4 names all four as the inputs to its go/no-go thresholds.

**Test first.** `tests/telemetry_http.rs` (2 tests: an end-to-end count of
exits, inherited shape bits, both validation paths and write gaps; the admin
gate — red on the missing route and the missing `metrics` field) and seven
unit tests in `src/store/telemetry.rs` (ring, nearest-rank percentiles, gap
buckets, the path guard, the text bit).

**What shipped.**

- `src/store/telemetry.rs`: two rings (`OTS_TELEMETRY_QUERY_RING` 8192,
  `OTS_TELEMETRY_VALIDATION_RING` 1024), counters, the gap histogram, the
  summary. The query path labels its exit and its elapsed time; two shape
  bits — the parallel classifier's aggregate verdict and a text scan for
  `COUNT(` / `GROUP BY` — are computed once per uncached evaluation and
  stamped on the cache entry, so a hit inherits them. A bit computed on
  misses only would under-count exactly the repeated aggregates a
  dashboard fires. The classifier's verdict now travels to the mirror
  instead of being computed a second time.
- Every SHACL run records source kind (`mirror` / `snapshot` / `live`),
  whether a run index was built, quads and graphs in scope, duration, and
  its path — `dataset`, `gate`, `pipeline` or `engine` — through a
  thread-local guard the caller sets and drops before any await. The same
  numbers ride in the report as an additive `metrics` object and are
  stored on the run row (four nullable columns on `shacl_validation_runs`,
  `ALTER TABLE`-guarded for existing installs), so the history outlives
  the rings.
- Writes record the gap since the previous one into eight buckets, on the
  outermost write guard only.
- `GET /api/admin/telemetry` (admin only; 401 / 403) returns the summary.
  Documented in `docs/performance.md` "Telemetry" and the OpenAPI.

**Cost.** `query/cache_hit/point_lookup` 103.13 ns before, 109.75 ns
after: +6.4 %, one lock and a 16-byte ring write on the hit path, inside
the programme's 20 % bound (123.8 ns). No other benchmark touches the
changed lines.

**Scope note.** `src/auth/db.rs` is touched again (the four run-row
columns), under the maintainer's earlier allowance for the LDES tables and
with the same nullable-column pattern; flagged here in case that allowance
was meant to be narrower.

Also in this commit: `docs/security.md` shows how to make the endpoint
ACL default-closed with rules (the maintainer's decision above).

### 2. Per-quad change capture with a durable cursor

**Verified first.** No write left a record of *what* it changed: the commit
trail (`src/commit_log.rs`) stores counts and graph names per handler call,
the LDES capture (`src/ldes/capture.rs`) diffs entity indexes for streams
only, and oxigraph exposes no sequence number (its RocksDB WAL is not a
public API). The delta-versioning note (§2, §4.3) had settled the shape:
one row per graph per write, an intent row before the mutation, the
sequence number taken inside a critical section around the commit so
`seq` order is commit order, `full` / `counts` / `unknown` extents with
caps, cursors that pin retention, and open-time repair by probing.

**Test first.** `tests/change_capture.rs` (16 tests, red on a missing
`changes()` accessor): one per primitive — ground updates as exact net
rows with post counts; a `DELETE/INSERT WHERE` scanned into a full net
row; `GRAPH ?g` as a store-scoped unknown; a scan over the cap as a
graph-scoped unknown, not a guess; a batch as one transaction with
consecutive sequence numbers that aborts whole; Graph Store `PUT` as the
net diff, `POST` as only the new quads, `DELETE` as every removed quad;
bulk primitives one row per graph with duplicates probed out; `store_quad`
probing before it counts; a streamed default-graph load as an honest
unknown; dense sequence numbers in commit order; the write context landing
on the rows; the off switch changing nothing else; a persistent reopen
that resolves a pending row and reconciles a count the log never saw; and
the admin API (`GET /api/admin/changes`, `/status`, `PUT`/`DELETE`
`/cursors/:name`, 401 anonymous). Thirteen unit tests in
`src/store/changes.rs` (rows and sequencing, the store-scoped row, abort,
the off switch, nested primitives, the context guard, the payload cap,
gzip round trip, pending resolution, retention below the lowest cursor,
the reconcile row, the commit-trail exclusion, the net diff).

**What shipped.**

- `src/store/changes.rs`: the `ChangeLog` — SQLite beside the store
  (`{data_dir}/changes/changes.db`, WAL, in memory for an in-memory
  store), `begin` (intent rows, one per target, or one store-scoped row),
  `commit_with` (the data commit inside the sequence section, then
  finalisation with payload, counts, post count and `seq`), `abort`,
  `rows_after` / `rows_after_in`, cursors with a TTL, `sweep` (age below
  the lowest live cursor), `resolve_pending` (probe added/removed quads;
  two sequenced rows on one graph, or any store-scoped row, make the
  committed ones unknown too — their order is unrecoverable),
  `reconcile_counts` (the last recorded post count per graph against the
  live index; a disagreement, or a graph the log never saw, gets an
  `unknown` row with `origin = reconcile`). Payloads are N-Quads, gzipped
  above 64 KiB; caps `OTS_CHANGE_CAPTURE_MAX_SCAN` / `MAX_PAYLOAD` (250k),
  `OTS_CHANGE_RETENTION_DAYS` (90), `OTS_CURSOR_TTL_DAYS` (30),
  `OTS_CHANGE_CAPTURE=off`.
- `src/store/engine.rs`: every mutation primitive records. Ground updates
  reuse the existing simulation (`ground_update_delta`: probes, no scan) so
  their rows are exact; other updates with static targets take a
  before-image when the targets' summed counts fit the scan cap, run on a
  transaction (`PreparedSparqlUpdate::on_transaction`), read the
  after-image through it and diff; `batch_update` likewise over the joined
  text; `graph_store_put` diffs the pre-image against the distinct new
  quads; `graph_store_delete` records the removed quads (`counts` on the
  chunked path); loads record fresh quads per graph, a streamed
  default-graph load a store-scoped unknown; `bulk_insert_quads` probes per
  graph and marks version-snapshot graphs unknown rather than copy them;
  `store_quad` probes first and now keeps the count index exact (it used
  to lag this path). Nested primitives are suppressed by a thread-local
  depth/claim on the write guard. The commit trail's own graph is never
  recorded (its row per commit would double every handler write).
- Open-time repair in `TripleStore::open`: `resolve_pending`, then
  `reconcile_counts` against the rebuilt index. `src/store/recovery.rs`: a
  quarantined store takes `changes/` with it, so the rebuilt store mints a
  new epoch.
- Handlers set a `WriteContext` (actor IRI as the commit trail mints it,
  commit kind) inside the blocking closure that runs the primitive — never
  across an await (the `IdentityGuard` precedent, delta note §2.3).
  Registry and commit writes on async threads stay system writes.
- `GET /api/admin/changes?after=&limit=&graph=` → `{epoch, rows,
  next_after}`; `GET /api/admin/changes/status`; `PUT` / `DELETE`
  `/api/admin/changes/cursors/:name`. Admin-only. OpenAPI,
  `docs/versioning.md` "Change log", `docs/api-reference.md`, CHANGELOG.

**Cost.** Update benchmarks — before (`d29134b`, a detached worktree of
the same tree, its own bench binary), the default (capture off) and capture
on — same machine, taken back to back in two pairs; the second pair is the
table, the first agreed within the baseline's own ±7 % pair-to-pair spread:

| benchmark (in-memory store) | before | capture off (default) | capture on |
|---|---|---|---|
| `insert/sparql_update/single_triple` | 74.8 µs | 77.1 µs (+3 %) | 84.2 µs (+13 %) |
| `insert/sparql_update_batch/10_triples` | 155.3 µs | 155.8 µs (+0 %) | 175.1 µs (+13 %) |
| `update/ground_delta/insert_data/1` | 14.6 µs | 14.8 µs (+1 %) | 15.7 µs (+7 %) |
| `update/ground_delta/insert_data/100` | 256.3 µs | 255.7 µs (−0 %) | 266.1 µs (+4 %) |
| `update/insert_where/100` | 325.8 µs | 334.9 µs (+3 %) | 823.9 µs (+153 %) |
| `update/insert_where/1000` | 2.79 ms | 2.81 ms (+1 %) | 7.18 ms (+157 %) |
| `update/insert_where/10000` | 40.15 ms | 39.76 ms (−1 %) | 106.38 ms (+165 %) |
| `update/delete_where/100` | 219.1 µs | 227.6 µs (+4 %) | 638.9 µs (+192 %) |
| `update/delete_where/1000` | 1.64 ms | 1.62 ms (−1 %) | 5.66 ms (+246 %) |
| `update/delete_where/10000` | 19.45 ms | 20.17 ms (+4 %) | 77.77 ms (+300 %) |

Bound: 20 % on any `docs/performance.md` benchmark. **The shipped default
holds it** (worst +4 %, inside noise). **Capture on does not**, and cannot:
a `WHERE` update's row needs the target graph read twice and the delta
serialised, and a ground update's row needs a handful of SQLite statements
on a 15 µs path. Three things were done about it before settling on opt-in:
the first measurement (capture on, then the only mode) was ×2–4 across the
board and +230 % on `insert_data/1`; the statement cache, an in-memory
sequence counter (durable at sweeps and open-time repair), gzip at the fast
level and the ground path reusing its own simulation instead of a second
parse brought the fixed cost from ~33 µs to ~1–9 µs per write; deriving the
log's targets from the exact delta removed a parse the off path was still
paying (+28 % on `insert_data/100` with capture off, now −0 %). What is
left is the scan and the payload, which are the feature. So: **off by
default, `OTS_CHANGE_CAPTURE=on` to enable**, and the P4 replication mode
enables it itself. Whether it should become the default is the
maintainer's call, put to them in the checkpoint; the bound is not
overridden here.

A unit-level measurement to keep in mind for P4: the fixed per-write cost
with capture on is ~1–9 µs on the in-memory store (`insert_data/1` +7 %,
`single_triple` +13 %); on RocksDB it sits beside a per-commit `fsync`.

**Suite.** 3,038 passed / 0 failed / 1 ignored (pre-existing) over 84
binaries, run with `OTS_CHANGE_CAPTURE=on` so every write path in the
suite exercised the log; the default path is the old code plus a branch,
and `tests/change_capture.rs` covers the off switch. The conformance
table (`scripts/conformance_table.py --write`, README and
`docs/standards.md`) is regenerated here for both new suites — the
telemetry commit should have carried its own regeneration and did not.

**Not done, on purpose.** Consumers (the replication follower, the
history endpoint's `seq_from`/`seq_to`) are the P4 work; the log is the
producer side only. The `late` origin (a graph a primitive discovered while
running) exists in the finaliser but no primitive uses it yet — every
primitive knows its targets or says `unknown`. `OTS_CHANGE_CAPTURE=off` is
the escape hatch if a workload finds a cost this measurement did not.

### 3. The 9M SHACL measurement

**Verified first.** The only 9M SHACL figure in the tree was 118 s,
in-process, pre-engine-rebuild, accelerator off (`docs/performance.md`, the
OTL-scale table), and the analytical-mirror note (§1.5) made the SHACL→SQL
decision conditional on re-measuring it on the deployment's real
configuration: A (mirror on) ≤ 15 s and B (the shipped 4g container) ≤ 60 s
keep the translator deferred. Neither `scripts/scale_compare_http.py` (no
SHACL phase) nor `examples/scale_otl.rs` (no settle, no source label) could
produce the row, and both live outside the programme's scope.

**Harness.** `tests/scale_shacl_9m.rs`, an `#[ignore]`d integration test —
the one place in scope where a multi-minute, nine-million-quad run can
live without touching `scripts/` or `examples/`. It ports the OTL
generator (40 object types, six typed properties, a part-of link, a
location, every 10 000th asset with a bad code), loads 1M assets in
50 000-asset Turtle chunks into a persistent store, waits for the mirror
to publish (the rebuild is triggered by the first query after writes go
quiet, so the test issues one and polls `mirror_full_copy()`), validates
twice, writes 500 quads and validates again. Each phase prints one JSON
line including the report's `metrics` — the telemetry item's source kind,
run index flag and engine duration — so a run says which path it took.
`OTS_SCALE_ASSETS` and `OTS_SCALE_SETTLE_SECS` scale it down for a smoke
run. **Why `#[ignore]`:** ~8 minutes and ~9M quads per configuration; it is
a measurement, not a regression test, and runs only when asked
(`-- --ignored --nocapture`). The conformance table is regenerated for the
one new test.

**Result** (reference system, AMD Ryzen 9 7900X3D, Docker, release build,
2026-09-16):

| 1M assets, 9M quads, RocksDB, in-process (release, Docker) | A — mirror on (55 GB budget) | B — the shipped 4g container |
|---|--:|--:|
| Load, 1M assets in 50k-asset Turtle chunks | 96.8 s (93k quads/s) | 93.6 s (96k quads/s) |
| Mirror published after the load | 131 s (one build) | never (over the cap) |
| SHACL, every asset, 6 property shapes — first run | **6.29 s** (source `mirror`, no run index) | **13.5 s** (source `snapshot`, run index) |
| — second run | 6.27 s | 13.8 s |
| — straight after a 500-quad `INSERT DATA` (mirror dirty) | 18.5 s (`snapshot`, run index built to the 8M cap) | 12.4 s (`snapshot`, run index at the 1.8M cap) |
| Violations found | 100 of 1 000 000 assets, both | 100, both |

**Decision, by the note's own thresholds.** A = 6.3 s (≤ 15; the linear
prediction was ≈ 7 s), B = 13.5 s (≤ 60): SHACL→SQL stays deferred, T4 does
not open, and the next lever is changed-node scoping of the gate and
pipeline runs (mirror note §12) — at 9M a whole-dataset run costs 6–14 s
whichever path it takes, and a pipeline that revalidates after every edit
pays that per edit. Two observations for later: the after-write run on
the large budget (18.5 s) was slower than the capped container's steady
state (13.5 s) because the run index was built to its 8M ceiling — the
cap's upper range is not free at this size (`src/shacl/view.rs`); and the
POST/PUT gate benchmark the note also asked for needs the HTTP harness and
stays open. Recorded in `docs/performance.md` (the table row, and a new
"The 9M SHACL measurement" section) and in the note's §1.5.

## Checkpoint (2026-09-16, HEAD `e94d889` + this note)

**Commits since the P3 checkpoint:** `d29134b` telemetry, `f97d894` change
capture, `e94d889` the 9M measurement — and the peer session's `5f8a6f5`
(federated ACL listener fix), on the same branch. One item per commit,
signed off, no branding.

**Suite.** 3,038 passed / 0 failed / 1 ignored over 84 binaries at
`f97d894`, run with `OTS_CHANGE_CAPTURE=on`. `e94d889` adds one binary with
one ignored test (the 9M harness) and no other code; the suite was not
re-run for it. Clippy (`--all-targets -D warnings`) is clean at every
commit. Conformance table regenerated twice (`f97d894`, `e94d889`).

**Performance bound (20 % on any `docs/performance.md` benchmark).**
Telemetry: `query/cache_hit` +6.4 %. Change capture: the shipped default
(off) within run-to-run noise on all ten update benchmarks (worst +4 %);
capture on exceeds the bound by construction on `WHERE` updates (×2.5–4)
and is therefore opt-in — the bound was not overridden. Paired
measurements used a detached worktree of `d29134b` at
`C:/Users/phili/Code/ots-bench-base` with its own target volume
(`ots-bench-base-tgt`); it is kept for P4's leader-side measurements and
removed at the P4 checkpoint.

**What P2 settled.** The analytical-layer decision tree of
`docs/notes/analytical-mirror-design.md` now has its inputs: telemetry
collects the analytical share, exits and validation sources; the change
log exists as the feed the note required before any mirror or SQL
substrate; and the 9M SHACL row is 6.3 s (mirror) / 13.5 s (4g container),
inside both thresholds, so SHACL→SQL stays deferred and the next lever is
changed-node scoping of gate and pipeline runs. Nothing in P2 added a
dependency or changed a status code, a JSON field or the on-disk store
format.

**Still the maintainer's.**

1. **Change capture default.** Off, with `OTS_CHANGE_CAPTURE=on`; P4's
   leader role switches it on itself. Making it the default costs the
   on-column of the table in §2 above on every write.
2. **`/sparql/batch` 200 on a rolled-back batch.** Explained in the chat:
   the endpoint reports per-statement outcomes in the body (`status:
   rolled_back` on every statement, `error` on the one that failed) and
   answers 200 because the old `partial` shape did, and clients read the
   body. A 409 or 422 for "nothing was applied" is the cleaner protocol and
   a status-code change, reserved for the maintainer.
3. **Endpoint ACL default open** — closed by the maintainer's answer,
   `docs/security.md` shows the default-closed rule set.
4. **`v0.6.0` at merge** — the CHANGELOG fold into a dated `[0.6.0]`
   section is the last commit before the release PR; nothing is tagged or
   pushed by this programme.

**Next: P4**, on the maintainer's design — replication as configurable
temperatures (cold / warm / hot) over all or some graphs, logical log
shipping on the change log the P2 work produced, the follower tailing
`/api/admin/changes` with a cursor, bulk loads as "graph replaced at seq
N" with the follower fetching the graph, identity DB replicated separately,
epoch fencing on failover, `/api/replication/status` beside `/livez`. The
consensus (Raft) variant needs a dependency and is asked about before
anything is written for it.

## Phase P4 — replication as configurable temperatures on the change log (2026-09-16)

The maintainer's design, taken as the specification: logical log shipping on
the per-quad change log P2 produced; a follower tails the rows with a cursor;
bulk loads travel as "graph X replaced at seq N" with the follower fetching
the graph; the identity database is replicated separately; failover fences
with epochs; `/api/replication/status` beside `/livez`; each node rebuilds
its own indexes; the temperature is decided after the log exists —
configurable as cold / warm / hot over all or some of the data.

### 1. The follower, the leader's manifest, temperatures and scopes

**Verified first.** No replication of any kind existed: `docs/operations.md`
and `docs/administration.md` describe backups and a single node; the LDES
client (P3) follows a stream into one graph and is the closest thing to a
follower — per dataset, pull-based, no position on the publisher. Background
loops start in `src/server/mod.rs` (out of scope), so the follower runs its
own thread from the store constructors, and only when the environment
configures a follower — tests drive one catch-up at a time.

**Test first.** `tests/replication.rs` (9 tests, red on a missing
`replication` module): bootstrap from the manifest then tail (deltas, the
count index following, the cursor set on the leader, `lag_rows` 0,
`healthy`); `counts`/`unknown` rows fetching the graph whole; a store-scoped
row resynchronising every graph in scope and dropping a graph the leader
deleted; a graph scope skipping out-of-scope rows while the cursor still
advances; a dataset scope resolved through the manifest; an epoch change
forcing a full resynchronisation; a follower read-only except for what it
replicates; the temperatures and the environment's spellings; and the HTTP
surface — the manifest (401 anonymous, the dataset's graphs listed), the
public status on a leader and on a follower, `503` on a follower's SPARQL
Update and Graph Store `PUT`, reads unaffected. Four unit tests in
`src/store/replication.rs` (scope selection through the manifest, the
bookmark surviving a reopen, health before and after a catch-up, the apply
guard's nesting). All in-process: an `InProcessLeader` implements the same
`LeaderSource` trait as the HTTP client, so no test opens a socket.

**What shipped.**

- `src/store/replication.rs`: `Role` (`none` / `leader` / `follower`),
  `Mode` (`cold` hourly, `warm` every minute — `medium` accepted —, `hot`
  every poll of 500 ms; an interval override), `Scope` (`all`, graph IRIs
  with `default`, or the leader's dataset ids resolved through the manifest
  at every catch-up), `ReplicationConfig::from_env` / `parse`, the
  `Replication` state (bookmark `{epoch, applied_seq}` persisted atomically
  as `<data-dir>/replication.json`, counters, last sync/error, `Status`),
  the `LeaderSource` trait with `HttpLeader` (reqwest on its own runtime,
  bearer token, `/api/replication/manifest`, `/api/admin/changes`, Graph
  Store `GET` as N-Triples, `PUT /api/admin/changes/cursors/<node>`) and
  the `InProcessLeader`, `TripleStore::replicate_once` (adopt the epoch or
  resynchronise; pages of 500 rows in order; a `full` row applied as a
  delta with the `post_count` check healing a divergence by fetching the
  graph; `counts`/`unknown` rows fetching the graph; store-scoped rows
  resynchronising from a fresh manifest; out-of-scope rows skipped; the
  bookmark and the leader-side cursor set after every page; at most 200
  pages per call), and the follower thread.
- `src/store/engine.rs`: `begin_write` now returns `Result` and refuses on a
  follower unless the thread is inside an `ApplyGuard` — so every mutation
  primitive, and every route behind it, is read-only on a follower with one
  check; `apply_delta_replicated` (insert the added, remove the removed, one
  transaction, the count index adjusted by what actually changed);
  `with_replication` (builder); `replication()`. `StoreError::ReadOnly` →
  `503` (`src/server/error.rs`; the SPARQL Update route, which mapped every
  store error to `400`, passes it through). `OTS_REPLICATION_ROLE=leader`
  switches change capture on (`changes.rs`). A quarantine takes
  `replication.json` with the store (`recovery.rs`).
- `GET /api/replication/status` (public) and `GET /api/replication/manifest`
  (admin) in `management_routes()`; OpenAPI; `docs/operations.md` (a new "Replication" chapter),
  `docs/administration.md` (nine variables), `docs/api-reference.md`,
  CHANGELOG. `tests/common` gained `test_state_with_store` /
  `admin_state_with_store` for tests that build their own store.

**Design points worth recording.**

- *Temperature is cadence only.* Cold, warm and hot apply the same rows the
  same way; only the interval differs. This keeps one code path to test and
  makes "some datasets, hot" and "everything, cold" the same follower.
- *Fencing by epoch, not by history.* A follower applies rows only from the
  epoch it adopted; any other epoch resynchronises whole. Promotion is a
  restart with the leader role; nothing is un-applied; a node that was
  behind can be promoted and its followers take its state.
- *A whole graph, not a replay, for what the log cannot carry.* `counts`,
  `unknown`, store-scoped rows and count disagreements all end in one
  Graph Store `GET` — the same primitive, the same test.
- *Read-only at the engine, not at the router.* The router lives in
  `src/server/mod.rs`; one check in `begin_write` covers SPARQL Update, the
  Graph Store, imports, registry data writes and everything added later.
  Cost on the write path: one atomic load and a thread-local read.

**Not built, stated plainly** (each is a question for the maintainer, in
the checkpoint): synchronous hot replication (the leader waiting on
follower cursors before acknowledging a write — the ack exists, the wait
and its policy do not); consensus (Raft — a dependency); identity database
replication (a follower authenticates with the `auth.db` it has; seed it
from the leader's backup); asset shipping (share the S3 bucket); a
follower's own change log as a source for further followers.

**Cost.** No benchmark exercises a follower; the leader's cost is the change
log's (§2 of P2, measured). The one addition to every write path on every
node is the read-only check in `begin_write`; the update benchmarks were
not re-run for a branch on an atomic — the P4 checkpoint's suite run and
the next paired measurement (the sync variant, if approved) will carry it.

## Checkpoint (2026-09-16, HEAD `b650972` + this note)

**Commits since the P2 checkpoint:** `b650972` replication (P4 item 1). One
item, signed off, no branding. The baseline worktree
(`C:/Users/phili/Code/ots-bench-base`, detached at `d29134b`) and its target
volume `ots-bench-base-tgt` are removed with this note; the next paired
measurement recreates them from whatever HEAD is the baseline then.

**Suite.** 3,053 passed / 2 failed / 2 ignored over 86 binaries at the pre-commit tree — the two failures were one test, the doc-parity check `docs::builtin_parity::every_top_level_doc_is_registered` in the lib and bin test binaries, tripped by a new `docs/replication.md` that `src/docs/mod.rs` (out of scope) would have had to register; the chapter moved into `docs/operations.md` and the docs tests were re-run green; the other 84 binaries were not re-run for a doc move and comment edits. The two ignored are the pre-existing one and the 9M harness (default environment: capture off, no replication
role). Clippy (`--all-targets -D warnings`) clean. Conformance table
regenerated for the new binary.

**Where P4 stands against the maintainer's design.**

| Design item | State |
|---|---|
| Change log = per-quad capture, deterministic deltas, own counter, atomic-with-data where possible | shipped in P2 (`f97d894`); intent rows, seq in the commit section, epochs |
| Follower tails `after=<seq>` | shipped: paged polling (500 rows), every poll in `hot`; long-poll/SSE not built — a hot follower's floor is the poll period (500 ms default, 50 ms minimum) |
| Bulk loads as "graph X replaced at seq N", follower fetches the graph | shipped: `counts` / `unknown` rows, store-scoped rows, count disagreements all end in one Graph Store `GET` |
| Assets beside the store | not built: share the S3 bucket; local-filesystem assets stay on the node that received them |
| Identity DB replicated separately | not built: a follower authenticates with the `auth.db` it has; seed from the leader's backup. Options for the maintainer: an application-level log of identity changes (the same shape as the change log, in `src/auth/db.rs`), or WAL shipping (Litestream-style, outside the binary) |
| Failover / fencing with epochs | shipped: a follower applies only its adopted epoch, resynchronises on any other; promotion = restart with the leader role |
| `/api/replication/status` beside `/livez` | shipped, public; `healthy` = last successful catch-up within three intervals |
| Each node rebuilds its own mirror / text index | as designed: the follower's writes go through the normal primitives, so the mirror, text and spatial indexes follow |
| Temperatures: all / some, cold / medium / hot | shipped: scope `all` / graph list / dataset list; `cold` hourly, `warm` (`medium`) every minute, `hot` every poll; interval override |
| Hot, asynchronous | shipped (this is what `hot` is) |
| Hot, synchronous | **not built — needs a decision**: the leader would wait, in its write path, until the cursors of the configured followers reach the write's seq (the ack exists: the cursor PUT). Adds a follower round-trip to every write's latency and needs a policy for a follower that is down (block, degrade to async, fail the write). Measurable against the update benchmarks once decided |
| Hot, consensus (Raft) | **not built — needs a dependency**: the programme adds none without the maintainer's decision |

**Still the maintainer's** (the P2 list, plus P4's):

1. Change capture default (off; `OTS_REPLICATION_ROLE=leader` turns it on).
2. `/sparql/batch` 200 on a rolled-back batch (keep, or 409/422).
3. `v0.6.0` at merge: the CHANGELOG fold is the last commit before the release PR.
4. Synchronous hot replication: build it (with which down-follower policy), or not.
5. Consensus: which library, if any (a dependency).
6. Identity database: application-level log in `auth.db`, or WAL shipping outside the binary.
7. The follower's boot seed: a follower logs the seed's read-only refusals at start; silencing them needs the seed callers (`src/saved_queries/seed.rs`, `src/shacl_studio/seed*.rs` — outside the programme's scope) to skip on a follower.

## Decisions taken after the P4 checkpoint (2026-09-16)

Answered by the maintainer on reading the checkpoint and the recommendation
for synchronous replication:

1. **Synchronous hot replication: build the recommendation** — named sync
   followers, a required count, a bounded wait, visible degradation with
   automatic recovery, the long-poll on the change endpoint — "with short
   explanations for the configured settings and the modes".
2. **Consensus: make a choice; dependencies may be added.**
3. **Identity database: make the best decision; WAL shipping looks good.**
4. **Change capture on by default**, explained in the configuration and
   the docs.
5. **`/sparql/batch`: 422 on a rolled-back batch**, with a message saying
   what went wrong.

Shipped in this order: 5, 4, 1, 3, 2 — the two small ones first, then the
three that carry design.

### 5. `/sparql/batch` answers 422 on a rolled-back batch

**Verified first.** The route answered 200 with `status: rolled_back` in the
body (kept in P0 for the earlier per-statement shape); `tests/sparql_batch_http.rs`
pinned the 200 and the body. **Test first:** the same test now expects 422
and a top-level `error` that starts with `statement 1 failed:`. **Shipped:**
the branch returns `UNPROCESSABLE_ENTITY` and adds `error` — the failing
statement's index and its message, "nothing was applied" — the rest of the
body unchanged; OpenAPI, `docs/api-reference.md` (table row and the note on
the change), CHANGELOG under Changed. 422 rather than 409: the request was
understood and well-formed, the state did not conflict with it — a
statement could not be executed. Parse and authorisation failures stay 400
and 403; a fully applied batch stays 200.

### 4. Change capture on by default

**Decision.** The maintainer chose the default the measurement in P2 §2
left open: on. **Shipped:** `ChangeLog::open` records unless
`OTS_CHANGE_CAPTURE=off`; a replication leader records regardless (its
followers read the log); a replication follower does not unless set to
`on` (its log is not a source — it would hold only the graphs it fetched
whole). The unit test of the off switch now pins "on is the default";
`tests/change_capture.rs` was already explicit through the builder, and the
full suite had already run with capture on at `f97d894` (3,038 / 0 / 1).
**Docs:** the configuration row explains what the log is for and what it
costs; `docs/versioning.md`, `docs/performance.md` (the update benchmarks'
shipped default now carries the "capture on" column, and the gate compares
like with like), `docs/api-reference.md`, `docs/operations.md`, CHANGELOG.
**Cost, restated:** ground updates +4–13 %, `WHERE` updates ×2.5–4 on the
in-memory benchmarks (P2 §2); on RocksDB the fixed part sits beside a
per-commit fsync. A write-heavy store with no consumer turns it off.

### 1. Synchronous replication, and the long-poll that makes it usable

**Verified first.** The follower's cursor (`PUT /api/admin/changes/cursors/<node>`,
set after a page is applied) was already the acknowledgement a synchronous
leader needs; nothing waited for it, and the follower's floor was its poll
period (500 ms) because the change endpoint answered at once.

**Test first** (`tests/replication.rs` +3): a leader with one named
synchronous follower and a 300 ms timeout — a write with nobody applying
takes the timeout and leaves the leader degraded; the next write, still
without a follower, does not wait; a follower named `f1` catching up every
20 ms in a thread makes the next write return well inside the timeout,
clears the flag, lists `f1` as acknowledged and sets `last_confirmed_seq`.
The settings' defaults and bounds (`all`, never more than named, the 50 ms
floor, the 2 s default). The HTTP long-poll: `wait_ms=200` with no write
answers empty after the wait; a write landing 100 ms into a 5 s wait is
answered at once; a Graph Store `PUT` on a degraded leader carries
`X-Replication-Ack: degraded` and the status shows `sync.degraded_since`.

**What shipped.**

- `src/store/changes.rs`: `rows_notified()` (a `tokio::sync::Notify` woken
  after every finalised write, enabled before the look so nothing is
  missed), `cursors_at`, and `wait_for_cursors` — a `Condvar` woken by every
  cursor move, so an acknowledgement is seen when it arrives, not on a
  poll.
- `src/store/replication.rs`: `OTS_REPLICATION_SYNC_FOLLOWERS`, `_REQUIRED`
  (`all` allowed, clamped to the names), `_TIMEOUT_MS` (50–60000, default
  2000); `after_write` — called by the outermost write guard on a leader,
  it waits for the required cursors to cover the write's newest sequence
  number; on timeout it marks the leader degraded once and logs once;
  **while degraded a write waits only if a follower has caught up to the
  position before it** (the sign it is back), so a dead follower costs one
  lookup per write and a returning one gets the full wait and clears the
  flag. `SyncStatus` in the status; `ack_state()` for the header. The
  hot follower long-polls (`wait_ms` = its poll period) and asks again
  immediately after a successful round instead of sleeping.
- `src/store/engine.rs`: the write guard records the log's position at
  entry and hands it to `after_write` when the outermost guard drops.
- `src/server/routes.rs`: `wait_ms` on `GET /api/admin/changes`;
  `X-Replication-Ack` on SPARQL Update, `/sparql/batch` (the applied case),
  Graph Store `PUT`/`POST`/`DELETE`. Other write routes reach the store
  through the same guard and wait the same way; they carry no header, which
  the status covers.
- Docs: `docs/operations.md` (the modes table, a "Synchronous replication"
  section with the settings, the guarantee in one sentence, and why not
  block or fail), `docs/administration.md`, `docs/api-reference.md`, OpenAPI,
  CHANGELOG.

**Design points.** The ack is *applied*, not received — the cursor is set
after the page is applied, so a synchronous success means a reader of the
follower sees the write (PostgreSQL's `remote_apply`). Failing the write
after the commit would be a lie and blocking would let a replica take the
leader down; the degrade-visibly policy is the recommendation the
maintainer accepted, and it is the only policy — no knob. The wait lives
in the write guard's drop, so every primitive and every route behind it
waits the same way with one check; when no synchronous follower is named
the cost is one boolean.

**Cost.** Asynchronous nodes: one boolean per write in the guard's drop.
Synchronous leaders: a round trip per write with a hot follower on a
healthy link; the timeout when it is not, once, then a lookup per write
until it returns.

### 3. The identity database, shipped whole

**Decision.** The maintainer chose "WAL" over an application-level log of
identity events: ship the database's own changes, not a model of them.
The in-binary form of that is not a stream of WAL frames — a follower
reads its `auth.db` through open connections, and SQLite does not support
applying foreign frames underneath them — but the supported equivalent:
a consistent snapshot from the online backup API, applied in place through
the same API's destination side. The database is kilobytes to megabytes, so
a whole snapshot per change costs nothing that matters, and the transport
can become frame-level later without changing the model.

**Verified first.** `src/backup/mod.rs` already used
`rusqlite::backup::Backup` for the scheduled backup (source side only);
nothing read `PRAGMA data_version`; the follower authenticated with
whatever `auth.db` it had.

**Test first** (`tests/replication.rs` +2): a file-based leader database
with one user, an in-memory follower database — the first round applies
the snapshot and the user appears; an unchanged leader makes the next
round a no-op (`identity_version` unchanged, nothing fetched); a second
user on the leader moves the version and the next round applies it. Over
HTTP: `GET /api/replication/identity` is 401 anonymous and answers a
SQLite file to an admin; the manifest carries `identity_version`.

**What shipped.** `AuthDb::snapshot_bytes` (backup API into a temp file,
read, removed), `AuthDb::apply_snapshot` (temp file, backup API into a
pooled connection — every connection sees the new pages on its next
statement; the accessible-graphs cache invalidated), `AuthDb::data_version`
(a watch connection kept for `PRAGMA data_version`; `None` in memory, where
a single connection sees no "other" commits). The manifest gains
`identity_version`; `LeaderSource::identity_snapshot`; `replicate_identity_once`
(fetch and apply when the version moved or is unknown; process-wide
`identity` state in the status); the identity follower thread, started by
`AuthDb::open` when the environment configures a follower, at
`OTS_REPLICATION_IDENTITY_INTERVAL_SECS` (the temperature's interval, at
least 5 s). `GET /api/replication/identity` (admin). Docs: an "Identity
database" section in `docs/operations.md` with the three things to know
(`jwt_secret`, local writes overwritten, the audit log comes across),
`docs/administration.md`, `docs/api-reference.md`, OpenAPI, CHANGELOG.

**Scope note.** `src/auth/db.rs` is touched again, under the maintainer's
allowance: three methods, two fields and one hook in `open`.

### 2. Consensus: Raft elects the leader, the change log stays the data path

**Decision, and the dependency.** The maintainer asked for a choice with
dependencies allowed. Chosen: `openraft` 0.9.25 (MIT / Apache-2.0), with
its `serde` and `storage-v2` features — an async Raft with pluggable
storage and network traits, a majority quorum, heartbeat and election
timeouts, and a maintained release line; `raft-rs` (tikv) is a sync core
that leaves timers, transport and the state loop to the caller, and the
newer Paxos crates have less mileage. One dependency; twenty transitive
crates, all permissively licensed.

**What Raft decides, and what it does not.** Only who leads. The
replicated state machine holds no data — membership and a heartbeat — and
the log and vote live in memory. The data path is unchanged: the elected
member records in its change log, the others follow it hot, and a write
is acknowledged synchronously by a majority (the leader plus half of the
rest) through the mechanism of item 1, with the follower list and the
count derived from the cluster. A member that loses leadership is
read-only from the next request (`effective_role` is read through the
process-wide view on every write); a member that wins it records and
serves; followers resynchronise once on the new epoch. This keeps the
dependency at its strength and the data where P4 put it; routing every
write through the Raft log would have meant a second on-disk format and a
rewrite of every primitive.

**Test first** (`tests/consensus.rs`, 3 tests): three in-process members
through an in-process router elect exactly one leader and all name it;
the leader goes off the network and stops; the other two elect another in
a higher term and the survivor follows it. A cluster member's derived
settings: `node-<id>`, the other members as synchronous followers, a
majority required, hot, read-only and `follower` until an election says
otherwise, `cluster` in the status with `starting`. The Raft routes answer
`404` off a cluster. Two unit tests in `src/store/consensus.rs` (the
cluster settings and the derived quorum; the empty view).

**What shipped.** `src/store/consensus.rs`: the type configuration, an
in-memory `RaftLogStorage` and `RaftStateMachine` (snapshots of the
membership), a `RaftNetwork` over the members' HTTP ports (JSON,
`X-Cluster-Secret`) or an in-process router for tests, `Member::start`
(every member proposes the static membership; an initialised log declines
and carries on), the process-wide `View` kept current from the metrics
watch, `ClusterConfig` (`OTS_REPLICATION_CLUSTER`, `_CLUSTER_ID`,
`_CLUSTER_SECRET`, `_ELECTION_MS`, `_HEARTBEAT_MS`), and the member thread
on its own runtime. `src/store/replication.rs`: `Role::Cluster`,
`apply_cluster` (the derived name, followers and quorum), `effective_role`
and a dynamic `leader_url` used by `read_only`, the synchronous wait, the
follower and identity threads (which now follow whoever leads and skip
while leading) and the status (`role` as played, `configured_role`,
`cluster`). `changes.rs`: a member records. The Raft routes in
`routes.rs`; OpenAPI; `docs/operations.md` (the modes table's consensus
row and a "Consensus" section), `docs/administration.md`,
`docs/api-reference.md`, CHANGELOG; `Cargo.toml` and `Cargo.lock`.

**Stated plainly.** The in-memory log and vote: a restarted member rejoins
with term 0; the election timeout (1.5–3 s) outlasts a restart, and
because the state machine holds no data and the data path fences by
epoch, a double vote could at worst elect a second leader for one term,
which resynchronises rather than diverges. A persistent vote is the first
thing to add if a status ever shows it. The Raft routes ride the server's
port: the secret authenticates them and the network should hide them.
`cargo deny` was not run here (the tool is not in the builder image); the
new crates are MIT / Apache-2.0 by their manifests.

## Checkpoint (2026-09-16, HEAD `f0adb95` + this note)

**Commits since the P4 checkpoint** (the five decisions, in the order they
shipped): `59c2b30` batch 422 · `0a42699` capture on by default · `9ab2fb2`
synchronous replication and the long-poll · `3be0f12` the identity database
shipped whole · `f0adb95` consensus. One item per commit, signed off, no
branding. `Cargo.toml` and `Cargo.lock` changed once, for `openraft`
(the maintainer allowed dependencies for the consensus item).

**Suite.** 3,067 passed / 0 failed / 2 ignored (the pre-existing one and the 9M harness) over 87 binaries at `f0adb95`, the default environment. Clippy (`--all-targets -D warnings`) clean at
every commit. Conformance table regenerated at each commit that added
tests.

**What the maintainer decided, and what it became.**

| Decision | Shipped as |
|---|---|
| 1 Synchronous hot: the recommendation, with the settings and the modes explained | named sync followers, `required` (`all` allowed), a 2 s default timeout, visible degradation (`X-Replication-Ack`, `sync` in the status), automatic recovery, no policy knob; the long-poll on the change endpoint so a hot follower's lag is a round trip; the modes table in `docs/operations.md` |
| 2 Consensus: make a choice, dependencies allowed | `openraft` 0.9.25; Raft elects and fences, the change log stays the data path; automatic failover; quorum acknowledgement derived from the cluster; the Raft transport over the server's port behind a shared secret |
| 3 Identity database: WAL-style | the database's own bytes, not an event log: a consistent snapshot from SQLite's backup API, applied in place under the follower's open connections when SQLite's own change counter moves |
| 4 Change capture on by default, explained | on unless `OTS_CHANGE_CAPTURE=off`; a leader or cluster member always on; a follower off unless asked; the configuration row and the docs say what it is for and what it costs |
| 5 `/sparql/batch` 422 with a message | 422 on a rolled-back batch, `error` naming the statement and why; 200 for an applied batch, 400/403 for parse and authorisation |

**Still the maintainer's, or worth knowing.**

1. **Persisting the Raft vote.** The log and vote are in memory (the
   module docs and `docs/operations.md` say why that is safe for an
   election-only state machine over an epoch-fenced data path). A
   persistent vote is a small addition if a status ever shows a double
   election.
2. **Asset shipping** stays "share the S3 bucket".
3. **`cargo deny`** was not run on the new crates (not in the builder
   image); their manifests say MIT / Apache-2.0.
4. **The boot seed on a follower** still logs its read-only refusals
   (the seed callers are outside the programme's scope).
5. **`v0.6.0`**: the CHANGELOG fold is the last commit before the release
   PR; nothing is tagged or pushed.

## Phase P5 — the analytical layer, built (2026-09-16)

The maintainer's steer after P2's design notes and P4: implement the SPARQL
evaluator in opengraph; the QLever implementation, configurable and on by
default; DuckDB only with an argument for it; a before-and-after measurement.
Three items — two of code, one of argument.

### 1. A columnar copy with its own SPARQL evaluator

**What it is.** `opengraph/src/columnar/`. `index.rs`: a `Dictionary` (terms to
`u32` ids, id 0 the default graph) and a `Columnar` of three sorted
permutations of the quads, graph-first `GSPO`, `GPOS`, `GOSP`, as
`Vec<[u32; 4]>` (rayon sort, dedup); a triple pattern is a `partition_point`
range on the permutation whose prefix it binds, `plan` picks the permutation.
`value.rs`: SPARQL value semantics on decoded terms over `oxsdatatypes`, the
crate Oxigraph itself uses. `eval.rs`: a static `accepts(&Query)` and an
`evaluate` over id rows — index nested loops in selectivity order, hash joins,
the solution modifiers, `GROUP BY`, and the expression language. In the mirror
(`src/store/parallel_mirror.rs`) it is a third copy, built from the full copy in
the same rebuild under the same cap and the same dirty protocol; `engine.rs`
consults it **after the shards and before the full copy**. `Served::Columnar`
in the telemetry; `OTS_COLUMNAR_QUERY` (on).

**Why an evaluator and not a translator.** The design note (§6.3) found the
fidelity problems of a SPARQL→SQL path one by one — unbound-aware joins, SPARQL
value equality against SQL equality, numeric promotion, the mixed-term order —
each a place a second engine answers differently unless the translator rewrites
around it. An evaluator has no translator: it implements the semantics once, on
ids. What it gives up is coverage, and that turned out to be the whole story of
this item.

**Why it sits after the shards.** The first cut put it first, before the
shards. That is wrong: a decomposable aggregate is 8–11x faster across the
shards' sixteen cores (§3) than in one single-threaded evaluator, so putting
the columnar copy first would have taken that work away from the faster path.
It belongs where the full copy is — the row-returning joins and ordered results
that the shards cannot decompose.

**What the review found, and what it cost.** The first version passed a parity
suite of about a hundred queries. An adversarial review of the module — seven
independent readings (basic graph patterns and joins; the expression language
and the value space; aggregation and the modifiers; the acceptance gate; graph
scoping; performance; the wiring), each finding refuted by a second reader
before it counted — raised 65 findings and confirmed 52, 38 of them silently
wrong answers. The parity suite had missed all of them.

Rather than take 38 claims on trust — two of them contradicted each other on
what `=` does across datatypes — every trigger became a test.
`opengraph/tests/columnar_corners.rs` runs 74 corner shapes and allows each
only two outcomes: declined, or equal to the engine. **Thirty-four diverged.**
The worst, and the one that justifies the whole exercise:

```sparql
SELECT * WHERE { ?s ex:p/(ex:r|ex:name) ?o }    # engine 5 rows, evaluator 21
```

The parser emits a sequence with an alternative on one side as two sibling
algebra nodes joined through a fresh blank node. `eval_bgp` stripped
blank-node columns at the end of each basic graph pattern, so the join key
vanished and the join became a cross product — on a 2M-quad mirror, an
unbounded one.

**What shipped as a result.** Declines where matching the engine exactly was
not worth the surface, because a decline is always safe — the engine answers:
property paths the parser cannot fold into a basic graph pattern; `GRAPH ?g`
over a body that need not bind a triple, or with `?g` reused inside it;
`SUBSTR`, `STRLANG`, `STRDT` and the date/time accessors; `REGEX`/`REPLACE`
with computed or `q` flags; and this module's own reserved variable namespace.
Fixes where the engine's rule was cheap to implement and the corner suite could
hold it there: an ill-typed literal is a type error everywhere rather than an
opaque string; a language-tagged literal has no effective boolean value; `=`
between two well-typed literals of different kinds is **false**, not an error
(so `!=` keeps the rows the engine keeps), while identical terms are equal
whatever their datatype — which is what makes `?v <= ?v` hold for every term;
`ORDER BY` breaks a tie between incomparable literals by lexical form then
datatype, not the reverse; an error anywhere in a `COUNT`, `MIN` or `MAX` makes
the whole aggregate unbound; `GROUP_CONCAT` is unbound unless every member is a
string; `STR()` of a blank node is a type error. The unreachable path expander
was then removed rather than left to mislead.

**The `LIMIT` budget.** The first measurement caught a 16x regression:
`query/lookup_with_limit/10000` went from 29 µs to 464 µs, because `Slice`
evaluated the whole inner pattern and then truncated while the engine stops
early. The evaluator now carries a row budget down through the operators that
are one-row-in-one-row-out (`Project`, `BIND`, `GRAPH`) or that can widen it
arithmetically (`OFFSET` + `LIMIT`, a `UNION`'s two sides); every other
operator bounds only its own output, because an operator that drops rows needs
more input than output. Inside a basic graph pattern only the *last* pattern
may stop early — a row an earlier one produced can still be dropped by a later
one. The budget is therefore a pure early exit on the same row order, and the
suite pins that directly: for every shape and every `n`, `LIMIT n` equals the
first `n` rows of the same query unlimited, and `OFFSET k LIMIT n` the `n`
after the first `k`. An `ASK` runs with a budget of one row.

**Tests.** `opengraph/tests/columnar_parity.rs` (8): about a hundred queries,
engine against evaluator, as multisets and in order where the query orders,
plus the declines and the budget property. `opengraph/tests/columnar_corners.rs`
(8): the 74 corners above. `tests/columnar_query.rs` (2): the live path — served
from the copy per the telemetry, equal to a store whose accelerator is off,
declined shapes fall through, a write dirties the copy. The W3C SPARQL 1.1,
SPARQLoscope and SPARQL-function conformance suites (20 / 67 / 125) run
unchanged.

**Measured.** Against `f0adb95` in a detached worktree, same machine, same
builder image, back to back:

| benchmark | `f0adb95` | this commit | change |
|---|--:|--:|--:|
| `query/simple_lookup/100` | 47.9 µs | 19.5 µs | **-59 %** |
| `query/simple_lookup/1000` | 293.2 µs | 134.4 µs | **-54 %** |
| `query/simple_lookup/10000` | 3.18 ms | 1.54 ms | **-52 %** |
| `query/lookup_with_limit/1000` | 25.1 µs | 7.2 µs | **-72 %** |
| `query/lookup_with_limit/10000` | 30.0 µs | 7.1 µs | **-76 %** |
| `query/lookup_with_limit/100000` | 34.6 µs | 7.2 µs | **-79 %** |
| `query/join_2way/100` | 88.8 µs | 36.2 µs | **-59 %** |
| `query/join_2way/1000` | 691.4 µs | 266.9 µs | **-61 %** |
| `query/join_2way/10000` | 8.44 ms | 3.12 ms | **-63 %** |
| `query/join_3way/100` | 135.3 µs | 52.3 µs | **-61 %** |
| `query/join_3way/1000` | 1.12 ms | 425.3 µs | **-62 %** |
| `query/join_3way/10000` | 13.29 ms | 4.84 ms | **-64 %** |
| `query/filter/1000` | 252.5 µs | 292.2 µs | **+16 %** |
| `query/filter/10000` | 2.52 ms | 2.80 ms | **+11 %** |
| `query/optional/1000` | 710.9 µs | 326.3 µs | **-54 %** |
| `query/optional/10000` | 9.46 ms | 3.49 ms | **-63 %** |
| `query/group_concat/1000` | 657.8 µs | 369.9 µs | **-44 %** |
| `query/group_concat/10000` | 8.34 ms | 3.91 ms | **-53 %** |
| `query/subquery/1000` | 782.2 µs | 414.6 µs | **-47 %** |
| `query/subquery/10000` | 9.12 ms | 4.10 ms | **-55 %** |
| `query/bind/1000` | 430.1 µs | 365.0 µs | **-15 %** |
| `query/bind/10000` | 4.58 ms | 3.57 ms | **-22 %** |
| `query/minus/1000` | 326.9 µs | 155.2 µs | **-53 %** |
| `query/minus/10000` | 3.60 ms | 1.54 ms | **-57 %** |
| `query/construct/1000` | 440.7 µs | 353.0 µs | **-20 %** |
| `query/construct/10000` | 592.2 µs | 338.1 µs | **-43 %** |
| `query/named_graph/1000` | 344.1 µs | 179.5 µs | **-48 %** |
| `query/named_graph/10000` | 3.51 ms | 2.32 ms | **-34 %** |
| `path/sequence/100` | 85.4 µs | 33.2 µs | **-61 %** |
| `path/sequence/500` | 324.9 µs | 128.8 µs | **-60 %** |
| `path/sequence/1000` | 633.3 µs | 247.7 µs | **-61 %** |
| `path/inverse/100` | 49.8 µs | 18.8 µs | **-62 %** |
| `path/inverse/1000` | 301.9 µs | 132.0 µs | **-56 %** |
| `path/inverse/10000` | 3.09 ms | 1.40 ms | **-55 %** |
| `path/negated_property_set/10000` | 15.06 ms | 12.43 ms | **-17 %** |
| *22 other benchmarks* | | | *within ±10 %* |

33 benchmarks are faster and 2 slower by more than 10 %, with 22 unchanged. The largest gain is `query/lookup_with_limit/100000` at -79 %; the largest loss is `query/filter/1000` at +16 %, inside the programme's 20 % bound.

**Dependencies.** `opengraph` gains `oxsdatatypes = "0.2"` and `regex = "1"`,
both already in the lock file through Oxigraph and the main crate; nothing new
is downloaded.

**Stated plainly.** The accepted set is now markedly smaller than the evaluator
can attempt, and it should stay that way until each declined shape has its own
corner test proving it. Two of the review's confirmed findings were *not*
fixed and are not defects in the shipped code because the shapes are declined
— the cross-product path and the unbounded `expand_path` — but they would
return the moment property paths are accepted again. The review's remaining
performance findings (a nested-loop `MINUS` where Oxigraph hash-joins;
`estimate()` counting per candidate per planning round; `REGEX` recompiled per
solution; the query parsed more than once on the way in) are real and unfixed;
none of them regressed a benchmark, so none was worth the risk in this pass.

### 2. QLever as a read backend, fed from the change log

**What.** `src/store/qlever.rs`: a feeder and a router. The feeder is a
change-log consumer — the same rows a replication follower reads, turned into
SPARQL Update: `full` rows as `DELETE DATA`/`INSERT DATA` in batches, rows that
only say a graph changed (counts, `unknown`, store-scoped, blank nodes) as a
graph replace from the local store, an epoch change as a replace of everything;
its bookmark is the cursor `qlever`, which pins retention. The router sends a
query only while the feeder is caught up (bookmark = newest sequence, no write
in flight) — the mirror's never-stale rule — and only in the shapes the policy
names: `analytical` (an `ASK`, or a query with an aggregate anywhere in it —
wider than the shard classifier's verdict, which is `None` for a count over
`GRAPH ?g`; the first cut used the classifier and the policy test caught it),
`all`, `first` (before the copies, for measurements), `off`. Any error falls
through and is kept in the status. `OTS_QLEVER_URL`, `_ENABLED` (on),
`_ACCESS_TOKEN`, `_ROUTE`, `_BATCH`, `_POLL_MS`, `_TIMEOUT_SECS`; the feeder
thread starts with the store when a URL is set;
`GET /api/admin/qlever/status` (admin); the `qlever` telemetry exit. The
endpoint is a trait — HTTP, or a stand-in in tests.

**Why this shape.** QLever's index is built from a dump, so this is not a
replica; it is a read backend for the tier the mirror cannot hold, and the feed
reuses P2's capture and its cursor semantics. The feeder is short because the
log already says what changed and how well it knows.

**Tests.** `tests/qlever_backend.rs` (3), a stand-in endpoint backed by an
in-memory store: the first round replaces everything, a ground update is a
delta, a scanned over-cap update is a graph replace, the router serves only
when caught up and equals the engine; the policy; the status route. A unit test
for the analytical shape.

**Not measured live.** A head-to-head against a running QLever needs its Docker
image (`adfreiburg/qlever`), a download the brief reserves for the maintainer's
say-so; the `first` route exists for exactly that measurement, and the existing
Fuseki/QLever comparison in `docs/performance.md` stands unchanged. Two of the
review's confirmed findings here are **open**, recorded below rather than
fixed: a graph resync split across several `INSERT DATA` operations turns one
blank node into several on the remote, and a full resync never clears a graph
the endpoint holds that the local store no longer does.

### 3. DuckDB: not built, and why

The brief allowed DuckDB only with an argument for it. None survives:

- **What DuckDB would add is a columnar representation with a vectorised
  engine.** Item 1 provides the representation in-process, without a C++ build,
  at 48 bytes a quad, with an evaluator that speaks SPARQL directly. The
  translation a DuckDB path needs — SPARQL to SQL — is exactly where the design
  note found the fidelity problems (§6.3). Item 1 is the evidence for how
  expensive that class of problem is: *without* a translator, writing the
  semantics directly, an adversarial review still found 34 real divergences. A
  translator to a SQL engine with its own value space and its own null
  semantics would face all of them and more, and would not have the option this
  item used — declining.
- **The workloads DuckDB is best at — scans and aggregates — are already the
  shards'** at the sizes the mirror holds (8–11x across sixteen cores, §3),
  which is also why the columnar copy was moved behind them. Above the cap,
  QLever (item 2) serves them from a proper RDF index with SPARQL fidelity and
  no translation.
- **Build cost.** A bundled DuckDB adds a very large C++ compilation to a
  builder that already produces random cross-compiler faults under RocksDB and
  GEOS, and a binary size the design note flagged as unmeasured.
- **The `/sql` endpoint**, the one deliverable that actually needs a SQL engine
  (§3.2), was not asked for.

If a SQL surface is ever wanted, DataFusion — Apache-2.0, pure Rust, a
whitelistable logical plan (§3.3) — over the columnar copy's arrays is the
route, with the design note's §6–7 as its specification. Until then the
columnar copy and QLever cover the analytical layer's two tiers: under the cap
in-process, above it out of process.

## Checkpoint (2026-09-17, HEAD `d2236a8` + this note)

**Commits.** `8f8b2e0` the columnar copy and its evaluator · `d2236a8` QLever
as a read backend. One item per commit, signed off, no branding. The third
P5 item is an argument, not code, and lives in section 3 above.

**Suite.** 3,080 passed / 0 failed / 2 ignored over 89 binaries (the two
ignored are the pre-existing one and the 9M harness). Clippy
(`--all-targets -D warnings`) clean at both commits. The conformance table
was regenerated at each; the W3C SPARQL 1.1, SPARQLoscope and
SPARQL-function suites (20 / 67 / 125) run unchanged, which is the check
that mattered most for an evaluator.

**Dependencies.** `opengraph` gained `oxsdatatypes` only. `regex` was added
for the evaluator's `REGEX`/`REPLACE` and then removed again when those were
declined, so the crate ends where it started but for one dependency already
in the lock file. `cargo deny` was not run (the tool is not in the builder
image); `oxsdatatypes` is Apache-2.0/MIT by its manifest.

**What the maintainer asked for, and what it became.**

| Asked | Shipped |
|---|---|
| Implement the SPARQL evaluator in opengraph | `opengraph::columnar` — dictionary, three sorted permutations, an evaluator over ids, in the mirror as a third copy after the shards; 33 of 57 benchmarks faster by more than 10 %, 2 slower and both inside the 20 % bound |
| The QLever implementation, configurable, on by default | `src/store/qlever.rs` — a change-log feeder and a route policy, on once `OTS_QLEVER_URL` is set, never serving while behind, every error falling through |
| DuckDB only with an argument for it | Not built. The argument against is section 3: the representation is now in-process without a C++ build, the scans and aggregates are already the shards', and a SPARQL→SQL translator would face every fidelity problem this item hit *plus* a second value space — without the option of declining |
| A before-and-after measurement | The paired table in `docs/performance.md` and in section 1, `f0adb95` against `8f8b2e0` in a detached worktree, back to back, median of the rounds |

**Still the maintainer's, or worth knowing.**

1. **The accepted set is narrow on purpose.** Widening it means adding the
   corner test first: that is what turned 34 silent divergences into 34
   declines or fixes. Property paths are the obvious candidate, and the
   cross-product bug (section 1) is what has to be fixed to accept them.
2. **Two QLever limits are open**, both in the resync path: blank nodes
   split across `INSERT DATA` batches, and a graph the endpoint holds that
   the local store no longer does. Neither can serve a wrong answer to a
   client — the router only reads — but both leave the remote index wrong.
3. **The review's remaining performance findings are unfixed**: `estimate()`
   counts per candidate per planning round, and the query is parsed more
   than once on the way in. Neither regressed a benchmark.
4. **The flat row layout** (`Vec<Vec<Option<u32>>>` → one flat array with a
   width) is the next real speed-up, and is what the two `query/filter`
   regressions are made of.
5. **The 9M measurement, `cargo deny`, the Raft vote, asset shipping** and
   **the boot seed on a follower** are unchanged from the P4 checkpoint.
6. **`v0.6.0`**: the CHANGELOG fold is still the last commit before the
   release PR; nothing is tagged or pushed.

