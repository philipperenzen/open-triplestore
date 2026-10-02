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
closure), timestamped. An edit anywhere inside the blank-node closure is a
change; writing the same structure again under fresh blank-node labels (a
Graph Store `PUT` of unchanged Turtle) is not. An entity that disappears
becomes a *tombstone* member, typed `as:Delete` (the
`ldes:versionDeleteObject` the stream declares on `ldes:versionDeletePath
rdf:type`, LDES §4.3) and `ots:Tombstone` (the type earlier versions used).
Writes to datasets that have not enabled a stream cost nothing extra.

Timestamps never go backwards (LDES §4.1): a member is stamped with its
write's time or with the newest timestamp the stream has already published,
whichever is later, so two concurrent writes that read the clock in one order
and append in the other cannot put a member below a bound a client holds.
The newest published timestamp is kept on the stream, so it holds even after
retention removed that member.

The stream (`text/turtle`, `application/ld+json` or `application/n-triples`
by `Accept`; readable by whoever may read the dataset):

```
GET /api/datasets/<id>/ldes                 # the ldes:EventStream, tree:view → the root node
GET /api/datasets/<id>/ldes/nodes/0         # the root node: relations to every fragment
GET /api/datasets/<id>/ldes/nodes/<n>       # fragment n (from 1), page_size members
GET /api/datasets/<id>/ldes/members/<m>     # one member, dereferenced
```

```turtle
<…/api/datasets/assets/ldes> a ldes:EventStream ;
    ldes:timestampPath dct:created ;
    ldes:versionOfPath dct:isVersionOf ;
    ldes:versionDeletePath rdf:type ;
    ldes:versionDeleteObject as:Delete ;
    ldes:pollingInterval 60 ;
    tree:shape <…/api/datasets/assets/ldes#member-shape> ;
    tree:view <…/api/datasets/assets/ldes/nodes/0> .

<…/api/datasets/assets/ldes#member-shape> a sh:NodeShape ;
    sh:property [ sh:path dct:isVersionOf ; sh:minCount 1 ; sh:maxCount 1 ; sh:nodeKind sh:IRI ] ,
                [ sh:path dct:created ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:dateTime ] .

# the root node: a lower and an upper bound on each full fragment, a lower
# bound on the first fragment still filling
<…/api/datasets/assets/ldes/nodes/0> a tree:Node ;
    tree:relation [ a tree:GreaterThanOrEqualToRelation ; tree:path dct:created ;
                    tree:value "2026-09-02T09:58:12Z"^^xsd:dateTime ;
                    tree:node <…/api/datasets/assets/ldes/nodes/1> ] ,
                  [ a tree:LessThanOrEqualToRelation ; tree:path dct:created ;
                    tree:value "2026-09-02T09:59:40Z"^^xsd:dateTime ;
                    tree:node <…/api/datasets/assets/ldes/nodes/1> ] ,
                  [ a tree:GreaterThanOrEqualToRelation ; tree:path dct:created ;
                    tree:value "2026-09-02T10:00:00Z"^^xsd:dateTime ;
                    tree:node <…/api/datasets/assets/ldes/nodes/2> ] .

# a fragment
<…/api/datasets/assets/ldes> tree:member <…/api/datasets/assets/ldes/members/17> .
<…/api/datasets/assets/ldes/members/17>
    dct:isVersionOf <https://example.org/layered/asset/b1> ;
    dct:created "2026-09-02T09:58:12Z"^^xsd:dateTime ;
    a ex:Bridge ; ex:name "Riverside Bridge" ; ex:status exd:in-service .
```

Members are version objects: the member IRI carries the entity's properties
at that moment, `dct:isVersionOf` names the entity, `dct:created` orders the
stream. Every member conforms to the stream's `tree:shape` (one IRI
`dct:isVersionOf`, one `xsd:dateTime` `dct:created`; open to the entity's own
properties). Blank nodes are labelled per member, so two versions of one
entity on a page never share a blank node.

**The search tree** is the one the LDES Server Primer §4 recommends: one root
node with two relations to each full fragment — the earliest and the latest
`dct:created` on it, recorded when it filled — and one relation, lower
bound only, to the first fragment still filling. Full fragments are
immutable (`<node> ldes:immutable true` and `Cache-Control: public,
max-age=31536000, immutable`) and link nowhere, so the bounds on each describe
everything reachable through it (TREE §3) and a client can skip a fragment
whose upper bound is before its bookmark. The fragments still filling chain
forward with lower bounds; only they and the root change. The root lists two
relations per full fragment, so it grows with the stream (about 2 000
relations for 100 000 members at the default page size); a second level is
the documented way to split it when that matters.

Every document carries an `ETag`, and a request whose `If-None-Match` matches
it is answered `304 Not Modified` (Server Primer §2, LDES §3.3).
`ldes:pollingInterval` (default 60 seconds) is set with `"polling_interval"`
in the same `PUT`. When a stream's documents are all being rendered — at most
16 at once per stream and 64 across the server, set with
`OTS_LDES_MAX_IN_FLIGHT_PER_STREAM` and `OTS_LDES_MAX_IN_FLIGHT` — further
requests are answered `429 Too Many Requests` with `Retry-After: 1` (Server
Primer §2) instead of queueing.

Streams published before this layout had sealed fragments link onward to the
next one. Clients that cached those immutable copies keep the stale forward
link, which is harmless: they reach the same members, and a fresh fetch
through the root gives the bounded tree.

## Retention

