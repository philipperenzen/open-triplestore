# Spark Chat Assistant

**Spark** is the platform's grounded chat assistant, open at `/chat` (the ✦ **Spark** entry in the navigation). Ask about the linked data on the platform in plain language — which datasets exist and what they cover, which API service can answer a question, what the graphs actually contain, or how RDF, SPARQL and SHACL work — in whatever language you prefer. Answers come back as an **interactive canvas**: runnable queries, one-click API calls, charts, maps, entity cards and tables, not just prose.

Typical questions:

- *How many datasets are there, and what topics do they cover?*
- *Is there an API service I can call to answer a question about cities, and how do I call it?*
- *Chart the number of triples per named graph as a bar chart.*

Spark requires an LLM endpoint (see [Configuration](#configuration)); when no gateway is reachable the chat reports itself offline.

## How answers are grounded

Spark does not answer about your data from model memory. Each turn:

1. **Platform context** — The server hands the model a snapshot of what *you* may see: your accessible datasets (name, visibility, description, DCAT themes and keywords), the API services runnable against them (with their parameters), and the named graphs in your read scope. The model is instructed never to claim something exists that is not in this list. Guests see public data only — sign in to ask about your own datasets.
2. **Scoped SPARQL retrieval** — When an answer needs the actual triples (counts, specific values, relationships, geometries), the model replies with a `SPARQL:` directive instead of an answer. The server runs that query read-only through the exact same `scope_query_to_authorized` boundary as a user-typed query, so the assistant can never read a graph you are not authorized to see. Up to 50 result rows are fed back (long cells truncated).
3. **Bounded iteration** — The model may use up to **three query rounds per turn**. After each round the rows — or the error message, so it can self-repair a broken query — go back into the conversation, letting it count first and then fetch geometry for a map, for example. After the last round it must answer with what it has.

The full retrieval trail is shown with each answer: every query of the turn (including failed attempts), its result table, and an **Open in SPARQL workspace** action so you can verify and refine the query yourself. Grounding constrains what Spark can see, not what it concludes — verify important results.

## The answer canvas

Answers are markdown plus a small set of fenced widget blocks that render as live, interactive elements. Spark emits these itself; the grammar below shows what each block contains. A malformed block degrades to plain code, so the answer stays readable.

### `sparql` — runnable query card

A query card with **Run**, **Copy** and **Open in SPARQL workspace** actions. Run executes the query through the normal scoped SPARQL endpoint and renders the results in place.

````markdown
```sparql
SELECT ?g (COUNT(*) AS ?triples)
WHERE { GRAPH ?g { ?s ?p ?o } }
GROUP BY ?g ORDER BY DESC(?triples)
```
````

### `api` — one-click API call

The first line is `GET <path>`. For API-service run URLs the card loads the service definition, offers editable parameters, and renders the negotiated result in place (SPARQL results table, CSV, RDF, JSON, …). Any other same-origin `GET /api/…` path runs as a plain authenticated read. Inline code spans like `GET /api/...` in the prose become clickable run chips too.

````markdown
```api
GET /api/datasets/<dataset-id>/api-services/<slug>/run?city=Nijmegen
```
````

### `chart` — bar, line or pie chart

A JSON spec with a `type` of `bar`, `line` or `pie`, an optional `title` / `xLabel` / `yLabel`, and either a flat `data` array or multiple named `series` (`{"type": "line", "series": [{"name": "2024", "data": […]}]}`).

````markdown
```chart
{"type": "bar", "title": "Triples per graph", "yLabel": "triples",
 "data": [{"label": "buildings", "value": 120953}, {"label": "roads", "value": 74210}]}
```
````

### `map` — WKT feature map

A JSON `features` array rendered as an interactive map. Each feature carries a `wkt` geometry — WGS84, longitude before latitude — plus an optional `label` and `iri` linking the marker back to the resource.

````markdown
```map
{"features": [{"label": "Waalbrug", "wkt": "POINT(5.8645 51.8519)",
               "iri": "http://example.org/id/waalbrug"}]}
```
````

### `card` — entity info card

An info card for a single entity — ideal for "tell me about X" answers: a `title`, optional `subtitle`, `iri` and `image`, and a list of label–value `facts`.

````markdown
```card
{"title": "Waalbrug", "subtitle": "Arch bridge across the Waal in Nijmegen",
 "iri": "http://example.org/id/waalbrug",
 "facts": [{"label": "Type", "value": "Bridge"}, {"label": "Opened", "value": "1936"}]}
```
````

### `csv` — table with download

CSV text (the first record is the header) rendered as a table preview with a download button.

````markdown
```csv
dataset,triples
Buildings,120953
Roads,74210
```
````

Other fenced languages (` ```turtle `, ` ```json `, ` ```xml `, …) render as syntax-highlighted code rather than widgets, and small markdown tables render natively. Widget parsing is hard-capped (chart points, map features, CSV rows), so a confused answer cannot freeze the browser tab.

## Configuration

Spark uses the same bring-your-own-LLM gateway as the platform's other AI features (see [API Services & AI Queries](/docs/api-services)):

| Variable | Purpose |
| --- | --- |
| `LLM_GATEWAY_URL` | Base URL of any OpenAI-compatible `/v1/chat/completions` endpoint — OpenAI, OpenRouter, Azure OpenAI, Ollama, LM Studio, vLLM, llama.cpp, or a self-hosted gateway. Defaults to `http://127.0.0.1:8000`. |
| `LLM_MODEL` | Model name sent on every completion (an OpenAI model id, an Ollama tag, a vLLM-served name, …). |
| `LLM_API_KEY` | Optional bearer token for the endpoint. Required by hosted APIs; leave unset for local servers. |

Availability is probed at `GET /api/llm/health`; the chat itself is served by `POST /api/llm/chat`. When the gateway is unreachable, the chat page shows an offline banner and the platform's other AI assistants stay hidden.

## Privacy & scope

- **The model never touches the store.** It sees only the per-user platform context and the capped result rows of queries the server executed under your own read scope. Chat-issued queries are read-only and re-scoped server-side exactly like a hand-typed query.
- **Conversations go to your gateway.** The chat messages, the platform context (dataset names, descriptions, topics, API-service names, graph IRIs) and query result rows are sent to the endpoint configured in `LLM_GATEWAY_URL`. Point it at a provider you trust — or keep everything on-premises with a local server such as Ollama.
- **Chat content can only trigger reads.** The only thing runnable from an answer is a **same-origin `GET /api/…`** call, executed by your own browser session under your own permissions. Other methods and other hosts are never runnable, so an answer cannot mutate data or call out elsewhere.
- **Feedback is per-message and explicit.** The **Helpful?** buttons send that turn (question, answer, last query) to the gateway's training track — the same feedback loop as the SPARQL editor.
- **Nothing is stored server-side.** The conversation lives in the page; **Clear chat** or navigating away discards it.
