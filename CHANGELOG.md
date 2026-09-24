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
- None.

### Changed
- None.

### Deprecated
- None.

### Removed
- None.

### Fixed
- None.

### Security
- None.

## [0.6.0] — 2026-09-24

Tagged after the fact from `main`: besides the changes of the original 0.6.0
release commit, this release contains everything merged to `main` up to the
vocabulary clean-up (#262–#267, #285, #288–#296), and its reference example is
a fictional bridge. Known issue: it ships `lru` 0.16 (RUSTSEC-2026-0253), which
0.7.0 fixes.

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
  `auth::policy` module, both knobs documented in `.env.example`).
  `OTS_GUEST_CAPABILITIES` selects from `write` / `create_datasets` /
  `api_tokens` / `publish` / `all` and defaults to read-only, applied as a clamp
  on every authentication path — a guest is equally limited arriving by session
  cookie, API token or OIDC token, and the clamp can only ever remove authority.
  `OTS_OIDC_SESSION_POLICY` decides what an OIDC access token may do: `session`
  (the default — reads and writes like an interactive session, but never mints
  API tokens), `scoped` (writes only when the token's `scope` grants it; issuers
  that namespace their scopes list the extra spellings in
  `OTS_OIDC_WRITE_SCOPES`) or `full` (the previous behaviour, kept as an escape
  hatch). `session` is the default deliberately: first-party apps commonly
  request only `openid profile email`, so enforcing scope by default would
  silently turn existing clients read-only, while blocking the token exchange
  closes the escalation (see Security) at no compatibility cost. An API token
  also no longer mints further API tokens. (#257)
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
- **SHACL Studio shape builder**: an add-property template menu, a
  relation/value/target legend, value-type hints, grouped advanced sections and
  a per-shape *Used by* (bindings plus live instance counts); the Studio
  overview's Datasets KPI opens the validation list. The round-trip engine is
  untouched. (#226)
- **Plugin accounts capability**: `ots-plugin-api` 0.2 adds
  `PluginContext::auth` (`PluginAuth`: bearer introspection + admin-gated
  users/organisations/LLM-stats overviews, enforced host-side), plus the new
  `plugins/accounts-dashboard` crate (feature `plugin-accounts-dashboard`,
  off by default): a deployment-wide accounts/entitlements/LLM-usage dashboard at
  `/ext/accounts-dashboard/ui`, merging the store's own AI-request log with
  an external LLM gateway's usage ledger (fail-soft). The Docker image gained
  a `CARGO_FEATURES` build arg to enable plugin features without patching.
- **Seed-bundle `[prefixes]` table**: a bundle may declare `prefix → namespace`
  mappings. Seeds form their own tier in the prefix registry, directly below the
  platform overlay and above the bundled prefix.cc snapshot — a deployment
  naming its own namespace outranks a community list — and the tier applies to
  forward lookup, reverse lookup and CURIE shrinking alike, so bundled datasets
  render prefixed names out of the box and their IRIs shrink back to CURIEs.
  Seeds are re-applied from the installed bundles on every start and never
  written to the on-disk cache; the first seed of a label wins, an override of
  a snapshot mapping is logged, and validation is unchanged. (#225, #258)
- **3D viewer sidebar grouping**: group elements by structure, location, format
  or type, with per-format badges (IFC / STL / CityJSON / glTF) instead of a
  bare "3D". Groups start closed, so regrouping 5k elements renders headers
  rather than 5k rows (7–21 ms measured, was seconds); *Structure* no longer
  vanishes during the two-phase feed load; the location lens is fed by
  server-side place inference (schema.org / ifcOWL address statements and
  `owl:sameAs` authorities such as BAG → a minted country/region/city path,
  never guessed from coordinates). The sidebar is responsive (`clamp()` width,
  `dvh` heights, coarse-pointer hit targets) and the group-by select follows
  the theme. (#233)
- **Volumetric WKT on the map**: `POLYHEDRALSURFACE Z` solids only ever appeared
  in the Cesium 3D-Tiles view — the viewer feed's reprojection went through the
  2-D geo crate, which cannot represent them, so the literals were silently
  dropped. The feed carries them in a dedicated `wkt3d` field (each `x y z`
  transformed on x/y, z kept as metres, structure untouched, malformed input
  fails closed) and the map builds real geometry from it — fan-triangulated
  faces in local metres about the footprint centroid, through the same entry
  contract models use, so fit, theming, suppression and picking need no special
  cases. (#233)
- **Spark widgets bind to retrieved rows**: ```` ```chart
  {"source":"query","x":"g","y":"n"} ```` and ```` ```map
  {"source":"query","wkt":"…"} ```` render the columns of the turn's last
  successful query exactly, so retrieved numbers can no longer be transcribed
  wrong — "chart the number of triples per named graph" used to produce a bar
  chart of 38 zeros (the model wrote `COUNT(?trip)` with `?trip` unbound and
  faithfully charted the result), and a 7B model corrupts digits when retyping
  30+ values. Column matching is name-based (a leading `?` tolerated) with
  auto-detection fallbacks; missing rows fail loudly. Chart labels show IRIs by
  their distinguishing tail segments instead of dozens of identical front-
  truncated ticks. (#233)
- **Spark grounding**: graphs of the datasets a question mentions take
  vocabulary and prompt slots before the positional truncation windows (on a
  200-graph instance the asked-about dataset never got a vocabulary block, so
  the model invented predicates and every query returned zero rows);
  vocabulary sampling is frequency-ordered and deeper (8 classes / 20
  predicates, was the first 6/12 in storage order — the BAG graph's
  construction-year predicate never made the prompt); the platform context
  lists each in-scope graph with its cached triple count and the dataset's
  **public** assets (the asset listing has no ACL, and a private filename in an
  anonymous prompt would be a disclosure), so "show me the IFC files" is
  answered from the inventory instead of by guessing graph shapes; inventories
  (datasets, graphs, files, services) are explicitly authoritative, graphs are
  for contents; a zero-row result, an all-zero aggregate and a parse error each
  get a targeted repair hint; the prompt teaches `/resource?iri=` entity links
  and carries worked query patterns; `model3d` specs accept an explicit
  `format` so extension-less asset download paths are loadable. Per-message and
  whole-conversation copy buttons. (#233)
- **Guest chat rate limit**: `LLM_RATE_LIMIT_ANON_PER_MIN` (default 5 per minute
  per IP) beside the signed-in `LLM_RATE_LIMIT_PER_MIN` (20); both are reported
  by `GET /api/llm/health` and shown in the chat UI (guest note and About
  panel), and the 429 message names the budget. (#233)
- **The vocabulary recommender accepts plain strings** — `{"terms": ["bridge"]}`
  beside the `{term, category}` objects — documented with a `curl` example in
  docs/vocabulary-search.md, in the OpenAPI description and in the Recommender
  tab. (#233)
- **`ots-geof:isClosed3d(geom)`** — a manifold closure test on the parsed
  geometry: a face set is watertight iff every undirected edge of its face
  rings is shared by exactly two faces (vertices matched on coordinates
  quantised to ~1e-9 of the geometry's magnitude, so WKT round-trips never
  split a shared vertex; unbound for face-less geometry). The seeded "3D
  solid closure" example shape used to count `((` face openers in the WKT
  text, so any open shell with four or more faces passed; it now calls the
  function, keeping the face-count guard as a fallback for builds without
  `geometry3d`, and the validation demo gains `OpenTank` — a five-face box
  missing one wall — as the case the heuristic waved through. (#267)
- **Labels follow the UI language.** `rdfs:label` / `rdfs:comment` and
  vocabulary titles were hardcoded to prefer English, so a Dutch UI showed
  English labels; every display surface (resource page, vocabulary cards,
  SPARQL completion tooltips, ontology header, model browser) now ranks
  exact locale tag > same primary subtag > English > untagged, and the viewer
  feed takes the client's language as `?lang=` so an element carrying
  `"Deur"@nl` and `"Door"@en` shows "Deur" to a Dutch reader (omitting the
  parameter keeps English-first). The light markup real vocabularies put in
  `rdfs:comment` / `skos:definition` — paragraphs, markdown links, bare URLs,
  `[[term]]` references — is rendered on the resource page and in the model
  browser instead of shown as a wall of text with brackets. (#265)
- **Shareable viewer links and reorderable inspector tabs.** The address bar
  mirrors the focused element as `?focus=<iri>` and the inspector has a
  copy-link action (the link carries no credentials — recipients still pass
  the dataset checks); inspector tabs can be reordered by dragging along the
  strip, with `Ctrl+Arrow` and menu items as the non-pointer equivalents;
  leaf parts (a door, a hinge) get *View in walkthrough*, which walks the
  ancestor building and spawns facing the part. (#265)
- **Spark shows its work.** A verbosity toggle in the chat header (eye icon,
  persisted) opens the retrieval trail by default and prints each query as it
  runs, so a turn that queries three times, fails once and retries shows
  exactly that, live; an explicit per-turn open/close still wins. Queries the
  model emits on one line are re-indented for the query card — layout only,
  token stream preserved, literals/IRIs/comments split out first — and the
  card's Run / copy / open-in-workspace use the formatted text, so what you
  see is what runs. IRIs in an answer are chips that open the resource page.
  (#288)
- **Resource hover cards wherever an IRI is shown.** Every IRI cell in a
  result table and every resource reference in a Spark answer shows a hover
  card — label in the UI language, up to three type chips, description, and
  how many facts the store holds — via one bounded, ACL-scoped
  `/api/browse/triples` fetch per IRI (350 ms show / 150 ms hide intent, a
  five-minute per-IRI cache, in-flight dedup); an IRI the store does not know
  renders an explicit "external link" card. (#295)
- **`LLM_CONTEXT_TOKENS`** declares the serving model's context window. When
  set, a turn budgets its prompt to window − `max_tokens` − margin: over
  budget, the oldest conversation turns are dropped first (the current
  question always survives), then graph-vocabulary blocks. Local runtimes
  (Ollama, vLLM, llama.cpp) truncate an over-long prompt silently from the
  top — deleting the execution protocol first — which from the outside looks
  like the assistant flipping mid-conversation from grounded answers into
  confident fabrication. Unset keeps the previous behaviour for large-context
  hosted APIs (and see the gateway discovery below). See docs/spark.md. (#295)
- **Spark re-surfaces question-matched API services at the prompt's tail.**
  Asked "is there an API service about cities?", a small model walked past a
  mid-prompt service literally named "Cities within a bounding box" and burned
  its rounds writing SPARQL. Up to three services whose name or description
  overlaps the question ride at the end of the system prompt with an
  instruction to answer with the `GET` path or an ```` ```api ```` widget
  before writing any SPARQL; no match, no section. The hint survives the
  over-budget vocabulary drop. (#295)
- **`LLM_CHAT_MODEL`** selects a model for Spark specifically (chat is the most
  demanding task; an instance running a small local model for NL→SPARQL often
  wants a stronger one here) and **`LLM_TIMEOUT_SECONDS`** raises the
  per-completion budget past the 120 s default a 20B+ model on local hardware
  needs. (#263)

### Changed
- Login accepts an internal-path `?next=` redirect (used by the OIDC
  authorize flow); absolute URLs are ignored (no open redirect).
- **The performance gate compares a change against its own merge base**, both
  benched in the same job (two passes a side, alternated, fastest median per
  benchmark), instead of against the stored baseline — runner hardware drift no
  longer reads as a regression. The default bar is 1.15: it was 1.25 against
  the stored baseline before this release, and briefly 1.10 within it, which
  produced four false regressions in one afternoon on benchmarks the changes
  could not reach. Benchmarks whose reference side is under 1 µs fall back to a
  1.35 floor, where a percentage bar is meaningless — a floor documented since
  #229 that had never run, because the baseline refresh silently dropped its
  keys until #260. Three benchmarks measured to be bimodal on these runners
  carry their own tolerances (`query_simple_lookup/100000` 1.45,
  `query_group_concat` 1.35, `query_alternative_path/10000` 1.5), and the
  baseline-refresh job measures the gated subset under the gate's own
  conditions. See [`docs/performance.md`](docs/performance.md). (#228, #229,
  #230, #231, #232, #260)
- **MSRV 1.88 → 1.94.1.** The coordinated aws-sdk/aws-smithy bump (`aws-sdk-s3`
  1.46 → 1.140, `aws-smithy-runtime` 1.7 → 1.12, `aws-credential-types` 1.3,
  the rest in lockstep — 1.3.0 cannot land alone) declares
  `rust-version = "1.94.1"`, so the package's own `rust-version` and the pinned
  builder images follow (Dockerfile and `.gitlab-ci.yml`: `rust:1.91-*` →
  `rust:1.94-*`). The plugin crates are untouched and still build on 1.88. The
  S3 client is handed an explicit ring-backed HTTP client instead of the SDK's
  default aws-lc-rs one — no CMake/C build, and one rustls crypto provider in
  the process, as before — and the never-imported `aws-config` is dropped.
  (#227)
- **Node 24.** Node 20 is end of life and receives no security patches; CI,
  GitLab CI and the production image's frontend stage were all on it. All six
  pins move to Node 24 LTS, `package.json` declares `engines.node >= 24` so a
  too-old Node warns instead of failing obscurely inside jsdom, and
  CONTRIBUTING / docs/windows.md say 24+. (#261)
- **Dependencies**: a batch of 19 updates including `age` 0.12
  (`Encryptor::with_recipients` takes an iterator and returns `Result`), `hmac`
  0.13 (with `sha1` 0.11 in lockstep — the digest 0.11 trait family, putting
  TOTP on the same generation as `sha2`/`hkdf`), `tower-http` 0.7, `geos` 11.1,
  `eslint-plugin-svelte` 3 (its ~430 new-rule hits on pre-existing code are
  staged as warnings pending triage; the lint gate is unchanged), `clap`,
  `uuid`, `svelte` 5.56, `vite` 8.2, `vitest` 4.1, `cesium` 1.143, the three
  `@codemirror` packages and `docker/login-action`; and `jsdom` 30, which the
  Node 24 move unblocked. TypeScript 7 is deliberately held: typescript-eslint
  does not yet support it. (#259, #261)
- **Dataset and organisation pages rebuilt around what a visitor came for.** The
  dataset page puts Files third instead of fourteenth: format badges (IFC / STL
  / CityJSON / RDF / image) from the viewer's own detector, size, date,
  restricted state, a plain download link to the streaming route, and **View
  in 3D** on model files, which mounts the viewer in the preview modal; four
  honest states (loading skeletons, a sign-in prompt, a retryable error, a real
  empty state) replace the one lie; a KPI strip (triples, graphs, files,
  versions, 3D elements) and a sticky section nav sit under the hero; Validation
  and Access are gated to the roles that can use them. The organisation page
  promotes datasets above the fold, finally shows the uploadable logo, omits KPI
  counts whose auth-gated fetch did not run rather than showing 0, shows
  members and groups to signed-in users only, gates manager controls, and
  confirms member removal; the organisations list no longer flashes "0
  organisations" before its first fetch. (#233)
- **Uploads inherit the dataset's visibility.** A file added to a public dataset
  is public by default (bulk import already did this); a manager can still opt
  one out. (#233)
- **Demo content and licences.** The Schependomlaan design model is replaced as
  the demo's headline IFC: its repository's LICENSE says CC BY 4.0, but the
  README records the grant the data owners actually gave — "for scientific and
  academic purposes" — so it cannot be redistributed here. The replacement is
  the Esplanades project (Maleva 18, Tallinn; CC BY 4.0 from the copyright
  holder via buildingSMART's community samples, 4× smaller and richer),
  anchored on the real building's OSM footprint. The other demo assets' terms
  are corrected to what upstream grants (Duplex Apartment from buildingSMART's
  CC BY 4.0 republication with the mandated credit; FZK-Haus / Smiley West
  pointing at KIT's actual terms; the Wikimedia landmark STLs recording
  `dcterms:license`/`creator`/`source` — four are CC BY 4.0 by Microsoft and
  carried no credit; the 3DBAG credit linking their copyright page). Demo
  geometry is traced from OSM (street line, park polygon, the WKT-Z solids on
  the real 68.2° street grid) and model orientation is measured. The demo
  refresh (content v13) purges the demo dataset's stale assets before
  reseeding — including the withdrawn 47 MB Schependomlaan.ifc, which the graph
  swap alone had left stored and publicly downloadable — refreshes the
  dataset's name and description, and seeds the five landmark STLs as
  first-class assets. `LICENSE` and `NOTICE` ship in the runtime image at
  `/app/`, and `NOTICE` is rewritten (web-ifc's MPL-2.0 source pointer,
  per-publisher vocabulary terms, IMBOR's real licences, the CLARIAH claim
  corrected, the demo-content section). (#233)
- **The bundled Ollama context window is 16k** (`OLLAMA_CONTEXT_LENGTH` in
  docker-compose): the 4096 default silently truncated Spark's grounded system
  prompt from the top, cutting the retrieval instructions themselves, so the
  model answered from dataset descriptions and never queried. (#233)
- **The reference example is a fictional bridge.** The SHACL/GeoSPARQL
  conformance oracle, the viewer-feed end-to-end test and the OGC GeoSPARQL
  round-trip run on `tests/fixtures/example-bridge/`: a made-up arch bridge
  with an English vocabulary (`https://example.org/def/`) and illustrative
  coordinates, in place of a real structure. The docs examples, Spark's prompt
  examples and the unit-test fixtures use fictional names as well.
- **The performance gate confirms before it fails.** A benchmark the four-pass
  screen flags is re-benched on both revisions before the gate fails, so a
  fluke on one of the benchmarks clears and a real regression repeats — three
  consecutive PRs had been failed on regressions their diffs could not reach
  (#266).
- **Spark no longer streams what it writes before its first retrieval.**
  Whatever the model writes before its first query cannot be grounded in
  data; streaming it painted a confident answer the next event had to wipe —
  the "it answers, then retracts and apologises" experience. Post-retrieval
  rounds still stream live. Result tables rendered into follow-up prompts get
  a total cap (`CHAT_TABLE_MAX_CHARS`): per-cell truncation alone let a wide
  50-row result reach several thousand tokens per round. (#295)
- **UI wording and layout.** The triple browser's "facets" are called
  "terms", matching the rail's *Terms in scope* heading; the Settings page is
  four hash-linkable tabs behind an identity header, on a two-column grid
  from 1024px instead of one 800px column with the token and danger-zone
  cards spanning the full width (#265). The walkthrough's two permanent cards
  are replaced by one quiet aim line (type · name · what a click does) and a
  detail card that opens only for a selected element the crosshair rests on
  for 1.6 s; models longer than ~120 m spawn over their own middle and let
  gravity settle the camera onto the deck, instead of 215 m past the end of a
  viaduct at ground level (#267).
- **Dependencies.** A batch supersedes 19 Dependabot PRs (#289), with the code
  migrations they require: `rand` 0.10 (`thread_rng()` → `rng()`; still the
  ChaCha12 CSPRNG, so token, TOTP and JWT-secret generation is unchanged) and
  `symphonia` 0.6, plus `base64` 0.23, `wkt` 0.14, `infer` 0.22, `zip` 8,
  `parry3d-f64` 0.30 and the semver-compatible Rust and npm updates; `ipnet`
  2.12.1 (#285). The never-imported `utoipa-swagger-ui` crate is gone (#291) —
  the OpenAPI document is still served at `/api-docs/openapi.json` and the
  interactive UI is the frontend's own page — which also removes the duplicate
  `axum` 0.8 and `zip` 3 from the tree.

### Deprecated
- None.

### Removed
- **The invented vocabulary term sets.** The seeded registry, the bundled
  `vocab/` files and the term-lookup map lose the four hand-authored files
  that had no authoritative source to copy from — `bag` (a "3DBAG
  Vocabulary" excerpt under `https://data.3dbag.nl/def/`) and `otl` (an
  "Object Type Library" excerpt under `https://example.org/vocab/def/`),
  whose IRIs were invented outright; the IMBOR `def/` excerpt version
  (`https://data.crow.nl/imbor/def/`, a namespace that serves no RDF; the
  full IMBOR 2025 `term/` vocabulary stays); and the alignment bridge that
  only existed to link them — together with the demo dataset's `assets`
  graph built on top of them. The VoID vocabulary file goes too: its
  canonical Turtle is no longer published at a stable URL, so only the curated
  `void` prefix entry remains. Existing installs keep whatever the registry
  already holds; only the seed no longer provides these. (`f20a87b`)

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
- **Basemap suppression, model orientation and ground line.** Suppression hides
  basemap buildings with MapLibre's viewport-independent `distance` filter and
  keeps a feature-id set only as the fallback for engines without it — an id
  set collected at one zoom hid unrelated buildings at other zooms, since
  planetiler feature ids are not stable across zoom levels; suppression resets
  during entry rebuilds (the phase-2 feed swap left whole blocks hidden for
  seconds with no models standing in) and footprints turn with a model's
  heading. Model orientation is measured rather than guessed —
  `ots:modelHeading` (the bearing of the model's +X axis) flows RDF → feed →
  placement matrix, landmark bearings are taken from real OSM footprints (the
  Empire State Building was 29° off the Manhattan grid), the Dragon Bridge's
  wrong `upAxis` no longer lays it on its side, and
  `IfcGeometricRepresentationContext.TrueNorth` is deliberately not applied
  (web-ifc already resolves placements, so it double-rotates). IFC buildings
  stand on their real ground line: web-ifc recentres a model arbitrarily in Y
  and `normalise()` rested the bounding-box minimum on the ground, so a
  building with foundations below grade stood lifted by that depth; the parse
  records the file's own elevation zero, placement rests the model on it when
  plausible, and the map clips geometry below the basemap plane (the modal and
  walkthrough keep the below-grade parts visible). (#233)
- **Dataset files were unreachable logged-out.** `GET /api/datasets/:id/assets`
  was mounted behind `require_auth`, so every anonymous visitor got a 401,
  which the dataset page swallowed and rendered as "No assets yet" — a public
  dataset's IFC and STL originals were invisible to exactly the audience the
  demo exists for. The list sits behind `optional_auth` like every other public
  dataset surface and filters to public assets for anonymous callers; download
  and metadata reject non-public assets outright; write handlers still require
  a real user. (#233)
- **Walkthrough jump and crouch work everywhere**: a missed floor raycast froze
  gravity — `grounded` never became true, which silently disabled both — and
  now falls back to the scene ground plane. (#233)
- **Vocabularies tab, relevance and dark mode.** The Vocabularies tab died on a
  Svelte `each_key_duplicate` (the catalogue can legitimately return two
  entries with the same prefix+namespace — a platform copy and a LOV record)
  and snapped back to the Terms tab; relevance was a bare 70px bar that made
  results incomparable and vanished on dark surfaces, and is now the normalised
  score as a number beside a small meter; the search inputs hardcoded a white
  background under the theme's near-white text, so typed text was invisible in
  dark mode. (#233)
- **Embedded 3D answers no longer hijack page scrolling**: wheel-zoom in the chat
  and inspector viewers is gated behind a click (OrbitControls' always-on wheel
  handler took over the moment the cursor crossed a 3D answer), and a model
  that fails to load is named in the console instead of rendering a silent
  wireframe cube. (#233)
- **The query cache deep-copied results it then discarded**: `QueryCache::put`
  decomposed every solution into a row vector *while* pulling, before it could
  know whether the result fit under the cap. Every SELECT over the 10 000-row
  cap paid ~10 001 throwaway row allocations and the matching term clones for
  nothing. Solutions are now buffered untouched and decomposed only in the
  branch that actually caches; variable lookup also stops being quadratic in
  projection width.
- The default-features build (`cargo check`/`cargo test` with no flags) broke
  on a `text-search`-gated `AtomicBool` import used by an ungated field, and
  on an ungated `Term::Triple` match arm in the SPARQL-functions conformance
  test. Both are feature-gated correctly now; CI's explicit feature list had
  masked them.
- **Full-text search returned nothing.** The index reader was never reloaded
  after a commit, so the request that had just rebuilt the index searched its
  emptied state; and the `text:search` magic-property expansion kept the `(?s
  ?score)` tuple in front of the generated `VALUES`, which SPARQL parsed as an
  RDF collection and matched nothing. Alongside: `ft:search` (documented,
  unimplemented) works in both spellings and their IRI forms; the
  `CONTAINS`/`STRSTARTS` push-down silently dropped correct rows (token
  matching is not a superset of substring matching — `CONTAINS "bridge"`
  missed "drawbridge" — and filters were hoisted out of
  `OPTIONAL`/`UNION`/`NOT EXISTS`), so candidates now come from a regex over
  an un-tokenised field and the rewrite is skipped whenever supersetness
  cannot be proven; `REGEX` is no longer pushed down at all (SPARQL uses XPath
  regexes, the index does not); the index is built at boot after the seed
  chain (a server booting with a populated store answered "no results" until
  the first write); and rebuilds are serialised. Spark's queries go through
  the same preprocessing. See docs/full-text-search.md. (#293)
- **Spark grounds on evidence and bounds each round.** Vocabulary sampling
  was all-or-nothing under one 3 s budget, so one slow graph discarded every
  sample — including ones that returned in milliseconds — and nothing was
  cached, so it failed identically every turn; each graph is now sampled
  against a shared deadline and kept as it lands. Slot selection ignored
  graph size, so a few huge derived layers displaced the small hand-authored
  graphs questions are about; non-priority slots fill smallest-first, and an
  uncounted graph counts as large rather than small. Identifier-shaped terms
  in the question are looked up in the text index to find the graphs that
  hold them. A retrieval round no longer inherits the endpoint's whole SPARQL
  timeout (one hallucinated pattern spent 120 s of a 222 s turn); it is
  bounded per round (`LLM_CHAT_QUERY_MAX_SECS`) and the bound that fired is
  reported so the model repairs against it. (#292)
- **Spark verifies model-written SPARQL before running or showing it.**
  Solution modifiers written inside the `WHERE` block (`} LIMIT 50 }`, which
  the parser reports as a baffling "expected OPTIONAL") are hoisted, and an
  IRI that differs from a sampled term by letter case alone is corrected
  (small models tidy `local_name` into camelCase, which parses and matches
  nothing). (#292, #295)
- **Spark retrieval works with weaker models.** A reply that writes its query
  in a ```` ```sparql ```` fence instead of emitting the `SPARQL:` directive is
  executed as long as no round has succeeded this turn (a fenced query in a
  final answer stays the query card the user is meant to see) — both a 1.5B
  and a 7.6B model, live, answered a failed round with prose plus a correct
  fenced query and then answered from memory. A turn that asks for no query
  at all gets one short, explicit nudge to query first; the model may decline
  (conceptual questions need no data) and its original answer is kept when it
  does, so the nudge can only add retrieval. (#263)
- **A failed Spark turn keeps its retrieval trail and offers a retry.** A
  transport error mid-turn replaced the whole assistant bubble with the error
  text and discarded the query chips the user had just watched run. The error
  bubble keeps the trail (those rounds really ran; only the answer was lost)
  and gains *Try again*, which re-runs the question in place with the failed
  pair removed first, so the replayed conversation is identical to a first
  attempt. (#295)
- **An empty Spark retrieval is "could not find", never "does not exist".**
  With its rounds exhausted the final-answer instruction said "answer as well
  as you can", and a small model turned three empty retrievals into "there
  are no cities in this dataset" — right past a platform-context section
  listing a cities API service. Both exhausted-round follow-ups now say that
  empty or failed retrieval means *could not find*, and point at the
  Datasets, API Services and Files sections before anything is declared
  absent. (#295)
- **Spark API blocks with parameters are runnable, and chart labels
  readable.** The endpoint parser stopped at the first space, so a GeoSPARQL
  call whose query string carries WKT (`?from=POLYGON((3.5 51, …))`) rendered
  as inert text while the same endpoint without parameters was runnable.
  Dense category axes draw every Nth label, steeper when dense, instead of a
  smear no label could be read in; every bar keeps its full label on hover.
  (#288)
- **The Model Registry page renders again.** `GET /api/models` returned one
  record per combination of a model's optional properties, so a stray second
  `dct:created` on 44 entries produced 89 records for 45 models with repeated
  ids — and the page, keyed by id, threw Svelte's `each_key_duplicate`, never
  cleared `loading`, and sat on "Loading models…" forever. One record per
  model; the read path is repaired rather than assuming single-valued
  optionals. (#296)
- **The triple browser works for blank-node resources.** Opening a blank
  node (`/browse?subject=_:abc`, the resource page's own deep link) compiled
  to `FILTER(?s = <_:abc>)` — a relative IRI — and the whole request was a
  400 SPARQL parse error; a blank node as an *object* filter returned 200
  with an empty page. Neither is expressible in SPARQL (a `_:x` is a fresh
  variable, `<_:x>` a parse error, `STR()` of a blank node a type error), so a
  blank-node-pinned request is answered from the quad index over the same
  ACL-resolved graph set the query path uses — a node in a graph the caller
  cannot read stays invisible — with the remaining filters applied in Rust;
  the `q` mini-language is parsed to an AST both paths evaluate identically,
  filters the scan cannot anchor on are a readable 400, and rows are ordered
  before paging. (#290)
- **The facet rail no longer hammers `/api/prefixes/reverse` into a 429.**
  Reverse-prefix results were cached by *prefix*, so a namespace whose label
  was already taken (`https://w3id.org/props#` → "w3id") was dropped on the
  floor, the caller re-requested it, and every success re-ran the reactive
  block — an infinite loop stopped only by the rate limiter (~15 identical
  requests per URL per page load). Results are keyed by namespace, concurrent
  callers share one in-flight request, and only definitive misses (404, or
  200 with no prefix) are cached — a 429 or a network blip is not, since that
  would blank the label for the 24 h negative TTL. (#290)
- **The command palette suggests real datasets.** Its dataset suggestions
  were a hardcoded array ("public-data", "linked-open-data", "geo-data");
  they come from `/api/datasets` now (scoped server-side, loaded once on the
  first keystroke, matched by the shared accent-folding filter on id and
  name), recent searches are filtered by the query too, and the suggestion
  panel is no longer clipped to an 11px sliver by the modal's overflow.
  (#293)
- **3D viewer: models measured at the wrong scale.** Since three r177
  `updateWorldMatrix()` skips nodes whose local matrix did not change, so
  after `normalise()` scaled a merged IFC model's group every `Box3`
  measurement read the meshes' pre-scale world matrices: the modal's
  post-load fit parked the camera ~40× too far out (the "model does not
  load" reports for Esplanades and Smiley West — an empty scene with the
  building a speck), models floated above their ground plane, and the map's
  shadow disc and footprint suppression were sized at raw scale. Volumetric
  WKT meshes are drawn double-sided: GeoSPARQL puts no winding requirement on
  polyhedral rings, and an extruded solid whose walls wound the other way
  rendered as its roof lying flat on the map. (#267)
- **Walkthrough is offered for every IFC model.** The action was gated on
  the exporter having nested its BOT containment, so an `IfcBridge` and its
  roads at the root of an IFC 4.3 infrastructure file offered no way in;
  linking an IFC file is the only requirement there ever was. (#267)
- **An `?element=` embed opens on the element, not the world.** The camera
  was aimed only after the full feed resolved (14k elements: tens of seconds
  at the dataset's whole extent, which for a globe-spanning demo is the
  globe), and the map placed — downloaded and parsed — a model per located
  element, so a one-building embed pulled every other building with it. The
  embed frames from the located feed, fits its first view to the link's
  subject, and shows the element, the ancestors it inherits geometry from and
  its own sub-elements. The map also observes its container: MapLibre sizes
  its canvas at construction and listens for window resizes only, so an
  iframe laid out afterwards kept a 400×300 canvas painted into one corner.
  (#267)
- **Viewer framing, the term filter, and a few dead controls.** The
  bounding-sphere fit sized by the largest axis left wide, shallow footprints
  filling well under half the frame; the fit is exact against the box's
  eight corners and the viewport aspect, zoom follows the cursor, near/far
  track the model so close-ups no longer clip, and a busy indicator replaces
  the empty grid during long IFC loads. The triple browser's term filter did
  nothing — the reactive statements read the query only inside a helper, so
  Svelte never tracked it. The admin token scope is not rendered for
  non-admins (the server rejects it with 403). Floating surfaces (the Spark
  info popover, the memory modal) get an opaque background instead of showing
  the chat through. (#265)
- **The dataset map is usable on a phone.** The layers/legend panel was a
  permanent ~170×220 overlay covering a third of a phone-width map; below
  700px it collapses behind one 40px button and opens as a two-column sheet
  anchored under it (a bottom-anchored sheet opened off-screen on a short
  phone); the basemap toggle and the layers button meet the 44px touch
  target; the page title takes its own row instead of wrapping to three lines
  in a 50px column; the phone breakpoint moves 620 → 700px so page and map
  chrome switch together. (#262)
- **Streamed SPARQL results are written in 64 KiB chunks.** The result
  serializers write in very small pieces (the JSON writer as little as one
  byte per call) and each became a heap allocation, a cross-thread channel
  handoff and its own HTTP chunk: a 500-row `SELECT` produced ~115k chunks
  averaging one byte, making JSON ~95× slower to serialise than the same rows
  as CSV. (#264)

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
- **OIDC access tokens were full-power credentials that could be exchanged for
  permanent ones.** The `scope` claim was minted into every token and never
  read back, and `write_access` was hardcoded true, so a token a user consented
  to for `openid profile email` could read and write like a session — and
  could be exchanged at `POST /api/auth/tokens` for a long-lived `ots_` token
  with write (or admin) scope, turning a read-only delegation into permanent
  account access. The default `OTS_OIDC_SESSION_POLICY=session` blocks the
  exchange while keeping existing clients working; `scoped` enforces the
  scope (see Added). An API token no longer mints further API tokens either.
  (#257)
- **Full-text search hits are filtered by the caller's read scope.** The text
  index stored no graph IRI, and an expanded `text:search` can be nothing but
  a `VALUES` clause — no triple pattern for the query's `FROM` scoping to
  constrain — so a guest could enumerate subjects whose literals matched in
  private graphs. Hits are filtered by the caller's readable graphs inside the
  index, and Spark's evidence lookup passes the same scope rather than
  filtering afterwards (hits from unreadable graphs no longer consume its
  evidence slots either). (#293, #294)
- **`rand` 0.10 clears RUSTSEC-2026-0097** (an unsoundness in `rand` 0.8),
  as part of the dependency batch (#289).

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
- **Schependomlaan demo** replaces the bridge example: the canonical open Dutch
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
- Reference-example conformance fixtures (a bridge; now `tests/fixtures/example-bridge/`) and an
  oracle (now `tests/example_bridge_conformance.rs`) encoding a GeoSPARQL +
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
  queries, SHACL-SPARQL constraints and rules (e.g. `ex:distanceMetres`). Bodies are
  evaluated against a fresh in-memory store, fully supporting expression-style functions.
- **Viewer feed** endpoint `GET /api/datasets/:id/viewer-feed`: per-element geometry +
  3D-file references resolved from the BOT/OMG/FOG/GeoSPARQL layering — labels, types,
  parent topology, IFC GlobalId, glTF/IFC/other file URLs, and geometry reprojected to
  EPSG:4326 and EPSG:3857 server-side. Anonymous access works for public datasets.
- **Compliance as data**: every official dataset validation run now also persists its
  `sh:ValidationReport` as RDF into `urn:system:reports:dataset:{id}` (replaced per run),
  so dashboards can query failures via SPARQL; severity rollup stays on the run rows.
- **3D & Map Viewer demo dataset** (`viewer-3d-demo`) in the standards demo seed: the
  reference bridge (EPSG:28992, IFC/glTF refs) plus real Wikidata landmarks (CC0 —
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
  **Projected-CRS WKT (e.g. EPSG:28992 RD New) is now reprojected
  client-side before plotting** — previously raw map previews plotted projected
  coordinates as lon/lat. Dark mode is supported across all maps and 3D scenes.
- **Official conformance suites in CI**: the W3C SHACL core test suite and the OGC
  GeoSPARQL 1.1 SHACL validator (+ its valid/invalid example corpus) are vendored under
  `tests/fixtures/{w3c-shacl,ogc-geosparql}/` and run with a two-way ratchet (unlisted
  tests must pass, listed known-failures must still fail). Scorecards:
  W3C core 46 pass / 52 known-fail / 15 aux skips; OGC examples 44/48 matching, and the
  reference bridge dataset round-trips through the official GeoSPARQL validator. See
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
