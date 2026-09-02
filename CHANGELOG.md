# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Convention.** Released sections SHOULD list the standard groups in the order
> `Added, Changed, Deprecated, Removed, Fixed, Security`, and SHOULD always include
> `### Deprecated` and `### Security` — writing `None.` when there is nothing to
> report. The annotated release tag and the published GitHub Release carry the
> section verbatim, so this keeps each release's security and deprecation posture
> explicit. See [`docs/release-process.md`](docs/release-process.md).

## [Unreleased]

### Added
- **Conformance layer (TBox/ABox separation).** `GET /api/datasets/:id/conformance`
  resolves a dataset's graphs by role, the model version it declares
  conformance to (`conforms_to_model`/`conforms_to_version`, published as
  `dct:conformsTo`), its bound shape graphs, and the derived
  `reasoning_sources` / `validation_shapes`. `POST /api/reasoning/materialize`
  takes `"dataset"` to reason over exactly that layer.
- **Layered-graph roles.** `GraphKind` now covers the whole one-graph-per-role
  convention — `instances`, `model` (alias `ontology`), `vocabulary`, `shapes`,
  `domain-values`, `linkset`, `provenance`, `catalog`, plus the orthogonal
  `entailment` and `system` — declarable per dataset, per graph and in seed
  bundle manifests, and inferred on import for alignment-only, PROV-O, DCAT/VoID
  and bare-SKOS-collection graphs. Domain-neutral by construction: the roles
  are NEN 2660-2's layers, but nothing in them is BIM-specific.
- `OTS_EXTERNAL_REASONER=konclude` / `OTS_EXTERNAL_REASONER_BIN` select the
  external OWL 2 DL reasoner bridge (experimental). It was previously
  unreachable: the materialiser hard-coded the native stub.
- Spark now orients every turn on what the question NAMES before the model writes
  a query. The platform context lists the registered data models & vocabularies
  with the named graph holding each one's current published definitions (so a
  question about a registered model's classes targets the registry graph, not an
  instance graph); IRIs the user pastes are located in the store with indexed probes
  (which readable graphs, which triple position); and the question's identifier
  tokens and salient words are resolved through the full-text index to the
  subjects and graphs that actually carry them. The findings ride into the prompt
  as a verified "where this conversation's names occur" section, and the graphs
  they point at take vocabulary-sampling slots ahead of the size heuristics.
- Spark discovers the serving model's **context window from the gateway** when
  `LLM_CONTEXT_TOKENS` is unset — vLLM's `max_model_len` on `/v1/models`, or an
  Ollama Modelfile `num_ctx` via `/api/show` — and budgets its prompt against
  it, instead of only against the declared knob. A declared window always wins.
  An Ollama model without a Modelfile `num_ctx` is deliberately NOT guessed at
  (its true serving context is invisible over the API, and both possible
  guesses hurt): the server warns once, and again per over-large prompt, that
  `LLM_CONTEXT_TOKENS` should mirror the real `OLLAMA_CONTEXT_LENGTH`.
  `GET /api/llm/health` reports the chat model and the effective window, so a
  misconfigured stack is visible instead of just wrong.
- Spark retrieval limits became knobs: `LLM_CHAT_MAX_ROUNDS` (default 3,
  clamped 1–8) and `LLM_CHAT_QUERY_MAX_SECS` (default 30, clamped 5–600) — a
  capable model on multi-part questions makes good use of more rounds, and a
  large ontology sometimes needs more than 30s for a legitimate property path.
- The vocabulary sample **widens with the window** (20 graphs × 16 classes +
  32 predicates at a declared 32k+, instead of 12 × 8 + 20), each block marks
  graphs whose members are `owl:Class`/`skos:Concept`-like as *"DEFINES terms"*
  so the model can tell a definitions graph from an instance graph, and a
  zero-row round's repair hint now embeds the queried graphs' **actual**
  sampled vocabulary — ground truth instead of "re-read the section" (which
  may not even cover the graph the query targeted).
