# Privacy Notice (Template)

> **This is a template, not legal advice.** Review with qualified counsel and
> adapt it to your specific deployment before relying on it. Whether your use of
> Open Triplestore complies with the GDPR or any other privacy law depends on how
> *you* configure, operate, and document your deployment — not on the software
> alone.

---

## 1. About this document

Open Triplestore is **source-available, self-hostable** RDF triplestore software
(licensed under the [AGPL-3.0 with the Commons Clause](LICENSE)). It is **not a
hosted service** operated by the project maintainers.

This means:

- **You (the person or organisation who deploys Open Triplestore) are the data
  controller and operator.** You decide what data is loaded, who may access it,
  and for how long it is kept.
- **The project maintainers do not operate your instance, cannot access it, and
  receive no data from it.** They are not a data controller or a data processor
  for your deployment.

This template is provided so that operators have a realistic starting point for
their own privacy notice. **You must complete the "For operators — fill in"
blocks and have the result reviewed by qualified counsel** before publishing it
to your end users.

## 2. No telemetry to the maintainers

**Open Triplestore ships no analytics, usage telemetry, or "phone-home"
mechanism that sends data to the project maintainers or to any third party
chosen by the project.** There is no embedded analytics SDK, no crash reporter,
and no background reporting endpoint controlled by the project.

The word "telemetry" appears in the source code only for instrumentation that
stays on your instance. **Workload telemetry** (`GET /api/admin/telemetry`,
administrators only) keeps exact query counters and fixed-size in-memory samples
of query latencies, SHACL validation timings and write spacing. It holds no
query text, user identifiers or IP addresses, is never written to disk, and is
lost on restart. The **usage tracking** of §4.5 and the **LLM request log** of
§4.7 are stored in your own metadata database, the **change log** of §4.9,
when it is switched on, in a SQLite file beside the RDF store, and the
**commit trail** of §4.10 in the RDF store itself. None of it is ever
transmitted off your instance by the software, except the change log to the
replication followers you set up (§4.9).

All network calls the software can make are **either local to your own
infrastructure or directed at endpoints that you explicitly configure**. They
are enumerated in §5, together with instructions for disabling each one.

## 3. Roles and responsibilities

| Party | Role |
|-------|------|
| **You / your organisation** (the operator) | **Data controller** (and, in many setups, the data processor as well). You determine the purposes and means of processing. |
| **Your end users** (people whose data you store, or who log in) | **Data subjects** |
| **Open Triplestore maintainers** | **Neither controller nor processor** for your deployment. They publish software; they do not process your data. |
| **Third-party services you connect** (e.g. an identity provider, an LLM API) | **Independent controllers or your processors**, depending on your contract with them. Assess each one separately. |

## 4. Categories of data the software stores

Open Triplestore stores data **on the infrastructure where you run it** (a local
SQLite metadata database, the RDF store and, when switched on, its change log
(§4.9), and — if configured — an S3-compatible
object store such as the Versity S3 Gateway the compose stack bundles, for uploaded assets). The categories below are what
the software is *capable* of storing; what your specific instance actually holds
depends on how you use it.

### 4.1 Operator-loaded RDF data

The triples, named graphs, and datasets you (or your users) import. **This is the
primary content of the system and may contain personal data** if you choose to
load it (for example, RDF describing people). The software treats this as opaque
content — controlling whether it contains personal data, and on what lawful
basis, is the operator's responsibility (data minimisation, see the
[GDPR guide](docs/gdpr.md)).

### 4.2 User accounts and credentials

For each local user account, the metadata database may store: a user ID,
username, email address, **a salted Argon2id hash of the password** (never the
plaintext password), system role, active/public flags, and optional profile
fields (display name, bio, website, phone, organisation, avatar reference).

- Passwords are hashed with **Argon2id** (`src/auth/password.rs`); the plaintext
  is never stored and is not recoverable.
- If you authenticate users via an external identity provider (OIDC/SAML, §5),
  account and claim handling is shared with that provider; review its own
  privacy terms.

### 4.3 Session tokens and API keys

