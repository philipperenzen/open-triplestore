# Operations

Endpoints and behaviours that matter when running the triplestore as a service.

## Health & service description

- **Health probe** — `GET /health` returns the software version and the status of each subsystem (triplestore, database, object storage, backup) as JSON, with a `503` when a core service is down — suitable for load-balancer and container health checks.
- **Service description** — `GET /` returns a SPARQL 1.1 Service Description (Turtle) advertising capabilities and the named graphs the caller may access.

## Bulk import

The [Data Import](/import) wizard can upload many files in a single request via `POST /api/import/bulk`. Before committing, `POST /api/import/analyze` runs a pre-flight pass that detects each file's format, embedded graphs, and role (see [Import Auto-Detection](/docs/import)) so you can review the plan first. New datasets and organisations are only created once the first file imports successfully.

## Backups

When a backup directory is configured, admins can create, list, and verify backups. A backup is a compressed snapshot of the RDF data and the account database, with a checksum manifest for integrity verification and optional encryption at rest. Endpoints: `GET` / `POST /api/admin/backup` to create and list, and `POST /api/admin/backup/{id}/verify` to verify against the manifest.

## Rate limiting

Requests are rate-limited per client IP, with stricter quotas on authentication endpoints to resist brute-force attacks and separate quotas for query and import traffic. When running behind a reverse proxy, configure the trusted proxy ranges so limits apply per real client IP rather than the proxy's own address.

For trusted/internal deployments or automated test harnesses that drive many requests from a single IP, the limiter can be switched off with `RATE_LIMIT_DISABLED=true` (see [administration.md](administration.md)). It is secure by default — leave it unset on any public server.

## Replication