- A turn whose every retrieval came back empty gets a mechanical epistemic
  caveat appended ("the data was not found, which is not proof it does not
  exist") — small models reliably upgrade "not found" to "does not exist"
  regardless of instructions, in whichever language they answer.
- **Spark speaks native tool calling** where the model supports it: every
  completion offers `run_sparql`, `text_search` and `vocab_term_search` as
  OpenAI-style function tools, running through the exact same scoped pipeline
  and user-visible trail as the `SPARQL:` directive protocol, which keeps
  working unchanged — the two run as a hybrid in one loop, so a model that
  ignores tools loses nothing. A gateway that rejects the `tools` parameter is
  retried without them once and remembered. `LLM_CHAT_TOOLS=off` disables.
- **Spark asks instead of guessing**: a new ```ask answer widget renders a
  question with clickable options (the click arrives as the user's next
  message), the system prompt instructs the model to use it whenever a real
  choice is open — and the platform context now lists in-scope **unpublished
  draft versions** of registered models as exactly that, so "published or
  draft?" becomes such a question instead of a silent guess.
- **Spark plans multi-part questions**: the model may declare a short `PLAN:`
  (one line per data need) with its first retrieval; the server repeats the
  plan back with every round's results and strips it from the final answer —
  long questions get worked through instead of half-answered.
- **Question words resolve against the installed vocabularies**: orientation
  now also consults the vocabulary term index (the engine behind
  `/api/vocab/terms/search`), so a plain word arrives with candidate standard
  term IRIs and labels before the model can coin one.

### Changed
- **Standards grades now match the code.** GeoSPARQL 1.1 (no geodesic
  metric family, `aggUnion`, GeoJSON literals or Query Rewrite Extension),
  OWL 2 RL (no Table 8 datatype rules), OWL 2 EL (`owl:equivalentClass` and
  `owl:TransitiveProperty` not applied), RML/R2RML (file sources only, no
  joins, first predicate/object map only), SHACL-AF (no custom constraint
  components, `sh:ask`, rule `sh:condition`/`sh:order`) and SHACL-C (subset)
  are graded *Partial* with the gaps listed. The README's "all 30 OGC
  requirements" and "all ~80 OWL 2 RL rules" are gone.
- **The conformance table is generated** from the test suites
  (`scripts/conformance_table.py`, checked in CI on GitHub and GitLab) and
  states per row whether a vendored corpus or a spec-derived suite runs.
- **The service description no longer advertises `sd:BasicFederatedQuery`**,
  since `SERVICE` is disabled by design.
- `plugin-accounts-dashboard` is compiled in GitHub CI; a per-feature
  availability matrix lives in docs/build-features.md.
- **ShEx and SWRL are graded Partial** in docs/standards.md, with the covered
  constructs listed; both now have semantic conformance suites instead of
  route-liveness smokes.
- **Dependencies.** Batched the outstanding Dependabot updates — `jsonwebtoken` 11
  (now on its pure-Rust `rust_crypto` backend; v11 ships **no** signing backend by
  default), `quick-xml` 0.42 (names and attribute values moved from bytes to
  `&str`), `lru` 0.18, `eslint` 10 (with `@eslint/js` 10), `maplibre-gl` 6.6,
  `uuid`, `futures`, `thiserror`, `symphonia`, `http-body-util`,
  `aws-smithy-http-client`, `parry3d-f64`, `cytoscape`, `globals`,
  `swagger-ui-dist`, `@codemirror/commands`, `@testing-library/jest-dom`,
  `@typescript-eslint/parser`, `@sveltejs/vite-plugin-svelte`,
  `eslint-plugin-svelte`, and the SHA-pinned `Swatinem/rust-cache` action —
  together with the code migrations they require. `svelte/no-reactive-functions`
  is disabled (its fixer calls an API eslint 10 removed and crashes the lint run);
  the stale `lru` advisory ignore was dropped from `deny.toml`.
- **Lint.** eslint 10's new `no-useless-assignment` now runs at its recommended
  `error` severity for `.js`/`.ts`, and the nine genuine dead stores it found —
  five in `.js`/`.ts`, four in plain helper functions inside components — are
  gone. It is off for `.svelte`: the rule assumes statements run once, so it
  cannot see that `$: if (x !== lastX) { lastX = x; … }` reads its own write on
  the next run, and all 28 remaining hits were that shape.
- **SAML 2.0 is now marked experimental and excluded from the `full` feature**
  (and so from the published image). The ACS handler has never been verified
  against a real IdP and has a known request-ID validation defect that makes
  every login fail; rather than ship a provider type that cannot work, it is
  gated behind an explicit `--features saml` build and labelled experimental in
  the admin UI. OIDC is unaffected. CI still compiles the feature.
- **The default Cargo feature set is now `full`.** A plain `cargo build`
  previously produced a binary with none of the optional standards compiled in,
  while the README's native install path told you to build exactly that way.
- **The `alerting` and `backup-encrypt` features are now part of `full`**, so
  the documented `ALERT_*` and `BACKUP_ENCRYPT` knobs work in the published
  image instead of being silently ignored.

### Deprecated
- None.

### Removed
- None.

### Fixed
- **Reasoning could not see dataset data.** `POST /api/reasoning/materialize`
  parsed `source_graphs` and ignored it, and every regime's rules read only
  the unnamed default graph — so a dataset's named graphs were invisible to
  materialisation however the endpoint was called. Scopes are now applied at
  the store level (a `USING` dataset on every rule): `dataset`, explicit
  `source_graphs` (read-checked per graph), or the default graph as before.
- **An unknown graph role is a 400.** Setting a dataset's or graph's role to
  a misspelled value used to fold to "no role" and silently clear it with a 200.
- **Data-model version gates answer 403, not 401,** to an authenticated
  non-admin (publish, deprecate, sub-graph transitions).
- **The in-app docs viewer surfaces every guide** — 18 were in the repository
  but never registered (every OWL 2 guide, LDP, RML, SPARQL 1.2, plugins, …);
  a unit test now keeps the registry complete.
- docs/sparql-12.md documented a builder API and a crate version that do not
  exist; docs/triplestore-comparison.md claimed the full W3C SPARQL 1.1
  corpus runs in CI; several guides still said Oxigraph 0.4. Corrected.
- **Spark no longer re-runs a query that already failed this turn.** The
  repeat is recorded in the retrieval trail with its own error, the store is
  not consulted, and the model is told the query was not run. The parser's
  "variable that is unbound" message — an aggregate projected next to an
  ungrouped variable, the exact failure two local models produced — now
  carries an actionable GROUP BY hint.
- **Spark runs a corrected query after a failed round.** A ```sparql fence
  counted as a query only while no round had been *attempted*; after a failed
  round the repair prompt invites "a corrected query as a fence", and models
  that took that path (a 1.5B and a 7.6B one, live) got a query card instead
  of results and answered from memory. A fence is now executed as long as no
  round has *succeeded* — the rule's own stated rationale — within the
  existing per-turn round cap.
- **Spark's system prompt taught doubled braces.** The worked SPARQL examples
  in the prompt were written `WHERE {{ GRAPH <g> {{ … }} }}` — a Rust format
  escape in a constant that is never formatted — so models saw that as the
  canonical query shape. Small models copied it verbatim, every query failed
  to parse, and the turn ended with `ran_query=false` and the broken query as
  the answer. The examples are plain SPARQL now, and a test asserts the prompt
  sent to the gateway contains no `{{`.
- **Spark keeps vocabulary at small context windows.** When the system prompt
  exceeded the declared window the budgeter dropped *every* graph-vocabulary
  block, so the model was left with graph IRIs and no predicates — and
  invented them, or fabricated an answer outright (observed live at an 8k
  window on a demo-seeded instance). Blocks are now trimmed one graph at a
  time, lowest priority first, so the conversation's own graphs stay described.
- **Spark degrades cleanly without a gateway:** an unreachable or failing
  `LLM_GATEWAY_URL` is now a 503 naming the endpoint and the knob, on the chat
  and feedback paths alike; it was a bare 500 "Internal server error".
- **LDP:** an ETag read from `GET` now satisfies `If-Match` on `PUT`/`PATCH`
  (GET hashed the re-serialised body while writes compared against the raw
  DESCRIBE hash, so every documented read→modify→write round trip ended in
  412); `HEAD` returns exactly GET's headers; `/ldp/constraints` — advertised
  as `constrainedBy` on every response — is served; and `POST` honours
  `Link: <ldp:DirectContainer>; rel="type"` (and Basic/Indirect), so Direct and
  Indirect containers can be created over HTTP.
- **`?entailment=owl2-dl` is honoured.** It was advertised in the OpenAPI spec
  but had no arm in the entailment match, so the query silently ran with no
  entailment graph.
- **ShEx:** CLOSED/EXTRA and value sets compared serialised terms with bare
  IRIs and never matched; the datatype check was a substring test skipped for
  simple literals (`"thirty"` satisfied `xsd:integer`); an unparseable schema
  parsed as an *empty* schema and validated everything with a 200. All fixed;
  semantics are pinned by a new conformance suite.
- **Cesium viewer works air-gapped:** the engine's runtime assets are served
  from the application's own bundle instead of a CDN pinned 21 minor versions
  behind the bundled library.
- **`guest` was missing from the frontend role list**, so a guest user's role
  select rendered blank; a parity test now reads the Rust enum.
- **Dataset versions answered 401 to an authenticated caller lacking write
  rights;** it is now 403, so clients no longer treat a missing grant as a
  session expiry.
- Spark's invented-IRI check judged a query's IRIs against the sampled vocabulary
  window (8 classes + 20 predicates per graph), so it routinely condemned REAL
  terms — `rdfs:label` where only `rdfs:comment` made a sample, a class outside a
  big ontology's top 8, even a legitimate named graph whose siblings were sampled
  — burning retrieval rounds on false "does not exist" errors and steering the
  model away from IRIs the user had pasted verbatim. Candidates are now verified
  against the store itself (four indexed probes: subject, predicate, object,
  named graph), and IRIs the user pasted are never rejected at all — an absent
  one runs to an honest empty result instead of an error blaming the user.
- The content-negotiated Turtle service description (`GET /` with an RDF `Accept`
  type) counted every accessible graph with a full `count_graph()` scan; on a store
  with a multi-million-triple graph that took tens of seconds per request. It now
  reads the maintained O(1) per-graph count index (`graph_count_cached`), the same
  fix the DCAT `void:triples` counts already had. Value-identical output.
- `cleanup-containers.sh` only removed three hardcoded container names, missing the
  project-prefixed variants other workspaces' `docker compose` runs create (e.g.
  `<dir>-minio-1`) — the exact "container name already in use" conflicts it exists
  to prevent. It now matches any container with a project-specific name component,
  while deliberately never matching bare service names like `minio`, so containers
  from unrelated projects are left alone.