- **Session tokens (JWT).** Browser sessions use short-lived access tokens and
  longer-lived refresh tokens, signed with your `OTS_JWT_SECRET`
  (`src/auth/jwt.rs`). In browsers they are delivered as **`HttpOnly`,
  `SameSite=Strict`** cookies (`access_token`, `refresh_token`); set
  `OTS_SECURE_COOKIES_ENABLED=true` behind HTTPS to add the `Secure` attribute.
- **API keys (`ots_…`).** Long-lived programmatic tokens. Only a **SHA-256 hash**
  of each key and a short non-secret prefix are stored — the full key is shown
  once at creation and cannot be recovered. Keys can carry scopes, expiry, and be
  revoked.
- **Refresh tokens** are likewise stored only as hashes and can be revoked.

### 4.4 Audit logs

An **append-only audit log** (`audit_events` table, enforced append-only by
database triggers — `src/auth/audit.rs`) records security-relevant events:
logins (success/failure), logout, token creation/revocation, password changes,
user create/update/deactivate, role changes, SPARQL updates, graph create/delete,
permission-denied/ACL events, backup events, and alert dispatches. Entries may
include **actor ID/username/role, timestamp, IP address, user agent, request ID,
and event details**. IP address and user agent are personal data; set your
retention period accordingly (§7).

### 4.5 Private usage tracking ("recently used")

A local, privacy-respecting feature records lightweight per-user usage events
(e.g. which dataset was viewed/validated, and when — `dataset_usage_events`,
`src/auth/db.rs`) to power "recently used / used a lot" rankings.

- **A user only ever sees their own footprint.** The per-user read path is scoped
  to the requesting user.
- **Cross-user aggregates are restricted to `super_admin`.**
- Purging a user account deletes that user's events.
- This data **never leaves your instance**; it is not sent to the maintainers or
  any third party.

### 4.6 Operational data

Application logs, optional backups, and (if configured) outbound operational
alerts (§5) may incidentally contain identifiers. Treat backups as containing
every category in this section, the change log of §4.9 included.

Workload telemetry (§2) is kept in memory only and holds no personal data. Each
stored SHACL validation run (the last 50 per dataset) also records how long it
took and how many quads it read.

### 4.7 LLM request log

When the AI features are in use (§5), every LLM-backed request (a chat turn,
natural-language→SPARQL, the SHACL assistant, a saved-query repair) adds one row
to the `llm_request_log` table (`src/server/llm_guard.rs`), and so does any
training-feedback signal the input guard blocks. Each row holds:

- the time, the endpoint and model, the outcome (`ok`/`error`/`blocked`), the
  guard rule that fired, if any, and an error message on failure;
- latency, time to first token, prompt and answer sizes in characters, and the
  number of query rounds;
- **the user ID of a signed-in caller and, for most requests, the caller's IP
  address**;
- **a preview of the question** (for a query repair, of the error message):
  its first 200 characters. The rest of the
  question and the answer are not stored in this log (saved conversations are
  a separate feature, §4.8).

The log is readable by administrators (`GET /api/admin/llm/requests`, with the
username, IP address and preview; `GET /api/admin/llm/stats` for counts per
outcome and the most active users); a plugin holding an administrator's token
can read the same counts and the top 10 users.

- **Retention:** rows older than `OTS_LLM_LOG_RETENTION_DAYS` (default **90**
  days) are deleted whenever a new row is written, so while the AI features go
  unused older rows stay until the next request.
- **Question preview:** on by default. Set `OTS_LLM_LOG_PREVIEW_ENABLED=false`
  to stop storing it; rows written before the change keep theirs until they
  expire.
- Purging a user account deletes that user's rows.
- This log **never leaves your instance**. The question itself does go to the
  LLM endpoint you configured (§5).

### 4.8 Assistant conversations and feedback reports

- **Saved chat conversations** (`chat_conversations`, `chat_messages`;
  `src/server/llm_history.rs`). A signed-in user's conversations with the
  assistant are stored in full — every message's text and the queries the
  assistant ran — so they can be reopened. Only their owner can read them;
  the owner can rename and delete them, and past 200 conversations per user the
  oldest are pruned. The standing instructions a user gives the assistant
  ("memory", up to 4,000 characters, `chat_user_memory`) are stored as well.
