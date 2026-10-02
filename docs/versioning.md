# Dataset Versioning & Sharing

> **Scope.** This page covers **dataset/artifact versioning** — the lifecycle of the RDF *data* you store (draft → staged → published → deprecated). For **code-release** versioning — how the Open Triplestore software itself is versioned, branched, and released — see [Release Process](release-process.md). The two are deliberately separate and should not be conflated.

Datasets can be snapshotted into immutable versions, organised on branches, and shared with people who do not have an account. Versions follow the same **draft → staged → published** lifecycle as the registries, plus **deprecate** and **restore**.

## Version lifecycle

1. **Snapshot** — Create a version from the dataset's current graphs. The version is frozen and independently queryable: a *checkpoint* copies the live data into dedicated snapshot graphs; with the [change log](#change-log) on, later versions are *deltas* that record the change since a checkpoint instead of copying (see [Delta versions](#delta-versions)).
2. **Stage** — Mark a draft as *staged* for review before it goes live.
3. **Publish** — Mark a version *published*. The published version is what saved-query APIs serve by default.
4. **Deprecate / Restore** — Retire an old version, or restore a deprecated one if you need to roll back.

Each version records who created it, an optional note, and its source-graph mapping. Download any version's data (content-negotiated, defaulting to TriG) at `/api/datasets/{id}/versions/{version}/data`.

## Delta versions

