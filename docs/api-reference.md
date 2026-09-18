# API Reference

The full machine-readable API specification is available as an OpenAPI 3 JSON document. You can import it into **Postman**, **Insomnia**, or any OpenAPI-compatible tooling to explore and test all available endpoints.

- **OpenAPI specification** — <a href="/api-docs/openapi.json" target="_blank" rel="noopener noreferrer">/api-docs/openapi.json</a> — machine-readable JSON, always up to date. (An interactive viewer is available at [API Reference](/api-docs).)
- **Authentication** — Most write endpoints and private resources require an `Authorization: Bearer <token>` header. Generate a token in **Settings → API Tokens** and include it with every request that needs access beyond public resources.

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
