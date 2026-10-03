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
- **W3C SPARQL 1.2 and RDF 1.2 test suites run in CI.** `sparql/sparql12`
  and the N-Triples, N-Quads, Turtle, TriG and RDF/XML suites of `rdf/rdf12`
  (with the `rdf/rdf11` suites they include) from w3c/rdf-tests are vendored
  unmodified under `tests/fixtures/w3c-sparql12/` and
  `tests/fixtures/w3c-rdf12/`, and run by `tests/w3c_sparql12_manifests.rs`
  (every query on the engine and again through the in-memory mirror) and
  `tests/w3c_rdf12_manifests.rs` (every file through the upload path) as
  two-way ratchets. They are subsets of W3C test suites, used under the W3C
  3-clause BSD licence for development and bug tracking, so no score is
  published; `docs/conformance/sparql12.md` and `docs/conformance/rdf12.md`
  describe the runs and list the known gaps. `tests/sparql12_conformance.rs`
  now runs every pin on both read paths and adds `VERSION`, the `LANGDIR`
  family, `~` / `{| |}` in updates and queries, duplicate `VALUES` variables,
  `LATERAL` with a per-row `LIMIT` and `ADJUST`.
- **JSON-LD remote contexts.** Every JSON-LD parse — uploads, imports, the
  Graph Store, LDP, seed bundles, LDES pages — now resolves an `@context` named
  by IRI through a document loader; before, any such document failed to parse.
  The W3C contexts of ActivityStreams 2.0, CSVW, LDP and ODRL 2.2 are bundled
  and resolve offline; any other context is fetched only from a URL in
  `OTS_REMOTE_ALLOWLIST` (deny by default), following redirects and `Link`
  alternates inside the allowlist, capped at `OTS_JSONLD_CONTEXT_MAX_BYTES`
  (1 MiB) and cached for an hour. See `docs/formats.md`.
- **The W3C json-ld-api toRdf and fromRdf sections run in CI**
  (`tests/w3c_jsonld_api_manifests.rs`, vendored unmodified under
  `tests/fixtures/w3c-jsonld-api/`) as a regression ratchet. They are a subset
  of a W3C test suite, so no score is published; the known gaps are in
  `docs/conformance/jsonld.md`.
- **SKOS-aware inferencing and integrity checking.** The standards matrix
  promised SKOS-aware inferencing, and there was none: `skos.ttl` was only a
  bundled vocabulary. A dataset can now select the `skos` entailment regime —
  OWL 2 RL over its conformance layer with the bundled W3C SKOS schema as a
  premise, the schema's own closure pruned — and a built-in *SKOS integrity*
  shape graph (`urn:system:shapes:skos-integrity`) checks the SKOS Reference's
  integrity conditions S9, S13, S14, S27, S36, S37 and S46. `docs/standards.md`
  grades SKOS *Full*; see `docs/reasoning.md#the-skos-regime`.
- **Full-text search graded as a feature.** `docs/standards.md` grades
  *SPARQL + full-text search (Tantivy)* — not a standard — as *Full*, now that
  the index follows every write (see *Fixed*).
- **W3C entailment corpora.** The RDF 1.1 Semantics test cases (`rdf-mt`) and the
  SPARQL 1.1 entailment-regime section are vendored unmodified
  (`tests/fixtures/w3c-rdf-mt/`, `tests/fixtures/w3c-sparql11/entailment/`) with
  runners (`tests/w3c_rdf_mt_manifests.rs`,
  `tests/w3c_sparql11_entailment_manifests.rs`), and the approved OWL 2 test cases
  of the RL profile run through `tests/w3c_owl2_rl_manifests.rs`. Each is a
  two-way known-failure ratchet; no score is published (W3C test-suite policy).
- **ShEx 2.1.** The ShEx engine is rewritten to the ShEx 2.1 specification
  (`src/shex/`). ShExC is parsed to the full 2.1 grammar (imports, start and
  start actions, `EXTERNAL`, node constraints combined with shapes, value-set
  stems, ranges and exclusions for IRIs, literals and languages, `/regex/`
  patterns, triple-expression labels and inclusions, bracketed groups with
  cardinalities, annotations, semantic actions); ShExJ (2.1 and `ShapeDecl`
  layouts) and ShExR are read too. Validation partitions each node's
  neighbourhood between the triple constraints and the remainder, decides
  recursion as a greatest fixpoint per strongly connected component and
  negation by strata, and checks typed terms (XSD lexical forms and ranges,
  exact decimals, code-point lengths, XPath regular expressions). The
  endpoints accept the ShapeMap language (`<n>@<S>`, `{FOCUS p o}@<S>`,
  `@START`) and ShapeMap JSON as well as the original map, and ShExJ
  schemas (`schema_format`, `base`). `IMPORT <g>` is resolved only from a
  named graph the caller may read, holding ShExR — never over the network.
  Semantic actions run only the shexTest Test extension; no action code is
  executed. ShEx 2.next (`EXTENDS`, `ABSTRACT`) is refused. Guide:
  `docs/shex.md`.
- **shexTest suite, vendored.** `tests/fixtures/shextest/` (validation,
  schemas, negative syntax and structure, pinned at shexTest fc784a95, W3C
  Software and Document License, see its `PROVENANCE.md`) and the runner
  `tests/shextest_conformance.rs`, a two-way ratchet that skips only tests
  tagged with a ShEx 2.next trait and also checks every validation case
  through the store.
- **VoID per dataset.** Each dataset in the catalogue is described over the
  graphs the caller may read with `void:distinctSubjects`,
  `void:distinctObjects`, `void:properties`, `void:classes`,
  `void:documents`, class and property partitions (`void:classPartition`,
  `void:propertyPartition`), `void:vocabulary`, `void:exampleResource`,
  `void:feature`, `void:dataDump` and `void:sparqlEndpoint`; a graph whose
  role is `linkset` is a `void:Linkset` with its link predicates and targets.
  Partitions are never computed for the store-wide aggregate. New settings
  `OTS_VOID_PARTITION_LIMIT` (partitions listed per kind, default 100) and
  `OTS_VOID_PARTITION_MAX_TRIPLES` (no partitions above it, default
  5,000,000); a value that is not a whole number stops the server at startup.
- **The official DCAT-AP shapes run in CI.** `tests/dcat_conformance.rs`
  validates the catalogue against SEMIC's DCAT-AP 3.0.1 shapes (vendored in
  `tests/fixtures/semic-dcat-ap-3.0.1/`, CC BY 4.0) and Geonovum's DCAT-AP-NL 3
  shapes (fetched at a pinned commit by
  `tests/fixtures/geonovum-dcat-ap-nl-3/fetch.sh`, sha256-checked) and
  asserts no violation; `tests/dcat_ap_http.rs` validates the served
  catalogue against the SEMIC shapes instead of a hand-written subset.
- **DCAT 3 versions, data services, coverage and catalogue records.** The
  dataset catalogue (`/.well-known/void`) describes each released (published
  or deprecated) dataset version as a DCAT 3 §11 `dcat:Dataset` —
  `dcat:isVersionOf`, `dcat:version`, `dcat:previousVersion`, `dct:issued`,
  `adms:versionNotes`, a TriG download — linked by `dcat:hasVersion` and, for
  the newest, `dcat:hasCurrentVersion`. The SPARQL endpoint (and the OGC API
  when a dataset has geometry) is a `dcat:DataService` with
  `dcat:servesDataset`, publisher, contact point and access rights. Datasets
  gain temporal coverage (`temporal_start` / `temporal_end` → `dct:temporal`)
  and an update frequency (`accrual_periodicity`, a code or IRI of the EU
  frequency table → `dct:accrualPeriodicity`) in the API, the OpenAPI document
  and the metadata dialog. Under `dcat-ap` / `dcat-ap-nl` every dataset has a
  `dcat:CatalogRecord`. New settings `CATALOG_CONTACT_NAME` /
  `CATALOG_CONTACT_EMAIL` (the contact point of the catalogue and its data
  services, and the fallback for a dataset with none) and
  `CATALOG_PUBLISHER_TYPE` (ADMS publisher type). `dcat:DatasetSeries` is not
  used: the product has no series concept. See `docs/dcat.md`.
- **`GET /sparql` without a query returns the SPARQL 1.1 Service
  Description** (SPARQL 1.1 Service Description §2), scoped to the caller like
  the one at `/`; the catalogue names it as the endpoint's
  `dcat:endpointDescription`.
- **OPM calculations; OPM graded Full.** `opm:Calculation`s are defined at
  `POST /api/datasets/:id/properties/calculations` (inferred property,
  argument paths such as `?foi ex:partOf/ex:height ?h`, an expression, and
  optional `opm:foiRestriction` / `opm:pathRestriction`) and run only on
  request, as OPM's REST guidance describes: `POST …/calculations/:calc`
  derives the property for every feature of interest that has the arguments
  and lacks the property, `PUT` recomputes the derived states whose argument
  states were outdated, and `GET …/:calc/outdated` lists them. A derived state
  is `opm:Derived` with the `opm:expression` and `prov:wasDerivedFrom` an
  `rdf:Seq` of the argument states; one run is one commit. Paths,
  restrictions and expressions are parsed with spargebra and allow-listed (no
  `SERVICE`, `GRAPH`, `FILTER`, `EXISTS`, subqueries, aggregates or
  non-deterministic functions), and the queries are built from the parsed
  form: matching reads only the dataset's own data graphs, capped by
  `OTS_OPM_CALC_MAX_ROWS` (default 10 000) and the query timeout, and the
  expression is evaluated in an empty scratch store. Calculations travel
  through OPM export and import (prefixed names resolve with the document's
  prefixes). `docs/standards.md` grades OPM *Full* (the project's own
  assessment; OPM has no test suite).
- **OPM property lifecycle and exchange.** Property states gain the rest of
  the Ontology for Property Management: `POST /api/datasets/:id/properties/delete`
  records a current `opm:Deleted` state with no value and removes the plain
  triple; `…/restore` brings back the last value (`prov:wasRevisionOf`);
  `reliability` accepts `required` (`opm:Required`) and states take
  `opm:documentation` IRIs. `GET /api/datasets/:id/properties` lists the
  properties of an item, of a property kind or of the whole dataset — latest
  state, full history or the state at a time (an item snapshot) — filtered by
  reliability, deletion and derivation. `GET …/properties/export` writes
  canonical OPM (`<item> <kind> <property>`) in Turtle, N-Triples, JSON-LD or
  RDF/XML and `POST …/properties/import` reads it back (state IRIs kept,
  duplicates skipped, states without `prov:generatedAtTime` rejected); every
  read also accepts canonical OPM loaded straight into a dataset graph, and
  `schema:value` in both the `http` and `https` scheme. The OPM profile shapes
  ship as the `opm-profile` seed bundle, at `GET /api/properties/profile`, and
  run over a states graph at `GET …/properties/validate`. All property routes
  are in the OpenAPI document.
- **`ALERT_SMTP_TLS`, and alerting falls back to the account-email relay.**
  Alert email can now choose its transport security (`none`, `starttls` or
  `implicit`, as for `SMTP_TLS`); unset, it stays implicit TLS. When
  `ALERT_SMTP_HOST` is unset, alerting uses the `SMTP_*` relay as a whole
  (host, port, `SMTP_USERNAME`, `SMTP_PASSWORD`, `SMTP_TLS`) and never mixes
  it with `ALERT_SMTP_*`, so the account relay's credentials are not sent to a
  separate alert host. The sender falls back from `ALERT_SMTP_FROM` to
  `SMTP_FROM`. Ops alerts still go only to `ALERT_SMTP_TO`. Saved-query
  breakage notices to dataset owners use the same relay, so a deployment with
  only `SMTP_*` set now delivers them.
- **Identity policy in the web UI.** The dataset page and the organisation page
  have an *Identity policy (owl:sameAs)* card over the existing
  `GET/PUT/DELETE /api/{datasets|organisations}/:id/identity` routes: it shows
  the policy in force and where it comes from (the dataset, its organisation,
  or the built-in default), and lets dataset editors and organisation admins
  pick `off`, `narrow` or `full`, or go back to inheriting.
- **A not-found page.** A path no route serves now shows "Page not found" with
  links back, instead of an empty page shell.
- **`GET /health` lists the standards the build serves** (`capabilities`, from
  the compiled feature set). The Home page shows these as its chips instead of
  a hard-coded list.

- **`OTS_OIDC_IDP_TOKEN_POLICY` and `OTS_OIDC_IDP_WRITE_SCOPES`.** What an access token from
  an external IdP (OIDC resource-server mode) may do is now a setting, with the
  same values as `OTS_OIDC_SESSION_POLICY`: `session` (default), `scoped`
  (write only when the token's `scope` or `scp` claim carries `write`,
  `admin` or a value listed in `OTS_OIDC_IDP_WRITE_SCOPES`) and `full`. It is separate
  from `OTS_OIDC_SESSION_POLICY` because the two token sources are issued to
  different clients. `docker-compose.yml` passes both through.
- **SP-initiated SAML sign-in (`saml` feature).** A SAML button on
  the login page now goes to `GET /api/auth/saml/{slug}/login`, which redirects
  to the IdP's SSO URL with an AuthnRequest (HTTP-Redirect binding) and binds
  the attempt to the browser with a short-lived `saml_state` cookie
  (`SameSite=None; Secure` whenever `BASE_URL` is https). The ACS now accepts only a
  signed response that answers that request (`InResponseTo`), from that
  browser, once and within 10 minutes. It then redirects to the SPA's
  `/oauth/callback` page instead of returning the tokens as JSON.
  IdP-initiated responses are refused. Before this, the ACS passed a
  confirmation-method URN where request IDs belong, so no SAML sign-in could
  succeed, and the login button led to a `404`. The store now identifies itself
  to the IdP with its own entity ID, the SP metadata URL
  `…/api/auth/saml/{slug}/metadata`, rather than reusing the IdP's entity ID.
  Re-register the SP at the IdP with that entity ID. See `docs/auth.md`.
- **SAML 2.0 completed: IdP metadata import, encrypted assertions, signed
  requests, Single Logout.** Per provider, a new `saml_config` holds our entity
  ID override, the IdP's logout URL, the NameID format (persistent by default;
  a transient NameID needs a subject attribute), attribute names (defaults now
  include the `urn:oid:` names and the Microsoft role claim), the IdP-initiated
  policy (off by default; when on, each assertion is accepted once), clock
  skew, request signing, an encrypted-assertion requirement and a contact.
  `POST /api/admin/oauth/saml-metadata` reads IdP metadata from an https URL
  (no redirects) or pasted XML into the entity ID, SSO/SLO URLs and every
  signing certificate; `idp_certificate` now holds several certificates, so an
  IdP key rollover trusts old and new at once. Each provider gets its own SP
  key pair, stored like an OAuth client secret, published in our metadata with
  `use="signing"` and `use="encryption"` (samael's metadata labels both
  `signing`; ours is written by the store) and managed under
  `/api/admin/oauth/providers/{id}/saml/keys` (add, activate, delete: key
  rollover). Assertions encrypted with AES-GCM or AES-CBC and RSA-OAEP decrypt;
  AuthnRequests can be signed. `GET|POST /api/auth/saml/{slug}/slo` handles
  Single Logout from the IdP (signed LogoutRequest → the named sessions'
  refresh-token families are revoked → signed LogoutResponse), and signing out
  of a SAML session revokes its family and returns `{"saml_logout_url"}` from
  `POST /api/auth/logout`, which the web UI follows. Access tokens issued
  before a logout stay valid until they expire. The admin form's SAML section
  covers all of it. Tests: a fake IdP in `tests/security_federated.rs`.
- **OWL 2 QL: DL-Lite_R closure, ground materialisation, consistency and
  existential query rewriting.** The `owl2-ql` regime of
  `POST /api/reasoning/materialize` and of a dataset's entailment now writes
  every entailed class membership and property assertion over the data's
  individuals (it wrote only the subclass/subproperty closure, so
  `?entailment=owl2-ql` said nothing about individuals). The TBox is closed
  over basic concepts and roles with qualified existentials on the right,
  intersections, complements, disjoint classes and properties,
  symmetric/asymmetric/reflexive/irreflexive properties and data properties;
  unsatisfiability propagates through negative inclusions, and an
  inconsistency fails the run with its rule (`ql-cls-disjoint`,
  `ql-prp-irp`, …). On `/sparql?entailment=owl2-ql` (and a dataset whose
  regime is `owl2-ql`) the query's blank nodes are rewritten existentially, so
  `ASK { ex:ann ex:hasChild [ a ex:Person ] }` is true when the schema says
  every parent has a child, with one solution per binding of the variables;
  the TBox is cached until the next write and a query without blank nodes is
  untouched. Reasoning reports gain `ignored_axioms` and `ignored_sample`,
  the axioms outside the profile that were not used. See `docs/owl2-ql.md`.
- **OWL 2 RL: `eq-ref` on request.** `Owl2RLReasoner::with_eq_ref(true)`, or
  `"eq_ref": true` in the body of `POST /api/reasoning/materialize`, also writes
  `x owl:sameAs x` for every subject, predicate and non-literal object. It is
  off by default (about one triple per term); `sameas-off` skips it.
- **OWL 2 DL reasoner sidecar (OWL API + HermiT).** `sidecars/reasoner/` is a
  small Java service that speaks the sidecar protocol v1 (`POST /v1/reason`,
  `POST /v1/check`, bearer token) and is published as its own image,
  `ghcr.io/philipperenzen/open-triplestore-reasoner`; `docker compose
  --profile reasoner up` starts it on the internal network only, and
  `OTS_DL_BACKEND=sidecar` with a shared `OTS_REASONER_TOKEN` points the server
  at it. It parses with the OWL API, never follows `owl:imports`, types
  undeclared properties by use the way the server's own mapping does, applies
  the OWL 1 DL compatibility rules of the RDF mapping (Tables 14, 15 and 18),
  checks the OWL 2 DL profile, and reasons with HermiT under a time limit
  (interrupted at the deadline: 504, result unknown). A materialisation reports
  class and property hierarchies, unsatisfiable classes, types, `owl:sameAs`
  and property assertions about named entities; an inconsistency comes with a
  minimal inconsistent subset of the axioms when the input is small enough
  (`OTS_REASONER_EXPLAIN_MAX_AXIOMS`). Entailment is checked by reduction to
  class satisfiability, because HermiT's own entailment check answered `false`
  for entailed class assertions until the ABox had been realised. HermiT and
  the OWL API ship unmodified as separate jars (LGPL-3.0; `NOTICE`, new
  `LICENSES/LGPL-3.0.txt`, `GPL-3.0.txt`, `LGPL-2.1.txt`, `EDL-1.0.txt`, and a
  generated `/app/THIRD-PARTY.txt` in the image).
- **W3C OWL 2 test cases in CI.** The approved cases of the OWL 2 Test Case
  Repository are vendored unmodified (`tests/fixtures/w3c-owl2/`, W3C
  Document License), and `tests/w3c_owl2_dl_manifests.rs` runs the OWL 2 DL /
  Direct Semantics ones through `POST /api/reasoning/check` against the
  sidecar, with a known-failures list and a pass floor. The conformance job
  builds and starts the sidecar and also runs new live tests in
  `tests/owl2_dl_conformance.rs` (existential witnesses, case splits,
  nominals, property assertions and `owl:sameAs`, facet inconsistencies,
  checks). No score is published for the suite
  ([docs/conformance/owl2-dl.md](docs/conformance/owl2-dl.md)).
- **SHACL-AF node expressions, expression constraints and target types.** The
  seven node-expression kinds of the SHACL Advanced Features Note — `sh:this`,
  constants, path (`sh:path` / `sh:nodes`), filter shape, intersection, union
  and function expressions — are evaluated against the run's data graphs.
  `sh:TripleRule` subjects, predicates and objects are node expressions (a rule
  derives a triple per combination of their results), and `sh:expression`
  checks that its expression produces exactly `true` for every value node.
  `sh:SPARQLTargetType` targets run their type's query with the target's
  parameter values bound as terms, and a shape whose only target is
  `sh:target` is found without an `rdf:type`. See `docs/shacl.md`.
- **`sh:SPARQLFunction` bodies read data, and `sh:ask` bodies work.** A
  function called from a shapes run reads that run's data graphs, from the same
  snapshot, confined to them whatever `FROM`, `FROM NAMED` or `GRAPH` the body
  names; it used to run on an empty store and return unbound for any body that
  read data. Arguments are bound to the body's variables as RDF terms (binding
  `$x` used to rewrite `$xy` too), parameters without `sh:order` are ordered by
  the local names of their paths (SHACL-AF §5.2), `sh:prefixes` follows
  `owl:imports`, and nested calls stop at 16 levels. A function called from
  `/sparql` still sees no data.
- **TopQuadrant's SHACL-AF tests run in CI.** The `expression`, `function`,
  `rules` and `target` tests of TopQuadrant/shacl (Apache-2.0, commit
  `6687b48`) are vendored under `tests/fixtures/shacl-af-topquadrant/` and run
  by `tests/shacl_af_corpus.rs` as a two-way ratchet: 9 of 10 cases pass
  (`docs/conformance/shacl.md`).
