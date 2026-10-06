# Linked-document containers (ICDD)

A container packages documents, RDF payloads and the link graphs that
connect them, with an index describing the lot. Open Triplestore imports a
container into a dataset, exports a dataset as one, and validates one without
storing anything. The mechanism is profile-neutral; **ICDD** (ISO 21597-1,
*Information Container for linked Document Delivery*, Part 1) is the first
profile.

```bash
# validate only (any signed-in user; nothing is stored)
curl -X POST 'http://localhost:7878/api/containers/validate?profile=icdd' \
  -H "Authorization: Bearer <token>" -H 'Content-Type: application/zip' \
  --data-binary @handover.icdd

# import (write access on the dataset; the body is the archive)
curl -X POST 'http://localhost:7878/api/datasets/<id>/containers/import?profile=icdd' \
  -H "Authorization: Bearer <token>" -H 'Content-Type: application/zip' \
  --data-binary @handover.icdd

# import, refusing a container with any violation
curl -X POST 'http://localhost:7878/api/datasets/<id>/containers/import?strict=true' …

# export (read access)
curl -OJ 'http://localhost:7878/api/datasets/<id>/containers/export?profile=icdd'
```

The archive body may be up to `OTS_MAX_UPLOAD_MB` (default 512 MB); it unpacks
to at most 10 000 entries, 512 MB per entry and 2 GB in total.

## Validation

Every import runs the profile's validator and returns its report as
`validation`; `POST /api/containers/validate` returns the same report alone:

```json
{ "profile": "icdd", "conforms": true, "violations": 0, "warnings": 2,
  "findings": [ { "severity": "warning", "code": "ontology-resource-missing",
                  "message": "…", "file": "Ontology resources/Container.rdf" } ] }
```

`conforms` means no violation. Each finding has a `severity` (`violation`,
`warning`, `info`), a stable `code`, a `message`, and where it applies the
index node (`focus`), property (`path`), archive entry (`file`) or shape.

For ICDD the validator has two halves:

- **Shapes.** Open Triplestore's own SHACL shapes
  ([`src/containers/icdd_shapes.ttl`](../src/containers/icdd_shapes.ttl)),
  written from the restrictions the Part 1 ontologies state, run with the
  built-in SHACL engine over `Index.rdf` and the linksets: one
  `ct:conformanceIndicator` and one `ct:publishedBy` on the container
  description; one `ct:name` on every document; one `ct:filename`,
  `ct:filetype` and `ct:belongsToContainer` on internal (and secured and
  encrypted) documents; one `ct:url` on external, one `ct:foldername` on
  folder, one `ct:checksum` and `ct:checksumAlgorithm` on secured documents;
  parties typed `ct:Person` or `ct:Organisation` (`ct:Party` is abstract)
  with one `ct:name`; links with at least two link elements, binary links with
  exactly two, directed binary links with one "from" and one "to"; one
  `ls:hasDocument` per link element. A few rules the file adds from the class
  definitions (directed links, identifiers) are warnings. ISO's own SHACL
  annexes are not used: their licence does not allow publishing them here,
  and they name properties the ontologies do not define.
- **Structure**, checked in Rust: one root `Index.rdf` in RDF/XML (another
  `index.*` imports but is a violation); exactly one
  `ct:ContainerDescription`; the `Ontology resources/`, `Payload documents/`
  and `Payload triples/` folders; ISO's `Container.rdf` and `Linkset.rdf` in
  `Ontology resources/` (a warning when absent: the container is then not
  Part 1-conformant); every listed file present at its path relative to its
  folder, with no `..` escapes and no two documents naming one file;
  secured documents' checksums (SHA-1, SHA-224, SHA-256, SHA-384, SHA-512;
  hex or base64); files in `Payload documents/` no document lists (warning);
  link elements that name a document the index does not list (warning);
  non-RDF/XML linksets (warning); and no `rdfs:subClassOf` /
  `rdfs:subPropertyOf` / `owl:equivalent…` of an ICDD class or property in
  the index, the linksets or an extra ontology — a Part 1 container may not
  extend them.

## What an import does

| In the archive | In the dataset |
|---|---|
| `Index.rdf` (the `ct:ContainerDescription`) | A graph with role **catalog**, `…/container/<cid>/index`, holding the index plus `ots:importedInto`, and per document `ots:downloadUrl` / `ots:assetId` (or `ots:assetFolder` for a folder) and `ots:checksumStatus`, per graph `ots:partOfContainer`. |
| `ct:InternalDocument` | An asset of the dataset in folder `containers/<cid>`, sub-folders of its `ct:filename` kept; its `ct:name` becomes the asset title. |
| `ct:FolderDocument` | Every file under its folder, as assets in the matching asset sub-folders. |
| `ct:SecuredDocument` | An asset, with its checksum verified (`checksum_status`: `verified`, `mismatch`, `unsupported-algorithm`). |
| `ct:EncryptedDocument` | An asset kept as an opaque `application/octet-stream` file, flagged `encrypted`; nothing is decrypted. |
| `ct:ExternalDocument` | Recorded with its URL; nothing is fetched. |
| `Payload triples/*` listed as `ct:Linkset` | A graph with role **linkset**. |
| An RDF document an Open Triplestore export wrote (`ots:graphRole`) | A graph with role **instances**. |
| Other RDF under `Payload triples/` | A graph with role **instances**. |
| RDF under `Ontology resources/` | A graph with role **model**, except ISO's `Container.rdf`, `Linkset.rdf` and `ExtendedLinkset.rdf`, which are recognised (`standard_ontologies`) and not loaded. |

