# Linked Data Modelling

How to model data so it stays consistent, valid, and discoverable. This is a working summary of the canonical **Linked Data Modelling Styleguide** ([read the full styleguide](/docs/linked-data-modelling-styleguide)) — the normative standard the triplestore and its companion tools all follow.

## The three layers

Every knowledge graph separates into three layers. Knowing which layer you are in is the single most important modelling habit — a class definition, a validation rule, and a fact about a real thing belong in three different graphs.

| Layer | Question it answers | Vocabularies | Graph role |
|---|---|---|---|
| Knowledge Model | "What kinds of things exist and how do they relate?" | SKOS, RDFS, OWL | Vocabulary / Model |
| Information Model | "What makes a piece of data valid?" | SHACL | Shapes |
| Instance Data | "Which specific things are we describing?" | the model's own terms | Instances |

These map directly onto the [graph roles](/docs/import) the store auto-detects, and the [Model & Vocabulary registries](/docs/models) that version them.

## Layer 1 — Knowledge Model (SKOS + OWL)

Define concepts with **dual typing** — each is both a `skos:Concept` (navigable, labelled, mappable) and an `owl:Class` (formally classifiable). Give every term bilingual labels (`@nl` and `@en`), a `skos:notation`, and a home in a `skos:ConceptScheme`.

```turtle
@prefix ex:   <https://example.org/showcase/> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .

ex:PublicationVocabulary a skos:ConceptScheme ;
    skos:prefLabel "Publication Vocabulary"@en , "Publicatievocabulaire"@nl ;
    skos:hasTopConcept ex:Publication .

ex:Book a skos:Concept , owl:Class ;
    skos:prefLabel "Book"@en , "Boek"@nl ;
    skos:definition "A written work published as a bound or digital volume."@en ;
    skos:notation "BOOK" ;
    skos:inScheme ex:PublicationVocabulary ;
    skos:broader ex:Publication ;
    rdfs:subClassOf ex:Publication .

ex:pageCount a owl:DatatypeProperty ;
    rdfs:label "Page count"@en , "Aantal pagina's"@nl ;
    rdfs:domain ex:Book ;
    rdfs:range xsd:integer .
```

## Layer 2 — Information Model (SHACL)

Constrain the data with SHACL shapes bound to a class via `sh:targetClass`. Shapes belong in their own **Shapes** graph and are resolved automatically when a dataset declares the model it `dct:conformsTo`. See [SHACL Validation](/docs/shacl).

```turtle
@prefix sh:   <http://www.w3.org/ns/shacl#> .
@prefix ex:   <https://example.org/showcase/> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .

ex:BookShape a sh:NodeShape ;
    sh:targetClass ex:Book ;
    sh:property [
        sh:path skos:prefLabel ;
        sh:datatype rdf:langString ;
        sh:minCount 1 ; sh:uniqueLang true ;
    ] ;
    sh:property [
        sh:path skos:notation ;
        sh:minCount 1 ; sh:maxCount 1 ; sh:pattern "^BOOK" ;
    ] ;
    sh:property [
        sh:path ex:pageCount ;
        sh:datatype xsd:integer ;
        sh:minInclusive 1 ; sh:maxInclusive 5000 ;
    ] .
```

## Layer 3 — Instance Data

Describe real things using the model's terms. Type every literal, language-tag every label, and store instances in a [dataset](/docs/datasets) named graph. Any spatial attribute (here, the place of publication) uses the GeoSPARQL blank-node shape.

```turtle
@prefix ex:   <https://example.org/showcase/> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
@prefix geo:  <http://www.opengis.net/ont/geosparql#> .

ex:Dracula a ex:Book ;
    skos:prefLabel "Dracula"@en , "Dracula"@nl ;
    ex:pageCount "418"^^xsd:integer ;
    ex:publicationYear "1897"^^xsd:integer ;
    ex:publishedBy ex:ExampleOrg ;
    geo:hasGeometry [ geo:asWKT "POINT(-0.1276 51.5074)"^^geo:wktLiteral ] .
```

## Describing datasets & organisations (DCAT · VoID · ADMS · ORG)

Metadata *about* a dataset or organisation is itself linked data. From the fields you set on a [dataset](/docs/datasets), the store generates a full DCAT 2 catalogue — with VoID statistics, ADMS status, licence, access rights, and ORG/FOAF publisher — at `/.well-known/void`. See the [DCAT Catalogue](/docs/dcat) guide.

```turtle
@prefix dcat: <http://www.w3.org/ns/dcat#> .
@prefix dct:  <http://purl.org/dc/terms/> .
@prefix void: <http://rdfs.org/ns/void#> .
@prefix adms: <http://www.w3.org/ns/adms#> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix org:  <http://www.w3.org/ns/org#> .

<https://triplestore.example/dataset/abc123> a dcat:Dataset , void:Dataset ;
    dct:title "Library Catalogue 2025" ;
    dct:accessRights <http://publications.europa.eu/resource/authority/access-right/PUBLIC> ;
    dct:publisher <https://triplestore.example/org/example-org> ;
    dct:license <https://creativecommons.org/licenses/by/4.0/> ;
    dct:conformsTo <https://triplestore.example/data-model/publication-model/version/2.1.0> ;
    adms:status <http://purl.org/adms/status/UnderDevelopment> ;
    void:triples 84200 .

<https://triplestore.example/org/example-org> a foaf:Organization , org:FormalOrganization ;
    foaf:name "Example Organization" .
```

## Versioning & deprecation

Models and vocabularies are versioned artefacts (Draft → Staged → Published → Deprecated; see [Model & Vocabulary Versioning](/docs/models) and [Dataset Versioning & Sharing](/docs/versioning)). Pin instance datasets to an explicit published version, and never delete a published term — deprecate it three ways so every layer sees it:

```turtle
ex:OldBookType
    owl:deprecated true ;
    adms:status <http://purl.org/adms/status/Deprecated> ;
    skos:historyNote "Merged into ex:Book in v2.0.0."@en .
```

## Modelling rules

- **One role per named graph** — Keep the Knowledge Model, Shapes, and Instances in separate graphs so the store classifies them correctly. Override an ambiguous mix with `?kind=` on upload.
- **Bilingual labels, one per language** — Every concept, class and property needs `skos:prefLabel` / `rdfs:label` in `@nl` and `@en`. Synonyms go in `skos:altLabel`.
- **IRIs, not strings, for links** — `dct:creator`, `dct:publisher` and `dcat:accessURL` take IRIs. Use named nodes for concepts; reserve blank nodes for anonymous values like a geometry or contact card.
- **Datatype every literal** — Dates as `xsd:date` / `xsd:dateTime`, numbers as `xsd:integer` / `xsd:decimal`; human labels always carry a language tag.
- **Reuse before minting** — Prefer W3C vocabularies (SKOS, OWL, DCAT, PROV, ORG) and domain standards for your field (e.g. schema.org, Dublin Core, BIBFRAME) over inventing custom terms.