- **RDF Patch logs per dataset.** Each dataset has an
  [RDF Patch log](https://afs.github.io/rdf-delta/rdf-patch-logs.html) at
  `/api/datasets/{id}/log`: `POST` appends a patch (exactly one `H id`, at
  most one `H prev`, which must name the latest entry, else `409` and nothing
  changes), applying it like `POST …/patch` and appending it only when that
  succeeds; `GET …/log`, `…/log/init` (version 0 as TriG), `…/log/current`
  and `…/log/patch/{version|id}` read it. Every version cut also appends the
  diff from the previous cut as a chained patch. SPARQL, Graph Store and
  import writes are not journaled. Entries that change a graph the caller may
  not read are withheld. See `docs/versioning.md#patch-logs`.
- **A prefix table per dataset.** `GET`/`PUT /api/datasets/{id}/prefixes` and
  `PUT`/`DELETE …/prefixes/{label}`. Each version records the table it was
  cut with and a restore brings it back. The dataset's Turtle and TriG
  exports (the Graph Store read of its graphs, a version's `/data`) declare
  it ahead of the prefix registry.
- **LDES publisher context and endpoints.** The event stream declares a
  generated `tree:shape` (one IRI `dct:isVersionOf`, one `xsd:dateTime`
  `dct:created`; open otherwise), `ldes:pollingInterval` (60 s by default, set
  with `"polling_interval"` on `PUT /api/datasets/:id/ldes`), and
  `ldes:versionDeletePath rdf:type` / `ldes:versionDeleteObject as:Delete`.
  Every stream document carries an `ETag` and answers a matching
  `If-None-Match` with `304`. Member IRIs dereference
  (`GET /api/datasets/:id/ldes/members/:m`, immutable). Stream documents render
  on the blocking pool behind a gate that answers `429` with `Retry-After`
  when a stream (16) or the server (64) has too many in flight
  (`OTS_LDES_MAX_IN_FLIGHT_PER_STREAM`, `OTS_LDES_MAX_IN_FLIGHT`).
- **LDES client: retries, conditional fetches, resumable state.** A sync
  retries `408`, `425`, `429`, `500`, `502`, `503` and `504` with exponential
  back-off and jitter, honouring `Retry-After` (`OTS_LDES_RETRIES`, default
  4; `OTS_LDES_MAX_RETRY_WAIT_SECS`, default 60 — a longer `Retry-After`
  fails the sync instead of holding it), and aborts on any other error status
  (LDES 1.0 §3.3). It asks for TriG, N-Quads, Turtle, N-Triples and JSON-LD.
  It remembers per stream the pages it processed as immutable (never fetched
  again), the `ETag` and relations of mutable pages (sent as `If-None-Match`;
  a `304` follows the remembered relations), and skips a node whose relations
  bound it below the bookmark on the timestamp path. The report adds
  `stream`, `root_node`, `polling_interval`, `shapes`, `nodes_not_modified`,
  `nodes_skipped_immutable`, `nodes_pruned`, `retries` and
  `versions_superseded`.
- **3D Tiles feature cap: `OTS_TILES3D_MAX_FEATURES`** (default 10 000). The 3D
  Tiles tileset is still a single tile holding one GLB, so the GLB now carries at
  most that many features, the first in IRI order. A capped tileset reports
  `asset.extras.truncated` (`served`, `total`, `maxFeatures`), the GLB an
  `X-Tiles3d-Truncated: served/total` header, and the server logs a warning.
- **SWRL: every §8 built-in, data ranges and class expressions — graded Full.**
  - **Built-ins.** All 79 `swrlb:` built-ins of the SWRL submission §8
    (comparisons, math, boolean, strings, dates/times/durations, URIs, lists)
    are evaluated natively, and they bind variables: `swrlb:add(?z, ?x, 1)`
    computes `?z`, the date/duration/URI constructors split a bound value
    into its components, `tokenize`, `member` and `sublist` bind one value
    per solution, and `add`/`subtract`/`unaryPlus`/`unaryMinus`/`booleanNot`/
    `equal` solve for one unbound operand. A pattern with infinitely many
    solutions is refused with `400`, naming the built-in. Constructed lists
    are minted as deterministic `urn:ots:swrl:list:<hash>` nodes.
  - **`DataRangeAtom`** is evaluated natively: datatypes by value space,
    facets, `DataOneOf` (which also enumerates), union, intersection,
    complement. Every syntax reads it (`xsd:integer(?v)` in SWRLAPI).
  - **Class-expression atoms** become auxiliary classes
    `urn:ots:swrl:aux:<hash>` equivalent to the expression, which the regime
    the rules run with materialises. `POST /api/swrl/execute` takes a
    `regime` that runs the rules and the regime to one joint fixed point
    (response adds `regime`, `rounds`, `regime_triples`); stored dataset rules
    use the dataset's regime.
  - New reference page `docs/swrl.md` (`/docs/swrl`); the demo rules dataset
    gains a rule with a binding built-in.
- **SWRL reads five more rule syntaxes and runs over datasets.**
  - **Syntaxes.** `POST /api/swrl/execute` now takes every `format` below.
    - `rdf`: the SWRL RDF syntax (`swrl:Imp` with argument lists), in any
      serialisation given as `rdf_format`.
    - `functional`: OWL 2 functional-syntax `DLSafeRule`.
    - `swrlapi`: the SWRLAPI human-readable syntax. Prefixes come from the
      request's `prefixes`, then the server's prefix registry.
    - `ruleml`: the SWRL §4 RuleML XML syntax.
    - OWL/XML (`xml`, now also `owlxml`) reads `Prefix` declarations,
      `abbreviatedIRI` and `xml:base`.
  - **Dataset scope.** A request may name a `dataset` and `source_graphs`.
    Rule bodies then read those graphs, with the read checks of
    `/api/reasoning/materialize`, instead of the unnamed default graph.
    Without a `target_graph`, a dataset run writes to the dataset's inference
    graph, which only its writers may fill. The response adds `target_graph`
    and `sources`.
  by `tests/shacl_af_corpus.rs` as a two-way ratchet, validation cases at full
  report equality like the W3C suite: 9 of 10 cases pass. The tenth,
  `target/sparqlTarget-001`, expects TopBraid's fallback to the file's Turtle
  prefixes, which the SHACL prefix mechanism (§5.2.1) does not have; it is
  counted as expecting behaviour outside the spec, and passes when validation
  fails as the spec requires (`docs/conformance/shacl.md`).
- **SHACL-AF result annotations (`sh:resultAnnotation`).** A `sh:sparql`
  constraint or a component validator can declare properties to add to its
  results (SHACL-AF §4): the value of a variable of the solution, or the
  annotation's `sh:annotationValue` when it is unbound. They are written into
  the RDF report as properties of the result node, typed, and listed in the
  JSON result and the write gate's 422 body as `annotations`
  (`[{"property", "value"}]`); a result without annotations has no such key,
  so other JSON is unchanged. An ill-formed annotation fails the shapes graph.
- **GeoSPARQL 1.1: the remaining query functions and spatial aggregates.**
  `geof:dimension`, `coordinateDimension`, `spatialDimension`, `is3D`,
  `isMeasured`, `isEmpty`, `isSimple`, `geometryType` (an `sf:` or `gml:` IRI),
  `numGeometries`, `geometryN` (from 1), `minX` … `maxZ`, `centroid`,
  `boundingCircle` (Welzl's minimum bounding circle, drawn as a polygon around
  it), `concaveHull` (GEOS, target 0 to 1, default 0.5), `length` and
  `perimeter` (with units, geodesic for a linear unit on a geographic CRS), and
  the aggregates `aggBoundingBox`, `aggBoundingCircle`, `aggCentroid`,
  `aggConvexHull` and `aggConcaveHull` (one argument, default target 0.5: the
  SPARQL parser allows one expression per custom aggregate), with
  `aggUnion`'s CRS, serialisation and error rules. Req 39, 40 and 42.
- **GeoSPARQL: KML literals, `geof:asWKT`, `geof:asGML` and `geof:asKML`.**
  `geo:kmlLiteral` (KML 2.2 `Point`, `LineString`, `LinearRing`, `Polygon`,
  `MultiGeometry`; always CRS84; an empty literal is the empty geometry) is a
  geometry for every `geof:` function, `geo:asKML` is indexed and fed to the
  map viewer, and four serialisation functions write any geometry as WKT (its
  CRS as the prefix), GML 3.2 (its CRS as `srsName`, `srsDimension="3"` for
  Z), GeoJSON or KML (both reprojected to CRS84). GeoSPARQL 1.1 Req 19, 24 and
  30–34.
- **GeoSPARQL: a documented GML profile.** The GML reader now also takes
  `Envelope`, a standalone `LinearRing`, `Curve` of `LineStringSegment`s,
  `OrientableCurve`, `CompositeCurve`, `Ring`s of linear `curveMember`s,
  `Triangle`, `Rectangle`, `Tin`/`TriangulatedSurface`, `PolyhedralSurface`,
  `CompositeSurface`, `OrientableSurface`, `pointMembers`/`curveMembers`/
  `surfaceMembers`, `gml:coordinates` with `cs`/`ts`/`decimal`, and GML 2's
  `gml:coord`. The profile is listed in `docs/geosparql.md` (Req 22).
- **GeoSPARQL Query Rewrite Extension.** A triple pattern whose predicate is
  one of the 24 topological relations (`geo:sfWithin`, `geo:ehMeet`,
  `geo:rcc8ntpp`, …) now also matches where the geometries imply the relation,
  as the standard's rules give it: each side a feature through
  `geo:hasDefaultGeometry` or a geometry itself, its literal from `geo:asWKT`,
  `asGML`, `asGeoJSON` or `asKML`, decided by the `geof:` function of the same
  name. Results have set semantics (two serialisations of one geometry, or an
  asserted and a derived relation, match once), stay inside a `GRAPH` pattern,
  and apply in queries and in the `WHERE` clause of `DELETE`/`INSERT` updates.
  Variable predicates, property paths and `SERVICE` blocks are left alone.
  `OTS_GEOSPARQL_QUERY_REWRITE=off` turns it off (`src/geo/query_rewrite.rs`).
- **GeoSPARQL RDFS Entailment Extension: geometry class hierarchies as
  premises.** OGC's Simple Features vocabulary (`sf.ttl`, registry entry `sf`,
  Apache-2.0, bundled unchanged) and a GML 3.2.1 geometry class hierarchy
  (`gml.ttl`, registry entry `gml-geometries`), which Open Triplestore wrote
  from the GML 3.2.1 schema's substitution groups because OGC no longer
  publishes one, are seeded as public reference models. A dataset whose graphs
  use any GeoSPARQL term now reasons over them and the GeoSPARQL ontology,
  without declaring conformance to them; `GET /api/datasets/{id}/conformance`
  lists them as `vocabulary_premises`.
- **GeoSPARQL requirement matrix.** `docs/conformance/geosparql.md` maps every
  requirement of GeoSPARQL 1.0 and 1.1 to its status and tests, and GeoSPARQL
  1.0 has its own row in `docs/standards.md`.
- **RML-Core and RML-IO.** RML mappings may be written in the W3C Knowledge
  Graph Construction Community Group's vocabulary (`http://w3id.org/rml/`),
  alongside R2RML and the legacy RML namespace, mixed freely. New with it:
  RFC 9535 JSONPath iterators and references (`serde_json_path`), XPath 1.0
  with attributes, axes and declared namespaces (`sxd-xpath`), references that
  select several values (a term per value, templates as the cartesian product
  of their references' values), `rml:languageMap` and `rml:datatypeMap`,
  `rml:childMap` / `rml:parentMap` join expressions, the `rml:URI`,
  `rml:UnsafeIRI` and `rml:UnsafeURI` term types, a blank-node term map with
  no expression, `rml:RelativePathSource` / `rml:FilePath` sources and CSVW
  `csvw:Table` sources with their dialect (delimiter, quote, header rows,
  skipped rows, comment prefix, trim, `csvw:null`), `rml:encoding` (any WHATWG
  label; a byte-order mark wins), `rml:compression` (gzip, zip, tar.gz,
  tar.xz; a decompressed source over `OTS_RML_MAX_SOURCE_BYTES`, default
  256 MiB, is refused) and JSON Lines files. A JSON value in an RML-Core
  mapping carries its natural datatype (`xsd:integer`, `xsd:double`,
  `xsd:boolean`, per the RML-IO registry); the legacy vocabulary keeps reading
  JSON values as plain strings. An SQL timestamp or boolean takes its natural
  RDF lexical form (`2009-10-10T12:12:22`, `true`) in literals and templates
  (R2RML §10.2). RML-FNML, RML-CC, RML-LV, RML-star and RML-IO targets are not
  implemented, and a mapping that uses their terms is refused naming the
  module; a remote source is never fetched. The file-mapping endpoints accept
  source parts that are not UTF-8. Three vendored corpora run in CI: the
  RML-Core test cases (`tests/rml_core_conformance.rs`), the RML-IO source
  test cases (`tests/rml_io_conformance.rs`) and the CSV, JSON and XML cases
  of the legacy rml-test-cases (`tests/rml_legacy_conformance.rs`), with
  their licences and provenance beside them and in `NOTICE`.
- **The IFC lift writes an IDS projection.** Beside the building-topology
  graph, an IFC import (`POST /api/datasets/:id/import/ifc`) now writes
  `…/building/ids`: every instance with its exact, schema-qualified class
  (no subclass axioms, so a class target matches the class alone) and all
  its explicit attributes from per-schema IFC2X3 / IFC4 / IFC4X3_ADD2
  tables; resolved predefined types (type object first, IFC2X3 type mapping
  table included); property and quantity sets with type inheritance,
  occurrence override and values in the SI units IDS nominates; part-of
  edges per relation (aggregation, nesting, containment, groups,
  voids/fills); materials with every name and category in their sets; and
  classifications with their system and reference chain. The
  building-topology output is unchanged (`tests/ifc_lift.rs` checks it is
  the same with the projection on or off). `ConvertOptions::include_ids` and
  `ifc::convert_layers` expose it to library callers; the demo seed does not
  write it.
- **The buildingSMART IDS test corpus runs in CI.** `tests/buildingsmart_ids_conformance.rs`
  runs all 334 IDS + IFC cases of the buildingSMART IDS repository's test
  corpus through the IFC lift, the IDS importer and the SHACL validator, as a
  two-way ratchet with a known-failures list. The corpus (CC BY-ND 4.0) is not
  vendored: the runner downloads it from a pinned commit and checks every
  file against `tests/fixtures/buildingsmart-ids/MANIFEST.sha256`, and
  `OTS_TEST_IDS_CORPUS_REQUIRED=1` (set in CI) fails a missing download.
  Development results only, no score published, not a buildingSMART
  certification ([docs/conformance/ids.md](docs/conformance/ids.md)).
- **ICDD container validation.** Every container import now returns a
  `validation` report (`conforms`, `violations`, `warnings`, `findings[]` with
  severity, code, message, index node and archive entry), `strict=true`
  refuses a container with any violation (422, nothing stored), and
  `POST /api/containers/validate` validates an archive without storing it.
  For ISO 21597-1 the validator runs the project's own SHACL shapes, written
  from the Part 1 ontology restrictions (`src/containers/icdd_shapes.ttl`; ISO's
  SHACL annexes are not used), plus structural checks: one root `Index.rdf`,
  one container description, the three folders, the ISO ontology files, every
  listed file at its path, duplicate names, path escapes, checksums, link
  elements naming listed documents, and no extension of the ICDD classes in a
  Part 1 container.
- **`OTS_ICDD_ONTOLOGY_DIR`.** Point it at a directory holding ISO's
  `Container.rdf` and `Linkset.rdf` (downloaded from ISO's maintenance portal;
  the project does not ship them) and ICDD exports embed both unchanged.
  Without it an export carries the `ontology-resource-missing` warning: not
  Part 1-conformant for that reason.
- **Seed bundles: `[account]` and `[[groups]]`.** Two optional manifest keys,
  purely additive (a manifest without them behaves exactly as before).
  `[account]` (`username`, `email`, `display_name`, `password_env`) names the
  account the bundle's content is attributed to: created when missing, with
  the system role `user` (never higher) and as an admin of the bundle's
  organisation, its password the value of the environment variable
  `password_env` names or, without one, a password nobody knows; an existing
  account is used as it is. It owns the bundle's saved-query services, which no
  longer wait for an instance admin to exist, and is the creator
  (`dct:creator`) of every version the bundle's `[[data_models]]` publish.
  `[[groups]]` (`name`, `role` = admin | member | viewer, `members`) are
  matched by name inside the organisation and created when missing; members
  are resolved by username, an existing membership keeps its role, and a
  username that does not resolve yet is deferred to the next reseed. Because a
  version with a creator is no longer "creator-less with the bundle's notes",
  the bundle now marks every version it registers (`ver:seededBy
  seed-bundle:<id>`) and recognises its versions by that marker first — a
  version an earlier build registered is marked at the next reseed — so
  attribution cannot break the manifest's `public = false` or a licence record.
- **Per-resource access control for LDP, with Web Access Control.** Every
  resource under `/ldp/` now has an ACL at `R.acl` (`C.acl` or `C/.acl` for a
  container), in the Solid WAC model (`acl:Authorization`, `acl:accessTo`,
  `acl:default`, `acl:agent` / `acl:agentGroup` / `acl:agentClass`,
  `acl:mode`), advertised with `Link: <R.acl>; rel="acl"` and `WAC-Allow` on
  `GET`/`HEAD`. Agents are this store's principals by stable IRI
  (`urn:ots:user:…`, `urn:ots:org:…`, `urn:ots:group:…`, `urn:ots:role:…`),
  plus `acl:AuthenticatedAgent` and `foaf:Agent`; `GET /api/auth/me` returns
  the caller's `agent_iri`. A resource without an ACL inherits the nearest
  container's `acl:default`; whoever creates a resource owns it
  (`R.acl#owner`: Read, Write, Control). The root ACL is seeded once, open by
  default (`LDP_ROOT_ACL=open`: every signed-in user may read, write and
  append, as before) or closed (`LDP_ROOT_ACL=owners`: admins only), and is
  edited like any other. A `foaf:Agent` Read grant makes a resource readable
  without a token. Not in scope: WebID-TLS, Solid-OIDC, `acl:origin`. See
  `docs/ldp.md`, "Access control".
- **Repair proposals.** `POST /api/datasets/:id/repair` proposes a fix for
  what the dataset's rules determine, and never applies it. The rules are
  compiled from its SHACL Core shapes (`sh:hasValue`, `sh:class`, a
  `sh:minCount` an IRI can satisfy, a one-member `sh:in`, through `sh:node`
  and nested property shapes) and its OWL axioms (`owl:hasKey`, functional
  and inverse-functional properties, `someValuesFrom` and `minCardinality 1`
  restrictions), or authored as `ots:Rule`s. They run as a restricted chase
  over an in-memory copy of the dataset, and the answer is an RDF Patch plus
  a report that explains every line: rule, trigger, premises, the violation
  it answers. Missing values are minted as content-derived IRIs under
  `{base}/.well-known/genid/`: a run repeated before the apply mints the same
  IRIs, and one after it proposes nothing. Equal
  terms are merged (`owl:sameAs`, or rewritten with `ots:mergeMode
  ots:Rewrite`), and a merge the data forbids is reported as a conflict.
  Three opt-in policies make a declared choice: `closed-delete`,
  `maxCount-keep-lexmin`, `datatype-relabel`. Everything else is listed
  report-only with its reason. The same dataset state gives byte-identical
  patch text. Budgets (rounds, nulls, lines, time) end a run with a partial
  proposal, never an error. Kept proposals (`persist: true`) are files under
  `{data_dir}/repair-proposals/`, listed, read page by page, rejected, or
  applied with `POST …/repair/proposals/:pid/apply`. The apply checks the
  proposal's base (`409` and `superseded` when the dataset moved), runs the
  write gates (`422`), and records one commit whose `metadata.repair` names
  the proposal. `POST /api/datasets/:id/patch` gains the base check as two
  opt-in preconditions over the dataset's graphs: `?if-base-commit=` (or
  `If-Match`) and `?if-base-sequence=` with the change log. Without them it
  behaves as before. SHACL
  Studio's assistant takes `task: "repair"`: it sends the model what no rule
  repaired, runs the rules the model answers with under a heuristic guard
  (smaller budget, nothing destructive, only predicates already in use), and
  keeps their proposal for review. Settings: `OTS_REPAIR_MAX_QUADS`,
  `OTS_REPAIR_CONCURRENCY` (default 1), `OTS_REPAIR_PROPOSAL_TTL_DAYS` (days,
  default 30). See `docs/repair.md`.
- **Model versions and the datasets that depend on them are linked.** A dataset
  version now records the model version its instances were pinned to when it was
  cut (`conforms_to_model` / `conforms_to_version` on `DatasetVersion`, stored as
  `ver:conformsToModel` / `ver:conformsToVersion` plus `dct:conformsTo` on the
  model-version IRI), so a published dataset version keeps saying which model
  version it conformed to after the dataset moves on. `GET /api/datasets/:id/conformance`
  reports the model's `latest_published`, whether the dataset is `pinned`, and
  `update_available` when a newer model version has been published than the pinned
  one. New `GET /api/models/:id/dependents` lists the (visible) datasets that declare
  conformance to a model, each with its pinned/effective version, whether it is
  behind, and its latest published dataset version. Publishing a model version is
  recorded on the model's commit log. Studio shows an "update available" badge on
  the dataset page and a "used by N datasets" card on the model page — the
  store-side half of a model-update procedure: an external validation service
  re-validates a dataset against the new version, collects or corrects what it
  asks for, and re-pins; the store now tells everyone which datasets that applies to.

- **OWL 2 DL backends, checks and background runs.** The `owl2-dl` regime
  now runs on the backend `OTS_DL_BACKEND` names:
  - `konclude`: a Konclude binary (`OTS_KONCLUDE_BIN`), driven through OWLlink
    and SPARQL files with a time limit;
  - `sidecar`: an HTTP reasoner service (`OTS_REASONER_URL`,
    `OTS_REASONER_TOKEN`) speaking the version-1 protocol in `docs/owl2-dl.md`;
  - `native`: the in-process rules.

  `OTS_REASONER_TIMEOUT_SECS` (default 300) and `OTS_REASONER_MAX_TRIPLES`
  (default 1,000,000) bound every external call. The input is mapped from RDF
  to OWL 2 (the reverse OWL 2 RDF mapping) and checked against the OWL 2 DL
  typing constraints and global restrictions before any backend sees it.
  - Input outside OWL 2 DL is a 422 `{in_profile: false, violations}`.
  - No backend, or an unreachable one, is a 503.
  - A run past the time limit is a 504 with `"result": "unknown"`.
  - Too much input is a 413; a backend failure is a 502.
  - A failing external backend never falls back to the native rules.
  - Only triples about named entities reach the target graph.
  - Reports add `backend`, `backend_version`, `complete` and `warnings`.

  New `POST /api/reasoning/check` answers consistency, entailment,
  satisfiability and profile questions with `true`, `false` or `unknown`
  (OWL 2 Conformance §2.2), as a 200; an inconsistent premise carries the
  materialisation 422's `consistent`, `rule` and `detail`.
  `?async=true` on materialise and check queues a job (202) that
  `GET /api/reasoning/jobs/{job_id}` reports, with the status and body the
  synchronous call would have answered.
- **Feedback dialog and admin inbox.** Signed-in users send a bug report,
  feature request, question or other note from the new **Feedback** button in
  the sidebar footer, or from the help card that now ends every documentation
  page. A report can carry the in-app page and the browser it was sent from;
  both are opt-out. It goes to this instance's admins, who triage it at
  **Admin → Feedback**: set its status, correct its type, reply to the reporter (who follows
  status and replies under **My reports**) and keep an internal note. New
  routes: `POST /api/feedback`, `GET /api/feedback/mine`, and
  `GET`/`PATCH`/`DELETE /api/admin/feedback[/{id}]`. Submissions need a
  write-capable principal, are rate-limited per client address and are capped
  at 20 per user per day.
- **A real FAQ.** `/docs/faq` grew from five technical answers to about sixty
  questions for people using the platform: accounts, datasets and access,
  importing, querying, validation, models, Spark, the API, troubleshooting and
  how to get help.
- **CI covers S3 storage, `sfcgal3d` and the container image.** Both CIs now
  run the S3 asset store against MinIO (`tests/storage_s3_live.rs`: object and
  HTTP round trips, overwrite, delete, an absent key reading as absent, wrong
  credentials refusing to start; skipped unless `OTS_TEST_S3_ENDPOINT` is
  set). Until now the S3 backend had no test that sent it a byte. GitHub
  compiles and unit-tests `sfcgal3d` in a Debian trixie job, because Ubuntu's
  `libsfcgal-dev` is 1.5 and the feature needs SFCGAL 2.x. Before, only GitLab
  built it. The release image is built and started (`/livez`) on every pull
  request that touches the Dockerfile, a Cargo manifest, the lock file,
  `.cargo/` or `vendor/`, and nightly (`.github/workflows/image.yml` and the
  GitLab `image-build` job). Before, it was first built at release.
  `scripts/check_env_drift.py` (both docs-parity jobs) fails when
  `docker-compose.yml` hands the server a variable that neither `.env.example`
  nor `docs/administration.md` documents, or when `.env.example` sets one that
  compose never passes on. Documented settings that compose does not forward
  are listed as a warning. `RUST_LOG` is now in the environment table.

### Changed

- **Standards Score recounted once for the second merge train: 25 of 28.**
  The "W3C SPARQL 1.1 Tests" row of `docs/triplestore-comparison.md` is
  retired for every system (owner decision, 2026-10-03): the W3C test-suite
  licence allows no performance claims on a subset of a suite, so the row
  could not be graded for this project; measured comparisons take its place in
  `docs/performance-comparison.md`. The matrix has 28 rows, and every other
  system's count drops by one. Open Triplestore's column, recounted from the
  merged matrix, is 25 of 28 (it was 15 of 29): SPARQL 1.1 Query, SPARQL 1.2,
  RDF 1.2, OWL 2 RL, SHACL Validation, ShEx, SWRL, GeoSPARQL 1.0 and 1.1, DCAT
  3 and VoID follow new Full grades, and JSON-LD 1.1 now shows its Partial
  grade. `docs/standards.md` states the Full rubric: a row is Full only when
  every remaining failure is a test-suite defect, blocked on an open external
  specification issue, or a limit of an external database engine, and a
  deliberate deviation keeps it Partial (JSON-LD 1.1 and RML for now).
  SPARQL 1.2 and RDF 1.2 are graded Full: every SPARQL 1.2 entry passes, and
  RDF 1.2's four remaining `rdf:XMLLiteral` entries wait on w3c/rdf-xml#97.
- **JSON-LD 1.1 graded *Partial*.** `docs/standards.md` grades it for the
  first time, on the json-ld-api run: three `fromRdf` entries fail because the
  serialiser writes every stored quad as it is, where the algorithm folds a
  list typed `rdf:List` into `@list` (dropping the type quads) or refuses an
  `rdf:JSON` literal that is not JSON; and uploads keep `@direction` as an RDF
  1.2 directional string, where a JSON-LD 1.1 processor drops it by default.
  The comparison's JSON-LD cell, which marked feature presence, follows
  (✅ → 🟡).
- **The JSON-LD processor is vendored with four fixes** (`oxjsonld` 0.2.6 in
  `vendor/oxjsonld/`, one commit and one upstream draft each, see
  `vendor/README.md`), and every `toRdf` entry of the W3C json-ld-api run now
  passes (`docs/conformance/jsonld.md`). What changes for uploads:
  - **Relative IRIs resolve as RFC 3986 says** when the base IRI or the
    reference has `.` or `..` segments: `"@base": "http://a/b/./c"` with
    `"@id": "../d"` now gives `http://a/d`, not `http://a/b/d`, and
    `//host/../x` gives `http://host/x`. Documents whose base has no dot
    segments are unaffected.
  - **An `@base` that is not a valid IRI** but has a scheme
    (`"http://invalid/<>/"`) no longer refuses the document: relative IRIs
    resolved against it are not well-formed and are left out, as the JSON-LD
    to RDF algorithm leaves out any such IRI.
  - **A type map applies the type's scoped context to the nodes inside it,
    nested ones included** (`"@container": "@type"`); nested nodes used to
    fall back to the definitions without it.
  - `@direction` is kept as an RDF 1.2 directional string as before; the
    processor now also offers JSON-LD 1.1's `rdfDirection` modes, which the
    W3C runner uses.
- **The store keeps every literal exactly as written.** Typed literals used to
  be stored as values and read back in a canonical form: `"1"^^xsd:boolean` as
  `true`, `"05"^^xsd:integer` as `"5"`, a `+00:00` time zone as `Z`, every type
  derived from `xsd:integer` (`xsd:int`, `xsd:nonNegativeInteger`, …) as
  `xsd:integer` and `xsd:dateTimeStamp` as `xsd:dateTime`. They now come back
  with the lexical form and datatype they were written with, from SPARQL, the
  Graph Store Protocol and downloads (RDF 1.1 Concepts §3.3). Oxigraph 0.5.11
  and its evaluator spareval 0.2.7 are vendored with this change
  (`vendor/README.md`; the on-disk format is unchanged and existing stores open
  as they are). What changes for queries:
  - **Graph patterns, joins, `DISTINCT`, `GROUP BY`, `sameTerm` and
    `DELETE DATA` match terms, not values.** `{ ?s ex:n 1 }` no longer finds
    `"01"^^xsd:integer` or `"1"^^xsd:int`, `true` no longer finds
    `"1"^^xsd:boolean`, a join between `"5"^^xsd:int` and `5` no longer
    matches, and `SELECT DISTINCT` returns both. Use a `FILTER` to match by
    value: `FILTER`, `ORDER BY`, arithmetic and aggregates still compare
    values as before. The same holds inside RDF 1.2 triple terms.
  - **Data loaded before this version keeps its canonical form**, so a query
    constant written as in the source file may no longer match it
    (`"05"^^xsd:integer` against a stored `"5"`), and appending the same file
    again adds the as-written literals beside the old ones. Reload such data
    with a replace (`PUT`, or `DROP` then load) rather than an append.
  - **Seeded vocabularies and seed-bundle models** are checked once on the first
    start and, where a copy holds its file's triples in the old canonical form,
    replaced by the file's triples (nobody's edit, so nothing is kept aside);
    their licence records then say "unchanged" without the canonical-form
    caveat. LOV installs record the copy exactly as installed.
  - **SHACL**: `sh:datatype` accepts valid data of the derived integer types
    and `xsd:dateTimeStamp` (it used to report every stored
    `"5"^^xsd:nonNegativeInteger` as a violation, which made write gates answer
    422 on valid data), checks their ranges (`"300"^^xsd:byte` is a violation),
    and requires a time zone on `xsd:dateTimeStamp`; `sh:minInclusive` and
    friends compare `xsd:dateTimeStamp` with `xsd:dateTime`. Activation flags
    take only the literal `true`: `sh:uniqueLang`, `sh:closed`,
    `sh:qualifiedValueShapesDisjoint`, `sh:deactivated` (on shapes, rules,
    constraints and validators) and `sh:optional` written as
    `"1"^^xsd:boolean` no longer activate, so the shapes uploads no longer
    need to refuse that form and accept it. `sh:hasValue`, `sh:in`,
    `sh:equals` and `sh:disjoint` compare terms as written. A shape that is an
    `rdfs:Class` through `rdfs:subClassOf` in the shapes graph is an implicit
    class target (SHACL §2.1.3.3; a direct `a rdfs:Class` was the only form
    found). W3C SHACL core: `core/property/uniqueLang-002` passes, so the core
    section has no known failure at full result-set equality, and **SHACL Core
    is graded Full** (`docs/standards.md`, the comparison matrix and the
    in-app capabilities graph, demo content version 16).
  - **OWL**: `dt-not-type` sees the datatype as written (`"300"^^xsd:byte` is
    an inconsistency). `cls-maxc1/2`, `cls-maxqc1–4` and `owl:hasSelf` read
    their number or flag by value, so `owl:maxCardinality 1` and
    `"1"^^xsd:nonNegativeInteger` both apply. Rules that join on a literal
    match it as written.
  - **Parallel and columnar query paths** keep the term as written wherever
    the engine does (`BIND(?o AS ?x)`, `IF`, `COALESCE`, `sameTerm`,
    `COUNT(DISTINCT ?o)`) and read `xsd:dateTimeStamp` as a dateTime.
- **JSON-LD 1.1 graded *Partial*.** `docs/standards.md` grades it for the
  first time, on the json-ld-api run: the JSON-LD processor keeps the dot
  segments of a base IRI, writes `@direction` as an RDF 1.2 directional string
  in the `rdf-12` build, mis-scopes type-scoped contexts in type maps and
  serialises invalid `rdf:JSON` literals. The comparison's JSON-LD cell, which
  marked feature presence, follows (✅ → 🟡).
- **OWL 2 RL runs all 78 RL/RDF rules; graded Full.** The Table 8 rules with
  literal subjects (`dt-type2`, `dt-eq`, `dt-diff`) are applied to data values
  through the 32-type RL datatype map: a literal written two ways
  (`"1"^^xsd:integer`, `"1.0"^^xsd:decimal`) gets the other's triples, so
  `owl:hasValue`, `owl:hasKey` and negative data assertions match by value; data
  values type the subjects of `owl:someValuesFrom` restrictions; a value outside
  its property's datatype range is a `dt-not-type` inconsistency, and two
  different values of a functional data property (or under a maximum cardinality
  of one) a `dt-diff` one. Runs that used to succeed on such data now report the
  inconsistency. A differential test checks the engine against a
  generalized-triple reference evaluator. `eq-ref` stays opt-in (decision D2).
- **RDFS follows RDF 1.1 Semantics; graded Full.** New: `rdfD2` (every predicate
  is an `rdf:Property`), the RDF and RDFS axiomatic triples, and RDF 1.1 `rdfs1`
  (recognized datatypes in use are `rdfs:Datatype`), replacing a non-standard
  rule that made every literal's datatype a subclass of `rdfs:Literal` directly.
  All patterns, axiomatic ones included, now run in one fixed-point loop over the
  data and the derivations, so `rdfs:Resource rdfs:subClassOf ex:C` reaches
  every resource. The infinite `rdf:_n` axioms stop at the largest index the data
  uses and `rdfD1` is not materialised (decision D11). Every run now writes the
  axiomatic triples, so the entailment graph holds more triples than before. A
  datatype clash (an ill-typed literal such as `"ten"^^xsd:integer`, or a value
  outside the datatype its property's range names) now ends an RDFS run in an
  inconsistency (422 over HTTP) instead of being ignored, and a clean RDFS run
  reports `"consistent": true` (it was `null`).
- **SHACL-C is the W3C SHACL Compact Syntax.** The parser behind
  `Content-Type: text/shaclc` (`PUT /api/datasets/{id}/shapes`,
  `PUT /api/shacl/shape-graphs/{id}/turtle`, `POST /api/shaclc/parse`) now
  implements the whole grammar and production rules of the SHACL Community
  Group report and builds the RDF graph directly: `BASE`/`IMPORTS`/`PREFIX`,
  `shapeClass`, several target classes after `->`, `param=value` node and
  property parameters, `|` and `!`, `@shape` references, nested `{ }` bodies,
  `[ … ]` arrays, full property paths, every Turtle literal form, and `.` after
  every constraint. **Results change:** a bare non-XSD IRI after a path is now
  `sh:class` (it was `sh:node`; write `@ex:Shape` for a shape reference), and
  the old dialect's keywords (`closed` in the header, `pattern "x"`,
  `// "msg"`, `or( … )`, `;`) are a `400` naming the line and column, with a
  pointer to the dialect switch. The 32 test cases of the report are vendored
  under `tests/fixtures/w3c-shaclc/` (W3C Software and Document License) and
  pass, each also round-tripped through the serializer
  (`tests/w3c_shaclc_conformance.rs`); `docs/standards.md` grades SHACL-C
  **Full** (was Partial). Bugs of the old parser that made its Turtle invalid
  (an undeclared `owl:` for `imports`, unescaped line breaks in messages and
  patterns, bare `urn:` IRIs) are fixed in the legacy dialect too.
- **SHACL-C downloads are lossless or a 422.** `GET …/shapes?format=shaclc`,
  the Studio's `GET /api/shacl/shape-graphs/{id}/turtle?format=shaclc` and
  `POST /api/shaclc/serialize` wrote only node shapes' first target class,
  paths, datatypes, node kinds, counts, patterns and messages, and dropped
  everything else without a word (`sh:class`, `sh:in`, `sh:hasValue`, ranges,
  lengths, logical constraints, node-level constraints; complex paths came out
  as an unparseable `_:b…`). The serializer now writes everything the syntax
  can express, checks that its output parses back to exactly the triples it
  wrote, and answers `422` with a `losses` list (subject, predicate, object,
  reason) when anything is left over. Three implied triples are omitted without
  counting as losses: `rdf:type sh:PropertyShape` and blank-node
  `rdf:type sh:NodeShape` on shapes it writes, and `sh:minCount 0`
  (docs/shacl.md, "Implied triples"); `?lossy=true` returns the partial
  document with an `X-SHACLC-Losses` count and a `# INCOMPLETE:` comment block.
  The form manifest's `shaclc` field is `null` for such a graph.
- **ShEx semantics corrected** (results change). A triple whose predicate a
  shape constrains but whose value fails the constraint now fails the shape
  instead of being ignored (unless the predicate is `EXTRA`); one triple no
  longer satisfies two constraints; `CLOSED {}` closes; `{m,}` is unbounded
  (it meant exactly m); recursion is no longer assumed to succeed; string
  lengths count code points, not bytes; numeric facets compare exactly.
  Schemas whose references cycle through a negation, and malformed shape
  maps, are now a `400`; a validation whose references nest deeper than
  20 000 levels is a `422` (validation runs off the async runtime on a
  thread sized for that depth). ShEx is graded Full in
  `docs/standards.md`, and the comparison's ShEx cell follows.
- **DCAT 3 / DCAT-AP 3 / DCAT-AP-NL 3 and VoID are graded Full.**
  `docs/standards.md` gives VoID its own row; the comparison's "DCAT" row is
  now "DCAT 3", and both cells follow the grades. The
  capabilities seed follows (`DEMO_CONTENT_VERSION` 16 refreshes it). The
  aggregate dataset carries the themes of the datasets it covers.
- **The DCAT catalogues use DCAT terms with their declared semantics.** The
  model registry's catalogue (`/api/catalog`) is built as RDF terms instead of
  hand-written Turtle: `dcat:hasVersion` points at version resources (it was a
  literal), `dcat:mediaType` is an IANA IRI typed `dct:MediaType` (it was a
  literal), a namespace that is not an IRI is dropped instead of interpolated,
  and only the serialisations the data endpoint serves are offered (it
  advertised JSON-LD and RDF/XML, which it answered with N-Quads and TriG).
  Both catalogues type every object with its DCAT range class
  (`dct:LicenseDocument`, `dct:LinguisticSystem`, `foaf:Document`, …) and
  describe themes, statuses and agent types as labelled `skos:Concept`s. A
  dataset's owner is now its `dct:publisher` and `dct:creator` in every
  profile. A data-model's void:triples no longer counts version `1.0.1`'s
  graphs into version `1.0`. The live dataset no longer carries the newest
  version's `dcat:version`, and a draft version is no longer listed. The
  SPARQL service's `dcat:endpointDescription` is the service description, not
  the homepage.
- **An unknown `DCAT_PROFILE` stops the server at startup** instead of
  silently publishing plain DCAT; so do an unknown `CATALOG_PUBLISHER_TYPE`, a
  `CATALOG_LANGUAGE` that is not an ISO 639-3 code, and a
  `CATALOG_PUBLISHER_URI` / `CATALOG_LICENSE` that is not an IRI.
- **Profile warnings.** What DCAT-AP-NL requires and the registry does not
  hold — a dataset's theme, contact point or licence — is logged once as a
  warning and never invented.
- **SAML 2.0 is graded Full and ships in the Docker image.** The Dockerfile's
  `CARGO_FEATURES` now defaults to `full,saml,plugin-postgres,plugin-mysql,plugin-mssql`;
  the image already carried libxml2 and libxmlsec1, so it gains no package.
  `saml` stays out of `full`, so a native `cargo build` still needs no
  libxmlsec1, pkg-config or libclang. A custom `CARGO_FEATURES` must now list
  `saml` to keep SAML. The admin UI no longer labels SAML experimental.
  `docs/standards.md` grades it Full for the web-browser SSO and Single Logout
  profiles with this store as the service provider (Artifact binding, ECP,
  attribute queries, NameID management and metadata aggregates/MDQ out of
  scope).
- **SAML needs an https `BASE_URL` outside localhost.** The login route refuses
  to start on a plain-http `BASE_URL` other than loopback, and the `saml_state`
  cookie follows `BASE_URL` (https → `SameSite=None; Secure`) instead of
  `SECURE_COOKIES`, which compose did not even forward. The public SP metadata
  route now answers only for an active provider; admins fetch it before
  activation from `/api/admin/oauth/providers/{id}/saml/metadata`.
