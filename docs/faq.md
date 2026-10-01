# Frequently Asked Questions

## What is the difference between a named graph and a dataset?

A dataset is a user-facing container with metadata, visibility settings, and optional SHACL constraints. Under the hood it maps to one or more named graphs in the triplestore. A named graph is simply an IRI that identifies a set of triples — the low-level storage unit. See [Named Graphs](/docs/named-graphs) and [Datasets](/docs/datasets).

## Where is the SPARQL endpoint?

The global SPARQL 1.1 query endpoint is at `/sparql` (GET or POST). SPARQL Update is accepted at `/sparql` with a POST of `Content-Type: application/sparql-update`. Datasets, organisations, and groups can additionally publish saved queries as REST endpoints at `/api/{scope}/{id}/api-services/{slug}/run` — see [API Services & AI Queries](/docs/api-services).

## How are model and vocabulary versions stored?

Each version is stored in a dedicated named graph following the pattern `{base-url}/data-model/{id}/version/{version}`. Data models and vocabularies share one Model Registry, distinguished by the entry's `kind` (`data-model` for classes, `vocabulary` for properties and SKOS concept schemes). Registry metadata (title, namespace, kind, status, version list) lives in a corresponding system graph. Use `/api/models/{id}/latest/data` to retrieve the most recently published version. See [Model & Vocabulary Versioning](/docs/models).

## Does the triplestore support RDF-star?

It supports RDF 1.2, which replaces the RDF-star Community Group model. The store and the SPARQL engine hold *triple terms*, written `<<( s p o )>>`, which may appear only as an object. To annotate a statement, use `<< s p o >>` in Turtle 1.2 or SPARQL 1.2: it is shorthand for a reifier (a blank node that `rdf:reifies` the triple term) and does not assert the statement. `s p o {| … |}` asserts it and annotates it in one go. RDF-star data that uses a quoted triple as a subject needs converting. Standard RDF 1.1 is always supported alongside it. See [SPARQL 1.2](/docs/sparql-12).

## How does content negotiation work?

Send an `Accept` header with your preferred MIME type. SPARQL results: `application/sparql-results+json`, `application/sparql-results+xml`, `text/csv`. RDF graphs: `text/turtle`, `application/n-triples`, `application/n-quads`, `application/trig`, `application/rdf+xml`, `application/ld+json`. The default is Turtle for graphs and JSON for SPARQL results. See [Supported RDF Formats](/docs/formats).
