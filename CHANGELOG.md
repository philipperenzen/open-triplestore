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
- **A switch between prefixed names and whole IRIs**, in the Triple Browser's
  toolbar beside SPARQL. The prefixed name (`rdf:type`) stays the default
  because it is what makes a table of triples readable, but it is a lossy
  rendering of the identifier you came to read, and the only way to see the
  IRI was to hover a tooltip — no use from a keyboard, and nothing at all on a
  touch screen. The preference is app-wide and persisted, so it is asked once
  and answered everywhere a term is rendered: the triple table, the graph, the
  resource page, the inspector panels. Literals and blank nodes have no
  prefixed form and are unaffected, and the copy controls still copy the IRI in
  either mode.
- **Copy the IRI of any term in the Triple Browser's table.** Subject,
  predicate, object and graph each carry the same copy control, and it copies
  the IRI itself rather than the prefixed form the cell displays — a literal
  by its value, a blank node as `_:label`, nothing for the default graph,
  which has none. The control is keyboard reachable and named for what it
  copies; before this the predicate and graph cells had no control at all and
  the other two were invisible and unfocusable. A copy that fails now says
  so, everywhere it can fail: the Clipboard API needs a secure context and a
  focused document, so it fails routinely on a store reached over plain HTTP
  on a LAN, and every caller had been testing the result and doing nothing
  with it.
- **A columnar copy with its own SPARQL evaluator** (`opengraph::columnar`,
  on by default; `OTS_COLUMNAR_QUERY=off`): the in-memory mirror keeps a
  third copy — a term dictionary and three sorted permutations of the quads
  as flat arrays of ids, about 48 bytes a quad — and answers from it, after
  the shards and in place of the full copy, the query shapes its evaluator
  implements exactly. A `LIMIT` now stops the scan rather than trimming a
  materialised result. Everything else is declined before any data is
  touched and served as before, so answers are unchanged; two parity suites
  hold the evaluator to the engine, one of them built from an adversarial
  review of where it could silently differ. A new telemetry exit,
  `columnar`. Joins, `OPTIONAL`, `MINUS`, subqueries and limited lookups
  are 44–79 % faster; the before-and-after table is in docs/performance.md,
  "The columnar copy".
- **Replication over the change log** (`OTS_REPLICATION_ROLE=leader|follower`):
  a follower tails the leader's change log with a cursor and applies rows
  as deltas, fetches a graph whole when a row says only that it changed,
  resynchronises on a store-scoped row or an epoch change, and keeps its
  store read-only (writes answer `503`). Configurable temperature
  (`cold` / `warm` / `hot` — how often it asks) and scope (all graphs, a
  list of graphs, or the leader's datasets). `GET /api/replication/status`
  (public, beside `/livez`) and `GET /api/replication/manifest` (admin).
  A leader naming `OTS_REPLICATION_SYNC_FOLLOWERS` is **synchronous**: a
  write returns once the required followers have applied it, or after a
  timeout, degraded and visibly so (`X-Replication-Ack: sync|degraded` on
  the write routes, `sync` in the status), recovering by itself when a
  follower catches up. `GET /api/admin/changes?wait_ms=` long-polls, which
  is how a hot follower keeps its lag to a round trip. The identity
  database is shipped whole: `GET /api/replication/identity` serves a
  consistent SQLite snapshot, the manifest carries SQLite's change counter,
  and a follower applies a changed snapshot in place under its open
  connections. **Consensus** (`OTS_REPLICATION_ROLE=cluster`, three or more
  members): Raft, through `openraft`, elects the leader and fences the old
  one; the elected member leads, the rest follow hot and acknowledge a
  majority; failover is automatic. Asset shipping: see
  docs/operations.md, "Replication".
- **The 9M-quad SHACL measurement** (`tests/scale_shacl_9m.rs`, which runs
  at 20 000 assets in the ordinary suite and at 1M with `OTS_SCALE_ASSETS`):
  whole-dataset validation of 1M assets against six
  property shapes on a persistent store takes 6.3 s with the accelerator
  on and 13.5 s in the shipped 4 GB container (was 118 s before the engine
  rebuild); the figures and what they settle are in docs/performance.md.
- **Per-quad change capture with a durable cursor** (off by default;
  `OTS_CHANGE_CAPTURE=on` turns it on, and a replication leader keeps it on
  regardless): every write records one row per graph it touched — the
  net delta as N-Quads, exact counts above the payload cap, or an honest
  `unknown` — with a dense sequence number in commit order, an epoch per
  store lineage, open-time repair of rows a crash left pending, and a
  count check that closes the chain of any graph changed behind the
  log's back. `GET /api/admin/changes?after=&limit=&graph=`,
  `GET /api/admin/changes/status`, `PUT`/`DELETE`
  `/api/admin/changes/cursors/{name}` (admin). Retention keeps every row
  above the lowest live cursor. The producer side of replication and
  history; see docs/versioning.md "Change log".
- **Workload telemetry** (`GET /api/admin/telemetry`, admin): which exit of
  the query path answered each query — result cache, count index, mirror
  shards, full copy, engine — with latency percentiles split by an
  *analytical* bit computed once per uncached evaluation and stamped on the
  cache entry, so a hit inherits it without a parse; every SHACL run's data
  source, run index, quads, duration and caller (`dataset`, `gate`,
  `pipeline`, `engine`), also carried in the report as `metrics` and
  stored on the run row; and the inter-write gap histogram that decides
  whether the in-memory mirror can ever publish. Fixed-size rings, nothing
  persisted but the run-row columns. These are the inputs to the
  analytical-layer go/no-go thresholds in
  docs/notes/analytical-mirror-design.md §1.4. See docs/performance.md.
- **A replication example you can start in two commands**:
  `docker-compose.replication.yml` is a stand-alone leader-and-hot-follower
  stack — one shared `JWT_SECRET`, separate volumes, nothing else — with
  the walkthrough in docs/operations.md, "Try it": leader up, mint the
  follower's token, follower up, a write on the leader appearing on the
  follower within a round trip, a write on the follower refused with 503.
  `.env.example` documents the follower's knobs.
- **A Node status page in the web UI** (`/admin/operations`, admin): this
  node's replication state in one word with what it means for reads and
  writes, its lag and last catch-up; which exit answered each query, each
  exit explained in plain words, with exact counts and sampled latencies;
  SHACL validations by caller and data source; the inter-write gap
  histogram; and the change log — off with the reason and the switch, or
  capturing with its rows, retention and every consumer's cursor. Refreshes
  every five seconds while open. English and Dutch.
- **IFC lift depth.** The importer keeps its flat BOT / `props:` contract
  untouched (pinned as exact triples) and emits, beside it: the IFC 4.3
  facility spine (`IfcBridge`, `IfcRoad`, … as `bot:Zone` plus the lift's own
  classes, with only the entities new in 4.3 typed under the lift namespace
  so IFC4 shapes keep matching 4.3 models); quantity sets and every property
  kind as nodes carrying `qudt:numericValue` / `qudt:hasUnit` from a QUDT 2.1
  table that never guesses (an unmapped unit is a label and a count);
  classifications as SKOS concepts and schemes, with the
  `props:ifcClassification` / `props:ifcMaterial` literals the IDS importer
  had targeted while nothing emitted them; the NEN 2660-2 relation family
  beside every BOT edge; and the map conversion as a CRS-qualified point,
  read and never applied. The vocabulary lives at `{base_url}/ns/ifc-lift#`
  and ships as the `ifc-lift` seed bundle, which a test holds the emitter to.
  See docs/geo-3d-platform.md §9.
- **`{base_url}` in seed bundles.** A manifest's model namespace, graph IRIs,
  shape bindings and payload text may name `{base_url}`, expanded at seed
  time to the deployment's base URL, so a bundle can mint IRIs under the
  instance that serves them.
- **LDES retention policies** (`ldes:fullLogDuration`, `ldes:versionAmount`,
  `ldes:versionDuration`, `ldes:versionDeleteDuration`, `ldes:startingFrom`),
  declared with `PUT /api/datasets/:id/ldes` as a `retention` object and
  published on the root node as an IRI described on every page. Fragments are
  frozen once full, so a page served as immutable only ever shrinks under
  retention and never renumbers; a frozen page emptied by the policy answers
  `410 Gone` with `tree:view` and every relation pointing past it, and full
  pages carry `<node> ldes:immutable true`. The sync client treats `410` as an
  empty page, keeps the publisher's policy in its report and warns when its
  bookmark predates the publisher's window. `tests/ldes_conformance.rs` holds
  the LDES / Server Primer / TREE rules as one assertion per clause — a probe
  of the pre-existing surface found everything green except the
  `ldes:immutable` triple. See docs/ldes.md.
- **A W3C DQV quality bundle** (`examples/seed-bundles/dqv-quality`, on by
  default, `SEED_DQV_QUALITY=false` to skip): a profile of quality categories,
  dimensions and metrics — DQV's Note ships one dimension and constrains
  nothing — and six SHACL shapes saying what a well-formed
  `dqv:QualityMeasurement` is, two of them SHACL-SPARQL because core cannot
  check that a value's datatype is the one its metric declares or that
  `dqv:computedOn` / `dqv:hasQualityMeasurement` agree. The sample plants one
  violation per shape; the metric IRIs are the ones a validation-run emitter
  would write.
- **SHACL → IDS export**, the inverse of the existing importer:
  `GET /api/shacl/exporters` and `POST /api/shacl/export/ids` (Turtle in, the
  report by default, the bare document with `?raw=true`). The response always
  carries a `losses` list, because IDS can express only a facet kind, a
  cardinality and one value restriction: everything outside that — `sh:nodeKind`,
  the logical operators, `sh:sparql`, multiplicities other than 0 and 1, and any
  target that is not class-based — is named rather than silently dropped, and a
  shape graph from which nothing can be expressed is a 422 instead of an empty
  document. Tests pin an import → export → import fixpoint. See docs/shacl.md.
- **SHACL-AF completed — constraint components, spec pre-binding, rule
  order/condition — and strict SHACLC.** A shapes graph can declare its own
  constraint components (`sh:ConstraintComponent` + `sh:parameter` + an
  `sh:validator` / `sh:nodeValidator` / `sh:propertyValidator` with `sh:ask`
  or `sh:select`; `$PATH`, `$value`, `{$param}` message templates). SPARQL
  constraints are evaluated with SHACL §5.3 pre-binding — `$this` reaches
  nested groups and `UNION` branches, `bound($this)` is true — and the
  features the specification forbids under pre-binding (`MINUS`, `VALUES`,
  `SERVICE`, a nested `SELECT` not projecting `$this`, `AS $this`) make the
  shapes graph fail to load instead of silently passing; `$PATH` works in
  `sh:sparql` on property shapes and `sh:prefixes` follows `owl:imports`.
  Rules honour `sh:order`, `sh:condition` and `sh:deactivated`, and a triple
  rule keeps the datatype of a literal object (`sh:object true` used to be
  inserted as the string `"true"`). The SHACLC parser is strict by default:
  input it does not recognise is a 400 naming the position, never an emptied
  shapes graph; `?lenient=true` on `PUT …/shapes` and `POST /api/shaclc/parse`
  restores the old drop-what-you-cannot-parse behaviour. The W3C SHACL test
  suite's `sparql/` section is vendored and ratcheted: 22 of 23 pass (119 of
  121 with `core`). See docs/shacl.md and docs/conformance/shacl.md.