A graph IRI the index gives a payload is kept only when it lies in the
importing dataset's own namespace and no other dataset has it; otherwise the
payload is re-homed under the container's namespace and the original IRI kept
as `ots:sourceIri`.

Every graph joins the dataset's registry with its role, so the conformance
layer, reasoning, SHACL, LDES and the DCAT catalogue see them like any other
graph; the import is one commit in the dataset's history. The response
lists:

- the container's `conformance`, `description`, `created_by` /
  `published_by` (`{iri, kind, name}`), `creation_date`, `version_id` and
  `version_description`;
- each document with its `kind` (`internal`, `external`, `folder`,
  `secured`, `encrypted`), `name`, `filename`, versioning (`version_id`,
  `version_description`, `prior_version`), `alternatives`, `requested`,
  `created_by` / `modified_by`, its asset id, URL and folder (a folder
  document lists its `files`), and `checksum_status` / `encrypted`;
- the graphs (with roles and triple counts), `warnings`, and `validation`.

Without `strict`, a container with violations still imports what it can
(a document missing from the archive is reported, not fatal). With
`strict=true`, any violation refuses the container with 422 and the report,
and nothing is stored. An index with several container descriptions is
always refused.

## What an export contains

An `<dataset>.icdd` file (a ZIP):

- `Index.rdf` (RDF/XML, `ICDD-Part1-Container`, importing the Container
  ontology) with the dataset's owner as `ct:publishedBy` / `ct:createdBy`
  (`ct:Person` for a user, `ct:Organisation` for an organisation or group);
- the dataset's assets under `Payload documents/`, each with `ct:name`,
  `ct:filename` (relative to that folder, asset folders kept; a name two
  assets share gets a `-2` suffix), `ct:filetype`, `ct:format` and
  `ct:belongsToContainer`;
- linkset-role graphs as RDF/XML linksets under `Payload triples/`; other
  data graphs as RDF documents under `Payload documents/` (marked
  `ots:graphRole`, so they re-import as graphs); model-layer graphs under
  `Ontology resources/`;
- the three folders as explicit entries, and ISO's `Container.rdf` and
  `Linkset.rdf` in `Ontology resources/` when the operator provides them (see
  below).

Documents an earlier import brought in go back out as they came: their
index IRIs (so linksets still resolve), kinds (folder, secured — with a
freshly computed checksum — encrypted, external), names, versions,
alternatives, `requested` and parties. Catalogue, provenance and entailment
graphs are not exported. A graph is written in Turtle only when it cannot be
written as RDF/XML (an RDF 1.2 triple term, say).

The export validates itself and says so in headers: `X-Container-Conforms`,
`X-Container-Validation` (`violations=…; warnings=…`) and
`X-Container-Findings` (the finding codes). POST the file to
`/api/containers/validate` for the full report.

### ISO ontology files (`OTS_ICDD_ONTOLOGY_DIR`)

ISO 21597-1 has every container carry the ISO `Container.rdf` and
`Linkset.rdf` ontologies. ISO's licence does not allow this repository to
publish them, so Open Triplestore does not ship them. To produce Part
1-conformant exports, download both files from ISO's maintenance portal,
<https://standards.iso.org/iso/21597/-1/ed-1/en/>, put them in one directory
and set `OTS_ICDD_ONTOLOGY_DIR` to it. Exports then embed them in
`Ontology resources/` unchanged. Without the setting (or with a file missing
or not RDF/XML) the export still works, but carries the
`ontology-resource-missing` warning: it is not Part 1-conformant for that
reason.

## Limits

- Part 2 of ISO 21597 (*Link types*: 15 typed links such as `IsIdenticalTo`,
  `HasPart`/`IsPartOf`, `Supersedes`/`IsSupersededBy`) is not interpreted:
  its linksets import as the RDF they are, a Part 2 conformance indicator is
  reported as an info finding, and exports are Part 1 containers. Checking
  Part 2's conformity criteria needs the ISO 21597-2 text, which is not
  publicly available.
- The index is read in any RDF syntax its extension names; it is written as
  RDF/XML.
- Needs the `asset-archive` build feature (in `full` and the image).

The container mechanism knows nothing about construction: a clinical
handover or an archive of measurement reports packages the same way, with a
different profile implementing the same trait.
