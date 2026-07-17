# Data Modeling Architecture

> **See also:** [**Linked Data Modelling Styleguide**](linked-data-modelling-styleguide.md) — the canonical, normative standard for *how* to model (vocabularies, IRIs, SKOS/OWL/SHACL, DCAT/VoID/ADMS metadata, versioning). This document covers the *architecture*; the styleguide covers the *conventions*.

This document describes how the triplestore separates **models** from **instance data** and how SHACL validation bridges the two.

---

## Layered Architecture

The triplestore implements the semantic web's modeling layers as two primary registration surfaces:

| Layer | Where | Contains | Examples |
|---|---|---|---|
| **Model & Vocabulary Registry** | `/api/models` | Models (classes), vocabularies (properties + concept schemes), SHACL shapes | Publication model, subject vocabulary, SHACL rules for catalogue data |
| **Datasets** | `/api/datasets` | Instance data conforming to those models | Actual catalogue records, book entries, holdings |

This separation follows the classic description logic distinction between the *terminological box* (T-Box), the *relational box* (R-Box) and the *assertion box* (A-Box). In Open Triplestore these correspond to distinct **graph roles**, three of which (Model, Vocabulary, Instances) are first-class layers in their own right:

- **Model** (the T-Box) — **class** definitions and class axioms (`owl:Class`, `rdfs:subClassOf`, `owl:equivalentClass`, restrictions, disjointness). Managed in the Model Registry with a full version lifecycle (Draft → Staged → Published → Deprecated).
- **Vocabulary** (the R-Box) — **property** definitions and relations (`owl:ObjectProperty`/`DatatypeProperty`, `rdfs:domain`/`range`, `rdfs:subPropertyOf`, `owl:inverseOf`) **plus** SKOS concept schemes and controlled vocabularies. Also managed in the Model Registry.
- **Shapes** (SHACL) — validation constraints. An orthogonal role, stored alongside Model/Vocabulary graphs in the Model Registry.
- **Instances** (assertion data; the A-Box) — individual facts. Stored in Datasets as named graphs with access control and SPARQL endpoints.

> **Note:** The terms T-Box, R-Box and A-Box come from Description Logic and the OWL 2 specification. Open Triplestore uses the more descriptive role names **Model** (classes), **Vocabulary** (properties + concepts), **Shapes**, and **Instances** in its UI and API, but the underlying concepts are the same. Each role's graph can be **decomposed** on import: classes route to a Model sub-graph, properties and concept schemes to a Vocabulary sub-graph, and individuals to an Instances sub-graph.

---

## Linking Datasets to a Model

Every dataset can declare which model it **conforms to** via `dct:conformsTo`. This link connects instance data to the model it was designed for:

```
dataset.conforms_to_model   = "publication-model"
dataset.conforms_to_version = "2.1.0"
```

In the DCAT catalog this produces:

```turtle
<http://example.org/dataset/library-catalogue>
    a dcat:Dataset, void:Dataset ;
    dct:title "Library Catalogue 2025" ;
    dct:conformsTo <http://example.org/data-model/publication-model/version/2.1.0> ;
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
  "conforms_to_model": "publication-model",
  "conforms_to_version": "2.1.0"
}'
```

**On update:**

```bash
curl -X PUT /api/datasets/{id} -d '{
  "name": "Library Catalogue 2025",
  "visibility": "members",
  "conforms_to_model": "publication-model",
  "conforms_to_version": "2.2.0"
}'
```

---

## SHACL Validation: Two Levels

SHACL validation operates at two distinct levels that correspond to the Model/Instances separation:

### 1. Model Validation (Model Registry)

Validates the **model itself** — are the SHACL shapes well-formed? Are class definitions consistent? This is done within the Model Registry when reviewing model versions before publishing. The shapes are stored as part of the versioned model graph.

### 2. Instance Data Validation (Datasets)

Validates the **instance data** against the shapes defined in the model. When a dataset is linked to a model via `conforms_to_model`, the triplestore resolves the SHACL shapes from that model version graph.

```
POST /api/datasets/{id}/validate
```

**Shape resolution order:**

1. **Dataset-specific shapes** — if a shapes graph is configured directly on the dataset (`shapes_graph_iri`), it takes precedence.
2. **Model version shapes** — if no dataset-specific shapes exist but `conforms_to_model` and `conforms_to_version` are set, the SHACL shapes from the model version's named graph are used.

This means you can:
- Upload a publication model with SHACL shapes to the Model Registry.
- Create a dataset of library catalogue records and link it to that model.
- Validate the catalogue data against the publication model's shapes — without duplicating the shapes.

### On-Write Validation

When `shacl_on_write` is enabled on a dataset, every `PUT`/`POST` to the Graph Store or LDP endpoints validates incoming data before committing. The same shape resolution order applies: dataset-specific shapes first, then linked model shapes.

---

## Workflow Example

### Step 1: Publish the Model

```bash
# Create the model entry
curl -X POST /api/models -d '{
  "title": "Publication Information Model",
  "namespace": "https://example.org/publication#",
  "description": "Classes, properties and SHACL shapes for library catalogue data"
}'

# Upload a version containing OWL classes + SHACL shapes
curl -X POST /api/models/publication-model/versions \
  -F file=@publication-model-v1.ttl \
  -F version=1.0.0

# Publish it
curl -X POST /api/models/publication-model/versions/1.0.0/publish
```

### Step 2: Create a Dataset Linked to the Model

```bash
curl -X POST /api/datasets -d '{
  "name": "Library Catalogue 2025",
  "owner_type": "organisation",
  "owner_id": "example-org",
  "conforms_to_model": "publication-model",
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
│   Model & Vocabulary Registry (Model / Vocabulary / Shapes) │
│                                                  │
│  ┌──────────────┐ ┌───────────────┐ ┌──────────┐ │
│  │ Model (T-Box) │ │ Vocabulary    │ │ SHACL    │ │
│  │ OWL Classes   │ │ (R-Box)       │ │ Shapes   │ │
│  │ Class axioms  │ │ Properties    │ │ (validn. │ │
│  │ Restrictions  │ │ Concept       │ │ constr.) │ │
│  │               │ │ schemes       │ │          │ │
│  └──────────────┘ └───────────────┘ └──────────┘ │
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
| Schema (classes) | RDFS | Class hierarchies in model versions |
| Formal semantics | OWL 2 | Class & property axioms and reasoning in model/vocabulary versions |
| Knowledge org | SKOS | Controlled vocabularies in vocabulary versions |
| Validation | SHACL, ShEx | Shapes in model versions or dataset-specific |
| Catalog | DCAT 2, VoID | Auto-generated at `/.well-known/void` |
| Conformance | `dct:conformsTo` | Links datasets to model versions |
| Query | SPARQL 1.1 | Uniform access to all layers |
