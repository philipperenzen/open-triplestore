# Supported RDF Formats

All import (upload, SPARQL-over-HTTP, Graph Store) and export (download, content negotiation) endpoints accept and produce any of these formats. Format is auto-detected from the file extension or the `Content-Type` / `Accept` HTTP header.

| Extension | Format | MIME Type | Notes |
|---|---|---|---|
| `.ttl` | Turtle | `text/turtle` | Recommended for hand-authoring |
| `.nt` | N-Triples | `application/n-triples` | Fastest for bulk import |
| `.nq` | N-Quads | `application/n-quads` | Preserves named graph info |
| `.trig` | TriG | `application/trig` | Multi-graph Turtle |
| `.rdf` | RDF/XML | `application/rdf+xml` | Broad tool compatibility |
| `.owl` | OWL/XML | `application/rdf+xml` | OWL Protégé exports |
| `.jsonld` | JSON-LD | `application/ld+json` | JSON-native integration |

SPARQL results are returned as `application/sparql-results+json`, `application/sparql-results+xml`, or `text/csv` based on the `Accept` header. JSON is the default.

## JSON-LD remote contexts

A JSON-LD document may name its `@context` by IRI instead of spelling it out. Every JSON-LD parse in the server — uploads, imports, the Graph Store, LDP, seed bundles, LDES pages — resolves such a context in this order:

1. **Bundled contexts**, served offline whatever the configuration: the W3C contexts of ActivityStreams 2.0 (`https://www.w3.org/ns/activitystreams`), CSVW (`http://www.w3.org/ns/csvw`), LDP (`https://www.w3.org/ns/ldp.jsonld`) and ODRL 2.2 (`https://www.w3.org/ns/odrl.jsonld`), over `http` or `https`.
2. **Fetched contexts**, only from URLs covered by `OTS_REMOTE_ALLOWLIST` — the operator allowlist SPARQL federation and LDES sync use. Unset, nothing is fetched and a document naming any other remote context is refused with an error that names the URL. A fetch follows at most 5 redirects, each of which must be allowlisted too, follows a `Link: <…>; rel="alternate"; type="application/ld+json"` header from a non-JSON page (the way `https://schema.org/` publishes its context), times out after `OTS_REMOTE_TIMEOUT_SECS`, and stops reading at `OTS_JSONLD_CONTEXT_MAX_BYTES` (default 1 MiB).
3. A fetched context is cached in memory for an hour (128 contexts at most).

The Schema.org context is not bundled (it is licensed CC BY-SA 3.0); to use documents that name it, allowlist `https://schema.org/`.

## JSON-LD direction and downloads

- A string with `@language` and `@direction` is stored as an RDF 1.2 directional language-tagged string (`"…"@ar--rtl`) and downloaded back with both keys. A JSON-LD 1.1 processor without its `rdfDirection` option would drop the direction.
- A JSON-LD download writes every quad of the graph as it is, so it reads back as the same graph as the Turtle or N-Quads download: lists stay `rdf:first` / `rdf:rest` nodes rather than `@list`, and an `rdf:JSON` literal stays a typed literal, even one whose text is not valid JSON.

See also: [Import Auto-Detection](/docs/import) and [Supported Standards](/docs/standards).
