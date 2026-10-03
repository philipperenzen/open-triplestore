# API Reference

The full machine-readable API specification is available as an OpenAPI 3 JSON document. You can import it into **Postman**, **Insomnia**, or any OpenAPI-compatible tooling to explore and test all available endpoints.

- **OpenAPI specification** — <a href="/api-docs/openapi.json" target="_blank" rel="noopener noreferrer">/api-docs/openapi.json</a> — machine-readable JSON. A unit test (`spec_documents_every_mounted_route` in `src/server/openapi.rs`) fails the build when a route the server mounts is missing from it, or when it documents one the server does not mount. The only routes it leaves out on purpose are the Raft transport between cluster members, the consent step behind the `/oauth/authorize` page, the banner-preset pickers of the web UI and the `/api/ogc/` alias of `/api/ogc`; plugin routes under `/ext/{name}` are the plugin's own. Each copy is tailored to the caller: operations it may not reach are left out. (An interactive viewer is available at [API Reference](/api-docs).)
- **Authentication** — every endpoint needs one of three levels, **none**, **token** or **admin**; [the table below](#authentication--the-level-every-endpoint-needs) states the level per endpoint. A **token** or **admin** endpoint wants an `Authorization: Bearer <token>` header, generated in **Settings → API Tokens**.

## Authentication — the level every endpoint needs

Authentication is a property of the route, not of the caller: the level below is
what the router demands before a handler runs. Three levels cover the whole API.

- **none** — answered without an `Authorization` header. What comes back is
  **public data only**: datasets marked *public*, the graphs and files they
  hold, and the metadata that names them. A **none** endpoint is not a hole in
  access control — the handler scopes its answer to what is public, so an
  anonymous caller reaches nothing private through one.
- **token** — any valid session or API token, sent as
  `Authorization: Bearer <token>` (mint one in **Settings → API Tokens**).
  Without one the answer is `401`. A token buys the caller *its own* access, not
  everyone's: a dataset it has no grant on still answers `403` (or `404`) with a
  perfectly valid token.
- **admin** — an admin or super-admin account. Anonymously `401`, with an
  ordinary token `403`.

Three facts worth knowing before an instance is exposed:

- **A public dataset is readable without a token, by design.** That is what
  *public* means here: its triples answer over `/sparql`, `/store`, the browse
  API and `/resource/…`, its files download, and it appears in
  `GET /api/datasets` — to anybody who can reach the host. Do not mark a dataset
  public unless the world may read it.
- **A private dataset discloses nothing to a caller without a grant.** Its
  triples and its uploaded files answer `401` anonymously and `403` to a
  signed-in user who holds no grant on it, on the read endpoints as well as the
  write ones, and it stays out of every listing. Only the owner — plus whoever
  has been granted access, and an admin — sees it.
- **Listing every account is admin-only.** `GET /api/users` and the
  `/api/admin/users` directory are **admin**. `GET /api/users/public` is
  deliberately *not* a roster: it answers with the accounts the caller could
  already infer — the owners of the datasets it may read, the members of its own
  organisations, and itself — so the UI can label an owner chip without
  enumerating the instance.

| Method | Path | Auth | Notes |
|---|---|---|---|
| `GET` | `/` | **none** | SPARQL 1.1 Service Description of this node. |
| `GET` | `/health` | **none** | Liveness plus store counters. |
| `GET` | `/livez` | **none** | Liveness only; never touches the store. |
| `GET` | `/sparql` | **none** | Query over the graphs the caller may read — anonymously, the public ones: the graphs of public datasets and of every published version of a public model-registry entry (the bundled vocabularies included). A private entry's version graphs are read by its owner, the owner organisation's members and admins, exactly as `/api/models/{id}/versions/{ver}/data` serves them. |
| `POST` | `/sparql` | **none** | The same query endpoint in the protocol's POST form. A body sent as `application/sparql-update` is a write and needs a **token**; writing a model-registry graph is refused unless the caller may write the entry, whether or not they may read it. |
| `GET` | `/store?graph={graph_iri}` | **none** | Graph Store read of one named graph, scoped exactly like the query endpoint: a public graph (a public dataset's, or a published version's of a public model) answers anonymously, a private one `401`/`403`. Turtle and TriG carry an `@prefix` header: a dataset graph's own prefix table first, then the prefix registry for the namespaces the graph actually uses; the line-based formats write every IRI in full. |
| `GET` | `/store` | **admin** | A read that names no graph dumps the **default graph**, which no per-graph ACL covers, so it is admin-only — a non-admin token is refused here too, with `401` rather than `403`. |
| `PUT` | `/store` | **token** | Graph Store Protocol: replace a graph. |
| `POST` | `/store` | **token** | Graph Store Protocol: merge into a graph. |
| `DELETE` | `/store` | **token** | Graph Store Protocol: drop a graph. |
| `POST` | `/sparql/batch` | **token** | Several updates as one transaction (see below). |
| `GET` | `/api/browse/graphs` | **none** | The graphs in scope for the caller. |
| `GET` | `/api/browse/triples` | **none** | Browse rows, filtered by the caller's visibility and graph permissions. |
| `GET` | `/api/browse/facets` | **none** | Facet counts over the same scope. |
| `GET` | `/api/browse/resource` | **none** | One resource's triples, in scope. |
| `GET` | `/api/browse/stats` | **none** | Counts over the scope. |
| `GET` | `/api/browse/suggest` | **none** | Type-ahead over the terms in scope. |
| `GET` | `/api/datasets` | **none** | The datasets the caller may read; anonymously, the public ones. |
| `POST` | `/api/datasets` | **token** | Create a dataset. |
| `GET` | `/api/datasets/{dataset_id}` | **none** | A public dataset's metadata; a private one is `401` anonymously, `403` without a grant. |
| `GET` | `/api/datasets/{dataset_id}/assets` | **none** | A public dataset's file list; per-file visibility still applies. |
| `POST` | `/api/datasets/{dataset_id}/assets` | **token** | Upload a file. |
| `POST` | `/api/datasets/{dataset_id}/validate` | **token** | Run SHACL validation over the dataset graphs the caller may read; a run that could not read them all is a test run. |
| `GET` | `/api/datasets/{dataset_id}/validation/latest` | **token** | The dataset's last validation run; its full report only for those who may read every graph it validated. |
| `POST` | `/api/datasets/{dataset_id}/repair` | **token** | Propose a repair (see `docs/repair.md`): an explained RDF Patch computed in a throwaway copy of the dataset. Write access to the dataset; nothing is written. |
| `GET` | `/api/datasets/{dataset_id}/repair/proposals` | **token** | The dataset's kept repair proposals, to its writers. |
| `GET` | `/api/datasets/{dataset_id}/repair/proposals/{proposal_id}` | **token** | One kept proposal: its report, patch and a page of its actions, to the dataset's writers. |
| `POST` | `/api/datasets/{dataset_id}/repair/proposals/{proposal_id}/apply` | **token** | Apply a kept proposal: its base is checked (`409`), the write gates run (`422`), and the commit names it. Write access. |
| `POST` | `/api/datasets/{dataset_id}/repair/proposals/{proposal_id}/reject` | **token** | Reject a kept proposal. Write access. |
| `GET` | `/api/datasets/{dataset_id}/shapes` | **token** | The dataset's shapes graph; a private one only for those who may read it. |
| `PUT` | `/api/datasets/{dataset_id}/shapes` | **token** | Replace the shapes graph (`text/shaclc` or RDF). |
| `POST` | `/api/datasets/{dataset_id}/patch` | **token** | Apply an RDF Patch (writers only); `PA` / `PD` change the dataset's prefix table. See [versioning.md](versioning.md#rdf-patch). |
| `GET` | `/api/datasets/{dataset_id}/prefixes` | **none** | A public dataset's prefix table; a private one only to those who may read it. |
| `PUT` | `/api/datasets/{dataset_id}/prefixes` | **token** | Replace the prefix table (writers only); `PUT` / `DELETE …/prefixes/{label}` change one entry. |
| `GET` | `/api/datasets/{dataset_id}/log` | **none** | A public dataset's RDF Patch log; `…/log/init`, `…/log/current` and `…/log/patch/{version-or-id}` serve its version 0 and patches. Entries that change a graph the caller may not read are withheld. |
| `POST` | `/api/datasets/{dataset_id}/log` | **token** | Append a patch to the log (writers only): `H prev` must name the latest entry (`409` otherwise). |
| `GET` | `/api/models` | **none** | The model-registry entries the caller may see; anonymously, the public ones. |
| `GET` | `/api/models/{id}/versions/{ver}/data` | **none** | A published version's graphs as RDF, to whoever may see the entry: a public model anonymously, a private one to its owner, the owner organisation's members and admins (`404` to everyone else, so the entry cannot be discovered). |
| `GET` | `/api/models/{id}/versions/{ver}/profile` | **none** | The version flattened for a mapping proposer (classes, properties, shapes, enumerations). Read by exactly who may read `/data`; it used to sit behind the admin-gated sources router and answer `401` for a public model. |
| `GET` | `/api/organisations` | **none** | Anonymously, only organisations that own something public. |
| `POST` | `/api/organisations` | **admin** | Provisioning an organisation is an operator action. |
| `GET` | `/api/organisations/{org_id}` | **none** | As the listing: an organisation that owns something public is visible. |
| `GET` | `/api/organisations/{org_id}/members` | **token** | Membership, to members and admins. |
| `GET` | `/api/users/public` | **none** | Scoped label lookup, not a roster — see above. |
| `GET` | `/api/users` | **admin** | Every account. |
| `GET` | `/api/auth/me` | **token** | The caller's own account. |
| `POST` | `/api/auth/login` | **none** | Rate-limited against brute force. |
| `POST` | `/api/import/bulk` | **token** | Bulk import; rate-limited. |
| `GET` | `/api/shacl/shape-graphs` | **token** | SHACL studio: the caller's shape-graph library. |
| `POST` | `/api/shacl/shape-graphs` | **token** | SHACL studio: create a shape graph. |
| `GET` | `/api/shacl/detect-shapes` | **token** | SHACL studio: infer shapes from data. |
| `GET` | `/api/shacl/dataset-shape-graphs` | **token** | The datasets that carry a shapes graph. |
| `POST` | `/api/shacl/validation/latest` | **token** | The last validation run of several datasets at once. |
| `POST` | `/api/shaclc/parse` | **token** | W3C SHACL-C → SHACL (see below). Needs a token since 0.6.x: it spends the instance's CPU on caller-supplied text. |
| `POST` | `/api/reasoning/materialize` | **token** | Materialise an entailment regime into a graph the caller may write, over graphs the caller may read; `?async=true` queues it as a job (202). `owl2-dl` needs a configured DL backend (503 without one). |
| `POST` | `/api/reasoning/check` | **token** | OWL 2 DL consistency, entailment, satisfiability or profile check over graphs the caller may read, or over Turtle in the body; `?async=true` queues it as a job. |
| `GET` | `/api/reasoning/jobs/{job_id}` | **token** | A background reasoning job, to the user who started it and admins (`404` to anyone else). |
| `POST` | `/api/shaclc/serialize` | **token** | SHACL → SHACL-C of a graph named by the caller, lossless or `422` (see below). Needs a token since 0.6.x, and the caller must be allowed to read that graph: it reads whatever IRI it is given out of the store, so it was previously a way for anyone to read any graph. A graph you may not read answers `403`, whether or not it exists. |
| `POST` | `/api/rml/preview` | **token** | Runs a mapping into a throwaway store. Needs a token since 0.6.x, for the same reason as `/api/shaclc/parse`. |
| `GET` | `/api/prefixes` | **none** | Bundled prefix registry; rate-limited. |
| `GET` | `/api/admin/prefixes` | **admin** | What this deployment has decided its prefixes mean. |
| `POST` | `/api/admin/prefixes` | **admin** | Claim a shorthand for a namespace. Refuses a label that already has an override, with `409` and what it currently resolves to — two prefixes with the same shorthand cannot both be right, and repointing one silently would change what every stored CURIE expands to. |
| `PUT` | `/api/admin/prefixes/{label}` | **admin** | Set or repoint a shorthand. `201` when it is new, `200` when it repointed one. |
| `DELETE` | `/api/admin/prefixes/{label}` | **admin** | Drop this deployment's opinion of a shorthand, so it falls back to the platform overlay, an installed bundle's seeds or the community snapshot. The prefix itself does not go away. |
| `GET` | `/api/vocab/search` | **none** | Bundled vocabulary search; rate-limited. |
| `POST` | `/api/vocab/install` | **admin** | Installs a vocabulary into the instance. |
| `GET` | `/api/vocab/notice` | **none** | Plain-text licence page of one LOV vocabulary (catalogue data). |
| `GET` | `/api/admin/telemetry` | **admin** | Store counters across every tenant. |
| `GET` | `/api/admin/changes` | **admin** | Change-log rows carry quads from every tenant. |
| `GET` | `/api/admin/changes/status` | **admin** | Capture state, epoch, cursors, caps. |
| `PUT` | `/api/admin/changes/cursors/{name}` | **admin** | Move a consumer's bookmark. |
| `DELETE` | `/api/admin/changes/cursors/{name}` | **admin** | Drop a bookmark. |
| `GET` | `/api/admin/users` | **admin** | The account directory. |
| `GET` | `/api/admin/audit` | **admin** | The audit trail. |
| `GET` | `/api/replication/status` | **none** | This node's role and lag; a health signal, beside `/livez`. |
| `GET` | `/api/replication/manifest` | **admin** | What a follower needs to bootstrap. |
| `GET` | `/api/replication/identity` | **admin** | The identity database, whole. |
| `GET` | `/resource/{path}` | **none** | Content-negotiated dereference of a public IRI. |
| `GET` | `/.well-known/void` | **none** | Catalog of the public datasets. |
| `GET` | `/api-docs/openapi.json` | **none** | The spec is tailored to the caller: operations it may not reach are left out. |
| `GET` | `/api/docs` | **none** | Documentation pages; admin-only pages are filtered out. |
| `POST` | `/api/feedback` | **token** | Send a bug report, feature request or question to this instance's admins. Needs a write-capable token; rate-limited and capped at 20 a day per user. |
| `GET` | `/api/feedback/mine` | **token** | Your own reports, with their status and the admins' reply. |
| `GET` | `/api/admin/feedback` | **admin** | The feedback inbox; `?status=` and `?kind=` narrow it. |
| `PATCH` | `/api/admin/feedback/{id}` | **admin** | Set a report's status, reply to the reporter, or keep an internal note. |
| `DELETE` | `/api/admin/feedback/{id}` | **admin** | Delete a report. |

`tests/api_reference_auth.rs` reads this table out of the shipped Markdown and
fires an anonymous request at every row it can address, so a level stated here
and the level the router enforces cannot drift apart.

## Common API paths

- `/sparql` — global SPARQL 1.1 endpoint (GET or POST)
- `/sparql/batch` — several SPARQL updates as one transaction (POST, authenticated; see below)
- `/store` — Graph Store HTTP Protocol (GET/PUT/POST/DELETE, with `?graph=<iri>`)
- `/api/{datasets|organisations|groups}/{id}/api-services/{slug}/run` — run a saved API service
- `/resource/<path>` — content-negotiated IRI dereference
- `/.well-known/void` — DCAT 3 / VoID dataset catalog (content-negotiated RDF)
- `/api/models/{id}/versions` — list model versions
- `/api/models/{id}/latest/data` — latest published model (content-negotiated RDF)

Use the **Copy URL** buttons on dataset, organisation, and model detail pages to quickly grab the correct endpoint URL for each resource.

## The dataset of a `/sparql` query

A query's RDF dataset is the one SPARQL 1.1 defines, confined to the graphs the
caller may read:

- **The request names a dataset** with `FROM` / `FROM NAMED` in the query, or with
  the protocol parameters `default-graph-uri` / `named-graph-uri` (each repeatable;
  they take precedence over the query's own clauses, SPARQL 1.1 Protocol §2.1.4).
  The clauses keep their meaning: `FROM <a>` alone makes `<a>` the default graph
  and leaves no named graphs, `FROM NAMED <b>` alone gives an empty default graph
  and the one named graph `<b>`, and several `FROM` graphs merge into one default
  graph (a triple held in two of them counts once). A graph the caller may not read
  is dropped, as if it were empty, so the answer does not reveal whether it exists.
  An admin's dataset is used as written (admins may name any graph).
- **The request names no dataset:** the default graph is the merge of every graph
  the caller may read (every registered graph, for an admin), and those graphs are
  also the named graphs, so a plain `SELECT * { ?s ?p ?o }` sees the data, which
  this store keeps in named graphs. The service description advertises this as
  `sd:UnionDefaultGraph`.
- `?entailment=<regime>` adds the regime's entailment graph to the default graph
  (and to the named graphs when no dataset is named).

GET carries the parameters in the URL; a form-encoded POST in its body (or the URL);
a POST with an `application/sparql-query` body in the URL. A parameter that is not
an absolute IRI is a `400`.

## Batched SPARQL updates — `/sparql/batch`

`POST /sparql/batch` with a JSON body `{"updates": ["<update 1>", "<update 2>", …]}`
(at most 1000 statements) runs the statements **as one transaction**: they
execute in order, each sees the effect of the previous ones, and either every
statement is applied or none is. The graph count index is maintained once for
the whole batch, which is why a batch is several times faster than the same
statements sent one by one.

| Outcome | HTTP | Body |
|---|---|---|
| Every statement applied | 200 | `{"status": "ok", "count": N}` |
| A statement failed at execution (for example `DROP GRAPH` of a graph that does not exist, without `SILENT`) | **422** | `{"status": "rolled_back", "count": N, "error": "statement k failed: …; nothing was applied", "results": [{"index": i, "status": "rolled_back"}, …, {"index": k, "status": "error", "error": "…"}, …]}` — nothing was applied; `error` says which statement failed and why, `results` marks every other one `rolled_back`, never `ok` |
| A statement does not parse, or the caller may not write one of its graphs | 400 / 403 | error body; nothing was applied |

A rolled-back batch answers **422 Unprocessable Entity** since 0.6.0 (the
maintainer's decision of 2026-09-16); earlier builds answered 200 with the
same body minus `error`. The HTTP code now says whether the batch landed;
the body still says what went wrong.

## LDES retention — `410 Gone` on a fragment

`PUT /api/datasets/{dataset_id}/ldes` accepts an optional `retention` object
(see [ldes.md](ldes.md#retention)); the response gains `members_pruned` and
`retention`. Once a policy is declared, `GET
/api/datasets/{dataset_id}/ldes/nodes/{n}` can answer **`410 Gone`** for a
frozen fragment whose members were all removed by it — the body names the
node the stream continues at. `404` keeps its meaning (no such node, no
stream). Streams without a policy never answer 410. Full fragments also carry
`<node> ldes:immutable true`. `POST /api/ldes/sync` reports gain
`nodes_gone`, `retention_policy` and `warnings`; existing fields are unchanged.

## LDES sync — an LDES 1.0 consumer

`POST /api/ldes/sync` initialises as LDES 1.0 §3.1 says: `url` must be the
event stream, its root node, a redirect to either, or a page with exactly one
`tree:view`. Anything else (several views, none at all) is a **`502`** whose
body names §3.1; it used to crawl whatever links it found. Redirects are
followed within `OTS_REMOTE_ALLOWLIST`, `408`/`425`/`429`/`5xx` are retried
with back-off, and any other error status is a `502`. The report gains
`stream`, `root_node`, `polling_interval`, `shapes`, `nodes_not_modified`,
`nodes_skipped_immutable`, `nodes_pruned`, `retries` and
`versions_superseded`; existing fields keep their meaning, except that
`members_skipped_older` no longer counts members on immutable pages the
client did not fetch again (those are in `nodes_skipped_immutable`). See
[ldes.md](ldes.md#syncing-a-stream-into-a-dataset).

## SHACL Compact Syntax — `?dialect`, `?lenient`, `?base`, `?lossy`

`PUT /api/datasets/{dataset_id}/shapes`, `PUT /api/shacl/shape-graphs/{id}/turtle`
(both with `Content-Type: text/shaclc`) and `POST /api/shaclc/parse` parse the
**W3C SHACL Compact Syntax** (the SHACL Community Group report's grammar)
**strictly**: input the grammar does not allow is a `400` whose body names the
line, column and offending token, and nothing is stored. **Changed after 0.7:** a
bare non-XSD IRI after a path is `sh:class` (it was `sh:node`), and the old
dialect's keywords are refused. `dialect=legacy` (deprecated, accepted for one
more release, logged on every use) parses the 0.7 dialect instead, and only
with it does `lenient=true` (or `1`) keep its old meaning of ignoring
unrecognised input; `lenient` without it is a `400`. `POST /api/shaclc/parse`
also takes `base=<iri>`, the initial base IRI.

Every SHACL-C read — `GET /api/datasets/{dataset_id}/shapes?format=shaclc`,
`GET /api/shacl/shape-graphs/{id}/turtle?format=shaclc` (or
`Accept: text/shaclc`) and `POST /api/shaclc/serialize` — is lossless or a
**`422`** `{"error": …, "losses": [{"subject", "predicate", "object",
"reason"}, …]}` listing every triple the compact syntax cannot express (it used
to answer `200` with those constraints silently missing). `lossy=true` returns
the partial document with `200`, an `X-SHACLC-Losses` count header and the
losses named in a leading comment block.

## Browse scope — `dataset_id`, `dataset_ids`, `org_id`, `org_ids`

`GET /api/browse/triples`, `/api/browse/facets` and `/api/browse/resource` take
the same scope parameters:

| Parameter | Scope it contributes |
|---|---|
| `dataset_id` | one dataset's named graphs |
| `dataset_ids` | a comma-separated list of dataset ids |
| `org_id` | every dataset owned by that organisation |
| `org_ids` | a comma-separated list of organisation ids (**new**) |

The four **union**: the request is scoped to the datasets named directly *plus*
every dataset of every organisation named, deduplicated. Earlier builds resolved
them in precedence order instead, so a request carrying both `dataset_ids` and
`org_id` — what the triple browser sends whenever a user picks datasets and an
organisation — silently dropped the organisation, from the rows and from the
"terms in scope" facets alike. Each parameter used on its own behaves exactly as
before.

A scope that resolves to no dataset (an organisation with no datasets, an
unknown id, an empty list) still means "nothing in scope", never "everything";
a request with none of the four parameters is unscoped as before. Access control
is unchanged: every dataset in the union is filtered by the caller's
visibility and graph permissions, whether it was named directly or reached
through an organisation, so widening the scope never widens what a caller can
read. `versions` pins still apply per dataset, however that dataset entered the
scope.

## Change log — `/api/admin/changes`

Every write records one row per graph it touched — the net delta as
N-Quads when it fits, exact counts otherwise, or an honest `unknown` — with
a dense sequence number in commit order. Capture is **off by default**
(`OTS_CHANGE_CAPTURE=on` turns it on, and a replication leader keeps it on
regardless); with it off, `status` says so and the rows are empty. Admin-only, since rows carry quads from every tenant. See `docs/versioning.md` "Change log" for the row format,
retention and the caps.

| Method | Path | Returns |
|---|---|---|
| `GET` | `/api/admin/changes?after=<seq>&limit=<n>&graph=<iri>` | `{ "epoch", "rows", "next_after" }` — rows above `after` in commit order (`limit` 1–5000, default 500); `graph` narrows to one graph plus the store-scoped rows |
| `GET` | `/api/admin/changes/status` | capture on/off, `epoch`, `next_seq`, row counts by state, `oldest_seq` / `newest_seq`, live `cursors`, the caps and retention |
| `PUT` | `/api/admin/changes/cursors/<name>` with `{ "seq": <n> }` | `200` the cursor; `400` for a bad name (1–64 of `[A-Za-z0-9._-]`) or a `seq` beyond the log |
| `DELETE` | `/api/admin/changes/cursors/<name>` | `204`, or `404` |

A cursor is a consumer's bookmark: retention never deletes rows above the
lowest live one, and a cursor idle for `OTS_CURSOR_TTL_DAYS` (30) is dropped.

## Replication — `/api/replication/*` and a follower's `503`

See `docs/operations.md, "Replication"`. Two new routes, no change to existing ones:

| Method | Path | Returns |
|---|---|---|
| `GET` | `/api/replication/status` | This node's `role`, `mode`, `scope`, `leader_url`, `node_id`, `read_only`; on a follower also `epoch`, `applied_seq`, `leader_newest_seq`, `lag_rows`, `last_sync_at`, `last_error`, `applied_rows`, `refetched_graphs`, `resyncs`, `interval_secs`, `healthy`. Public, beside `/livez`. |
| `GET` | `/api/replication/manifest` | The leader's change-log `epoch`, `newest_seq`, `capture_enabled`, every graph (`graphs`, `null` for the default graph), `datasets` (`id`, `graphs`) and `identity_version` (SQLite's change counter of the identity database; `null` for an in-memory one). Admin only (`401` / `403`). |
| `POST` | `/api/replication/raft/vote`, `/append`, `/snapshot` | The Raft transport between cluster members (JSON, `X-Cluster-Secret`). Not user routes, and not in the OpenAPI document: `404` off a cluster, `401` without the secret, `503` while the member starts. |
| `GET` | `/api/replication/identity` | The identity database, whole, as a consistent SQLite snapshot (`application/vnd.sqlite3`). Admin only (`401` / `403`). |

`GET /api/admin/changes` takes `wait_ms` (at most 30000): when no row is
above `after`, the request is held until one lands or the wait runs out,
then answered — the long-poll a hot follower uses. On a leader with
synchronous followers (`OTS_REPLICATION_SYNC_FOLLOWERS`), the write routes —
SPARQL Update, `/sparql/batch`, Graph Store `PUT`/`POST`/`DELETE` — add
the response header `X-Replication-Ack: sync` while the required followers
keep up and `X-Replication-Ack: degraded` while a success means "durable on
the leader only"; status codes are unchanged.

On a node started with `OTS_REPLICATION_ROLE=follower`, every write —
SPARQL Update, Graph Store `PUT`/`POST`/`DELETE`, imports, data writes made
by the registry — answers `503 Service Unavailable` with the body
`read-only replica: writes go to the leader at <url>`. Reads are unchanged.
A node without the role behaves exactly as before.
