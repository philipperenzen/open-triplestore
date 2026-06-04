# Data Modeling Architecture

> **See also:** [**Linked Data Modelling Styleguide**](linked-data-modelling-styleguide.md) — the canonical, normative standard for *how* to model (vocabularies, IRIs, SKOS/OWL/SHACL, DCAT/VoID/ADMS metadata, versioning). This document covers the *architecture*; the styleguide covers the *conventions*.

This document describes how the triplestore separates **models** from **instance data** and how SHACL validation bridges the two.

---

## Layered Architecture

The triplestore implements the semantic web's modeling layers as two primary registration surfaces:

| Layer | Where | Contains | Examples |
|---|---|---|---|
| **Ontology Registry** | `/api/ontologies` | Models, schemas, SHACL shapes, SKOS vocabularies | Publication ontology, subject vocabulary, SHACL rules for catalogue data |
| **Datasets** | `/api/datasets` | Instance data conforming to those models | Actual catalogue records, book entries, holdings |

This separation follows the classic description logic distinction between the *terminological box* (T-Box) and the *assertion box* (A-Box). In Open Triplestore these correspond to distinct **graph roles**:

- **Model** (OWL/RDFS terminological schema; what description logic calls the T-Box) — class definitions, property axioms. Managed in the Ontology Registry with a full version lifecycle (Draft → Staged → Published → Deprecated).
- **Vocabulary** (SKOS; also part of the terminological layer) — concept schemes and controlled vocabularies. Also managed in the Ontology Registry.
- **Shapes** (SHACL) — validation constraints. Stored alongside Model graphs in the Ontology Registry.
- **Instances** (assertion data; what description logic calls the A-Box) — individual facts. Stored in Datasets as named graphs with access control and SPARQL endpoints.

> **Note:** The terms T-Box and A-Box come from Description Logic and the OWL 2 specification. Open Triplestore uses the more descriptive role names **Model**, **Vocabulary**, **Shapes**, and **Instances** in its UI and API, but the underlying concepts are the same.

---

## Linking Datasets to Ontologies

Every dataset can declare which ontology it **conforms to** via `dct:conformsTo`. This link connects instance data to the model it was designed for:

```
dataset.conforms_to_ontology = "publication-model"
dataset.conforms_to_version  = "2.1.0"
```

In the DCAT catalog this produces:

```turtle
<http://example.org/dataset/library-catalogue>
    a dcat:Dataset, void:Dataset ;
    dct:title "Library Catalogue 2025" ;
    dct:conformsTo <http://example.org/ontology/publication-model/version/2.1.0> ;
    void:triples 84200 .
```

### Setting Conformance

**On creation:**

```bash
curl -X POST /api/datasets -d '{
  "name": "Library Catalogue 2025",
  "owner_type": "organisation",
  "owner_id": "example-org",
  "visibility": "members",
  "conforms_to_ontology": "publication-model",
  "conforms_to_version": "2.1.0"
}'
```

**On update:**

```bash
curl -X PUT /api/datasets/{id} -d '{
  "name": "Library Catalogue 2025",
  "visibility": "members",
  "conforms_to_ontology": "publication-model",
  "conforms_to_version": "2.2.0"
}'
```

---

## SHACL Validation: Two Levels

SHACL validation operates at two distinct levels that correspond to the Model/Instances separation:

### 1. Model Validation (Ontology Registry)

Validates the **ontology model itself** — are the SHACL shapes well-formed? Are class definitions consistent? This is done within the Ontology Registry when reviewing ontology versions before publishing. The shapes are stored as part of the versioned ontology graph.

### 2. Instance Data Validation (Datasets)

Validates the **instance data** against the shapes defined in the model. When a dataset is linked to an ontology via `conforms_to_ontology`, the triplestore resolves the SHACL shapes from that ontology version graph.

```
POST /api/datasets/{id}/validate
```

**Shape resolution order:**

1. **Dataset-specific shapes** — if a shapes graph is configured directly on the dataset (`shapes_graph_iri`), it takes precedence.
2. **Ontology version shapes** — if no dataset-specific shapes exist but `conforms_to_ontology` and `conforms_to_version` are set, the SHACL shapes from the ontology version's named graph are used.