Without the change log every version is a full physical copy of the graphs
it covers, so a dataset re-imported N times kept N copies. With the
[change log](#change-log) on (`OTS_CHANGE_CAPTURE=on`), the first version of
a dataset is a **checkpoint** — a copy, as before — and the versions after it
are **deltas**: a base checkpoint, a range of the change log and a content
hash. A delta copies nothing when it is cut; it reads exactly like the
checkpoint it replaces.

| | Checkpoint | Delta |
|---|---|---|
| Cut | copies the covered graphs into the version's snapshot graphs | records `base_version` and `patch_range` (`epoch`, `from_seq`, `to_seq`) |
| Read | as it is | built on first read into the same snapshot-graph IRIs a checkpoint has, then served like one |
| `kind` in the API | `checkpoint` | `delta` |

**When a cut is a delta.** The change log is recording, the version is not
Published (a published version is always physical, so it is in every
backup), you did not ask for `"checkpoint": true`, none of the graphs is
registered to another dataset too, and an *anchored* checkpoint of the
dataset covers the graphs with an intact chain of change rows from it to now.
A checkpoint is *anchored* when no write touched its graphs while it was being
copied, which pins its content to one sequence number of the log. Otherwise
the cut is a checkpoint, and the response says why in `checkpoint_reason` —
for example `change capture is off`, or `sequence 412 changed <…> by more
quads than a row carries`. To keep chains short, a cut is also a checkpoint
once `OTS_CHECKPOINT_EVERY` (100) deltas chain from the same base, or when
the changes since the base exceed `OTS_CHECKPOINT_FRACTION` (0.25) of its
size (and more than 1 000 quads).

**Building a delta.** The first read of a delta's data — `…/versions/:ver/data`,
a `versions=` pin in browse, a dataset service's `?version=`, a saved query
pinned to it, its tests — builds its graphs, forward from the base checkpoint
or backward from the live graphs, whichever is cheaper. A build is a store
write, so it takes a slot of the expensive-operation pool, answers 503 when
it outlasts the write timeout (it carries on in the background), and is
refused with **507** when it would hold more than `OTS_MATERIALISATION_MAX_QUADS`
(2 000 000) quads. **An anonymous caller never triggers a build**: it gets
503 with `Retry-After` until someone signed in has read the version (the
versions anonymous readers are usually entitled to — Published ones — are
always physical). A build unused for `OTS_MATERIALISATION_TTL_SECS` (3600)
is dropped again; `materialised` on the version record says whether it is
built now. Each build checks itself against the content hash, and a build
that fails from one end is tried from the other. A delta that neither end
can rebuild (rows lost to retention, the base gone or no longer matching its
hash, live graphs changed by writes the log did not record) is marked
`chain_broken` and its data answers **410**.

**Content hash.** Every version records `content_hash`, a 256-bit set hash of
its quads (per graph, and in total), computed while a checkpoint is copied
and, for a delta, from its base's hash and its chain — without building it.
Publishing a delta builds it and checks what was built against the hash
before it becomes physical; a mismatch marks the chain broken and refuses
with 409.

**Publishing** a delta makes it a checkpoint: it is built, verified, and its
built graphs become its own snapshot graphs.

## Retention: diff, delete, garbage-collect

A checkpoint keeps a full copy of the graphs it covers; a delta keeps only its
rows of the change log, which its range pins against retention. Three
endpoints keep that under control:

| Call | Effect |
|---|---|
| `GET /api/datasets/:id/versions/:a/diff/:b` | Per-graph triple delta from `a` to `b` (`added`/`removed`, plus totals). `b` may be `live` to compare a version with the dataset's current graphs. When both sides sit on one chain the diff is folded from the change log — `"source": "chain"`, O(change), nothing built — else the two sides are compared, building any delta first (`"source": "materialised"`). |
| `DELETE /api/datasets/:id/versions/:ver` | Removes the version, its snapshot graphs and its version-scoped validation graph (a delta: its build, if any, and its pin). A **published** version answers 409 — deprecate it first — unless `?force=true`; so does a checkpoint a delta chains from (`dependants` lists them). |
| `POST /api/datasets/:id/versions/gc` with `{"keep": N}` | Deletes all but the newest `N` non-published versions: deltas first, then the checkpoints nothing chains from any more (`kept_as_base` lists the rest). Published versions are never collected. |

Deleting a dataset deletes its versions too (they used to stay behind, and a
new dataset reusing the id saw them). `POST /api/admin/versions/sweep`
(admin) cleans up what a deletion before this left: versions of datasets that
no longer exist, version graphs no record names, pins of versions that are
gone, expired builds.

## Restore

`POST /api/datasets/:id/versions/:ver/restore` puts the dataset's live graphs
back to the version, in one of two ways — `tier` in the response says which:

- **Tier 1** — when the version is anchored in the change log's current
  epoch, the chain from it to now is intact, and its inverse fits
  `OTS_APPLY_DELTA_TX_MAX_QUADS` (50 000) quads: the inverse of everything
  since the version is applied in **one transaction across all its graphs**,
  then each graph is checked against the version's content hash. Nothing is
  copied, and no reader ever sees the version half-restored.
- **Tier 2** — otherwise (and when a tier-1 check fails because a write
  landed in between): the version is built if it is a delta, each snapshot is
  copied into a staging graph and swapped into place with one `MOVE`, so a
  reader never sees a live graph empty or half-written.

Either way the restore is one commit in the dataset's history, captured by
its LDES stream, entailment and full-text index. Snapshot, branch and
restore stream their copies in batches rather than holding every quad in
memory.

## Branches

Branches let you fork a version line to develop changes in parallel — for example a `staging` branch alongside `main`. List and create branches at `/api/datasets/{id}/branches`, specifying the branch name and the version it forks from. A branch of an anchored version copies nothing: it is a delta on the same base (on the version itself, for a checkpoint). Its `ahead`/`behind` counts against the latest published version come from the change log when both sit on one chain.

Version operations on one dataset — create, branch, publish, restore, delete,
gc, a pipeline's auto-version, validate-and-commit — are serialised by a
per-dataset lock: two concurrent creates of one label make one version and a
400, where they used to make two half-copies under one record.

## Share links

Mint a tokenised share link to grant read access to a dataset (or a specific version) without requiring the recipient to sign in. Links can be revoked at any time. This is the simplest way to hand a colleague or external reviewer a private dataset without changing its visibility or creating an account.

See also: [Datasets](/docs/datasets) and [Model & Vocabulary Versioning](/docs/models).

## RDF Patch

Any version diff is available as an [RDF Patch](https://afs.github.io/rdf-delta/rdf-patch.html)
document — one transaction of `A`/`D` quads against the dataset's live graph
IRIs that transforms the version into the other side:

```bash
curl -H 'Accept: application/rdf-patch' \
  http://localhost:7878/api/datasets/<id>/versions/1.0.0/diff/live
```

```
H id <urn:uuid:…> .
H dataset <http://localhost:7878/dataset/assets> .
H from "1.0.0" .
H to "live" .
TX .
D <https://example.org/asset/b2> <https://example.org/status> "planned" <https://example.org/assets/instances> .
A <https://example.org/asset/b2> <https://example.org/status> "in-service" <https://example.org/assets/instances> .
TC .
```

A patch applies to a dataset atomically, as one commit in its history:

```bash
curl -X POST http://localhost:7878/api/datasets/<id>/patch \
  -H "Authorization: Bearer <token>" -H 'Content-Type: application/rdf-patch' \
  --data-binary @changes.rdfp
```

Supported: `H`, one `TX`…`TC` (or `TA`, which applies nothing), `PA`/`PD`
prefixes, `A`/`D` with a graph term — a dataset patch may only touch the
dataset's registered graphs, so triples without a graph are refused. Terms
are IRIs, prefixed names, literals, blank nodes and RDF 1.2 triple terms
(`<<( s p o )>>`, as an object).

The patch applies as **one transaction** with the net effect of its lines —
the last line on a quad decides whether it ends up present or absent — and is
recorded in the change log under one commit. A `D` line may name a blank
node: a `_:label` this store exported (a change's patch, a version diff)
addresses the stored node, while a hand-written label matches nothing. The
response's `effective` counts say what actually changed (`added`/`removed`
count the lines). The route needs an account (401 anonymous).

`H base-digest "<64 hex digits>"` makes the apply conditional: the set hash
of the graphs the patch names must be that value before it applies, else
**412** and nothing is applied. It is the same hash versions record as
`content_hash` (`store::set_hash`: the sum of the SHA-256 of each quad
printed as `<graph> s p o`, modulo 2^256).

Each change of the change log is a patch too — see
[The dataset's change feed](#the-datasets-change-feed): `H seq` and `H epoch`
give its place in the log, `H commit` the commit it belongs to, and its
inverse carries `H prev` naming it.

## Change log

Every write records what it did, per graph, in a small SQLite log beside
the store (`{data_dir}/changes/changes.db`; in memory for an in-memory
store). Capture is **off by default** — `OTS_CHANGE_CAPTURE=on` turns it on,
and a replication leader keeps it on regardless. It is off because a
`WHERE` update pays ×2.5–4 for it (see [What it costs](#what-it-costs)),
which is not a price a store with no consumer for the log should pay.
The log is the source for the replication work and for the dataset history,
and it is designed so that a consumer can *tail* it: rows carry a dense
sequence number in commit order, a consumer bookmarks the last one it
applied, and retention keeps every row above the lowest live bookmark.

### What a row says

One row per graph per write. `seq` is the position in commit order — the
order the repo's own commit calls ran in, taken inside a critical section
around the commit — so a consumer applying rows in `seq` order reproduces
the store. `epoch` names the store's lineage: a store rebuilt after a
quarantine (`STORE_AUTO_RECOVER`, see `docs/administration.md`) mints a new
one, and a consumer holding rows of another epoch must resynchronise rather
than apply them.

| Field | Meaning |
|---|---|
| `scope` | `graph` — the row is about `graph_iri` (`null` is the default graph); `store` — the write's targets could not be bounded (a `GRAPH ?g` update, a streamed load into the default graph), so every graph may have changed |
| `extent` | `full` — `added` and `removed` carry the net delta as N-Quads (gzipped above 64 KiB; `added_n` / `removed_n` are the counts); `counts` — the counts are exact but the payload was above the cap; `unknown` — the graph changed, nothing more is known |
| `post_count` | the graph's quad count after the write, when known |
| `state` | `committed`, `unknown`, `pending` (a write in flight — or one a crash interrupted, settled at the next start), `aborted` |
| `origin` | the engine primitive: `update`, `update_targeted`, `update_scoped`, `batch_update`, `graph_store_put`, `graph_store_delete`, `load`, `load_reader`, `bulk_insert_quads`, `bulk_delete_graphs`, `store_quad`, `late` (a graph the write discovered while running) |
| `kind`, `actor_iri`, `commit_iri` | the commit-trail kind (`sparql`, `graph-store`, …) and the actor IRI as the trail mints it, when the write came through a handler; `null` for system writes |
| `has_bnode` | the payload names a blank node; a consumer that re-labels blank nodes cannot apply it verbatim |

How a primitive arrives at its row:

- A ground update (`INSERT DATA` / `DELETE DATA`) is simulated against the
  store before it runs, so the row is the exact net delta — quads already
  present are not "added", quads absent are not "removed".
- Any other update with statically known targets (`GRAPH <g> { … }`,
  `WITH`, `INSERT { GRAPH <g> … }`) takes a before-image of those graphs
  when their summed counts are within `OTS_CHANGE_CAPTURE_MAX_SCAN` (default
  250 000 quads), runs inside one transaction, reads the after-image through
  the same transaction and records the diff. Above the cap the row is
  `unknown` for each target.
- A `/sparql/batch` batch is one transaction and one `txn` with consecutive
  sequence numbers; a batch that rolls back records nothing.
- Graph Store `PUT` records the net difference between the old and the new
  graph, `POST` only the quads that were new, `DELETE` every removed quad
  (`counts` above 100 000, the chunked path).
- A bulk load into a named graph records the quads that were new; a
  streamed load into the default graph is a store-scoped `unknown` — it
  never materialises what it wrote. Version-snapshot graphs
  (`…/version/…`) are recorded `unknown` rather than have their copy stored
  a second time.

### Reading and bookmarking

```
GET    /api/admin/changes?after=<seq>&limit=<n>&graph=<iri>
GET    /api/admin/changes/status
PUT    /api/admin/changes/cursors/<name>      {"seq": <n>}
DELETE /api/admin/changes/cursors/<name>
```

`GET /api/admin/changes` returns `{ "epoch", "rows", "next_after" }`; pass
`next_after` back as `after` to continue. `graph=` narrows to one graph's
rows *plus* the store-scoped rows, which a reader of any graph must see.
`PUT …/cursors/<name>` bookmarks a consumer at the last `seq` it applied
(`0` … the newest `seq`; names are 1–64 characters of `[A-Za-z0-9._-]`),
`DELETE` drops it (`204`, or `404`). All four are admin-only: rows carry
quads from every tenant. `GET /api/admin/changes/stats` (admin) accounts for
the log: rows and payload bytes per dataset, live builds of delta versions
and the quads they hold, checkpoints and deltas per dataset, the pins and
cursors holding retention back and their owners.

### The dataset's change feed

A dataset's readers can tail its part of the log:

```
GET    /api/datasets/<id>/changes?since=<seq>&limit=<n>&wait_ms=<ms>
GET    /api/datasets/<id>/changes/<seq>
GET    /api/datasets/<id>/changes/<seq>/inverse
PUT    /api/datasets/<id>/changes/cursors/<name>      {"seq": <n>}
DELETE /api/datasets/<id>/changes/cursors/<name>
```

The feed lists the rows of the dataset's graphs above `since`, without their
quads, plus `next` to resume from; `wait_ms` long-polls (up to 30 s) when
nothing is new. Only rows of graphs the caller may read are listed — dataset
access does not imply access to its private graphs, and an unreadable row is
omitted, never counted. Store-scoped rows (writes whose graphs were not
known) are listed for every dataset: they bound every chain.

`…/changes/<seq>` is that change as an RDF Patch (`application/rdf-patch`),
`…/inverse` the patch that undoes it; **410** when retention has removed the
change, **409** when the row carries no quads (it was larger than a row
holds, or the log could not bound it). A cursor (any signed-in reader) pins
retention like an admin cursor; it belongs to whoever set it — someone else
gets 403 — and an admin can delete it. Every route answers from an empty log
while capture is off.

The commit trail points into the log: a commit whose write the log recorded
carries `seq_from` / `seq_to` (its rows) and `net_added` / `net_removed`
(what it actually changed — unlike `added` / `removed`, whose meaning differs
per write path) on `GET /api/datasets/<id>/commits`, and `ver:seqFrom` /
`ver:seqTo` in `urn:system:commit-log`. Commits made before the log existed
have neither; order those by `created`.

### What it costs

Criterion medians on the in-memory store, the same machine, baseline and
change taken back to back (two pairs, agreeing within the baseline's own
±7 % run-to-run spread):

| benchmark (in-memory store) | before | capture off (default) | capture on |
|---|---|---|---|
| `insert/sparql_update/single_triple` | 74.8 µs | 77.1 µs (+3 %) | 84.2 µs (+13 %) |
| `insert/sparql_update_batch/10_triples` | 155.3 µs | 155.8 µs (+0 %) | 175.1 µs (+13 %) |
| `update/ground_delta/insert_data/1` | 14.6 µs | 14.8 µs (+1 %) | 15.7 µs (+7 %) |
| `update/ground_delta/insert_data/100` | 256.3 µs | 255.7 µs (−0 %) | 266.1 µs (+4 %) |
| `update/insert_where/100` | 325.8 µs | 334.9 µs (+3 %) | 823.9 µs (+153 %) |
| `update/insert_where/1000` | 2.79 ms | 2.81 ms (+1 %) | 7.18 ms (+157 %) |
| `update/insert_where/10000` | 40.15 ms | 39.76 ms (−1 %) | 106.38 ms (+165 %) |
| `update/delete_where/100` | 219.1 µs | 227.6 µs (+4 %) | 638.9 µs (+192 %) |
| `update/delete_where/1000` | 1.64 ms | 1.62 ms (−1 %) | 5.66 ms (+246 %) |
| `update/delete_where/10000` | 19.45 ms | 20.17 ms (+4 %) | 77.77 ms (+300 %) |

Off, the write path is the one it was. On, a ground update pays a few
microseconds for its row; a `WHERE` update pays the before-image scan of its
target graph, the after-image read through the transaction, the diff, and
the N-Quads payload (gzipped above 64 KiB) — proportional to the target
graph and the delta, and the reason capture is opt-in. On a persistent
(RocksDB) store the fixed cost sits beside a `fsync` per commit and weighs
less; the scan cost does not shrink.

### Retention, crashes, caps

- **Retention.** Rows older than `OTS_CHANGE_RETENTION_DAYS` (default 90)
  are swept, but never above the lowest live cursor or pin. A cursor that has
  not been updated for `OTS_CURSOR_TTL_DAYS` (default 30) stops pinning
  retention and is dropped: a consumer that went away must resynchronise,
  not hold the log forever. A delta version pins its range for as long as it
  lives (deleting or publishing it releases the pin). Rows of a version's own
  snapshot graphs and of restore staging graphs, which no chain needs, go
  after a day. The highest sequence number retention deleted is remembered,
  so a chain that would need a row below it counts as broken — the next cut
  is a checkpoint — rather than silently missing a change.
- **Capture switched off and on again.** A log kept from an earlier run
  stays readable while capture is off (delta versions resolve through it),
  but records nothing; the next run that records starts a new epoch, so no
  chain is ever extended across writes the log did not see.
- **Bounded whole-graph operations.** `CLEAR`, `DROP`, `CREATE` and `LOAD`
  of a named graph (and `MOVE`/`COPY`/`ADD`, which are made of them) are
  bounded to that graph and recorded like any other write; only `CLEAR`/`DROP
  ALL` and `NAMED` are store-scoped. A streamed load of a multi-graph file
  probes its quads on the way past (up to the scan cap) and records exact
  rows per graph.
- **Crashes.** A row is written *before* its write runs and finalised with
  the sequence number right after the commit. A crash between the two
  leaves it `pending`; the next start probes the store for the intended
  quads and settles the row as `committed`, `aborted`, or `unknown` when it
  cannot tell (a payload was never recorded). At the same start every
  graph's count is compared with the last count the log recorded for it; a
  graph that changed outside the log gets an `unknown` row (`origin` =
  `reconcile`), so a consumer sees that its chain broke.
- **Caps.** `OTS_CHANGE_CAPTURE_MAX_SCAN` (250 000) bounds the before-image
  scan; `OTS_CHANGE_CAPTURE_MAX_PAYLOAD` (250 000) bounds the quads a
  `full` row may carry — above it the row is `counts`.
- **Off by default.** A node records only when asked
  (`OTS_CHANGE_CAPTURE=on`); a replication leader or cluster member records
  regardless, because its followers read this log and the role means nothing
  without it; a replication follower does not unless asked, because its own
  log would hold only the graphs it fetched whole — a partial log, worse than
  none. What turning it on buys: a follower, the dataset history and an audit
  can start from the log as it stands, with no reload. What it costs is the
  table above, and the shape of that cost is the reason for the default — a
  ground update pays 4–13 % because the statement already is the delta, while
  a `WHERE` update pays ×2.5–4 because it names its target by pattern, so the
  graph has to be read before the update and again through the transaction and
  the two subtracted. That is proportional to the target graph rather than to
  the size of the change, which makes a small `DELETE WHERE` against a large
  graph the worst case. `OTS_CHANGE_CAPTURE_MAX_SCAN` bounds it, at the price
  of rows that say `unknown`.
- **What is not recorded.** The commit-trail insert itself (its own
  `urn:system:commit-log` graph would otherwise produce a row per row);
  registry writes to the identity database (not RDF); the text, spatial and
  parallel-mirror indexes (each node rebuilds its own).
