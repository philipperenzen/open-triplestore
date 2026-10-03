# DCAT 3 Catalogue Guide

> **See also:** [**Linked Data Modelling Styleguide** §7](linked-data-modelling-styleguide.md#7-dataset-catalogue-and-organisation-metadata-dcat--void--adms--org) — the canonical conventions for describing datasets, organisations and services in DCAT/VoID/ADMS/ORG. This guide is the endpoint reference; the styleguide is the modelling standard.

The triplestore generates a [W3C DCAT 3](https://www.w3.org/TR/vocab-dcat-3/) catalogue from its dataset registry and store statistics, optionally under the [DCAT-AP 3.0.1](https://semiceu.github.io/DCAT-AP/releases/3.0.1/) or [DCAT-AP-NL 3](https://geonovum.github.io/DCAT-AP-NL30/) application profile. The catalogue is served at `/.well-known/void` and is content-negotiated. The model registry has a catalogue of its own at `/api/catalog`.

---

## What is included

| Section | Contents |
|---|---|
| `dcat:Catalog` | Title, description, publisher, contact point, language, licence, homepage, theme taxonomy, issued/modified, its datasets and data services |
| Aggregate `void:Dataset` | Total triple count, distinct subjects/objects/predicates, named graph count — over the graphs the caller may read |
| Per-dataset `dcat:Dataset` | Title, description, issued/modified, access rights, publisher and creator, contact point, licence, themes, keywords, spatial and temporal coverage, update frequency, ADMS status, conformance, provenance, `void:subset` per graph |
| Versions | Each released (published or deprecated) version as a DCAT 3 §11 `dcat:Dataset` — `dcat:hasVersion`, `dcat:hasCurrentVersion`, `dcat:isVersionOf`, `dcat:version`, `dcat:previousVersion`, `dct:issued`, `adms:versionNotes`, a TriG download |
| `dcat:Distribution` | SPARQL endpoint, Graph Store HTTP Protocol, one Turtle download per graph, LDES when published, OGC API – Features / 3D Tiles / viewer feed when there is geometry |
| `dcat:DataService` | The SPARQL endpoint (and the OGC API when a dataset has geometry): `dcat:servesDataset`, `dcat:endpointURL`, `dcat:endpointDescription` (the SPARQL service description), publisher, contact point, access rights, the served datasets' themes |
| `dcat:CatalogRecord` | Under `dcat-ap` / `dcat-ap-nl`: one record per dataset with `foaf:primaryTopic`, `dct:issued`/`modified`, `dct:conformsTo` (the profile) and `dct:language` |
| `dct:conformsTo` | The dataset's SHACL shapes graph when configured; the model version its instance data conforms to |
| Agents | `foaf:Agent` publishers with `foaf:name`: organisations (`foaf:Organization` + ORG types), users (`foaf:Person`, ADMS publisher type *Private Individual(s)*), groups |
| `sd:Service` | The SPARQL service description |

`dcat:DatasetSeries` is not emitted: the product has no series concept, and a dataset's versions are DCAT 3 versions, not members of a series.

### Range typing

Every object is typed with the class its property's range names, as DCAT 3 declares and the DCAT-AP 3.0.1 shapes check: a licence is a `dct:LicenseDocument`, a language a `dct:LinguisticSystem`, access rights a `dct:RightsStatement`, a conformance target a `dct:Standard`, a media type (an IANA IRI) a `dct:MediaType`, a file type a `dct:MediaTypeOrExtent`, a homepage or landing page a `foaf:Document`, a spatial coverage a `dct:Location`, a temporal coverage a `dct:PeriodOfTime`, an update frequency a `dct:Frequency`, a commit a `prov:Activity`. Themes, statuses and agent types are `skos:Concept`s with the `skos:prefLabel` of their vocabulary (the EU authority tables and ADMS); the theme taxonomy is a `skos:ConceptScheme` with a title. A catalogue therefore validates without the EU vocabularies loaded as background knowledge.

### Temporal coverage and update frequency

A dataset's `temporal_start` / `temporal_end` (`YYYY-MM-DD` or an RFC 3339 date-time; either end may stay open, the end not before the start) become `dct:temporal [ a dct:PeriodOfTime ; dcat:startDate … ; dcat:endDate … ]`, typed `xsd:date` or `xsd:dateTime`. `accrual_periodicity` takes a code of the [EU frequency table](http://publications.europa.eu/resource/authority/frequency) (`ANNUAL`, `MONTHLY`, …) or its IRI and is stored as the IRI. Set them in the dataset's **Edit metadata** dialog or with `PUT /api/datasets/{id}`.

---

## Profiles: DCAT-AP and DCAT-AP-NL

The catalogue is generated from the dataset registry as an RDF graph and
serialised on request (Turtle, JSON-LD, RDF/XML, N-Triples by `Accept` or
`?format=`), so every user-supplied value is a real RDF term — a title with a
quote, a description with a `>`, or a malformed theme IRI cannot break the
document (malformed IRIs are dropped with a warning in the log).

`DCAT_PROFILE` selects the application profile. Any other value stops the server at startup.

| Value | Effect |
|---|---|
| `dcat` (default) | DCAT 3 with VoID statistics. |
| `dcat-ap` | DCAT-AP 3: `dct:identifier` and `adms:identifier` on datasets, `dct:language` everywhere it applies, `dct:format` from the EU file-type authority beside `dcat:mediaType` on every distribution, licences repeated on distributions, identifier, language and licence on the data services, a `dcat:CatalogRecord` per dataset, and the dataset falls back to the catalogue licence and contact point. |
| `dcat-ap-nl` | DCAT-AP-NL 3 on top of DCAT-AP: the language defaults to Dutch, labels and keywords are tagged `@nl`, and the profile warnings below cover what DCAT-AP-NL makes mandatory. |

The catalogue's own metadata comes from the environment (checked at startup):

| Variable | Meaning |
|---|---|
| `CATALOG_TITLE`, `CATALOG_DESCRIPTION` | Title and description of the `dcat:Catalog`. |
| `CATALOG_PUBLISHER_URI`, `CATALOG_PUBLISHER_NAME`, `CATALOG_PUBLISHER_IDENTIFIER` | The publishing organisation (`foaf:Agent`); the identifier is emitted as `dct:identifier` (an OIN or KvK number in the NL profile). Defaults to `<base>/publisher` named after the instance. |
| `CATALOG_PUBLISHER_TYPE` | The publisher's ADMS publisher type (`dct:type`): a code such as `LocalAuthority`, `NationalAuthority`, `Company`, `NonProfitOrganisation`, or an IRI. Unset: no type. |
| `CATALOG_CONTACT_NAME`, `CATALOG_CONTACT_EMAIL` | The contact point (`vcard:fn`, `vcard:hasEmail`) of the catalogue, the aggregate dataset and the data services — and, under a profile, of a dataset that has none of its own and whose organisation has none. |
| `CATALOG_LANGUAGE` | ISO 639-3 code (`ENG`, `NLD`, …) mapped onto the EU language authority; default `ENG`, or `NLD` under `dcat-ap-nl`. |
| `CATALOG_LICENSE` | Licence IRI for the catalogue, the data services and, under a profile, datasets that declare none. |

### Fallbacks and profile warnings

DCAT-AP-NL makes several properties mandatory that a registry entry may not
hold. The catalogue fills them only from real data:

- **Creator.** A dataset's owner is both its `dct:publisher` and its
  `dct:creator`, whether a user, an organisation or a group.
- **Contact point.** A dataset's own contact, else (under a profile) its
  organisation's, else the catalogue's `CATALOG_CONTACT_*`. The data services
  use the catalogue's.
- **Theme.** Never invented. A data service carries the themes of the datasets
  it serves; a dataset without a theme has none.

What is still missing — a dataset or data service without a theme, a contact
point or a licence, a theme or status IRI whose label the catalogue does not
know — is reported as a **profile warning**: logged once per message at `WARN`
(`dcat (DCAT-AP-NL 3): dataset `x` has no dcat:theme …`), and returned by
`dcat::catalog::build_catalog_report` for library callers.

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

# One organisation's catalogue
curl http://localhost:7878/acme/.well-known/void

# The model registry's catalogue (data models and vocabularies)
curl http://localhost:7878/api/catalog
```

---

## Example Output (Turtle, abridged)

```turtle
@prefix dcat:   <http://www.w3.org/ns/dcat#> .
@prefix dct:    <http://purl.org/dc/terms/> .
@prefix void:   <http://rdfs.org/ns/void#> .
@prefix foaf:   <http://xmlns.com/foaf/0.1/> .
@prefix adms:   <http://www.w3.org/ns/adms#> .
@prefix sd:     <http://www.w3.org/ns/sparql-service-description#> .
@prefix skos:   <http://www.w3.org/2004/02/skos/core#> .
@prefix xsd:    <http://www.w3.org/2001/XMLSchema#> .

<http://localhost:7878/catalog>
    a dcat:Catalog ;
    dct:title "Open Triplestore Catalog"@en ;
    dct:publisher <http://localhost:7878/publisher> ;
    dct:language <http://publications.europa.eu/resource/authority/language/ENG> ;
    foaf:homepage <http://localhost:7878/> ;
    dcat:themeTaxonomy <http://publications.europa.eu/resource/authority/data-theme> ;
    dcat:service <http://localhost:7878/sparql> ;
    dcat:dataset <http://localhost:7878/dataset> , <http://localhost:7878/dataset/abc123> .

<http://localhost:7878/dataset/abc123>
    a dcat:Dataset , void:Dataset ;
    dct:title "My Dataset"@en ;
    dct:issued "2025-01-15T10:00:00Z"^^xsd:dateTime ;
    dct:accessRights <http://publications.europa.eu/resource/authority/access-right/PUBLIC> ;
    dct:publisher <http://localhost:7878/org/org-001> ;
    dct:creator <http://localhost:7878/org/org-001> ;
    dcat:theme <http://publications.europa.eu/resource/authority/data-theme/EDUC> ;
    dct:temporal [ a dct:PeriodOfTime ; dcat:startDate "2025-01-01"^^xsd:date ] ;
    dct:accrualPeriodicity <http://publications.europa.eu/resource/authority/frequency/MONTHLY> ;
    dct:conformsTo <http://localhost:7878/data-model/publication-model/version/1.0.0> ;
    dcat:hasVersion <http://localhost:7878/dataset/abc123/version/1.0.0> ;
    dcat:hasCurrentVersion <http://localhost:7878/dataset/abc123/version/1.0.0> ;
    void:triples 5200 ;
    dcat:distribution [ a dcat:Distribution ;
        dcat:accessURL <http://localhost:7878/sparql> ;
        dcat:accessService <http://localhost:7878/sparql> ;
        dcat:mediaType <https://www.iana.org/assignments/media-types/application/sparql-results+json> ] .

<http://localhost:7878/dataset/abc123/version/1.0.0>
    a dcat:Dataset ;
    dcat:isVersionOf <http://localhost:7878/dataset/abc123> ;
    dcat:version "1.0.0" ;
    dct:issued "2025-03-01T09:00:00Z"^^xsd:dateTime .

<http://publications.europa.eu/resource/authority/data-theme/EDUC>
    a skos:Concept ; skos:prefLabel "Education, culture and sport"@en .

<http://localhost:7878/sparql>
    a dcat:DataService , sd:Service ;
    dct:title "SPARQL endpoint"@en ;
    dcat:endpointURL <http://localhost:7878/sparql> ;
    dcat:endpointDescription <http://localhost:7878/sparql> ;
    dcat:servesDataset <http://localhost:7878/dataset> , <http://localhost:7878/dataset/abc123> ;
    sd:endpoint <http://localhost:7878/sparql> ;
    sd:supportedLanguage sd:SPARQL11Query , sd:SPARQL11Update .
```

`GET /sparql` without a `query` parameter returns the SPARQL 1.1 Service Description, which is why the endpoint is its own `dcat:endpointDescription`.

---

## The model registry's catalogue

`GET /api/catalog` describes the published data models and vocabularies the caller may read: each a `dcat:Dataset` with its identifier, title, description, ADMS asset type (`Ontology` or `CodeList`), namespace (`foaf:page`), publisher (its owning organisation or user), and its released versions per DCAT 3 §11 (`{base}/data-model/{id}/version/{semver}`). Distributions are offered only in the serialisations the data endpoint serves — Turtle, N-Triples, N-Quads and TriG — each with its IANA media type and EU file type.

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

The root dataset's statistics (`void:triples`, `void:distinctSubjects`, `void:distinctObjects`, `void:properties`, `void:documents`) cover the graphs the caller may read — the whole store, default and named graphs alike, for an administrator; the readable named graphs for anyone else, so an anonymous caller counts only public graphs. They are computed with `COUNT` queries the first time a caller with that set of graphs requests the catalogue after a write, and cached until the next write.

A dataset's `void:triples` is the sum of its registered graphs' triple counts, which the store keeps current on every write — no query runs for it. Each registered graph (system graphs excluded) is also listed as a `void:subset`.

---

## Linked Data and IRIs

The catalog uses the `base_url` configured at startup (default `http://localhost:7878`) as the IRI namespace for:

- `<{base}/catalog>` — the catalog itself
- `<{base}/catalog/record/{id}>` — a dataset's catalogue record (profiles only)
- `<{base}/dataset>` — the aggregate dataset
- `<{base}/dataset/{id}>` — individual datasets
- `<{base}/dataset/{id}/version/{semver}>` — their released versions
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