- **NEN 2660-2 relation profile and consistency shapes**
  (`examples/seed-bundles/nen2660-relations`). The part-whole, containment,
  constitution and connection relations with the characteristics OWL can
  carry — transitivity on proper parthood (`hasPart` and its functional /
  technical sub-relations) only, never on `contains`, `consistsOf` or
  `connects*` — and the ones it cannot as SHACL-SPARQL shapes: a
  decomposition is acyclic and irreflexive, containment and connection are
  irreflexive, a part's geometry lies within its whole's and is an RCC8
  proper part of it, a contained object lies within its region (GeoSPARQL,
  computed at validation time, never asserted). Ships a sample with planted
  violations; the shapes and sample run in CI, the NEN 2660-2 RDFS file is
  fetched. The modelling styleguide gains a part-whole section (Keet et al.).
- **OWL 2 RL: composite `owl:hasKey`, the Table 8 datatype rules, and an
  honest rule inventory.** `prp-key` fires for keys of any length (only
  single-property keys used to); `dt-type1` declares the datatype map and
  `dt-not-type` makes an ill-typed literal (`"abc"^^xsd:integer`) an
  inconsistency; the engine lists the 63 rules it runs and the 15 it does
  not, with reasons, and `tests/owl2_rl_conformance.rs` pins the two lists
  against the specification's 78. Per-dataset materialisation is
  incremental for additive writes: a Graph Store `POST` extends the
  entailment graph (the rules re-run on top of the existing consequences)
  instead of clearing and rebuilding it, and a write that changed nothing
  triggers no run. See docs/owl2-rl.md.
- **Identity policy — what reasoning does with `owl:sameAs`.** Per
  organisation (inherited by the datasets it owns) and per dataset:
  `sameas-off` (the OWL 2 RL equality rules never run), `sameas-narrow` (the
  built-in default: they run over the dataset's own graphs, `linkset`-role
  graphs are not premises, so a cross-source sameAs never leaks attributes
  between representations) or `sameas-full` (the previous behaviour).
  `GET/PUT/DELETE /api/datasets/:id/identity` and
  `…/api/organisations/:id/identity`; `GET …/entailment` reports the policy
  in force, its source and the effective reasoning sources; `PUT
  …/entailment` accepts `identity`. Typed correspondences
  (`prov:specializationOf`, `prov:alternateOf`, `skos:*Match`,
  `rdfs:seeAlso`) never feed the equality rules. The OWL 2 DL reasoner's RL
  phase now reads the caller's scope (it ran over the default graph only).
  See docs/reasoning.md.
- **Official W3C SPARQL 1.1 test suite in CI.** The query and update
  sections of `w3c/rdf-tests` (485 entries) are vendored under
  `tests/fixtures/w3c-sparql11/` and run manifest-driven through the store
  by `tests/w3c_sparql11_manifests.rs`: 475 pass, 10 known failures (all
  oxigraph 0.5 evaluator behaviours, listed in docs/conformance/sparql11.md),
  two-way ratchet with a pass floor. The generated conformance table now
  scores three vendored corpora.
- **OTL-scale benchmark.** `examples/scale_otl.rs` generates asset-shaped
  data at scale and measures load, six query shapes (cache off), SHACL over
  every asset and a concurrent writers-plus-readers phase;
  `scripts/scale_compare_fuseki.sh` loads the same data into Fuseki. Results
  and the write-side finding it produced are in docs/performance.md.