By default a stream keeps every member. A retention policy
([LDES 1.0 §4.4](https://w3id.org/ldes/specification#retention-policies)) is
declared with the same `PUT`, and enforced from then on:

```bash
curl -X PUT http://localhost:7878/api/datasets/<id>/ldes \
  -H "Authorization: Bearer <token>" -H 'Content-Type: application/json' \
  -d '{"enabled": true, "retention": {
        "full_log_duration": "P30D", "version_amount": 2,
        "version_delete_duration": "P7D"}}'
```

| Field | Property | Meaning |
|---|---|---|
| `full_log_duration` | `ldes:fullLogDuration` | every member created within this long ago is kept, whatever else says |
| `version_amount` | `ldes:versionAmount` | beyond that window, the newest N versions of each entity are kept |
| `version_duration` | `ldes:versionDuration` | … but only this long (default: for ever); needs `version_amount` |
| `version_delete_duration` | `ldes:versionDeleteDuration` | tombstones are kept this long, after which a deleted entity's history is gone |
| `starting_from` | `ldes:startingFrom` | nothing created before this instant is kept |

A member is kept while *any* rule keeps it; `starting_from` cuts regardless.
Durations are the `PnYnMnDTnHnMnS` subset of `xsd:duration`; for the sweep a
year is 365 days and a month 30 (the declared lexical form is published as
is). `"retention": {}` clears the policy; omitting it leaves it alone.

The policy is published on the root node — the `tree:view` target — as an IRI
whose description travels with every page that names it:

```turtle
<…/ldes> tree:view <…/ldes/nodes/0> .
<…/ldes/nodes/0> a ldes:EventSource ;
    ldes:retentionPolicy <…/ldes#retention> .
<…/ldes#retention> a ldes:RetentionPolicy ;
    ldes:fullLogDuration "P30D"^^xsd:duration ;
    ldes:versionAmount 2 ;
    ldes:versionDeleteDuration "P7D"^^xsd:duration .
```

**What pruning can and cannot do to a page.** Once a fragment is full, its
member→node assignment is frozen (the id range is recorded when the next
member arrives). Retention only ever deletes members *inside* a frozen range:
a page served as immutable can lose members, but never gains one, never hands
one to another page and never changes the bounds the root carries for it —
so a cached copy is a superset of the live page, which is exactly what a
consumer of a retention-declaring stream must expect. A frozen page whose
members are all gone answers **`410 Gone`**, and the root no longer links it.
The last page is never frozen and never 410. Members are kept five minutes longer than declared, so a
consumer's own clock skew never finds the server stricter than its policy.

The sweep runs when the policy is set and after writes to the dataset (at
most once a minute per dataset; `OTS_LDES_SWEEP_INTERVAL_SECS` changes that).
A stream nobody writes to keeps its members past the window, which is the
safe direction: a server may retain more than it declares, never less.
Declaring a policy without enforcing it is harmless; the reverse — removing
members a consumer was told would be there — is the violation, and it cannot
happen here because nothing is removed before a policy is declared.

Not supported: `ldes:versionKey` (compound keys over arbitrary property
paths — the member log has one entity per member) and the discouraged pre-1.0
classes (`ldes:DurationAgoPolicy`, `ldes:LatestVersionSubset`) on the
publishing side; the client reads both forms.

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

A fragment that answers `410 Gone` — compacted away by the publisher's
retention policy — is processed as an empty page, not as a failure (LDES
§3.3); the report counts them in `nodes_gone`. The report also carries the
publisher's declared policy (`retention_policy`, read from the root node in
either the 1.0 or the legacy form) and a warning when the sync's bookmark is
older than the publisher's full-log window, because members created between
the two may have been compacted before this mirror saw them.

Outbound requests are subject to the remote allowlist: the stream's origin
must be listed in `OTS_REMOTE_ALLOWLIST`, and each fetch has the usual timeout and
body limit (`OTS_REMOTE_TIMEOUT_SECS`, `OTS_REMOTE_MAX_BYTES`).

## What is and is not implemented

- Fragmentation: time-ordered, fixed-size pages under one root node with
  `tree:GreaterThanOrEqualToRelation` and `tree:LessThanOrEqualToRelation` on
  `dct:created`, frozen once full. No geospatial or substring fragmentations
  (optional TREE views), no `ldes:sequencePath` (members with equal
  timestamps are ordered by member id only).
- Publisher context: `ldes:timestampPath`, `ldes:versionOfPath`, the delete
  path and object, `ldes:pollingInterval`, a generated `tree:shape`, `ETag` /
  `304`, `429` when busy, dereferenceable member IRIs. No transactions.
- Retention: `ldes:fullLogDuration`, `ldes:versionAmount`,
  `ldes:versionDuration`, `ldes:versionDeleteDuration`, `ldes:startingFrom`,
  enforced by in-place deletion inside frozen pages, `410 Gone` for a page
  emptied by it. Not `ldes:versionKey`.
- Members: entity-level version objects (IRI subjects; blank-node closure
  included). Triple-level changes to an entity produce a full new version of
  it, not a delta.
- Client: follows any TREE relation (all are treated as "worth following"),
  honours the stream's declared `ldes:timestampPath` / `ldes:versionOfPath`
  (defaults `dct:created` / `dct:isVersionOf`), treats `410 Gone` as an empty
  page, recognises deletes typed `as:Delete` or `ots:Tombstone`, keeps the
  publisher's retention policy as context, and materialises the newest
  version per entity. It does not evaluate relation values to prune pages,
  and does not yet read a stream's declared delete path and object.

The spec rules the implementation is held to are in
`tests/ldes_conformance.rs`, one assertion per clause of the LDES
specification, its Server Primer §6 and TREE. There is no LDES or TREE test
corpus to vendor — the specs ship prose and a vocabulary — so those are
derived rules, not an external oracle.

Nothing here is specific to a domain: an asset registry, a patient register
or a vocabulary publish and sync the same way.