- **Feedback reports** (`feedback_reports`; `src/feedback/`). A report a user
  sends from the in-app dialog stores its title and text, the page it was sent
  from (optional), the browser's user agent and the app version, with the
  reporter's user ID. The reporter sees their own reports; administrators
  read, answer and delete them.

Both are deleted when the user account is purged.

### 4.9 Change log

The change log is **off by default**: `OTS_CHANGE_CAPTURE_ENABLED=on` switches
it on, and a replication leader or cluster member (`OTS_REPLICATION_ROLE=leader`
or `cluster`) keeps it on regardless, because its followers read it. While it
is on, every write to the RDF store adds a row per graph to a SQLite database
beside the store, `<data dir>/changes/changes.db` (`src/store/changes.rs`; in
memory for an in-memory store). Each row holds:

- the time of the write, the graph, the kind of write and the commit it
  belongs to;
- **the quads the write added and removed**, unless the write was too large
  to copy (`OTS_CHANGE_CAPTURE_MAX_PAYLOAD` and `OTS_CHANGE_CAPTURE_MAX_SCAN`,
  250 000 quads each by default), when the row keeps only the counts or just
  the fact that the graph changed. Any personal data in your RDF (§4.1) is copied
  into the log this way, **including the quads a write deleted**. User profile
  graphs (`urn:system:user:{id}`) are the exception: a row of one keeps the
  time, the graph and the number of statements changed, and never the
  statements, so neither the values an edit replaced nor the values of a
  purged profile are in the log;
- **the account that made the write** (`actor_iri`, of the form
  `{OTS_BASE_URL}/users/{user ID}`) when the write came through SPARQL Update,
  the Graph Store protocol, RDF Patch, a version restore or an applied repair.
  Other writes, such as file imports and the server's own writes, leave it
  empty.

A consumer's bookmark (cursor) in the log records the user ID of the account
that set it.

**Administrators** read every row, quads and account included
(`GET /api/admin/changes`). `GET /api/admin/changes/status` and
`GET /api/admin/changes/stats` add counts and the bookmarks and pins with the
user or dataset that owns them. A replication follower reads the same feed
with an administrator's credentials and so receives every row.

**Anyone who may read a dataset** (anonymous callers too, for a public
dataset) lists the rows of the dataset's graphs they may read
(`GET /api/datasets/{id}/changes`): time, graph, kind and counts, without
the quads. Two settings, both `writers` by default, decide who gets more:

- **the account IRI** on those rows goes to the audience of
  `OTS_ACTOR_VISIBILITY`;
- `GET /api/datasets/{id}/changes/{seq}`, which returns one change's quads,
  removed ones included, as an RDF Patch, answers the audience of
  `OTS_CHANGE_PATCH_ACCESS`, and only that audience can set a bookmark on
  the dataset's feed.

`writers` is the accounts that may write the dataset, administrators
included. An administrator can widen either setting to `signed-in` (any
account that may read the dataset) or `readers` (everyone who may read it,
anonymous callers on a public dataset included); a notice for an instance
that does so should name the wider audience here.

- **Retention:** rows older than `OTS_CHANGE_RETENTION_DAYS` (default **90**
  days) are deleted by a sweep that runs a minute after the server starts and
  every hour from then on, so a row that nothing holds is gone within an hour
  of leaving the window. The sweep never deletes a row at or above the lowest live bookmark or pin, of
  any graph: a bookmark holds rows until it goes unused for
  `OTS_CURSOR_TTL_DAYS` (default 30 days), and a delta version's pin holds
  them until the version is deleted or published. Rows of version-snapshot
  and restore-staging graphs go after a day.
- **Off switch:** leave `OTS_CHANGE_CAPTURE_ENABLED` unset (or `off`) on a node
  that is not a replication leader or cluster member. Switching it off stops
  recording but **does not delete a log an earlier run wrote**: that file
  stays readable through the routes above and goes into backups, and the
  hourly sweep keeps applying retention to it. To remove it, stop the server and delete `<data dir>/changes/`; an unpublished delta
  version, which is a checkpoint plus a range of the log, can then no longer
  be built.
