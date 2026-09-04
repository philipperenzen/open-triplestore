# Linked-document containers (ICDD)

A container packages documents, RDF payloads and the link graphs that
connect them, with an index describing the lot. Open Triplestore imports a
container into a dataset and exports a dataset as one. The mechanism is
profile-neutral; **ICDD** (ISO 21597-1, *Information Container for linked
Document Delivery*) is the first profile.

```bash
# import (write access on the dataset; the body is the archive)
curl -X POST 'http://localhost:7878/api/datasets/<id>/containers/import?profile=icdd' \
  -H "Authorization: Bearer <token>" -H 'Content-Type: application/zip' \
  --data-binary @handover.icdd

# export (read access)
curl -o handover.zip 'http://localhost:7878/api/datasets/<id>/containers/export?profile=icdd'
```

## What an import does

| In the archive | In the dataset |
|---|---|
| `Index.rdf` (the `ct:ContainerDescription`) | A graph with role **catalog**, `…/container/<cid>/index`, holding the index plus `ots:importedInto`, `ots:downloadUrl` per document and `ots:partOfContainer` per graph. |
| `Payload documents/*` listed as `ct:InternalDocument` | Assets of the dataset in folder `containers/<cid>`, served by the assets API. |
| `ct:ExternalDocument` | Recorded with its URL; nothing is fetched. |
| `Payload triples/*` listed as `ct:Linkset` | A graph with role **linkset**, keeping the linkset's IRI from the index. |
| Other RDF under `Payload triples/` | A graph with role **instances**. |
| RDF under `Ontology resources/` | A graph with role **model**. |

Every graph joins the dataset's registry with its role, so the conformance
layer, reasoning, SHACL, LDES and the DCAT catalogue see them like any other
graph; the import is one commit in the dataset's history. The response lists
the documents (with asset ids and URLs), the graphs (with roles and triple
counts) and any warnings — a document listed in the index but missing from
the archive, for example, is reported rather than failing the import.

## What an export contains

`Index.rdf` (RDF/XML, `ICDD-Part1-Container`) describing the dataset's assets
as `ct:InternalDocument`s under `Payload documents/`, its linkset-role graphs
as `ct:Linkset`s and its other data graphs as Turtle documents under
`Payload triples/`, and model-layer graphs under `Ontology resources/`.
Catalogue, provenance and entailment graphs are not exported. An export
re-imports into another dataset unchanged.

## Limits

- Archives are bomb-guarded: at most 10 000 entries, 512 MB per entry, 2 GB
  in total.
- Part 2 of ISO 21597 (dynamic semantics, linkset extensions) is not
  interpreted; linksets are stored as the RDF they are.
- The index is read in any RDF syntax its extension names; it is written as
  RDF/XML.
- Needs the `asset-archive` build feature (in `full` and the image).

The container mechanism knows nothing about construction: a clinical
handover or an archive of measurement reports packages the same way, with a
different profile implementing the same trait.
