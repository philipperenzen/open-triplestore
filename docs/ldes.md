# Linked Data Event Streams (LDES)

An [LDES](https://w3id.org/ldes/specification) publishes a dataset as an
append-only stream of immutable *version objects*, fragmented with
[TREE](https://w3id.org/tree/specification) hypermedia, so a client can fetch
the whole history once and then only the increments — without the publisher
knowing who is consuming. Open Triplestore can be both ends: any dataset can
publish a stream, and any dataset can sync one from elsewhere.

## Publishing a dataset as a stream

In the web UI, the dataset page's **Exchange** section has the stream's
settings: publishing on or off, members per page (the fragmentation),
the polling interval and the retention policy, with the stream URL, the member
count and the number of sealed pages. Readers see the settings; writers can
change them. The form reads `GET /api/datasets/<id>/ldes/config`, a JSON view
of the same settings (`enabled`, `configured`, `page_size`,
`polling_interval`, `retention`, `members`, `sealed_nodes`, `created_at`,
`stream`, `can_write`), and saves with the `PUT` below.

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

How a write is compared depends on the [change log](/docs/versioning). With
it on, the write's own rows name what changed: their IRI subjects, plus the
entity a changed blank-node subtree hangs from (walked up at most
`OTS_LDES_BNODE_DEPTH`, 8, levels) — no graph is scanned. A graph whose rows
do not carry their quads (a change over the row cap, or one the log could not
bound), or with more than `OTS_LDES_BNODE_SUBJECTS_MAX` (10 000) blank-node
subjects in one write, is republished whole. With the log off, every tracked
graph is indexed before and after the write and the two compared, as
before. Capturing from the rows costs 5–15 ms where the two indexings cost
0.7–3.3 s ([performance.md](performance.md#delta-versions-at-scale--change-capture-on-09m-to-9m-quads-2026-10-15),
item 11). A change to a blank node more than `OTS_LDES_BNODE_DEPTH` levels
below its entity is **not published**: the walk stops without an owner. An
RDF list needs its length plus one; raise the setting for data with long
lists or deep trees (each level is one index lookup).

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

The publisher does not write the discouraged pre-1.0 classes
(`ldes:DurationAgoPolicy`, `ldes:LatestVersionSubset`,
`ldes:PointInTimePolicy`); the client reads them as well as the 1.0
properties. (`ldes:versionKey`, which earlier drafts had, is not in the
LDES 1.0.0 vocabulary.)

## Syncing a stream into a dataset

```bash
curl -X POST http://localhost:7878/api/ldes/sync \
  -H "Authorization: Bearer <token>" -H 'Content-Type: application/json' \
  -d '{"url": "https://other.example.org/api/datasets/roads/ldes",
       "dataset_id": "roads-mirror", "graph_iri": "https://example.org/roads-mirror/instances"}'
```

Admins manage these sync jobs in **Admin → Event stream sync**: every
`(dataset, url)` sync with its target graph, bookmark, members applied, the
last run's time and status, its error when it failed, its last report, and
its schedule with the next run; a form adds a stream (and runs it), and each
job can be run again, scheduled or forgotten. `GET /api/admin/ldes/syncs`
lists the jobs (`last_status` `ok` or `error`, `last_error`, `last_report`,
and the schedule fields of [Scheduled sync](#scheduled-sync)); a failed run is
recorded too and keeps the previous bookmark, so the next run resumes from it.
`DELETE /api/admin/ldes/syncs?dataset_id=…&url=…` forgets a job and its client
state; the mirrored graph stays, and the next sync from that source reads the
whole stream again.

The `url` may be the event stream, its root node, a redirect to either, or a
page with exactly one `tree:view` (LDES 1.0 §3.1); the report names the
`stream` and `root_node` it found and the stream's `polling_interval` — the
`ldes:pollingInterval` it declares, which an `auto` schedule waits between
runs ([Scheduled sync](#scheduled-sync)). The client
reads the stream's context from the root node, follows each page's
`tree:relation`s, and extracts every member: its triples in the default graph,
every quad in the graph named after it, and the blank nodes they reach
(§3.4). It keeps the newest version of each entity and writes that entity's
description into the target graph, replacing exactly what the entity's
previous version wrote there (its subjects and their blank nodes); a delete
removes it. Every sync is a commit in the dataset's history.

How a member maps to an entity and its version follows the stream's
declarations (§4.3), each a SHACL property path:

- `ldes:versionOfPath` names the entity; without it every member is its own
  entity (a log of immutable members, like sensor observations).
- The version is ordered by `ldes:versionTimestampPath` (else
  `ldes:timestampPath`), then `ldes:versionSequencePath` (else
  `ldes:sequencePath`), so a version published out of order, older than one
  already applied, is not applied (`versions_superseded`).
- `ldes:versionDeletePath` / `ldes:versionDeleteObject` mark deletes,
  `…CreatePath` / `…CreateObject` creates (added without removing what the
  entity held), `…UpdatePath` / `…UpdateObject` updates; each path defaults to
  `rdf:type`. A stream that declares no delete object still has its members
  typed `ots:Tombstone` treated as deletes (this project's publishers before
  LDES 1.0).
- When the member IRI names a graph with quads in it, that graph is the
  entity's description and the member's own triples are version metadata.
  Otherwise the member's triples are re-subjected to the entity, without the
  stream's version properties and markers.

The sync remembers, per `(dataset, url)`:

- a bookmark — the newest member timestamp it saw, compared as an
  `xsd:dateTime` instant (no timezone is read as UTC) — and the members that
  carry exactly that timestamp, so a member published later with the same
  timestamp is still emitted and none is emitted twice (§3.2);
- the pages it processed as immutable (`<page> ldes:immutable true` or
  `Cache-Control: immutable`), which it never fetches again
  (`nodes_skipped_immutable`);
- the `ETag` and relations of each mutable page: the next run sends
  `If-None-Match`, and a `304` follows the remembered relations
  (`nodes_not_modified`);
- the version and subjects it last applied per entity.

A node whose relations, on the stream's timestamp path, bound every member it
can hold below the bookmark (`tree:LessThanRelation` at or before it,
`tree:LessThanOrEqualToRelation` before it) is not fetched (`nodes_pruned`).
The root node is fetched in full every run, since it carries the context.

A fragment that answers `410 Gone` — compacted away by the publisher's
retention policy — is processed as an empty page, not as a failure (LDES
§3.3); the report counts them in `nodes_gone`. `408`, `425`, `429`, `500`,
`502`, `503` and `504` are retried with exponential back-off and jitter, or
after the wait a `Retry-After` header asks for (`OTS_LDES_RETRIES`, default 4;
`OTS_LDES_MAX_RETRY_WAIT_SECS`, default 60: a longer `Retry-After` fails
the sync rather than holding it); `retries` counts them. Any other error
status aborts the sync. The client asks for TriG, N-Quads, Turtle, N-Triples
and JSON-LD.

The report also carries the publisher's declared policy (`retention_policy`,
read from the root node in the 1.0 form or a legacy one, including
`ldes:PointInTimePolicy` as `starting_from`) and a warning when the sync's
bookmark is older than the publisher's window (its full-log duration or its
starting point), because members created between the two may never reach this
mirror.

Outbound requests are subject to the remote allowlist: the stream's origin
must be listed in `OTS_LDES_ALLOWLIST`, a redirect is followed only to a URL
that list covers too (§3.3: "A client MUST follow redirects"; the URL after
the redirects is the page's base IRI), and each fetch has the usual timeout and
body limit (`OTS_REMOTE_TIMEOUT_SECS`, `OTS_REMOTE_MAX_BYTES`).

### Scheduled sync

A sync job runs on its own once it has a schedule. Every job starts `off`,
existing ones included: it runs when someone runs it. Set the schedule in
**Admin → Event stream sync**, or:

```bash
curl -X PUT 'http://localhost:7878/api/admin/ldes/syncs/schedule?dataset_id=roads-mirror&url=https%3A%2F%2Fother.example.org%2Fapi%2Fdatasets%2Froads%2Fldes' \
  -H "Authorization: Bearer <admin token>" -H 'Content-Type: application/json' \
  -d '{"schedule": "auto"}'
```

- `auto` waits the stream's `ldes:pollingInterval` between runs. The LDES
  vocabulary defines it as "the number of seconds the client should keep
  between synchronization run calls", and LDES 1.0 §3.1 says a client "SHOULD
  check whether an ldes:pollingInterval was set". The interval used is the one
  the stream declared at the job's last successful run; a stream that
  declares none is read every 60 seconds.
- `interval`, with `interval_secs`, waits a fixed number of seconds: at least
  60, at most a year.
- No schedule runs a job more often than once a minute, whatever the stream
  declares.

The next run is due an interval after the last one, whoever started it, so
"run now" moves it too; the list shows it as `next_run_at` (**Next run** on
the page). A failed scheduled run is recorded like any other (`last_status`
`error`, `last_error`, the bookmark kept) and doubles the wait before the
next: twice the interval after one failure, four times after two, at most six
hours (or the interval, when that is longer). `consecutive_failures` counts
them, and the first success resets it.

A scheduled run acts as the job's owner — the user who first ran it,
`created_by` — with their federated identity, as their own request would.
Their authority is checked at every run, the way `POST /api/ldes/sync` checks
a caller's: the account still exists and is active, may still write the
dataset, and the target graph still passes the dataset's graph gate. When it
does not, nothing runs and the job is paused: `paused_at` is set,
`next_run_at` is cleared and `last_error` says why ("schedule paused: its
owner … may no longer write dataset …"). Setting the schedule again, to
anything, resumes the job and forgets its failures. To hand a job over to
another account, forget it and add it again as that account; its first run
reads the whole stream again.

At most `OTS_LDES_SYNC_CONCURRENCY` scheduled runs go at once (default 2), on
the server's blocking pool; a due job waits for a free one. Two runs of one
job never overlap: "run now" waits for a scheduled run of the same job, and
the scheduler skips a job that is running. The server looks for due jobs
every 10 seconds. A scheduled run's commit in the dataset's history reads
"Scheduled LDES sync from …".

Only the node that writes runs scheduled jobs. A replication follower holds a
copy of every job, because the identity database is shipped to it, but its
store is read-only, so it runs none; in a Raft cluster only the member that
leads at the time runs them, and a newly elected leader carries on from each
job's `next_run_at`. `OTS_OFFLINE` does not stop scheduled runs: it is about
downloads at startup, and what a sync may read is decided by
`OTS_LDES_ALLOWLIST`, at every run.

## What is and is not implemented

- Fragmentation: time-ordered, fixed-size pages under one root node with
  `tree:GreaterThanOrEqualToRelation` and `tree:LessThanOrEqualToRelation` on
  `dct:created`, frozen once full. No geospatial or substring fragmentations
  or search forms — optional TREE views, not required by LDES 1.0 — and no
  `ldes:sequencePath` (members with equal timestamps are ordered by member id
  only).
- Publisher context: `ldes:timestampPath`, `ldes:versionOfPath`, the delete
  path and object, `ldes:pollingInterval`, a generated `tree:shape`, `ETag` /
  `304`, `429` when busy, dereferenceable member IRIs. No transactions.
- Retention: `ldes:fullLogDuration`, `ldes:versionAmount`,
  `ldes:versionDuration`, `ldes:versionDeleteDuration`, `ldes:startingFrom`,
  enforced by in-place deletion inside frozen pages, `410 Gone` for a page
  emptied by it.
- Members: entity-level version objects (IRI subjects; blank-node closure
  included). Triple-level changes to an entity produce a full new version of
  it, not a delta.
- Client (unordered mode, LDES 1.0 §3): §3.1 initialisation; redirects
  within the allowlist; retries with back-off on 408/425/429/5xx; every RDF
  format the spec lists; §3.4 member extraction including named graphs;
  declared paths evaluated as SHACL property paths; version, create, update
  and delete semantics from the stream's declarations; a bookmark on
  `xsd:dateTime` values that keeps the members at its own timestamp;
  immutable pages fetched once, ETags kept, nodes pruned by relation bounds;
  `410 Gone` as an empty page; the retention policy, legacy classes included,
  kept as context; scheduled runs that wait the stream's
  `ldes:pollingInterval` (or a fixed interval), never less than a minute. The
  run's end is its finalisation: entities are written when every page has
  been read. Not implemented, and not required: an ordered mode (a priority
  queue over relation values), transactions (`ldes:transactionPath`), and
  source selection.

The spec rules the implementation is held to are in
`tests/ldes_conformance.rs`, one assertion per clause of the LDES
specification, its Server Primer §6 and TREE. There is no LDES or TREE test
corpus to vendor — the specs ship prose and a vocabulary — so those are
derived rules, not an external oracle.

Nothing here is specific to a domain: an asset registry, a patient register
or a vocabulary publish and sync the same way.