- **Backups:** a backup copies the change log whenever the store keeps one
  (encrypted like the rest of the backup when backup encryption is on), and
  restoring the backup puts its log back.
- **Purging a user account does not touch the change log.** Its rows keep the
  account's IRI and the quads of its writes until retention removes them. The
  instance can no longer turn that IRI into a username, but the user ID in it
  is the one the audit log (§4.4) keeps with the username. An admin purge
  and a self-service purge also delete the user's profile graph. The log
  records that delete without the removed values: the row keeps the time, the
  graph IRI (which contains the user ID) and the number of statements
  removed. A log written by an earlier version is cleaned of profile values
  at the first start after the upgrade; backups taken before then still hold
  the earlier rows.
- The log stays on your instance and on the replication followers you set
  up; nothing in it is sent anywhere else.

### 4.10 Commit trail and statement provenance

A write through any of these leaves a **commit** in the RDF store itself, in
the named graph `urn:system:commit-log` (`src/commit_log.rs`): SPARQL Update,
the Graph Store protocol, RDF Patch, bulk imports and containers, LDES syncs,
dataset version operations, model, vocabulary and shape-graph editor saves,
datasource runs, applied repairs and backup restores. It cannot be switched
off. A commit holds no copy of the data. It records:

- the time, the kind of write, the graphs it touched and the number of quads
  added and removed (and, while the change log of §4.9 is on, the sequence
  numbers of its rows there);
- **the account that made the write** (`prov:wasAssociatedWith
  <{OTS_BASE_URL}/users/{user ID}>`) for a signed-in caller. An anonymous
  write and a write the server makes on its own, such as a backup restore,
  carry none;
- **a message**: the text a user typed when saving a model, vocabulary or
  shape graph, the `X-Commit-Message` header of a SPARQL Update, or a fixed
  description otherwise (`Graph Store PUT <graph>`, `Imported 2 file(s) into
  1 graph(s)`). A message is stored as typed;
- for an editor save, the model or shape graph, version, branch and revision
  it belongs to and the editor's metadata (JSON).

A dataset can, in addition, record **statement provenance**. A dataset writer
switches it on with `PUT /api/datasets/{id}/settings` (`"provenance":
"minimal"` or `"full"`; off by default; it works only while the change log of
§4.9 is on and in a build with the `rdf-12` feature, which the published image
has). From then on a background consumer annotates every change to the
dataset's statements, in RDF 1.2, in one of two graphs of the dataset
(`src/statement_provenance.rs`): `urn:ots:statement-provenance:{id}` for the
changes to its graphs that are not private, and
`urn:ots:statement-provenance:{id}:private` for the changes to the graphs it
holds as private. An annotation holds:

- **a copy of each changed statement** as a triple term (`rdf:reifies
  <<( s p o )>>`), **removed statements included**, with its graph, and for
  each change the operation (`add` or `remove`), the change-log sequence
  number and the commit;
- with `"full"`, **the account that made the change** (the same
  `prov:wasAssociatedWith` IRI, for a write through SPARQL Update, the Graph
  Store protocol, RDF Patch, a version restore or an applied repair) and
  **its time** (`prov:atTime`).

A statement with a blank node is skipped (unless the dataset imports with
Skolem IRIs); each skip is recorded as a gap, without the statement.

**Who reads them.** The commit graph belongs to no dataset, so a `/sparql`
query that does not name it never sees it, and the Graph Store protocol
refuses it to everyone but administrators. An **administrator** reads it whole
by naming it (`FROM NAMED <urn:system:commit-log>`, or
`GET /store?graph=urn:system:commit-log`), and so can list every write an
account ever made. **Anyone who may read a dataset** (anonymous callers too,
for a public dataset) lists the commits of the graphs they may read
(`GET /api/datasets/{id}/commits`: time, kind, message, counts and graphs)
and gets the same trail as PROV-O (`GET /api/datasets/{id}/provenance`).
**The writer's account IRI, with the username and display name** while the
account exists, is on both only for the audience of `OTS_ACTOR_VISIBILITY`
(§4.9): by default the accounts that may write the dataset, administrators
included. The PROV-O document names the dataset's owner (a user, group or
organisation) to every caller. **That setting does not reach the history of
a model, vocabulary or shape graph**: anyone who may see a model or
vocabulary reads its commits with each writer's account IRI, username and
display name (`GET /api/models/{id}/commits`), and so does a signed-in user
who may see a shape graph (`GET /api/shacl/shape-graphs/{id}/commits`). The
web UI shows these histories. The DCAT catalogue names only each dataset's
latest commit, without its account.