- **A `geo:wktLiteral` prefixed with EPSG:4326 is now read in the authority's
  `(latitude, longitude)` axis order**, as GeoSPARQL prescribes; only the
  unprefixed default and `OGC/1.3/CRS84` are `(longitude, latitude)`. Both were
  treated as lon/lat, which transposed every authority-ordered geometry.
  `geof:transform` into EPSG:4326 likewise emits lat/lon, and results that
  default to WGS84 lon/lat now carry the CRS84 URI rather than the EPSG one.
  **This changes results for data that carries the EPSG:4326 prefix but was
  written lon/lat in violation of the spec** — a common real-world mistake. If
  your data does that, relabel it CRS84 (or drop the prefix); the store now
  believes the prefix.
- **GeoSPARQL binary functions now harmonise their operands' coordinate
  reference systems.** Both `<crs>` prefixes were previously stripped and
  discarded, so a query mixing RD New (EPSG:28992, metres) with CRS84 (degrees)
  compared incompatible numbers and returned a confident `false`. The second
  operand is transformed into the first's CRS; when the two name different CRS
  and either is one this build cannot reproject, the result is now unbound
  rather than wrong. **This changes results for existing mixed-CRS data** — for
  the better, but re-check any saved query or shape that relied on the previous
  behaviour.
- **Constructive GeoSPARQL functions keep their operand's CRS.** `geof:buffer`,
  `geof:envelope`, `geof:boundary`, `geof:convexHull`, `geof:intersection`,
  `geof:union`, `geof:difference` and `geof:symDifference` emitted bare WKT with
  the prefix dropped, so `geof:getSRID(geof:buffer(<RD New geometry>, 10))`
  reported CRS84 — relabelling metres as degrees and making the result unusable
  as an operand.

### Security
- None.

## [0.6.0] — 2026-07-31

### Added
- **The store as an OIDC provider** (Unified Accounts): client apps sign
  their users in against this store with authorization-code + PKCE —
  discovery, `/oauth/jwks` (ES256), `/oauth/token` (rotating single-use
  refresh tokens), `/oauth/userinfo`, an SPA-driven `/oauth/authorize` with
  a remembered consent screen, an admin-managed client registry
  (Security → *Sign-in apps*, `/api/admin/oauth-clients`) and a declarative
  `OAUTH_CLIENTS_JSON` boot seed. Provider access tokens carry role and
  org/group membership claims and are accepted by the auth middleware like
  any first-class credential. See [`docs/oidc-provider.md`](docs/oidc-provider.md).
