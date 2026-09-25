# API Reference

The full machine-readable API specification is available as an OpenAPI 3 JSON document. You can import it into **Postman**, **Insomnia**, or any OpenAPI-compatible tooling to explore and test all available endpoints.

- **OpenAPI specification** — <a href="/api-docs/openapi.json" target="_blank" rel="noopener noreferrer">/api-docs/openapi.json</a> — machine-readable JSON, always up to date. (An interactive viewer is available at [API Reference](/api-docs).)
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
| `GET` | `/sparql` | **none** | Query over the graphs the caller may read — anonymously, the public ones. |
| `POST` | `/sparql` | **none** | The same query endpoint in the protocol's POST form. A body sent as `application/sparql-update` is a write and needs a **token**. |
| `GET` | `/store?graph={graph_iri}` | **none** | Graph Store read of one named graph, scoped exactly like the query endpoint: a public graph answers anonymously, a private one `401`/`403`. Turtle and TriG carry an `@prefix` header built from the prefix registry for the namespaces the graph actually uses; the line-based formats write every IRI in full. |
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
| `GET` | `/api/datasets/{dataset_id}/shapes` | **token** | The dataset's shapes graph; a private one only for those who may read it. |
| `PUT` | `/api/datasets/{dataset_id}/shapes` | **token** | Replace the shapes graph (`text/shaclc` or RDF). |
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
| `POST` | `/api/shaclc/parse` | **token** | SHACLC → SHACL. Needs a token since 0.6.x: it spends the instance's CPU on caller-supplied text. |
| `POST` | `/api/shaclc/serialize` | **token** | SHACL → SHACLC of a graph named by the caller. Needs a token since 0.6.x, and the caller must be allowed to read that graph: it reads whatever IRI it is given out of the store, so it was previously a way for anyone to read any graph. A graph you may not read answers `403`, whether or not it exists. |
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

`tests/api_reference_auth.rs` reads this table out of the shipped Markdown and
fires an anonymous request at every row it can address, so a level stated here
and the level the router enforces cannot drift apart.

## Common API paths

- `/sparql` — global SPARQL 1.1 endpoint (GET or POST)
- `/sparql/batch` — several SPARQL updates as one transaction (POST, authenticated; see below)
- `/store` — Graph Store HTTP Protocol (GET/PUT/POST/DELETE, with `?graph=<iri>`)
- `/api/{datasets|organisations|groups}/{id}/api-services/{slug}/run` — run a saved API service
- `/resource/<path>` — content-negotiated IRI dereference
- `/.well-known/void` — DCAT 2 / VoID dataset catalog (content-negotiated RDF)
- `/api/models/{id}/versions` — list model versions
- `/api/models/{id}/latest/data` — latest published model (content-negotiated RDF)

Use the **Copy URL** buttons on dataset, organisation, and model detail pages to quickly grab the correct endpoint URL for each resource.

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

## SHACL Compact Syntax — `?lenient`

`PUT /api/datasets/{dataset_id}/shapes` (with `Content-Type: text/shaclc`) and
`POST /api/shaclc/parse` parse SHACLC **strictly**: input the grammar does not
recognise is a `400` whose body names the line, column and offending text, and
nothing is stored. The optional query parameter `lenient=true` (or `1`) restores
the previous behaviour, in which unrecognised input is ignored and whatever parsed
is kept. Status codes and response bodies are otherwise unchanged.

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
| `POST` | `/api/replication/raft/vote`, `/append`, `/snapshot` | The Raft transport between cluster members (JSON, `X-Cluster-Secret`). Not user routes: `404` off a cluster, `401` without the secret, `503` while the member starts. |
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