**The statement-provenance graphs are graphs of the dataset.** Everyone who
may read the dataset reads `urn:ots:statement-provenance:{id}`, over `/sparql`
(`GRAPH <urn:ots:statement-provenance:{id}>`), the Graph Store protocol and
the browser (`graph=`), **including, under `"full"`, the account behind each
change: `OTS_ACTOR_VISIBILITY` does not filter a graph.** The private one is
read as the dataset's private graphs are, by its writers and administrators.
A graph made private takes its annotations along to the private graph; what
was annotated while a graph was private stays there when the graph is made
public again. Both are left out of LDES feeds, reasoning, validation and the
default browse view, and out of a version unless the version is cut with the
graph named.

- **Retention: none.** Nothing in the software removes a commit or an
  annotation; there is no retention setting and no sweep. Setting
  `"provenance": "off"` stops new annotations but keeps the graphs. A dataset
  delete removes the dataset's statement-provenance graphs but **not its
  commits**, which keep their graph names, messages and account IRIs. An
  administrator can empty the commit graph (`DROP GRAPH
  <urn:system:commit-log>` over SPARQL Update, or a Graph Store `DELETE` of
  it; the drop is recorded as the first commit of the emptied trail), or
  remove one account's attribution from it with a SPARQL `DELETE` that names
  the graph and deletes the triples naming the account's IRI (the commits
  stay, unattributed). No request may write or clear a statement-provenance
  graph, administrators included: it goes with its dataset, or with
  `CLEAR ALL` / `DROP ALL`.
- **Backups:** both are RDF in the store, so every backup's dump holds them
  and a restore brings them back (§4.6). A restored store's trail is the
  backup's, plus a commit recording the restore.
- **Purging a user account keeps its name in both, or replaces it, as you set
  `OTS_PURGE_ATTRIBUTION`.** With `keep`, the default, a purge touches
  neither. The account's commits keep its IRI, messages and graph names for
  good, and a statement-provenance graph keeps its IRI, the times and the
  statements the account added or removed. The instance can no longer turn
  that IRI into a username (the `/commits` endpoints then show the IRI alone,
  to those they show it to), but the user ID in it is the one the audit log
  (§4.4) keeps with the username. With `deleted-user`, erasing an account (an
  administrator's purge, the user's own, or `DELETE /api/users/{id}`) first
  attributes the account's commits and, under `"full"`, its statement changes
  to one shared agent, `{OTS_BASE_URL}/users/deleted`, shown as "Deleted
  user". The commits and annotations stay, with their times, messages, graphs
  and statements; which account made them is no longer in either record. If
  the replacement fails, the account is not erased. The setting covers these
  two records only. It does not rewrite an account erased earlier under
  `keep`, the change log (§4.9 keeps the account on its rows, and the rows of
  the replacement itself, until retention), backups taken before, a
  replication follower's copy of the commit graph (the one it fetched at its
  last full resynchronisation), a commit message that names the person, or
  other records that say who made something, such as the creator of a
  dataset or model version and the author of a datasource run or review.
  Keeping the identifier of an erased account needs a ground of your own: a
  legal duty to keep the record, or an interest in it that outweighs the
  person's. Choose `deleted-user` if you have none, and say in your notice
  which you chose. The purge itself leaves no commit; it deletes the profile
  graph directly.
- Both stay on your instance and the replication followers or cluster
  members you set up; nothing in them is sent anywhere else.

## 5. External network calls and how to disable each

By design, every outbound call is **off by default or points only at endpoints
you configure**. None of them send data to the project maintainers.

