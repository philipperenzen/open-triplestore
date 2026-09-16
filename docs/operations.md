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
| **Temperature** — how often a follower asks | `OTS_REPLICATION_MODE` | `cold` (every hour), `warm` (every minute, the default; `medium` is accepted), `hot` (every poll, `OTS_REPLICATION_POLL_MS`, default 500) |
| **Scope** — what a follower applies | `OTS_REPLICATION_GRAPHS` / `OTS_REPLICATION_DATASETS` | `all` (default); a comma-separated list of graph IRIs (`default` for the default graph); or a list of the leader's dataset ids, resolved to graphs through the leader's manifest at every catch-up |
| Leader | `OTS_REPLICATION_LEADER_URL`, `OTS_REPLICATION_TOKEN` | the leader's base URL and an admin API token minted there |
| Identity | `OTS_REPLICATION_NODE_ID` | this follower's name (default: `HOSTNAME`, else `follower`); the cursor it keeps on the leader |
| Override | `OTS_REPLICATION_INTERVAL_SECS` | a catch-up interval that replaces the temperature's |

Temperature changes *only* how often the follower asks. Cold, warm and hot
apply the same rows the same way; a cold follower that has not asked for an
hour applies an hour of rows when it does. This is the asynchronous variant
of hot replication: the leader never waits for a follower, and a follower is
as far behind as its last catch-up. The synchronous and consensus variants
are not built (see "What is not here").

### How it works

**The leader** sets `OTS_REPLICATION_ROLE=leader`, which switches change
capture on (`OTS_CHANGE_CAPTURE=on` does the same without the role). It
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
3. after every page, bookmarks the last applied sequence number — locally
   (`<data-dir>/replication.json`, so a restart continues where it stopped)
   and on the leader (`PUT /api/admin/changes/cursors/<node-id>`), so the
   leader's retention never sweeps a row this follower still needs.

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
younger than three intervals. A load balancer that must not route stale
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

### What a follower does and does not replicate

- **RDF data**, per graph, per the scope. Version-snapshot graphs
  (`…/version/…`) come across whole (the leader records them `unknown`
  rather than copy them into its log).
- **Not the identity database** (`auth.db`: users, organisations, datasets,
  tokens, ACLs). A follower serves data with the identity database it has.
  For a read replica that must authenticate the same users as the leader,
  seed it from the leader's backup (`docs/administration.md`, backups
  include `auth.db`) at bootstrap; keeping it current is the next step of
  this work (an application-level log of identity changes, or WAL shipping),
  not this one.
- **Not the object store.** Point both nodes at the same S3 bucket; with the
  local filesystem store, assets exist only on the node that received them.
- **Not the text, spatial or accelerator indexes.** Each node rebuilds its
  own from the data it receives.
- **Not the follower's own change log.** Applying a delta does not produce
  a row on the follower; a follower is not a leader for further followers.
  (A follower with `OTS_CHANGE_CAPTURE=on` records the graphs it fetches
  whole, which is a partial log; leave capture off on followers.)
- **At boot, a follower logs the seed's refusals.** The boot-time seed (the
  Studio shapes, the bundled demo data) writes to the store; on a follower
  those writes are refused and logged as warnings, and the same graphs
  arrive from the leader instead.

### Sizing and cost

A follower's catch-up costs the leader one manifest read and one page read
per 500 rows, plus a Graph Store read per graph fetched whole. Hot followers
poll every 500 ms; a leader with many hot followers pays that many small
requests per second. The change log itself costs the leader what
[versioning.md](versioning.md#what-it-costs) measured: a few microseconds
per ground update, a scan of the target graph per `WHERE` update.
Retention on the leader (`OTS_CHANGE_RETENTION_DAYS`, 90) never sweeps above
the lowest live cursor, and a cursor idle for `OTS_CURSOR_TTL_DAYS` (30)
expires — a follower away longer than that resynchronises whole when it
comes back.

### What is not here

- **Synchronous hot replication** — the leader acknowledging a write only
  after a follower has applied it. The cursor a follower sets is the ack the
  leader would wait for; what is missing is the wait in the leader's write
  path and the policy (which followers, how long, what when one is down).
- **Consensus (Raft)** — automatic leader election and a quorum write path.
  It needs a consensus library, a dependency this programme does not add
  without the maintainer's decision.
- **Identity database replication** and **asset shipping**, as above.
