# Linked Data Event Streams (LDES)

An [LDES](https://w3id.org/ldes/specification) publishes a dataset as an
append-only stream of immutable *version objects*, fragmented with
[TREE](https://w3id.org/tree/specification) hypermedia, so a client can fetch
the whole history once and then only the increments — without the publisher
knowing who is consuming. Open Triplestore can be both ends: any dataset can
publish a stream, and any dataset can sync one from elsewhere.

## Publishing a dataset as a stream

Enable the stream (write access on the dataset):

```bash
curl -X PUT http://localhost:7878/api/datasets/<id>/ldes \
  -H "Authorization: Bearer <token>" -H 'Content-Type: application/json' \
  -d '{"enabled": true, "page_size": 100}'
```

Enabling publishes every entity currently in the dataset's graphs as a first
member. From then on every write — Graph Store `PUT`/`POST`/`DELETE`, SPARQL
Update, bulk import, version restore, an LDES sync into it — is compared with
the state before it, per entity (IRI subject), and each changed entity becomes
a new member: its current description (direct triples plus blank-node
closure), timestamped. An entity that disappears becomes a *tombstone* member
(`ots:Tombstone`). Writes to datasets that have not enabled a stream cost
nothing extra.

The stream (`text/turtle`, `application/ld+json` or `application/n-triples`
by `Accept`; readable by whoever may read the dataset):

```
GET /api/datasets/<id>/ldes                 # the ldes:EventStream, tree:view → first node
GET /api/datasets/<id>/ldes/nodes/<n>       # fragment n (1-based), page_size members
```

```turtle
<…/api/datasets/assets/ldes> a ldes:EventStream ;
    ldes:timestampPath dct:created ;
    ldes:versionOfPath dct:isVersionOf ;
    tree:view <…/api/datasets/assets/ldes/nodes/1> .

<…/api/datasets/assets/ldes/nodes/1> a tree:Node ;
    tree:relation [ a tree:GreaterThanOrEqualToRelation ;
                    tree:path dct:created ;
                    tree:value "2026-09-02T10:00:00Z"^^xsd:dateTime ;
                    tree:node <…/api/datasets/assets/ldes/nodes/2> ] .

<…/api/datasets/assets/ldes> tree:member <…/api/datasets/assets/ldes/members/17> .
<…/api/datasets/assets/ldes/members/17>
    dct:isVersionOf <https://example.org/layered/asset/b1> ;
    dct:created "2026-09-02T09:58:12Z"^^xsd:dateTime ;
    a ex:Bridge ; ex:name "Waalbrug" ; ex:status exd:in-service .
```

Members are version objects: the member IRI carries the entity's properties
at that moment, `dct:isVersionOf` names the entity, `dct:created` orders the
stream. Full fragments are immutable and served with a long `Cache-Control`;
only the last one changes.

## Syncing a stream into a dataset

```bash
curl -X POST http://localhost:7878/api/ldes/sync \
  -H "Authorization: Bearer <token>" -H 'Content-Type: application/json' \
  -d '{"url": "https://other.example.org/api/datasets/roads/ldes",
       "dataset_id": "roads-mirror", "graph_iri": "https://example.org/roads-mirror/instances"}'
```

The server follows the stream's `tree:view` and every `tree:relation`,
collects the members, keeps the newest version of each entity, and writes each
entity's current description into the target graph (replacing what it held for
that entity; tombstones delete it). The sync remembers the newest timestamp it
applied per `(dataset, url)`, so running it again applies only newer members.
Every sync is a commit in the dataset's history.

Outbound requests are subject to the remote allowlist: the stream's origin
must be listed in `OTS_REMOTE_ALLOWLIST`, and each fetch has the usual timeout.

## What is and is not implemented

- Fragmentation: time-ordered, fixed-size pages with
  `tree:GreaterThanOrEqualToRelation` on `dct:created`. No geospatial or
  substring fragmentations, no `tree:shape`, no retention policies
  (`ldes:retentionPolicy`) — the stream keeps every member.
- Members: entity-level version objects (IRI subjects; blank-node closure
  included). Triple-level changes to an entity produce a full new version of
  it, not a delta.
- Client: follows any TREE relation (all are treated as "worth following"),
  honours the stream's declared `ldes:timestampPath` / `ldes:versionOfPath`
  (defaults `dct:created` / `dct:isVersionOf`), and materialises the newest
  version per entity. It does not evaluate relation values to prune pages.

Nothing here is specific to a domain: an asset registry, a patient register
or a vocabulary publish and sync the same way.
