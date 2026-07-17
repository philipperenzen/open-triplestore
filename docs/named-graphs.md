# Named Graphs

A named graph is an IRI that identifies a set of RDF triples in the triplestore. It forms the low-level storage unit for datasets and provides a way to organize, query, and manage RDF data by logical context. Unlike the default (unnamed) graph, named graphs allow fine-grained control over which triples belong to which logical collection.

## Key Concepts

- **IRI identifier** — Each named graph has a unique IRI, for example `https://example.org/graphs/products` or `urn:dataset:public`. The system also provides a default unnamed graph (`null` in SPARQL) for triples that do not belong to any named graph.
- **Triple storage** — A named graph contains RDF triples: (subject, predicate, object) tuples that describe facts. The same triple can exist in multiple named graphs; the graph acts as a scoping mechanism, not as storage deduplication.
- **SPARQL querying** — Query a specific named graph using the `GRAPH <iri>` clause:

  ```sparql
  SELECT ?s WHERE {
    GRAPH <https://example.org/graphs/products> { ?s a ex:Product }
  }
  ```

  Omit `GRAPH` to search the default graph.
- **Graph Store Protocol** — Use HTTP operations at `/sparql?graph=<iri>` to manage named graphs directly:
  - `GET /sparql?graph=<iri>` — retrieve all triples in the graph
  - `PUT /sparql?graph=<iri>` — replace the entire graph
  - `POST /sparql?graph=<iri>` — merge triples into the graph
  - `DELETE /sparql?graph=<iri>` — delete the entire graph
- **Dataset ownership** — A dataset is a higher-level concept: it is owned by a user or organisation, has visibility settings (public / members / private), and may span multiple named graphs. You typically interact with datasets through the UI; named graphs are the storage primitive underneath.
- **Reasoning and inference** — When you trigger OWL reasoning, inferred triples are written to a separate named graph (e.g. `urn:entailment:rdfs`) so you can inspect and manage them separately from source data.

## Common Patterns

- **One graph per dataset** — Simple case: each dataset manages a single named graph. All triples for the dataset are stored there.
- **Multiple graphs per dataset** — Advanced case: a dataset can span multiple named graphs — for example, separate graphs for base data, inferred data, and metadata.
- **Graph versioning** — Create a new named graph for each version: `urn:datasets:products:v1`, `urn:datasets:products:v2`, and so on. Switch which graph a dataset points to for rollback.
- **Model registry graphs** — Versioned data models and vocabularies (distinguished by the entry's `kind` — `data-model` for classes, `vocabulary` for properties and SKOS concept schemes) are stored in named graphs following the pattern `{base-url}/data-model/<id>/version/<version>`. Registry metadata lives in the corresponding system graph.

See also: [Datasets](/docs/datasets), [Model & Vocabulary Versioning](/docs/models), [OWL Reasoning](/docs/reasoning).
