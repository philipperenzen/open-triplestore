# Full-text Search

A **Tantivy**-backed full-text index is maintained automatically over all string literals (`xsd:string` and plain literals) in the store. It updates incrementally on every import or SPARQL Update.

## UI search (Cmd / Ctrl + K)

Opens the global search overlay. Type any keyword to find matching resources. Results show the subject IRI, a preview of matching text, and a link to the resource and its containing graph.

## SPARQL custom function

Use `ft:search()` to integrate full-text results directly into SPARQL graph patterns:

```sparql
PREFIX ft: <tag:open-triplestore,2024:ft:>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>

SELECT ?s ?label ?score WHERE {
  (?s ?score) ft:search("semantic web") .
  OPTIONAL { ?s rdfs:label ?label }
}
ORDER BY DESC(?score)
LIMIT 20
```

The index is automatically rebuilt on server startup if it is missing or stale. An admin can trigger a manual reindex via `POST /api/text-search/reindex`.
