# Platform Overview

A self-hosted, feature-complete RDF triplestore with a full web interface. Built on the **Oxigraph** storage engine, it supports the core W3C Semantic Web stack — SPARQL, OWL, SHACL — alongside modern extensions including GeoSPARQL, RDF-star, and full-text search.

## Capabilities

- **Datasets** — Named RDF graphs with visibility controls (public / members / private), SPARQL sub-endpoints, asset management, SHACL on-write validation, and LDP container support. See [Datasets](/docs/datasets).
- **Organisations & Groups** — Team-based access control. Datasets owned by an organisation are visible to all its members; **groups** sub-divide an organisation for finer-grained sharing (and have their own API Services). Admins manage membership; members share a common query namespace. See [Organisations](/docs/organisations).
- **Model & Vocabulary Registries** — Version-controlled storage for data models and vocabularies with a draft → published lifecycle. Upload, compare diffs, and publish RDF/OWL models and SKOS vocabularies in any supported format. See [Model & Vocabulary Versioning](/docs/models).
- **SHACL Validation** — Validate graphs against SHACL Core and Advanced constraints. Attach a shapes graph to a dataset to enforce constraints on every write automatically. See [SHACL Validation](/docs/shacl).
- **GeoSPARQL** — OGC GeoSPARQL 1.1 via the GEOS library. Store WKT/GML geometry literals and run spatial relation queries (intersects, within, distance, buffer…). See [GeoSPARQL](/docs/geosparql).
- **OWL Reasoning** — On-demand materialised inference with RDFS and OWL 2 RL/EL/QL/DL profiles. Inferred triples are written to a separate named graph for inspection. See [OWL Reasoning](/docs/reasoning).
- **Full-text Search** — Tantivy-backed index over all string literals. Accessible via `Cmd/Ctrl + K` in the UI or via the `ft:search()` SPARQL custom function. See [Full-text Search](/docs/full-text-search).
- **API Services** — Save a parameterised SPARQL query and expose it as a versioned REST API with an auto-generated OpenAPI spec. Dataset-scoped services are auto-tested when a version is committed, and an LLM can repair a query that breaks against new data. Available for datasets, organisations and groups. See [API Services & AI Queries](/docs/api-services).
- **Dataset Versioning** — Immutable SemVer snapshots of a dataset, time-travel queries pinned with `?version=`, branches, and share-links for unauthenticated read. See [Versioning](/docs/versioning).
- **Asset Management** — Store binary assets (images, 3D/CAD, point clouds, geo files, PDFs, spreadsheets, audio/video) alongside a dataset and get typed RDF metadata extracted automatically — dimensions, SHA-256 checksums, geo bounding boxes, page counts — all dereferenceable as linked data. See [Datasets](/docs/datasets).
- **AI Assistance** — Natural-language → SPARQL plus **Spark**, a grounded chat assistant with an interactive answer canvas, served by a pluggable LLM gateway; every accept / edit / reject is fed back to improve the model. Hidden gracefully when no gateway is configured. See [Spark Chat Assistant](/docs/spark) and [API Services & AI Queries](/docs/api-services).
- **Authentication** — JWT sessions for browser users, long-lived bearer API tokens for programmatic access, plus OAuth 2.0 / OIDC provider integration. See [Authentication & API Tokens](/docs/auth).

## Where to start

- New to the data model? Read [Named Graphs](/docs/named-graphs) and [Linked Data Modelling](/docs/modelling).
- Loading data? See [Supported RDF Formats](/docs/formats) and [Import Auto-Detection](/docs/import).
- Integrating programmatically? See the [API Reference](/docs/api-reference) and [API Services & AI Queries](/docs/api-services).
- Curious what's implemented? See [Supported Standards](/docs/standards).
- Want a guided tour? Every fresh install seeds a public **Open Triplestore** demo organisation — one dataset per standard, each with runnable API Services — so you can explore SPARQL, SHACL, GeoSPARQL and reasoning immediately. (A full multi-app demo walkthrough lives in `docs/demo-guide/` at the workspace root.)