- **Docker Compose passes every `.env` setting to the server.** The
  `triplestore` service listed its environment by name and had no `env_file`,
  so documented settings such as `SECURE_COOKIES`, `TRUSTED_PROXY_CIDRS`,
  `OTS_REMOTE_ALLOWLIST`, `OTS_DISABLE_REGISTRATION`, `LDP_ROOT_ACL`,
  `RATE_LIMIT_DISABLED` and the `LLM_*` tuning never reached it, and
  `BACKUP_RETENTION_COUNT`, `BACKUP_SCHEDULE_HOURS`, `S3_BUCKET`, `S3_REGION`
  and `RUST_LOG` were hard-coded over `.env`. It now loads `.env` through
  `env_file` (`required: false`, Compose 2.24+), and those five take their value
  from `.env` with the old default. `scripts/check_compose_env.py`, in both CI
  pipelines, fails when a documented variable is not forwarded or is
  hard-coded.
- **`OAUTH_CLIENTS_JSON` client secrets accept secret references.** A
  `secret` may be `env:NAME`, `file:/path` or `vault:…`, resolved at boot like
  `JWT_SECRET`. Under `OTS_ENV=production` a raw secret now stops the server at
  startup, as do the other raw-secret settings, and so does a reference that
  does not resolve; no client from the variable is seeded until every secret
  in it has resolved. Development still accepts a raw value, with a one-time
  warning.
- **Settings read in two places now come from one parsed value.**
  `--serve-frontend false` now also stops `/` and `/sparql` from serving the
  web UI to a browser; they used to read `SERVE_FRONTEND` from the environment
  on their own. Federated identity assertions (`OTS_TRUSTED_ISSUERS`) are
  checked against `--base-url` as well as `BASE_URL`; the built-in default
  still counts as unset. `BACKUP_DIR` is read once: an empty value now means
  `<data-dir>/backups` for `--restore` and store auto-recovery too, which used
  to take it as the working directory.
- **Library: `dcat::generate_dcat_catalog` and `generate_org_dcat_catalog`
  removed.** Only tests called them. Use `dcat::generate_catalog_bytes` with
  `RdfFormat::Turtle`.
- **The app's own dialogs replace the browser's.** The 11 `window.confirm()`,
  3 `window.prompt()` and 31 `window.alert()` calls in the web UI are now the
  app's confirmation dialog, a text-entry dialog and error toasts: translated,
  themed and non-blocking.
- **Datasets list: badges and filters for all ten graph roles.** The list used
  its own six-role map, so `domain-values`, `linkset`, `provenance` and
  `catalog` graphs got no badge; it now follows the canonical role list.
- **Shape graph revisions open in the history dialog** instead of as raw text
  in a new browser window.
- **The "Transitive ancestors" demo service says what it does**: a SPARQL
  property path, not OWL 2 RL reasoning. The e2e "standards" checks for
  reasoning, SWRL and LDP now call the engines
  (`POST /api/reasoning/materialize`, `POST /api/swrl/execute`, the `/ldp`
  routes) instead of only reading seeded triples.

- **The conformance suites run once per CI run.** GitHub's conformance job ran
  every `*conformance*` test binary that the backend job's `cargo test
  --workspace` had already run, with fewer features. It now keeps only the
  reasoner-sidecar and seed-bundle steps.
- **Settings added in this release are named for what they cover.** Before
  release, five new settings were renamed, and the old names are not read:
  `OIDC_TOKEN_POLICY` and `OIDC_WRITE_SCOPES` are now
  `OTS_OIDC_IDP_TOKEN_POLICY` and `OTS_OIDC_IDP_WRITE_SCOPES` (IdP tokens;
  `OTS_OIDC_SESSION_POLICY` and `OTS_OIDC_WRITE_SCOPES` keep governing this
  store's own provider tokens); `OTS_REMOTE_RETRIES` and
  `OTS_REMOTE_MAX_RETRY_WAIT_SECS` are `OTS_LDES_RETRIES` and
  `OTS_LDES_MAX_RETRY_WAIT_SECS`, since only LDES sync retries;
  `OTS_REPAIR_PROPOSAL_TTL` is `OTS_REPAIR_PROPOSAL_TTL_DAYS`; and
  `TILES3D_MAX_FEATURES` is `OTS_TILES3D_MAX_FEATURES`.
- **The `SERVICE` deadline follows the query timeout.** Unset,
  `OTS_SERVICE_DEADLINE_SECS` now defaults to `SPARQL_QUERY_TIMEOUT_SECS`
  (30 by default), so raising the query timeout no longer leaves federated
  queries failing at 30 s.
- **`OTS_REASONER_TOKEN` is read as a secret.** The server resolves it like
  `LLM_API_KEY`: `env:`, `file:` and `vault:` references work, a raw value is
  accepted with a deprecation warning, and `OTS_ENV=production` refuses a raw
  value.
- **IdP access tokens no longer create API tokens by default.** In OIDC
  resource-server mode an access token issued by the external IdP was treated
  as a full interactive session, including `POST /api/auth/tokens`, so any
  client holding a user's IdP token for this store's audience could turn it
  into a permanent `ots_` token for that account. Under the new default
  `OTS_OIDC_IDP_TOKEN_POLICY=session` such a token still reads and writes but gets
  `403` when it asks for an API token, the rule `OTS_OIDC_SESSION_POLICY`
  already set for this store's own provider tokens. Create API tokens from a
  web UI sign-in, or set `OTS_OIDC_IDP_TOKEN_POLICY=full` to restore the old behaviour.
- **`OIDC_DEFAULT_ROLE` is capped at `user`.** The default role applies to
  every account an IdP token creates, so `admin` or `super_admin` made every
  account the IdP knows an administrator; claim-mapped roles were already
  capped at `admin`. Such a value now logs an error at startup and `user` is
  used. The cap also applies to the `env-oidc` provider entry's default role,
  so an entry created by an earlier version with an admin default, or edited
  to one, creates `user` accounts. `guest` is kept. Grant admin per account
  through `OIDC_ROLE_CLAIM_MAP` or the UI.
- **Federation: `SERVICE ?var`, per-query limits, and the W3C federation tests.**
  An endpoint named by a variable that the pattern before the `SERVICE` binds —
  `?d void:sparqlEndpoint ?ep . SERVICE ?ep { … }` — used to fail with "the
  variable encoding the service name is unbound"; it is now evaluated as a
  lateral join, once per row, with that row's endpoint (SPARQL 1.1 Federated
  Query §4; `OPTIONAL { SERVICE ?ep { … } }` likewise). The endpoint still has
  to pass the allowlist, and a `urn:source:<id>` value is resolved for the
  caller exactly like a written-out `SERVICE <urn:source:id>`. The calls of one
  query now share a budget: an answer for the same endpoint and pattern is
  fetched once and reused; a query may contact `OTS_SERVICE_MAX_ENDPOINTS`
  endpoints (default 16) with `OTS_SERVICE_MAX_CALLS` requests (default 64),
  and over either cap the call fails like any other (an error, or the empty
  solution under `SILENT`); and its `SERVICE` calls must finish within
  `OTS_SERVICE_DEADLINE_SECS` (default 30) of its start, after which the query
  fails, `SILENT` or not — before, nothing stopped a query that kept calling
  remotes after its HTTP request had timed out. A query containing `SERVICE`
  is evaluated against the store itself, never by the in-memory copies. The
  `service/` and `syntax-fed/` sections of the W3C SPARQL 1.1 test suite are
  vendored unmodified and run against local endpoints
  (`tests/w3c_sparql11_federation.rs`, unscored), and `docs/standards.md` now
  grades SPARQL 1.1 Federated Query **Full — deny-by-default** (was Partial).
  See docs/federation.md.
- **A `SERVICE` result over a cap fails instead of being truncated.** A remote
  result with more than `OTS_SERVICE_MAX_ROWS` rows used to be cut to the cap
  and joined as if it were the whole answer, which silently changed the result
  of the query around it. It is now a failed invocation, like a refused
  endpoint or a timeout: the query errors with a message naming
  `OTS_SERVICE_MAX_ROWS`, and under `SERVICE SILENT` the clause yields the
  single empty solution (SPARQL 1.1 Federated Query §3.2). Every outbound
  response body — `SERVICE` results, virtual-datasource queries and snapshots,
  LDES pages — is now read as a stream under a new limit,
  `OTS_REMOTE_MAX_BYTES` (default 64 MiB), and a body over it fails the same
  way; before, bodies were read whole with no limit. A query or a snapshot
  that relied on truncation, or on a body above 64 MiB, now fails: raise the
  variable or narrow the pattern. See docs/federation.md.
- **OWL 2 RL runs 75 of the 78 RL/RDF rules (was 63), and lists and inverse
  properties work everywhere.** New: `eq-diff2`/`eq-diff3`
  (`owl:AllDifferent`), `prp-pdw` (`owl:propertyDisjointWith`), `prp-adp`
  (`owl:AllDisjointProperties`) — inconsistencies, reported as a 422 with their
  rule id — and `prp-ap`, `cls-thing`, `cls-nothing1`, `scm-op`, `scm-dp`,
  `prp-eqp1/2`, `eq-ref` (opt-in). `scm-cls` now derives all four of its
  consequences (`C ≡ C` and `owl:Nothing ⊑ C` were missing). `cls-int1`,
  `prp-spo2` and `prp-key` match lists of any length (intersections and chains
  of three or more never fired); `scm-int`, `scm-uni` and `cax-adc` no longer
  skip blank-node members. An inverse property expression `[ owl:inverseOf P ]`
  works in domains, ranges, characteristics, sub-/equivalent properties, chains,
  keys and restrictions. Before, those rules matched nothing, and an inverse key
  property was dropped from the key, so individuals merged on the remaining
  properties alone. `prp-npa1/2` no longer require an
  `rdf:type owl:NegativePropertyAssertion` triple. `prp-trp` keeps the reflexive
  triple a cycle derives. `x owl:differentFrom x` is an inconsistency. The
  duplicate `prp-hv1/2` copies of `cls-hv1/2` are gone. Every run now adds the
  13 axiomatic triples (9 annotation properties, `owl:Thing`/`owl:Nothing` as
  classes) plus their `scm-cls` consequences, so an empty store's
  materialisation reports 48 triples instead of 32. The only rules not run are
  `dt-type2`, `dt-eq` and `dt-diff`.
- **OWL 2 DL is graded Full with the reasoner sidecar** (`docs/standards.md`
  footnote 4, with a new legend sentence on grades that depend on an optional
  component the project ships); without it the native rules stay sound but
  incomplete. The comparison matrix's OWL DL cell follows. The server's RDF →
  OWL 2 mapping now also reads a named class carrying several
  `owl:oneOf`/`owl:intersectionOf`/… lists as one `EquivalentClasses` axiom per
  list, OWL 1's `owl:DataRange` as `rdfs:Datatype`, and ignores
  `rdf:type owl:NamedIndividual` on a blank node, instead of refusing them as
  outside OWL 2 DL.
- **`ReasoningError::Inconsistency` names its rule.** The library variant is now
  `Inconsistency { rule, detail }` (it was `Inconsistency(String)`), and
  `ReasoningError::NotConverged { regime, iterations }` is new. Code that
  matched `Inconsistency(_)` matches `Inconsistency { .. }`.
- **OWL 2 QL is graded Full.** Data ranges are now decided on values through
  the OWL 2 datatype map (`src/reasoning/datatypes.rs`, the nineteen
  datatypes of the EL and QL maps): `-5` against `xsd:nonNegativeInteger`,
  `"1.5"^^xsd:decimal` against `xsd:integer` and a language-tagged string
  against `xsd:string` are inconsistent (`ql-dt-range`), and so is an
  ill-typed literal anywhere in the data (`ql-dt-not-type`). `∃U.D` with a
  data range `D` works on the left of an inclusion (a subject with a `U` value
  in `D` gets the class, in materialisation and in the stand-alone
  rewriting), a class that needs a value outside its property's range is
  unsatisfiable, data ranges may be intersections or datatype definitions, and
  disjoint data properties compare values (`1` and `"1.0"^^xsd:decimal` clash).
  Ranges on datatypes outside the QL map (`xsd:boolean`, `xsd:double`, …) are
  reported as ignored axioms instead of being checked by datatype family.
  `docs/standards.md` grades OWL 2 QL Full, and its cells in
  `docs/triplestore-comparison.md` follow.
- **`owl2-ql` materialisation results change** (see Added): the target graph
  now holds ground atoms, a dataset run writes to the dataset's own
  `urn:entailment:owl2-ql:<id>` graph (it wrote the TBox closure to the shared
  graph), and an inconsistent ontology fails the run.
- **OWL 2 EL is a native EL++ reasoner covering the whole profile; graded Full.**
  Results change. The SPARQL `INSERT` loop is replaced by a saturation engine in
  Rust (`src/reasoning/owl2_el/`): it reads the ontology from the quad index,
  normalizes it, applies the EL++ completion rules with a worklist and writes
  the new consequences with one `insert_quads` call.
  - It now derives what the loop missed: every subsumption that needs an
    anonymous successor, TBox property chains (`findingSite ∘ partOf ⊑
    findingSite`), chains of any length, ranges on existential successors,
    ⊥ through successors, and `owl:Thing ⊑ C`. An unscoped run sees its own
    consequences. Classification also writes `owl:equivalentClass`, and the
    property hierarchy writes `owl:equivalentProperty`.
  - New constructs: `owl:hasValue` (object and data), one-individual
    `owl:oneOf`, `owl:hasSelf`, `owl:sameAs`, `owl:differentFrom`,
    `owl:AllDifferent`, negative property assertions, functional data
    properties, data ranges and the nineteen EL datatypes with value semantics
    (`"1"^^xsd:integer` equals `"1.0"^^xsd:decimal`). Classification stays
    complete with nominals: a class whose subsumers depend on them is checked
    against a hypothetical instance.
  - Inconsistency also covers equality against `owl:differentFrom`, negative
    property assertions, ill-typed literals and data-range violations,
    including two values of a functional data property.
  - Only triples that are not already in a premise graph are written. A
    triple that is both asserted and entailed is no longer copied into
    `urn:entailment:owl2-el`.
  - **API addition (`ReasoningReport::ignored`):** the axioms outside the EL
    profile that a run left out, as `[{construct, count, example}]`.
    `POST /api/reasoning/materialize` returns the list when it is not empty.
  - `docs/owl2-el.md` is rewritten, with performance measured on the
    saturation core, and `docs/standards.md` grades OWL 2 EL Full.
    `tests/owl2_el_conformance.rs` adds 34 tests, two of them randomised
    differential tests against the RL engine on the EL ∩ RL fragment;
    `test_biomedical_classification` no longer passes on an asserted triple.
- **SHACL report graphs follow the W3C results vocabulary** (release note for
  anyone who reads `urn:system:reports:*` or a SHACL Studio pipeline's report
  graph). The RDF a validation run writes used to carry display strings; it
  now carries the terms the engine saw:
  - `sh:focusNode` and `sh:value` are typed terms: a literal keeps its
    datatype and language tag (`200`, not `"200"`; `"x"@en`, not `"x"`), and a
    blank node stays a blank node instead of the string `"_:b0"`.
  - `sh:resultPath` is a SHACL path structure (`[ sh:inversePath ex:p ]`, an
    RDF list for a sequence) instead of a string literal in SPARQL syntax.
  - `sh:sourceConstraintComponent` is the component IRI
    (`sh:MinCountConstraintComponent`); it used to be a string literal such as
    `"sh:minCount 1"`.
  - `sh:sourceConstraint` names the `sh:sparql` node of a SPARQL-based
    constraint, and `sh:resultSeverity` keeps a custom `sh:severity` IRI
    (it collapsed to `sh:Violation`).
  - `sh:result` nodes are written by an RDF serializer, so messages with any
    characters round-trip.

  Queries that matched the old string literals must match terms now (for
  example `sh:sourceConstraintComponent sh:MinCountConstraintComponent`, or
  `FILTER(str(?value) = "200")`). Reports written before the upgrade keep the
  old shape until the next run replaces them. The JSON report adds
  `source_constraint_component` (the component IRI; empty on stored runs from
  before) and the write gate's 422 body `sourceConstraintComponent`; the
  existing JSON fields are unchanged. One result changes in the JSON too: a
  `sh:sparql` constraint or `sh:select` validator on a node shape whose row
  leaves `?value` unbound now reports the focus node as the value, as SHACL
  §5.3.2 says (it reported none).
- **The W3C SHACL corpus is compared at full report equality.** The runner,
  now `w3c_shacl_full_report_equality`, compares every result on focus node,
  path, value, source shape, component, severity and `sh:sourceConstraint`
  (everything but the message), through the RDF report the engine writes. It
  used to compare `sh:conforms` and the focus-node multiset only; both levels
  agreed on every case (119 pass) before the old one was retired.
  `sparql/pre-binding/shapesGraph-001` moved from the known failures to a new
  category, optional and unsupported: `$shapesGraph` / `$currentShape` are
  optional in SHACL §5.3.1, which requires a processor without them to report
  a failure, which this one does (w3c/data-shapes#426 contests the test; SHACL
  1.2 drops the variables). `docs/conformance/shacl.md` has the results.
- **`sh:expression` has the SHACL-AF semantics.** It used to read a form of
  this engine's own: a path plus comparison constraints on the expression node
  (`sh:expression [ sh:path P ; sh:minExclusive 9.09 ]`). Under the Note that
  node is a path expression, whose values must all be `true`, so **a shapes
  graph in the old form now reports every focus node** whose values are not
  `true`. Rewrite it as a function expression (`docs/shacl.md`, *Expression
  constraints*) or as a property shape; the reference example
  `tests/fixtures/example-bridge/shapes-af.ttl` shows the first.
- **More SHACL-AF declarations fail the shapes graph instead of being
  ignored.** A `sh:SPARQLFunction` whose body does not parse, whose `sh:select`
  has other than one result variable, or which has both or neither of
  `sh:select` and `sh:ask` fails the run of the shapes graph that declares it
  (in an admin-designated function graph it is skipped with a warning); the
  function used to be left out, and every call to it was unbound. A
  `sh:target` with neither a `sh:select` nor a `sh:SPARQLTargetType` type, a
  node expression that contains itself or is none of the seven kinds, and a
  target-type parameter given twice or as a blank node fail the shapes graph
  too.
- **RDF Patch `PA`/`PD` change the dataset's prefix table**, as the format
  page has it ("Prefixes do not apply to the data of the patch. They are
  changes to the data the patch is applied to"); they used to change nothing.
  A version diff served as a patch carries the prefix-table changes as
  `PD`/`PA` rows, and diffs between versions chain: a diff to a version keeps
  one name-based `H id`, and its `H prev` names the diff into `from`. `A`/`D`
  rows may still use the patch's own prefixed names (a documented extension).
  `POST …/patch` answers `401` instead of `500` without a token. The RDF
  Patch row of `docs/standards.md` is now Full.
- **A version's TriG download is one document with named graphs.**
  `GET /api/datasets/{id}/versions/{ver}/data` wrote every snapshot's triples
  into the default graph; each graph is now written under the live graph it
  was cut from, so the download lines up with the version's RDF Patch diffs.
- **RDF Patch follows the format page.** `PA`/`PD` take the prefix name as a
  keyword or a quoted string and the namespace as an IRI or a string, so
  patches written by Jena or RDF Delta (`PA "rdf" "http://…" .`) apply; the
  old `PA ex: <…>` form is still read. A patch may hold several `TX` blocks,
  and a `TA` discards only its own. Rows end at their `.`, not at a line
  break, and `#` comments may follow a row. Blank nodes, `_:x` or `<_:x>`,
  name the store's own nodes: a patch can delete a blank-node triple, and a
  version diff with blank nodes applies faithfully (adds used to go through
  `INSERT DATA`, which minted fresh nodes). Triples without a graph go to the
  registered graph `?graph=` names. Patches are applied by a new
  transactional quad path (`TripleStore::apply_quad_ops`) that records the
  exact net change in the change log, and the response's `added`/`removed`
  are that net change. Spec-derived tests in `tests/rdf_patch_conformance.rs`.
- **RML file sources read JSON and XML by the RML-IO registry.** A JSONPath
  iterator or reference is now RFC 9535 JSONPath and an XPath one is XPath
  1.0, where the engine walked dotted keys and matched element names. Under
  the legacy RML vocabulary this keeps what worked — a bare name is the member
  or child element of that name, an iterator that selects one array iterates
  its elements, a relative XML iterator (`person`) matches anywhere — and
  adds nested references (`a.b`, `../@id`, attributes). An XML reference's
  value is the node's string value (all its text, nested elements included)
  rather than its first text child. A template with an unbalanced brace —
  one opened inside a reference, closed outside one, or never closed — is
  refused at upload instead of being read as best it could. RML stays graded
  Partial in `docs/standards.md`: one remaining R2RML failure, R2RMLTC0002f
  (SQL identifier case-folding), is a deliberate deviation, which the Full
  rubric does not accept.
- **RML / R2RML terms follow R2RML.** A file mapping, a dataset's stored
  mapping and every newly frozen datasource mapping version now generate terms
  by R2RML's rules, so their output changes:
  - a template object map with no `rr:termType` is an **IRI** (§7.4), not a
    literal — write `rr:termType rr:Literal` for text;
  - a template value is encoded only when the term is an IRI, and only outside
    RFC 3987 `iunreserved` (§7.3): `a-b.c_d~e` and non-ASCII letters stay as
    they are where they used to become `a%2Db%2Ec%5Fd%7Ee`, and a literal
    template is no longer percent-encoded;
  - a blank node is one per value and graph (§11.2, §9.1), not one per row;
  - graph maps sit on the subject map (`rr:graphMap` or `rr:graph`) and
    predicate-object maps, and a triple goes to the **union** of both graphs
    instead of the predicate-object map's overriding the subject's; `rr:class`
    triples go to the subject's graphs, `rr:defaultGraph` names the default
    graph, every graph map is used rather than the first, and a malformed one
    is an error instead of being dropped. A graph map on the triples map is
    still read, as a subject graph map;
  - a relative IRI resolves against a base IRI: a triples map's `rml:baseIRI`,
    or `?base=` on `POST /api/datasets/:id/mappings/execute`.
  **Existing datasource mapping versions keep their output.** Each version is
  now stamped with the rules it runs under (`semantics`: `r2rml` or `legacy`,
  on the version entity and in the API); a version frozen before carries no
  stamp and runs as `legacy`. `POST`/`PUT /api/mappings` and an inline dry-run
  accept `"semantics": "legacy"` to freeze a new version under the old rules,
  so a fix does not rename every entity. Whatever the rules:
  - `rr:object <IRI>` is an IRI — it used to be written as a string literal,
    which YARRRML's `[ex:p, ex:Term]` produced — and a constant literal keeps
    its datatype and language tag (a constant is no longer typed by whether it
    contains `://`);
  - a literal constant in a subject, predicate or graph position is a mapping
    error;
  - `rr:tableName` may be schema-qualified and its parts delimited
    (`"Student"`, `` `x` ``, `[x]`), each re-quoted by the dialect; a delimited
    column name (`rr:column "\"ID\""`, `{"ID"}`, join columns) reads column
    `ID`.
  The YARRRML and legacy-format converters now write an explicit `rr:termType`
  on every template and column term map.
- **An RML predicate-object map generates every predicate × every object.**
  A predicate-object map with several `rr:predicateMap` / `rr:predicate` or
  `rr:objectMap` / `rr:object` values used only the first of each, in store
  order, and dropped the rest without a word. It now generates one triple per
  predicate per object (R2RML §6.3, §11.1), referencing object maps included,
  into each of its graphs, on file and datasource runs and in dry runs. This
  applies to every mapping version, legacy-stamped ones included: the triples
  it adds were missing, and no existing term changes.
- **An RML mapping that does not conform to R2RML is refused, with the
  construct named.** Parsing used to take the first of two subject maps,
  logical sources or term-map kinds, drop an invalid language tag at run
  time and ignore an unknown `rr:termType`. Now a triples map needs exactly
  one logical source and one subject map; a term map exactly one of
  `rr:constant`, `rr:template`, `rr:column` / `rml:reference`, each once;
  `rr:termType` must be `rr:IRI`, `rr:BlankNode` or `rr:Literal` and legal
  where it stands (a literal subject, predicate or graph, or a blank-node
  predicate or graph, is an error); `rr:language` and `rr:datatype` exclude
  each other and apply to literals only; a language tag must be valid BCP 47;
  `rr:class` values must be IRIs; a logical table cannot have both
  `rr:tableName` and a query; a referencing object map takes no term map of
  its own. The error names the triples map and the construct. A mapping
  version stored before that breaks one of these rules now fails to run with
  that message, under either term-rules stamp.
- **A column the source does not have is an error.** A term map, function
  parameter or join condition naming a column that a CSV header lacks, or
  that a datasource's query does not return, failed silently — every row
  generated nothing for it. It is now refused before the first row, naming
  the column, where the mapping uses it and the columns the source has. A
  query result with two columns of one name is refused too (R2RML §5.2).
  Connectors gain `SourceConnection::columns(query)` in `ots-plugin-api`
  (additive, default `None`); SQLite, PostgreSQL, MySQL and SQL Server
  describe a query without running it.
