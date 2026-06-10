# API Services & AI Queries

Turn a SPARQL query into a stable, parameterised REST endpoint. API services are scoped to a dataset, organisation, or group, and every scope gets an auto-generated OpenAPI document so the endpoints drop straight into Postman, Swagger UI, or generated client code. Open them from any dataset, organisation, or group via its **API Services** tab.

## Saved queries as endpoints

- **Parameters** — Bind named parameters in the query and declare each one's type: `iri`, `string`, `integer`, `decimal`, `boolean`, `date`, or `dateTime`. Callers pass them as query-string arguments.
- **Run** — Execute a service at `/api/{datasets|organisations|groups}/{id}/api-services/{slug}/run`, which returns standard SPARQL-results JSON. Public services run without authentication.
- **OpenAPI** — Every scope publishes a spec at `/api/{datasets|organisations|groups}/{id}/openapi.json` describing all of its services and parameters.
- **Revisions** — Each save creates a revision, so you can track how a service's query evolved over time.
- **Version pinning** — A run can target a specific dataset version (`?version=…`); the version actually served is returned in the `x-ots-dataset-version` response header. By default the published version is used.

## Tests & monitoring

Attach test cases to a service so it is exercised after edits and on a schedule. Failing tests are surfaced for review and can be acknowledged once triaged, giving a clear signal of which published endpoints are currently healthy.

## AI query assistance

When an LLM endpoint is configured, two assistants appear in the query editor and the API-services authoring form — and [Spark](/docs/spark), the platform-wide chat assistant with its interactive answer canvas, comes online at `/chat`. **Bring your own LLM:** set `LLM_GATEWAY_URL` to any OpenAI-compatible chat endpoint (OpenAI, OpenRouter, Azure OpenAI, Ollama, LM Studio, vLLM, llama.cpp, or a self-hosted gateway), choose the model with `LLM_MODEL`, and set `LLM_API_KEY` if the endpoint needs a bearer token. Per-task overrides `LLM_SPARQL_MODEL` / `LLM_SHACL_MODEL` fall back to `LLM_MODEL`.

- **Natural language → SPARQL** — Describe what you want in plain language and get a generated query, optionally grounded with prefix/schema hints. Backed by `POST /api/llm/sparql`.
- **Query repair** — When a saved query fails, request an AI-suggested fix to review and save as a new revision.
- **Feedback loop** — Approving or editing a generated query sends a training signal back to the gateway. Availability is probed at `GET /api/llm/health`; if no gateway is configured, these assistants stay hidden.
