# Datasets Guide

## Overview

A **dataset** is a named collection of RDF triples, stored as one or more named graphs in the triplestore. Each dataset is a first-class resource with its own metadata, access controls, and capabilities.

Datasets serve as the organizational unit for:
- Managing collections of related RDF graphs
- Controlling access and permissions
- Applying validation rules (e.g., SHACL)
- Executing data transformations (e.g., RML mappings)
- Providing SPARQL query endpoints

---

## Dataset Graphs & Roles

Within each dataset, graphs are organized by **role**, indicating their purpose and content type. The store recognises six role strings — `instances`, `model`, `vocabulary`, `shapes`, `entailment` and `system` — aligned to the Description-Logic *boxes*:

| Role | Purpose | Content Type | Queryable | Editable | Description |
|---|---|---|---|---|---|
| **instances** | Instance Data (A-Box) | RDF assertions about individuals | ✓ | ✓ | Contains concrete data instances, facts, and relationships. Used for storing actual data (e.g., people, organizations, events). Multiple instance graphs allowed per dataset. |
| **model** | Model (T-Box) | **Class** definitions and class axioms | ✓ | ✓ | Contains the categories of the domain: `owl:Class` / `rdfs:Class`, `rdfs:subClassOf`, restrictions, disjointness. Typically managed in the [Model Registry](/docs/models), but a dataset may carry its own. |
| **vocabulary** | Vocabulary (R-Box) | **Property** definitions and SKOS concepts | ✓ | ✓ | Contains the relations and controlled terms: `owl:ObjectProperty` / `DatatypeProperty`, `rdfs:domain`/`range`, `rdfs:subPropertyOf`, `owl:inverseOf`, plus SKOS concept schemes and concepts. |
| **shapes** | SHACL Shapes Graph | Shape definitions for validation | ✓ | ✓ | Contains SHACL NodeShapes and PropertyShapes for validating data. Used by SHACL validation engine when `shacl_on_write` is enabled. |
| **entailment** | Inferred / Derived Data | Computed triples from reasoning | ✓ | ✗ | Contains triples derived by the reasoning engine (OWL 2 RL, RDFS, etc.). Read-only; automatically populated from the schema (Model + Vocabulary) plus instances. |
| **system** | Internal / System Metadata | Configuration and metadata | ✓ | ✗ | Reserved for system use (RML mappings metadata, dataset configuration, internal bookkeeping). Read-only for end users. |

### Notes on Graph Roles

- **Three first-class layers**: `model`, `vocabulary` and `instances` are the three primary layers; `shapes` and `entailment` are orthogonal roles and `system` is internal. A single upload that mixes them can be **auto-split** into one graph per role on import (see [Import Auto-Detection](/docs/import)).
- **Legacy aliases**: the older role names `tbox` (Terminological Box) and `abox` (Assertion Box) are still accepted on input as aliases — `tbox` for `model` and `abox` for `instances` — but the canonical strings are `model`, `vocabulary` and `instances`. Note that `tbox` historically lumped properties together with classes; properties now belong to the `vocabulary` role.
- **Multiple graphs per role**: Datasets can contain multiple graphs with the same role (e.g., multiple instance graphs for different data subsets).
- **Entailment is computed**: The `entailment` graph is automatically populated by the reasoning engine and cannot be directly written to.
- **System is reserved**: The `system` role is reserved for internal metadata and should not be modified by end users.
- **Role assignment**: Graphs are assigned roles via the dataset API (`PUT /api/datasets/:id/role`).

---

## Creating a Dataset

```bash
curl -X POST http://localhost:7878/api/datasets \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "my-dataset",
    "description": "A dataset for testing",
    "visibility": "private",
    "owner_type": "user",
    "owner_id": "<user_id>"
  }'
```

**Response:**
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "name": "my-dataset",
  "description": "A dataset for testing",
  "owner_type": "user",
  "owner_id": "<user_id>",
  "visibility": "private",
  "created_at": "2026-05-07T10:00:00Z"
}
```

---

## Adding Graphs to a Dataset

```bash
# Add a graph (without specifying role)
curl -X POST http://localhost:7878/api/datasets/<dataset_id>/graphs \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{"graph_iri": "urn:example:my-graph"}'

# Assign or update a graph's role
curl -X PUT http://localhost:7878/api/datasets/<dataset_id>/role \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{
    "graph_iri": "urn:example:my-graph",
    "role": "instances"
  }'
```

---

## Dataset Features

### SHACL Validation on Write

Enable automatic validation when data is written to the dataset:

```bash
curl -X PUT http://localhost:7878/api/datasets/<dataset_id> \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{"shacl_on_write": true}'
```

Upload a shapes graph:

```bash
curl -X PUT http://localhost:7878/api/datasets/<dataset_id>/shapes \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/turtle' \
  --data-binary @shapes.ttl
```

See [shacl.md](shacl.md) for full SHACL documentation.

### RML Data Transformation

Use RML (RDF Mapping Language) to transform CSV/JSON/XML into RDF:

```bash
# Upload an RML mapping
curl -X PUT http://localhost:7878/api/datasets/<dataset_id>/mappings \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: text/turtle' \
  --data-binary @mapping.ttl

# Execute the mapping
curl -X POST http://localhost:7878/api/datasets/<dataset_id>/mappings/execute \
  -H "Authorization: Bearer <token>" \
  -F 'data.csv=@data.csv'
```

See [rml.md](rml.md) for full RML documentation.

---

## Visibility & Access Control

Datasets support three visibility levels:

| Visibility | Public Access | Registered Users | Owner | Admin |
|---|---|---|---|---|
| `public` | Read-only | Read-only | Read + Write | Read + Write |
| `members` | ✗ | Read-only | Read + Write | Read + Write |
| `private` | ✗ | ✗ | Read + Write | Read + Write |

### Explicit access grants

Beyond visibility and org/group membership, a dataset manager can grant access
to a specific **principal** — a `user`, a `group`, or an `organisation` — at one
of three levels:

| Grant level | Capability |
|---|---|
| `viewer` | Read only |
| `editor` | Read + write data |
| `admin` | Manage the dataset, its metadata, and its access grants |

A grant to a group or organisation applies to all of its members. Grants combine
with membership-derived access, taking the strongest — except that an org/group
admin can never be demoted below `admin` on resources their org/group owns.

---

## Listing Datasets

```bash
# List all datasets you have access to
curl -H "Authorization: Bearer <token>" \
  'http://localhost:7878/api/datasets'

# Filter by visibility
curl -H "Authorization: Bearer <token>" \
  'http://localhost:7878/api/datasets?visibility=public'

# Filter by owner
curl -H "Authorization: Bearer <token>" \
  'http://localhost:7878/api/datasets?owner_id=<user_id>'
```

---

## Best Practices

1. **Organize by role**: Use graph roles consistently — keep instance data in `instances` graphs, classes in `model`, properties and concept schemes in `vocabulary`.
2. **Enable validation early**: Set up SHACL shapes and enable `shacl_on_write` to catch data quality issues proactively.
3. **Use meaningful IRIs**: Graph IRIs like `urn:dataset:<id>:instances:main` are more readable than UUIDs.
4. **Plan for growth**: Multiple instance graphs allow logical separation of data subsets without changing the schema.
5. **Version metadata**: Store dataset metadata as RDF for discoverability via SPARQL.