- **A data error aborts an RML run and names the rows.** A value an IRI term
  map cannot make a valid IRI of — and, new, a value outside its
  `rr:datatype`'s lexical space (R2RML §10.3: `"forty-two"` as
  `xsd:integer`) — used to drop that term without a word. Now the run fails
  and writes nothing, and its error names the first ten offending rows with
  their values (R2RML §4.3), for every mapping version, legacy-stamped ones
  included. A run that would rather go on can opt in: `"onDataError": "skip"`
  on `POST /api/sources/:id/runs`, `?on_data_error=skip` on
  `POST /api/datasets/:id/mappings/execute` and `POST /api/rml/preview`. It
  leaves those terms out and reports the rows (`dataErrors` on the run
  record, `data_errors` in the file endpoints' response). A dry-run never
  aborts on one and lists them as `dataErrors`. `otsfn:mintIri` reports an
  IRI it cannot make the same way.
- **An empty value is a value; RML-IO `rml:null` says what is NULL.** An
  empty CSV cell, an empty JSON string, an empty XML element and a SQL `''`
  generated no term. Under R2RML and RML-IO only a NULL does — a SQL NULL, a
  JSON `null` or missing key — so these now generate an empty literal (or an
  IRI built from the empty string), and an empty XML element `<a/>` reads as
  `""` where it read as missing. `rml:null "…"` on the logical source (RML-IO
  `<http://w3id.org/rml/null>`, also read in the legacy `rml:` namespace)
  lists further values that count as NULL. A mapping version stamped
  `legacy` keeps generating no term from an empty value and ignores
  `rml:null`. The legacy-format converter writes `rml:null ""` on every
  logical source, so a converted mapping still reproduces the legacy
  transformer's output.
- **LDES search tree: one root node with bounded relations.** `tree:view` now
  points at a root node (`/ldes/nodes/0`, no members) that links every full
  fragment with a `tree:GreaterThanOrEqualToRelation` and a
  `tree:LessThanOrEqualToRelation` on `dct:created` (Server Primer §4), and
  the first unsealed fragment with a lower bound. Full fragments no longer
  link onward, so each bound covers everything reachable through it; unsealed
  fragments still chain forward. Page numbers are unchanged. Tombstones are
  typed `as:Delete` as well as `ots:Tombstone`.
- **Outbound requests follow redirects within the allowlist.** SPARQL
  federation (`SERVICE`), virtual SPARQL sources and LDES sync used to refuse
  every redirect. They now follow up to 10, and each hop must be covered by
  `OTS_REMOTE_ALLOWLIST` again; a hop off the list fails the request ("redirected
  to … which is not in OTS_REMOTE_ALLOWLIST") without contacting it, and a hop
  to another host or port drops the `Authorization` header. LDES 1.0 §3.3 and
  TREE require clients to follow redirects.
- **The LDES client follows the LDES 1.0 consumer specification.** It
  initialises as §3.1 says: the URL given is the event stream (its one
  `tree:view` is dereferenced), its root node, a redirect to either, or a page
  with exactly one `tree:view`; anything else is an error naming §3.1, where it
  used to crawl whatever `tree:view` or `tree:node` it found. It reads context
  from the root node, follows only the page's own `tree:relation`s, and
  extracts members as §3.4 does: `<stream> tree:member ?m`, the star pattern of
  `m` and every quad in the named graph `m`. A member's named graph, when it has
  one, is the payload written for its entity. The declared paths
  (`ldes:timestampPath`, `versionOfPath`, `versionTimestampPath`,
  `sequencePath`, `versionSequencePath`, and the create / update / delete
  paths) are evaluated as SHACL property paths with the SHACL engine's
  parser. Deletes are recognised by the stream's declared
  `ldes:versionDeletePath` / `ldes:versionDeleteObject` — `as:Delete` is no
  longer hard-coded; `ots:Tombstone` still counts for a stream that declares
  no delete object. A create adds to an entity without removing what it held;
  versions published out of order are ordered by `ldes:versionTimestampPath`
  and `ldes:versionSequencePath`, so an older version arriving later is not
  applied. The legacy `ldes:PointInTimePolicy` is read as `starting_from`.
  The client records which subjects each entity's version wrote, and replaces
  exactly those (with their blank nodes) when a newer version arrives.
- **3D Tiles GLB positions are relative to a local origin.** The mesh node
  carries a `translation` to the centre of the content and the POSITION
  accessor holds f32 offsets from it. Absolute ECEF coordinates in f32 had
  snapped every vertex to a 0.25–0.5 m grid at Dutch latitudes; vertices now
  keep millimetre precision. Clients that read the POSITION accessor directly
  must apply the node translation. The tileset's bounding region is computed
  from per-feature bounding boxes, without triangulating.
- **The Docker image ships the SQL connectors.** 0.7.0 announced PostgreSQL,
  MySQL / MariaDB and SQL Server datasources, but the image built only
  `--features full`, which leaves `plugin-postgres`, `plugin-mysql` and
  `plugin-mssql` out, so the published image could register only `sqlite` and
  `sparql` sources. The Dockerfile's `CARGO_FEATURES` now defaults to
  `full,plugin-postgres,plugin-mysql,plugin-mssql`. The connectors are pure
  Rust over rustls and need no new system package; the binary grows by about
  4%. `full` and a plain `cargo build` are unchanged. A custom image that sets
  `CARGO_FEATURES` replaces this list, so it must name the connectors it wants
  (`CARGO_FEATURES=full` builds an image without them). The SQL Server driver
  brings tiberius's rustls 0.21 stack into the image; `deny.toml` now says so
  and keeps its three advisory ignores on reachability grounds.
  `docs/build-features.md` lists the three features, `docs/sources.md` says
  what the image carries, and GitHub CI's backend job compiles the main crate
  with all three (it built only `plugin-postgres`, in the live-sources job).
- **`/sparql` gives a query the dataset SPARQL defines.** The dataset is now set
  on the parsed query instead of being spliced into its text, and keeps its
  meaning: `FROM <a>` alone makes `<a>` the default graph and no longer also a
  named graph, `FROM NAMED <b>` alone leaves the default graph empty, and an
  admin's `FROM` is no longer widened with every registered graph. Only a query
  that names no dataset gets the union of the caller's readable graphs (as
  before). Graphs the caller may not read are still dropped silently. The
  protocol's `default-graph-uri` / `named-graph-uri` (queries) and
  `using-graph-uri` / `using-named-graph-uri` (updates) are now honoured; they
  were advertised but ignored. A query whose outer group has no `WHERE` keyword
  and holds a sub-select no longer fails with 400. Clients that relied on
  `FROM <g>` also exposing `<g>` to `GRAPH ?g` must add `FROM NAMED <g>`.
- **Grades:** SPARQL 1.1 Query is graded Full again, and SPARQL 1.1 Protocol has
  its own graded row (Full) in `docs/standards.md`, and the comparison's cells
  follow.
- **SPARQL query results follow the specification in six more places.** The
  engine's SPARQL parser, evaluator and optimizer (Oxigraph's `spargebra` 0.4.7,
  `spareval` 0.2.7 and `sparopt` 0.3.7) are now vendored under `vendor/` and
  patched, one commit per fix, each with a
  draft upstream PR (`vendor/README.md`):
  - `GRAPH ?g { … }` no longer puts `?g` in scope inside the pattern: around a
    `VALUES`, an aggregate sub-select or a `MINUS` it now enumerates the named
    graphs as SPARQL defines (backport of oxigraph `fdc32b5`, issue #1905).
  - A default graph made of several `FROM` (or `USING`) graphs is their RDF merge:
    a triple held in two of them matches once, so `COUNT` and `SUM` over it no
    longer double-count (backport of oxigraph #1920, issue #1919). The columnar
    query copy deduplicates the same way.
  - A zero-length property path (`*`, `?`) with a constant endpoint matches that
    term even when the graph does not hold it (`ASK { :x :p* :x }` is true on an
    empty graph).
  - `GROUP_CONCAT` returns a plain `xsd:string`, never a language-tagged string.
  - `BNODE("label")` returns a fresh blank node per solution (the same one within
    a solution), accepts any string, and no longer returns the same node in every
    later request.
  - An aggregate nested in another one's argument (`SUM(COUNT(?x))`) is a syntax
    error (400), as SPARQL requires; it used to be accepted (the vendored
    `spargebra` 0.4.7 parser).
  The vendored W3C SPARQL 1.1 query and update sections have no open known
  failures left (`docs/conformance/sparql11.md`).
- **SPARQL 1.2: three more fixes in the vendored engine; the W3C suite is down
  to one known failure.** One commit each in `vendor/`, each with a draft
  upstream PR:
  - `=`, `!=` and `IN` between two literals that both carry a base direction
    (`"abc"@en--ltr = "abc"@en--rtl`) answer by RDF 1.2 term equality; they used
    to panic in the evaluator, a 500 over HTTP.
  - A literal or a triple term as the subject of a triple-term expression
    (`BIND(<<( "x" :p :o )>> AS ?t)`) is a syntax error (400), as the SPARQL 1.2
    grammar requires (`ExprTripleTermSubject ::= iri | Var`).
  - In an aggregating query a SELECT expression may use the variable of an
    earlier SELECT expression, `SELECT (COUNT(?v) AS ?n) (?n + 1 AS ?m)`, as
    SPARQL 1.2 allows; it used to be refused as an unbound variable.
  With the nested-aggregate refusal above, every entry of the vendored W3C
  SPARQL 1.2 suite passes except `grouping#group01`, which needs numeric lexical
  forms kept in storage. SPARQL 1.2 stays *Partial* in `docs/standards.md`, now
  waiting only on that change (`docs/conformance/sparql12.md`).
- **SWRL results change.** Rules that were refused now run: a built-in
  variable bound only by a built-in (`swrlb:add(?z, ?x, 1)` → `?z` in the
  head), any §8 built-in beyond the comparisons, arithmetic, `stringConcat`,
  `contains` and `matches`, `DataRangeAtom`s, and class-expression atoms when
  a regime runs with them. A rule with built-ins runs as a `SELECT` plus
  native evaluation, so its `rule_results[].sparql` shows the `SELECT` and the
  built-ins as comments. Comparisons now compare literals by value across
  types (`1 = 1.0`). `docs/standards.md` grades SWRL Full (was Partial).
- **SWRL rules stored with a dataset always run with it.** `swrl:Imp` rules
  in a dataset's `entailment`- and `model`-role graphs, and in the model
  version it conforms to, re-run after every write to one of the dataset's
  graphs.
  - **Regime in `materialize` mode:** they reach one joint fixed point with
    the regime in its graph.
  - **No regime:** they run on their own into `urn:entailment:swrl:<dataset>`,
    which `?entailment_dataset=` now adds to a query when the dataset has no
    regime.

  `GET /api/datasets/{id}/entailment` reports `inference_graph` and `rules`
  (the graphs read, the count or why they cannot be read, the last run). The
  seeded "Rules (SWRL)" demo dataset's rule had atoms without arguments and
  could never run; it is now valid SWRL RDF.
  `tests/fixtures/example-bridge/shapes-af.ttl` shows the first. In the RDF
  report each result names the node expression as its `sh:sourceConstraint`
  (SHACL-AF §7).
- **GeoSPARQL 1.1 is graded Full for every conformance class but DGGS.** The
  requirement matrix (`docs/conformance/geosparql.md`) has every requirement of
  OGC 22-047r1 met outside the optional DGGS class; `docs/standards.md` grades
  1.1 Full with that scope stated, and the comparison matrix follows. The
  project's own grade, not an OGC certification.
- **GeoSPARQL 1.0 is graded Full.** With the documented GML profile (R15, R17)
  it meets every requirement of OGC 11-052r4 in the requirement matrix
  (`docs/conformance/geosparql.md`); `docs/standards.md` and the comparison
  matrix follow. The project's own grade, not an OGC certification.
- **GeoSPARQL: geometry results follow their first operand's serialisation.**
  `buffer`, `union`, `envelope`, `transform` and the other functions that
  return a geometry used to return a `geo:wktLiteral` whatever the operand; they
  now return a GML, GeoJSON or KML literal for a GML, GeoJSON or KML first
  operand, in its CRS (a GML `srsName` kept as written), as GeoSPARQL 1.1
  §10.9.1 says. GeoJSON and KML exist only in CRS84, so one transformed into
  another CRS is still a WKT literal. `geof:aggUnion` returns the group's
  serialisation when every value shares one.
- **GeoSPARQL: Z survives GML and GeoJSON.** `srsDimension="3"` GML and GeoJSON
  positions with an altitude were read in 2D; they now keep Z (a geometry
  mixing 2D and 3D positions is still read in 2D). The geometry cache keeps Z
  on GEOS 3.11 too. The map viewer still draws the 2D footprint.
- **GeoSPARQL: GML is read strictly.** A multi-patch `gml:Surface` is now a
  `MULTIPOLYGON` of its patches (its second patch used to become a hole of the
  first). Arcs and other curved segments, solids, a `posList` whose numbers do
  not divide into positions, a number that does not parse and a `Multi*`
  member that does not read now make the literal unbound; they used to be read
  as straight lines through the control points, or silently dropped. Text
  outside the coordinate elements (`gml:name`) is no longer read as
  coordinates.
- **GeoSPARQL relation patterns match derived relations.** A query or update
  that reads `?a geo:sfWithin ?b` (or any other of the 24 relations) used to
  match asserted triples only; it now also matches pairs whose geometries are
  related, and each pair once. Set `OTS_GEOSPARQL_QUERY_REWRITE=off` for the
  old behaviour. A pattern with both sides unbound compares every pair of
  geometries in scope.
- **buildingSMART IDS 1.0 is graded Full** in `docs/standards.md`: all 334
  cases of the buildingSMART IDS test corpus pass in CI with an empty
  known-failures list — a development result, not a buildingSMART
  certification (no comparison-matrix row).
- **IDS import validates the document and checks the IDS projection.** An
  IDS is checked against the IDS 1.0 XSD and the IDS audit rules (entity,
  attribute and data-type names per schema, predefined types, value and
  restriction types, lexical validity, satisfiable specifications) and
  refused with every problem listed when it fails. The shapes are now
  SHACL-SPARQL over the IDS projection, one constraint per requirement facet
  with IDS facet semantics (exact classes, every matching property set and
  property, transitive aggregation and nesting, classification parents,
  material sets), plus a model-level shape for required specifications.
  Shapes written over `props:` / `bot:` by earlier imports keep working as
  they are; re-import the IDS and import the IFC again to check a model with
  the new shapes.
- **IDS export is lossless for imported specifications.** The importer
  records each specification and a fingerprint of its shapes; the exporter
  writes the recorded specification back while the shapes are unchanged
  (import → export → import is a fixpoint), and reports an edited one
  instead of exporting a stale source.
- **IDS import compares values the way IDS does.** Values are typed by the
  facet's IDS `dataType` (the IDS data-type table) or the restriction's
  `base`: a double is equal within the IDS tolerance (`v ± (|v|·1e-6 +
  1e-6)`), an integer by value, a boolean as `true`/`false`; `xs:pattern` is
  anchored and translated from XSD regular expressions (several patterns are
  alternatives); bounds are typed (a `0.` bound used to be invalid Turtle) and
  carry no tolerance; a value invalid for its base is refused. A prohibited
  facet is the negation of the required one instead of `sh:maxCount 0`, which
  passed a matching value; applicability facets are conditions the node must
  meet; a required specification fails when the model has no entity of an
  applicable class; a prohibited specification uses all its applicability
  facets and may not carry requirements; every `ifcVersion` listed is
  targeted; entity patterns are expanded against the schema; a specification
  without an entity is converted instead of skipped. The importer reads the
  IFC2X3, IFC4 and IFC4X3_ADD2 schema tables now generated by
  `scripts/gen_ifc_schema_tables.py` (`src/ifc/schema/`).
- **ICDD import is complete for Part 1.** Folder documents import as asset
  sub-folders (they were skipped), secured documents' checksums are verified
  (`checksum_status`), encrypted documents are kept as flagged opaque files,
  `ct:filename` sub-folders are kept as asset folders, ISO's own ontology files
  are recognised instead of loaded as model graphs, and each document's name,
  versions, alternatives, `requested` flag and parties are reported (the
  container's parties and version too). An index with several container
  descriptions is refused; a root `index.*` other than `Index.rdf` still
  imports but is reported. Anonymous imports answer 401 (they were a 500), and
  the import accepts archives up to `OTS_MAX_UPLOAD_MB` (512 MB) instead of the
  8 MB default.
- **ICDD export passes its own validator.** Documents carry `ct:name` and
  `ct:belongsToContainer`, parties are `ct:Person` / `ct:Organisation` with a
  real IRI (not the abstract `ct:Party` named by its own name), filenames are
  relative to `Payload documents/` with asset folders kept and clashes renamed
  (two assets of one name in different folders made a duplicate ZIP entry and a
  500), linksets are RDF/XML, data graphs are RDF documents under
  `Payload documents/`, the folders are explicit entries, the index imports the
  Container ontology, and the download is `<dataset>.icdd`. The README.txt that
  stood in for the ontology files is gone. Documents an earlier import brought
  in keep their index IRIs, kinds, names, versions, alternatives and parties, so
  linksets still resolve after a round trip. The `X-Container-Conforms`,
  `X-Container-Validation` and `X-Container-Findings` headers report the
  export's validation.
- **DOAP is the upstream Apache-2.0 file.** The bundled `vocab/doap.ttl` was
  LOV's re-serialization of the old DOAP namespace document (2009-2015). That
  file stated no licence, and its 97 Japanese-language labels and comments were
  never in the Apache-2.0 upstream repository. It is now
  https://github.com/ewilderj/doap's own `schema/doap.rdf` at commit `d164b82d`
  (2022-03-13, its latest revision), converted to Turtle with its triples
  unchanged: 741 triples with labels in six languages, "Copyright © 2004-2016
  Edd Dumbill, 2016-2017 Edd Wilder-James, 2018- The DOAP Authors", under the
  Apache License 2.0. The registry seeds it as DOAP `2022-03-13` and points the
  entry at it. On an install that seeded the old copy, version `2012-01-04` is
  kept, deprecated. Its licence record no longer calls it the bundled file: it
  says what the copy is and that part of it has no published licence. `NOTICE`
  and `vocab/NOTICE.md` list DOAP under Apache-2.0.
- **Releases are prepared by a workflow.** *Prepare release* in the Actions
  tab takes a `patch`, `minor` or `major` bump and opens a PR that writes the
  next version into `Cargo.toml`, `Cargo.lock`, `README.md` and `CHANGELOG.md`
  (and, for a minor or major, the supported-versions tables). Merging it opens
  the `develop → main` PR. Merging that tags the release and publishes the
  GitHub Release and the image. No personal access token is needed:
  `auto-tag.yml` calls `release.yml` itself instead of relying on its tag
  push, and takes the version from `Cargo.toml` instead of a keyword in the PR
  title. A `patch` on a `release/X.Y` branch releases that line and leaves the
  `latest` GitHub Release and image tag alone. `release.yml` can be re-run for
  an existing tag. See `docs/release-process.md`.
- **RML joins run on file sources.** The upload path (CSV, JSON, XML) had no
  join resolver, so a referencing object map resolved to nothing: the run
  wrote the rest of the mapping, dropped every link it asked for, and reported
  success. Now the parent's rows are indexed by the parent side of the join —
  the same index the relational executor uses, bounded by
  `OTS_SOURCES_JOIN_MAX_ROWS` — and each child row links to every parent row
  whose values equal its own on every join condition, across files and
  formats. A key with a NULL in it matches nothing. Tests:
  `src/rml/executor.rs`, `tests/rml_conformance.rs`.
- **A join-less referencing object map joins each row to itself.** Without a
  `rr:joinCondition` (legal only when both triples maps read the same logical
  source), R2RML §8's joint query is the child query itself: the object is the
  parent's subject for the same row. The relational executor and the dry-run
  sampler made it a cross join, linking every child row to every parent row's
  subject. This applies to every mapping version, legacy-stamped ones
  included: the old output was wrong data, not different names. Two maps over
  the same file now count as the same logical source only when their iterator
  and reference formulation match too.
- **The W3C R2RML test cases run in CI.** `tests/w3c_r2rml_conformance.rs`
  runs the RDB2RDF Working Group's R2RML cases on SQLite in the conformance job
  and on PostgreSQL 16 and MySQL 8.4 in the live-database job, comparing each
  output dataset by isomorphism and holding a per-database known-failure list
  as a two-way ratchet. The cases are fetched at a pinned commit and checked by
  sha256 (`scripts/fetch-w3c-r2rml-tests.sh`), not vendored, and no score is
  published (W3C test-suite policy). `rml::sql::execute_relational_as_mapped`
  runs a relational mapping into the graphs its graph maps name — the output
  dataset R2RML defines — where a registered mapping's run still writes
  everything into its run graph.
- **`docker-compose.override.yml` no longer ships.** It was one machine's
  workaround for an unstable build host (thin LTO, two build jobs), and Compose
  merges the file automatically, so every `docker compose build` got the slow,
  less optimised image while `docs/development.md` called the file git-ignored.
  It is now ignored; keep a local copy if you use one.
- **Standards grades follow the code.** `docs/standards.md` regrades
  **SPARQL 1.1 Query** and **OWL 2 QL** from Full to Partial, and every footnote
  now lists the real gaps. SPARQL 1.1 Query: the W3C entries the oxigraph 0.5
  evaluator fails and why, duplicate matches across several `FROM` graphs
  (oxigraph#1919), the `/sparql` dataset rewrite, and shards that evaluate
  `EXISTS` per shard. OWL 2 QL: two unsound rewrites, no `rdfs:range`, and a
  regime that materialises only the TBox closure. OWL 2 EL: an unsound CR3 rule.
  OWL 2 RL: `dt-type1` and `dt-not-type` are implemented (the docs and README
  said they were not), and unscoped runs miss joins over two derived premises.
  SHACL Advanced: the gaps that remain. GeoSPARQL: the
  functions that answer wrongly today. The RDF Patch and LDES rows are reworded:
  RDF Patch lacks multiple transaction blocks (there are no nested ones), and
  `ldes:versionKey` is not part of LDES 1.0. The in-app capabilities graph (the
  `capabilities` demo dataset) gave every standard "Full"; it now lists every
  row of `docs/standards.md` with its grade, a unit test keeps the two in step,
  and existing installs get the new graph on their next start (demo content
  version 14). The conformance table counts `tests/ldes_conformance.rs` as the
  "LDES 1.0 / TREE" suite and relabels the DCAT and OWL 2 DL rows.
- **The comparison matrix follows the grades on every row.**
  `docs/triplestore-comparison.md` shows 🟡 for every standard
  `docs/standards.md` grades Partial, including SPARQL 1.1 Query, OWL 2 QL,
  SHACL-AF inference, DCAT and VoID, so the standards score is recounted,
  16 → 11 of 29. With this release's Full grades for SPARQL 1.1 federation,
  OWL 2 EL, OWL 2 QL and OWL 2 DL (with the reasoner sidecar) it is recounted
  again, 11 → 15 of 29 (2026-10-03). The OWL DL and SWRL text no longer contradicts the matrix. The
  unused spatial R-tree, the unbuilt property-path memoisation and
  "single-node only" are corrected, and its footnote numbers no longer collide.

- **`owl2-dl` needs a configured backend (breaking).** There is no default
  any more: without `OTS_DL_BACKEND` every `owl2-dl` request answers 503.
  Set `OTS_DL_BACKEND=native` to keep the previous in-process rules, which are
  sound but not complete (`complete: false` and a warning in every report).
  `OTS_EXTERNAL_REASONER` and `OTS_EXTERNAL_REASONER_BIN` are removed; a server
  that still sets them logs a warning. Input that is not OWL 2 DL is refused
  with the violations listed, where it was reasoned over before. In the library,
  `owl2_dl::{ExternalReasoner, ExternalReasonerBridge, NativeTableauStub}` and
  `konclude_bridge::KoncludeReasoner` are replaced by `dl_backend::DlBackend`,
  `dl_backend::{materialize, check}` and `konclude_bridge::KoncludeBackend`.
- **The native OWL 2 DL rules reach one joint fixed point with OWL 2 RL.** The
  RL rules used to run once, before the DL rules, so a DL consequence such as
  `x p x` from `owl:hasSelf` never reached `rdfs:domain`, `rdfs:subPropertyOf`
  or any other RL rule. They now alternate until neither adds anything. New:
  - `owl:hasSelf` also works backwards (`x p x` ⇒ `x : ∃p.Self`);
  - `owl:ReflexiveProperty` relates every individual in scope to itself, so a
    property that is also irreflexive is reported inconsistent (`prp-irp`).

  The DL layer's own 1- and 2-property `owl:hasKey` rules and its
  negative-assertion checks duplicated RL `prp-key` and `prp-npa1/2` and are
  gone. An inconsistent negative assertion now names `prp-npa1` or `prp-npa2`
  instead of `dl-negative-object-assertion` / `dl-negative-data-assertion`.
- **Cardinality obligations moved to a diagnostics graph (breaking).** The
  `urn:dl:minCardinality` / `exactCardinality` / `minQualifiedCardinality` /
  `exactQualifiedCardinality` triples are not inferences. They are now written
  to `<target>:diagnostics` (for example
  `urn:entailment:owl2-dl:diagnostics`), which `?entailment=` never folds into
  a query, instead of the entailment graph itself.
- **`owl2-dl` datasets re-materialise in the background.** After a write, a
  dataset in `materialize` mode with the `owl2-dl` regime no longer reasons
  inside the write. A background run starts once no write has arrived for
  `OTS_DL_DEBOUNCE_MS` (default 2000), and writes during a run queue one more.
  `GET /api/datasets/{id}/entailment` adds `status` (`queued`, `running`, `ok`,
  `inconsistent`, `not_in_profile`, `unavailable`, `timeout`, `too_large`,
  `failed`, …), `error`, `backend`, `complete` and `dl_backend`. A `PUT` of the
  setting still runs at once.

- **SHACL: a property path reads the merge of a run's data graphs.** SHACL
  validates one data graph (§3.4), and a run over several validates their
  merge. A `sh:path` from an IRI focus node used to be evaluated inside each
  data graph in turn, so a path whose hops live in different graphs (a
  sequence, `sh:zeroOrMorePath`, `sh:oneOrMorePath`) found nothing, while the
  same rule as a `sh:sparql` constraint, or the same path from a blank-node
  focus node, found the value. It now reads the merge for every focus node.
  Validation and inference results change for datasets with more than one
  graph: a `sh:minCount` can now pass, a `sh:maxCount`, `sh:uniqueLang` or
  `sh:qualifiedMaxCount` can now fail, and a SHACL-AF rule whose `sh:condition`
  reads such a path can now fire (scheduled inference materialises what it
  derives). Single-graph runs, and so every write gate, are unchanged. The
  `OTS_SHACL_REACH_PROBE` setting that measured the difference is removed.
- **GeoSPARQL: a GML literal's `srsName` counts everywhere.** Only the metric
  functions read it. Topology, `geof:relate`, the constructive functions,
  `getSRID`, `transform` and `aggUnion` took every GML literal for CRS84, so
  `sfEquals` and `metricDistance` disagreed about the same literal. Every
  function now reads a literal's CRS from one place: the WKT `<crs>` prefix or
  the GML `srsName`. `EPSG:28992`, `urn:ogc:def:crs:EPSG::28992` and the other
  spellings normalise to `http://www.opengis.net/def/crs/EPSG/0/28992`. Axis
  order follows the CRS, so an EPSG:4326 GML `<gml:pos>1 2</gml:pos>` is
  latitude 1, longitude 2: `sfEquals` with `"POINT(2 1)"` is now true, and
  with `"POINT(1 2)"` false. `getSRID` reports the `srsName`, and a
  constructive result or `aggUnion` keeps it.
- **GeoSPARQL: `geof:relate` harmonises its operands' CRSs**, as the sf/eh/rcc8
  functions do. It compared RD New metres with CRS84 degrees.
- **GeoSPARQL: `geof:ehCoveredBy` uses the spec's DE-9IM mask `TFF*TFT**`**,
  which makes it the exact inverse of `ehCovers`. It used GEOS `covered_by`, so
  a line on a polygon's boundary was covered by it (now false). A geometry
  strictly inside another is now `ehInside` and not `ehCoveredBy`; one inside
  that touches the other's boundary is still covered by it.
- **GeoSPARQL: the perimeter of a non-areal geometry is its length.**
  `geof:metricPerimeter` of a line returned 0. It now returns the line's
  geodesic length (0 for a point), as GeoSPARQL 1.1 says.
- **GeoSPARQL: `geof:transform` fails instead of passing coordinates through.**
  A coordinate outside a CRS's domain was copied into the result unchanged, so
  the north pole "transformed" into Web Mercator and Paris got RD New
  coordinates. That result is now unbound, and so is any operation that has to
  harmonise such a geometry. RD New has a domain: its EPSG area of use plus
  about 50 km. Outside it the polynomial approximation returned plausible
  garbage. `transform` now keeps Z, and it accepts its target CRS as an
  `xsd:anyURI` literal as well as an IRI. Building now needs **GEOS 3.11 or
  later** (the `geos` crate's `v3_11_0` feature, for `transform_xy`). Debian
  bookworm, current Ubuntu, Debian trixie and vcpkg all ship 3.11 or later;
  Ubuntu 22.04's 3.10 no longer builds.
- **GeoSPARQL: an empty geometry literal is the empty geometry.** `""^^geo:wktLiteral`
  (with or without a CRS), `""^^geo:gmlLiteral` and `""^^geo:geoJSONLiteral` were
  not geometries, so every function over them was unbound (GeoSPARQL 1.1
  Req 17, 21 and 27). An empty plain string is still not a geometry.
- **GeoSPARQL units of measure: more IRIs, and no silent fallback.** Unit
  arguments accept QUDT units (`unit:M`, `KiloM`, `CentiM`, `MilliM`, `FT`,
  `MI`, `MI_N`, `DEG`, `RAD`, `M2`, `KiloM2`, `HA`), EPSG units (9001, 9002,
  9036, 9101, 9102) and the OGC `uom:` units, given as an IRI or as an
  `xsd:anyURI` literal. An unknown unit used to be ignored, which returned
  planar degrees as if they were metres. It is now unbound. So are an angular
  unit on a projected CRS, any unit on a CRS this build cannot reproject, and
  an area unit for a distance. `geof:area` takes an area unit: geodesic on a
  geographic CRS, planar on a projected one. No unit, or `uom:unity`, still
  means the CRS's own units.

### Deprecated
- **The SHACL-C dialect of 0.7 and earlier** is parsed only when a request passes
  `?dialect=legacy`, logs a deprecation warning each time, and is accepted for
  one more release only; `?lenient=true` now applies to it alone. The migration
  table is in `docs/shacl.md` ("Migrating from the legacy dialect").

### Fixed
- **Triple terms and base direction no longer get lost.** The canonical and
  Skolem blank-node modes now walk into RDF 1.2 triple terms: a blank node
  inside `<<( … )>>` is relabelled or skolemized with the same label as the
  node outside it (it kept its input label before, breaking co-reference), and
  two different triple terms no longer hash alike. Model version diffs render
  terms in N-Triples form (escaped literals, `"x"@ar--rtl`, `<<( s p o )>>`;
  every triple term rendered as `<< >>` before, so different ones compared
  equal), the commit log keeps a triple-term value instead of an empty
  string, and the browse endpoints' JSON carries a literal's `"its:dir"`, also
  inside triple terms. Because diffs now escape quotes and newlines, the
  draft revision token (`ETag`) of a version holding such a literal changes
  once.
- **The full-text index follows every write.** It used to be kept in step only
  by SPARQL Update, Graph Store writes, imports, version restores and seeds, so
  literals written through LDP, RDF Patch, RML runs, SHACL rule output,
  entailment materialisation, replication, LDES sync or repair stayed
  unsearchable until an unrelated write forced a rebuild. Every mutating store
  operation now records what it is about to change in the store's search
  journal, and the index catches up on it before it answers a text query —
  touched graphs re-indexed, touched quads reconciled; only an unbounded write
  rebuilds the whole index. A timed-out SPARQL Update or Graph Store write no
  longer leaves the index stale either. Default-graph literals are refreshed
  under the same key the full rebuild gives them.
- **A dataset's governance metadata graph failed to load with an
  `adms_status` code.** `urn:system:metadata:dataset:{id}` was hand-written
  Turtle: a status given as a code (`completed`) became a relative IRI, the
  document failed to parse, and the graph was silently not written. It is now
  built as RDF terms (the code becomes its EU dataset-status IRI) and also
  carries access rights, temporal coverage and update frequency.
- **Dataset version IRIs in the catalogue did not match the version
  registry's.** The catalogue percent-encoded the version label
  (`…/version/1%2E0%2E0`); the registry, the version routes and the model
  conformance links use `…/version/1.0.0`.
- **Signing out revokes the session's refresh token.** The web UI sent `{}` as
  the logout body, and the server read a body without `refresh_token` as "no
  token" instead of falling back to the cookie, so the refresh token of an SSO
  or password session stayed valid after sign-out. The UI now sends the token
  it holds, and the server falls back to the cookie.
- **AI assistant calls time out.** NL→SPARQL (`/api/llm/sparql`), the SHACL
  assistant, saved-query repair, SQL-source review suggestions and
  `/api/llm/feedback` had only a connect
  timeout, so a gateway that accepted the connection and stalled held the
  request indefinitely. They now share the chat's per-completion budget
  (`LLM_TIMEOUT_SECONDS`, default 120).
- **Spark's `vocab_term_search` tool says when it cannot search.** Without the
  vocabulary term index it answered that no installed vocabulary defines the
  term; it now says the search is not available, as `text_search` does.
- **The service registry's refusals are logged.** A 401 or 500 from the
  registry (`LD_DISCOVERY`) passed as a successful registration. The first
  rejection, and each change of status, is now a warning that names the status
  (and `LD_REGISTRY_TOKEN` for a 401 or 403).
- **Docs.** `docs/spark.md` no longer says both that tool rounds are not
  streamed and that every reply streams token by token: the answer streams on
  the directive protocol and arrives per round with native tools. Two broken
  `sources.md#secret-references` links now point at the section on secret
  references.
- **`/admin/docs` is admin-only in the UI too.** The docs editor had no guard;
  like the other `/admin/*` pages it now sends anyone but an admin home before
  loading anything (the write endpoints were already admin-only).
- **Translated HTML is escaped and sanitized.** Translations with inline
  markup were rendered with `{@html}`, most without a sanitizer, and dataset,
  organisation and shape-graph names were spliced into them as HTML. Values
  are now escaped and the result keeps only inline formatting tags.
- **Turning on "Gate writes" asks for confirmation.** The pipeline editor's
  check ran on the wrong transition (it asked when switching the gate *off*,
  and could not undo either way).
- **Untranslated Dutch strings** in the dataset viewer and the release titles.
- **LDP `DELETE` removes the member from its container.** A container created
  by POSTing to `/ldp/c` (no trailing slash) is `…/ldp/c`, but `DELETE` looked
  for the parent at `…/ldp/c/`, so the `ldp:contains` triple survived and the
  container kept listing the member. The parent is now the container that
  lists the member. Found by the new e2e
  check that drives the `/ldp` routes.

- **More `.env` settings reach the server under Docker Compose.**
  `docker-compose.yml` passes the server an explicit environment list, so a
  setting `.env.example` documents had no effect until it was on that list.
  The list now includes the per-task model overrides (`LLM_SPARQL_MODEL`,
  `LLM_SHACL_MODEL`, `LLM_CHAT_MODEL`), the LLM rate limits
  (`LLM_RATE_LIMIT_PER_MIN`, `LLM_RATE_LIMIT_ANON_PER_MIN`), the prompt guard
  (`LLM_GUARD_INJECTION_ACTION`, `LLM_GUARD_BLOCKLIST`,
  `LLM_GUARD_MAX_MESSAGE_CHARS`, `LLM_GUARD_MAX_MESSAGES`,
  `LLM_GUARD_MAX_TOTAL_CHARS`), the LLM request log
  (`LLM_LOG_PREVIEW_DISABLED`, `LLM_LOG_RETENTION_DAYS`), `CLAMAV_ADDR` and
  `OTS_ENV`. Each is passed empty when unset, which the server treats as
  unset. `.env.example` now lists `OTS_ENV` and `LLM_CHAT_MODEL`. It also notes
  that the production posture refuses the raw `JWT_SECRET` and
  `S3_SECRET_KEY` this compose file passes, so those must be references first.
- **OIDC resource-server mode, provider-token policy and several settings
  are documented.** The resource-server mode, which accepts an external IdP's
  access tokens (`OIDC_ISSUER`, `OIDC_AUDIENCE`, `OIDC_DEFAULT_ROLE`, the
  claim-mapping variables and `ACCEPT_LEGACY_TOKENS`), was configured only
  through `docker-compose.yml` and `.env.example`. It now has a section in
  `docs/auth.md`: token requirements, account linking, role and organisation
  mapping, what an IdP token may do (including minting API tokens), and that
  `ACCEPT_LEGACY_TOKENS=false` also refuses the web UI's own sign-in.
  `docs/oidc-provider.md` said provider tokens work "like any session token".
  It now describes `OTS_OIDC_SESSION_POLICY` and `OTS_OIDC_WRITE_SCOPES`:
  under the default `session` policy every registered client's token can
  write, admin tokens always write, and because the provider issues only
  `openid profile email`, `scoped` makes provider tokens read-only.
  `.env.example` had claimed a client could widen its scopes; that claim is
  corrected. The administration env table gains `WRITE_TIMEOUT_SECS` (a
  timed-out write answers `503` but may still complete), `VALIDATION_API_URL`
  (forwards the caller's bearer token), `SEED_STANDARDS_DEMO`,
  `EMBED_FRAME_ANCESTORS` and the query-cache, mirror, SHACL run-index and
  telemetry tuning knobs. `docs/plugins.md` lists the accounts-dashboard
  settings. `docker-compose.yml` now passes `OTS_OIDC_SESSION_POLICY`,
  `OTS_OIDC_WRITE_SCOPES`, `OTS_GUEST_CAPABILITIES` and
  `EMBED_FRAME_ANCESTORS` through to the server; before, setting them in
  `.env` had no effect.
- **The login page no longer offers SSO buttons that cannot sign in.** With
  `OIDC_ISSUER` set, the store creates an *Environment OIDC* provider entry
  (slug `env-oidc`, no client ID) to hold the accounts of IdP bearer tokens,
  and the login page listed it as a sign-in option that failed with "has no
  client_id". `GET /api/auth/oauth/providers` now lists only active entries a
  browser sign-in can start from: OIDC entries with a client ID, and SAML
  entries with an SSO URL in a build with the `saml` feature.
  `GET /api/auth/oauth/{slug}/authorize` answers `404` for the others instead
  of `500`. IdP bearer tokens are unaffected, whatever an admin later
  edits on the entry; turning it off is no longer needed to hide it.
- **OIDC browser sign-in lands signed in.** After the code exchange the
  callback redirected to `/#access_token=…`, a page that never reads the
  fragment, so the tokens were dropped and the user arrived signed out. It now
  redirects to `/oauth/callback#…`, which stores them and removes them from the
  URL bar.
- **Security → Identity providers form now matches the provider API.**
  Opening a provider for editing threw (the form treated `scopes` as an
  array), and saving was refused with a 422 (`scopes` and `role_claim_map`
  were sent as an array and an object, and the required `is_active` as
  `enabled`); the table showed every provider as Disabled. The form now reads
  and sends `scopes` and `role_claim_map` as strings and `is_active` as the
  on/off switch, refuses a role claim map the server would read as empty, and
  keeps fields it does not show (such as `tenant_id`) through an edit. Fields
  the API never had (`role_claim`, explicit endpoints) are gone. The separate
  "Azure AD" type is folded into OIDC, which is the only type the sign-in
  flow runs. SAML providers get entity ID, SSO URL and certificate fields in
  place of an IdP-metadata field the server never read. The default-role list
  no longer offers `super_admin`, which sign-in caps at `admin`. The synthetic
  `env-oidc` row (`OIDC_ISSUER`) can be edited, but its slug cannot be changed.
- **Editing a SAML provider no longer erases its IdP certificate.** Reads
  redact the certificate, so no client could send it back, and an update
  overwrote it with nothing. `PUT /api/admin/oauth/providers/:id` now keeps
  the stored certificate when the body omits it, as it already did for the
  client secret.
- **A graph named `…/validation` keeps its version snapshot.** A snapshot
  graph is named after its source graph's last path segment, under the version
  IRI, which is also where the version's validation-layer graph lives
  (`{base}/dataset/{id}/version/{v}/validation`). A source graph whose last
  segment slugified to `validation` was copied there, then cleared when the
  bindings snapshot was written, so the version kept none of its triples (only
  the bindings, if any), and deleting the version dropped that graph twice.
  Snapshots, branches included, no longer take the name `validation`: such a
  graph becomes `validation-1`. An index suffix can no longer collide with
  another graph's name either (`a`, `a-1`, `a` now give `a`, `a-1`, `a-2`
  rather than two `a-1`s). Versions cut earlier keep their IRIs, and their lost
  snapshot cannot be recovered: restoring one of them replaces that live graph
  with what the snapshot holds. Tests: `tests/dataset_versions_http.rs`.
- **SWRL refuses rules it cannot run as written.** Every rule is now checked
  before any runs, and one that fails refuses the whole request with `400`;
  nothing is written. Previously an untranslatable rule was skipped with
  `success: false` while the others ran. The checks:
  - unsafe rules: a head variable not bound in the body, or a built-in
    variable that only a built-in mentions (built-ins cannot bind yet);
  - built-ins in the head, which were skipped with a warning;
  - a literal in an individual position or an individual in a data position,
    and a variable used as both.

  Object and data property atoms are no longer translated alike: a typed
  variable in an object position binds only its own kind of term. The text
  form has no declarations, so `p(?x, ?y)` there stays untyped.

  Any variable IRI now works (the OWL API's `urn:swrl#x`, an ontology
  namespace, `abbreviatedIRI`), each mapped to a generated SPARQL variable.
  Plain and language-tagged `Literal`s are read, and unknown `format` values
  are refused.

  The report counts only what the run wrote to the target graph, not the
  whole store. It now says whether the fixed point was reached: `converged`,
  and `stop_reason` (`fixpoint`, `max_iterations` or `timeout`). Hitting
  `max_iterations` used to look like success.
- **OWL 2 EL and QL no longer derive what does not follow.** Results change.
  - EL: the CR3 rule turned `A ⊑ ∃p.B` and `B ⊑ C` into `∃p.C ⊑ A`, so in a
    dataset run any individual with a `p`-successor typed `C` was typed `A`.
    It is gone. In its place, a structural CR4 handles filler subsumption and
    the property hierarchy (`A ⊑ ∃r.B`, `B ⊑ C`, `r ⊑ s`, `∃s.C ⊑ D` give
    `A ⊑ D`). EL now also reads `owl:equivalentClass` (both directions),
    `rdfs:subPropertyOf` / `owl:equivalentProperty`, `owl:TransitiveProperty`
    and `owl:disjointWith`, and keys with any number of properties.
    Individuals get the existential restrictions they satisfy as types, so a
    definition such as `D ≡ E ⊓ ∃p.C` classifies them.
  - EL consistency: an unsatisfiable class with no instances is no longer
    called an inconsistency. `classify()` now checks consistency (an
    individual in `owl:Nothing` or in two disjoint classes) and fails with
    `ReasoningError::Inconsistency`, as RL does. An inconsistent EL run of
    `POST /api/reasoning/materialize` that used to succeed now returns an
    error.
  - QL: `C ⊑ ∃P.D` was read as `∃P ⊑ C`, so `x P y` made `x` a `C`; that
    rewrite is gone. Every rewritten existential atom gets its own fresh
    variable (one shared name joined independent atoms). The rewriter now uses
    `rdfs:range`, expands domains and ranges through the class and property
    hierarchies, and composes inverses with sub-properties. The `owl2-ql` TBox
    closure gains the sub-properties entailed through inverses.
  - `POST /api/reasoning/rewrite` reads the TBox only from graphs the caller
    may read. It used to read the unnamed default graph for any authenticated
    caller and spell its class hierarchy out in the answer. An admin's
    rewriting still reads the default graph.
- **Reasoners see their own consequences on an unscoped run.** Without
  `dataset` or `source_graphs`, the OWL 2 RL, EL and DL rules read only the
  unnamed default graph while every consequence went to the named target
  graph, so a rule whose premises were both derived never fired: the third hop
  of a transitive property, a three-link `owl:sameAs` chain,
  `owl:equivalentProperty` propagation (`prp-eqp1/2` are subsumed only once
  `prp-spo1` sees what `scm-eqp1/2` derived), a range reached through a
  sub-property, and every consistency check on derived facts (`eq-diff1` after
  `prp-fp`, `cls-nothing2` after `cax-sco`, `prp-irp`, `prp-asyp`, `cls-com`,
  `cls-maxqc1/2`). Unscoped runs now read the default graph together with the
  target graph (`TripleStore::update_over` / `query_over`). Scoped and
  per-dataset runs already read their target graph and are unchanged. (OWL 2
  QL computes its closure in memory and never reads its own output, so its
  reads are unchanged.) An
  unscoped run can therefore derive more than before, and a store that used to
  pass may now be reported inconsistent.
- **An inconsistent ontology is a 422, not a 500.** `POST
  /api/reasoning/materialize` and `PUT /api/datasets/{id}/entailment` answer
  `422` with `{consistent: false, rule, detail, regime, target_graph}`, naming
  the check that fired; the consequences derived before it stay in the target
  graph. A successful run reports `consistent` (`true` for `owl2-rl` and
  `owl2-dl`, `null` for a regime without inconsistency rules), and
  `GET /api/datasets/{id}/entailment` records what the last run found
  (`consistent`, `inconsistency`).
- **A reasoning run that hits the iteration limit fails instead of returning a
  partial closure.** RDFS, OWL 2 RL, EL and DL stopped silently after 500
  fixed-point rounds and reported success. They now fail with
  `ReasoningError::NotConverged`, a `422` with `converged: false` over HTTP.
- **`POST /api/reasoning/materialize` no longer blocks an async worker.** The
  rules run on the blocking pool, as the per-dataset run already did.
- **A SHACL rule's `$this` reaches an expression that is its only use.** In
  a `sh:SPARQLRule` such as `CONSTRUCT { $this ex:label ?l } WHERE { BIND
  (ex:labelOf($this) AS ?l) }`, `$this` was left unbound — the query parser
  projects the WHERE onto the variables its patterns bind, and the focus node
  was bound only through that projection — so the rule derived nothing.
- **A SHACL rule's expressions see `$this` as bound.** The query optimizer
  treated the pre-bound focus node as unbound wherever an expression used it:
  `BIND ($this AS ?x)` was dropped, `FILTER (BOUND ($this))` was always false,
  and `FILTER (?v = $this)` compared terms instead of values when `$this`
  occurred in no triple pattern (a literal focus node `1` did not equal
  `1.0`). Each `sh:SPARQLRule` CONSTRUCT now has `$this` pre-bound in every
  scope of the query, as SHACL pre-binding defines — the same mechanism the
  `sh:sparql` constraints use — and its triple patterns are still seeded with
  the focus node, so rules derive what they say and keep the optimizer's join
  ordering.
- **SHACL validation and write gates no longer pass data the shapes forbid.**
  Gates get stricter: data that used to be accepted may now be refused with
  422, and a shapes graph that used to load may now fail the run.
  - Every value of `sh:not`, `sh:hasValue`, `sh:pattern` and
    `sh:qualifiedValueShape`, and every list of `sh:and`, `sh:or` and
    `sh:xone`, is its own constraint (SHACL §4). Only the first one was read.
  - `sh:deactivated true` works on property shapes and inline shapes
    (`sh:node`, `sh:not`, logical members, qualified value shapes, rule
    conditions), with the spec's meaning: every term conforms, so `sh:not` of a
    deactivated shape fails. It worked on top-level shapes only.
  - A property shape without a `sh:path`, with several, or with one that is not
    a well-formed path, and a shape typed `sh:PropertyShape` without one, fail
    the shapes graph. They were skipped with a warning.
  - A SPARQL target (`sh:target [ sh:select … ]`) that errors when it runs fails
    the run; it selected no focus nodes. A rule shape whose target cannot be
    loaded fails inference instead of never firing.
  - A `sh:TripleRule` whose subject, predicate or object is a node expression
    (a blank node such as `[ sh:path ex:p ]`) is evaluated as one (see
    *SHACL-AF node expressions* under Added). It wrote the shapes graph's own
    blank node into the data graph.
  - The IDS export reports a second `sh:pattern` or `sh:hasValue` and
    deactivated shapes as losses instead of exporting them with another meaning.
- **SHACL-SPARQL constraints check blank nodes and follow the spec's result
  rules.** Gates get stricter here too.
  - `sh:sparql` constraints and constraint-component validators skipped
    blank-node focus nodes, and ASK validators skipped blank-node values: the
    pre-bound value was pasted into the query text, and no SPARQL syntax names
    a stored blank node, so those nodes conformed unchecked. `$this`, `$value`
    and the component parameters are now bound as RDF terms.
  - A solution binding `?failure` to `true` is reported as a failure of the
    constraint; it was an ordinary result. `?message` sets the result message,
    `{?var}` / `{$var}` in `sh:message` are filled from the solution, and
    `?path` is used only when it is an IRI.
  - `sh:deactivated true` on a `sh:sparql` constraint or on a validator is
    honoured; a constraint component falls back to `sh:validator` or switches
    off.
  - Every value of a single-parameter constraint component is its own
    constraint; only the first was read. Several values for one parameter of a
    multi-parameter component fail the shapes graph, as does a sub-select that
    does not project every pre-bound variable (parameters included). A
    validator without `sh:message` uses the component's.
  - A SHACL-AF `sh:SPARQLRule` whose `$this` appears only in a `FILTER` (no
    triple pattern binds it there) never fired: the query optimizer, not told
    that `$this` is bound, dropped the filter's group. Rules now pre-bind
    `$this` the way constraints do, in every scope of the query (SHACL
    Appendix A). The pre-bound values also seed the evaluation, so a triple
    pattern on `$this` is still looked up by the focus node in rules and
    constraints alike, not scanned in full once per focus node.
  - On the vendored OGC GeoSPARQL validator corpus, `S21-invalid.ttl` (a
    blank-node geometry whose `geo:dimension` exceeds its
    `geo:coordinateDimension`) is now caught: 47 of 48 examples match the OGC
    oracle.
- **`EXISTS` no longer runs per shard.** The in-memory mirror split a query
  across subject shards whenever its triple patterns shared a subject, without
  looking inside `FILTER`, `BIND` or `COUNT` expressions. An `EXISTS` there
  reads triples about *other* subjects, so `ASK { ?s :p ?o FILTER NOT EXISTS
  { ?o :q ?x } }`, or a `COUNT` over that pattern, was answered by shards that
  each saw only their own subjects: `true` where the store says `false`, a
  count that was too low. Such queries now stay off the shards (the full copy
  or the store answers them).
- **RDF 1.2 base direction survives the columnar copy.** The columnar copy
  decoded `"hello"@en--ltr` as a plain language-tagged string, so on the
  default read path `DATATYPE` returned `rdf:langString`, the literal compared
  equal to `"hello"@en`, and `LANG`/`ORDER BY` treated it the same way. A query
  whose expressions reach a literal with a direction, or a triple term, is
  now declined by that copy at evaluation, and the full copy or the store
  answers it. A triple term in an expression used to read as a type error
  there, which dropped rows a `FILTER` should have kept.
- **The W3C SPARQL 1.1 runner also checks the in-memory mirror.** Every
  query-evaluation entry now runs a second time with the mirror on (four
  shards, the columnar copy, the full copy, no rebuild debounce) and must end
  as it does on the engine. Its first run found one more divergence, now
  fixed: the columnar copy ignored the query's `BASE` in `IRI()`/`URI()`, so
  `BASE <http://example.org/> SELECT (IRI("x") AS ?i) {}` returned an unbound
  `?i` instead of `<http://example.org/x>`.
- **LDES member timestamps never go backwards.** A member was stamped with the
  time its write started, so two concurrent writes could publish a member
  earlier than one already published, below a bound clients held (LDES 1.0
  §4.1). The stamp is now raised to the newest published timestamp inside the
  same SQLite transaction as the insert, and that mark survives retention.
- **LDES: edits inside blank nodes publish a version.** Change capture hashed
  only an entity's direct triples while the member carried the whole
  blank-node closure, so an edit to, say, an address node published nothing.
  The hash now covers the closure by content (re-writing the same structure
  under new blank-node labels is still not a change). Two versions of one
  entity on a page no longer share blank-node labels, which merged their
  structures in the served document.
- **LDES sync no longer drops members that share a timestamp.** The
  bookmark skipped every member at or before the newest timestamp applied, so
  a member published later with that same timestamp never arrived, and
  timestamps were compared as strings (`09:30:00-02:00` sorted before
  `10:00:00Z`). Timestamps now compare as `xsd:dateTime` instants and the
  client remembers the members at the bookmark's own timestamp (LDES 1.0
  §3.2). A stream without `ldes:versionOfPath` or a timestamp path used to
  yield no members at all; each member is now its own entity.
- **MySQL / MariaDB datasources connect over TLS.** The `plugin-mysql` driver
  was built without a TLS backend, so `tls: true` against a server that offers
  TLS (MySQL 8 does by default) panicked inside the driver and every probe,
  introspection and run answered an opaque 500 — leaving cleartext as the only
  setting that worked. The driver now carries rustls over ring, as the
  PostgreSQL plugin does, with the bundled Mozilla roots plus a private CA from
  `options.sslrootcert`. Beyond MySQL: the host now contains a panic in any
  source driver as a named `driver failure` error (the detail goes to the
  server log, and that connection is retired), and a PostgreSQL TLS refusal
  says why (`invalid peer certificate: UnknownIssuer`) instead of only "error
  performing TLS handshake". CI's `live-sources` job gives PostgreSQL, MySQL,
  MariaDB and SQL Server a certificate from a throwaway CA and runs every
  driver once more with `tls: true`
  ([`scripts/live-sources-tls.sh`](scripts/live-sources-tls.sh)).
- **The OpenAPI document describes every route the server mounts.** About 60
  operations were missing from `/api-docs/openapi.json`, so Postman and
  generated clients could not see them: the OGC API – Features endpoints, 3D
  Tiles, the viewer feed and geo-stats probes, the built-in OIDC provider and
  its client registry, `/livez`, `/api/browse/facets`, the admin prefix
  overrides, `/api/docs`, `/api/plugins`, `/ldp/constraints` and LDP's
  `HEAD`/`OPTIONS` (plus `PUT`/`PATCH`/`DELETE` on the root container), and on a
  dataset `permissions/me`, `conformance`, `provenance`, `patch`,
  `entailment`, `containers/{import,export}`, `properties/*`, `form-manifest`,
  `ingest/cityjson`, `assets/{id}/{download,metadata}`, `versions/gc`,
  `versions/{ver}/diff/{other}` and `DELETE versions/{ver}`. A unit test now
  compares every `.route(...)` with the document, both ways, against a short
  list of deliberate exceptions. The Raft transport between cluster members
  is one of them and has left the document. Two documented paths that answered
  `404` are corrected: a dataset's SPARQL endpoint is
  `/api/datasets/{id}/services/{service}/sparql` (docs/embedding.md), and an
  IFC file is uploaded through `POST /api/import/bulk`
  (docs/geo-3d-platform.md).
- **R2RML natural lexical forms and language tags.** An SQL timestamp read
  as `xsd:dateTime` kept the space between date and time
  (`"2009-10-10 12:12:22"`, which is not an `xsd:dateTime`), and an SQL
  boolean stored as `0` / `1` stayed so; both now take their natural RDF
  lexical form (`2009-10-10T12:12:22`, `false`, R2RML §10.2), in literals and
  in templates. `rr:language` now refuses a well-formed tag whose primary
  language subtag cannot name a language (`english`: BCP 47 has no
  registered subtags of four to eight letters). An `rr:sqlQuery` /
  `rml:query` that ends in `;` no longer fails when it is wrapped as a
  subquery (a join, a column check, the PostgreSQL cursor), and the
  PostgreSQL connector reads a `CHAR(n)` value with its padding, as the
  server holds it, where the cast to text stripped it. Found by the W3C R2RML
  test cases (R2RMLTC0016c, R2RMLTC0015b, R2RMLTC0011a, R2RMLTC0015a,
  R2RMLTC0018a).
- **"Delete version" on a model page works.** The button has called
  `DELETE /api/models/:id/versions/:ver` since 0.1.0, a route that did not
  exist, so it always failed with `405`. The route now exists: admins, and
  publishers who may write the entry, delete one version, its graphs and its
  registry record in one transaction (a graph another version record also
  names is kept), recorded on the entry's commit log and in the audit log. A
  published version answers `409` unless `?force=true`; while datasets depend
  on the version (pinned to it, floating on it as the latest published one, or
  holding a dataset version that is not deprecated and conformed to it) it
  answers `409` whatever `force` says, listing the datasets the caller may read
  and counting the others. The model page shows those reasons and offers
  "Delete anyway" only when force would succeed.
- **Serving under a path prefix.** The web UI built with `OTS_BASE_PATH=/ots/`
  (or `docker build --build-arg OTS_BASE_PATH=/ots/`) now works below that
  prefix, behind a reverse proxy that strips it. Before, only the bundler knew
  the prefix: the router matched the raw `/ots/…` path against its routes and
  showed an empty page, and links, `config.json`, the API calls and the
  `/embed/*` detection all went to the host root. The router now matches the
  path below the prefix, and links, history entries, fetches, downloads, copied
  endpoint URLs and embed snippets carry it. The server scopes its session and
  OIDC-state cookies below the proxy's `X-Forwarded-Prefix`, so refresh and
  single sign-on keep working. The 303 from an IRI to its page in the UI uses a
  relative `Location`, and a 3D Tiles tileset refers to its content relative to
  `tileset.json`. A prefix that starts with one of the app's own paths (`/api`,
  `/sparql`, …) fails the build. nginx and Traefik configuration in
  [docs/operations.md](docs/operations.md#serving-under-a-path-prefix); a root
  deployment is unchanged. A Playwright smoke test (`npm run e2e:subpath`)
  builds the UI under `/ots/` and loads deep links.
- **SHACL enforces every value of `sh:not`, `sh:and`, `sh:or` and `sh:xone`.**
  SHACL Core (§4.6.1–4.6.4) lets a shape carry several values of each of these
  parameters, each a separate constraint. The shape loader read only one
  `sh:not` value and followed only one list per `sh:and`/`sh:or`/`sh:xone`, so
  a shape such as `ex:S sh:not [ sh:class ex:A ] ; sh:not [ sh:class ex:B ]`
  silently enforced one of the two and let data breaking the other validate
  as conforming. Each value now loads as its own constraint. Shapes graphs
  that split these constraints over several shapes as a workaround keep
  working unchanged.
- **SHACL result paths no longer render with a stray `>`.** The backend
  serialises a result's `path` in SPARQL path syntax (`<http://ex.org/label>`,
  `^<a>`, `<a>/<b>`, `<a>|<b>`, `<a>*`), and the UI shortened that string as if
  it were a bare IRI, showing `ex.org:label>`. The dataset validation dialog,
  `/validation`, `/shacl/results`, the shape-graph meta report and the source
  dry-run findings now shorten every `<…>` term and keep the operators, so a
  sequence path reads `ex.org:a/ex.org:b`; the tooltip keeps the raw path.
- **Docs and UI help text match the server again.**
  - `docs/datasets.md` said any signed-in user can read a `members`
    dataset. Only members of the owning organisation or group, and users with
    an explicit grant, can. It also said a grant combines with membership by
    taking the strongest role; in fact the grant replaces the membership role,
    except that an org/group admin is never demoted.
  - `docs/named-graphs.md` and the Graphs page placed the Graph Store Protocol
    at `/sparql?graph=`; it lives at `/store?graph=`.
  - The Datasets page's help (English and Dutch) advertised a per-dataset
    endpoint at `/api/datasets/{id}/sparql`, which does not exist. Each SPARQL
    service on a dataset answers at
    `/api/datasets/{id}/services/{slug}/sparql`.
  - The Datasets and organisation pages' help (English and Dutch) said a
    `private` dataset is visible to its owner only, and that every member of
    an organisation can see the datasets it owns. Organisation admins and
    users granted access can also see a `private` dataset, and plain members
    of the organisation cannot.
  - `docs/shacl.md` now says which writes are validated (Graph Store
    `PUT`/`POST`, bulk import, validate-and-commit). It also says that SPARQL
    Update skips every write gate, SHACL Studio pipelines and bindings
    included, not only `shacl_on_write`.
- **A published model's graphs read the same everywhere.** The graphs of a
  published model-registry version were served by
  `GET /api/models/{id}/versions/{ver}/data` to whoever may see the entry (a
  public entry: everyone), but were invisible to non-admins over `/sparql`
  (`GRAPH <iri>` matched nothing, a `FROM` naming it was dropped) and refused
  with `401` by `GET /store?graph=<iri>` — the seeded SKOS graph included.
  The readable-graph set that every read path scopes by (`/sparql`, the Graph
  Store, the SHACL Studio's read scope, dereferencing, the LLM context) now
  adds the base graph and sub-graphs of every published version of an entry
  the caller may see: a public entry to everyone, anonymous callers included; a
  private entry to its owner, the owner organisation's members and admins.
  Those graphs count as managed, so a private entry's graph is no longer an
  "unmanaged" graph to anyone. Writes are unchanged: a `SPARQL UPDATE` or
  Graph Store write into a registry graph still needs write authority on it,
  and a no-derivatives version still refuses every write.
  `GET /api/models/{id}/versions/{ver}/profile` moved from the admin-gated
  sources router to the data-model routes: it is read by exactly who may read
  `/data` (it answered `401` anonymously for a public model whose `/data`
  answered `200`). The API reference's auth table gains the model rows, and
  `tests/api_reference_auth.rs` probes them with a seeded vocabulary.
- **A seed bundle can no longer attach a model-registry graph to a dataset.**
  `apply_bundle` registered every `[[datasets.graphs]]` IRI through the internal
  path, past the refusal `POST /api/datasets/:id/graphs` applies to graphs the
  model registry holds. Seen in the field: the boot sweep released a legacy
  registration of a registry graph, and the bundle re-added the same graph in
  the same boot, making the model dataset-scoped again. A bundle dataset graph
  the registry holds (a version's base graph or sub-graph, or a graph under
  `{base}/data-model/`) is now skipped with a warning that names the rule, is
  not counted as registered (`SeedReport::graphs_refused` counts it), and the
  sweep that releases such registrations runs on every boot — one registry
  query — so a row that slips in by any path does not survive the next start.
- **`cargo check --all-targets --features full` builds without `test-utils`.**
  `tests/replication.rs`, `tests/ldp_conformance.rs` and `tests/query_cache.rs`
  use probes the library only exports with the `test-utils` feature
  (`InProcessLeader`, `AppState::test_default_with_store`, `query_cache_len`),
  so checking every target without it failed. Those three test targets are now
  declared in `Cargo.toml` with `required-features = ["test-utils"]`; the CI
  test job enables the feature, so they still run there, and auto-discovery of
  every other `tests/*.rs` is unaffected. The conformance table's totals are
  regenerated for the three suites this change set adds.
- **The supported-versions tables were five releases old.** `SECURITY.md` and
  `SUPPORT.md` still named `0.2.x` as the current line. They now list `0.7.x`
  as Active and `0.6.x` as Security-only until 0.8.0, and the prepare workflow
  keeps them current.
- **The UI says when a validation or inference run left graphs out.** A run
  that could not read every graph or shapes graph of a dataset answers
  `partial: true`: a validation run is then a test run, not recorded, and an
  inference run skips the rules it may not read. The dataset page, the
  Validation page, the SHACL results page, the import wizard's pre-validation
  and the shapes editor's Infer now show a note when that happens. Asking for
  an official run on the SHACL results page used to reload the unchanged
  latest run, so the click seemed to do nothing; the page now shows the test
  run it got. In the same pass:
  - The import wizard's pre-validation read the verdict off the response
    envelope instead of its report, so it always showed "undefined issue(s)
    found", even for conforming data.
  - The dataset page's validation dialog showed the escape `\u2014` as text
    where its summary line has a dash.
- **The bundled Open Triplestore ontology names all ten graph roles.** The
  catalogue writes `ots:graphRole` values for `DomainValues`, `Linkset`,
  `Provenance` and `Catalog`, but the `ots-ontology` demo dataset defined only
  the first six roles, as `GraphRole` individuals and as SKOS concepts. New
  installs get all ten; existing ones keep their seeded copy.
- **GitLab CI runs the gates GitHub runs.** The frontend job type-checks
  (`npm run typecheck`); e2e uses Node 24; `cargo-deny` covers every feature;
  new `minimal-build` (`--no-default-features`) and `plugins` (each plugin crate
  on its own) jobs; and the perf job self-tests the gate, screens softly and
  re-benches flagged benchmarks before failing, and clears each pass's
  directory first — the cached `target/` could nest one pass inside the last
  and switch off every tolerance key.
- **Docs that contradicted the code.**
  - Graph roles: the styleguide said six (there are ten) and gave them an
    `…/ns/role#` namespace (they are `https://opentriplestore.org/ns#`);
    `docs/datasets.md` had its `catalog` row stranded below the table.
  - `docs/rml.md` and `docs/standards.md` said SQL and SPARQL sources and joins
    were not implemented; `standards.md` also listed SHACL-AF custom constraint
    components, `sh:ask` validators and rule `sh:condition` / `sh:order` as
    missing.
  - `docs/sparql-12.md` called `LATERAL` planned and rejected by the parser (it
    works) and let triple terms be subjects (RDF 1.2 allows objects only).
  - `docs/owl2-el.md` named `Owl2ELReasoner` (the type is `El2Classifier`), and
    the reasoner examples passed a string to `TripleStore::open`, which takes a
    `Path`. The OWL 2 DL docs said keys of more than two properties produce no
    `owl:sameAs`; the RL phase merges keys of any length.
  - `docs/dcat.md` said VoID statistics are computed per request and never
    cached; they are cached until the next write.
  - `docs/data-modeling.md` said SHACL-on-write covers LDP writes; it covers
    Graph Store writes to dataset graphs only.
  - `docs/administration.md` said interactive OIDC sign-in ignores group
    claims; it maps `groups` and `roles`. `docs/auth.md` left the `guest` role
    and `OTS_GUEST_CAPABILITIES` out.
  - `docs/development.md` and `docs/windows.md` put `saml` in `full` (it is not);
    the native Windows build drops its hand-written feature list for the
    default `full`.
  - `docs/triplestore-comparison.md` marked federation, OWL 2 EL, RL and DL,
    ShEx, SWRL and RML full where `docs/standards.md` grades them Partial; the
    standards score is recounted, 23 → 16 of 29. Its GeoSPARQL note still
    called the geodesic metric functions, `aggUnion` and GeoJSON missing.
  - `docs/performance.md` showed a whole-store `COUNT(*)` scanning (7.09 ms at
    10k); the count index answers it in ~0.14 µs at every size.
  - `PRIVACY.md` listed Leaflet from unpkg and OpenStreetMap tiles; the UI
    bundles its map libraries and fetches OpenFreeMap and, with an operator's
    key, Esri imagery. `frontend/index.html` no longer pre-resolves an Esri
    host the code stopped using.
  - `CONTRIBUTING.md` told contributors to use `--all-features`, which needs
    native SFCGAL; it now gives CI's feature set and the typecheck step.
    `.github/RELEASE_TEMPLATE.md` used H2 groups where release notes and the
    `### Security` / `### Deprecated` check use H3.
  - `frontend/public/vocab/NOTICE.md` counted 17 files from LOV; DOAP's
    replacement left 16.
- **Feature docs that overstated the code.**
  - `docs/sparql-12.md` now follows Appendix A of the SPARQL 1.2 Working Draft
    of 2026-10-01. `LATERAL` and `ADJUST` are listed as Oxigraph's SEP
    extensions, not SPARQL 1.2; `CALL` and "COUNT deduplication" (neither in
    the draft) are gone. The `ADJUST` example passed a string offset, which the
    built-in answers with unbound; it now uses an `xsd:dayTimeDuration`, and
    the non-standard `sparql:adjust` function (reached only by its IRI) is
    described separately. README, the FAQ and `docs/datatypes.md` describe
    `<< >>` as RDF 1.2 reifier shorthand and base-direction strings as
    supported; `docs/datatypes.md` warns that derived integer types are stored
    as `xsd:integer`.
  - OWL docs: `docs/owl2-el.md` claimed nominals and listed completion rules
    the code does not have, and quoted unmeasured SNOMED/GO timings; its rule
    table now matches the code, including the unsound CR3. `docs/owl2-rl.md`
    mis-described `cls-uni`, `cls-svf1/2`, `cls-maxc`, `cls-nothing*` and
    `cls-thing`, and said literals are compared by value (rules match terms).
    `docs/owl2-ql.md` claimed PerfectRef and now lists the rewriter's limits.
    `docs/rdfs-entailment.md` showed `RdfsMaterializer::new`, an
    `Accept-Entailment` header and an `AppState` option that do not exist.
  - `docs/owl2-dl.md` called Konclude Apache-2.0 (it is LGPL-3.0) and gave an
    install recipe and a stdin hand-off that cannot work; the bridge is now
    documented as experimental and not working. `docs/reasoning.md` no longer
    names HermiT and Pellet as if they were wired.
  - `docs/geosparql.md` lists every implemented function and what is missing;
    the GeoSPARQL test header and `scripts/run_tests.sh` no longer say the
    tests derive from the GeoSPARQL Compliance Benchmark or test "OGC
    conformance".
  - `docs/shacl.md`: the validate example showed camelCase keys and a
    component IRI; the response is snake_case with a readable constraint label.
    The SHACL-C serializer was said to keep what it cannot express as
    comments; it drops it, and the doc now lists what.
  - Design-note status lines (delta versioning, analytical mirror),
    `docs/datasets.md` (change capture is not roadmap) and the 2026-06
    reference tables in `docs/performance.md`, now dated.

- **The Konclude bridge works.** It piped Turtle on stdin, but Konclude reads
  only OWL/XML or functional-style syntax, from files. It also passed an
  undocumented `-f` flag, guessed consistency from the log text, had no time
  limit and kept only subclass edges. It now:
  - writes functional-style syntax mapped from the RDF;
  - asks for consistency (`IsKBSatisfiable`), classification and realisation
    (types and `owl:sameAs`) over OWLlink, and for object property assertions
    over SPARQL;
  - kills the process at the time limit.

  Checked against Konclude v0.7.0-1138. Data values entailed through
  `owl:hasValue` are not reported by Konclude and not materialised.

- **Konclude: wrong answers for `owl:datatypeComplementOf`.** Konclude
  v0.7.0-1138 says "consistent" for some inconsistent data complements, for
  example `∃p.(xsd:integer ⊓ ¬xsd:integer)`, `∃p.¬rdfs:Literal`, or a range
  `¬xsd:integer` with the value `1`. The bridge now writes `¬rdfs:Literal` as
  the empty data range, so the entailment check for a `rdfs:Literal` range no
  longer answers "not entailed". When the input still contains a data
  complement after that, a check answers `unknown` instead of consistent, not
  entailed or satisfiable. A materialisation reports `complete: false` with a
  warning. Answers that rest on a clash (inconsistent, entailed,
  unsatisfiable) are unchanged. See `docs/owl2-dl.md`.

### Security
- **A signed-in caller's DCAT catalogue was marked `Cache-Control: public`.**
  `/.well-known/void` and `/{org}/.well-known/void` are scoped to the caller
  (their datasets, their readable graphs' statistics), but every response said
  `public, max-age=60` with `Vary: Accept` only, which lets a shared cache
  store a response to a request with credentials and serve it to others. A
  signed-in caller's copy is now `private`, and the responses vary on
  `Authorization` and `Cookie` too.
- **SAML responses are checked more strictly.** Signatures must use RSA or
  ECDSA with SHA-256, -384 or -512 (samael accepted any algorithm, SHA-1
  included, when none was configured); a message with a DOCTYPE or larger than
  1 MiB is refused before libxml parses it; a provider without an IdP signing
  certificate never starts a sign-in (samael skips verification without one);
  a refused response gets a generic error, with the reason in the audit log
  only; request IDs carry 128 random bits. RSA PKCS#1 v1.5 key transport in
  encrypted assertions is refused.
- **Property states of a private graph stay private.** A state written with
  `graph` naming a private dataset graph mirrored its value into the states
  graph, which every viewer of the dataset could read through
  `…/properties/history` and `…/as-of`. Each state now records its data graph
  (`ots:dataGraph`), and every property-state read and the export leave out
  states whose graph the caller may not read.
- **Spark's streaming endpoint no longer sends internal error text.** On
  `POST /api/llm/chat/stream` a server fault reached the browser verbatim in
  the `error` event and in a failed query's result; it is now "Internal server
  error", with the detail in the server log, as on the JSON endpoints.
- **Backup manifests cannot point outside their backup.** `verify` and the S3
  upload joined the manifest's file names onto the backup directory without
  the check restore makes, so an edited manifest with an absolute or `..` path
  could have the server hash, or upload to S3, any file it can read. All three
  now refuse such a manifest.
- **Audit rows and the guest AI budget record the real client IP.** Both took
  the left-most `X-Forwarded-For` entry (then `X-Real-IP`) from any caller and
  never saw the TCP peer address, so a login failure, a permission denial or an
  SSO failure could be attributed to an IP of the caller's choosing. A random
  header also bought a fresh guest budget on the AI endpoints
  (`LLM_RATE_LIMIT_ANON_PER_MIN`). On a deployment without a proxy, audit rows
  carried no IP at all and every guest shared one budget. They now derive the
  client IP the way the per-IP rate limiter already did: the TCP peer address,
  with forwarded headers believed only when the peer is inside
  `TRUSTED_PROXY_CIDRS` and the chain read right to left. **Behaviour change:**
  behind a reverse proxy, set `TRUSTED_PROXY_CIDRS` to its address range, or
  every audit row and guest budget is keyed on the proxy's address.
  `docs/administration.md` and `docs/operations.md` say so.
- **A `sh:SPARQLFunction` stored in any graph could redefine functions for
  every caller.** Definitions were collected from the whole store and
  registered into every query's evaluator after the built-ins. The SPARQL
  engine consults custom functions before its `xsd:` casts, and the last
  registration of an IRI wins, so any writer of any graph could redefine
  `xsd:integer(…)`, `geof:sfWithin` or a function another tenant's shapes call,
  for other tenants' `/sparql` queries, `sh:sparql` constraints, write gates and
  pipelines. A function now belongs to the runs of the shapes graph that
  declares it: validation, inference, Studio pipelines and write gates that use
  that graph. `/sparql`, SPARQL Update and the reasoners see only the server's
  own functions and those in the graphs an admin names in the new
  `OTS_SPARQL_FUNCTION_GRAPHS` setting. Only `urn:system:functions` and graphs
  under `urn:system:functions:` can be named there, so only an admin can write
  them. No function may take an IRI in the `xsd:`, `rdf:`, `rdfs:`, `owl:`,
  `sh:`, `sparql:`, XPath or GeoSPARQL namespaces or one the server registers
  (GeoSPARQL, 3D, RDF 1.2, `ADJUST`). A shapes graph that declares one fails
  its run, naming the function. A designated graph's definition is skipped
  with a warning instead. **Upgrade note:** a query that called a function
  stored in an ordinary graph now fails with an unsupported-function error. So
  does a constraint whose shapes graph calls a function that only another
  shapes graph declares, which is now reported as unevaluable. Move the
  definition into a designated function graph, or into the shapes graph that
  calls it. Tests:
  `tests/sparql_scope_boundary_http.rs`, `tests/shacl_conformance.rs`.
- **ShEx validation read the whole store.** `POST /api/shex/validate` and
  `POST /api/datasets/{id}/shex/validate` matched triples and discovered focus
  nodes in every graph, whoever asked. The dataset route checked access to the
  dataset and then ignored it. A report names its focus nodes, and a verdict
  such as `PATTERN "^123"` answers a question about the data, so any signed-in
  user could probe private graphs and other tenants' datasets. Both routes now
  read what `/sparql` lets the caller read (admins: everything). The dataset
  route reads only that dataset's graphs, without its stored report graphs. A
  triple held by two graphs in scope is now counted once. Tests:
  `tests/dataset_validation_read_scope_http.rs`.
- **A padded version label no longer reads a dataset's private snapshots.**
  The dataset service (`GET`/`POST /api/datasets/{id}/services/{slug}/sparql`)
  trimmed `version` before resolving the pinned version's snapshot graphs, but
  handed the private-graph filter the raw label, which the version registry
  refuses. With whitespace around the label (`?version=1.0.0%20`, a `+`, a tab,
  or the same in a form body) the filter found no version and withheld nothing,
  so a viewer, or an anonymous caller on a public dataset, read the snapshots
  of graphs flagged private. Both lookups now use the one trimmed label, and
  the filter fails closed: a pinned version it cannot read, or private flags it
  cannot list, refuse the read, and a snapshot graph the version's map ties to
  no source is withheld from non-writers, as the version-data and saved-query
  paths already did. Every release since 0.4.0 is affected. Tests:
  `tests/security_routes.rs`.
- **SWRL rules can no longer fire without their guards, and the target graph
  is checked.** The OWL/XML reader behind `POST /api/swrl/execute`
  (`format: "xml"`) matched only `BuiltinAtom`, but the OWL API and Protégé
  write `BuiltInAtom`. That element, like any atom element it did not know
  (`DataRangeAtom`, RDF/XML's `IndividualPropertyAtom`), was dropped, so its
  rule ran with one condition fewer. A `ClassAtom` over a class expression was
  read as its inner class (`ObjectComplementOf(A)(?x)` became `A(?x)`), and
  `ObjectInverseOf(p)` lost its direction. `swrlb:stringConcat` became a
  FILTER that every non-empty string passed. The reader now understands every
  element inside a `DLSafeRule` or refuses the document with an error naming
  the element. It accepts both built-in spellings and swaps the arguments of
  `ObjectInverseOf`. It refuses class-expression atoms, `DataRangeAtom`,
  anonymous individuals and prefixed names until they are supported, and
  `stringConcat(?r, …)` now means `?r = CONCAT(…)`. Built-ins are recognised
  only in the `swrlb:` namespace, and `matches` with more than three
  arguments is refused rather than truncated. `target_graph` was pasted into
  the generated update as `GRAPH <…>`; it must now be an absolute IRI (`400`
  otherwise). Execution runs off the async runtime, under the
  expensive-operations limit, and stops at the write timeout
  (`write_timeout_secs`).
- **RDF Patch passes the SHACL write gates.** `POST /api/datasets/:id/patch`
  skipped the gates a Graph Store write to the same graph passes. It now runs
  Studio `gate_writes` pipelines, bound shapes and the dataset's
  `shacl_on_write` shapes over what each touched graph would hold after the
  patch, and refuses with a `422` and the report.
- **The public map and 3D viewer endpoints are throttled, cached and off the
  async runtime.** The viewer feed, geo stats (per dataset and batched), the
  public asset download and both 3D Tiles routes are reachable anonymously for
  public datasets, but had no rate limit; the 3D Tiles routes rebuilt the whole
  dataset on every request inside the async handler. They now share a per-IP
  rate limit (60 a minute, burst 40, its own bucket), the store work runs on
  the blocking pool, and the tileset and GLB are cached per dataset, caller
  read scope and store write generation.
- **Known advisories now in the image, through the SQL Server driver.** With
  `plugin-mssql` in the image (see *Changed*), tiberius 0.12.3, the latest
  release, brings rustls 0.21 and rustls-webpki 0.101 with it. Their
  advisories RUSTSEC-2026-0098 and RUSTSEC-2026-0099 (name constraints) need a
  misissuing name-constrained CA among the roots a SQL Server connection
  trusts, and RUSTSEC-2026-0104 (a CRL parsing panic) needs CRL checking,
  which tiberius never turns on; rustls-pemfile 1 is unmaintained
  (RUSTSEC-2025-0134). Only SQL Server connections use this stack. An image
  built with `CARGO_FEATURES=full,plugin-postgres,plugin-mysql` leaves it out.
- **Every configured secret goes through the secrets module.** `JWT_SECRET`,
  `LD_REGISTRY_TOKEN`, a replication follower's `OTS_REPLICATION_TOKEN` and the
  accounts-dashboard plugin's `ACCOUNTS_DASHBOARD_GATEWAY_KEY` were read as raw
  environment values, though the module's own documentation listed the JWT
  signing secret among the settings it covers. Each now takes a secret
  reference (`env:NAME`, `file:/path`, `vault:…`) that resolves at startup or
  at use, and a raw value is refused under `OTS_ENV=production` (warned about
  once in development), as `S3_SECRET_KEY` already was: a raw `JWT_SECRET` or
  `LD_REGISTRY_TOKEN` stops the server at startup with the module's
  `RawSecretRefused` message, a raw replication token or gateway key is dropped
  with an error in the log. Plugins get the same rule through a new
  `PluginSecrets` capability on `PluginContext` (`ots-plugin-api`), so a
  plugin never reads a credential variable itself. `.env.example`,
  `docs/administration.md` and `docs/sources.md` say so.
- **The LLM feedback relay screens every string in the signal.**
  `POST /api/llm/feedback` forwards a training signal to the gateway's
  `/v1/signals` with the server's key attached, and screened only the signal's
  top-level string fields for size and injection — while the free text a
  pipeline ingests sits nested (`input.nl_question`, `label.comment`,
  `output.*`) and reached the gateway unscreened. The relay now walks the JSON
  recursively, to a bounded depth of 16 levels, applies the same per-field,
  count and whole-conversation caps and the injection heuristics to every
  string leaf, and refuses a signal larger as a whole than a conversation may
  be, so bulk cannot hide in numbers or nesting. A nested oversized or
  injection-flagged field, or a signal nested past the bound, answers `400`
  before anything reaches the gateway; the signals the UI sends relay as
  before.
- **LDP resources were readable and writable by every signed-in user, and
  `PATCH` could rewrite the whole default graph.** Every `/ldp/` verb now
  checks the resource's WAC ACL: `acl:Read` for `GET`/`HEAD`, `acl:Write` for
  `PUT`/`PATCH`/`DELETE`, `acl:Append` on the container for `POST` and a
  creating `PUT`, `acl:Control` to read or change an ACL. Admins pass; a
  failed membership or ACL lookup refuses the request. `PATCH` no longer runs
  its SPARQL Update against the store: it is restricted to `INSERT DATA`,
  `DELETE DATA` and `DELETE/INSERT WHERE` on the default graph, evaluated on
  the resource's own triples only, and its result may not describe another
  resource under `/ldp/` or a server-managed triple; `PUT` and `POST` bodies
  are held to the same rule, so a write authorized for one resource cannot
  reach another through its body. On upgrade the open root ACL keeps every
  request that worked before working; operators who want a closed space set
  `LDP_ROOT_ACL=owners` before first start or tighten `/ldp/.acl` afterwards.
  Tests: `tests/ldp_wac_security_http.rs`.
- **`SERVICE <urn:source:id>` used a datasource's stored account for any
  caller.** A local query naming a registered virtual source was sent to its
  endpoint with the account the source was registered with, whoever asked —
  including an anonymous caller of `/sparql`. The account is now the source's
  to share: it resolves for an administrator, for the source's owner, and for
  a signed-in user who holds a role on the dataset the source is bound to (as
  its owner, a member of the owning organisation or group, or through a
  grant). A public dataset's visibility alone does not count, and an
  anonymous caller never qualifies. For anyone else the source does not
  exist: the `SERVICE` fails exactly as one naming an unregistered source
  does, and the endpoint never sees the account. Live queries over a source
  are served by `/sparql` and SPARQL Update; a query evaluated anywhere else
  — a path that does not say whom it acts for — resolves no source. Tests:
  `tests/sources_virtual_http.rs`.
- **A SPARQL Update could probe a graph its writer cannot read through
  `EXISTS`.** The update's read-side ACL walked the `WHERE` clause's patterns
  but not its expressions, so `INSERT { GRAPH <mine> { … } } WHERE { FILTER
  EXISTS { GRAPH <other> { … } } }` told a writer of `<mine>` whether
  `<other>` held a triple. Every `EXISTS` / `NOT EXISTS` — in a `FILTER`, a
  `BIND`, an `OPTIONAL`'s condition, an `ORDER BY` key or an aggregate — is
  now checked like the rest of the clause: a named graph needs read access,
  and a variable graph or a `SERVICE` inside one makes the update admin-only.
  LDP `PATCH` applies its no-`GRAPH`, no-`SERVICE` rule inside `EXISTS` too.
  Tests: `tests/security_graph_acl_protocol_parity.rs`.
- **The catalogue's aggregate VoID statistics counted private graphs.** The
  whole-store dataset in `/.well-known/void` (`void:triples`,
  `void:distinctSubjects`, `void:distinctObjects`, `void:properties`,
  `void:documents`) counted every graph, private and system ones included,
  for anonymous callers too, and each dataset's entry listed its private
  graphs as `void:subset`s and counted them in its `void:triples` for anyone
  who could see the dataset. Both now stay inside the graphs the caller may
  read over `/sparql` (an anonymous caller: the public ones), as the service
  description at `/` already did: a private graph is listed and counted for
  its dataset's writers only. An administrator still sees the whole store.
  Tests: `tests/dcat_ap_http.rs`.

## [0.7.0] — 2026-09-28

SQL datasources end to end: relational data is profiled, mapped, dry-run,
gated, materialised, validated, reviewed and watched for drift inside the store,
with PostgreSQL, MySQL / MariaDB and SQL Server connectors, virtual SPARQL
sources and an external writeback worker. GeoSPARQL 1.1 gains GeoJSON literals,
the geodesic `metric*` functions and `geof:aggUnion`. Bundled third-party data
now ships only as its licence allows, with its notices. And a sweep of the whole
API for authorization gaps. Before upgrading, read **Security**: several
endpoints now answer 403 or 404, or return less, where earlier releases leaked
data or allowed writes they should not have. Also read **Changed**: `uom:metre`
on longitude/latitude geometry is now geodesic metres.

### Added
- **Third-party data carries its licences, and the image ships only what may
  be redistributed.** A licence audit of every vendored dataset, vocabulary,
  test suite and bundled binary (each verdict challenged against the rights
  holder's own terms) led to:
  - **LOV:** the catalogue records each vocabulary's own licence
    (`license`, `license_declared`, `license_status`, `redistributable` on
    every vocabulary record; `license_url`, `license_scope` and
    `modifications` on the source) instead of stamping LOV's CC BY 4.0 on
    everything. Descriptions copied from vocabularies whose licence does not
    allow redistribution are gone. The Docker image bakes only the corpus
    graphs whose licence allows a verbatim copy (listed in
    `assets/vocab/lov-redistributable.txt`); the full dump still works when an
    operator supplies it (`VOCAB_CORPUS_PATH`). An install records the
    vocabulary's own licence, and one whose licence does not allow
    redistribution installs privately. The vocabulary search shows each
    licence.
  - **Notices and licence texts:** a rewritten `NOTICE` with correct holders,
    licences, sources and modification statements; `LICENSES/` with the texts
    the bundled material requires (shipped at `/app/LICENSES/`);
    `frontend/public/vocab/NOTICE.md` (served at `/vocab/NOTICE.md`) with one
    entry per bundled vocabulary and the notices their licences require, also
    in the header of each file that has one (imbor.ttl is shipped unmodified,
    with no header); licence files and provenance for the W3C SPARQL, W3C
    SHACL and OGC GeoSPARQL test suites; the libraries statically linked into
    `web-ifc.wasm`; the Lucide/Feather icons, IFC schema names, EPSG
    parameters and LOINC codes credited where they are used.
  - **Credits on screen:** the 3DBAG credit shows on the Cesium globe, embeds
    and 3D previews as well as the map, and 3D Tiles carry it as glTF
    `asset.copyright`; the OpenStreetMap/CARTO and Esri basemap credits on the
    globe are shown on screen instead of behind a pop-up.
  - **Term search serves only what may be redistributed.** The term index
    skips every LOV vocabulary whose licence does not allow redistribution,
    whatever corpus is mounted (a full dump included); existing indexes are
    rebuilt on first boot. Each shipped graph's required notice is filled in
    per graph (catalogue `license_notice`, and a notice column in
    `lov-redistributable.txt`); four vocabularies are withheld because LOV's
    copy is not faithful to a work that allows no modification, the licence's
    required copyright line is not published, or the licence's version is not
    stated (`redistribution_withheld`). The OGL, Flemish-licence and ISA
    graphs carry their required notices, the ISA No Warranty disclaimer
    included; where LOV mis-decoded characters in a graph whose licence allows
    modification (14 graphs), its notice says so. The catalogue adds
    `license_uris`, `no_derivatives` and `lov_misdecoded`.
  - **LOV installs carry a licence record; earlier installs are checked.**
    Each version installed from the LOV corpus gets a licence record (licences
    with URIs, the required notice, the source, and a link to the new
    `/api/vocab/notice` licence page); downloads carry `rel="license"` links,
    and a CC BY-ND or OGC Document Notice vocabulary cannot be drafted,
    branched or edited (403). The copy check compares with LOV's copy as the
    store holds it (typed literals in its canonical form, with the same
    values). Every start checks for LOV installs of earlier releases,
    recognised only when everything their installer did holds (its exact
    note, an entry with no owner under the LOV prefix and namespace, the
    conventional version graph, an admin creator), so a user's model that
    copies the note is never touched. Vocabularies that may not be
    redistributed, and no-derivatives ones, are made private once (each is
    logged; an admin may make them public again, and later starts leave that
    alone), and each earlier install gets a licence record that names the
    vocabulary's own licence and says the earlier release may have added an
    `owl:versionInfo` triple and that the graph may have been modified. Their
    graphs and notes are not changed. Followers and Raft members that do not
    lead leave the check to the leader. Superseded LOV term indexes are
    removed from disk.
  - **Seeded vocabularies carry their licence.** Each bundled vocabulary
    seeded into the model registry has a licence and attribution record
    (licence names and URIs, copyright, required notice, document status,
    source, changes, a link to `/vocab/NOTICE.md`), stored as registry
    metadata — never in the vocabulary graph — and returned as `attribution`
    by `/api/models`; the model pages show it and the term card's source pill
    links to the notice. Downloads carry `Link` headers and the file's header
    as comments (IMBOR: headers only), and the server serves
    `/vocab/NOTICE.md` itself. Seeded and LOV-installed graphs are loaded
    unchanged: the loader used to add an `owl:versionInfo` triple to files
    that state none. On earlier installs, a seeded copy whose only difference
    from its file is that triple has it removed at the next start and is
    recorded as unchanged; a copy with any other difference, and every LOV
    install, keeps it, and their licence records say the copy differs from
    the file or may have been modified.
    A download calls its content the bundled file, unchanged, only when the
    seeder has checked it: each seeded version's record keeps the SHA-256 of
    its file and a digest of the stored triples, and `attribution.unchanged`
    says whether the check passed. Drafts, branches, merges, rebases and
    edited copies keep the licence records of the versions they draw on, and
    say they may have been modified. In the IMBOR entry (and any entry whose
    licence record allows no altered copies), uploading, editing, drafting,
    branching, merging, rebasing and publishing are refused (403), and only
    the checked copy is served to users who cannot write the entry.
  - **Seed bundles can declare a model's licence.** A `[data_models.license]`
    table (`licenses`, `copyright`, `source`, `notice`, `changes`, `remarks`,
    `notice_url`, `no_derivatives`) becomes the model's licence record, checked
    against the bundle's files on every start; the `nen2660-imbor` example
    declares CROW's licence with `no_derivatives = true` for `imbor-otl`. A
    bundle model may no longer use the id of a vocabulary the server seeds.
  - **Dependency notices ship with the build.** `npm run build` writes
    `dist/THIRD-PARTY-LICENSES.txt` (served at `/THIRD-PARTY-LICENSES.txt`)
    with the licence and notice files of every npm package the web UI bundles
    or copies, Cesium's third-party modules included, and of the material
    bundled outside npm — the libraries statically linked into
    `web-ifc.wasm`, the Lucide/Feather icon shapes drawn inline and the EPSG
    attribution, whose texts the build takes from `LICENSES/`; the Docker builder
    writes `/app/THIRD-PARTY-LICENSES-server.txt` for every crate linked into
    the server (`scripts/gen_rust_third_party_licenses.py`).
  - **The prefix snapshot documents its sources:** prefix.cc (no licence
    published for the data; the operator's stated public-domain intent) and
    the LOV-derived entries (CC BY 4.0, with the modification statement); the
    Turtle and SPARQL exports of `/api/prefixes/all` open with a credit
    comment.
- **The database connectors run against live servers in CI.** A
  `live-sources` job (GitHub Actions and GitLab alike) starts PostgreSQL 16,
  MySQL 8.4, MariaDB 11.4 and SQL Server 2022 as service containers and runs
  each driver's live test plus the whole pipeline over HTTP through the
  PostgreSQL plugin. `OTS_TEST_LIVE_REQUIRED=1` turns a missing server
  variable into a failure instead of a skip, and each test retries its
  administrator connection while a server starts.
- **The real-data seed bundles run in CI.** NEN 2660-2 and GWSW are not
  vendored — NEN 2660-2 carries no licence that allows redistributing it,
  GWSW's ontology states none — so the conformance job downloads the NEN
  2660-2, IMBOR and GWSW payloads from their publishers with each bundle's
  `fetch.sh`, and `OTS_TEST_SEED_PAYLOADS_REQUIRED=1` fails a missing payload
  where the tests used to skip green. The NEN 2660-2 fetch now reads from
  NEN's own repository, which took the files over from DigiGO's.
- **No test is ignored, and CI keeps it that way.** `scripts/no-ignored-tests.sh`
  rejects an `#[ignore]` — plain, with a reason, or behind a `cfg_attr` for
  one platform — and the backend test step fails when cargo reports any
  ignored test, doctests included.
- **`ots-writeback`, the external writeback worker** (`tools/writeback`, a
  separate binary — never inside the store). It follows a dataset's LDES
  stream from where it left off, reads an RML mapping backwards — a table
  source, a one-placeholder subject template or a column subject, and
  column-valued objects; anything else is reported and left alone — and
  upserts the changed entities into SQLite or PostgreSQL with a writing
  account of its own, one transaction per fragment, a tombstone as a delete.
  Secrets are `env:` / `file:` references, never command-line values;
  `--dry-run` prints the SQL, `--from-file` works without a store.
- **Virtual sources: a SPARQL endpoint as a datasource** (dialect `sparql`,
  in core). An Ontop virtual knowledge graph — or any endpoint — is
  registered like a database, with the same secret reference and the same
  allowlist, and introspected as class-tables: each class a table, `subject`
  its key, the predicates its columns. A mapping reads a class with
  `rr:tableName` or a SPARQL `SELECT` as `rml:query`; a run of it is a run
  like any other. A **snapshot run** (`mode: snapshot`) materialises the
  endpoint's whole graph with no mapping — gated, swapped in, reviewable —
  and `SERVICE <urn:source:id>` in a local query resolves to the endpoint
  with its credential, so the source is also queryable live.
- **PostgreSQL, MySQL / MariaDB and SQL Server datasource connectors** as
  plugins (`plugins/postgres`, `plugins/mysql`, `plugins/mssql`; features
  `plugin-postgres`, `plugin-mysql`, `plugin-mssql`), each keeping the
  connector contract in its dialect's terms: read-only enforced server-side
  (`default_transaction_read_only`, `SESSION TRANSACTION READ ONLY`, and for
  SQL Server a role check at connect that refuses a writing account), a
  statement timeout on everything (a MySQL server that knows neither
  `max_execution_time` nor `max_statement_time` is refused), rows streamed
  in batches (PostgreSQL through a server-side cursor), every column typed
  from the statement's own description and carried as text in the lexical
  shape the natural datatype mapping expects, and TLS through rustls with
  `options.sslrootcert` for a private CA and no trust-all switch. The three
  share one `INFORMATION_SCHEMA` catalogue and one aggregate profiler in
  `ots_plugin_api::sources::catalogue`. Each crate carries a live test that
  runs when `OTS_TEST_<DIALECT>_HOST` is set.
- **The mapping proposer's scoped access.** Two API-token scopes name what
  an external proposer may do and nothing else: `sources:read` reads the
  datasource registry, profiles, mappings, runs, tickets, the mapping gates
  and the ontology profile, and `mappings:propose` creates and refines a
  mapping in the `proposed` state and dry-runs it. A non-admin's view of a
  datasource omits its location — host, port, database, account and the
  credential reference — and never reaches `/preview` or the review queue:
  the proposer never receives a DSN or a row. A proposer that names any
  other state, or refines a mapping a reviewer has moved on, is refused.
- **Review decisions as PROV, and calibration**
  (`POST /api/mappings/{id}/decisions`, `GET /api/mappings/{id}/reviews`,
  `GET /api/mappings/{id}/provenance`, `POST /api/sources/calibration`).
  Approve, edit and reject are three distinct `ds:ReviewDecision`
  activities, each naming the mapping version it judged, the reviewer, the
  confidence the proposal carried and the note; an approval moves the
  mapping on and re-baselines drift. The decisions list is the proposer's
  training data, and the calibration endpoint fits stated confidence to
  observed acceptance by isotonic regression — refusing one-class data,
  which would assign its single outcome to every confidence.
- **Review items, the deterministic fixer and promotion**
  (`GET /api/sources/{id}/reviews`, `GET /api/reviews/{id}`,
  `POST /api/reviews/{id}/status`, `POST /api/reviews/{id}/autofix`,
  `POST /api/reviews/{id}/suggest`, `POST /api/runs/{id}/promote`). A run
  the gate refuses opens one review item per subject with violations in
  `urn:system:reviews:<datasource>`, with the violations and a snapshot of
  the subject, capped per run by `OTS_REVIEW_MAX_ITEMS`. The fixer applies
  two rules only — a sign typo against a non-negative `sh:minInclusive`, and
  a clamp to an inclusive bound — previews the change as an RDF Patch and
  applies it through the store's patch path; anything that would need an
  invented value is left to a human with a 422. A human sets an explicit
  status with a note; the model-assisted suggestion sends the constraints,
  the values only where the datasource allows model assistance, and applies
  nothing. Promotion re-gates the corrected candidate and gives it the
  production role exactly as a passing run would, recorded as a
  `ds:Promotion` activity on the run's PROV trail naming who released it.
- **Studio: Explore, Map and Dry-run in the Sources workspace.** Explore
  shows the profile the store computed — per table and column the counts,
  code lists, detected patterns, keys and foreign keys, in a simple and an
  advanced view — profiles on demand, checks drift against the mapping's
  baseline and lists the re-map tickets it opens. Map is three views of one
  mapping graph: a matrix of what each triples map reads, mints and asserts
  with every predicate's object described in a word, the Turtle itself in
  the editor (save as a new version, or register as a new mapping), and a
  YARRRML composer; a legacy `mapping.sql2rdf.yaml` bundle converts straight
  into the editor. Dry-run takes a registered mapping or the editor's
  unsaved content, a table and a sample size, splits the result into
  mapping defects and data issues, shows every entity with its own Turtle
  and violations, and keeps each attempt as a round. The Sources page
  carries the mapping gates as an editable card.
- **Dry-run: a sample of a mapping, validated, with every violation
  classified** (`POST /api/sources/{id}/dry-run`). A registered mapping, a
  version graph, or unregistered RML / YARRRML — what the proposer sends
  before it writes a proposal — is materialised into a scratch graph
  `urn:dryrun:<id>` from a few rows per triples map, and the sample is closed
  under its joins: every row a sampled row references through
  `rr:parentTriplesMap` is fetched by key and mapped too, so one row of a
  child table does not fake an `sh:class` violation on every reference. The
  sample is validated against the shapes named in the request, the mapping's
  or the model version's, and each violation is classified: one hitting at
  least `systematicShare` of a type's subjects over at least
  `systematicMinSubjects` of them is a **mapping defect**, anything sparser a
  **data issue**. The response carries per-entity Turtle with each entity's
  violations, the classification, the report and what each triples map
  contributed. Scratch graphs live for `OTS_DRYRUN_TTL_SECS` (fifteen
  minutes) and are swept at start-up. See [`docs/sources.md`](docs/sources.md).
- **The mapping gates as a config graph** (`GET`/`PUT /api/sources/gates`,
  `urn:config:mapping-gates`): the confidence bands, the datatype-mismatch
  cap, the ambiguity margin, the enumeration match minimum, the dry-run
  classifier's two numbers, the drift threshold and the deterministic lexical
  scorer's weights, with documented defaults until an administrator saves
  a configuration. `PUT` is a partial update that refuses an unknown field
  and any value that cannot be applied. The proposer reads them from here.
- **Drift between two profile versions, and the re-map tickets it opens**
  (`POST /api/sources/{id}/drift`). Compared against the profile version the
  mapping was registered or approved against — recorded as `profileVersion`
  on the mapping — or the previous version: new and removed columns and
  tables, type changes, code lists whose value distribution moved (KL
  divergence above the gates' threshold), code lists gained or lost, the
  structural hash, and a model-version bump when the mapping's model has
  published a newer version. Anything affected opens **one** re-map ticket
  per (datasource, mapping) — a model bump lists every table on it rather
  than opening one per table — and a later check updates that ticket.
  `GET /api/sources/{id}/tickets`, `GET /api/tickets/{id}`,
  `POST /api/tickets/{id}/close`.
- **A one-time converter for the legacy `mapping.sql2rdf.yaml` bundle**
  (`POST /api/mappings/convert`). Entities, typed literals, `lookup`,
  `reference` and `enumeration` objects and `nested` maps become standard RML
  through the same description YARRRML translates into; `{value_slug}` and
  `{column_slug}` placeholders become the new `otsfn:mintIri` function. The
  converter's fixture reproduces the legacy transformer's triples byte for
  byte. Empty cells: this engine emits no term for one, as the legacy
  transformer did, so the default keeps `rr:tableName`; `emptyAsNull` turns
  the logical sources into `NULLIF` queries for a mapping that must behave
  the same under another processor.
- **`otsfn:mintIri`**, the engine's second function: an IRI from a template
  whose placeholders are `{column}` or `{column_slug}` — the value as an ASCII
  slug — on an object map or, new for the engine, on a **subject map**
  (`fnml:functionValue` on `rr:subjectMap`). A function-valued subject is
  never pushed down as a join parent; it resolves through the index.
- **The store's own vocabularies are known to the prefix registry.** `ds:`
  (datasources), `dsprof:` (source profiles), `otsfn:` (mapping functions) and
  `ots:` (the validation layer) are seeded into every registry at construction,
  in the tier a seed bundle's declarations use, so a datasource IRI shortens to
  `ds:SqlSource` and a CURIE typed against one of these labels expands to the
  store's namespace rather than to whatever the community snapshot binds the
  label to. `ds` shadows a defunct DCAT extension on purpose — a platform
  naming its own namespace outranks a community list, as a bundle does. `fn`
  and `prof` were *not* claimed: they are the XPath functions and W3C Profiles
  namespaces everywhere else, which is why the labels are `otsfn:` and
  `dsprof:`. An administrator's override still outranks all of it.
- **SHACL Studio in the OpenAPI document.** The shape-graph family
  (`/api/shacl/shape-graphs…`: content, revisions, restore, clone, import,
  meta-validation, commits and lifecycle), the shapes catalog, in-place
  registration, bindings, a dataset's effective shapes, pipelines and their
  runs, the model context and shape derivation were served without being
  documented. `GET …/turtle` documents `?format=shaclc`; `PUT …/turtle`
  documents `?message=`, the revision note the history shows.
- **`geof:aggUnion`, the GeoSPARQL 1.1 spatial aggregate.** A real SPARQL
  aggregate: `SELECT ?k (geof:aggUnion(?geom) AS ?u) … GROUP BY ?k` folds each
  group's geometries into their union (GEOS unary union), one
  `geo:wktLiteral` — with or without `GROUP BY`, in `HAVING`, in sub-selects
  and in the `WHERE` of an update, over WKT, GML and GeoJSON alike. A group in
  one CRS keeps it; a group mixing CRSs is unioned in CRS84. It follows
  SPARQL's aggregate error rules (a value that is not a geometry makes the
  group's union unbound, as a non-number does to `SUM`), the union of no
  geometries is `GEOMETRYCOLLECTION EMPTY`, and the result does not depend on
  the order the solutions arrive in. `geof:aggUnion(DISTINCT ?g)` does not
  parse — spargebra takes no `DISTINCT` in a custom aggregate's call — and
  would change nothing: a union absorbs duplicates. Every parse of a query
  now goes through one parser that knows the aggregate (`opengraph::sparql_parser`, fed by a
  registry the store fills when it opens): undeclared, `geof:aggUnion(?g)` read
  as a plain function call — a syntax error under `GROUP BY`, and on the
  accelerator's planners a different query, a row-local `BIND` that a
  surrounding `COUNT` could have summed across the subject shards. The shards
  and the columnar copy decline the aggregate; the full in-memory copy and the
  engine evaluate it. The service description advertises it as
  `sd:extensionAggregate`. It was a tracked gap.
- **The GeoSPARQL 1.1 metric functions** — `geof:metricDistance`,
  `metricLength`, `metricPerimeter`, `metricArea` and `metricBuffer` — measure in
  metres (square metres) on the WGS84 ellipsoid whatever CRS the operand is
  written in: it is reprojected to CRS84 first, and a CRS this build cannot
  reproject gives an unbound result, not a number in unknown units. Distances,
  lengths and areas are Karney's geodesic algorithms (through the `geo` crate
  already in the tree — no new dependency), exact to nanometres and convergent
  for nearly antipodal points; the nearest points of two geometries and a
  buffer are found in an ellipsoidal azimuthal equidistant plane around them,
  and a buffer is returned in its operand's CRS. The suite checks them against
  published values: GeographicLib's JFK–LHR (5 551 759.400 m) and
  Wellington–Salamanca (19 959 679.267 m), the equatorial degree and the
  meridian arc. They were tracked gaps.
- **`geo:geoJSONLiteral` geometries (GeoSPARQL 1.1).** A GeoJSON literal — an
  RFC 7946 geometry object, `Point` through `GeometryCollection`, always CRS84
  longitude/latitude — is a geometry wherever a WKT or GML literal is: every
  `geof:` function takes it, binary functions harmonise it with a projected
  operand, `geof:transform` reprojects it, and the spatial index and the map
  viewer feed read `geo:asGeoJSON` next to `geo:asWKT`/`geo:asGML`. It is
  translated to WKT on the way in, so it takes the same GEOS path; a malformed
  one (bad JSON, a `Feature`, a short position, an unclosed ring) is not a
  geometry, and functions over it are unbound rather than a panic. New
  **`geof:asGeoJSON`** serialises any geometry as a `geo:geoJSONLiteral`,
  reprojecting it to CRS84 first. Both were tracked gaps.
- **LLM services in Service health.** The sidebar's Service health popover now
  lists the LLM gateway and each AI feature — Spark chat, NL→SPARQL and the
  SHACL assistant — with the model it sends and a note when the gateway's
  `/v1/models` list does not serve that model, so a mistyped model name shows
  up before the first failed completion. `GET /api/llm/health` gains
  `configured` (is `LLM_GATEWAY_URL` set) and `services` (`id`, `model`,
  `listed`: `true`/`false`, `null` when there is no list to judge by), read
  from the probe it already makes. The popover fetches it only when opened or
  refreshed and shows an absent or unreachable LLM in yellow, since the AI
  features are optional; the health badge and `GET /health` never depend on
  the gateway.
- **An admin page for prefix overrides** (`/admin/prefixes`). Lists what this
  deployment has decided its prefixes mean, adds one, repoints one and removes
  one. While a label is being typed it resolves that label live and says what
  it means today and from which tier — "geo: already resolves to
  http://www.opengis.net/ont/geosparql# (from the bundled snapshot)" — so
  claiming a shorthand is an informed choice rather than a surprise, and a
  refused duplicate shows the namespace from the `409` rather than a bare
  error. Removal asks first, and says the prefix is not deleted: it falls back
  a tier.
- **An administrator can say what a prefix means here** (`/api/admin/prefixes`,
  admin-only). A community list is a good default and a poor authority: `geo`
  means one thing on prefix.cc and another on a deployment that publishes its
  own geo namespace. An override is stored in the identity database — so it
  survives a restart and reaches a follower with the rest of it — and sits in a
  new top tier of the resolver, above the platform overlay, an installed
  bundle's seeds and the bundled snapshot, for lookup, reverse lookup, search,
  listing, CURIE expansion and IRI shrinking alike. **No two overrides share a
  shorthand**: the label is the primary key of the table they live in, so
  `POST` of a label that already has one is refused with `409` and told what it
  currently resolves to. Repointing is a `PUT`, a different request on purpose
  — a prefix changing meaning should be a decision, not a side effect. Deleting
  an override does not delete the prefix; the label falls back to whichever
  lower tier answers first.
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
- **W3C SPARQL 1.1 test suite (query and update sections) in CI.** The query
  and update sections of `w3c/rdf-tests` are vendored unmodified under
  `tests/fixtures/w3c-sparql11/` and run manifest-driven through the store
  by `tests/w3c_sparql11_manifests.rs` as a two-way regression ratchet with a
  pass floor; the known failures (all oxigraph 0.5 evaluator behaviours) are
  tracked in docs/conformance/sparql11.md. No score is published for this
  subset: W3C's test-suite licence policy allows no performance claims on a
  subset of a W3C test suite, so it is used for development and bug tracking
  only.
- **SQL sources: datasources, standard RML mappings and store-native runs.**
  `/api/sources` registers a SQL database as RDF in `urn:system:sources` —
  dialect, location, a read-only account, a mandatory statement timeout and a
  **reference** to the credential in an external secret store. Mappings are
  standard RML stored as one graph per frozen version, whose IRI is the
  version IRI, so a run's `prov:used` names exactly the triples that executed.
  A run materialises into a fresh `urn:run:<id>`, records a PROV activity,
  passes the SHACL write gate on that graph, and only then takes the
  production role — in one update. A failing gate leaves production untouched
  and keeps the candidate for inspection; `POST /api/runs/{id}/rollback`
  re-points the datasource at the graph it served before, without re-running.
  Drivers sit behind a `SourceConnector` trait in `ots-plugin-api`: SQLite is
  in core, other dialects arrive as plugins. See [`docs/sources.md`](docs/sources.md).
- **Secret references.** Every credential the server needs is configured as
  `env:NAME`, `file:/path` or `vault:<mount>/data/<path>#<key>` (HashiCorp KV
  v1 and v2, token from `VAULT_TOKEN_FILE` — the Vault Agent sink — or
  `VAULT_TOKEN`, optional namespace and private CA) and resolved at the moment
  of use. Resolved values are cached briefly, never persisted, never logged,
  and stripped from any error that can reach a caller. OIDC client secrets,
  `LLM_API_KEY`, `SMTP_PASSWORD`, `ALERT_SMTP_PASS` and `S3_SECRET_KEY` all
  read references now; plaintext still works outside the production posture
  with a deprecation warning.
- **Production posture.** `OTS_ENV=production` turns the security rules into
  startup and registration errors rather than warnings: a raw secret, an
  unresolvable reference, a missing statement timeout, a datasource host
  outside `OTS_REMOTE_ALLOWLIST` and a file-backed datasource outside
  `OTS_SOURCES_DIR` are all refused.
- **RML: relational logical sources.** `rr:tableName` / `rml:query` over a
  registered datasource, streamed in batches; R2RML natural datatypes for bare
  column references; and an RML-FNML function for enumerations with an explicit
  policy for values the map does not cover (keep as a literal so SHACL flags
  it, omit, or mint from an absolute template).
- **Join planning.** `rr:parentTriplesMap` is pushed into the child's query
  when the catalogue proves the parent's join columns cover a unique key — so
  the join cannot duplicate a child row — and falls back to a bounded hash
  index otherwise. Pushing down on a non-unique key would multiply the child
  row and re-emit its own triples once per match, so the planner declines
  unless it can prove otherwise.
- **YARRRML authoring.** A mapping may be submitted as `yarrrml` instead of
  `rml` and is translated on the way in; only RML is stored, so there is one
  representation to version, diff, gate and execute. Includes a code-list
  extension for SQL enumerations. Constructs outside the translated subset are
  errors naming the construct, not silent omissions.
- **Incremental runs.** `mode: "watermark"` copies the graph in production,
  re-maps only the rows past the recorded cursor, and replaces those entities
  wholesale — so the candidate is still a complete graph and the SHACL gate,
  the atomic swap and rollback all behave as they do for a full run. The cursor
  lives in the run log, so a rollback cannot silently strip rows it has passed.
- **LDES members from runs.** A run whose dataset has a stream enabled now
  publishes the entities it wrote, after the swap. A full run publishes every
  entity, an incremental one only what moved. Previously a materialisation run
  published nothing at all.
- **Source profiling.** `POST /api/sources/{id}/profile` writes a versioned
  profile graph per datasource — per-column distinct and NULL counts,
  cardinality, length and numeric summaries, a sampled lexical-shape detection
  with its confidence, and a structural hash per table that moves when the
  schema does and not when a row is inserted. Aggregated in SQL, never by
  streaming a table into the server. Values appear only as the top-k of a
  genuinely low-cardinality column, and a column whose values are too long to
  be codes yields none at all rather than a truncated list. Reusing csvw: for
  structure and void: for counts, with a small `dsprof:` namespace for the
  statistics.
- **Ontology profile.** `GET /api/models/{id}/versions/{v}/profile` returns a
  model version flattened for a mapping proposer: classes with full superclass
  chains, properties with domain/range/datatype, every SHACL property shape
  flattened past `sh:node`, and enumerations from `owl:oneOf`, SKOS schemes and
  `sh:in`. A fixed number of queries whatever the size of the ontology, with
  byte-identical output for unchanged data. A bound shape graph is included
  only when the caller may read that shape set.
- **SHACL Studio: prefixes, links and the editor.** Shape-graph Turtle is
  served with an `@prefix` header resolved from the instance's prefix registry,
  so IRIs read as CURIEs instead of full `<http://…>` in both the source view
  and the visual builder; only namespaces the graph actually uses are declared.
  The Studio pages now shorten IRIs through the shared helper (with the full
  IRI on hover) instead of four private last-segment truncators, and resolve
  dataset names instead of showing raw ids. A dataset's "effective shapes" link
  pointed at a route that does not exist and went to a blank page. The editor
  gains line wrapping, a Turtle/SHACL completer, parse errors as positioned
  diagnostics, Cmd-S, an unsaved-changes guard, and a per-save revision message
  (every revision previously read "Edited").
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
- **Turtle served to people carries a prefix header.** Graph Store reads
  (`GET /store?graph=`, Turtle and TriG, streamed and label-filtered alike), a
  dataset's shapes graph, its RML mapping, the RML preview and execute
  responses and a datasource's profile now declare an `@prefix` line for each
  namespace the graph actually uses, resolved through the prefix registry,
  the way SHACL Studio's shape-graph content already did. The line-based
  formats are unchanged. A profile used to carry a header over a body of full
  IRIs — a dead header — and now reads as CURIEs.
- **The mapping-function label is `otsfn:`.** The RML-FNML enumeration
  function's namespace is unchanged (`https://w3id.org/open-triplestore/fn#`);
  the label the documentation, the YARRRML translator and the error messages
  use is `otsfn:` rather than `fn:`, which is the XPath functions namespace in
  every prefix list. A mapping may still declare any label it likes for the
  namespace.
- **A run's SHACL gate report carries the run metrics** (duration, quads read,
  source) the validation report gained, folded across the shapes graphs the
  gate evaluates the way the validate route folds them.
- **`geof:distance` and `geof:buffer` honour a metre unit on a geographic
  CRS.** With `uom:metre` (or `kilometre`, `centimetre`, `millimetre`) on a CRS84
  or EPSG:4326 operand, `geof:distance` returned the planar distance in
  *degrees* and `geof:buffer` buffered by that many degrees — the unit was
  ignored. Both are geodesic on the WGS84 ellipsoid now, the same as
  `geof:metricDistance` / `geof:metricBuffer`: London–Paris with `uom:metre` is
  343 923.120, not 3.63, and a `uom:metre` radius of 1000 is a kilometre, not a
  thousand degrees. On a projected CRS (RD New, Web Mercator) both stay planar in
  the CRS's own metres, as before, but `geof:buffer` now converts a kilometre
  (centimetre, millimetre) radius, which it used to take as metres. An angular
  unit on a geographic CRS keeps the planar degree distance, now converted for
  `uom:radian` (it was returned in degrees); without a unit nothing changes.
  **This changes results** for every query, shape or saved query that passes
  `uom:metre` with CRS84 or EPSG:4326 geometry — `FILTER(geof:distance(?a, ?b,
  uom:metre) < 25)` over lon/lat data used to compare degrees with 25 and so
  held for almost everything, and now compares metres. The bundled "Distance
  from a city" saved query, labelled metres, now returns metres.
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
- **The performance gate gates every group.** The insert, update, SHACL and
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
- **Bundled vocabularies are complete upstream copies.** `sosa` and `ssn`
  (the W3C SDW source), `saref` (ETSI SAREF core 3.1.1), `bot` (W3C LBD CG
  0.3.2), `omg` (0.3, was a 0.0.1 excerpt) and `fog` (0.0.4, was 0.0.1) are
  complete copies from their publishers, unchanged below an added comment
  header. They used to be hand-authored
  excerpts whose class and property IRIs were modelled plausibly against the
  namespace rather than copied from the source, and nothing in the UI or the
  registry told them apart from real terms. The files with no authoritative
  source are gone — see Removed. (`f20a87b`)
- **Dependencies.** Three more batches supersede the open Dependabot PRs
  (#326, the advisory bumps in #321, and #357), with the code migrations they
  require: `jsonwebtoken` 11 (now on its pure-Rust `rust_crypto` backend; v11
  ships **no** signing backend by default — the OIDC provider's stored ES256
  key still parses), `quick-xml` 0.42 (names and attribute values moved from
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
  stale `deny.toml` ignores were dropped. TypeScript 7 remains held:
  typescript-eslint has no release that supports it. The September batch
  (#357) brings `argon2` 0.6 (on `password-hash` 0.6 the hasher draws its own
  16-byte salt; password hashes stored under 0.5 still verify — pinned by a
  regression test on real 0.5-minted PHC strings — and new hashes keep the
  same `$argon2id$v=19$m=19456,t=2,p=1$` form, so a rollback reads them too),
  `vitest` 5 (no config or test changes needed), `oxigraph` 0.5.11 with its
  `oxrdf` / `spargebra` / `spareval` / `sparopt` / `sparesults` / `oxttl`
  siblings in lockstep, `aes-gcm` 0.11.1 (stored OAuth client-secret blobs
  still decrypt, pinned by a known-answer test), `tower-http` 0.7.1, `flate2`
  1.1.10, `lru` 0.18.4, `aws-sdk-s3` 1.142, `svelte` 5.57, `vite` 8.2.2, `n3`
  2.6, `marked`, `devalue` past GHSA-9rgm-9g3h-6x36, `uuid`,
  `aws-smithy-http-client`, `proj4`, `dompurify`, `@codemirror/search`,
  `@codemirror/state`, `@typescript-eslint/parser`, and the SHA-pinned
  `softprops/action-gh-release` 3.0.3; `docker/build-push-action` 7.4.0
  followed (#368). `oxiri` 0.3 is held: its `Iri` type crosses the
  federation service-handler API, so it has to stay on the 0.2 line Oxigraph
  still uses.

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
- **The nightly ignored-tests job.** It ran `#[ignore]`d tests so the set
  stayed visible; with none left, the gate above replaces it.
- **The Triple Browser's Simple/Advanced switch.** Everything it gated is
  simply available, and in its place is one SPARQL button that opens the
  query behind the current view. The natural-language panel now appears on
  its own real condition — whether a gateway is configured — rather than on a
  mode. The five dead translation keys are out of both dictionaries, and
  docs/search-syntax.md, which is compiled into the binary and linked from
  the browser's own help popover, no longer tells the reader to flip a
  control that is not there.
- **The IMBOR `def/` excerpt is deprecated on existing installs.** 0.6.0
  stopped seeding it; an install that still holds it keeps it, deprecated and
  served to no one who may not write the `imbor` entry, because it is not
  CROW's content.

### Fixed
- **Street maps no longer show "API KEY REQUIRED" tiles.** CARTO watermarks
  every keyless basemap tile since September 2026, which covered the 3D globe
  and the map previews. Those maps now draw OpenFreeMap's vector tiles into
  raster tiles in the browser, in the app's light or dark theme. No key is
  needed, and every map carries the credit "OpenFreeMap © OpenMapTiles Data
  from OpenStreetMap". Satellite imagery (Esri World Imagery) is offered only
  when the deployment sets its own ArcGIS key in `/config.json`
  (`basemaps.esriApiKey`), because Esri's terms tie the imagery to one.
  Without a key, the 2D map, the globe and embeds show streets and no
  satellite toggle. `?basemap=satellite` on an embed falls back to streets.
- **`public = false` in a seed-bundle manifest reaches entries an earlier
  build registered public** (NEN 2660-2 and the NEN relation profile, whose
  models the example bundles now keep private because NEN grants no licence to
  redistribute them). At the next start an entry the bundle provably created
  is made private once; only its visibility changes, and an admin who makes it
  public again is not overruled.
- **IMBOR is shipped unmodified.** `frontend/public/vocab/imbor.ttl` had an
  added comment header and one altered definition while being described as
  verbatim; CROW's management plan names CC BY-ND 4.0, which allows no altered
  copies. The file is now byte-identical to CROW's release, with its
  provenance and licence in the notices instead.
- **Existing installs get CROW's IMBOR release back, and nothing stored is
  lost.** On every start the seeder checks the copies it created itself: each
  one carries a registry marker, and a record from an earlier release counts
  only when its creator, id, version, graph, notes and creation date all read
  as that seeder wrote them. A copy whose licence allows other copies and that
  differs from the bundled file (an admin's edit, an earlier file) is never
  modified, only labelled as possibly modified. The one exception is a copy
  whose only difference is the `owl:versionInfo` triple earlier loaders
  added: that triple is removed, and the copy is the file again. IMBOR is checked on every start, also with
  `SEED_STANDARD_VOCABS=false`: a copy that differs, such as the altered source
  note installs seeded by 0.5.0 and 0.6.0 hold, is first kept as a private,
  deprecated version `2025-kept-<n>`, served only to the entry's writers, and
  only then is version `2025` restored from a staging graph in one
  transaction. The hand-authored IMBOR "excerpt" is kept, deprecated and
  withheld. Models the seeder did not create (made through the API, promoted
  from a dataset, or registered by a bundle or a LOV install, even under an id
  like `imbor`) are never touched. Replicas and non-leading cluster members
  leave all of this to the leader. Merge and rebase no longer get around the
  IMBOR refusal or drop the licence record, and a merge of a version into
  itself is refused (400).
- **Accurate licence statements.** GWSW's ontology was described as CC0 (the
  CC0 covers RIONED's server data, not the ontology); several vocabularies
  were credited to the wrong holder or licence; test-suite results were
  presented as official conformance; a Uniclass code in an IFC test fixture
  was paired with a title that is not its own (the fixture now uses a
  made-up classification); a NEN 2660-2 definition was quoted in the
  relations profile (now in the project's own words).
- **The Turtle and SPARQL prefix exports parse.** One prefix.cc namespace is
  not an IRI (it carries two `#`) and made the whole export unparseable; the
  loader and the snapshot build leave it out.
- **The main map's data credit shows on first load.** The map read its
  attribution once, before the dataset feed had arrived, so the 3DBAG credit
  never appeared; it now follows the feed.
- **The Docker image builds again.** The workspace gained `tools/*` (the
  writeback worker) but the image's planner and builder stages never copied
  `tools/`, so cargo could not load the workspace.
- **Claims match the results.** SHACL Core and GeoSPARQL are graded Partial
  where the suites show gaps, GeoSPARQL is described as implemented in part
  and not OGC-certified, and the comparison matrix's own cells follow the
  same grades.
- **Map credits.** The Leaflet streets basemap credits "OpenStreetMap
  contributors" with a link to the copyright page, as OSM asks.
- **Frontend typecheck.** A test read CodeMirror's internal `streamParser`,
  which `tsc` rejects; the Turtle tokenizer is exported as
  `turtleStreamParser` and the language is built from it.
- **The IMBOR bundle's sample conforms to the real Kern.** Every IMBOR
  beheerobject inherits NEN 3610 `identificatie` and `domein`, a `geometrie`
  and a Geo-object `status` from its superclasses; the three sample trees
  carried none of them, so validation reported every tree, not only the
  planted violation. The sample now carries them and the test asserts that
  the planted `kiemjaar` datatype violation is the only result.
- **Two persistence tests ran nowhere on Apple silicon.** Their macOS arm64
  `ignore` cited a RocksDB `TryFromIntError` that no longer occurs; they run
  everywhere again.
- **MySQL, MariaDB and SQL Server values come out in one spelling.**
  Fractional seconds lose their trailing zeros (MySQL pads `DATETIME(6)` to
  `…12:00:00.000000`) and doubles take their shortest round-trip form (SQL
  Server's lossless style 3 printed `2.0000000000000000e+000`), in the shared
  canonicaliser, so every driver writes the same lexical form for the same
  value. On MariaDB a column default is reported the way MySQL reports it —
  `'x'` unquoted, `DEFAULT NULL` as no default rather than the string
  `NULL` — and the server version names MariaDB. The first runs against real
  servers also corrected the live tests themselves: a reserved column name,
  timeout probes that MySQL answers without an error and SQL Server's
  optimiser answers instantly, and the `VIEW DEFINITION` a SQL Server reader
  needs to see column defaults.
- **The Turtle editor colours an escaped local name as one name.**
  `ex:shapes\/PersonShape` — what the prefixed serializer writes for a
  path-style IRI — was tokenised as a name, an operator and a stray word. The
  tokenizer now reads Turtle's `PN_LOCAL`, backslash escapes and
  percent-encoding included.
- **The shapes catalog's prompts and notices are translated.** The name
  prompts when composing or registering a shape graph, and the notices that
  followed, were English whatever the interface language.
- **Release tags keep their section headers.** `auto-tag.yml` created the
  annotated tag with git's default message cleanup, which deletes every line
  that starts with `#`: the v0.5.0 tag lost all of its `### Added` …
  `### Security` headers. The workflow and the manual command in
  `docs/release-process.md` now pass `--cleanup=whitespace`.
- **`geof:getSRID` of a GML literal returned its opening tag as a CRS IRI**
  (`<gml:Point srsName=…>` read as a `<crs>` prefix — an invalid IRI in the
  results). It returns CRS84 now, the CRS the other functions already treat a
  GML literal as being in (`srsName` is not read yet); a GeoJSON literal is
  CRS84 by definition.
- **A blank `LLM_GATEWAY_URL` falls back to the built-in default.** It was used
  as given, so `LLM_GATEWAY_URL=` (as an `--env-file` line with no value
  produces) sent every probe and completion to a relative URL that could never
  answer, instead of to `http://127.0.0.1:8000`; a value with surrounding
  whitespace was not trimmed either. It is now read like the other `LLM_*`
  settings: trimmed, and unset or blank means the default.
- **Deactivating a SPARQL service did not stop it answering.** `PUT
  /api/datasets/:dataset_id/services/:service_id` with `"is_active": false`
  stored the flag, and the dataset page greyed the service out and stopped
  showing its endpoint URL, but the query route
  (`/api/datasets/:dataset_id/services/:service_slug/sparql`) never looked at
  it. A public dataset's service switched off by its owner kept answering
  anyone who had the URL. An inactive service now answers `404 Service not
  found`, the same body as a service that does not exist, on `GET` and both
  `POST` forms, with or without `?version=`. The same answer goes to every
  caller: the dataset's owner, its writers and a super admin get it too,
  because "inactive" is a property of the endpoint, not of who is asking. They
  can reactivate the service or query the dataset through `/sparql`. This did
  not widen what anyone could read: the route still required dataset access
  and still filtered private graphs, so the flag was a switch that did not
  work, not a read boundary that leaked. The SPARQL editor no longer offers
  inactive services as endpoints, or routes a version-pinned query through
  one.
- **A Raft member's vote survives a restart.** The vote was kept in memory with
  the log, so a member that restarted could vote a second time in the same
  term. The consequences were bounded and documented — the election timeout
  (1.5–3 s) usually outlasts a restart, and because the state machine holds no
  data and the data path fences by epoch, a double vote could at worst elect a
  second leader for one term, which resynchronises rather than diverges — but
  bounded is not absent, and remembering one `{term, node}` pair is cheap. It
  lives in `{data_dir}/raft-vote.json`, written and renamed so a crash cannot
  leave half of one, rewritten only on a vote and so never on a hot path. An
  in-memory store has no file and behaves as before; an unreadable file is
  logged and ignored rather than refused, because starting without it is
  exactly where this began.
- **Three controls rendered the name of their translation key.** `common.loading`
  and `common.delete` were asked for in the admin security page and the OAuth
  consent screen, and there is no `common` namespace in either dictionary —
  svelte-i18n renders a missing key as the key, so those spinners and that
  button read literally `common.loading` and `common.delete` on screen, in both
  languages, without anything failing. They point at `system.*`, which exists.
  A test now scans the source for literal `$t('…')` references and requires
  each to resolve in English and in Dutch, so the next one fails the build
  instead of shipping.
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
- **Full-text index rebuilds no longer stall the server.** Rebuilds run on
  the blocking pool (one query stalled the whole runtime for 1.19 s on a
  20k-literal store), and `POST /api/text-search/reindex` runs under the same
  lock as the background sync, so the two cannot race. A `text:search` from
  Spark and the same query opened in the SPARQL workspace behave identically.
  (#347)

- **Spark checks model-written SPARQL against the store.** Trailing prose
  after an unfenced `SPARQL:` directive is cut at the parser's reported error
  position when the prefix alone parses (a model that explained itself after
  its query used to fail every counting question this way), and every IRI
  outside the vocabulary sample is checked against the store itself — four
  indexed probes: subject, predicate, object, named graph — so an invented
  one fails fast with the offending IRIs named, while a real term outside the
  sample runs. IRIs the user pasted are never rejected: an absent one runs to
  an honest empty result instead of an error blaming the user. (#322)

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
- **The registry card's delete button no longer covers the expand chevron.**
  The admin-only trash icon was absolutely positioned in the same corner as
  the card's open affordance, so on hover the two targets were
  indistinguishable and mis-clickable; it sits in the header row before the
  chevron. (#298)
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
- **A graph-ACL read grant let a user write the graph through the SHACL
  Studio.** `POST /api/shacl/register-shape-graph` checked only read access,
  then made the caller the owner of a Library entry over the graph, and the
  Studio's save, restore and import checked only who owns the entry. Reading
  a graph was enough to overwrite it, or to make it world-readable by setting
  the entry public. Now the right to write follows the graph. Registering
  needs it, and every Studio write checks it again: save, restore, import
  shapes, and a visibility change of an entry whose graph the Studio did not
  mint. The right is held by an admin, by whoever manages a graph the Studio
  minted (`urn:shapes:…`), by whoever may write a dataset that holds the
  graph (its namespace, or registered to it), by whoever may write the
  registry entry holding a model graph, or through a graph-ACL write grant.
  Entries made before this change give their owners no more than that. Org
  members still edit their dataset's shapes graph in the Studio without a
  grant. An org viewer, who may not write the dataset, no longer can. In the
  same pass:
  - `PATCH /api/datasets/{id}/graphs` with role `shapes` answers 404 for a
    graph not registered to the dataset. Before, it adopted any graph holding
    a shape into the Library, owned by the dataset's owner.
  - Registering a graph named `urn:shapes:…` is for admins only.
  - Asking to register a graph that already has an entry returns that entry
    only to a caller who may see it.
- **A dataset could take over, then delete, graphs it did not make.**
  `POST /api/datasets/{id}/graphs` let a non-admin attach any graph that no
  other dataset had registered: a graph an admin loaded over the Graph Store,
  another user's SHACL Studio shape graph, a source run, or another dataset's
  entailment, property-states or assets graph. Attaching one made it
  writable through the dataset (bulk import, RML, RDF Patch, validate-and-
  commit, LDES) and deleted it on detach. The same held for a shapes graph set
  with `PUT /shacl`, which `PUT /shapes` then overwrote. Now:
  - A graph that already holds data, or that the graph ACL grants to
    someone, is attached only by a caller who may write it directly: an
    admin, or a graph-ACL write grant.
  - The graphs the server names for its own features are never attached:
    `urn:shapes:`, `urn:source:`, `urn:mapping:`, `urn:run:`, `urn:dryrun:`,
    `urn:ots:`, `urn:config:` (the mapping gates), `urn:entailment:`, and
    other datasets' `{base}/datasets/{id}/…`.
  - `dataset_graphs` records whether the dataset created each graph it adds.
  - A detach, or a dataset or organisation delete, deletes a graph only if it
    is the dataset's own (its namespace, or a graph it created) or the caller
    may delete it directly. Anything else loses only its registration. Older
    registrations outside the namespace count as not created.
  - Linking a shapes graph is a read: the caller must be able to read the
    graph, and another dataset's shapes graph (its default one included) may
    be shared by those who can read that dataset; an empty graph another
    dataset links is linkable once its shapes are written. `PUT /shapes`
    writes a linked graph only if it is new or the caller may write it; that
    write registers the graph to the dataset for its editors (an admin's
    write only when a non-admin could have claimed the graph as new). A
    dataset delete does not delete a graph it only links, except one left in
    a deleted dataset's namespace, as its last user. Resending an unchanged
    link (toggling `shacl_on_write`) is not checked again. A linked shapes graph outside the
    namespace that was filled before this release is not registered, so
    non-admin editors can no longer write it; an admin can attach it to the
    dataset to hand it back.
  - An RML run registers every `rml:graphMap` destination it creates, not
    only `?graph=`, so the mapping runs again (an admin's run of a stored
    mapping only those a non-admin could have claimed). A `rml:graphMap`
    output written before this release is not registered, so a non-admin's
    re-run is refused until an admin attaches it to the dataset.
  - A version restore goes through the same gate as a new target graph. A
    graph the dataset has since let go is skipped and listed under
    `skipped`; the rest is restored.
  - IFC and CityJSON imports compared their target with the dataset's IRI
    without a trailing slash (dataset `bridge` could write
    `{base}/dataset/bridge-inventory/…`). They now use the dataset boundary
    and refuse model-registry graphs. The bulk IFC import's derived
    `{target}/ifcowl` graph is now checked too; it was written unchecked.
- **Old registrations of model-registry graphs are released at startup.**
  `dataset_graphs` rows that named a registry graph before registration
  refused them still made that graph dataset-scoped for reads, and hid it
  from everyone else. A one-time sweep at the leader's boot removes them (on a
  Raft cluster, once a member leads). A shapes-role row released this way
  stays bound to its dataset in the SHACL Studio, so validation keeps reading
  it. A dataset's shapes graph setting naming a registry graph is kept: it
  scopes no reads, and every path that could write or delete it refuses
  registry graphs. The boot adoption of legacy shapes graph settings into the
  Library no longer adopts registry, system or `urn:shapes:` graphs.
- **A SHACL-AF rule was a SPARQL UPDATE with the store's own authority.** The
  `sh:construct` body of a `sh:SPARQLRule` was rewritten textually and handed
  to `TripleStore::update`, which authorizes nothing and confines nothing — so
  a shapes graph, which any writer of a dataset may upload
  (`PUT /api/datasets/{id}/shapes`) and then run
  (`POST /api/datasets/{id}/infer`, or a SHACL Studio pipeline), could read and
  write every graph in the store: another tenant's private data, `urn:system:*`,
  the model registry. `DROP ALL` was a rule; with one data graph the `WITH <g>`
  prefix the engine added confined nothing, since
  `WITH <g> INSERT { GRAPH <any> { … } } WHERE { GRAPH <any> { … } }` is valid
  SPARQL. A `sh:TripleRule` was reachable the same way, because the focus node
  was pasted into the generated update as `<{focus}>` and a focus node may be a
  literal (`sh:targetNode "…"`) whose lexical form the shapes author writes.
  Now: `sh:construct` must parse as the CONSTRUCT query SHACL-AF says it is
  (the `INSERT { … } WHERE { … }` convenience form still parses as one;
  anything else is refused by name, at load time), it is evaluated **read-only**
  over the run's data graphs with its own `FROM`/`FROM NAMED` clauses replaced,
  `$this` is bound as a term instead of being pasted in, and the engine — not
  the rule — inserts the derived triples, into one graph the dataset holds.
  `POST /api/datasets/{id}/infer` names that graph: the single data graph as
  before, or, over several, the dataset's own `urn:dataset:{id}:inferred`
  (registered with the `entailment` role) instead of the store's global default
  graph, where derived triples were both unowned and unreadable.
- **A `sh:sparql` constraint or `sh:SPARQLTarget` could read graphs the run may
  not.** Their queries were scoped by prepending `FROM` clauses, which a
  `FROM NAMED <someone-elses-graph>` written into the shape simply added to.
  The dataset of a shape's query is now replaced outright with the run's data
  graphs, and no named graph is available, so a `GRAPH` block inside one
  matches nothing.
- **The SHACL Studio shapes catalogue listed shapes from graphs the caller
  could not read.** `GET /api/shacl/shapes` hid only Library entries the
  caller could not see. Any other graph holding shapes (a private dataset's
  graph with embedded shapes, a graph an admin loaded) was listed to every
  signed-in user, and `?graph=` returned its shape IRIs, labels, target
  classes and paths. A graph not in the Library is now listed, and drilled
  into, only for a caller who may read it by the rule `/sparql` applies
  (dataset visibility, a private graph only for its dataset's writers, plus
  graph-ACL read grants; admins read every graph); any other graph answers
  403. Library entries keep the Library's own rule.
- **A validation pipeline could read another tenant's data.** A pipeline's
  graphs, datasets and shape graphs were never checked for read access, and a
  run returns its report (focus nodes and values) to the caller. So any user
  could validate a private graph with shapes of their own and read its values
  back, and anyone who could see a shared pipeline could run it, or open a
  stored run, over data they could not read. Creating, updating, running and
  test-running a pipeline, and opening a run's report, now need read access to
  every dataset, data graph and shape graph in its scope (403 otherwise). The
  check runs each time, so a grant revoked since, or a pipeline stored before
  this release, gives no more than the caller may read. A scheduled run is
  checked against the pipeline's creator and skipped when they may no longer
  read its scope. Run summaries (counts only) stay listed to everyone who can
  see the pipeline.
- **A database error lifted every SHACL write gate it touched.** Finding a
  write's gates read a failed lookup as "nothing found": an error listing the
  gating pipelines dropped every `gate_writes` pipeline, an error finding the
  graph's dataset dropped the dataset-scoped pipelines, the dataset's
  validation-layer bindings and its `shacl_on_write` gate, and a failed
  binding query dropped the bindings. The write then landed unvalidated, on
  the Graph Store path and on bulk import alike. A failed lookup now refuses
  the write with the existing 422 and a `gate-evaluation-failure` report, as
  any other gate the server cannot evaluate does; a bulk import is refused
  before anything is written. The Graph Store path's `shacl_on_write` gate
  refuses the same way when its own dataset lookup fails.
- **A pipeline's persisted report was readable by everyone who could read its
  dataset.** A run that persists its report as RDF (`results_target`), or its
  inferred triples in a new graph, attached that graph to the dataset holding
  its data, non-private, even when the run had validated one of the
  dataset's private graphs or a graph that belongs to no dataset. So a
  public dataset's viewers read the private graph's focus nodes and values
  over `/sparql`. A derived graph is now attached to a dataset only when that
  dataset holds every graph the run validated, and is private there when any
  of them is. A pipeline's own report or inferred graph collects every run,
  so before a run over other data it is detached, and it is newly attached
  only while empty. A graph the pipeline's owner named as the target keeps
  its registrations; it is attached only when nothing in the scope is private.
- **Validating a dataset showed its private graphs to anyone who could view
  it.** `POST /api/datasets/{id}/validate` checked only that the caller could
  see the dataset, then validated every graph in it, private ones included,
  and returned the report (focus nodes and values). An official run was also
  recorded, overwriting the dataset's validation status, and its report was
  written as RDF to `urn:system:reports:dataset:{id}`, attached to the dataset
  non-private. So a public dataset's viewers read its private graphs' values
  in the response, over `/sparql`, and from `…/validation/latest` and
  `…/validation/runs/{run_id}`. Now:
  - A run validates only the dataset graphs the caller may read by the rule
    `/sparql` applies (a private graph only for the dataset's writers, plus
    graph-ACL read grants; admins read every graph). A private shapes-role
    graph shapes no run of a caller who may not read it.
  - A run that could not read every graph of the dataset is not official: it
    answers as a test run (`test: true`, `partial: true`), records nothing and
    leaves the dataset's validation status as it was.
  - Recording an official run requires write access to the dataset. A reader
    whose run *was* complete (nothing hidden) would otherwise overwrite the
    dataset's status and history or forge a verdict, so a non-writer's non-test
    request is now refused (`403` — retry with `?test=true`) rather than
    recorded. The self-heal that adopts and binds a dataset's shapes graph into
    the Studio Library likewise runs only for a writer, never under a reader's
    authority.
  - The report graph is private in the dataset whenever the run validated a
    private graph, or a model graph not everyone may read, and it is never
    made public again.
  - A stored run records the graphs it validated. Its full report goes to the
    dataset's writers and to callers who may read each of those graphs when
    they ask; anyone else gets the run's summary with `report: null` and
    `report_withheld: true`. A run stored before this release goes in full to
    the dataset's writers only.
  - An explicit `shapes_graph` must be readable by the same rule. It used to
    need a graph-ACL grant, so a graph of a dataset the caller may read now
    works too.
- **A dataset's private shapes graph reached everyone who could view the
  dataset.** A graph marked private in a dataset is its writers' to read, but
  four paths served a private shapes graph to the dataset's viewers:
  - `GET /api/datasets/{id}/shapes` returned it.
  - The SHACL Studio Library adopts a dataset's shapes graph in place when the
    dataset is validated or imported into, or when its shapes graph or graph
    roles change, and gave the entry the dataset's visibility. A public
    dataset's private shapes graph became a public Library entry: its Turtle,
    its revisions (revision 1 is a copy), a clone, the Library list, bindings,
    effective shapes, the catalogue and pipelines were open to every signed-in
    user.
  - That adoption also bound the graph to the dataset, and the form manifest
    (`GET /api/datasets/{id}/form-manifest`, anonymous for a public dataset)
    carries the Turtle of every bound shapes graph, so anonymous callers got it.
  - `PUT /api/datasets/{id}/shacl` let anyone who could see a dataset link its
    shapes graph as the shapes graph of a dataset of their own, then read it
    from there.

  Now a graph some dataset holds as private is read only by those who may read
  it by the rule `/sparql` applies (its dataset's writers, graph-ACL read
  grants, admins), whatever names it:
  - `GET …/shapes`, validation runs and the form manifest leave out a private
    shapes graph the caller may not read, whether it is this dataset's or
    another's linked or bound here. `GET …/shapes` answers 404 when nothing is
    left, the manifest no longer lists private data graphs to them either, and
    a validation run that leaves out a shapes graph is a test run.
  - A private graph is adopted as a `private` Library entry. Every Library path
    that reads an entry (the entry, its Turtle, revisions, clone, the list,
    bindings, effective shapes, the catalogue, pipelines, re-registration)
    withholds an entry of a private graph from those who may not read the
    graph, whatever the entry's visibility. That covers entries adopted before
    this release and graphs marked private after adoption, with no migration.
    Marking the graph public again gives the entry back.
  - Linking a private graph the caller may not read as a shapes graph is
    refused (403).
  - A validation report names its shapes, their paths and messages, so it
    follows the same rule:
    - A write a gate refuses (Graph Store `PUT`/`POST`, validate-and-commit,
      bulk import; by a binding, a gating pipeline or `shacl_on_write`)
      answers a writer who may not read one of that gate's private shapes
      graphs with only that the write does not conform, and by how many
      results. The write is refused all the same.
    - A stored run records the shapes graphs it used. Its full report is
      withheld from anyone but an admin who may not read one of them that is
      private when they ask, the dataset's writers included: a dataset's
      writer may link its private shapes graph into another dataset, whose
      writers need not be allowed to read it.
    - An official run shaped by another dataset's private graph writes no
      report graph, and clears the last one.
    - A pipeline with such a graph bound to a dataset or graph in its scope
      is refused (403) to whoever may not read it.
    - The model profile (`GET /api/models/{id}/versions/{ver}/profile`, which
      any user may read with a `sources:read` token they mint) and the SQL
      source dry run leave out a private shapes graph the caller may not
      read, whether it is bound to the model or named in the request or the
      mapping.
  - Making a graph private (`PATCH /api/datasets/{id}/graphs`) takes the
    validation reports on it along. A dataset whose latest official run
    validated the graph, or was shaped by it, has its report graph made
    private when it holds the graph and cleared when it does not. A data
    graph made private after a run used to leave that run's report graph
    readable to the dataset's viewers.
- **Inference ran a private shapes graph's rules for a writer who may not
  read it.** `POST /api/datasets/{id}/infer` runs the SHACL-AF rules of every
  shapes graph of the dataset and writes what they derive into the dataset,
  where its writers and readers read the rules' constants and structure back.
  A writer of two datasets may link one's private shapes graph into the other,
  whose other writers need not be allowed to read it, and they could run its
  rules. Now a run leaves out a private shapes graph the caller may not read,
  by the rule validation applies. It answers 400 when no shapes graph is left,
  and says `partial: true` when it left one out.
- **A SQL-source dry run was shaped by any graph its caller named.**
  `POST /api/sources/{id}/dry-run` validates a sample of a mapping and returns
  the report: the shapes' IRIs, paths and messages. Any user may mint the
  `mappings:propose` token it accepts, and it validated against whatever
  shapes graph the request or the mapping named (a proposer writes mappings
  too): a graph of a private dataset, a graph registered to no dataset, or a
  private model's shapes named through `model` + `modelVersion`. Now a named
  shapes graph applies only when the caller may read it by the rule `/sparql`
  applies, or through the endpoint that already serves it to them (a SHACL
  Studio Library entry they are shown, a graph of a model version they may
  read). A model's shapes apply only when the caller may read the model. For
  anyone else they are left out with a warning. Admins read every graph.
- **Detaching or deleting a dataset could wipe graphs it never owned.**
  `DELETE /api/datasets/{id}/graphs` deleted any graph no other dataset
  claimed, so any user who could create a dataset could wipe the model
  registry, IMBOR, the copies the seeder keeps aside, or another user's model
  with one request. It now deletes the stored graph only when the dataset had
  it registered (otherwise 404, nothing changes), and never a registry or
  system graph. Deleting a dataset or an organisation applies the same rule to
  every registered graph and to the shapes graph, keeps any graph another
  dataset still uses, and fails closed where it used to fail open.
- **Model-registry graphs cannot be attached to or written by a dataset.**
  Registering one to a dataset, setting it as a shapes graph, or targeting it
  with a dataset's RML mapping, validate-and-commit, a bulk import or an LDES
  sync answers 403, for admins too. Registry graphs are the registry graph,
  anything under `{base}/data-model/`, and any graph a model version names.
- **SHACL Studio could alter and serve a no-derivatives model graph.** A seed
  bundle binds a model's graph as a Studio shape graph in place (the
  nen2660-imbor bundle binds CROW's IMBOR Kern). Studio save, restore and
  import into it, a clone of it, an import of its shapes, and a pipeline's
  in-place inference or report into it now answer 403 for a version whose
  licence allows no altered copies, admins included; a Studio write into any
  other attributed version marks its licence record first. The Studio serves
  such a graph to everyone only while it is the checked, unchanged copy, and
  deleting a Library entry clears only a graph the Studio created.
- **The re-check after an admin update that writes unnamed graphs could be
  skipped.** It now runs in the write's own task right after the write (so a
  timeout or a dropped client cannot skip it), and again at the leader's boot
  (so a crash cannot); it runs only for writes to graphs that cannot be named
  in advance.
- **Public term search served text from no-derivatives content that
  downloads withhold.** Vocabulary search and autocomplete index such an entry
  only while its latest published version is a checked, unchanged copy.
- **Direct writes cannot alter content whose licence allows no altered
  copies.** SPARQL Update, `/sparql/batch`, Graph Store PUT/POST/DELETE, and
  reasoning and SWRL targets aimed at the graph of a model version whose
  licence record allows no altered copies (IMBOR, no-derivatives LOV installs,
  bundle models declared so) answer 403, for admins too. Writes into other
  attributed versions mark their licence record "may have been modified"
  before they run, and an admin update that names no graph is followed by a
  re-check of every checked copy.
- **A group's membership could be read and rewritten from any organisation's
  path.** The three group-member endpoints — `GET` / `POST
  /api/organisations/:org_id/groups/:group_id/members` and `DELETE
  …/members/:user_id` — checked only the caller's role in the `org_id` taken from
  the path (the segment the caller controls), never that `group_id` actually
  belonged to that organisation. An admin of *any* organisation could therefore
  list another org's group membership, add members to it — **including
  themselves, escalating into a tenant they had no authority over** — or remove
  members from it, simply by naming the foreign group under their own org's path.
  The sibling `get_group` / `update_group` / `delete_group` handlers already
  guarded this; the three member handlers now apply the same check, confirming
  the group belongs to the path's organisation before any read or write and
  answering `404` on a mismatch — for platform admins too, so the endpoints
  cannot be used to probe which group ids exist either.
- **A non-publisher could make a dataset public by editing it.** `PUT
  /api/datasets/:id` gated a visibility change on *manage* rights alone, while
  `create_dataset` gates public *creation* on the publish capability. A user who
  could manage a dataset but held no publish capability could therefore create it
  private and then `PUT` it public, bypassing the publisher gate that creation
  enforces. `update_dataset` now requires publisher rights for the transition
  into public, matching creation (`is_publisher()` still covers platform admins).
  Only that transition is gated: an unchanged or narrowing visibility — including
  editing an already-public dataset's metadata, which the frontend resends with
  the current visibility on every save — is unaffected.
- **Several read endpoints leaked private-graph content — and one leaked
  non-public asset bytes — to anyone who could read the dataset.** A private
  dataset graph is meant to be visible only to a writer (owner / maintainer /
  admin); the rule `GET /api/datasets/:id/graphs` already enforces. But the
  viewer feed, geo-stats (single and batched), 3D-Tiles, the OGC API – Features
  collection/items, the triple-browser suggestions and the dataset commit log
  each scoped on the *raw* registered-graph list after only checking dataset
  access, so a plain viewer — or an anonymous caller on a public dataset — saw
  private graphs' geometry, labels, feature IRIs, autocomplete values and commit
  history (message, affected graph IRIs, add/remove counts, actor). Separately,
  the ICDD **container export** zipped *every* asset regardless of its `public`
  flag, so an anonymous export of a public dataset downloaded its non-public
  files. Each read path now scopes to the graphs the caller may actually read
  (a new fail-closed `AuthDb::list_readable_dataset_graphs`: all graphs for a
  writer, non-private ones for everyone else — a lookup error propagates as
  `500` rather than degrading to an empty or full list), and the container
  export drops non-public assets from an anonymous export, matching
  `list_assets` / the asset download route. A writer/owner still sees everything.
- **A write-scoped user could write into any named graph, bypassing the graph
  ACL.** The SPARQL UPDATE path resolves and ACL-checks every named-graph write,
  but two data-loading paths did not, because they loaded the request body while
  keeping its embedded graph names. The Graph Store Protocol default-graph write
  (`PUT`/`POST /store` with no `?graph`) accepted TriG, N-Quads and JSON-LD and
  kept their graph names, and an LDP RDF Source loaded from `application/ld+json`
  kept the body's JSON-LD named graphs (`{"@id":"<victim graph>","@graph":[…]}`).
  The default graph and one's own LDP resource are writable without a per-graph
  grant, so any write-scoped user — a self-registered account included — could
  write into another tenant's private dataset graph or a `urn:system:*` graph.
  Both paths now load **triples only**: a body that names a graph of its own is
  rejected (`400`) and nothing is written. A `?graph`-targeted Graph Store write
  is unaffected (every quad is forced into that one graph, which is ACL-checked),
  and multi-graph loads still go through the dataset import API, which enforces the
  per-graph boundary. Every released version was affected. *(Note: LDP resources
  still follow the global RBAC rather than per-resource ACLs — see `docs/ldp.md`;
  tightening that is tracked separately.)*
- **`GET /api/shacl/detect-shapes?graph=<iri>` counted the SHACL shapes in any
  named graph, for any signed-in caller.** The handler took a graph IRI from
  the query string and scanned it for `sh:NodeShape`/`sh:PropertyShape`
  declarations with no read check at all, so a signed-in principal could learn
  the shape count of another tenant's private shapes graph — or of a
  `urn:system:*` graph — simply by naming it. It now applies
  `check_graph_read_access`, the same visibility helper that gates `/store`,
  `/sparql` and `POST /api/shaclc/serialize`: a graph the caller may not read
  answers `403` whether or not it exists, so it cannot be used to discover which
  graph IRIs are present either. Admins keep their bypass, because that helper
  denies `urn:system:*` and unregistered graphs even to them, and an admin's own
  imports land in unregistered graphs — exactly the graph the importer probes
  right after writing it.
- **`POST /api/reasoning/materialize` over a `dataset` reasoned across the
  dataset's private graphs and wrote the consequences into a graph the caller
  can read.** The handler checked only that the caller could *access* the
  dataset, then took its whole reasoning layer from `conformance::resolve`,
  which lists every dataset graph without regard to who is asking. A viewer of a
  public dataset could therefore materialise a private graph's triples — the
  RDFS/OWL closure over data they were never allowed to see — into a
  caller-chosen target they could read back, laundering the private data out.
  The reasoning source set is now filtered to the graphs the caller may read
  (admins still read all; the model registry's own visibility rule still admits
  model graphs), exactly as the endpoint already checks any explicitly named
  `source_graphs`. A dataset owner or other writer still reasons over the whole
  dataset.
- **Saved-query (API service) private-graph and lifecycle leaks.** Four fixes in
  the `…/api-services/…/run` subsystem:
  - A run over a **version snapshot** (`?version=<label>`, or the default run of a
    dataset that has any version) leaked private graphs. A snapshot copies private
    graphs into version-scoped IRIs, and the reader filter compared them against
    *live* private IRIs, so it removed nothing — a viewer, or an anonymous caller
    on a public dataset's API service, read them. The filter is now version-aware:
    it maps each snapshot back to its live source graph and drops the private ones
    for a non-writer.
  - An **organisation/group-scoped** service read the union of *every* graph in
    the owner's datasets, private ones included. It now includes a private graph
    only for a caller who can write that dataset.
  - A dataset **Editor** could make a service `public`, exposing the dataset's
    (non-private) data to anonymous callers, without the publish rights
    `create_dataset` requires. Setting `visibility=public` now needs manage rights
    on the scope (and, for a dataset, the publish capability); the value is also
    validated.
  - Deleting a dataset, organisation or group left its API services behind, and
    ids are reusable slugs — so a `public` service planted on an id could, after
    the id was reused by an unrelated tenant, read the new resource's data.
    Deletes now remove the owner's services in the same transaction, a one-time
    sweep drops pre-existing orphans, and a dataset-scoped run/read requires the
    dataset to exist even for a public service. Every released version was
    affected.
- **A dataset version's data dump and diff leaked private graphs.** A version
  snapshot copies every graph the dataset held at the time — private ones
  included — into version-scoped IRIs that never appear in the dataset's graph
  list, so the private-graph filter that guards `/sparql` and the dataset-service
  version reads was a no-op on two other version endpoints. `GET
  /api/datasets/{id}/versions/{ver}/data` served every snapshot graph's triples,
  and `GET /api/datasets/{id}/versions/{ver}/diff/{other}` returned private
  triples (as an RDF-Patch) and per-graph add/remove counts keyed by the private
  source graph's IRI, to anyone who could read the dataset — a viewer, or an
  anonymous caller on a public dataset. Both now map each snapshot back to its
  live source graph and drop the ones flagged private for a caller who cannot
  write the dataset; a writer (and an admin) still sees everything. A dataset
  with no private graph is unaffected, so legacy versions with no source map keep
  working. Every released version was affected.
- **The `/sparql` read boundary could be tricked into reading any graph in the
  store, unauthenticated.** A non-admin query is scoped by rewriting its text:
  `scope_query_to_authorized` strips the caller's `FROM` / `FROM NAMED` clauses
  and injects a prologue naming only the graphs the caller may read. Text
  rewriting cannot be made perfect, and two inputs slipped through. A ` WHERE `
  (or `{`) inside a string literal mis-anchored the injection, so the prologue
  landed **inside** the literal and the query reached the engine with no dataset
  clause at all — reading every named graph, including other tenants' private
  datasets and graphs flagged private, with no graph IRI needing to be known
  (`SELECT ?g ?o ("""x WHERE x""" AS ?z) WHERE { GRAPH ?g { ?s ?p ?o } }`). And a
  `FROM NAMED` the scanner did not recognise — a prefixed-name source,
  `FROM NAMED<iri>` with no space, or a comment between the keyword and the IRI —
  survived unstripped, letting the caller name a private graph directly. The same
  rewriter also backs the dataset-service SPARQL endpoint and the saved-query
  `…/run` API, so the bypass reached those too. An anonymous request to a public
  dataset was enough. Every non-admin query is now checked one more time, after
  rewriting and immediately before it reaches the engine: the exact text is
  parsed and refused with `403` unless its dataset names only graphs the caller
  may read (a query left with no dataset clause, or a `FROM` without a matching
  `FROM NAMED`, is refused as tampering). The check is fail-closed and does not
  depend on the scanner being perfect. Admins are unchanged — they are scoped
  additively over every registered graph and may read all of it. Every released
  version was affected.
- **Any signed-in user could block writes to a graph with a gating
  pipeline.** A SHACL Studio pipeline with `gate_writes` refuses (422) every
  write its shapes reject to the graphs it covers, for everyone, the graphs'
  owners and editors included. Creating or updating one checked only the
  graphs the pipeline writes itself (inference and report targets), so any
  signed-in user could gate a public dataset, or any graph they could name,
  with shapes that reject everything and block every write to it. Setting a
  gate now needs what a validation-layer binding, which gates writes the same
  way, needs: write access to every dataset it covers (dataset targets, and
  `dataset_ids` while no `graph_iris` narrow the scope) and a graph-ACL write
  grant on every graph it names (graph targets, `graph_iris`). Admins pass.
  Anything else answers 403, and a dataset that does not exist 404. A
  pipeline that only validates needs no write access. The gate acts with its
  creator's authority, checked at every write, so a gating pipeline stored
  before this release, or one whose creator has since lost that write access
  or been deactivated, no longer gates. The server logs a warning at each
  write such a pipeline would have gated.
- **A dataset's SPARQL service could read any graph in the store.** Adding
  a graph to a service (`POST /api/datasets/{id}/services/{service_id}/graphs`)
  checked only that the caller could write the dataset in the path. It
  checked neither the graph nor that the service belonged to that dataset.
  So any user who could create a dataset could scope a service to another
  tenant's private graph or a `urn:system:` graph and read it through the
  service, and so could everyone who could read that dataset, anonymous
  callers on a public one included. A writer of one dataset could also read,
  rename, delete or re-scope another dataset's service by putting its id
  under their own dataset's path. Every `/services/{service_id}` route now
  answers 404 for a service of another dataset. Adding a graph needs the
  dataset to hold it (its namespace, its well-known graphs, or registered
  to it), except for admins. A service query serves only the service graphs
  the dataset holds when it runs, so rows made before this fix, and rows
  whose graph was detached since, serve nothing. A service left with no
  such graph returns nothing; it does not fall back to the whole dataset.
  Every released version was affected.
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
    used for service-registry self-registration to match. The default
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

[Unreleased]: https://github.com/philipperenzen/open-triplestore/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/philipperenzen/open-triplestore/compare/v0.6.0...v0.7.0
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
