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

See also: [Import Auto-Detection](/docs/import) and [Supported Standards](/docs/standards).
