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
- `CORS_ORIGINS=*` now enables permissive **mirror mode**: the server reflects the request's `Origin` (and its requested headers) with credentials, so a browser client served from any origin — e.g. the OTL viewer on `http://localhost:5190` — can connect cross-origin. Previously `*` was refused and the server silently fell back to same-origin only. An empty `CORS_ORIGINS` (the default) and explicit origin lists are unchanged.

### Deprecated
- None.

### Fixed
- Cross-origin browser clients (e.g. the OTL viewer) were blocked by a CORS preflight failure (`No 'Access-Control-Allow-Origin' header is present`) when talking to a store that did not list their exact origin; operators can now allow any origin with `CORS_ORIGINS=*`.

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

[Unreleased]: https://github.com/philipperenzen/open-triplestore/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/philipperenzen/open-triplestore/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/philipperenzen/open-triplestore/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/philipperenzen/open-triplestore/compare/v0.2.4...v0.3.0
[0.2.4]: https://github.com/philipperenzen/open-triplestore/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/philipperenzen/open-triplestore/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/philipperenzen/open-triplestore/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/philipperenzen/open-triplestore/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/philipperenzen/open-triplestore/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/philipperenzen/open-triplestore/releases/tag/v0.1.0