| Feature | What it contacts | When | How to disable |
|---|---|---|---|
| **Prefix resolution** | Nothing, by default: prefixes resolve against a **bundled snapshot** of the prefix.cc + LOV registries (~3.7k prefixes) plus this instance's own vocabularies (`src/prefixes/`). Only with the opt-in `OTS_PREFIX_CC_FALLBACK_ENABLED=true` does an unknown label fall through to `https://prefix.cc`, sending the prefix **label** only (e.g. `foaf`), never your data. | Fallback only: when explicitly enabled *and* a query uses a prefix unknown to every local tier. Results are cached locally. | Already off by default — leave `OTS_PREFIX_CC_FALLBACK_ENABLED` unset. Lookups fail soft. |
| **LOV corpus download** | The pinned Linked Open Vocabularies corpus snapshot URL (`OTS_VOCAB_CORPUS_URL`, default an Internet Archive snapshot), fetched **once** and sha256-verified, to enable vocabulary term search and offline vocabulary installs. Docker images normally ship the corpus baked in, so no runtime call happens at all. (`src/vocab_search/`) | Only at first boot when the corpus file is not already present on disk. No instance data is sent. | Set `OTS_OFFLINE=true`. Vocabulary search then covers the vocabularies registered on this instance only. |
| **AI / LLM assistance** | An **OpenAI-compatible LLM endpoint that you configure** (`OTS_LLM_GATEWAY_URL`) — e.g. OpenAI, OpenRouter, Azure, or a local Ollama/vLLM server. Used for natural-language→SPARQL, the SHACL assistant, and the grounded chat; optional "training feedback" signals are forwarded to that same endpoint. (`src/server/llm_sparql.rs`) | Only when a user invokes an AI feature. The prompt/context (which may include schema and query text) is sent to **your** endpoint. | **Leave `OTS_LLM_GATEWAY_URL` unset/unreachable** — the AI features then hide in the UI. If you use a hosted LLM, review *that provider's* privacy terms; prefer a local/self-hosted model to keep data on your infrastructure. |
| **Identity providers (SSO)** | An **OIDC** and/or **SAML** provider that you configure (`src/auth/`, `OTS_OIDC_ISSUER`/`OTS_OIDC_AUDIENCE`, SAML metadata). | Only when SSO login is configured and used. Standard auth handshakes occur with that provider. | Leave OIDC/SAML unconfigured to use local password + API-key auth only. |
| **OAuth login** | Your configured OAuth provider(s) (`src/auth/oauth.rs`). | Only when configured/used. | Leave unconfigured. |
| **Operational alerts** | An **HTTP webhook** (`OTS_ALERT_WEBHOOK_URL`) and/or an **SMTP server** (`OTS_ALERT_SMTP_*`, requires the `alerting` build feature) that you configure. (`src/alerting/`) | Only on operational events when configured. Sends alert text you can scope. | Leave `OTS_ALERT_WEBHOOK_URL` and `OTS_ALERT_SMTP_*` unset. |
| **Service registry (companion-app discovery)** | A service registry you run (`OTS_REGISTRY_URL`, default `http://localhost:8500`). Advertises this instance's own URL so sibling services can discover it. (`src/svc_registry.rs`) | Best-effort heartbeat at startup and periodically. Fail-soft: the registry being down never affects the store. | Do not run a registry at that address (calls simply fail and are ignored), or point `OTS_REGISTRY_URL` at an unused address. No data beyond the instance's own name/URL is sent. |
| **Validation service** | An external SHACL validation service you configure (`OTS_VALIDATION_API_URL`). (`src/dataset_versions/commit.rs`) | Only when the validate-and-commit endpoint is used. Forwards the data to be validated and the caller's bearer token. | Leave `OTS_VALIDATION_API_URL` unset; the endpoint then returns a configuration error instead of calling out. |

### 5.1 Third-party requests made by the web frontend (the user's browser)

The bundled web UI, when loaded in a browser, fetches some assets from public
CDNs. These requests come **from the end user's browser** to third parties and
expose the user's IP address to them:

- **Google Fonts** (`fonts.googleapis.com`, `fonts.gstatic.com`) — web fonts,
  loaded on every page (`frontend/index.html`).
