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
| A statement failed at execution (for example `DROP GRAPH` of a graph that does not exist, without `SILENT`) | 200 | `{"status": "rolled_back", "count": N, "results": [{"index": i, "status": "rolled_back"}, …, {"index": k, "status": "error", "error": "…"}, …]}` — nothing was applied; `results` names the failing statement, every other one is reported as `rolled_back`, never `ok` |
| A statement does not parse, or the caller may not write one of its graphs | 400 / 403 | error body; nothing was applied |

The 200 on a rolled-back batch keeps the response shape of the earlier
per-statement report; check `status`, not the HTTP code, to know whether the
batch landed.
