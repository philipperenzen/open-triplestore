# API Reference

The full machine-readable API specification is available as an OpenAPI 3 JSON document. You can import it into **Postman**, **Insomnia**, or any OpenAPI-compatible tooling to explore and test all available endpoints.

- **OpenAPI specification** — <a href="/api-docs/openapi.json" target="_blank" rel="noopener noreferrer">/api-docs/openapi.json</a> — machine-readable JSON, always up to date. (An interactive viewer is available at [API Reference](/api-docs).)
- **Authentication** — Most write endpoints and private resources require an `Authorization: Bearer <token>` header. Generate a token in **Settings → API Tokens** and include it with every request that needs access beyond public resources.

## Common API paths

- `/sparql` — global SPARQL 1.1 endpoint (GET or POST)
- `/store` — Graph Store HTTP Protocol (GET/PUT/POST/DELETE, with `?graph=<iri>`)
- `/api/{datasets|organisations|groups}/{id}/api-services/{slug}/run` — run a saved API service
- `/resource/<path>` — content-negotiated IRI dereference
- `/.well-known/void` — DCAT 2 / VoID dataset catalog (content-negotiated RDF)
- `/api/models/{id}/versions` — list model versions
- `/api/models/{id}/latest/data` — latest published model (content-negotiated RDF)

Use the **Copy URL** buttons on dataset, organisation, and model detail pages to quickly grab the correct endpoint URL for each resource.
