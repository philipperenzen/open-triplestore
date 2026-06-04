# Linked Data Platform (LDP) 1.0

The triplestore implements the full [W3C Linked Data Platform 1.0](https://www.w3.org/TR/ldp/) specification, including all four resource types and all seven HTTP methods.

Enabled with the `ldp` Cargo feature (included in the `full` feature set).  Routes are mounted under `/ldp/`.

---

## Resource types

| Type | IRI | Description |
|---|---|---|
| `ldp:RDFSource` | `http://www.w3.org/ns/ldp#RDFSource` | Plain RDF document; supports full SPARQL Update via PATCH |
| `ldp:BasicContainer` | `http://www.w3.org/ns/ldp#BasicContainer` | Ordered set of `ldp:contains` members |
| `ldp:DirectContainer` | `http://www.w3.org/ns/ldp#DirectContainer` | Writes a configurable membership triple on every POST |
| `ldp:IndirectContainer` | `http://www.w3.org/ns/ldp#IndirectContainer` | Membership triple object read from the new member's body |
| `ldp:NonRDFSource` | `http://www.w3.org/ns/ldp#NonRDFSource` | Arbitrary binary resource; stored as base64 and returned with original `Content-Type` |

Every resource is also typed as `ldp:Resource`.  RDF resources additionally carry `ldp:RDFSource`.

---

## HTTP Methods

| Method | Semantics |
|---|---|
| `GET` | Fetch resource description as N-Triples.  Containers include `ldp:contains` member triples (unless `Prefer: return=minimal`).  Non-RDF Sources return raw binary bytes with original `Content-Type`. |
| `HEAD` | Same as GET but no body.  Returns `ETag` and `Link` headers. |
| `POST` | Create a new member resource.  Uses `Slug` header as IRI hint (falls back to UUID).  Accepts Turtle, JSON-LD, and binary bodies.  Direct/Indirect Containers automatically write membership triples. |
| `PUT` | Replace (or create) a resource.  Accepts Turtle, JSON-LD, RDF/XML, or binary bodies.  Supports `If-Match` ETag for optimistic concurrency. |
| `PATCH` | Apply a SPARQL Update to an RDF Source in place.  Requires `Content-Type: application/sparql-update`.  Supports `If-Match` ETag. |
| `DELETE` | Remove the resource and its `ldp:contains` triple from the parent container.  Also removes the membership triple from Direct/Indirect Containers. |
| `OPTIONS` | Advertise `Allow`, `Accept-Post`, `Accept-Patch`, and `Link` headers. |

---

## Headers reference

### Request headers

| Header | Applies to | Description |
|---|---|---|
| `Slug` | POST | Suggested local name for the new member IRI. Falls back to a random UUID if absent or empty. |
| `If-Match` | PUT, PATCH | ETag value from a prior GET/HEAD.  Request fails with `412 Precondition Failed` if the ETag has changed.  Use `*` to skip the check. |
| `Prefer` | GET | `return=minimal` omits `ldp:contains` and membership triples from the body.  `return=representation` (default) includes them. |
| `Content-Type` | POST, PUT, PATCH | `text/turtle`, `application/ld+json`, `application/rdf+xml`, `application/sparql-update` (PATCH only), or any MIME type for binary resources. |

### Response headers

| Header | Applies to | Description |
|---|---|---|
| `ETag` | GET, HEAD, PUT, PATCH | SHA-256-based content hash, quoted string (e.g. `"a3f8…"`). |
| `Location` | POST | Full IRI of the newly created member resource. |
| `Link` | All | LDP type annotations (`rel="type"`) + `constrainedBy` rel (see below). |
| `Preference-Applied` | GET | Echoes `return=minimal` or `return=representation` to confirm the server processed the `Prefer` header. |
| `Vary` | GET | `Accept, Prefer` — tells caches the response varies on these headers. |
| `Allow` | OPTIONS | `GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS` |
| `Accept-Post` | OPTIONS | `text/turtle, application/ld+json` |
| `Accept-Patch` | OPTIONS | `application/sparql-update` |

### Constrained-By

Every response includes:
```
Link: <{base_url}/ldp/constraints>; rel="http://www.w3.org/ns/ldp#constrainedBy"
```

This points to the server's constraint document, advertising any additional constraints beyond LDP core.

---

## Pagination

Container GETs support pagination via query parameters:

```
GET /ldp/my-container/?page=1&page_size=50
```

| Parameter | Default | Max |
|---|---|---|
| `page` | `0` | — |
| `page_size` | `100` | `1000` |

When more members exist beyond the current page, the response includes a `Link: <…?page=N>; rel="next"` header.

---

## Direct Containers

A Direct Container writes a configurable membership triple to a designated resource (`ldp:membershipResource`) on every POST.

### Creating a Direct Container

```sparql
INSERT DATA {
  <http://localhost/ldp/dc/> a ldp:DirectContainer, ldp:RDFSource, ldp:Resource ;
    ldp:membershipResource <http://localhost/ldp/dc/> ;
    ldp:hasMemberRelation ldp:member .
}
```

Or use the Rust API:

```rust
container::ensure_direct_container(
    &store,
    "http://localhost/ldp/dc/",
    "http://localhost/ldp/dc/",   // membershipResource
    "http://www.w3.org/ns/ldp#member",
    None,                         // insertedContentRelation (None for Direct)
)?;
```

After each POST to `/ldp/dc/`, the triple `<membershipResource> ldp:member <new-member>` is automatically inserted.

---

## Indirect Containers

An Indirect Container reads the membership triple **object** from the new member's body via `ldp:insertedContentRelation`.

### Example

```sparql
INSERT DATA {
  <http://localhost/ldp/ic/> a ldp:IndirectContainer, ldp:RDFSource, ldp:Resource ;
    ldp:membershipResource <http://localhost/ldp/collection> ;
    ldp:hasMemberRelation  <http://example.org/hasBook> ;
    ldp:insertedContentRelation <http://example.org/bookIRI> .
}
```

When a resource is POSTed with body:
```turtle
<http://localhost/ldp/ic/entry1> <http://example.org/bookIRI> <http://books.org/isbn/978-3> .
```

The membership triple written is:
```
<http://localhost/ldp/collection> <http://example.org/hasBook> <http://books.org/isbn/978-3>
```

---

## Non-RDF Sources

Binary resources can be stored in any container.  POST a body with a non-RDF MIME type:

```bash
curl -X POST http://localhost:7878/ldp/images/ \
  -H "Content-Type: image/png" \
  -H "Slug: photo.png" \
  --data-binary @photo.png
```

The server stores the binary data as a base64-encoded triple internally and returns the original bytes on GET with the original `Content-Type`.

> **Note for large files:** For files larger than a few MB, use the dataset asset storage API (`POST /api/datasets/:id/assets`) which streams to the configured S3/MinIO backend rather than encoding in the triple store.

---

## PATCH with SPARQL Update

PATCH an RDF Source by sending a `application/sparql-update` body:

```bash
curl -X PATCH http://localhost:7878/ldp/my-resource \
  -H "Content-Type: application/sparql-update" \
  -d 'DELETE { <http://localhost/ldp/my-resource> <http://example.org/status> "draft" }
      INSERT { <http://localhost/ldp/my-resource> <http://example.org/status> "published" }
      WHERE {}'
```

The SPARQL Update executes against the full triple store (not just the resource's triples), so you can reference any named graph in the WHERE clause.

---

## JSON-LD

POST and PUT accept `application/ld+json` bodies:

```bash
curl -X POST http://localhost:7878/ldp/collection/ \
  -H "Content-Type: application/ld+json" \
  -H "Slug: item1" \
  -d '{"@id":"http://localhost/ldp/collection/item1","http://schema.org/name":[{"@value":"Item One"}]}'
```

---

## curl Examples

### Create a Basic Container

```bash
curl -X PUT http://localhost:7878/ldp/my-container/ \
  -H "Content-Type: text/turtle" \
  -d '<http://localhost/ldp/my-container/> a <http://www.w3.org/ns/ldp#BasicContainer> .'
```

### List container members

```bash
curl http://localhost:7878/ldp/my-container/
```

### Add a member

```bash
curl -X POST http://localhost:7878/ldp/my-container/ \
  -H "Content-Type: text/turtle" \
  -H "Slug: item1" \
  -d '@prefix ex: <http://example.org/> . <> ex:name "Item 1" .'
```

### List with minimal representation

```bash
curl -H "Prefer: return=minimal" http://localhost:7878/ldp/my-container/
```

### Replace with ETag

```bash
# Get current ETag
ETAG=$(curl -sI http://localhost:7878/ldp/my-container/item1 | grep -i etag | awk '{print $2}' | tr -d '\r')

curl -X PUT http://localhost:7878/ldp/my-container/item1 \
  -H "Content-Type: text/turtle" \
  -H "If-Match: $ETAG" \
  -d '<http://localhost/ldp/my-container/item1> <http://example.org/updated> true .'
```

### Delete a resource

```bash
curl -X DELETE http://localhost:7878/ldp/my-container/item1
```

---

## Limitations

- **`ldp:MemberSubject`** is not yet supported as a value for `ldp:insertedContentRelation`.
- **Access control** for LDP resources follows the triplestore's global RBAC (JWT/API key), not per-resource ACLs (LDP ACL extension is not implemented).
- **Transactions**: LDP operations are not atomic across multiple requests.  Use SPARQL Update transactions for multi-step changes.

---

## References

- [W3C LDP 1.0 Specification](https://www.w3.org/TR/ldp/)
- [W3C LDP Primer](https://www.w3.org/TR/ldp-primer/)
- [LDP Test Suite](https://w3c.github.io/ldp-testsuite/)
