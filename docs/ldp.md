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

### Resources that do not exist

A resource exists once an LDP write created it (`PUT`, `POST`, or the
container a `PUT` lands in); the root container `/ldp/` always exists. A `GET`
or `HEAD` on any other path — never created, deleted, an intermediate path
such as `/ldp/a/` when only `/ldp/a/b/c` was written, or an IRI that other
triples merely mention — answers `404 Not Found`. The 404 still carries the
discovery headers: `Link: <…/path.acl>; rel="acl"`, the `constrainedBy` link
and `WAC-Allow` with the modes the caller would hold on that path (inherited
from the nearest container ACL), so a client can find the ACL that governs the
path and tell whether it may create it with `PUT`. A caller without
`acl:Read` on the path gets `403` whether or not the resource exists, so a 404
never tells a stranger what is there. `GET` on an ACL a resource does not have
is a 404 that links the ACL to itself, which is where a `PUT` creates it.

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
| `Link` | All | LDP type annotations (`rel="type"`), the `constrainedBy` rel (see below) and the resource's access control list (`rel="acl"`, see [Access control](#access-control)). |
| `WAC-Allow` | GET, HEAD | `user="read write append control", public="read"` — the access modes the caller and the public hold on the resource, on a `404` too (see [Resources that do not exist](#resources-that-do-not-exist)). |
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

The update is confined to the resource: it may be `INSERT DATA`, `DELETE DATA` or `DELETE/INSERT … WHERE` on the default graph (no `GRAPH`, `LOAD`, `CLEAR`, `CREATE`, `DROP`, `SERVICE`, `WITH`, `USING`, nor a `GRAPH` or `SERVICE` inside an `EXISTS`), it is evaluated against the resource's own triples only (the `WHERE` clause sees nothing else in the store), and the result may not describe another resource under `/ldp/` or touch a server-managed triple (`ldp:*`, `rdf:type ldp:*`). Anything else is refused with `403` and nothing changes; a syntax error is a `400`. Triples about subjects outside `/ldp/` are allowed, as they are in a `PUT` body. Triples whose object is a blank node are not visible to a `PATCH` and cannot be removed by one; replace the resource with `PUT` instead. A relative `<>` in the body is the resource.

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

## Access control

Every LDP resource has its own access control list, in the [Web Access Control](https://solidproject.org/TR/wac) (WAC) model Solid servers and clients use: `acl:Authorization` nodes with `acl:accessTo`, `acl:default`, an agent and `acl:mode`. The ACL of a resource `R` is the LDP resource `R.acl` (`C.acl` for a container `C`; `C/.acl` names the same ACL), advertised on every response with `Link: <R.acl>; rel="acl"`. `GET`/`HEAD` show `WAC-Allow` with the caller's and the public's modes.

**What each verb needs.**

| Request | Mode |
|---|---|
| `GET`, `HEAD` on `R` | `acl:Read` on `R` |
| `PUT` replacing `R`, `PATCH`, `DELETE` | `acl:Write` on `R` |
| `POST` to a container, `PUT` creating a resource | `acl:Append` (or `acl:Write`) on the container |
| `GET`, `PUT`, `DELETE` on `R.acl` | `acl:Control` on `R` |

Admins pass every check. An API token without write scope still cannot write, and endpoint ACL rules still apply; WAC sits behind them. `acl:Write` satisfies `acl:Append`; nothing else is implied. If the memberships or the ACL cannot be looked up, the request is refused (`403`).

**Inheritance.** A resource without an ACL of its own follows the nearest container whose ACL has an authorization with `acl:default <container>`, up to the root `/ldp/`. Writing `R.acl` replaces the inherited policy for `R` (and, for a container, through its own `acl:default`, for everything under it that has no ACL of its own); deleting `R.acl` restores inheritance.

**Ownership.** Whoever creates a resource (`POST`, or a `PUT` that creates, the containers it brings into being included) gets `acl:Read`, `acl:Write` and `acl:Control` on it, recorded as the server-managed node `R.acl#owner`. It is shown by `GET R.acl`, cannot be written (`<#owner>` in a `PUT` body is a `400`) and goes away with the resource: whoever recreates the path owns it. The owner grant is additive: creating a resource does not change who else may reach it; only writing its `.acl` does. A container's owner reaches its members the same way, until a member gets an ACL of its own. The root container has no owner.

**Agents.** Authorizations name this store's principals by stable IRIs, so an install that changes its base URL keeps its ACLs:

| Principal | IRI | Predicate |
|---|---|---|
| a user | `urn:ots:user:{user_id}` | `acl:agent` |
| an organisation | `urn:ots:org:{org_id}` | `acl:agentGroup` |
| a group | `urn:ots:group:{group_id}` | `acl:agentGroup` |
| a role | `urn:ots:role:{admin\|super_admin\|user\|guest}` | `acl:agentClass` |
| any signed-in user | `acl:AuthenticatedAgent` | `acl:agentClass` |
| anyone, anonymous included | `foaf:Agent` | `acl:agentClass` |

`GET /api/auth/me` returns the caller's IRI as `agent_iri`. A `foaf:Agent` `acl:Read` grant makes a resource readable without a token (`GET`/`HEAD` only); everything else under `/ldp/` still requires authentication.

**Writing an ACL.** `PUT R.acl` with Turtle, JSON-LD or RDF/XML; the body is validated whole and either replaces the resource's own authorizations or is refused with `400`:

```turtle
@prefix acl: <http://www.w3.org/ns/auth/acl#> .

<#team> a acl:Authorization ;
  acl:accessTo </ldp/project/> ; acl:default </ldp/project/> ;
  acl:agentGroup <urn:ots:group:3f9c…> ;
  acl:mode acl:Read, acl:Write .

<#public> a acl:Authorization ;
  acl:accessTo </ldp/project/> ;
  acl:agentClass <http://xmlns.com/foaf/0.1/Agent> ;
  acl:mode acl:Read .
```

Every node must be named under the ACL resource (`<#name>`), be typed `acl:Authorization`, have `acl:accessTo` (and/or, for a container, `acl:default`) equal to the resource the ACL governs, at least one agent of the shapes above, at least one of the four modes, and nothing else. `DELETE R.acl` removes the resource's own authorizations (not the owner grant); `POST` and `PATCH` on a `.acl` are `405`. Names ending in `.acl` are reserved.

**The root ACL.** At first start (or at the first LDP request, whichever comes first) the server seeds `/ldp/.acl`, recorded with a `dcterms:created` so it is seeded once, with:

- `LDP_ROOT_ACL=open` (default): `acl:AuthenticatedAgent` gets `acl:Read`, `acl:Write` and `acl:Append` on `/ldp/` and, through `acl:default`, on everything under it; the `admin` and `super_admin` roles get every mode. This is the behaviour of releases before WAC, written where an admin can change it; the server logs one line at start while the root is open this way.
- `LDP_ROOT_ACL=owners`: only the two admin roles. Users reach only what they create or are granted.

To tighten an open install, an admin `PUT`s `/ldp/.acl` without the `#authenticated` node (keep the admin nodes, or an equivalent). Owners keep what they created; anything else becomes unreachable for non-admins until it is granted.

**What is protected, and what is not.** Every LDP verb, `.acl` reads and writes, and the `PUT`/`POST`/`PATCH` bodies: a body may describe the target and things outside `/ldp/`, not another resource under `/ldp/` (refused with `403`), and it cannot name a graph, so it can never plant an authorization. Authorizations live in the system graph `urn:system:ldp-acl`, which is not reachable over `/sparql` or the Graph Store Protocol. Out of scope: WebID-TLS, Solid-OIDC and any external identity (agents are this store's users, organisations, groups and roles); `acl:origin` / `acl:trustedApp`; access control for anything outside `/ldp/`. LDP resources still live in the default graph, so a principal with write access to the default graph through another protocol (a Graph Store write with no `?graph`, a SPARQL Update) is not bound by WAC; per-graph rules for those are in [security.md](security.md).

---

## Limitations

- **`ldp:MemberSubject`** is not yet supported as a value for `ldp:insertedContentRelation`.
- **Transactions**: LDP operations are not atomic across multiple requests.  Use SPARQL Update transactions for multi-step changes.
- **Access control** applies to `/ldp/` only, with this store's principals as agents; see [Access control](#access-control) for what is out of scope.

---

## References

- [W3C LDP 1.0 Specification](https://www.w3.org/TR/ldp/)
- [W3C LDP Primer](https://www.w3.org/TR/ldp-primer/)
- [LDP Test Suite](https://w3c.github.io/ldp-testsuite/)