This means you can:
- Upload a publication ontology with SHACL shapes to the Ontology Registry.
- Create a dataset of library catalogue records and link it to that ontology.
- Validate the catalogue data against the publication ontology's shapes — without duplicating the shapes.

### On-Write Validation

When `shacl_on_write` is enabled on a dataset, every `PUT`/`POST` to the Graph Store or LDP endpoints validates incoming data before committing. The same shape resolution order applies: dataset-specific shapes first, then linked ontology shapes.

---

## Workflow Example

### Step 1: Publish the Ontology Model

```bash
# Create the ontology entry
curl -X POST /api/ontologies -d '{
  "title": "Publication Information Model",
  "namespace": "https://example.org/publication#",
  "description": "Classes, properties and SHACL shapes for library catalogue data"
}'

# Upload a version containing OWL classes + SHACL shapes
curl -X POST /api/ontologies/publication-model/versions \
  -F file=@publication-model-v1.ttl \
  -F version=1.0.0

# Publish it
curl -X POST /api/ontologies/publication-model/versions/1.0.0/publish
```

### Step 2: Create a Dataset Linked to the Ontology

```bash
curl -X POST /api/datasets -d '{
  "name": "Library Catalogue 2025",
  "owner_type": "organisation",
  "owner_id": "example-org",
  "conforms_to_ontology": "publication-model",
  "conforms_to_version": "1.0.0"
}'
```

### Step 3: Upload Instance Data

```bash
curl -X PUT "/store?graph=urn:catalogue:2025" \
  -H "Content-Type: text/turtle" \
  -d @catalogue-records.ttl

# Register the graph under the dataset
curl -X POST /api/datasets/{id}/graphs -d '{"graph_iri": "urn:catalogue:2025"}'
```

### Step 4: Validate

```bash
# Validates instance data against SHACL shapes from publication-model v1.0.0
curl -X POST /api/datasets/{id}/validate
```

The validation report shows which instances violate the model's constraints:

```json
{
  "conforms": false,
  "results_count": 2,
  "results": [
    {
      "severity": "Violation",
      "focus_node": "urn:book:dracula",
      "path": "https://example.org/publication#publicationDate",
      "message": "Less than 1 values on publication:publicationDate"
    }
  ]
}
```

---

## Composition Summary

```
┌──────────────────────────────────────────────────┐
│     Ontology Registry (Model / Vocabulary / Shapes)   │
│                                                  │
│  ┌─────────────┐  ┌──────────────────────────┐   │
│  │ OWL Classes  │  │ SHACL Shapes             │   │
│  │ Properties   │  │ (validation constraints) │   │
│  │ Axioms       │  │                          │   │
│  └─────────────┘  └──────────────────────────┘   │
│                                                  │
│  Versioned: Draft → Staged → Published           │
└──────────────┬───────────────────────────────────┘
               │ dct:conformsTo
               ▼
┌──────────────────────────────────────────────────┐
│               Datasets (Instances)               │
│                                                  │
│  ┌─────────────────────────────────────────────┐ │
│  │ Named Graphs with Instance Data             │ │
│  │ (concrete facts: catalogue records,           │ │
│  │  book entries, holdings)                    │ │
│  └─────────────────────────────────────────────┘ │
│                                                  │
│  SPARQL endpoints · Access control · VoID stats  │
└──────────────────────────────────────────────────┘
```

---

## Relation to Standards

| Concept | Standard | Role in Triplestore |
|---|---|---|
| Instance data | RDF | Stored in dataset named graphs |
| Data model | Turtle, JSON-LD, N-Triples | Serialization formats for import/export |
| Schema | RDFS | Class/property hierarchies in ontology versions |
| Ontology | OWL 2 | Formal semantics and reasoning in ontology versions |
| Knowledge org | SKOS | Controlled vocabularies in ontology versions |
| Validation | SHACL, ShEx | Shapes in ontology versions or dataset-specific |
| Catalog | DCAT 2, VoID | Auto-generated at `/.well-known/void` |
| Conformance | `dct:conformsTo` | Links datasets to ontology versions |
| Query | SPARQL 1.1 | Uniform access to all layers |
