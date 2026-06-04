# Frequently Asked Questions

## What is the difference between a named graph and a dataset?

A dataset is a user-facing container with metadata, visibility settings, and optional SHACL constraints. Under the hood it maps to one or more named graphs in the triplestore. A named graph is simply an IRI that identifies a set of triples — the low-level storage unit. See [Named Graphs](/docs/named-graphs) and [Datasets](/docs/datasets).

## Where is the SPARQL endpoint?

The global SPARQL 1.1 query endpoint is at `/sparql` (GET or POST). SPARQL Update is accepted at `/sparql` with a POST of `Content-Type: application/sparql-update`. Datasets, organisations, and groups can additionally publish saved queries as REST endpoints at `/api/{scope}/{id}/api-services/{slug}/run` — see [API Services & AI Queries](/docs/api-services).

## How are model and vocabulary versions stored?

Each version is stored in a dedicated named graph. Data model versions use IRIs of the form `urn:data-model:{id}:v{version}`; vocabulary versions use `urn:vocabulary:{id}:v{version}`. Registry metadata (title, namespace, status, version list) lives in a corresponding system graph. Use `/api/data-models/{id}/latest/data` or `/api/vocabularies/{id}/latest/data` to retrieve the most recently published version. See [Model & Vocabulary Versioning](/docs/models).

## Does the triplestore support RDF-star?

Yes. Both the storage engine and SPARQL processor support RDF-star (quoted triples). Use the `<< s p o >>` syntax in Turtle 1.2 or SPARQL 1.2 queries. Standard RDF 1.1 is always supported alongside it.

## How does content negotiation work?

Send an `Accept` header with your preferred MIME type. SPARQL results: `application/sparql-results+json`, `application/sparql-results+xml`, `text/csv`. RDF graphs: `text/turtle`, `application/n-triples`, `application/n-quads`, `application/trig`, `application/rdf+xml`, `application/ld+json`. The default is Turtle for graphs and JSON for SPARQL results. See [Supported RDF Formats](/docs/formats).