- **Federated access control.** With `OTS_REMOTE_AUTH=assert` an instance
  calling an allowlisted peer for a user (`SERVICE`, LDES sync) sends a
  five-minute ES256 identity assertion signed with its OIDC-provider key;
  with `OTS_TRUSTED_ISSUERS` a peer accepts such assertions after verifying
  them against the issuer's JWKS and audience, provisions a read-only
  federated user with organisation memberships from the assertion's `org:`
  groups, and authorises locally as for any user. An assertion speaks only
  for the peer's user: identities hang off a per-peer provider row (never
  the environment OIDC provider), a peer-supplied e-mail is discarded so
  provisioning never links to a local account by address, no claim maps to a
  role, and the principal is capped at `User` with no write access. See
  docs/federation.md. (#347)
- **RP-initiated logout and `prompt=none` for the OIDC provider.** Discovery
  advertises an `end_session_endpoint`: `GET /oauth/logout` ends the store's
  browser session (revokes the refresh token carried by the cookie, clears
  both auth cookies) and returns the browser to `post_logout_redirect_uri` —
  only when it is on a registered client's origin, with `state` echoed;
  anything else lands on the store's sign-in page, so the endpoint is never
  an open redirector. Before this a client app's "sign out" could only drop
  its own tokens: the store session survived and the next "sign in" silently
  re-authenticated from it — on a shared machine, as the previous person.
  The authorize page answers `prompt=none` with `error=login_required`
  instead of showing a login form to a client that is only probing for a
  session (silent renew). (#348)
- **Domain starter profiles.** `examples/seed-bundles/clinical-reference`
  (a FHIR-shaped record model, loaded in CI) proves the layered convention is
  domain-neutral next to `layered-reference` and the real-data
  `nen2660-imbor`; `gwsw` for RIONED's urban-water ontology (GWSW Totaal 1.7.0, fetched from data.gwsw.nl).
- **Linked-document containers, ICDD first.** `POST
  /api/datasets/:id/containers/import` unpacks an ISO 21597-1 container:
  documents become assets, linksets and payload triples become role-typed
  graphs, the index becomes a catalogue graph; `GET …/containers/export`
  packages a dataset as one. Graphs marked private are exported only to
  callers who can write the dataset. The container mechanism is
  profile-neutral. See docs/containers.md. (#347)
- **Per-dataset entailment.** `PUT /api/datasets/:id/entailment` selects a
  regime (rdfs, owl2-rl/el/ql/dl) and a mode; `materialize` rebuilds the
  dataset's own entailment graph after every write over its conformance
  layer; queries opt in with `?entailment_dataset=<id>` — now honoured on
  `POST /sparql` too (query parameters and form fields), where the
  entailment parameters used to be dropped.
- **RDF Patch.** A version diff is served as an RDF Patch document
  (`?format=rdf-patch` or `Accept: application/rdf-patch`), and `POST
  /api/datasets/:id/patch` applies a patch atomically to the dataset's
  registered graphs as one commit. Every term of an `A`/`D` line is
  validated: prefixed names must be declared by a preceding `PA` and be
  well-formed, quads must name a graph registered to the dataset, and a
  malformed line is a 400 before any update text exists. (#347)
- **DCAT-AP / DCAT-AP-NL catalogue.** `DCAT_PROFILE=dcat-ap|dcat-ap-nl`
  adds the application-profile properties (typed publisher agents,
  identifiers, language, file-type and media-type on distributions, the
  SPARQL endpoint as a `dcat:DataService`, EU-authority status values); the
  catalogue is now built as an RDF graph and serialised per request, so
  user-supplied values can no longer corrupt it, VoID statistics cover named
  graphs and are cached until the next write, and LDES streams are advertised
  as distributions. Catalogue metadata comes from `CATALOG_*` variables.
- **Time-evolving properties (OPM profile).** `POST
  /api/datasets/:id/properties/state` records a property value as an
  `opm:PropertyState` (valid-from, recording time, agent, reliability, note)
  in the dataset's provenance-role states graph while keeping the current
  value as a plain triple in the data graph; `…/properties/history` and
  `…/properties/as-of` read the chain. A `language` tag or `datatype` IRI in
  the body is validated before it reaches the update. Domain-neutral —
  material passports and clinical records supply the property IRIs. (#347)
- **Constraint-specification importers, IDS first.** `POST
  /api/shacl/import/ids` turns a buildingSMART IDS 1.0 document into SHACL
  Core shapes over the IFC RDF this store emits, optionally creating the
  shape graph in SHACL Studio; `GET /api/shacl/importers` lists the formats.
  The importer interface is generic — the next domain's format is a new
  implementation, not a new endpoint.
- **LDES publishing and client sync.** Any dataset can be published as a
  Linked Data Event Stream (`PUT /api/datasets/:id/ldes`): every write becomes
  entity-level version-object members with tombstones for deletions,
  fragmented into immutable time-ordered TREE nodes; `POST /api/ldes/sync`
  pulls a remote stream (allowlisted) into a dataset and keeps syncing
  increments. Graphs marked private are never published to a stream.
  Domain-neutral; see docs/ldes.md. (#347)
- **Per-graph, per-transaction PROV-O.** Every commit now types its
  affected graphs as `prov:Entity` generated by (and attributed to the agent
  of) the activity, with a start/end time; `GET /api/datasets/:id/provenance`
  assembles a dataset's trail as PROV-O Turtle (entities, activities, agents,
  versions as revisions); the DCAT catalogue attributes each dataset to its
  owner and names its last generating activity — it declared the prov prefix
  and emitted no PROV triple before.
- **SPARQL federation behind an allowlist.** `SERVICE <endpoint> { … }` works
  again for endpoints covered by `OTS_REMOTE_ALLOWLIST`, with a per-call
  timeout (`OTS_REMOTE_TIMEOUT_SECS`) and row cap (`OTS_SERVICE_MAX_ROWS`);
  redirects are refused so an allowlisted host cannot bounce a request
  elsewhere, and anything not allowlisted still errors (SSRF mitigation;
  `SERVICE SILENT` yields the empty solution the specification prescribes).
  An entry covers a URL when scheme, host and port are equal, the URL
  carries no credentials, and the entry's path is a prefix of the URL's path
  on segment boundaries — never a raw string prefix. The same allowlist and
  timeout gate the LDES client, the only other place a request names a
  remote URL. A federated query is never stored in the result cache.
  `sd:BasicFederatedQuery` is advertised only when an allowlist is
  configured. See docs/federation.md. (#347)
- **Seed bundles ship the model layer.** A manifest can declare
  `[[data_models]]` (registered with one published version), a dataset's
  `conforms_to`, and `shape_graphs` that are registered in the SHACL Studio
  library and bound to the dataset. `examples/seed-bundles/layered-reference`
  exercises the whole layered convention end to end (classification against
  the model layer, SHACL validation through the bound shapes) and is run in
  CI; `examples/seed-bundles/nen2660-imbor` carries the Dutch standards on the
  same mechanism with a `fetch.sh` for the (non-vendored) RDF.
- **Dataset version retention:** `GET …/versions/:a/diff/:b` (or `…/diff/live`)
  reports the per-graph triple delta; `DELETE …/versions/:ver` reclaims a
  version's snapshot graphs (published ones need deprecating first, or
  `?force=true`); `POST …/versions/gc` keeps the newest N non-published
  versions. Previously every versioned re-import retained a full copy with no
  way to delete it.
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
- **Register models from the import wizard.** Files detected as a knowledge
  model or vocabulary get a per-file destination — *Dataset* (the unchanged
  default) or *Model Registry* — and the banner that used to push users away
  to the models page becomes a one-click, non-blocking suggestion that flips
  those files' destination. Registry-bound files carry a title (from the
  filename), a version (from `owl:versionInfo`, editable), a namespace (from
  the ontology IRI) and a public toggle; the wizard creates the model (or
  versions an existing one) through the registry API under the owner
  selected in step 2, and the result panel links to the model page. A batch
  that is entirely registry-bound needs no dataset. (#321)
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
- **The Triple Browser sends its whole scope, remembers it, and offers the
  query behind the view.** A selection mixing datasets with an organisation
  sent only part of itself — and one dataset plus one organisation matched no
  branch at all, so it sent no scope — which is why the rows, the count and
  the facet rail's "Terms in scope" described less than was selected. Every
  selected dataset and organisation is now sent, through a new `org_ids`
  parameter; `org_id` is still sent alongside it, so a backend without
  `org_ids` degrades to the old behaviour rather than losing the organisation
  half. The scope also survives the tab: it is a versioned `localStorage`
  snapshot, a scope in the URL still wins, an empty scope is remembered as a
  choice, datasets that are gone or no longer visible are dropped once the
  inventories load, and a failed inventory request never wipes a good scope.
  IRI filter fields are wider and monospaced, and park their scroll at the
  local name when unfocused.
- **The API reference says what every endpoint needs.** Each documented
  endpoint carries its level — none, token or admin, defined once — where the
  reference previously said only that "most write endpoints and private
  resources require an Authorization header", leaving a reader to guess about
  every individual endpoint. The facts a deployer needs are stated where they
  will be read: public datasets are readable without a token by design, a
  private dataset's triples *and* its files are refused both to anonymous
  callers and to signed-in users without a grant, and listing all accounts is
  admin-only. A document about access control is worth nothing once it
  drifts, so `tests/api_reference_auth.rs` parses the levels back out of the
  table and fires an anonymous request at every documented endpoint — `none`
  must not answer 401, `token` and `admin` must — and asserts it exercised a
  reasonable number of rows, so an unparseable table fails loudly rather than
  vacuously passing.
- **The README and the overview page now lead to what the store can do
  under load and in production**: Highlights rows for the in-memory
  accelerator, the change log (with why it is off by default), replication
  and failover, and workload telemetry, each linking to its chapter; a
  Quick Start paragraph for the read-replica compose file; and, on the
  in-app overview, capability bullets for change log & replication and for
  observability, plus a "running it for others?" starting point.
- **`POST /sparql/batch` answers 422 when a statement fails at execution.**
  The batch is one transaction, so nothing is applied; the body keeps its
  shape (`status: rolled_back`, per-statement `results`) and gains `error`,
  which names the failing statement and its message. Earlier builds
  answered 200 with the same body. Parse and authorisation failures stay
  400 / 403; a fully applied batch stays 200.
- **Benchmarks measure the engine, not the result cache.** Every read
  benchmark in `benches/performance.rs` repeats one query on an unchanged
  store; with the result cache on, 63 of the 68 gated read benchmarks measured
  cache hits (the cache landed in June). The bench file now builds every store
  with the cache disabled, every bench runner exports `OTS_QUERY_CACHE=off`,
  and one explicit `query/cache_hit` benchmark covers the cached path. Read
  numbers in `benches/perf_baseline.json` recorded before this are cache-hit
  figures until the next refresh. See "What the read benchmarks measure" in
  docs/performance.md.
- **The performance gate confirms before it fails, and gates every group.**
  A benchmark the four-pass screen flags is re-benched on both revisions
  before the gate fails, so a fluke on one of 68 benchmarks clears and a real
  regression repeats — three consecutive PRs had been failed on regressions
  their diffs could not reach (#266). The insert, update, SHACL and
  concurrent groups — 35 benchmarks that could previously regress arbitrarily
  — are gated too, with screening tolerances, and two new benchmarks cover the
  ground-update delta path and the SHACL snapshot path (#347). See
  docs/performance.md.
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
- **The service description advertises `sd:BasicFederatedQuery` only when
  federation is available** — that is, when `OTS_REMOTE_ALLOWLIST` is
  configured. 0.6.0 advertised it unconditionally while `SERVICE` was disabled,
  so a federating client that trusted the description planned calls that
  failed every time. (#347)
- `plugin-accounts-dashboard` is compiled in GitHub CI; a per-feature
  availability matrix lives in docs/build-features.md.
- **ShEx and SWRL are graded Partial** in docs/standards.md, with the covered
  constructs listed; both now have semantic conformance suites instead of
  route-liveness smokes.
- **`BACKUP_ENCRYPT=true` requires an operator-supplied recipient.** The
  recipient at `BACKUP_ENCRYPT_KEY_PATH` is never generated any more: the
  server refuses to start, with instructions, when encryption is on and no
  recipient is present, and a failed encryption init is fatal rather than
  logged (continuing produced a manager whose every run failed, so
  "encryption is on" and "no backup has ever succeeded" looked identical from
  outside). `--restore` decrypts an encrypted archive in-process from
  `BACKUP_DECRYPT_IDENTITY_PATH` before anything is replaced, so a wrong
  identity fails with the live store intact. Why: see "Encrypted backups
  were unrecoverable" under Fixed. See docs/administration.md. (#347)
- **`OTS_MAX_UPLOAD_MB` replaces the fixed upload limits.** Graph Store
  bodies were capped at 50 MB and bulk imports at 200 MB, so a 226 MB dataset
  that loads fine in five appends was refused with 413. The defaults are
  512 MB (Graph Store) and 1 GB (bulk import); bodies are buffered and parsed
  before they replace anything, so the limit also bounds memory per request.
  (#347)
- **Model Registry writes are ownership-scoped, not publisher-gated.** A
  blanket `require_publisher` layer over the whole registry, on top of an
  admin-only create handler, made the registry read-only for regular
  accounts — while the import wizard advised registering model files there.
  Any signed-in account (guest-capability gated) may now create a model owned
  by itself or by an organisation it may act for; making a model public, or
  flipping a private one public, requires publisher rights (the rule public
  datasets already follow); owner reassignment stays admin-only, version
  uploads are checked against ownership, delete is super-admin-only and
  publish/deprecate transitions stay admin-only. The registry page shows
  *New model* to any signed-in user. (#321)
- **The import wizard asks merge-or-replace only when the target holds
  data.** The review step knows each registered graph's triple count: files
  whose targets are new (a new dataset, an unregistered or empty graph) show a
  *New graph* badge instead of a meaningless choice, populated targets show
  the toggle with the current size, and the version-bump selector appears
  only when a populated target is actually replaced. The post-import SHACL
  shapes probe fills its card after the success screen instead of keeping the
  wizard on "Importing…". (#321)
- **Spark no longer streams what it writes before its first retrieval.**
  Whatever the model writes before its first query cannot be grounded in
  data; streaming it painted a confident answer the next event had to wipe —
  the "it answers, then retracts and apologises" experience. Post-retrieval
  rounds still stream live. Result tables rendered into follow-up prompts get
  a total cap (`CHAT_TABLE_MAX_CHARS`): per-cell truncation alone let a wide
  50-row result reach several thousand tokens per round. (#295)
- **Bundled vocabularies are verbatim upstream copies.** `sosa` and `ssn`
  (the W3C SDW source), `saref` (ETSI SAREF core 3.1.1), `bot` (W3C LBD CG
  0.3.2), `omg` (0.3, was a 0.0.1 excerpt) and `fog` (0.0.4, was 0.0.1) are
  complete copies from their publishers. They used to be hand-authored
  excerpts whose class and property IRIs were modelled plausibly against the
  namespace rather than copied from the source, and nothing in the UI or the
  registry told them apart from real terms. The files with no authoritative
  source are gone — see Removed. (`f20a87b`)
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
- **Dependencies.** Three batches supersede the open Dependabot PRs (#289,
  #326, plus the advisory bumps in #321), with the code migrations they
  require: `rand` 0.10 (`thread_rng()` → `rng()`; still the ChaCha12 CSPRNG,
  so token, TOTP and JWT-secret generation is unchanged), `symphonia` 0.6,
  `base64` 0.23, `wkt` 0.14, `infer` 0.22, `zip` 8, `parry3d-f64` 0.30,
  `jsonwebtoken` 11 (now on its pure-Rust `rust_crypto` backend; v11 ships
  **no** signing backend by default — the OIDC provider's stored ES256 key
  still parses), `quick-xml` 0.42 (names and attribute values moved from
  bytes to `&str`), `lru` 0.18 (clears RUSTSEC-2026-0253 for real), `h2`
  0.4.19 (RUSTSEC-2026-0258), `eslint` 10 (with `@eslint/js` 10),
  `maplibre-gl` 6.6, `nanoid` past GHSA-2v37-7h3g-55p8, `tokio`, `uuid`,
  `futures`, `thiserror`, `http-body-util`, `aws-smithy-http-client`,
  `cytoscape`, `globals`, `swagger-ui-dist`, `n3`, `proj4`, `dompurify`,
  `playwright`, `@codemirror/commands`, `@testing-library/jest-dom`,
  `@typescript-eslint/parser`, `@sveltejs/vite-plugin-svelte`,
  `eslint-plugin-svelte`, and the SHA-pinned `Swatinem/rust-cache` and
  `docker/login-action` actions. `svelte/no-reactive-functions` is disabled
  (its fixer calls an API eslint 10 removed and crashes the lint run); the
  stale `deny.toml` ignores were dropped. The never-imported
  `utoipa-swagger-ui` crate is gone (#291) — the OpenAPI document is still
  served at `/api-docs/openapi.json` and the interactive UI is the frontend's
  own page — which also removes the duplicate `axum` 0.8 and `zip` 3 from the
  tree. TypeScript 7 remains held: typescript-eslint has no release that
  supports it.
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
- **The Triple Browser's Simple/Advanced switch.** Everything it gated is
  simply available, and in its place is one SPARQL button that opens the
  query behind the current view. The natural-language panel now appears on
  its own real condition — whether a gateway is configured — rather than on a
  mode. The five dead translation keys are out of both dictionaries, and
  docs/search-syntax.md, which is compiled into the binary and linked from
  the browser's own help popover, no longer tells the reader to flip a
  control that is not there.
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
- **A follower's first boot printed a wall of warnings.** The boot seed writes
  the Studio's meta-shapes, the per-standard shape graphs, the bundled demo
  organisation and its data, the standard vocabularies and the built-in
  documentation — and every one of those writes is refused on a node that keeps
  its store read-only, each refusal logged as a warning. An operator had no way
  to tell them from a real fault. A follower skips the seed now and says so
  once, at info level. Nothing is lost: the same graphs arrive from the leader,
  and a follower's identity database is replaced wholesale by the leader's
  snapshot, so anything seeded locally was overwritten at the first catch-up
  anyway — which it had been, silently creating local dataset rows the leader
  then replaced.
- **The in-page section bar was a hard rectangle across the page.** Square
  corners, a single bottom rule and negative margins bleeding it to the
  container edges, so the moment it stuck it cut the page in two and the
  content scrolling under it ran straight into that edge. It is a rounded
  translucent bar now, the same radius as the cards it sits among, inset from
  the edges with a gap above it when stuck.
- **A dark-mode form field was the same colour as the card holding it.** The
  dark input rule set `background: var(--bg-strong)`, which is also the
  background of every card and dialog those fields sit in — measured on the
  dataset page-settings dialog, an input and its dialog were both
  `rgb(17, 24, 39)`, a contrast ratio of **1.00** — and the border sat at 0.18
  alpha, about 1.9:1 against that surface. A form read as a column of labels
  with nothing underneath. Fields are sunk below their surface now with an edge
  at 3.1:1, the contrast WCAG 1.4.11 asks of a UI component's boundary. The
  section headings in the dataset and organisation settings dialogs were also
  under AA — 3.73:1 in dark and 3.54:1 in light for 12.5px text — and are now
  6.9:1.
- **Every dialog in the app opened in the middle of the page, not the middle
  of the screen.** `.route-view` carried `animation: routeIn … both`, and
  `both` includes `forwards`: the final keyframe keeps applying after the
  animation ends. That frame reads `transform: none`, but a filled animation
  still *sets* the property, so the computed value stayed
  `matrix(1, 0, 0, 1, 0, 0)` — and any transform but `none` makes an element
  the containing block for its `position: fixed` descendants. A
  `position: fixed; inset: 0` backdrop was therefore the size of the routed
  page rather than the viewport: measured 5052px tall against a 900px screen,
  so a centred dialog opened about 2500px down, out of sight, at a scroll
  position unrelated to where the reader was. `backwards` keeps the entrance
  and drops the residue.
- **The SHACL Studio's pages were cramped at phone width.** On the validation
  list each dataset row crammed a status pill, a shapes picker, a run button
  rendered as a wide empty bar and a toggle whose label broke over two lines
  into one narrow card; the shapes library's grid had a 320px column floor that
  pushed a 320px screen sideways; the pipelines list drew its two corner links
  on top of the heading; and the shape-graph editor stacked seven full-width
  buttons down the page. Across all seven Studio pages the control rows wrap or
  stack below 720px, tap targets are at least 40px, and the facet labels, run
  names, pipeline names and selection chips wrap rather than being cut.
- **The documentation index had no structure.** It was a bare two-column list
  of plain-text links under all-caps headings, with column heights so uneven
  that a one-entry section sat beside a four-entry one, and no cards, borders
  or separation anywhere — on desktop as much as on a phone. The sections are
  cards now, in balanced columns that reflow to one at phone width, matching
  the rest of the app. The article also keeps its reading measure at tablet
  widths (it had been allowed to run to the full column), headings step down on
  a phone, and long doc titles wrap instead of being cut. A page whose widest
  code block set the layout width no longer scrolls the whole page sideways at
  320px.
- **The dataset list was a desktop table squeezed into a phone.** "About" sat
  as a small chip on its own line above a full-width "New Dataset" pill; the
  table below had a name column so narrow that every name wrapped, a nearly
  empty validation column, and rules that simply *hid* the owner below 700px
  and the role and visibility below 480px. The two actions are one row now,
  and below 720px each dataset is a card carrying its name, description, roles,
  validation state, visibility and owner — nothing is hidden any more, and
  nothing scrolls sideways.
- **The SPARQL editor's Format button was drawn on top of the query.** At
  phone width it covered both `PREFIX` lines, because the button is
  `position: absolute` *and* a `.btn`, which `app.css` stretches to
  `width: 100%` below 720px — a full-width overlay pinned across the top of the
  editor. Below that breakpoint it now sits in its own right-aligned row above
  the editor instead of floating over it, and the editor soft-wraps rather than
  scrolling sideways. The "empty black box" between the LLM chip and Generate
  was the natural-language question field, squeezed to nothing by the same
  rule; the row wraps now, so the field keeps a readable size and the button
  takes the line below. Desktop keeps the floating button and the unwrapped
  editor.
- **The global search palette did not fit a phone.** The dialog was wider
  than a 375px viewport, so the "Open" button was cut off past the right edge;
  the input had collapsed to roughly its magnifier icon; and the three
  "Navigate to" cards had squeezed their labels to "Da…", "Or…", "Mo…" over
  sublabels reading "Coll…", "Tea…", "Ver…". Below 720px the input takes its
  own row with the button beneath it, the cards stack, and every label reads in
  full. On desktop the only change is that the card sublabels wrap to a second
  line instead of being cut.
- **Asking a node for an asset it does not hold was a `500`.** A follower
  replicates the store and the identity database but not the object store, so
  its file libraries list the leader's files — the metadata travels with the
  identity database — while the bytes stay on the leader. Downloading one
  answered `500 Failed to read asset "/data/assets/…": No such file or
  directory`, which blames the node for working as designed, tells the caller
  nothing about where the file is, and puts a server-side absolute path in the
  response body. It is a `404` now, and on a follower the message names the
  leader that holds the bytes. A store that is genuinely misconfigured or
  unreachable is a different thing and keeps its `500`: conflating the two
  would make a broken S3 endpoint look like an empty one.
- **The Studio's Datasets tab left the Studio.** It pointed at `/datasets`,
  the global catalogue, which is not part of the SHACL workspace and offers no
  way back into it — while the Overview's own "Datasets to validate" card
  pointed at `/validation`, the validation overview, which is what "datasets"
  means inside the Studio. The tab now goes where the card goes. The tabs also
  run in the order the work runs — Overview, Shapes, Datasets, Pipelines,
  Results — and the validation overview renders the Studio bar, which it never
  did, so the page is reachable *and* leaveable. Results no longer claims
  `/validation` as its own: that claim was invisible while nothing on the page
  drew the bar, and would now light the wrong tab.
- **Terms, names and IRIs were cut off rather than shown.** Measured on the
  seeded demo, one screen of the triple browser had **110 elements whose text
  did not fit the box drawn for it** — a cell 31px wide holding an IRI that
  needed 280, predicate chips 12px wide needing 105 — because `td` carried
  `overflow: hidden; text-overflow: ellipsis; white-space: nowrap`. Half an
  IRI is not a shorter IRI, it is the wrong one, so nothing is trimmed now: a
  term too wide for its column wraps inside it, the column widths stay (which
  is what keeps four columns on screen without a horizontal scrollbar), and
  the copy control stays on the first line beside it. The same goes for the
  names and owner chips in the validation list and the file browser's dataset
  cards. Descriptions are the one exception and are marked as such: they are
  prose rather than identifiers, run past a thousand characters on the demo,
  and are clamped to two lines with the whole text on the dataset's own page.
  jsdom has no layout, so the regression tests measure real boxes in a real
  browser — including the wrong fix, which is to hand the overflow to the
  document and give the page a horizontal scrollbar.
- **Every dataset name in the SHACL validation list rendered at zero width.**
  Measured 0.0px against a `scrollWidth` of 138px: the text in the DOM and
  nothing on screen, on every row. `overflow: hidden` replaces a flex item's
  automatic minimum size with 0, so the flex algorithm was free to shrink the
  name away entirely — and did, because the owner chip beside it has
  `overflow: visible`, whose minimum is its content, and gave up none of its
  115px. The name now has a 4.5rem floor and grows into whatever is left, the
  chip yields, and the status pill wraps under the title instead of crushing
  it; measured after, names are 117–155px and the chip falls to 72px where a
  name is long. jsdom has no layout, so no component test can see this class
  of bug at all: the regression test is a Playwright spec that measures, and
  it fails against the previous CSS with `"3D, Map & BIM Demo" is 0px`.
- **The validation page's "link shapes" picker offered two graphs where the
  Shapes view offers twenty-seven.** It listed the datasets that already had
  a shapes graph attached rather than the SHACL library, so a shape graph
  authored in the Library was unreachable until somebody had already linked
  it somewhere else. It now offers the library itself — the same source the
  Shapes view reads — labelled by each graph's own name and shape count, with
  the old source folded in behind it so nothing that used to be linkable
  stopped being so. The Studio nav gains a Datasets tab, and wraps rather
  than overflowing once there are five.
- **A browse scope naming datasets *and* organisations dropped the
  organisations.** The handlers resolved their scope as `if dataset_ids …
  else if org_id …`, so a caller sending both — which the Triple Browser does
  whenever a selection mixes them — had the organisation silently ignored, at
  the facets path, the triples query builder and the resource view alike.
  `scope_dataset_ids` now returns the deduplicated union of the datasets
  named directly and the datasets of every organisation named; `dataset_id`,
  `dataset_ids` and `org_id` used alone behave exactly as before, and an
  empty union still means nothing in scope rather than everything. Access
  control is unchanged — the union only builds a list of ids, which every
  caller passes through the same per-dataset visibility filtering as before —
  and a test asserts that a non-member reaches a private graph by none of the
  four spellings.
- **The graph's double-click looked broken because it was silent.** The
  handler fires, fetches and merges correctly; it simply said nothing in four
  of its five outcomes, which made the ways it can legitimately do nothing
  indistinguishable from a dead control: the neighbours are already on the
  canvas (a node with seven edges fetched exactly seven triples and changed
  nothing), the node was expanded earlier and its neighbours came back with
  the restored working state — a cache hit with no request and no visible
  change, which survives a reload and so can make a whole session feel dead —
  the scope genuinely has no more neighbours, or the expansion failed and a
  bare `catch {}` swallowed it. Every outcome now says which it was, once,
  through the existing toast. The gesture was also tighter than the
  platform's, a hand-rolled 300ms window against a ~500ms default
  double-click speed on both Windows and macOS; it now listens for the
  container's native `dblclick` and keeps the manual detector for touch only.
- **The resource page's Copy IRI was easy to miss and silent when it
  failed.** An unlabelled 16px icon sat at the far end of a one-line IRI that
  was itself truncated with an ellipsis, so selecting the text by hand
  yielded a clipped IRI; and the failure path was an `if` with no `else`, on
  a path that fails whenever the document is unfocused or the store is
  reached over plain HTTP. The control is a labelled button, the IRI wraps in
  full and stays selectable, the two duplicated header blocks are one
  snippet, and a failed copy says to select the text and press Ctrl/Cmd + C.
- **A follower waits out the leader's rate limiter instead of restarting its
  bootstrap.** The leader answers `429` with `Retry-After` to a follower
  like to any client of its address; the follower took that as a failed
  catch-up and started again from the first graph on its next tick, which
  kept the limiter drained — a leader with more graphs than the limiter's
  burst (40) never got a follower past bootstrap. The client now honours
  `Retry-After` — a `0`, which the limiter writes for any sub-second wait,
  is read as a second — and sends the same request again, bounded (eight
  waits, 30 s each at most), so a bootstrap proceeds at the limiter's
  sustained rate.
  And a hot follower's long-poll is held up to 25 s (it was one poll
  period, 500 ms): the leader still answers the moment a row lands, but an
  idle hot follower now costs it a handful of small requests per 25 s
  instead of four a second — the rate at which the same limiter had cut
  the tailing off right after the bootstrap, so the leader's writes never
  arrived. Its lag stays a round trip while the leader's writes leave
  room under the limiter (about one every three seconds sustained);
  faster than that it is paced by the limiter and applies rows in pages. And a
  row that fetched a graph whole is bookmarked at once rather than at the
  end of its page: under the limiter a page of bulk-load rows takes a
  second a row, and a status frozen at the page's start read as stale
  while the follower was applying rows the whole time.
- **An API token's `last_used_at` is written at most once a minute.** It
  was rewritten on every request, which on a replication leader moved the
  identity database's version at every request its follower made — so
  the follower fetched the whole identity database again at every check,
  every five seconds on a hot follower. The stamp is bookkeeping; once a
  minute keeps it useful and means the follower's own polling moves the
  version it watches at most once a minute.
- **STEP parser: an empty list swallowed every argument after it.** `()` was
  read as a list holding one unknown byte with the closing paren consumed, so
  an `IfcProject` written with empty `RepresentationContexts` — most
  exporters — never had a `UnitsInContext`, and `IfcRelConnectsPathElements`
  lost its connection types. Nothing had read past those positions before.
- **Graph Store `PUT` replaces a graph in one transaction.** The replace
  cleared the graph in one transaction and bulk-loaded the new quads in
  another, so a concurrent reader could see the graph empty in between and a
  crash in between left it empty for good. Clear and insert now commit
  together: a reader sees the old graph or the new one, never an empty or
  half-filled one. A first `PUT` into an empty graph keeps the bulk loader.
  The cost is one write batch the size of old + new (a 900k-quad replace
  17.7 s → 32.3 s in-process, accepted for the guarantee); see
  docs/performance.md.
- **Graph Store `PUT` no longer wipes a graph when its body is rejected.**
  The handler cleared the target graph and only then parsed the request
  body, so a `PUT` with one syntax error returned 4xx and left the graph
  empty, with nothing in the response saying so. It parses first now. (#347)
- **`/sparql/batch` is one transaction.** The endpoint documented the batch
  as atomic while each statement ran as its own transaction, so a statement
  that failed at execution left the earlier ones applied. The statements now
  run in order on one transaction (each sees the previous ones) and the
  first failure rolls everything back: `status: ok` is unchanged; a failed
  batch answers `status: rolled_back` with per-statement `results` — the
  failing statement `error`, every other one `rolled_back`, never `ok` —
  instead of `partial`. OpenAPI states the real codes (it promised a 204 the
  handler never sent); see docs/api-reference.md.
- **Read isolation is pinned, and the `opengraph` MVCC note corrected.** A
  `SELECT` joining several patterns sees one committed state for its whole
  duration — oxigraph 0.5 binds one storage snapshot per query — on RocksDB
  and in memory alike. `tests/read_isolation.rs` races a writer that flips two
  properties of every subject in one transaction against a reader joining
  them and observes no torn read; `opengraph/src/mvcc.rs` no longer claims a
  snapshot per iterator.
- **SHACL write gates fail closed on an unevaluable `sh:sparql` constraint.**
  A `sh:select` that did not parse (or errored at evaluation) produced no
  violations, so the graph conformed by accident and Graph Store writes gated
  on it went through with 204 — on the dataset `shacl_on_write` path and on
  Studio gating pipelines alike. Such a constraint now fails the shape load
  (an ill-formed shapes graph is an error the caller sees: 422 with a
  gate-evaluation report), a runtime error is a violation of the focus node,
  and `load_shapes` no longer drops a shape that fails to load with a warning
  — which required class/predicate targets of blank-node shapes to resolve
  through the quad index instead of an invalid `<_:…>` SPARQL IRI. The
  POST-merge gate (`sh:maxCount 1` on a second value) and the stale-cache race
  were already closed in #347 and are now pinned by tests.
- **Studio write gates fail closed on every infrastructure failure.** A temp
  store that would not allocate, a target graph IRI that would not parse,
  quads that would not stage, a shape graph that failed to copy (validation
  then ran against an empty shapes graph and conformed trivially) or a SHACL
  engine error all returned "let the write through", so the write landed
  unvalidated with nothing anywhere recording it; the same happened when a
  gating pipeline's shape graph had been deleted or its lookup failed — the
  gate silently stopped gating. All of these now refuse the write with the
  existing 422 path and a `gate-evaluation-failure` report saying the failure
  was server-side and the write was not applied. (#347)
- **A SHACL write gate on `POST` validates the post-merge state.** Both gates
  staged only the request payload, which is right for a `PUT` and wrong for
  a `POST` in both directions: merging one property onto an existing node was
  rejected (`sh:minCount` evaluated against a node stripped of every property
  the payload did not repeat), and adding a second value passed `sh:maxCount
  1` because the temp store held only the new one. Merge seeds the target
  graph's current contents before the payload; replace is unchanged. (#347)
- **SHACL validation at scale.** The engine ran one full SPARQL round trip
  per focus node and property path (three parses, a fresh evaluator with
  forty custom-function registrations and a store-wide `sh:SPARQLFunction`
  scan, a plan compile, a result-cache mutex that never hit) and took a
  RocksDB snapshot per probe under the database's global mutex; every
  constraint re-fetched its value nodes through a per-thread string cache.
  A validation now opens one data source for the whole run — the query
  accelerator's clean in-memory copy when one is published, else one RocksDB
  snapshot, else the live memory store — resolves value nodes natively from
  the quad index for every path form (fetched once per focus node and
  property shape), resolves targets and `sh:class` from per-run class sets,
  and on the snapshot path builds a per-run adjacency for the shape
  predicates (`OTS_SHACL_RUN_INDEX_MIN_PROBES`, `OTS_SHACL_RUN_INDEX_MAX_QUADS`).
  No SPARQL runs on the per-focus-node path; `sh:sparql` constraints and
  SPARQL targets still read the live store. Result changes, all pinned by
  tests: a `+` path whose focus lies on a cycle yields the focus (SPARQL
  semantics; the old native walk for blank-node focus nodes never emitted
  it); `sh:class` and `sh:targetClass` both count instances typed through an
  anonymous subclass; an invalid IRI among a run's data graphs skips only
  that graph instead of voiding every target and value; result order within
  a report is unspecified. `write_generation()` now advances with the result
  cache disabled, every write is bracketed so the accelerator cannot publish
  a copy built during a write, and a pending accelerator rebuild no longer
  lets queries starve it with full-store `len()` scans.
- **Query performance at scale.** Graph-scoped grouped aggregates — the form
  every HTTP query takes after ACL scoping — now run on the sharded,
  multi-core path (the planner refused any `GRAPH` pattern); `COUNT(*)`
  inside `GRAPH <g>` / `GRAPH ?g` is answered from the count index instead of
  a scan; the accelerator's RAM-aware cap works on macOS, and a background
  tick rebuilds it once writes go quiet instead of waiting for the next
  query to pay for the rebuild. `sh:class` checks use a cached subclass
  closure instead of one SPARQL `ASK` per value node, and `sh:pattern`
  compiles each regex once per thread instead of running a SPARQL `ASK`
  per value. Large graph clears run in chunked transactions. (#347)
- **Write throughput at scale.** Three per-write costs proportional to the
  size of the written graph or store are gone: the count index no longer
  rescans a graph after every load or ground update, the text index no
  longer drops and re-indexes every literal of the graph after every write,
  and incremental writes no longer commit the text index per request. On a
  900k-quad graph the concurrent-write phase of the scale benchmark went
  from 2 000 to 900 000+ quads in 20 s in-process, and over HTTP from 3 500
  to the level of Apache Jena Fuseki on the same machine (docs/performance.md).
- **Query-accelerator rebuilds run in the background and never serve stale
  data.** The first aggregate query after writes went quiet paid the
  in-memory mirror rebuild inline — ~29 s on a 1.7M-triple store, past the
  30 s SPARQL timeout. Rebuilds above 50k triples now run on a background
  thread and a query that arrives meanwhile is served by the persistent store
  instead of blocking. The mirror stays dirty until its copies are published,
  is unavailable (not stale) while building, and stays dirty if a write lands
  during the build: an intermediate version marked itself clean before the
  copies existed, and a SHACL run right after an import reported violations
  that did not exist, then none at all. (#321, #347)
- **The store stays responsive during and after uploads.** The first
  `CONTAINS`/`STRSTARTS` query after any write paid a whole-store full-text
  reindex inline (39.8 s on a 1.7M-triple store); bulk import never
  invalidated the text index at all, so search silently missed everything
  just uploaded until an unrelated write forced a rebuild; Graph Store
  `DELETE` never invalidated it either. Write paths now refresh exactly the
  graphs they touched (bulk import including replace targets and
  version-archive graphs, Graph Store `PUT`/`POST`/`DELETE`, SPARQL Update
  with known targets, the IFC/CityJSON pipelines); a dirty index makes
  `CONTAINS`/`STRSTARTS` skip the push-down (plain evaluation, still correct)
  and kicks one background resync, and only `text:search` — whose expansion
  *is* the result — waits for it. Replace imports compare triple sets by
  64-bit hash instead of formatted strings, and post-import bookkeeping
  (graph registration, role detection on a 100k-quad sample, DCAT rewrite,
  the shapes probe) runs off the request path. (#321)
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
  the first write); rebuilds are serialised and run on the blocking pool (one
  query stalled the whole runtime for 1.19 s on a 20k-literal store), and
  `POST /api/text-search/reindex` runs under the same lock so it cannot race
  the background sync. Spark's queries go through the same preprocessing, so
  a `text:search` from the chat and "Open in SPARQL workspace" behave
  identically. See docs/full-text-search.md. (#293, #347)
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
  Solution modifiers written inside the `WHERE` block (`} LIMIT 50 }`) are
  hoisted; an IRI that differs from a sampled term by letter case alone is
  corrected (small models tidy `local_name` into camelCase, which parses and
  matches nothing); trailing prose after an unfenced `SPARQL:` directive is
  cut at the parser's reported error position when the prefix alone parses
  (a model that explained itself after its query used to fail every counting
  question this way); and every other IRI is checked against the store
  itself — four indexed probes: subject, predicate, object, named graph — so
  an invented one fails fast with the offending IRIs named, while a real term
  outside the vocabulary sample runs. IRIs the user pasted are never
  rejected: an absent one runs to an honest empty result instead of an error
  blaming the user. (#292, #295, #322)
- **Spark retrieval works with weaker models.** A reply that writes its query
  in a ```` ```sparql ```` fence instead of emitting the `SPARQL:` directive is
  executed as long as no round has succeeded this turn (a fenced query in a
  final answer stays the query card the user is meant to see) — both a 1.5B
  and a 7.6B model, live, answered a failed round with prose plus a correct
  fenced query and then answered from memory. A turn that asks for no query
  at all gets one short, explicit nudge to query first; the model may decline
  (conceptual questions need no data) and its original answer is kept when it
  does, so the nudge can only add retrieval. (#263, #347)
- **Spark no longer re-runs a query that already failed this turn.** The
  repeat is recorded in the retrieval trail with its own error, the store is
  not consulted, and the model is told the query was not run. The parser's
  "variable that is unbound" message — an aggregate projected next to an
  ungrouped variable, the exact failure two local models produced — now
  carries an actionable GROUP BY hint.
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
- **Stale entailments and wrong counts on rematerialisation.**
  Rematerialising clears the target `urn:entailment:*` graph first, so a
  removed premise no longer leaves its stale inferences behind, and
  `triples_added` is the delta rather than the graph size (RDFS, RL, EL, DL);
  `rdfs5` reads the target graph as well as the source, so `subPropertyOf`
  chains close beyond three links and a second RDFS run adds nothing. (#347)
- **SHACL-AF rules materialise into the data graph.** Rule materialisation
  built `INSERT DATA` / `INSERT … WHERE` with no `GRAPH` clause, so every
  inferred triple landed in the store's default graph — outside every
  registered, ACL'd, dataset-owned graph and invisible to the very graph the
  rule was inferring over (a test asserted that placement). With one data
  graph, rules now write into it (`WITH <g>`, spliced in after the rule's
  `PREFIX` prologue so prefixed rules still parse); with several there is no
  single "the" graph, so those keep the previous placement. Rule execution
  also swallowed every SPARQL error with a warning, so `infer` returned 0
  whether the rules matched nothing or every one failed to parse; errors now
  propagate. (#347)
- **The query cache could serve a stale result as fresh.** `put()` read the
  write generation after evaluation had finished, so a write committed
  between evaluating and storing stamped the *new* generation onto the *old*
  value, and `get()` served it as fresh until the next write — for a hot
  query on a busy store, indefinitely. The generation is snapshotted before
  evaluation, and a result whose generation has moved is not cached. (#347)
- **ShEx:** CLOSED/EXTRA and value sets compared serialised terms with bare
  IRIs and never matched; the datatype check was a substring test skipped for
  simple literals (`"thirty"` satisfied `xsd:integer`); an unparseable schema
  parsed as an *empty* schema and validated everything with a 200; `PATTERN`
  was a substring test — the anchored `^[0-9]{4}$` rejected the conforming
  `"1234"` and `^abc` accepted `"xxabcxx"`, and the flags were discarded — and
  the numeric facets (`MININCLUSIVE`, `MAXEXCLUSIVE`, `TOTALDIGITS`, …) were
  declared in the AST but parsed and evaluated by nothing, so `xsd:integer
  MININCLUSIVE 0` accepted -5. All fixed: patterns compile as regexes with
  their ShEx/XPath flags (an uncompilable pattern is a schema error, not a
  pass), the facets are enforced, and the semantics are pinned by a new
  conformance suite. (#347)
- **SHACLC serialisation of blank-node property shapes.** Property-shape
  attributes were looked up by interpolating the subject into a SPARQL query;
  for the standard `sh:property [ … ]` idiom that subject is a blank node,
  which SPARQL reads as a variable, so every property shape drew an arbitrary
  path, datatype and cardinality from whichever shape matched first (a
  two-property shape did not even emit both paths). Lookups go through the
  quad index by term. Affects `GET …/turtle?format=shaclc`, `POST
  /api/shaclc/serialize` and the form manifest. (#347)
- **RML builds terms with oxrdf, not string interpolation.** Literals
  hand-escaped only backslash and quote, so a quoted multi-line CSV field put
  a raw newline into the generated Turtle and the whole mapping failed with
  "Failed to load generated triples" — every row, not the offending one; IRIs
  from `rml:reference` / `rr:column` with `rr:termType rr:IRI` were
  interpolated unvalidated, so a value with a space produced invalid Turtle
  and one with `>` could close the IRI and inject further triples into the
  generated document. An unrepresentable term now skips that term rather than
  corrupting the batch. (#347)
- **SWRL: an untranslatable builtin no longer drops its guard.** A builtin the
  translator could not express caused its `FILTER` to be dropped and the rest
  of the rule to run, asserting the head for every binding — silently unsound
  inference whose only signal was a debug line. It is a rule-level error now,
  and an OWL/XML atom that fails to build fails the document instead of being
  dropped from its rule (a rule missing one body atom fires with one condition
  fewer). Two text-form limits are documented: predicates are taken verbatim
  as IRIs, and every two-argument atom becomes a property atom, so the text
  form cannot express `swrlb:` builtins. (#347)
- **Encrypted backups were unrecoverable, and the scheduled backup opened a
  database that did not exist.** `init_backup_encryption` generated an age
  keypair, wrote only the public half and let the identity drop — every
  backup encrypted under an auto-generated recipient was encrypted to a key
  that existed nowhere, and `--restore` refused encrypted archives outright,
  so the loss was invisible until a restore was attempted (the logged
  warning, "store the private key securely", named a key the operator never
  had). Separately, the backup source defaulted to `data/auth.sqlite` while
  the server opens `--db-path`, else `<data-dir>/auth.db`, so on any install
  without `AUTH_DB_PATH` the scheduled backup failed after writing the RDF
  dump, leaving no manifest and one warning line. The resolved path is now
  threaded from the CLI; the recipient requirement is under Changed. (#347)
- **Operational alerts fire.** `ALERT_WEBHOOK_URL` and `ALERT_SMTP_*` were
  documented as ordinary controls, but the dispatcher — the only producer of
  the `alert_sent` audit event — had no call sites, so an operator could
  configure alerting completely and never receive an alert. The scheduler
  raises one on scheduled-backup failure (a distinct kind for upload
  failures); dispatch is best-effort and never breaks the path that raised
  it. (#347)
- **Unattended backups were silently disabled in the Docker image.**
  `BACKUP_DIR` defaulted to the working-directory-relative `data/backups` —
  `/app/data/backups` in the image, root-owned and unwritable for the service
  user — so every default deployment logged "backup: disabled — init failed"
  once and never backed up. The default is now `<data-dir>/backups`. The
  identity-database pool also opened all eight connections at once, each
  running `journal_mode=WAL`, and the losers logged a spurious "database is
  locked" error at every boot; it warms one connection and opens the rest on
  demand. (#347)
- **The commit log covers every data mutation.** It claimed to, and
  `GET /api/datasets/:id/commits` presented it as the dataset's history, but
  Graph Store PUT/POST/DELETE, bulk imports, every dataset-version operation
  (cut, publish, deprecate, restore, delete, GC, branch) and backup restores
  left no trace. Each records a `prov:Activity` now, with actor, affected
  graphs and counts where known; new kinds `graph-store`, `import`, `backup`.
  Found on the way: a multipart `meta` part sent after a `dataset_id` part
  silently replaced it, so the import ran without a dataset and left its
  graphs unregistered; `meta` keeps an earlier `dataset_id` unless it sets
  its own. (#347)
- **Version restore is atomic and streams.** Restore cleared the live graph
  *before* copying the snapshot back (a window in which the dataset's graph
  was simply gone), snapshot/branch/restore materialised every quad of every
  graph in memory first, and the full-text index was never refreshed after a
  restore. Copies now stream in batches, restore swaps a staging graph in with
  one `MOVE`, and the index is refreshed for the restored graphs.
- **Reasoning could not see dataset data.** `POST /api/reasoning/materialize`
  parsed `source_graphs` and ignored it, and every regime's rules read only
  the unnamed default graph — so a dataset's named graphs were invisible to
  materialisation however the endpoint was called. Scopes are now applied at
  the store level (a `USING` dataset on every rule): `dataset`, explicit
  `source_graphs` (read-checked per graph), or the default graph as before.
- **Read-scoped API tokens can use the SPARQL Protocol `POST` query form.**
  The write-scope guard treated every `POST` as a mutation, so a read-scoped
  token (and a federated identity) could not `POST /sparql` a query at all;
  queries are reads now, updates keep their own check. (#347)
- **An unknown graph role is a 400.** Setting a dataset's or graph's role to
  a misspelled value used to fold to "no role" and silently clear it with a 200.
- **Data-model version gates answer 403, not 401,** to an authenticated
  non-admin (publish, deprecate, sub-graph transitions).
- **Dataset versions answered 401 to an authenticated caller lacking write
  rights;** it is now 403, so clients no longer treat a missing grant as a
  session expiry.
- **The Model Registry page renders again.** `GET /api/models` returned one
  record per combination of a model's optional properties, so a stray second
  `dct:created` on 44 entries produced 89 records for 45 models with repeated
  ids — and the page, keyed by id, threw Svelte's `each_key_duplicate`, never
  cleared `loading`, and sat on "Loading models…" forever. One record per
  model; the read path is repaired rather than assuming single-valued
  optionals. (#296)
- **The registry card's delete button no longer covers the expand chevron.**
  The admin-only trash icon was absolutely positioned in the same corner as
  the card's open affordance, so on hover the two targets were
  indistinguishable and mis-clickable; it sits in the header row before the
  chevron. (#298)
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
- **The in-app docs viewer surfaces every guide** — 18 were in the repository
  but never registered (every OWL 2 guide, LDP, RML, SPARQL 1.2, plugins, …);
  a unit test now keeps the registry complete.
- docs/sparql-12.md documented a builder API and a crate version that do not
  exist; docs/triplestore-comparison.md claimed the full W3C SPARQL 1.1
  corpus runs in CI; docs/standards.md still said the engine was oxigraph 0.4
  on the RDF-star CG model with `rdf:reifies` and `{| |}` unsupported (all
  three are supported — the five ignored triple-term tests were rewritten to
  the RDF 1.2 model and run); several guides still said Oxigraph 0.4.
  Corrected. (#347)
- **Cesium viewer works air-gapped:** the engine's runtime assets are served
  from the application's own bundle instead of a CDN pinned 21 minor versions
  behind the bundled library.
- **`guest` was missing from the frontend role list**, so a guest user's role
  select rendered blank; a parity test now reads the Rust enum.
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
- **`POST /api/shaclc/serialize` read any named graph, for anyone.** It took
  a graph IRI from the request body and handed it straight to the serialiser
  — no authentication, no authorisation — so any caller could name any named
  graph in the store and read back whatever the SHACLC serialiser could
  express of it: shape IRIs, target classes, property paths, datatypes and
  constraint values. Verified against a running instance before the fix, a
  private dataset's shapes graph came back in full to a client with no token
  at all. The route now sits with its authenticated SHACL siblings **and**
  the handler asks `check_graph_read_access`, the same visibility helper that
  gates `/store` and `/sparql`, because a token alone would only have
  narrowed the leak from everyone to every signed-in user. A graph the caller
  may not read answers `403` whether or not it exists, so the endpoint cannot
  be used to discover which graph IRIs are present either.
- **`GET /api/users/public` returned every active account to anyone who
  asked** — id, username and avatar, signed in or not. The endpoint exists so
  the web UI can label the owner of something public, which is fair; handing
  over the whole roster is account enumeration, and it made "only an admin
  sees all users" true of `/api/users` alone. It now lists only the users the
  caller could already infer: the owners of datasets they may read, the
  members of organisations they belong to, and themselves. An admin still
  sees everyone, and the status codes and JSON shape are unchanged, so the
  owner chips that depend on it keep working. In the same pass, `POST
  /api/shaclc/parse` and `POST /api/rml/preview` join their authenticated
  siblings: neither discloses anything stored, but both spent the instance's
  CPU for callers it could not name, and no frontend page calls either.
- **LDP `PATCH` ran arbitrary SPARQL Update without authorisation.**
  `PATCH /ldp/*path` read a SPARQL Update from the request body and ran it
  verbatim; the handler took no authenticated user, so the only gate was the
  blanket `require_auth` on the mount — any authenticated caller could
  `DROP ALL`, or delete another tenant's named graph, bypassing every
  per-graph ACL that `POST /sparql` enforces (verified: a plain user's
  `DROP ALL` returned 204). It runs through the same gate as `POST /sparql`
  now: API-token write scope, admin gating of all-graph and
  variable-graph/`SERVICE` operations, read+write permission on every ground
  graph the update touches, audit event and provenance record. The remaining
  LDP verbs write into the default graph; that scoping is the documented
  remaining gap. (#347)
- **SWRL execution was unauthenticated and injectable.**
  `POST /api/swrl/execute` took no authenticated user, so any caller could
  materialise triples into any `target_graph` — another tenant's, or a shared
  `urn:entailment:*` graph; it now requires write permission on the target,
  like `/api/reasoning/materialize`. Rule terms were built with
  `format!("\"{}\"")` and `format!("<{}>")`, and class and property
  predicates were pasted between angle brackets after trimming, so a literal
  containing a quote, an IRI containing `>`, or a class name such as
  `http://ex/Person> } ; INSERT DATA { … ` closed the generated update and
  appended attacker-chosen SPARQL. Terms and predicates go through oxrdf's
  constructors and `Display` (N-Triples escaping); a predicate that is not an
  IRI — including a bare name, which is a relative IRI — is a 400 before any
  SPARQL exists, in both the text and the OWL/XML form. (#347)
- **`validate-and-commit` could overwrite another tenant's graph.** The
  handler registered the caller-supplied graph IRI to the caller's dataset
  and then `PUT` it — which *replaces* the graph — while `can_write_dataset`
  only proved the caller owned *their* dataset. Naming someone else's graph
  overwrote it wholesale, on both the `dataset` and the `new` branch
  (verified: 201 Created with the victim's triples replaced). The target now
  passes the same dataset-boundary gate as import and mapping execution, with
  the same admin bypass. (#347)
- **SHACL Studio introspection with no scope read the default graph.**
  `model-context` and `derive` with neither `?dataset=` nor `?graphs=`
  resolved to an empty graph list, which dropped the `GRAPH` wrapper and ran
  the introspection as a bare pattern over the default graph — which no
  per-graph ACL covers, and which holds LDP resources and anything loaded
  without a target graph — so any authenticated caller could enumerate it by
  omitting both parameters. No scope now means the caller's readable graphs
  (exactly what `/sparql` scopes to, so Studio can never see more than
  SPARQL), and an empty list matches nothing. (#347)
- **Graph Store reads ignored explicit graph ACL grants, and served the
  default graph to anyone.** The read check consulted only dataset-derived
  visibility while the SPARQL path also honours `graph_acl` read grants, so
  one grant meant rows over `/sparql` and 401 over `/store` — though
  docs/security.md presents graph ACLs as covering both. And the check ran
  only when `?graph=` was named: `GET /store` (or `?default`) fell through
  with no check and dumped the default graph to any caller, anonymous
  included — a test asserted exactly that and passed. Reads use the same
  accessible-graph set as queries; the default graph is admin-only. (#347)
- **Triple-level security labels never matched, and failed open.** The admin
  API stored terms exactly as sent (`http://ex/s`) while the filter's lookup
  keys are N-Triples (`<http://ex/s>`), so no label ever withheld anything —
  a labelled salary triple was served in full to a reader without access to
  its label graph. Both lookups also turned a database error into "no labels
  exist" and served every quad unfiltered. Terms are canonicalised on write
  and a migration canonicalises existing rows (bare IRIs in subject and
  predicate, and objects that look like absolute IRIs; bare literal text is
  left alone), the lookups withhold and log on error, the admin form's
  missing required `graph_iri` field is added, and docs/security.md now says
  what is true: only the Graph Store path is label-filtered, not SPARQL
  results — data that must be unreachable over `/sparql` needs a named graph
  and a graph ACL. (#347)
- **The endpoint ACL applied to six routes and never to anonymous callers.**
  `endpoint_acl_guard` was mounted on the `/api/browse/*` routes only, while
  the admin UI and docs/security.md present it as endpoint access control
  over any path pattern: a deny rule on `/sparql` or `/api/admin/**` was
  accepted, listed back and did nothing (verified with three deny rules, all
  200). Its anonymous branch returned allow unconditionally, so no rule could
  restrict public access on exactly the routes where that matters. The guard
  is now mounted alongside every auth route layer; anonymous callers match
  the reserved principal `('role', 'public')`; default-allow is unchanged (a
  request matching no rule proceeds, and role/scope middleware still applies)
  and deny rules bind admins too. Because a bad rule now reaches the whole
  API rather than six routes, `ENDPOINT_ACL_ENFORCE=false` disables
  enforcement outright as an operator escape hatch. (#347)
- **The Spark guard exempted whatever the client labelled "assistant".** The
  chat handlers filtered assistant messages out before screening, and the
  client submits the entire transcript on every turn, so labelling a message
  `assistant` exempted unlimited content from `max_message_chars`,
  `max_messages`, `max_total_chars` and the blocklist. Size caps and the
  blocklist now cover every message; the injection heuristics still skip
  assistant turns, for the real reason — the model quotes retrieved data back
  to the user, so "ignore previous instructions" in the *data* would block
  every following turn. The guard also covers `/api/llm/feedback` and
  saved-query repair, and repair validates the model's SPARQL before it can
  be persisted, reporting `valid`/`parseError` instead of writing an
  unparseable revision live. (#347)
- **The remote allowlist matched raw string prefixes.** `is_allowed` was
  `url.starts_with(entry)`, so with
  `OTS_REMOTE_ALLOWLIST=https://sparql.example.org` both
  `https://sparql.example.org@evil.net/` (the entry becomes the username,
  `evil.net` the host) and `https://sparql.example.org.evil.net/` passed —
  and federation then minted an identity assertion with `aud` = the
  attacker's origin and posted it there as a bearer token. Entries and
  candidates are parsed: scheme, host (case-insensitive) and port must be
  equal, the URL may carry no userinfo, and the entry's path must be a prefix
  of the URL's path on segment boundaries; malformed entries are dropped with
  one warning each, unparsable candidates are refused. The allowlist and
  federation are new in this release: no released version carried the bug.
  (#347)
- **Private graphs leaked through container export and LDES publishing.** A
  graph marked private is visible only to principals who can write its
  dataset, but ICDD export dumped every registered graph of any dataset the
  caller could access (an anonymous caller on a public dataset received its
  private graphs), and LDES seeding and change capture published private-graph
  entities as stream members served to anyone who could read the dataset.
  Export drops private graphs unless the caller can write the dataset;
  private graphs are never published to a stream (members published before a
  graph was marked private are not retracted — noted as a follow-up). Both
  features are new in this release: no released version was affected. (#347)
- **Full-text search hits are filtered by the caller's read scope.** The text
  index stored no graph IRI, and an expanded `text:search` can be nothing but
  a `VALUES` clause — no triple pattern for the query's `FROM` scoping to
  constrain — so a guest could enumerate subjects whose literals matched in
  private graphs. Hits are filtered by the caller's readable graphs inside the
  index, and Spark's evidence lookup passes the same scope rather than
  filtering afterwards (hits from unreadable graphs no longer consume its
  evidence slots either). (#293, #294)

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