Open Triplestore replicates by **logical log shipping**: a leader records
every write in its [change log](versioning.md#change-log) — one row per graph
per write, in commit order — and a follower tails those rows and applies
them. There is no second on-disk format, no RocksDB checkpoint to ship and
no lockstep between nodes: the follower is an ordinary server that keeps its
store read-only and asks the leader what changed.

What varies is configurable, on the follower:

| | Environment | Values |
|---|---|---|
| **Role** | `OTS_REPLICATION_ROLE` | `none` (default), `leader`, `follower` |
| **Temperature** — how often a follower asks | `OTS_REPLICATION_MODE` | `cold` (every hour), `warm` (every minute, the default; `medium` is accepted), `hot` (long-polls: a request for rows is held on the leader up to 25 s and answered the moment a row lands; `OTS_REPLICATION_POLL_MS`, default 500, paces the retry after a failure) |
| **Scope** — what a follower applies | `OTS_REPLICATION_GRAPHS` / `OTS_REPLICATION_DATASETS` | `all` (default); a comma-separated list of graph IRIs (`default` for the default graph); or a list of the leader's dataset ids, resolved to graphs through the leader's manifest at every catch-up |
| Leader | `OTS_REPLICATION_LEADER_URL`, `OTS_REPLICATION_TOKEN` | the leader's base URL and an admin API token minted there |
| Identity | `OTS_REPLICATION_NODE_ID` | this follower's name (default: `HOSTNAME`, else `follower`); the cursor it keeps on the leader |
| Override | `OTS_REPLICATION_INTERVAL_SECS` | a catch-up interval that replaces the temperature's |

Temperature changes *only* how often the follower asks. Cold, warm and hot
apply the same rows the same way; a cold follower that has not asked for an
hour applies an hour of rows when it does. A hot follower long-polls: its
request for rows is held on the leader until a row lands, so its lag is a
network round trip, not the poll period. The hold is 25 s at most (the
leader caps a long-poll at 30 s), which is what lets a hot follower live
under the leader's [rate limiter](#rate-limiting): idle, it costs the
leader about seven small requests per 25 s — the held request and a
manifest read per catch-up, and the identity check's manifest read every
5 s — rather than four a second. The lag stays a round trip while the
leader's writes leave room under that limiter: a catch-up that carries
rows costs three requests, so up to about a write every three seconds
sustained; faster than that the follower is paced by the limiter — it
waits its `Retry-After` and applies rows in pages of up to 500 — and its
lag grows with the backlog until the writes slow down. A leader whose
clients are its followers can lift the limit (see [Sizing and
cost](#sizing-and-cost)).

**The modes, in one place:**

| Mode | Where it is set | What it means |
|---|---|---|
| `cold` | follower, `OTS_REPLICATION_MODE` | a disaster-recovery copy: catches up every hour |
| `warm` (or `medium`) | follower | a reporting replica: catches up every minute |
| `hot` | follower | a read replica: long-polls the leader continuously; lag is a round trip |
| asynchronous | leader, the default | a write returns as soon as it is durable on the leader; followers catch up at their own pace |
| synchronous | leader, `OTS_REPLICATION_SYNC_FOLLOWERS` set | a write returns once the required followers have *applied* it, or after the timeout, degraded and visibly so (below) |
| consensus | every member, `OTS_REPLICATION_ROLE=cluster` | Raft elects the leader and fences the old one; the elected member leads, the others are hot, synchronous followers, and a write is acknowledged by a majority (below) |

Synchronous mode needs hot followers to be usable: a warm follower
acknowledges once a minute, so every write on its leader would wait the
timeout out.

### Try it: a leader and a hot follower with Docker Compose

[`docker-compose.replication.yml`](../docker-compose.replication.yml) is a
stand-alone stack — a leader on port 7878 and a hot follower on 7879, one
shared `JWT_SECRET`, separate data volumes, nothing else — that shows
every piece of this chapter in a few minutes. The follower needs an
admin API token minted on the leader, so the stack comes up in two steps.

**1. The leader, and a token for the follower.**

```bash
cp .env.example .env
printf 'JWT_SECRET=%s\n' "$(openssl rand -hex 32)" >> .env
docker compose -f docker-compose.replication.yml up -d leader

# The first registered user is the super_admin; the reply carries an access token.
curl -s -X POST localhost:7878/api/auth/register -H 'Content-Type: application/json' \
     -d '{"username":"admin","email":"admin@example.org","password":"<choose one>"}'
# Mint the follower's token with that access token. The reply's `token` is shown once.
curl -s -X POST localhost:7878/api/auth/tokens -H 'Content-Type: application/json' \
     -H 'Authorization: Bearer <access_token>' \
     -d '{"name":"replica-1","scopes":["read","admin"]}'
printf 'OTS_REPLICATION_TOKEN=%s\n' '<token>' >> .env
```

**2. The follower.** It bootstraps — reads the leader's manifest, fetches
every graph whole, adopts the epoch — and then long-polls. The demo
organisation the leader seeded on its first boot is what comes across
(the compose file leaves the IFC building demos out of that seed with an
empty `SEED_IFC_URL`, so the leader is not still lifting models while
the follower bootstraps; the leader's rate limiter allows a follower one
whole-graph fetch a second past its burst).

```bash
docker compose -f docker-compose.replication.yml up -d follower
curl -s localhost:7879/api/replication/status
# "role":"follower", "mode":"hot", "epoch":"…", "lag_rows":0, "healthy":true
```

The bootstrap fetches every graph whole — the seeded demo is about a
hundred graphs — at one a second past the leader's rate-limiter burst of
40, so allow a minute or two before the status reads `lag_rows: 0`. The
leader also goes on seeding — vocabularies, shapes, the demo datasets —
for about a minute after it is up; a follower started during that applies
the seed's rows as they land, at the same pace, and `applied_rows` climbs
until the seed is done.

**3. A write on the leader shows up on the follower.** Capture is on
because the leader role turns it on; the follower applies the row within a
round trip. The graph is read through the Graph Store, under the same
token — `/sparql` scopes a query to the graphs of datasets the caller may
see, so an ad-hoc graph is invisible there on either node, while the Graph
Store reads any graph an admin asks for (it is how the follower itself
reads a graph whole).

```bash
curl -s -X POST localhost:7878/sparql -H 'Authorization: Bearer <access_token>' \
     -H 'Content-Type: application/sparql-update' \
     --data 'INSERT DATA { GRAPH <urn:demo:replicated> { <urn:s> <urn:p> "hello" } }'
curl -s 'localhost:7879/store?graph=urn:demo:replicated' \
     -H 'Authorization: Bearer <access_token>' -H 'Accept: application/n-triples'
# <urn:s> <urn:p> "hello" .   — and /api/replication/status counts one more applied row
```

**4. A write on the follower is refused.** The follower is read-only and
says where writes go:

```bash
curl -s -o /dev/null -w '%{http_code}\n' -X POST localhost:7879/sparql \
     -H 'Authorization: Bearer <access_token>' -H 'Content-Type: application/sparql-update' \
     --data 'INSERT DATA { <urn:s> <urn:p> "no" }'
# 503 — read-only replica: writes go to the leader at http://leader:7878
```

The same access token works on both nodes because they share the secret and
the follower's identity database is a copy of the leader's
([below](#identity-database)). The web UI on either port shows the same
data; the **Node status** page under *Admin* shows this node's role, lag and
last catch-up, the leader's change-log status, and the follower's cursor as
the leader sees it. `OTS_REPLICATION_MODE=warm` in `.env` turns the
follower into a once-a-minute reporting replica; uncommenting
`OTS_REPLICATION_SYNC_FOLLOWERS=replica-1` on the leader makes every write
wait for the follower to apply it.

### How it works

**The leader** sets `OTS_REPLICATION_ROLE=leader`, which turns change
capture on whatever `OTS_CHANGE_CAPTURE` says — capture is off by default
elsewhere, and a leader without it has nothing for a follower to read. It
serves three things a follower reads, all under an admin token:

- `GET /api/replication/manifest` — its change-log epoch and newest
  sequence number, every graph it holds (`null` is the default graph), and
  each dataset with its graphs;
- `GET /api/admin/changes?after=<seq>&limit=<n>` — the rows, in commit
  order;
- the Graph Store, `GET /store?graph=<iri>` as N-Triples — a whole graph,
  when a row says only that the graph changed.

**The follower** sets `OTS_REPLICATION_ROLE=follower`, the leader's URL and
token, a temperature and a scope, and starts. Its store is **read-only**:
every write — SPARQL Update, Graph Store `PUT`/`POST`/`DELETE`, imports,
registry data writes — answers `503 read-only replica: writes go to the
leader at <url>`. Reads are what it is for. On its first catch-up it
**bootstraps**: it reads the manifest, fetches every graph in scope whole,
adopts the leader's epoch and bookmarks the leader's newest sequence number.
From then on each catch-up:

1. reads the manifest (the epoch, the newest sequence number, the dataset
   graphs a dataset scope needs);
2. asks for rows after its bookmark, page by page (500 rows), and applies
   each in order:
   - a `full` row — the net delta as quads — is applied **as a delta**: the
     added quads inserted, the removed ones removed, in one transaction; the
     row's `post_count` is then compared with the graph's count here, and a
     disagreement fetches the graph whole (the divergence heals itself);
   - a `counts` or `unknown` row says "graph *X* changed at seq *N*": the
     follower fetches the graph and replaces its copy. A bulk load on the
     leader is one such row per graph, not nine million quads in a log;
   - a **store-scoped** row (a `GRAPH ?g` update, a streamed load) means
     anything may have changed: every graph in scope is fetched again, and
     graphs the leader no longer lists are dropped;
   - a row for a graph outside the scope is skipped; the cursor advances
     over it;
3. bookmarks the last applied sequence number — locally
   (`<data-dir>/replication.json`, so a restart continues where it stopped)
   after every page and after every row that fetched a graph whole, so a
   page of bulk-load rows that takes minutes shows its progress in the
   status; and on the leader after every page
   (`PUT /api/admin/changes/cursors/<node-id>`), so the leader's retention
   never sweeps a row this follower still needs.

**Epochs and failover.** The leader's change log has an epoch — a name for
the store's lineage. A follower applies rows only from the epoch it adopted;
a different epoch (a follower promoted to leader, a restore, a store rebuilt
after a quarantine) makes it resynchronise every graph in scope and adopt
the new epoch. So a failover is: stop writing to the old leader, start the
promoted node with `OTS_REPLICATION_ROLE=leader` (its own capture, its own
epoch), point the other followers at it. They resynchronise once and tail
again. The old leader, brought back as a follower of the new one, does the
same. Rows a follower applied are never "un-applied": fencing is by epoch,
not by comparing histories, which is what makes it safe to promote a node
that was behind — its followers get its state, whole.

**Status.** `GET /api/replication/status` is public, beside `/livez`: the
role, temperature and scope; on a follower the adopted epoch, the applied
and the leader's newest sequence numbers, the lag in rows, the last catch-up
time and error, and `healthy` — true while the last successful catch-up is
younger than three intervals (plus the long-poll hold, on a hot follower).
A load balancer that must not route stale
reads checks `lag_rows` and `healthy`.

```json
{
  "role": "follower", "mode": "hot", "scope": "all",
  "leader_url": "https://leader.example.org", "node_id": "replica-1",
  "read_only": true, "epoch": "9d1c…", "applied_seq": 48213,
  "leader_newest_seq": 48213, "lag_rows": 0,
  "last_sync_at": "2026-09-16T10:41:07Z", "last_error": null,
  "applied_rows": 48213, "refetched_graphs": 12, "resyncs": 1,
  "interval_secs": 0.5, "healthy": true
}
```

### Synchronous replication

A leader that names synchronous followers waits, at the end of every
write, until enough of them have applied it. The acknowledgement is the
cursor a follower sets on the leader after applying a page — so "applied"
means visible to a reader of the follower, not merely received.

| Setting (leader) | Default | Meaning |
|---|---|---|
| `OTS_REPLICATION_SYNC_FOLLOWERS` | *(unset: asynchronous)* | The node ids (`OTS_REPLICATION_NODE_ID` on each follower) whose acknowledgement a write waits for. |
| `OTS_REPLICATION_SYNC_REQUIRED` | `1` | How many of them must have applied the write before it returns; `all` for every one. One of two survives a follower outage; `all` does not. |
| `OTS_REPLICATION_SYNC_TIMEOUT_MS` | `2000` | How long a write waits (50–60000 ms). A hot follower on a healthy link acknowledges in a round trip; the timeout is for the unhealthy case. |

**When the followers keep up**, a successful write means the data is on
the leader *and* on at least the required followers; the response carries
`X-Replication-Ack: sync`. **When they do not** — a follower down, a
partition, a follower that fell behind — the write still succeeds after
the timeout, because it is already durable on the leader and failing it
would make a client retry a write that landed. The leader marks itself
*degraded*: the response carries `X-Replication-Ack: degraded`, the status
shows `sync.degraded_since`, and a warning is logged once. While degraded,
writes do not wait the timeout out one by one: a write waits only when a
follower has caught up to the point before it, which is the sign that it
is back. The first write those followers acknowledge clears the flag,
logs the recovery and the leader is synchronous again. Nothing is
un-applied or replayed.

The guarantee, in one sentence: while `sync.degraded_since` is null, a
successful write has been applied on at least `required` of the named
followers; while it is set, a successful write is durable on the leader
only, and the header on each response says which case a client is in.

Why not block until a follower returns, or fail the write: blocking turns
a replica outage into a leader outage; failing after the commit would be a
lie, since the data is already visible on the leader. Both are discussed
in the improvement log (P4, decision 1).

```json
"sync": {
  "followers": ["replica-1", "replica-2"], "required": 1, "timeout_ms": 2000,
  "acked": ["replica-1"], "degraded_since": null, "last_confirmed_seq": 48213,
  "waits": 48213, "degraded_waits": 17
}
```

### Consensus

A cluster is three or more members that elect their leader. Raft — through
the `openraft` crate — decides exactly one thing here: **who leads**. The
replicated state machine holds no data; its entries are the membership and
a heartbeat. The data path is the one above: the elected member records
every write in its change log, the other members are hot followers of it,
and every write is acknowledged synchronously by a majority (the leader
plus half of the rest) before it returns — with the same visible
degradation when a majority cannot be reached.

| Setting (every member) | Meaning |
|---|---|
| `OTS_REPLICATION_ROLE=cluster` | This node is a member. |
| `OTS_REPLICATION_CLUSTER` | Every member as `id=url`, comma-separated: `1=https://a:7878,2=https://b:7878,3=https://c:7878`. The same list on every member. |
| `OTS_REPLICATION_CLUSTER_ID` | This node's id in that list. Its follower name is `node-<id>`. |
| `OTS_REPLICATION_CLUSTER_SECRET` | A shared secret; the Raft messages between members carry it (`X-Cluster-Secret`). Required. |
| `OTS_REPLICATION_TOKEN` | An admin API token, minted once on any member: the identity database (and so the token) is shipped to every member, and a follower uses it to read the leader. |
| `OTS_REPLICATION_ELECTION_MS` | The election timeout's lower bound (default 1500; the upper bound is twice that). A leader that misses heartbeats for this long is replaced. |
| `OTS_REPLICATION_HEARTBEAT_MS` | The leader's heartbeat (default a fifth of the election timeout). |

What happens: every member starts its Raft instance and, on a fresh log,
proposes the static membership (all of them do; the ones that find a log
already initialised are told so and carry on). Within an election
timeout one member is leader. Its status says `role: leader`,
`configured_role: cluster`; the others say `follower`, follow it hot, and
answer `503` to writes. **Failover is automatic:** when the leader stops
or is cut off, the remaining majority elects another within an election
timeout; the new leader's change log has its own epoch, so the followers
resynchronise from it once and tail again. **Fencing:** a leader that
loses contact with the majority steps down by itself and refuses writes
from the next request on; a client that reaches a stale leader gets a
`503`, not a lost write. **Quorum writes:** the other members are the
leader's synchronous followers with `required` set to a majority minus
the leader itself (one of three, two of five); a write returns once that
many have applied it, and degrades visibly when they cannot — exactly the
synchronous mode above, with the follower list and the count derived
from the cluster.

Two things to know. The Raft log and vote are kept in memory: a restarted
member rejoins with term 0 and learns the current term from the first
heartbeat; because the state machine holds no data and the data path
fences by epoch, the worst a double vote could do is elect a second leader
for one term, which the epoch handling turns into a resynchronisation,
not a divergence. And the members talk over the server's own HTTP port
(`POST /api/replication/raft/vote`, `/append`, `/snapshot`); put those
paths on the private network — the secret authenticates them, the
network should hide them.

```json
"cluster": {
  "id": 2, "leader": 1, "is_leader": false, "term": 7, "state": "follower",
  "members": { "1": "https://a:7878", "2": "https://b:7878", "3": "https://c:7878" },
  "millis_since_quorum_ack": null
}
```

### Identity database

The identity database is shipped as SQLite's own bytes, not as an
application-level log of identity events. The leader serves a consistent
snapshot of `auth.db` at `GET /api/replication/identity`, taken with
SQLite's online backup API (safe under WAL, no lock on the writers), and
its manifest carries `identity_version` — SQLite's own change counter
(`PRAGMA data_version`, read on a connection kept only for watching), which
moves whenever anything commits to the database. The follower checks the
manifest every `OTS_REPLICATION_IDENTITY_INTERVAL_SECS` (the temperature's
interval, at least 5 s) and, when the version moved, fetches the snapshot
and applies it **in place** — the backup API's destination side writes the
pages under the follower's open connections, so no file is swapped, no
pool reopened, and every connection sees the new state on its next
statement. The status shows it under `identity`.

Why whole snapshots rather than a stream of WAL frames: a live follower
reads its database through open connections, and applying foreign WAL
frames underneath them is not something SQLite supports; the backup API is
the supported way to replace a live database's pages. The identity database
is small (kilobytes to a few megabytes), so a whole snapshot per change is
cheap, and the transport can become frame-level later without changing the
model. Three things to know:

- The follower's own requests move the version it watches at most once a
  minute: an API token's `last_used_at` is stamped no more often than
  that, so a follower polling with its token fetches the database again
  at most once a minute rather than at every check.
- Copy `<data-dir>/jwt_secret` from the leader to the follower: tokens the
  leader issued then validate on the follower. API tokens live in the
  database and come across with it; `OTS_REPLICATION_TOKEN` should be one
  minted on the leader.
- A follower's own identity writes — login timestamps, audit rows, tokens
  minted on the follower — are overwritten by the next apply. Log in and
  administer on the leader.
- The leader's audit log is part of the database and comes across too.

### What a follower does and does not replicate

- **RDF data**, per graph, per the scope. Version-snapshot graphs
  (`…/version/…`) come across whole (the leader records them `unknown`
  rather than copy them into its log).
- **The identity database** (`auth.db`: users, organisations, datasets,
  tokens, ACLs), shipped whole — see "Identity database" below.
- **Not the object store.** Point both nodes at the same S3 bucket; with the
  local filesystem store, assets exist only on the node that received them.
- **Not the text, spatial or accelerator indexes.** Each node rebuilds its
  own from the data it receives.
- **Not the follower's own change log.** Applying a delta does not produce
  a row on the follower; a follower is not a leader for further followers.
  (Capture is off by default, and a follower is no exception unless
  `OTS_CHANGE_CAPTURE=on`: a follower's log would only hold the graphs it
  fetched whole, a partial log.)
- **At boot, a follower logs the seed's refusals.** The boot-time seed (the
  Studio shapes, the bundled demo data) writes to the store; on a follower
  those writes are refused and logged as warnings, and the same graphs
  arrive from the leader instead.

### Sizing and cost

A follower's catch-up costs the leader one manifest read and one page read
per 500 rows, plus a Graph Store read per graph fetched whole. A hot
follower keeps one request held on the leader at a time and asks again the
moment it is answered, so idle it costs about seven small requests per
25 s (the held request, a manifest read per catch-up, and the identity
check's manifest read every 5 s); a leader with many hot followers holds
one request per follower. The change log itself costs the leader what
[versioning.md](versioning.md#what-it-costs) measured: a few microseconds
per ground update, a scan of the target graph per `WHERE` update.
Retention on the leader (`OTS_CHANGE_RETENTION_DAYS`, 90) never sweeps above
the lowest live cursor, and a cursor idle for `OTS_CURSOR_TTL_DAYS` (30)
expires — a follower away longer than that resynchronises whole when it
comes back.

A follower is a client of the leader's [rate limiter](#rate-limiting) like
any other at its address: the Graph Store reads a bootstrap makes are
limited to a burst of 40 and one a second after that. When the leader
answers `429`, the follower waits the `Retry-After` out and sends the same
request again — bounded to eight waits of at most 30 s each — so a
bootstrap of many graphs proceeds at the limiter's sustained rate rather
than restarting from the first graph. A leader whose only clients are its
followers can lift the limit (`RATE_LIMIT_DISABLED`, or trusted proxy
ranges so the limit applies per real client) and let a bootstrap run at
full speed.

### What is not here

- **Asset shipping**, as above.