- **Dataset file manager**: files and assets are managed like a real file
  system rather than a flat list — folders per dataset, a full file-browser UI
  replacing the dataset page's asset list, a new top-level **Files** page, and a
  reusable browser modal/picker. Folders are database rows (`assets.folder` plus
  an `asset_folders` table for explicitly created empty ones), so moves and
  renames never touch storage keys — bytes and ETags stay stable. The
  `/api/datasets/:id/folders` API creates, renames (subtree, rewriting contained
  assets' RDF folder literals) and deletes folders, with traversal-, depth- and
  length-checked path sanitising, and public datasets stay browsable logged-out.
- **Guest self-registration toggle** (admin, default off): with normal
  registration closed, the public register page may create low-privilege
  `guest` accounts. Turning the toggle off bulk-disables guest accounts with
  a specific "guest access has been disabled by the administrator" sign-in
  message; turning it back on re-enables exactly those accounts.
- **Configurable guest capabilities and OIDC token authority**: both principals
  carried more authority than their names implied, and the right limit differs
  per deployment, so both become policy with a conservative default (new
  `auth::policy` module, documented in `.env.example`).
  `OTS_GUEST_CAPABILITIES` selects from `write` / `create_datasets` /
  `api_tokens` / `publish` / `all` and defaults to read-only, applied as a clamp
  on every authentication path.
- **Membership-aware introspection**: `GET /api/auth/me` now includes
  `organisations` and `groups` arrays, and the new
  `GET /api/datasets/:id/permissions/me` reports the caller's effective
  `{read, write, manage}` on a dataset (404 for invisible datasets) — for
  resource servers that authorize on ownership without re-deriving ACLs.
- **SHACL for 3D and BIM**: three built-in example shape graphs and pipelines
  wired to the 3D/Map/BIM demo — `ifc` (IFC building elements: labels, exactly
  one `props:ifcGuid`, IRI sub-element links), `geo3d` (every `geo:Feature`
  carries a geometry; a `POLYHEDRALSURFACE Z` solid has ≥4 faces) and `file3d`
  (3D distributions declare `dct:format` + `dcat:downloadURL`) — plus a
  `validation-3d.ttl` demo graph carrying deliberate failures so the examples
  always surface a real violation. Validation issues are now openable in 3D:
  `IssueResults` gains a **Show in 3D** action and `DatasetViewer` honours
  `?focus=<iri>`, framing and highlighting the element by IRI or IFC GlobalId.
- **Plugin accounts capability**: `ots-plugin-api` 0.2 adds
  `PluginContext::auth` (`PluginAuth`: bearer introspection + admin-gated
  users/organisations/LLM-stats overviews, enforced host-side), plus the new
  `plugins/accounts-dashboard` crate (feature `plugin-accounts-dashboard`,
  off by default): a deployment-wide accounts/entitlements/LLM-usage dashboard at
  `/ext/accounts-dashboard/ui`, merging the store's own AI-request log with
  an external LLM gateway's usage ledger (fail-soft). The Docker image gained
  a `CARGO_FEATURES` build arg to enable plugin features without patching.
- **Seed-bundle `[prefixes]` table**: a bundle may declare
  `prefix → namespace` mappings, seeded into the prefix registry's persisted
  cache tier at boot (existing entries win) — so bundled datasets render
  prefixed names out of the box.

### Changed
- Login accepts an internal-path `?next=` redirect (used by the OIDC
  authorize flow); absolute URLs are ignored (no open redirect).
- **The performance gate now compares a change against its own merge base**,
  both benched in the same job, instead of against the stored baseline — runner
  hardware drift no longer reads as a regression. It fails at +10 %, with a
  small-benchmark floor below 1 µs (where a percentage bar is meaningless) and
  per-benchmark tolerances for the three benchmarks measured to be bimodal on
  these runners. See [`docs/performance.md`](docs/performance.md).
- **Dependencies**: a coordinated aws-sdk/aws-smithy bump, and a batch of 19
  further updates including `age` 0.12, `hmac` 0.13 (with `sha1` 0.11 in
  lockstep — the digest 0.11 trait family), `tower-http` 0.7, `geos` 11.1 and
  `eslint-plugin-svelte` 3. `jsdom` 30 and TypeScript 7 are deliberately held:
  the former requires a Node baseline past this project's Node 20, and
  typescript-eslint does not yet support the latter.

### Deprecated
- None.

### Removed
- None.

### Fixed
- **3D models were unreachable from any device but the host**: the viewer feed
  baked the origin into model URLs at seed time, so a default install served
  `http://localhost:7878/…` to every client. Self-hosted URLs are now rewritten
  origin-relative in the feed JSON; the RDF and external URLs are untouched.
- **A failed model load froze the tab**: a worker-side fetch/parse failure fell
  back to re-parsing tens of megabytes of IFC on the main thread. The fallback
  now fires only when the worker cannot start.
- **Blank basemap under MapLibre 6**, which moved its worker out of the bundle
  and resolved it at runtime — Vite never emitted the file, the SPA fallback
  served `index.html`, and the worker died silently.
- **Buildings sliced in half by basemap suppression**: suppression now hides
  whole buildings by feature id rather than by a `distance` filter evaluated per
  tile fragment, and resets across entry rebuilds. Model orientation is now
  measured rather than guessed — `ots:modelHeading` flows RDF → feed → placement
  matrix, with landmark bearings taken from real OSM footprints.
- **The query cache deep-copied results it then discarded**: `QueryCache::put`
  decomposed every solution into a row vector *while* pulling, before it could
  know whether the result fit under the cap. Every SELECT over the 10 000-row
  cap paid ~10 001 throwaway row allocations and the matching term clones for
  nothing. Solutions are now buffered untouched and decomposed only in the
  branch that actually caches; variable lookup also stops being quadratic in
  projection width.
- **Seeded prefixes now outrank the bundled snapshot**, so a bundle's own
  `[prefixes]` mappings are not shadowed by stale snapshot entries.
- The default-features build (`cargo check`/`cargo test` with no flags) broke
  on a `text-search`-gated `AtomicBool` import used by an ungated field, and
  on an ungated `Term::Triple` match arm in the SPARQL-functions conformance
  test. Both are feature-gated correctly now; CI's explicit feature list had
  masked them.

### Security
- **SPARQL injection through version strings** (`insert_version` and
  `get_version`): a caller-supplied version containing a quote, backslash,
  angle bracket or newline could close the literal and continue the query, and
  was reachable from seven upload/seed/pipeline paths. All sinks now validate
  through `data_models::version_iri::validate_version`; the two HTTP boundaries
  reject with `400` rather than `500`.
- **Refresh-token rotation was not atomic**: `take_client_refresh_token` ran a
  `SELECT` followed by a separate `DELETE` whose affected-row count it
  discarded, so two concurrent `grant_type=refresh_token` requests could both
  observe the row and both be issued a fresh token pair — making a stolen
  refresh token replayable inside that window and defeating the single-use
  guarantee rotation exists to provide.
- **Guests were unconstrained**: `SystemRole::Guest` was stored but no predicate
  consulted it, so a self-registered guest could create datasets, write graph
  data, publish and mint API tokens like a full user. Guest authority is now
  clamped on every authentication path and defaults to read-only.

## [0.5.0] — 2026-07-24

### Added
- **Outbound email in Docker Compose** (`--profile mail`): a bundled send-only
  Postfix relay ([`boky/postfix`](https://github.com/bokysan/docker-postfix)) so
  account mail (verification links, password resets, username reminders) is
  actually delivered instead of only logged. The relay is reachable solely on the
  compose network (no host port), persists its queue across restarts, and either
  delivers directly to recipient MXes or routes through a smarthost
  (`MAIL_RELAYHOST` + credentials). All account-email settings (`SMTP_*`,
  `PUBLIC_BASE_URL`, `OTS_REQUIRE_VERIFIED_EMAIL`) are now wired through
  `docker-compose.yml`, so setting them in `.env` is enough. See `.env.example`
  and [`docs/auth.md`](docs/auth.md).
- `SMTP_TLS` option for the account mailer: `none` | `starttls` | `implicit`.
  `none` (plaintext) enables the hop to a relay on a trusted private network —
  like the bundled compose relay; the legacy `SMTP_STARTTLS` switch still works
  and the port-based default (465 ⇒ implicit TLS, else STARTTLS) is unchanged.
- **Per-building selection in shared CityJSON blocks** (3D/map viewer): CityJSON
  now carries per-`CityObject` identity (the analogue of an IFC `#GlobalId`), so
  clicking one house in a LoD2 block selects *that* building — opening its
  linked-data inspector when it maps to an RDF element (the authored
  neighbourhood/zone buildings, wired via a `#objectId` model link in the seed),
  or a BAG-id/attributes popup with an x-ray highlight for geometry-only houses
  (the 3DBAG block). A `#objectId` fragment also isolates a single building in the
  element modal's 3D tab.
- **Walk / Fly walkthrough modes** for IFC buildings: the first-person view now
  offers a true ground-bound **Walk** mode (eye-height, gravity, floor/stair
  follow, Space to jump) alongside free-fly **Fly** (creative/"god") mode —
  toggle in the header or with `F`. An **Explore inside** action in a building's
  inspector opens the walkthrough directly (no longer only via the zoomed-in map
  hint).
- **Internal vocabulary search + prefix service** (LOV & prefix.cc replacement).
  Public LOV is unreachable and prefix.cc's TLS certificate has expired, so both
  are now first-class internal services integrated with the model/vocabulary
  registry. A bundled prefix snapshot (3,695 prefix.cc + LOV mappings with a live
  overlay of platform-registered vocabularies) resolves SPARQL auto-prefixing
  fully offline (`/api/prefixes*`; live prefix.cc is opt-in via
  `PREFIX_CC_FALLBACK`), and a Tantivy-backed vocabulary term search (the
  `vocab-search` feature) indexes the bundled LOV corpus plus the platform's
  registry vocabularies. Both degrade gracefully with no network access.
- **Real per-building 3DBAG linked data** in the 3D/BIM demo: each 3DBAG `Pand`
  is mapped to an addressable RDF element, so the neighbourhood block is real,
  properly-georeferenced linked data end to end rather than a geometry-only
  overlay.

### Changed
- **Dependencies.** Batched the outstanding Dependabot updates — `aes-gcm` 0.11,
  `quick-xml` 0.41, `toml` 1, `zip` 3, `calamine` 0.36, `lru` 0.16,
  `maplibre-gl` 6, `three` 0.185, and others — together with the breaking-API
  migrations they require, and migrated the SPARQL engine off oxigraph 0.5's
  deprecated `Store::query` / `Update` API onto the `SparqlEvaluator` interface.
  CI clippy now runs with `-D warnings`, so warnings fail the build.

### Deprecated
- None.

### Removed
- None.

### Fixed
- Outgoing email now carries a proper RFC 5322 `Message-ID` (`<uuid@from-domain>`),
  in the account mailer and in both `ALERT_SMTP_*` alerting senders. Gmail
  rejects messages without a valid Message-ID outright (`550 5.7.1`), and SMTP
  relays only repair the header for clients they consider local — which a
  compose sibling container is not. The bundled relay additionally runs with
  `always_add_missing_headers = yes` as a safety net for any submitter.
- `BASE_URL` set in `.env` now actually reaches the compose container (it was
  recommended in the production `.env` docs but never forwarded), so linked-data
  IRIs, the WebAuthn/passkey relying party and emailed action links pick up the
  deployment's public origin in Docker deployments.
- **3D map viewer — duplicate CityJSON blocks.** A self-georeferenced CityJSON
  file referenced from several elements (a zone *and* its buildings, or the same
  3DBAG block linked from three demo graphs) was rendered once per reference at
  the identical spot, z-fighting into a "duplicated" blur. Each file now renders
  exactly once (a whole-file reference supersedes its object fragments).
- **Big Ben (and other landmark models) colliding with the basemap building.** A
  just-loaded model now suppresses the OSM 3D extrusion it stands on immediately
  (previously only re-evaluated on the next map pan), and a tall, thin tower's
  suppression footprint is floored at a real building size so its own OSM block no
  longer pokes through the model.
- **Ungrounded Dragon Bridge landmark.** Its STL is Z-up (deck height along Z) but
  was unannotated, so it rendered tipped ~82 m onto its side; it now lies flat
  (`ots:modelUpAxis "Z"`).

### Security
- None.

## [0.4.0] — 2026-07-17

### Added
- **Extension/plugin architecture**, so a downstream operator can customize an
  instance without patching upstream source — see [`docs/plugins.md`](docs/plugins.md):
  - **Seed bundles** (`src/seed_bundles/`, `--seed-dir` / `SEED_DIR`): boot-time
    org/dataset/graph/saved-query loading from a directory of `manifest.toml` +
    RDF payload files. Idempotent, fail-soft, per-bundle opt-out env var. The
    bundled standards demo (`src/saved_queries/seed.rs`) now runs through this
    same engine as the reference bundle, and a documented example ships in
    `examples/seed-bundles/`.
  - **Compile-time plugins** (`plugins/api`, `plugins/hello`): a `Plugin` trait
    (routes mounted under `/ext/<name>`, `on_boot`, background-task spawn) plus
    a registry in `src/plugins.rs`. Each plugin is its own crate, enabled by a
    `plugin-<name>` Cargo feature — following the existing `[features]`
    pattern (`rdfs-entailment`, `owl2-*`, …) rather than dynamic library
    loading. `GET /api/plugins` lists what's compiled in. `plugins/hello` is
    both a working example and the copy-this-crate template.
  - **Frontend runtime config**: `serviceRegistry.ts` now resolves each
    backend URL with precedence `VITE_<SERVICE>_URL` (build-time) >
    `/config.json` (runtime, no rebuild) > `/registry` discovery > localhost
    defaults. `/config.json` also carries branding (title, logo, accent color),
    applied at boot with no rebuild — see `runtimeConfig.ts`. `vite.config.js`
    gained an `OTS_BASE_PATH` build-time option for static sub-path deploys.
  - **Opt-in port fallback** (`--port-fallback` / `PORT_FALLBACK`, default
    off): when the requested port is busy, bind any free port instead of
    refusing to start (`src/netutil.rs`), rewriting the advertised base URL
    used for service-registry self-registration to match. Upstream's default
    "refuse to start on a busy port" behavior is unchanged unless this is set.
- **IFC → linked data**: bulk import accepts `.ifc` files — stored as a downloadable
  dataset asset and transformed into a BOT topology graph (storeys/elements,
  property sets, FOG file references) plus a full ifcOWL-style instance lift
  (`src/ifc/`). Graph Store reads gain `?format=` (turtle/jsonld/rdfxml/ntriples/
  trig/nquads) with download disposition, and assets gain an anonymous-capable
  `…/download` route gated by dataset visibility.
- **Schependomlaan demo** replaces the Waalbrug example: the canonical open Dutch
  BIM dataset (Nijmegen, CC BY 4.0) is fetched on first boot (`SEED_IFC_URL`),
  with the real 3DBAG LoD2.2 city block (CC BY 4.0) bundled for the map.
- **Viewer**: in-browser IFC rendering (web-ifc) with per-element picking —
  clicking a beam opens that element's linked-data panel; multiple movable
  element panels with a dock; map layer toggles + legend; "Show on map";
  a model-format picker; ontology viewer standards header + full-page viewer.
- **Spark chat v2**: signed-in users keep their conversations — a history
  sidebar (new / open / rename / delete), restored with their full retrieval
  trail and widgets — plus editable "memory" (standing preferences injected
  into the system prompt, screened for injection at save time). New answer
  widgets: `model3d` (orbit viewer), `file` (preview/download card), and
  `map` with georeferenced 3D `models`. An "About Spark" panel surfaces the
  live model/gateway and grounding/privacy notes.
- **Admin → AI Requests** (`/admin/llm`): a request log for every LLM-backed
  call (chat, NL→SPARQL, SHACL) — outcome, latency, time-to-first-token,
  sizes and the guard rule that fired — with 24h/7-day aggregates. Message
  contents are never stored, only a short question preview (`LLM_LOG_*`).
- **vLLM serving profile** (`docker compose --profile llm-vllm`, NVIDIA GPU):
  automatic prefix caching reuses Spark's shared system prompt across turns
  for near-instant time-to-first-token; the bundled Ollama profile now keeps
  the model resident (`OLLAMA_KEEP_ALIVE`) and serves requests in parallel.

### Changed
- App-wide motion polish: route transitions, staggered table rows, delayed
  loading indicators (no sub-500 ms skeleton flash), reduced-motion guard.
- SPARQL/read rate limit raised to an interactive burst (40 @ 60/min) and 429s
  now carry a standard `Retry-After`; the web client retries them transparently.
- **Developer build speed**: a hot-reload loop (`make watch` / `watch-check` via
  cargo-watch), `make nextest` for parallel tests, dependency-only debuginfo
  stripping for faster debug/test links, a `CARGO_PROFILE` Docker build-arg for
  fast `release-dev` local images, BuildKit cargo/npm cache mounts plus `npm ci`,
  and a separate rust-analyzer target dir to avoid build-lock contention. New
  guide: [`docs/development.md`](docs/development.md).
- Spark chat streams over SSE for fast first tokens; the server keeps a pooled
  gateway connection and builds the prompt deterministically so gateway-side
  prompt caches hit.

### Deprecated
- None.

### Fixed
- STL models rendered lying flat (Z-up vs Y-up) and basemap building extrusions
  overlapping real 3D models on the map.
- Boot seeding serialized + self-healing (a half-seeded instance left public
  demo graphs registered but empty, so logged-out visitors saw no data and a
  zero landing count); SQLite `busy_timeout` now precedes WAL setup.
- Ontology viewer rendered empty for model-registry versions (preloaded store
  now supersedes an empty SPARQL load).
- **Spark**: a guard-rejected question (prompt-injection / rate limit) is no
  longer replayed as context on later turns — one blocked message used to
  re-block every following turn and freeze the chat; rejected questions stay
  visible but dimmed and are excluded from the conversation and from history.
- `docker-compose.yml` no longer hardcodes container names — every service's
  name (and its containers/networks/volumes) now derives from the compose
  project, so a second concurrent `docker compose up` (e.g. a second git
  worktree) no longer fails with "container name already in use".
- Published host ports (`7878`, `9000`/`9001`, `11434`, `8000`) are now
  overridable via `TRIPLESTORE_PORT` / `MINIO_PORT` / `MINIO_CONSOLE_PORT` /
  `OLLAMA_PORT` / `VLLM_PORT` (`.env`), so two concurrent `docker compose up`
  checkouts no longer fight over the same host port; the `info` banner service
  reports the actual configured ports.

### Security
- Authorization matrix tests (role × visibility × endpoint) pinning anonymous
  access to public data across browse/SPARQL/GSP/datasets/service description.
- **LLM guard rails** on every Spark endpoint: a per-principal request rate
  limit (separate from the global governor), size caps, a configurable phrase
  blocklist and prompt-injection heuristics on user input
  (`LLM_GUARD_INJECTION_ACTION` block/flag/off), plus an output screen that
  redacts verbatim system-prompt leaks. Stored chat memory is screened the same
  way at save time. All verdicts land in the admin request log.

## [0.3.0] — 2026-06-10

### Added
- **Spark documentation page** (`docs/spark.md`, in-app at `/docs/spark` under
  *Query & Search*): what the chat assistant is, how answers are grounded (platform
  context + scoped SPARQL, up to 3 query rounds per turn), the widget block grammar
  (`sparql`/`api`/`chart`/`map`/`card`/`csv`) with examples, `LLM_*` configuration,
  and privacy/scope notes. Cross-linked from the overview, API-services doc and README.
- SHACL-SPARQL **prefixes mechanism** (`sh:prefixes` → `sh:declare`/`sh:prefix`/
  `sh:namespace`): a `PREFIX` prologue is now prepended to every `sh:select`,
  `sh:construct` and SPARQL-target body, so constraints/rules/targets that use prefixed
  names (`da:`, `geo:`, `geof:` …) parse instead of being silently skipped.
- Per-constraint `sh:severity` on a `sh:SPARQLConstraint` node (e.g. `sh:Warning`) now
  overrides the shape-level severity for that constraint's results.
- Waalbrug reference-example conformance fixtures (`tests/fixtures/waalbrug/`) and an
  oracle (`tests/waalbrug_conformance.rs`) encoding the IMBOR/NEN 2660-2 GeoSPARQL +
  SHACL (Core/SPARQL/AF) pass/fail matrix.
- SHACL **complex property paths** are now parsed from RDF: sequence paths `( p1 p2 … )`,
  `sh:inversePath`, `sh:alternativePath`, `sh:zeroOrMorePath`, `sh:oneOrMorePath` and
  `sh:zeroOrOnePath` (previously only a single predicate IRI was understood).
- GeoSPARQL **`geo:gmlLiteral`** parsing (GeoSPARQL 1.1 Req 2): the GML 3.2 geometry
  subset — `Point`, `LineString`/`Curve`, `Polygon`/`Surface` and the `Multi*`
  collections — is translated to WKT and handled by the existing GEOS path, so `geof:*`
  functions now accept GML geometry literals (was WKT-only).
- GeoSPARQL **`geof:transform`** for CRS reprojection between EPSG:28992 (Amersfoort /
  RD New), EPSG:4326 / CRS84 (WGS84) and EPSG:3857 (Web Mercator), via pure-Rust
  closed-form transforms (no PROJ dependency). Feeds map/3D reprojection for the viewer.
- `geof:distance` now honours its units-of-measure argument for linear units
  (`metre`/`kilometre`/`centimetre`/`millimetre`) over a metre-based CRS.
- SHACL-AF **`sh:expression`** node expressions (path + comparison subset): values
  reached along an expression's `sh:path` must satisfy its comparison constraints
  (e.g. `sh:minExclusive`), reported with the expression's `sh:message`.
- SHACL-AF **`sh:SPARQLFunction`**: user-defined functions (`sh:parameter`/`sh:order`/
  `sh:select` + `sh:prefixes`) are registered as callable SPARQL functions, usable from
  queries, SHACL-SPARQL constraints and rules (e.g. `ex:afstandMeter`). Bodies are
  evaluated against a fresh in-memory store, fully supporting expression-style functions.
- **Viewer feed** endpoint `GET /api/datasets/:id/viewer-feed`: per-element geometry +
  3D-file references resolved from the BOT/OMG/FOG/GeoSPARQL layering — labels, types,
  parent topology, IFC GlobalId, glTF/IFC/other file URLs, and geometry reprojected to
  EPSG:4326 and EPSG:3857 server-side. Anonymous access works for public datasets.
- **Compliance as data**: every official dataset validation run now also persists its
  `sh:ValidationReport` as RDF into `urn:system:reports:dataset:{id}` (replaced per run),
  so dashboards can query failures via SPARQL; severity rollup stays on the run rows.
- **3D & Map Viewer demo dataset** (`viewer-3d-demo`) in the standards demo seed: the
  Waalbrug bridge (EPSG:28992, IFC/glTF refs) plus real Wikidata landmarks (CC0 —
  Dragon Bridge Da Nang, Big Ben, White House, Empire State Building, Sannō Shrine)
  whose open 3D models live on Wikimedia Commons, and a synthetic CityJSON LoD2
  demo block (EPSG:7415, semantic roof/wall/ground surfaces) bundled with the
  frontend so georeferenced CityJSON rendering is demonstrable offline.
- **Dataset 3D & map viewer** (frontend, `/datasets/:id/viewer`): an interactive map
  (Leaflet, now a bundled npm dependency) and a 3D scene (three.js — glTF via
  GLTFLoader, STL via STLLoader for the Commons landmark models) over the viewer feed,
  with a shared selection: clicking a part on the map, in 3D, or in the element list
  shows that element's linked data (via the existing browse API + `RdfTerm`).
  `GeoPreview` migrated from CDN-loaded Leaflet to the bundled dependency.
- **Geo data explorer** (`/datasets/:id/viewer`, rebuilt): the map is now an explorable
  MapLibre GL world — zoomed out, located elements are dots; zooming in, elements with a
  3D model show the *actual model* standing georeferenced and to real scale next to OSM
  building extrusions (tilt/rotate, streets/satellite basemaps, light + dark styles).
  Clicking a feature or list row opens a draggable element inspector with Properties,
  the BOT/IFC substructure tree (every sub-element navigable and visualizable, IFC
  GlobalId + BIM file facts) and an interactive orbit 3D tab. Datasets without
  geometry fall back to a pure 3D model explorer. Supports glTF, STL, CityJSON and
  CityGML (client-side CRS reprojection via proj4).
- **3D/geo everywhere**: RDF terms rendered anywhere (triple table, graph explorer,
  resource panels, chat) get inline affordances — a map chip on `geo:wktLiteral`
  values and a 3D chip on model-file URLs — opening a global draggable preview
  overlay. Resource detail pages show a 3D model (BIM) card with IFC GlobalId and
  file links (following named `hasGeometry` nodes one hop), and the geometry map
  gains a *to scale* toggle driven by the model's measured real-world size.
  **Projected-CRS WKT (e.g. the Waalbrug demo's EPSG:28992) is now reprojected
  client-side before plotting** — previously raw map previews plotted projected
  coordinates as lon/lat. Dark mode is supported across all maps and 3D scenes.
- **Official conformance suites in CI**: the W3C SHACL core test suite and the OGC
  GeoSPARQL 1.1 SHACL validator (+ its valid/invalid example corpus) are vendored under
  `tests/fixtures/{w3c-shacl,ogc-geosparql}/` and run with a two-way ratchet (unlisted
  tests must pass, listed known-failures must still fail). Scorecards:
  W3C core 46 pass / 52 known-fail / 15 aux skips; OGC examples 44/48 matching, and the
  Waalbrug dataset round-trips through the official GeoSPARQL validator. See
  `docs/conformance/`.

- **Spark chat is now an interactive linked-data canvas.** Assistant answers render
  runnable widgets: `GET /api/.../run` mentions (fenced or inline) become one-click
  API calls whose results show in place exactly like the API-services page (SPARQL
  result table with linked RDF terms, CSV, RDF, JSON — with parameters, dataset
  version and download); fenced ```sparql blocks get Run / copy / open-in-workspace
  actions and execute under the caller's normal read scope; and the model can emit
  ```chart (bar/line/pie), ```map (WGS84 WKT on Leaflet), ```card (entity info card)
  and ```csv preview blocks. Spark itself may now run up to three scoped SPARQL
  rounds per turn (with error feedback for self-repair), the full retrieval trail is
  shown per answer with syntax-highlighted queries, and WKT result cells survive
  long enough to be mapped.

### Changed
- None.

### Deprecated
- None.

### Fixed
- SHACL engine, found by the official conformance suites:
  `sh:not`/`sh:and`/`sh:or`/`sh:xone`/`sh:node` in property-shape context were evaluated
  against the focus node instead of each value node along the path (SHACL §4.6) — e.g.
  an `sh:or` of datatype branches over `geo:asWKT` values mis-fired on every geometry.
  Node-level `sh:nodeKind sh:Literal` could never match (focus nodes are lexical
  strings); a blank/scheme-shaped/other heuristic now classifies them.
- **Cross-store path-cache poisoning**: the per-thread SHACL property-path cache was
  keyed by `(focus, path)` only, and rayon worker caches survive across validation
  passes — two stores in one process sharing a focus IRI and path could serve each other
  stale values, yielding nondeterministic validation results. Cache keys now include a
  process-unique per-store id.
- SHACL-SPARQL constraints, rules and custom targets that referenced prefixed names were
  silently skipped (the query failed to parse and the result was swallowed), so the
  corresponding violations/inferences never appeared. They now resolve via the declared
  `sh:prefixes`.
- An inline blank-node `sh:qualifiedValueShape [ … ]` was silently skipped: the value
  shape was looked up by IRI in the top-level shapes list, where an inline shape never
  appears. It is now loaded inline (like `sh:not`/`and`/`or`) and enforced.
- **Viewer feed**: WKT/GML literals carrying a CRS the server cannot reproject
  (anything beyond EPSG:28992/4326/3857, e.g. EPSG:25832) are no longer emitted
  verbatim as `wkt4326` — projected metre coordinates used to reach the map as
  lon/lat and crash MapLibre's `fitBounds`, breaking the whole explorer; such
  geometries are now omitted (the element still appears, without a location).
  Datasets with plain GeoSPARQL geometry but no BOT containment topology now
  appear in the feed as parentless roots (previously: an empty feed). 3D GML
  (`srsDimension="3"`) coordinate lists now parse correctly (Z dropped) instead
  of mis-pairing into garbage 2D coordinates. The unused per-element `wkt3857`
  field (computed and serialized, read by nothing) was removed.
- **SHACL `sh:nodeKind`** (node shapes): focus-node term kinds are recorded at
  target resolution, so string literals shaped like IRIs (`"mailto:x@y.org"`,
  `"urn:isbn:…"`) reached via `sh:targetObjectsOf` no longer wrongly satisfy
  `sh:IRI` / wrongly violate `sh:Literal`. Custom `sh:SPARQLFunction` bodies
  evaluate against a shared empty store instead of constructing a fresh
  in-memory store per invocation (per binding row).
- **Spark chat**: the `SPARQL:` execution directive only counts when it starts a
  line, and a final answer that embeds a corrected ```sparql block is kept
  instead of being demoted to the bare fallback table; query extraction stops at
  the first code fence (a stray closing ``` and trailing prose no longer get
  glued onto the query); the "values were not retrieved" caveat recognises every
  fence variant the frontend renders (`~~~`, indentation, `geo`/`infocard`
  aliases); GML cells get the same prompt budget as WKT. Client-side: transport
  error bubbles are no longer replayed into the model conversation, feedback
  submits the last *successful* query of the trail, and TSV responses normalise
  CRLF and ragged rows.
- **Viewer UI**: stale-response races on the resource page (slow geometry-hop /
  model-measure fetches from a previously viewed resource no longer paint onto
  the current one); the reused geo-preview overlay no longer goes permanently
  blank when its first preview had unparseable WKT; `GEOMETRYCOLLECTION`
  elements are included in map bounds/focus; out-of-range coordinates can no
  longer crash the map; Escape closes only the topmost panel when the preview
  overlay is stacked over the element inspector, and the inspector's drag
  offset resets on close; fallback 3D-explorer models load concurrently.

### Security
- The element inspector's BIM file links now pass RDF-derived URLs through the
  `safeExternalUrl` scheme allowlist like every other RDF-derived href, closing
  the one sink where an uploaded `javascript:`/`data:` URL round-tripped into an
  `<a href>` (low impact in modern browsers — `target="_blank"` blocks
  new-context `javascript:` navigation — but a gap against the project's own
  XSS control).

## [0.2.4] — 2026-06-09

### Added
- None.

### Changed
- `CORS_ORIGINS=*` now enables permissive **mirror mode**: the server reflects the request's `Origin` (and its requested headers) with credentials, so a browser client served from any origin can connect cross-origin. Previously `*` was refused and the server silently fell back to same-origin only. An empty `CORS_ORIGINS` (the default) and explicit origin lists are unchanged.

### Deprecated
- None.

### Fixed
- Cross-origin browser clients were blocked by a CORS preflight failure (`No 'Access-Control-Allow-Origin' header is present`) when talking to a store that did not list their exact origin; operators can now allow any origin with `CORS_ORIGINS=*`.

### Security
- Documented and pinned the invariant that makes `CORS_ORIGINS=*` mirror mode safe: both session cookies (`access_token`, `refresh_token`) are `SameSite=Strict`, so the browser withholds them on cross-site requests and the only cross-origin credential is the unforgeable `Authorization` bearer token. A new regression test fails CI if either cookie is ever downgraded to `SameSite=Lax`/`None`. Mirror mode remains explicit operator opt-in; the default stays same-origin only.

## [0.2.3] — 2026-06-09

### Added
- The Spark assistant renders its replies as full markdown, so example queries appear as syntax-highlighted code blocks in the chat instead of plain text (#78).

### Changed
- NL→SPARQL generation in the SPARQL editor now declares every prefix it uses (and the server fills in any the model still omits), parse-validates the result and repairs it once if it is invalid, auto-formats the query into the editor, and can refine the query already in the editor instead of always replacing it (#78).
- Spark chat replies are no longer cut off at a low output cap (raised from 700 to 2048 tokens) (#78).

### Deprecated
- None.

### Fixed
- Signing in to the same account from a second browser no longer logs you out of the first. Refresh-token reuse detection is now scoped to a single session ("token family") with a short rotation-grace window, so a concurrent-refresh race — e.g. browser session-restore reopening several tabs that refresh the same cookie at once — can no longer revoke every session (#78).
- Hard-refreshing or deep-linking the `/sparql` page now serves the web UI instead of the SPARQL endpoint's "Missing 'query' parameter" error (#78).
- Copy buttons now work when the app is served over plain HTTP on a LAN/IP. The async Clipboard API only exists in a secure context (HTTPS or `http://localhost`), so direct `navigator.clipboard.writeText` calls silently did nothing off localhost — first noticed as "I can no longer copy my API token", and the same for copy-IRI / copy-SPARQL / endpoint-URL / asset / inspector-value buttons. A shared `copyToClipboard` helper now falls back to a hidden-textarea `execCommand('copy')` in insecure contexts and reports success so the UI only flags "Copied!" when it actually copied (#82, #84).

### Security
- Refresh-token reuse/theft detection now revokes only the affected session family instead of every refresh token the user holds; genuine reuse of a fully-rotated chain still invalidates that session, and legacy pre-migration tokens (no family) still trigger a full revoke (#78).

## [0.2.2] — 2026-06-08

### Added
- An optional bundled LLM service (Ollama) for the platform's AI features: `docker compose --profile llm up` starts a local OpenAI-compatible model server and auto-pulls `qwen2.5:7b`; add `-f docker-compose.gpu.yml` to use an NVIDIA GPU. The triplestore points at it by default (`LLM_GATEWAY_URL=http://ollama:11434`); set `LLM_GATEWAY_URL`/`LLM_API_KEY` to use an external API instead.
- A default-banner picker for datasets and organisations: pick a built-in animated or gradient banner, or upload your own image, from the page editor. The bundled demo datasets now ship with a themed icon and a matching animated banner.
- The model registry now ships the standard RDF vocabularies (RDF, RDFS, OWL, XSD, SKOS, DCAT, DCTERMS, PROV-O, FOAF, ORG, QB, schema.org, SHACL, OWL-Time, VANN, VoID, GeoSPARQL, and the Open Triplestore vocabulary) seeded as public reference entries with browsable, queryable data out of the box (idempotent; opt out with `SEED_STANDARD_VOCABS=false`).

### Changed
- Dataset pages render the animated linked-data banner behind a liquid-glass header, consistent with organisation pages, and the landing hero and page banners use a lighter glass blur. The separate "Page settings" and "Edit metadata" actions are unified into one page editor.
- Standard-vocabulary seeding now parses each bundled TTL once (for kind detection and loading) instead of twice, halving the parse work on first-run/post-recovery seeding.

### Deprecated
- None.

### Fixed
- The triple store now auto-recovers from RocksDB corruption on startup (e.g. an unclean shutdown leaving `SST file is ahead of WALs`) instead of crash-looping: the corrupt files are quarantined (preserved, never deleted), the newest backup is restored if present, and seeds repopulate the rest. Opt out with `STORE_AUTO_RECOVER=false`.
- Corruption recovery no longer reports a reassuring "starting fresh" when only **encrypted** (`rdf.nq.gz.age`) backups exist — which the node cannot auto-decrypt (the age private key is held off-box). It now logs a prominent error with the quarantine path and manual-restore guidance, so an encrypted-backup deployment isn't silently brought up empty.
- Assigning a dataset graph the `model`/`vocabulary` role now copies the dataset's graphs into a published `1.0.0` version in the model registry, instead of creating an empty registry entry with no data.

### Security
- The `model`/`vocabulary` graph-role promotion now enforces the same `can_write_ontology` authorization on the destination registry entry that every other registry write applies. Previously, because the registry id is derived from the dataset's free-form, non-unique name, a user with write access to their own dataset could inject a published version into another owner's same-named registry model (cross-tenant integrity / stored data injection). Found and fixed in pre-release review; never shipped in a tagged release. Covered by new regression tests in the CI `security` gate.

## [0.2.1] — 2026-06-07

### Added
- Golden-standard conformance and high-complexity test suites spanning 11 standards across the engine, HTTP API, and web UI (#58).
- A performance-regression CI gate plus an opt-in pre-push hook, both checking against a committed benchmark baseline (this change).
- Tag-driven releases: pushing an annotated `vX.Y.Z` tag now publishes a GitHub Release and a GHCR Docker image (this change).
- A documented OSS versioning and release process — branch model, release and security-hotfix flows, and support policy (this change).

### Changed
- Multi-core `/sparql` query execution on the persistent backend via a subject-sharded parallel mirror — 8–11× faster on aggregate/COUNT-heavy queries (#60).
- Web UI overhaul: redesigned SPARQL editor, triple browser, and graph view ("liquid-glass" styling), unified model/vocabulary registry views, and expanded internationalisation (#64).

### Deprecated
- None.

### Fixed
- LDP root-container methods, relative-IRI request bodies, and CORS preflight headers (#59).
- SHACL Advanced-Features (SHACL-AF) fixes (#60).
- Authentication: give JWTs a unique `jti` so tokens minted in the same second no longer collide on the refresh-token unique index — fixes intermittent login failures after a password change or rapid re-login (#63).

### Security
- Fixed cross-tenant graph IDOR (read via add-dataset-graph, write via RML execute) (#60).
- Fixed three LOW-severity authentication findings from the 2026-06 follow-up audit (#61).
- Reject unsafe URL schemes in metadata to prevent stored XSS (#62).

## [0.2.0] — 2026-06-05

### Changed
- **Merged the Model and Vocabulary registries into a single Model Registry.** OWL/RDFS ontologies and SKOS vocabularies now live in one registry served under `/api/models`. Each entry carries a `kind` (`data-model` | `vocabulary`), auto-detected from the uploaded RDF on every version upload and surfaced as a badge with an ontology/vocabulary filter in the web UI.
- Publishing stamps version metadata by graph content — OWL `owl:versionIRI` / `owl:priorVersion` for ontologies and DCAT/PAV/SKOS (`dcat:hasVersion`, `pav:version`, `dcterms:issued`/`modified`, `dcterms:isReplacedBy`) for vocabularies — and applies both for mixed packages.
- Per-term dereference (`/api/models/{id}/term`) now also returns the enclosing `skos:ConceptScheme` for SKOS concepts.

### Removed
- The standalone Vocabulary registry: its `/api/vocabularies` endpoints and dedicated web-UI pages. Vocabularies are now managed in the unified Model Registry (pre-1.0 breaking change).

## [0.1.0] — 2026-06-03

First public, source-available release of **Open Triplestore**.

### Added
- RDF triple store built on [Oxigraph](https://github.com/oxigraph/oxigraph) with an
  [Axum](https://github.com/tokio-rs/axum) HTTP layer.
- **SPARQL 1.1** (SELECT/CONSTRUCT/ASK/DESCRIBE/UPDATE) and **SPARQL 1.2 / RDF-star**.
- **GeoSPARQL 1.1** (all 30 OGC requirements) via GEOS.
- **OWL 2** reasoning — RDFS, RL/EL/QL profiles natively, plus a DL external-reasoner bridge.
- **SHACL** validation (Core + Advanced), SHACL-on-write, and SHACL Compact Syntax.
- **LDP 1.0**, **RML** mapping, full-text search (Tantivy), and a **DCAT 2 / VoID / ADMS / PROV** catalogue at `/.well-known/void`.
- JWT + API-key authentication, RBAC, OAuth 2.0 / OIDC, optional SAML 2.0 SSO.
- Datasets, organisations/groups, model & vocabulary registries, dataset versioning, and binary asset management with extracted RDF metadata.
- A full-featured **Svelte** web UI, OpenAPI docs/Swagger UI, and a Docker image.
- Bundled **opengraph** engine layer (durable blank-node identity: RDFC-1.0 canonical labels + opt-in Skolemization).
- Optional, configurable **graph-viewer** deep-link integration (off by default; set `VITE_GRAPH_VIEWER_URL`) and a `form-manifest` endpoint for external form platforms.

### Notes
- Licensed under **AGPL-3.0 + Commons Clause** (source-available). See [`LICENSE`](LICENSE).

[Unreleased]: https://github.com/philipperenzen/open-triplestore/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/philipperenzen/open-triplestore/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/philipperenzen/open-triplestore/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/philipperenzen/open-triplestore/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/philipperenzen/open-triplestore/compare/v0.2.4...v0.3.0
[0.2.4]: https://github.com/philipperenzen/open-triplestore/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/philipperenzen/open-triplestore/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/philipperenzen/open-triplestore/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/philipperenzen/open-triplestore/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/philipperenzen/open-triplestore/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/philipperenzen/open-triplestore/releases/tag/v0.1.0
