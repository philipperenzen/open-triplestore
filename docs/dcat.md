# DCAT 2 Catalog Guide

> **See also:** [**Linked Data Modelling Styleguide** §7](linked-data-modelling-styleguide.md#7-dataset-catalogue-and-organisation-metadata-dcat--void--adms--org) — the canonical conventions for describing datasets, organisations and services in DCAT/VoID/ADMS/ORG. This guide is the endpoint reference; the styleguide is the modelling standard.

The triplestore automatically generates a full [W3C DCAT 2](https://www.w3.org/TR/vocab-dcat-2/) catalog from its dataset registry and store statistics. The catalog is served at `/.well-known/void` and is content-negotiated.

---

## What is included

| Section | Contents |
|---|---|
| `dcat:Catalog` | Title, homepage, language, links to all datasets |
| Aggregate `void:Dataset` | Total triple count, distinct subjects/objects/predicates, named graph count |
| Per-dataset `dcat:Dataset` | Title, description, issued/modified dates, access rights, creator/publisher |
| `dcat:Distribution` | SPARQL endpoint distribution + Graph Store HTTP Protocol distribution |
| VoID statistics | Per-dataset `void:triples` from actual SPARQL COUNT queries |
| `dct:conformsTo` | Links to the dataset's SHACL shapes graph when configured |
| `dct:conformsTo` | Links to the ontology version the dataset's instance data conforms to |
| Organization metadata | `foaf:Organization` + `org:FormalOrganization` for organisation-owned datasets |
| `sd:Service` | SPARQL service description |

---

## Accessing the Catalog

```bash
# Turtle (default)
curl http://localhost:7878/.well-known/void

# JSON-LD
curl -H 'Accept: application/ld+json' http://localhost:7878/.well-known/void

# N-Triples
curl -H 'Accept: application/n-triples' http://localhost:7878/.well-known/void

# RDF/XML
curl -H 'Accept: application/rdf+xml' http://localhost:7878/.well-known/void

# Query param override (for browser links)
curl 'http://localhost:7878/.well-known/void?format=jsonld'
curl 'http://localhost:7878/.well-known/void?format=turtle'
curl 'http://localhost:7878/.well-known/void?format=ntriples'
curl 'http://localhost:7878/.well-known/void?format=rdfxml'
```

---

## Example Output (Turtle)

```turtle
@prefix dcat:   <http://www.w3.org/ns/dcat#> .
@prefix dct:    <http://purl.org/dc/terms/> .
@prefix void:   <http://rdfs.org/ns/void#> .
@prefix foaf:   <http://xmlns.com/foaf/0.1/> .
@prefix org:    <http://www.w3.org/ns/org#> .
@prefix sd:     <http://www.w3.org/ns/sparql-service-description#> .
@prefix xsd:    <http://www.w3.org/2001/XMLSchema#> .

# ── Catalog ──────────────────────────────────────────────────────────────────

<http://localhost:7878/catalog>
    a dcat:Catalog ;
    dct:title "Open Triplestore Catalog" ;
    dct:language <http://id.loc.gov/vocabulary/iso639-1/en> ;
    foaf:homepage <http://localhost:7878/> ;
    dcat:dataset <http://localhost:7878/dataset> ,
                 <http://localhost:7878/dataset/abc123> .

# ── Aggregate store ───────────────────────────────────────────────────────────

<http://localhost:7878/dataset>
    a void:Dataset, dcat:Dataset ;
    dct:title "Open Triplestore" ;
    void:sparqlEndpoint <http://localhost:7878/sparql> ;
    void:triples 150000 ;
    void:distinctSubjects 42300 ;
    void:distinctObjects 98100 ;
    void:properties 87 ;
    void:documents 12 ;
    dcat:distribution [
        a dcat:Distribution ;
        dcat:accessURL <http://localhost:7878/sparql> ;
        dct:title "SPARQL Endpoint"
    ] ;
    dcat:landingPage <http://localhost:7878/> .

# ── Per-dataset entry ─────────────────────────────────────────────────────────

<http://localhost:7878/dataset/abc123>
    a dcat:Dataset, void:Dataset ;
    dct:title "My Dataset" ;
    dct:description "A sample dataset." ;
    dct:issued "2025-01-15T10:00:00"^^xsd:dateTime ;
    dct:modified "2025-03-20T14:30:00"^^xsd:dateTime ;
    dct:accessRights <http://publications.europa.eu/resource/authority/access-right/PUBLIC> ;
    dct:publisher <http://localhost:7878/org/org-001> ;
    dct:conformsTo <urn:dataset:abc123:shapes> ;
    dct:conformsTo <http://localhost:7878/ontology/publication-model/version/1.0.0> ;
    void:triples 5200 ;
    dcat:distribution [ a dcat:Distribution ; dcat:accessURL <http://localhost:7878/sparql> ] ;
    dcat:distribution [ a dcat:Distribution ; dcat:accessURL <http://localhost:7878/store> ] ;
    dcat:landingPage <http://localhost:7878/> .

# ── Organization ──────────────────────────────────────────────────────────────

<http://localhost:7878/org/org-001>
    a foaf:Organization, org:FormalOrganization ;
    foaf:name "Acme Corp" .

# ── SPARQL service ────────────────────────────────────────────────────────────

<http://localhost:7878/sparql>
    a sd:Service ;
    sd:endpoint <http://localhost:7878/sparql> ;
    sd:supportedLanguage sd:SPARQL11Query, sd:SPARQL11Update .
```

---

## Access Rights Mapping

Dataset visibility maps to EU Publications Office access rights URIs:

| Visibility | `dct:accessRights` |
|---|---|
| `public` | `http://publications.europa.eu/resource/authority/access-right/PUBLIC` |
| `members` | `http://publications.europa.eu/resource/authority/access-right/RESTRICTED` |
| `private` | `http://publications.europa.eu/resource/authority/access-right/NON_PUBLIC` |

---

## VoID Statistics

VoID statistics (`void:triples`, `void:distinctSubjects`, `void:distinctObjects`, `void:properties`) are computed live via SPARQL COUNT queries when the catalog is requested. For large stores this may add a few hundred milliseconds; the results are not cached between requests.

The aggregate `void:triples` at the root dataset level reflects `SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }` across all graphs. Per-dataset counts scope the query to `GRAPH <iri> { ?s ?p ?o }` for each registered graph.

---

## Linked Data and IRIs

The catalog uses the `base_url` configured at startup (default `http://localhost:7878`) as the IRI namespace for:

- `<{base}/catalog>` — the catalog itself
- `<{base}/dataset>` — the aggregate dataset
- `<{base}/dataset/{id}>` — individual datasets
- `<{base}/org/{id}>` — organisations
- `<{base}/user/{id}>` — individual user creators
- `<{base}/sparql>` — SPARQL service endpoint

Set the base URL in production:

```bash
./open-triplestore --base-url https://triplestore.example.com
# or
BASE_URL=https://triplestore.example.com ./open-triplestore
```

---

## IRI Dereference

Any resource IRI in the `{base}/resource/` namespace can be dereferenced:

```bash
# Returns RDF (Turtle, JSON-LD, etc.) for clients that accept it
curl -H 'Accept: text/turtle' http://localhost:7878/resource/alice

# Returns 303 redirect to SPA page for browsers
curl -H 'Accept: text/html' http://localhost:7878/resource/alice
# → 303 See Other: /resource?iri=http%3A%2F%2Flocalhost%3A7878%2Fresource%2Falice

# ?format= override
curl 'http://localhost:7878/resource/alice?format=jsonld'
```

Responses include `Link` headers pointing to the SPARQL endpoint and VoID catalog.