- **OpenFreeMap** (`tiles.openfreemap.org`) — the street basemap's style, vector
  tiles and label fonts, fetched when a map is rendered (the 2D and 3D viewers
  and the geometry preview). Every page also opens a connection to this host in
  advance (`<link rel="preconnect">` in `frontend/index.html`) so maps paint
  sooner; no request is sent over it until a map renders, but the host sees the
  connection.
- **Esri World Imagery** (`ibasemaps-api.arcgis.com`) — satellite tiles, only
  when the operator has configured an Esri API key (runtime config
  `basemaps.esriApiKey`, see [docs/plugins.md](docs/plugins.md)) **and** the user
  switches a map to satellite.
- **Files linked from the data** — previewing a 3D model or similar file whose
  IRI points at another host (the 3D chip on a value) makes the browser fetch it
  from that host, on the user's click.

The map libraries themselves (Leaflet, MapLibre, Cesium and its assets) are
bundled and served from this instance; 3D tiles of a dataset come from this
instance's own `/api/datasets/{id}/3dtiles/` endpoint.

Prefix lookups from the browser (SPARQL editor autocomplete, prefix search)
go to this instance's own `/api/prefixes` service — the UI no longer contacts
`prefix.cc` directly.

> **Operator note:** If your privacy posture requires it, you can self-host these
> assets / fonts / map tiles, or disable the geo-preview, so that no third-party
> browser requests are made. Document whatever you decide in your own notice and
> cookie information below.

## 6. Cookies and local storage

- **Authentication cookies** (browser login): `access_token` and `refresh_token`
  — **strictly necessary**, `HttpOnly`, `SameSite=Strict`, and `Secure` when
  `OTS_SECURE_COOKIES_ENABLED=true`. They exist to keep a user signed in and are not used for
  tracking or advertising.
- The frontend may also keep non-sensitive UI state (e.g. language preference) in
  the browser.
- API clients typically send the `ots_…` API key or a bearer token instead of
  cookies.

If your jurisdiction requires consent for non-essential cookies, note that the
above are essential; document any others your customised deployment introduces.

## 7. For operators — fill in

> Complete every item below for your deployment and remove this quote block once
> done. **Have the result reviewed by counsel.**

- **Controller identity:** _[Your legal entity name, address, registration no.]_
- **Contact for privacy enquiries / DPO:** _[name, email, postal address]_
- **Purposes of processing:** _[why you store the RDF data and operate accounts]_
- **Lawful basis** (per purpose): _[e.g. legitimate interests, contract, consent,
  legal obligation]_
- **Categories of data subjects:** _[e.g. staff, customers, data described in RDF]_
- **Categories of personal data actually held:** _[derive from §4 for your case]_
- **Recipients / processors:** _[hosting provider, configured LLM provider, IdP,
  alert/SMTP provider, validation service — list each you enabled in §5]_
- **International transfers:** _[where your servers and any processors are located;
  safeguards used]_
- **Retention periods:** _[per category — RDF data, accounts, audit logs (incl.
  IP/user-agent), usage events, the LLM request log (incl. IP address and
  question preview; `OTS_LLM_LOG_RETENTION_DAYS`, default 90 days), saved chat
  conversations, feedback reports, the change log if switched on (incl. the
  account IRI and the added and removed quads; `OTS_CHANGE_RETENTION_DAYS`,
  default 90 days, longer while a bookmark or delta-version pin holds the
  rows), the commit trail and, per dataset, statement provenance (incl. the
  account IRI; nothing prunes them, and `OTS_PURGE_ATTRIBUTION` decides
  whether a purge keeps it, see §4.10), backups]_
- **Data-subject rights & how to exercise them:** _[your process; see
  [docs/gdpr.md](docs/gdpr.md) for the feature mapping]_
- **Right to lodge a complaint:** _[your lead supervisory authority]_
- **Security measures:** _[summarise; see [SECURITY.md](SECURITY.md) and
  [docs/gdpr.md](docs/gdpr.md)]_
- **Last updated:** _[date]_

## 8. Changes to this notice

Operators should version and date their published privacy notice and inform data
subjects of material changes.

---

*See also: [TERMS.md](TERMS.md) · [GDPR compliance guide](docs/gdpr.md) ·
[SECURITY.md](SECURITY.md) · [LICENSE](LICENSE).*
