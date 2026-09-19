I now have every source I need. Verification summary before writing: all 16 "wrong" items check out against the cited files (e.g. `rocksdb_options_optimize_for_point_lookup` at rocksdb_wrapper.rs:395-396 with `use_iter: false` only on `ID2STR_CF` at storage/rocksdb.rs:108; `insert_version` called in `async fn` bodies at handlers.rs:221/:548 while `IdentityGuard::set` sits inside the `spawn_blocking` closure at routes.rs:657-659; `last_run_generations` at entailment.rs:127-131 with a `std::sync::Mutex<HashMap>`; the label minted at exec.rs:368 outside the `for … in by_ds` loop at :369). Two additional facts settle the amendments: `PreparedSparqlUpdate::on_transaction` (update.rs:140) gives the repo a separable commit for SPARQL primitives, and `IdStr::new` writes `{id:x}` (blank_node.rs:262), closing the hex round-trip question.

# Delta versioning: per-quad change capture with a durable cursor, versions as checkpoint plus patch chain, RDF-star statement provenance

> **Status: design note only.** Nothing here is implemented and nothing in this phase will be. Every code-level claim is cited as `path:line` against the `feat/improvements` worktree (crates against the cargo registry); where a figure does not exist the note says "unmeasured" and names the experiment. Constraints honoured: RocksDB layout and on-disk format unchanged; existing routes, status codes and JSON fields kept; `src/auth/`, the frontend, CI and `Cargo.toml` dependencies untouched.
>
> **Maintainer's steer.** The reliable changelog is built first; SHACL→SQL is *deferred pending measurement*, not rejected; bulk revalidation after a shape or model update is a real, recurring workload. §8.3 specifies the 9M-quad gating experiment.
>
> **Approval required before implementation (§8.4).** The design touches `src/dataset_versions/`, `src/commit_log.rs`, `src/rdf_patch.rs`, `src/provenance.rs` and route mounting in `src/server/mod.rs`, none of which the brief lists; and it leaves one headline limitation in place (§1.1, §3.2: replace-imports of graphs above the capture cap still cut full physical copies).

## 1. Purpose, and what it replaces

### 1.1 A version is a full physical copy

`snapshot_graphs` clears the target snapshot graphs with `bulk_delete_graphs`, then `copy_graph` streams `quads_for_pattern` into `{base}/dataset/{id}/version/{ver}/{slug}` through `bulk_insert_quads` in batches of `const BATCH: usize = 50_000;` (`src/dataset_versions/snapshot.rs:107-110`, `:58-59`). Every version of every selected graph is a second full copy, cut on five paths: the HTTP create route, replace-imports, validate-and-commit, pipelines with `NewVersion` targets, and `create_branch` (`src/dataset_versions/handlers.rs:195`; `src/imports/handlers.rs:422`; `src/dataset_versions/commit.rs:295`; `src/shacl_studio/exec.rs:368-370`; `src/dataset_versions/handlers.rs:498`, `:525-531`).

Two paths are automatic: a replace-import archives the previous contents as a **Published** version (`src/imports/handlers.rs:422-433`) and an identical re-upload still cuts a Draft (`:444`). Retention is manual: `DELETE …/versions/:ver` (409 for Published unless `?force`, `src/dataset_versions/handlers.rs:650`) and `POST …/versions/gc` (`:811`), which skips Published (`:814`), the kind imports produce. Per the readiness audit, "A dataset re-imported N times retains N full copies with no retention policy, no GC and no size accounting" (`docs/notes/readiness-audit-2026-09.md:1041`). Dataset deletion never touches the registry or snapshots (`src/auth/handlers.rs:4085-4112`).

The copy is expensive while made: `bulk_insert_quads` ends with `recount_specific_graphs` (`src/store/engine.rs:1641`) and `copy_graph` calls it per 50 000-quad batch against the growing target (`src/dataset_versions/snapshot.rs:76`), so a 1M-quad graph pays twenty scans of a graph growing to 1M, on top of a minute of bulk-load I/O at 9M (0.14–0.17 Mt/s, `docs/performance.md:586`).

**What this design does and does not change about the import case.** The audit's headline case is only partly addressed. Drafts, pipeline versions, validate-and-commit and branches stop copying (§3.2). The replace-import archive is different: its pre-image is destroyed by `bulk_delete_graphs` in the same call (`src/imports/bulk.rs:436-440`), so a delta can only stand in for the copy when that pre-image was captured exactly, which §2.2 caps at `OTS_CHANGE_CAPTURE_MAX_SCAN` quads per graph. **Above the cap every replace-import still cuts a full physical copy**, exactly as today; what changes is that the copies become visible in size accounting (§2.5, §7) and that the GC rules of §3.7 apply to them. Below the cap, Q10 asks whether the archive may be a delta at all, since the archive is Published and §3.2 makes Published physical. This limitation is stated here so that the design is not read as fixing the audit line it quotes.

### 1.2 Diffs, restore and change detection are O(graph)

`triple_delta` runs `SELECT ?s ?p ?o` per graph and takes `HashSet` differences (`src/data_models/diff.rs:197`, `:245`); `rdf_patch::triples_of` holds one `"s p o"` `String` per quad, both sides at once (`src/rdf_patch.rs:442`, `:452`); `list_branches` does this for every tip on every GET (`src/dataset_versions/handlers.rs:475-480`). Restore issues one `MOVE SILENT GRAPH` per graph (`src/dataset_versions/snapshot.rs:164-178`), forcing a full count-index rebuild each time (§3.6). No timing exists for any of these. LDES capture pays the same inside the write path: `subject_index` scans every quad of a tracked graph (`src/ldes/capture.rs:41`) in `before` (`:113`) and `after` (`:126`). Only `NamedNode` subjects are indexed, hashing `(predicate, object.to_string())` per quad (`:51-52`), so a change inside a blank-node subtree that leaves the blank-node label intact (a SPARQL update on the inner triple) is never detected; a re-import that relabels the node changes the owner's object string and *is* republished, and `describe_entity` includes the blank-node closure (`:60-62`).

### 1.3 The commit log records counts, and there is no durable sequence

`CommitRecord` carries `added: usize`, `removed: usize`, no quads (`src/commit_log.rs:89`); `insert_commit` writes one `INSERT DATA` into `urn:system:commit-log` via `store.update` after the data write has committed (`:16`, `:177`, `:277`), failure only warned (`:82-84`). The counts differ per path: PUT stores post/pre totals (`src/server/routes.rs:1667`), POST a count delta (`:1763`), the patch apply A/D *line* counts (`src/rdf_patch.rs:614`), version operations 0/0 (`src/dataset_versions/handlers.rs:562-598`), and SPARQL 0/0 although the exact delta was computed one screen earlier for the text index alone (`:794` versus `:835`). Boot seed, recovery, reasoners, LDP and plugins record nothing (`src/seed_bundles/mod.rs:398`, `:519`; `src/store/recovery.rs:182`; `src/reasoning/rdfs.rs:88`; `src/ldp/handler.rs:649`, `:833`; `src/plugins.rs:34`).

Ordering is by timestamp: `commit_id` is `uuid::Uuid::new_v4()` (`src/commit_log.rs:126`), `list_commits` orders `DESC(?created)` with no tie-break (`:394`), `CommitsParams` has no cursor (`:299`). `write_generation()` (`src/store/engine.rs:502`) is an `AtomicU64::new(0)` per process (`src/store/query_cache.rs:127`). Oxigraph exposes no durable version: `rocksdb_get_latest_sequence_number` is bound (`oxrocksdb-sys-0.5.9/rocksdb/db/c.cc:2436`; `build.rs:33`) but `storage` is private (`oxigraph-0.5.9/src/lib.rs:11`; `Store::storage()` is `pub(super)`, `store.rs:1099`), and the only persisted number is `LATEST_STORAGE_VERSION = 2` (`storage/rocksdb.rs:37`). A WAL feed would not help even with a fork, since bulk-loader commits are SST ingests with no WAL record (§2.4). The one monotonic sequence in the system is `ldes_members.id INTEGER PRIMARY KEY AUTOINCREMENT` (`src/auth/db.rs:659-660`), per entity.

### 1.4 What this note proposes

One primitive, produced inside the engine's mutation methods and persisted in a dedicated SQLite sequence table: an ordered, durable, per-quad change record. Versions become a physical **checkpoint** plus a **patch chain**, materialised lazily. Statement provenance becomes an opt-in RDF 1.2 annotation written from the same record into a Provenance-role graph. The commit log stays as the human trail and gains a pointer into the sequence. The record also makes the count index exact everywhere, replaces the LDES scans, and feeds the text index, entailment, SHACL revalidation and any derived substrate.

## 2. The change-capture primitive

### 2.1 Record format

One row per logical write **per graph**; rows of one write share a `txn` id. Two numbers identify a row, and they are deliberately distinct: `row` is allocated when the intent row is inserted (before the store is mutated, §2.4) and never changes; `seq` is assigned at commit time inside the critical section of §4.3 and is `NULL` while the row is `pending`. Only `seq` is the durable cursor: chains, pins and `ver:patchToSeq` key on it, and because it is handed out in commit order a row that is still pending can only ever receive a `seq` above every `seq` already assigned.

| Column | Type | Meaning |
|---|---|---|
| `row` | `INTEGER PRIMARY KEY AUTOINCREMENT` | intent identity (the `ldes_members.id` shape, `src/auth/db.rs:660`); allocated at primitive entry, immutable |
| `seq` | `INTEGER NULL UNIQUE` | the durable cursor; assigned from `meta.next_seq` at commit (§4.3); `NULL` while `pending` |
| `epoch`, `txn`, `gen` | `TEXT`, `TEXT`, `INTEGER` | store epoch (§2.4); write UUID; `write_generation()` at the hook, diagnostic only |
| `scope`, `graph_iri` | `TEXT`, `TEXT NULL` | `graph` (one graph; `NULL` `graph_iri` is the default graph) or `store` (targets unknown, §2.2) |
| `origin`, `kind`, `commit_iri`, `actor_iri` | `TEXT` | engine primitive; the `CommitKind` string (`src/commit_log.rs:27`); commit IRI as `insert_commit` mints it (`:171`); actor IRI as `record` mints it (`:76`; `src/server/routes.rs:833` duplicates the format); `NULL` for system writes |
| `extent` | `TEXT` | `full`: payload is the complete net delta; `counts`: payload omitted above the payload cap, `added_n`/`removed_n` exact; `unknown`: nothing about the delta is known (§2.2) |
| `added`, `removed` | `BLOB NULL` | N-Quads 1.2 text, gzip above a threshold (`flate2`, `Cargo.toml:218`); `NULL` unless `extent = 'full'` |
| `added_n`, `removed_n`, `post_count` | `INTEGER` | counts, so listings never decode blobs; graph count after the write |
| `has_bnode` | `INTEGER` | any blank-node subject or object; gates patch export (§5) and provenance (§6) |
| `state`, `created_at` | `TEXT` | `pending` \| `committed` \| `aborted` \| `unknown`; RFC 3339 |

N-Quads rather than RDF Patch, because the patch parser refuses `D` lines with blank nodes (`src/rdf_patch.rs:356-360`) and cannot tokenise triple terms (`:103-111`), while oxrdf prints `<<( … )>>` (`oxrdf-0.3.3/src/triple.rs:431`) and the N-Quads 1.2 parser accepts it (`oxttl-0.2.3/src/line_formats.rs:152`), keeping blank-node labels verbatim (`:85`, `:141`; `oxrdfio-0.2.5/src/parser.rs:227`). Labels round-trip inside one store: `IdStr::new` prints a numerical id as lower-case hex with no leading zeros (`write!(&mut str[..], "{id:x}")`, `oxrdf-0.3.3/src/blank_node.rs:262`) and `BlankNode::new` maps such a label back through `to_integer_id` (`:38`, `:53`, `:333`); that a re-parsed blank node matches on `Transaction::remove` is **untested** (§8.2).

### 2.2 Where the record is produced, per mutation primitive

Inside each engine primitive, between `begin_write` and the drop of its `WriteGuard`. `begin_write` sees all twelve sites but `pub(crate) struct WriteGuard<'a>(&'a TripleStore);` carries no payload (`src/store/engine.rs:304`, `:430`), fires for `rebuild_graph_index`, which mutates nothing (`:1794-1797`), and double-fires when guards nest (`:1446` into `:1281`). The HTTP layer cannot see seed, recovery, reasoner, LDP or plugin writes (§1.3). Oxigraph offers no hook: `Transaction::insert`/`remove` return `()` and `commit(self)` returns `Result<(), StorageError>`, yielding nothing about what was written (`oxigraph-0.5.9/src/store.rs:1455`, `:1499`, `:1629`); the WriteBatch is never iterated (`rocksdb_wrapper.rs:939`), and the only callbacks are the bulk loader's `on_progress`/`on_parse_error` (`store.rs:1784`).

Each primitive does two things: it inserts the **intent row** (`state = 'pending'`, `seq NULL`) at entry, before touching the store, and it **finalises** the row (payload, counts, `committed`, `seq`) at the point in the table below. Per primitive ("free": quads already in a local; "probe": one `contains` per quad; "scan": before-image read):

| Primitive (`src/store/engine.rs`) | Delta finalised at | Added / removed | Scan |
|---|---|---|---|
| `update` `:749` | after `execute()` `:768` | scan / scan | over `static_update_targets` (`:764`); unbounded when `None` (`:825`, `:832`) → a `scope = 'store'` intent row |
| `update_scoped` `:857` | `:877` | as above | the reasoners' path |
| `update_targeted_delta` `:942` | `:971` | free / free | only the non-ground fall-through `:981-988` |
| `batch_update` `:1131` | per statement `:1156`, against `tx` | probe via `Transaction::contains` (`store.rs:1279`) | non-ground statements, via `Transaction::quads_for_pattern` (`store.rs:1254`) |
| `load_reader_with_base` streaming `:1188` | `:1202` | never materialised | `extent = 'unknown'` row in phase 1 (Q2) |
| `insert_quads_and_reindex_into` `:1275` | `fresh` at `:1313`/`:1323` | free for a named target (`:1287-1292`) | default graph only (`:1316`) |
| `graph_store_put` empty target `:1466-1478` | `:1478`, where `.map(\|_\| ())` discards the value | free / empty | no |
| `graph_store_put` replace `:1493-1498` | between `start_transaction` `:1493` and `clear_graph` `:1494` | `quads` (`:1460`) minus pre-image / pre-image | one `quads_for_pattern` in the same transaction |
| `graph_store_post_delta` `:1527` | `:1534` | free | default graph only (`:1535`) |
| `clear_graph_chunked` `:1545`; `graph_store_delete` `:1564` | `quads` at `:1546`; `:1573` | — / free | no |
| `bulk_delete_graphs` `:1588` | before the DROP string | — / scan | O(sum of graph sizes) |
| `bulk_insert_quads` `:1624`; `store_quad` `:1691` | `:1630`; `:1693` | probe (the loop at `:1292`) | no |

Five primitives already hold the exact delta and discard it, three need a probe, five need a scan. Two mechanical rules: **suppress the inner record when guards nest** (`:1446` → `:1281`; one PUT advances the generation four times), so `:1478` and `:1313` are one row; and **never record the commit-log `INSERT DATA`** (`src/commit_log.rs:277`), which a consumer of the row writes. `ground_update_delta` counts a quad with a blank-node subject or object as fresh without a probe (`:1073-1080`); the sink probes those too. A named reifier object is `Term::Triple`, not `Term::BlankNode`, and already goes through `store.contains` (`:1039`).

**Two caps, one for scans and one for payloads.** A scan is taken only when its size is known and bounded: `GraphIndex` gives every target's count in O(1) (`src/store/engine.rs:93`, `:1782`), so the primitive sums the counts of `static_update_targets` and scans only at or below `OTS_CHANGE_CAPTURE_MAX_SCAN` (proposed 250 000 quads; unmeasured). Separately, the "free" primitives hold their whole payload in a `Vec<Quad>` (`:1275-1302`, `:1546-1549`, `:1624-1633`), and serialising a 900k-quad PUT or a 9M-quad import to gzipped N-Quads inside the write path is a cost the 20 % budget governs (bulk load 59 s at 9M, `docs/performance.md:585`; measurement 9). So a payload is stored only at or below `OTS_CHANGE_CAPTURE_MAX_PAYLOAD` (proposed 250 000 quads per row; unmeasured); above it the row has `extent = 'counts'`: counts and `post_count` exact (the count index stays exact), no blob, and a chain cannot cross it. **Writes whose target is a version snapshot graph (`{base}/dataset/{id}/version/…`, `src/dataset_versions/snapshot.rs:100`) are always `counts` rows**, otherwise cutting a checkpoint through `copy_graph` → `bulk_insert_quads` (`snapshot.rs:59`, `:76`) would write the whole graph a second time into `changes.db`. Above the scan cap, or when the targets are `None` (variable `GRAPH ?g`, LOAD, CLEAR, DROP, MOVE), the primitive writes `extent = 'unknown'`: one row per affected graph when the graphs are known after the fact, else the single `scope = 'store'` row. Consumers treat `unknown` as a hard boundary: the count index recounts (the graph, or every graph for a store-scoped row), a chain cannot cross it, LDES falls back to `publish_all` (`src/ldes/capture.rs:169`), a derived copy rebuilds.

**Failure, not crash.** A primitive returning `Err` after its intent row marks the row `aborted` synchronously, so stale `pending` rows never accumulate.

**Context from the handler, and which thread runs the primitive.** Actor, commit IRI and kind reach the engine through a thread-local RAII guard, as `SERVICE` evaluation carries identity today (`IdentityGuard`, `src/federation.rs:86`, `:91-100`). The precedent works only because the guard is set *inside* the `spawn_blocking` closure (`src/server/routes.rs:657-659`); a thread-local set in an `async fn` body does not survive an `.await` and leaks to whatever task next runs on that worker. The rule is therefore: a `WriteContext` guard in `src/store/` is set inside the blocking closure that calls the primitive, never in the async body. This is possible for every data write, which already runs in `spawn_blocking` (`update_targeted_delta` at `routes.rs:773-775`). It is **not** the case for the registry and commit writes that run directly on the async thread: `registry::insert_version` inside `async fn create_version` and `create_branch` (`src/dataset_versions/handlers.rs:221`, `:548`), `update_version_status`/`update_latest_published` in `publish_version` (`:322-341`), and `insert_commit` after the `.await`s of the SPARQL handler (`routes.rs:835`, after `:767`, `:777`). Those rows are recorded with `NULL` actor and commit as system writes (which registry writes are in any case); the commit-log insert is never recorded (rule above). Capture is synchronous on whichever thread runs the primitive: two small SQLite statements on an async thread for the registry writes, measured under item 3.

**System writes are captured but tagged**: registry writes (`src/dataset_versions/registry.rs:10`), entailment materialisation (`src/entailment.rs:46`, `:167-170`), restore staging (`src/dataset_versions/snapshot.rs:165`) and version snapshot graphs (§3.3). Retention (§2.5) is keyed on **registration and role, not IRI prefix**, because `urn:system:inferred:{pipeline}` and `urn:system:reports:{pipeline}` are registered dataset graphs with roles (`src/shacl_studio/exec.rs:253-256`, `:279`; `src/server/routes.rs:7401`) and a prefix rule would punch chain holes.

**Off switch, and what it does not switch off.** `TripleStore::in_memory()` is called ~230 times across `src/` and `tests/`; it gets an in-memory SQLite, and `OTS_CHANGE_CAPTURE=off` disables only the SQLite sink. Delta computation, the exact `GraphIndex::adjust` and the nested-guard suppression are part of the primitive and run regardless, so the test stores exercise the same count-index code path as production and the `scan_count()` test (§2.6) is meaningful with the sink off. Per-test overhead of the in-memory sink is unmeasured (§8.2).

### 2.3 Persistence, with the RocksDB layout unchanged

**(a) A table in `auth.db`.** Feasible without editing `src/auth/`: `entailment.rs` creates `identity_policy` on the shared pool and says "the identity database's schema (src/auth) is not touched" (`src/entailment.rs:385-386`, `:396-398`), and it would ride the SQLite online backup (`src/backup/mod.rs:184`). Rejected: a per-write hot table with blobs would share the identity pool and its `PRAGMA busy_timeout=5000` (`src/auth/db.rs:338`) with every login and ACL lookup; `ldes_members` already grows unbounded there (`docs/ldes.md:84-85`); and the backup advantage is illusory since the RDF dump and SQLite copy are taken sequentially (`src/backup/mod.rs:168`, `:184`).

**(b) A dedicated SQLite database owned by `src/store/`, recommended.** `{data_dir}/changes/changes.db`, opened by `TripleStore::open` (`src/store/engine.rs:318`), holding `changes`, `meta` (epoch, `next_seq`, clean-shutdown marker, `first_seq_commit`), `graph_state` (per graph: last committed `seq`, `post_count`), `cursors`, `materialisations` (§3.3) and `dataset_settings` (the §5–6 opt-ins). `TripleStore` has two constructors (`:318`, `:355`) and is `Clone`, so one `Arc<ChangeLog>` field propagates. The subdirectory is tidiness, not safety: `is_rocksdb_file` matches only RocksDB names (`CURRENT`, `MANIFEST-`, `.sst`, `.log`, …, `src/store/recovery.rs:122-132`), SQLite's sidecars are `-wal`/`-shm`, and the regression test lists `auth.db-wal` and `auth.db-shm` among files that must not be quarantined (`:272-275`); `tantivy/` is the precedent (`src/main.rs:502`). `rusqlite` is already a dependency (`Cargo.toml:178-181`).

**(c) Append-only patch files per dataset.** Rejected for now: no range query for `since=`, retention becomes a filesystem problem, and a loose `*.log` at the data-dir root *would* be quarantined (`src/store/recovery.rs:129`).

Nothing in (b) writes to RocksDB. The commit node gains one literal, `ver:seqTo`; the body is not stored as a literal, because `id2str` is append-only (`insert_str` is a plain put, `oxigraph-0.5.9/src/storage/rocksdb.rs:1028`; `remove_encoded` never deletes from it, `:1035-1077`) and a compacted body could never be reclaimed.

### 2.4 Durability: not atomic with the data, and how gaps are found and healed

The transactional paths could carry extra *RDF*: `PreparedSparqlUpdate::on_transaction` stores a `BorrowedReadable` transaction and the matching `execute` arm returns without committing (`oxigraph-0.5.9/src/sparql/update.rs:140`, `:150`, `:200-207`; the `OwnedReadable` arm that `on_store` takes commits itself at `:197`), so one `Transaction::commit()` applies quads and an `INSERT DATA` in one `rocksdb_write_writebatch_wi` (`store.rs:1338`; `rocksdb_wrapper.rs:1048-1057`); `batch_update` and the PUT replace hold such a transaction (`src/store/engine.rs:1154`, `:1493`). The bulk-loader paths ingest SST files and can carry nothing (`storage/rocksdb.rs:1457-1473`; `src/store/engine.rs:1200`, `:1300`, `:1631`), and nothing orders an SST ingest against a WriteBatch under `kill -9` (not established; measurement 7). So a crash can fall between the two writes, and the design makes every gap **detectable**.

**Intent rows.** Each primitive writes its row *before* mutating the store with `state = 'pending'` and `seq NULL`, commits the data, then inside the §4.3 critical section assigns `seq`, flips the row to `committed` and updates `graph_state`. At the next open every `pending` row is resolved by probing: all `added` present and all `removed` absent means committed (it is assigned the next `seq` at boot; if two pending rows touch the same graph their relative commit order is unrecoverable and both become `unknown`); the inverse means `aborted`; anything mixed, `extent != 'full'`, or `scope = 'store'` becomes `unknown`. Probing is exact for the transactional and SST paths, which are all-or-nothing: `BulkLoader::without_atomicity` (`store.rs:1778` → `storage/rocksdb.rs:1396`) is never called by OTS (grep `src/`: none), so the RocksDB loader keeps `atomic = true`; whether one `insert_stt_files` call across nine column families is atomic under `kill -9` is RocksDB behaviour not readable here (measurement 7). The one non-atomic primitive, `clear_graph_chunked` at or above 100 000 quads (`src/store/engine.rs:1550-1560`), is correctly reported as `unknown`. Cost: two SQLite statements per write; unmeasured.

**Count reconciliation, and its blind spot.** `TripleStore::open` rebuilds `GraphIndex` eagerly (`:318`); every graph whose fresh count differs from `graph_state.post_count` gets a synthetic `unknown` row. This is a second line of defence for a write that produced no row at all, and it is **blind to same-count swaps**: a `DELETE/INSERT WHERE` that replaces N quads with N others leaves the count unchanged. The design does not rely on it for the crash case: because the intent row (per graph, or `scope = 'store'` when targets are `None`) is written before the mutation, a crash after `execute()` and before the flip leaves a `pending` row, which resolves to `unknown` when it cannot be probed, closing every chain it could affect. The residual gap is a write for which no primitive wrote an intent row, which the design treats as a bug, not a state.

**Epochs.** `meta.epoch` is re-minted whenever the store is replaced wholesale: recovery quarantines the RocksDB files and reloads `rdf.nq.gz` through `load_reader` (`src/store/recovery.rs:28`, `:182`); a backup restore runs `DROP ALL` then `load_reader` (`src/backup/mod.rs:543-548`). Both flow through engine primitives and so produce `unknown` rows without any change to `src/backup/`. `changes.db` is not in the backup payload (exactly the RDF dump, the SQLite copy and `manifest.json`, `src/backup/mod.rs:159-205`), so after a restore its rows belong to a dead epoch and no chain from it is applied.

**Healing** is one operation: a chain meeting an `unknown` row or an epoch boundary is closed and the next version cut is a checkpoint; never repair by guessing.

### 2.5 Retention and compaction

Rows are pinned while referenced by a delta version's range (§3.1), an LDES sync bookmark (`src/ldes/store.rs:144`) or a live `cursors` row. A cursor carries an `owner` (the creating user or `system`), an `expires_at` (default `OTS_CURSOR_TTL_DAYS`, proposed 30, refreshed by every `PUT`), and can be deleted by its owner or an admin (§7), so an abandoned cursor stops pinning after its TTL rather than forever. Below the lowest pin, rows older than `OTS_CHANGE_RETENTION_DAYS` (proposed 90; nothing prunes `urn:system:commit-log` today) have blobs nulled, then are deleted. Compaction of a pinned range is the version mechanism itself: materialise, snapshot, re-point, unpin. Rows for unregistered, Entailment-role, staging and version-snapshot graphs are deleted after 24 hours. Change-log size, pinned ranges, materialisation disk use and checkpoint counts are exposed on an admin endpoint (§7), closing the audit's "no size accounting".

### 2.6 What the primitive unifies

- **Count index, exact everywhere.** `GraphIndex::adjust` is used only by `update_targeted_delta` (`src/store/engine.rs:197`, `:974`); a recount "made a 500-quad insert into a 900k-quad graph cost a 900k-quad scan" (`:955-958`). With a `full` or `counts` row every recount and rebuild site collapses to an adjust: `:772`, `:882`, `:988`, `:1166`, `:1204`, `:1311`, `:1316`, `:1507`, `:1641`; the `scans` counter and `scan_count()` (`:102`, `:126`) become the regression test, and it runs with the sink off (§2.2).
- **Text index.** `text_index_apply_delta` (`src/server/mod.rs:486`) receives quads at two lines only (`src/server/routes.rs:794`, `:1741`); everything else refreshes whole graphs (`:1851-1858`).
- **Entailment.** `after_additive_write` is called only from GSP POST (`src/server/routes.rs:1753`); every other write rebuilds (`src/entailment.rs:184`, `:197-225`). Any add-only `full` row takes the additive path.
- **LDES.** Changed entities = IRI subjects of the row's quads plus the owner reached from a blank-node subject by a reverse lookup (`quads_for_pattern(None, None, Some(bnode), graph)`, repeated up the tree to a depth of `OTS_LDES_BNODE_DEPTH`, proposed 8). This is bounded per row, not per graph, but for IFC and geometry data with deep blank-node trees its cost is unmeasured (measurement 11); above `OTS_LDES_BNODE_SUBJECTS_MAX` blank subjects in one row (proposed 10 000) capture falls back to `publish_all` for that graph. Both graph scans go (`src/ldes/capture.rs:113`, `:126`); `describe_entity` (`:62`), which is forward-only, remains.
- **SHACL revalidation** (§8.3), **versions** (§3), **a future mirror feed**: `seq` is the durable cursor `write_generation()` cannot be.

## 3. Version model: checkpoint plus patch chain

### 3.1 Two kinds of version, one registry

A **checkpoint** is a version as today: physical snapshot graphs plus the description `insert_version` writes into `urn:system:dataset-version-registry` (`src/dataset_versions/registry.rs:10`, `:263-287`). A **delta version** has no `ver:subGraph` and names a base checkpoint, a covered graph set and a sequence range, with new predicates in `urn:system:vocab/` (`:11`):

| Predicate | Meaning |
|---|---|
| `ver:baseVersion` | the checkpoint the chain starts from (`prov:wasDerivedFrom` keeps its branch-only meaning, `:231-240`) |
| `ver:graphMap` | one node per covered graph: `ver:source <src>`, `ver:target <{ver_iri}/{suffix}>` — the same IRI `snapshot_graphs` would mint (`src/dataset_versions/snapshot.rs:100`), unpopulated until materialised |
| `ver:patchEpoch`, `ver:patchFromSeq`, `ver:patchToSeq` | epoch and exclusive-lower/inclusive-upper bounds over the covered graphs |
| `ver:changeCount`, `ver:contentHash` | `added_n + removed_n` over the range; content identity (§3.5) |
| `ver:chainBroken`, `ver:materialised` | set by the reconciler when an `unknown` row falls inside the range; present while a materialisation is live |
| `ver:seqTo` | on the **commit** node: the pointer into the sequence |

**Covered graphs.** A delta covers exactly the source graphs selected at cut time, as `snapshot_as_version` takes `source_graphs` today (`src/dataset_versions/mod.rs:20-25`); the set must be a subset of its base's covered set, otherwise the cut is a checkpoint. A graph registered to the dataset after the base is therefore either excluded from the delta or forces a checkpoint; it is never silently unversioned. Because the delta's `ver:graphMap` targets are the deterministic snapshot IRIs, the JSON fields `snapshot_graphs` and `source_map` (`src/dataset_versions/models.rs:33-36`) are populated for a delta exactly as for a checkpoint, so `?graph=<suffix>` filtering in `get_version_data` (`src/dataset_versions/handlers.rs:107-113`), the snapshot→source mapping in `private_snapshot_graphs` (`src/server/routes.rs:3297-3301`) and the drop list in `purge_version` (`handlers.rs:607`) work unchanged; the IRIs simply resolve to empty graphs until materialised, and a purge of a never-materialised delta drops nothing.

A delta carries no `ver:subGraph` deliberately: `get_snapshot_graphs` is the only reader of that predicate (`src/dataset_versions/registry.rs:165-179`, from `:82-83` and `:143-144`) and `purge_version` drops every graph it lists (`src/dataset_versions/handlers.rs:607`), so pointing a delta at its *base's* snapshots would let a GC of the delta delete the base; pointing it at its own snapshot namespace is safe. Resolver rule: **`ver:subGraph` present means physical, IRIs returned unchanged (no migration); `ver:graphMap` without `ver:subGraph` means resolve the chain.** `ver:patchToSeq` at cut time is the highest committed `seq` over the dataset's covered graphs; pending rows of any dataset are irrelevant to the boundary, because a pending row receives its `seq` only at commit and that `seq` is above every `seq` already assigned (§2.1, §4.3), so a multi-minute import into another dataset never blocks a cut. `DatasetVersion` (`src/dataset_versions/models.rs:26`) gains the matching `#[serde(default)]` fields (§7). Snapshot immutability is convention only (unregistered graphs, no write guard, `src/server/routes.rs:3268-3269`); unchanged here, and the persisted hash lets an audit detect drift.

### 3.2 When a checkpoint is cut

- **On publish, always.** `publish_version` (`src/dataset_versions/handlers.rs:313`) materialises and snapshots before flipping status, so a Published version stays physical, retention-independent and present in backups (`src/backup/mod.rs:168`). Consequence, stated plainly: the replace-import archive is created Published (`src/imports/handlers.rs:422-433`) and therefore remains a full physical copy under this rule, independently of the capture cap (§1.1; Q10 asks whether to relax the rule for import archives).
- **Every N deltas** on one base (proposed 100) or **on size**, when `ver:changeCount` exceeds a fraction of the base's count (proposed 25 %; unmeasured).
- **At an `unknown` row or epoch boundary, or when a `counts` row falls inside the range** (a chain needs payloads). The replace-import already cuts its Published copy in `before_replace` before `bulk_delete_graphs` (`src/imports/bulk.rs:436-440`, reaching `snapshot_as_version` at `src/imports/handlers.rs:422`), so its pre-image is a checkpoint; above `OTS_CHANGE_CAPTURE_MAX_SCAN` the delete also produces an `unknown` row, which is the same boundary twice.
- **When the covered set is not a subset of the base's** (§3.1).
- **Manually**: `POST …/versions` gains optional `"checkpoint": true`.

Drafts and staged versions default to delta, as do the pipeline's auto-versions (`src/shacl_studio/exec.rs:368`) and validate-and-commit (`src/dataset_versions/commit.rs:295`), removing the synchronous copy from those paths.

### 3.3 Consumers that need physical graphs, and lazy materialisation

Every consumer takes `DatasetVersion.snapshot_graphs` or `.source_map` and hands the IRIs to the store:

| Consumer | Line | Served by |
|---|---|---|
| `GET …/versions/:ver/data` | `src/dataset_versions/handlers.rs:107-141` | materialise |
| restore | `:394-407` | inverse patch, else materialise (§3.6) |
| diff, both modes; branch ahead/behind | `:713-744`, `:755-787`; `:475-480` | chain (§3.5) |
| `create_branch` → `clone_version` | `:525-531` | registry only: same base, covered set and range |
| `purge_version` | `:602-617` | drops the `ver:graphMap` targets (materialisation, if any), validation sibling, registry triples; never the base |
| browse `versions=`; dataset-service `?version=` | `src/server/routes.rs:3341`, `:3397`, `:3239`; `:4651`, `:4810` | materialise |
| private-graph filter via `source_map` | `:3277`, `:3299` | unchanged; the delta's `source_map` is its `ver:graphMap` |
| saved queries Pinned/Default; version tests | `src/saved_queries/exec.rs:154`, `:176-183`; `testing.rs:55-62` | materialise |
| archive text indexing; backup | `src/imports/handlers.rs:497`; `src/backup/mod.rs:168` | physical only |

The sibling `{ver}/validation` (`src/shacl_studio/bindings.rs:179-180`) is never under `ver:subGraph` and is dropped by `purge_version` explicitly (`src/dataset_versions/handlers.rs:607-612`); delta purge and GC drop it the same way.

One resolver, `dataset_versions::resolve::graphs(state, dataset_id, version, caller)`, fronts every "materialise" row. For a physical version it returns the IRIs without taking any lock. For a delta it looks up `materialisations`; on a miss it builds into the `ver:graphMap` targets by whichever direction is cheaper per graph, judged by `added_n + removed_n`: `copy_graph` from the base (`src/dataset_versions/snapshot.rs:58`) then `apply_delta` forward, or copy the live graph and apply the inverse of `(patchToSeq, head]` backwards, recording `last_used` and `ver:materialised`. Once built, a delta is indistinguishable from a checkpoint to every consumer in the table, and a checkpoint policy that promotes it (§3.2) only adds `ver:subGraph`.

**A cold read is a store write, and the design says so.** The build goes through `bulk_insert_quads` and `apply_delta`, each of which calls `begin_write`, which marks the mirror dirty and empties the query cache for the **whole store** (`src/store/engine.rs:430-434`, `:1629`); the mirror rebuild that follows is inline below 50 000 triples and background above, and was measured at ~29 s for a 1.7M-triple store (`src/store/parallel_mirror.rs:84-92`). The TTL sweep through `bulk_delete_graphs` (`:1589`) invalidates again. The semaphore, timeout and 507 below bound duration and disk, not this side effect. Rules adopted: (i) **anonymous callers never trigger a build** — the read routes run under `optional_auth` (`src/dataset_versions/routes.rs:9-10`), and an unauthenticated request for an unmaterialised delta answers 503 with `Retry-After`, while Published versions, the ones anonymous readers are entitled to, are always physical (§3.2); (ii) the build is **one write bracket**: copy and `apply_delta` run under a single outer `WriteGuard` so the store is invalidated once, not twice; (iii) the sweep deletes all expired materialisations of a tick in one `bulk_delete_graphs` call; (iv) the cost per cold read is measurement 10 and, if it dominates, the fallback is eager materialisation at cut time for Draft versions on datasets above a size threshold, which reintroduces the copy the design removes and is therefore a last resort. The build takes a **per-version build mutex** (not the dataset's version lock of §4.2, which it holds only long enough to read the record, so operations on other versions and reads of physical versions do not wait on a cold read), a permit on `expensive_semaphore` (`src/server/mod.rs:277`; `available_parallelism/2`, `:227-231`), and runs under the `write_timeout_secs` bracket the dataset delete uses (`src/server/mod.rs:268`, default 120 at `:337`; `src/auth/handlers.rs:4095-4097`), answering 503 on timeout rather than holding a request thread for minutes. A disk budget `OTS_MATERIALISATION_MAX_QUADS` refuses with 507 (Q6; proposed default in §8.6). Sweeping rides the 500 ms accelerator tick (`src/server/mod.rs:2393`) after `OTS_MATERIALISATION_TTL_SECS` (proposed 3600) or on purge, via `bulk_delete_graphs`, which removes index entries without a rebuild (`src/store/engine.rs:1613-1615`).

### 3.4 `apply_delta`: a new engine primitive, not SPARQL

Materialisation, inverse restore and the HTTP patch apply go through `TripleStore::apply_delta(added, removed)`: one `start_transaction()`, `Transaction::remove` then `insert` per quad, one `commit()` (`oxigraph-0.5.9/src/store.rs:580`, `:1499`, `:1455`, `:1629`), producing its own rows and an exact adjust. Reasons: `DELETE DATA` cannot name a blank node (`GroundQuad::try_from`, `spargebra-0.4.6/src/parser.rs:1439`; `TryFrom<Term> for GroundTerm` rejects `Term::BlankNode`, `term.rs:56-64`) and `Transaction::remove` can; `store.update` forces a recount of every target (`src/store/engine.rs:772`), which the patch apply pays today (`src/rdf_patch.rs:598`); and the per-graph `MOVE` loop leaves a multi-graph version observable half-restored (`src/dataset_versions/snapshot.rs:164-178`). Caveat: a transaction "keeps the complete set of changes into memory" (`store.rs:552-553`), so above `OTS_APPLY_DELTA_TX_MAX_QUADS` (proposed 50 000, the `clear_graph_chunked` size, `quads.chunks(50_000)`, `src/store/engine.rs:1554`; measurement 5 sets it) the primitive chunks and **loses cross-chunk atomicity**. Callers must therefore choose: a materialisation build may chunk freely, because the target graph is unobservable until the `materialisations` row is written; a restore may not (§3.6). RSS per staged quad is unmeasured.

### 3.5 Diffs from the chain, and content identity

When both sides sit on one chain in one epoch, the diff is the net fold of `added`/`removed` over the range (add-then-delete cancels), O(delta) rather than O(|from| + |to|), rendered into the JSON and RDF Patch shapes of `diff_versions` (`src/dataset_versions/handlers.rs:700`, `:713`) with an additive `"source": "chain" | "materialised"` field; otherwise the set-difference path remains; ahead/behind is the same fold. `version_revision` is the right shape for `ver:contentHash` (sorted lines, SHA-256, already an `If-Match` token, `src/data_models/diff.rs:225`; `src/data_models/handlers.rs:898-911`) after three fixes: triple terms collapse to `"<< >>"` (`diff.rs:303`), literals are re-serialised unescaped (`:294`), the digest is truncated to 128 bits (`:241`).

### 3.6 Restore: inverse patch versus `MOVE`

`MOVE` forces a full rebuild one level below the match arm: spargebra has no Move variant and rewrites `MOVE SILENT GRAPH <a> TO GRAPH <b>` into `[Drop, DeleteInsert, Drop]` (`spargebra-0.4.6/src/parser.rs:1294-1301`); `static_update_targets` returns `None` for `Drop` (`src/store/engine.rs:832`); `update()` then calls `graph_index.rebuild`, one count per named graph, per restored graph (`src/dataset_versions/snapshot.rs:172`). No restore timing exists. `ADD` would stay inside `static_update_targets` (`parser.rs:1284-1291`) but merges rather than replaces and reopens the emptied-graph window `MOVE` closed (`snapshot.rs:152-158`).

Inverse restore has two tiers, so that it is never less atomic than today's per-graph `MOVE`. **Tier 1**, when the chain is intact to head and the folded inverse of `(target.patchToSeq, head]` is at or below `OTS_APPLY_DELTA_TX_MAX_QUADS`: swap added and removed, reverse the order, apply through `apply_delta` in **one** transaction across all covered graphs, which is strictly better than `MOVE` (atomic across graphs, no rebuild); the restore produces its own rows (`kind = 'restore'`) with exact adjusts. **Tier 2**, when the fold is larger or the chain is broken: materialise the target version into its snapshot graphs (§3.3, chunked, unobservable) and then run today's per-graph `MOVE` (`snapshot.rs:164-178`), which keeps today's per-graph atomicity and today's rebuild cost. There is no tier in which a single graph is observable half-restored. The handler's bracketing (`src/dataset_versions/handlers.rs:406-438`) is unchanged.

### 3.7 GC rules and dataset delete

Extending `gc_versions` (`src/dataset_versions/handlers.rs:811-828`): Published never collected (`:814`); a checkpoint that is any live version's `ver:baseVersion` never collected, with `DELETE` answering 409 as for Published (`:650`); collecting a delta deletes registry triples, its materialisation (the `ver:graphMap` targets) and validation sibling, touches no other snapshot, unpins its range. Dataset delete remains a leak (`src/auth/handlers.rs:4085-4112`) whose fix is a `src/auth/` edit; in scope are a `purge_all_for_dataset` helper and an admin sweep for `…/version/…` graphs no registry record references. Graphs registered to more than one dataset are **excluded from delta versioning in phase 3** (Q5).

## 4. Concurrency

### 4.1 The races

`create_version` checks `version_exists` at `src/dataset_versions/handlers.rs:168` and inserts at `:221` with nothing held between, while `snapshot_graphs` clears targets before copying (`src/dataset_versions/snapshot.rs:107-110`): two concurrent creates of one label both pass, the second's clear wipes the first's partial copy, and both `INSERT DATA` into one IRI, leaving duplicate `ver:status` triples (`get_version` takes the first, `src/dataset_versions/registry.rs:136-137`). `create_branch` has the same shape; the pipeline mints one `%Y%m%d%H%M%S` label per run, shared by every dataset in the pipeline (`src/shacl_studio/exec.rs:368-369`), with no `version_exists` check before `insert_version` (`:406`); `restore_version` reads at `:394` and restores at `:407`, and a concurrent gc (`:824`) can drop the graphs between. No lock exists in `src/dataset_versions`, `begin_write` only bumps counters (`src/store/engine.rs:430-432`), and oxigraph's `Store::start_transaction` (`store.rs:580-582`) is a `WriteBatchWithIndex` over a snapshot with no conflict detection (`rocksdb_wrapper.rs:475-495`; the plain `WriteBatch` at `:461-473` is the non-readable variant).

### 4.2 The lock to add

A per-dataset `tokio::sync::Mutex` (the guard spans `.await` points between `spawn_blocking` calls) held across the whole version operation: label check, snapshot or range selection, registry insert, bindings snapshot, commit record. Placement: `static VERSION_LOCKS: OnceLock<DashMap<String, Arc<tokio::sync::Mutex<()>>>>` in `src/dataset_versions/`, reusing the `static … OnceLock` plus `get_or_init` shape of `entailment::last_run_generations()` (`src/entailment.rs:127-131`, whose map is a `std::sync::Mutex<HashMap<String, u64>>`, `:128`); `dashmap` is already a dependency (`Cargo.toml:317`) and `OAuthSessions` the keyed-map precedent (`src/auth/oauth.rs:55`). Acquired in create, branch, publish, restore, delete, gc and `snapshot_affected_versions`; the resolver takes it only to read the record and releases it before a build, which is guarded by the per-version build mutex of §3.3. No cross-process lock is needed, since RocksDB's `LOCK` file makes the store single-process (`src/store/recovery.rs:123`). The pipeline label collision needs `version_exists` plus a bump regardless; the pinning test is a concurrent double `create_version`, which today leaves duplicate `ver:status` triples.

### 4.3 Sequence order versus commit order

With no store-level lock, two writers touching the same quad compute net deltas against pre-states the other overwrites; replaying rows in an order different from commit order would diverge from the store. Serialising every data write to a delta-versioned dataset would settle it at a cost the mixed benchmark (4 writers on one graph, 36.8k quads/s, p95 85 ms, `docs/performance.md:655`) makes visible and unmeasured. The rule adopted: **`seq` is assigned inside a short critical section that brackets only the RocksDB commit and the `pending → committed` flip**, so `seq` order equals commit order without serialising evaluation. Two conditions make this achievable in the repo:

1. **The commit must be the repo's call.** For the bulk paths it already is (`loader.commit()` at `src/store/engine.rs:1202`, `:1302`, `:1633`), as for `batch_update` (`tx.commit()`, `:1163`) and the PUT replace (`:1498`). For `update`, `update_scoped` and `update_targeted_delta` it is not: they call `.on_store(&self.store).execute()` (`:765-768`, `:876-877`, `:963-966`), and `on_store` evaluates *and* commits inside oxigraph (`OwnedReadable` arm, `update.rs:100-104`, commit at `:197`). Those three primitives therefore switch to `start_transaction()` plus `.on_transaction(&mut tx).execute()` (`update.rs:140`), whose `BorrowedReadable` arm returns without committing (`:200-207`), followed by the repo's own `tx.commit()`. Memory is unchanged: `on_store` already holds a readable transaction of the same kind until it commits. This is a mechanical change to three call sites in `src/store/engine.rs`, and it is also what §2.4 and Q3 rely on.
2. **The section is keyed per dataset, taken in sorted dataset-id order** when a write touches graphs of several datasets (`batch_update` accepts arbitrary statements, `:1131-1163`), and is the **global section** when the targets are `None`, since the affected datasets are unknown until after evaluation. Reasoners (`src/reasoning/rdfs.rs:88`), registry writes and restore staging go through the same primitives and take the same short section; none of them is serialised for the duration of its evaluation.

The intent row keeps its `row` id; the section assigns `seq` from `meta.next_seq`, flips the state and updates `graph_state` in one SQLite statement, so nothing is re-sequenced and pins never move. Chains are also verified by content hash at checkpoint time (§3.5); a divergence that slips through is marked `ver:chainBroken` and healed by checkpoint. The section's cost is measurement 3.

`write_generation()` stays what it is: in-process, `AtomicU64::new(0)` per boot (`src/store/query_cache.rs:127`), two bumps per write and four when guards nest (`src/store/engine.rs:432`, `:309`), the cache-invalidation invariant at `src/store/query_cache.rs:9-19`. `seq` is durable within an epoch and orders cursors and chains.

## 5. RDF Patch as the delta format

### 5.1 What the parser and generator support and lack

Supported: `H`, one `TX` with `TC`/`TA` (a second `TX` is rejected, `src/rdf_patch.rs:311`), `PA`/`PD`, `A`/`D` with three or four terms; `to_sparql_update` flushes a block at each polarity flip (`:427`); `generate` emits `H id <urn:uuid:…>`, sorted `D` then `A`, `TC .` (`:466`); media type `application/rdf-patch` (`:30`). Lacking:

1. **Blank nodes in `D`**: `allow_bnode = !delete` (`:356-360`) while `generate` prints `_:` labels (`:452`), so a generated diff does not round-trip.
2. **Triple terms**: the `<` arm scans to the first `>` (`:103-111`); a prerequisite for patching annotations.
3. **Inverse**: swap `A`/`D`, reverse op order (so the flush at `:427` reproduces it), fresh `H id`, `H prev <forward id>`.
4. **Position and precondition headers**: `H seq`, `H epoch`, `H dataset`, `H commit`, and an optional `H base-digest` checked against `ver:contentHash` on apply (precedent: `If-Match` on `version_revision`, `src/data_models/handlers.rs:898-911`).
5. **Graph lifecycle**: no line creates or drops a graph (a `DELETE DATA` that empties one drops the index entry, `src/store/engine.rs:203`); graph existence is a flag on the row.

The only producer today emits the wrong direction for a restore: for `other == "live"` the mapping is (live, snapshot, live), so applying the emitted patch is a no-op (`src/dataset_versions/handlers.rs:700`, `:713-744`); §3.6 replaces it with the inverse of the chain.

### 5.2 Division of labour, and blank nodes

Internally, chains are quads applied through `apply_delta`. RDF Patch is the **HTTP-boundary format**: `GET …/diff/:other?format=rdf-patch`, `POST /api/datasets/:id/patch` (`src/rdf_patch.rs:507`) and the routes of §7. Items 2–5 land in `src/rdf_patch.rs`; item 1 is handled by routing the apply endpoint through `apply_delta`, with the caveat that a client-supplied `_:` label addresses a stored blank node only if it is the store's own hex id (`oxrdf-0.3.3/src/blank_node.rs:53`, `:262`), which an exported patch carries and a hand-written one does not.

`BlankNodeMode` defaults to `Preserve` and no production code calls `with_blank_node_mode` (grep outside `engine.rs`: none), so `Canonical` and `Skolem` are dead at runtime (`src/store/engine.rs:64-83`). Delta versioning does **not** require Skolem, since rows capture the store's own labels. For datasets exchanging patches externally, offer `Skolem` as a **per-dataset opt-in** in `dataset_settings`, applied in `parse_quads` on import (`:1260`). Consequences: re-imports stop producing spurious `D`+`A` pairs; exported data no longer contains blank nodes (documented at the point of opting in); SPARQL `INSERT DATA` with `_:` terms still mints blank nodes; the `"<<triple>>"` collapse in canonical hashing (`opengraph/src/canonical.rs:119`) is reachable only under `Canonical`. Blank-node prevalence is not established (§8.2).

## 6. RDF-star statement provenance

### 6.1 The model as implemented

This is RDF 1.2, not the RDF-star CG model (`docs/sparql-12.md:14` and `docs/faq.md:17` still teach the CG model; `docs/standards.md:103` is correct): `oxrdf::Triple.subject` is `NamedOrBlankNode` (`oxrdf-0.3.3/src/triple.rs:540`), so a triple term is only ever an object. `<< s p o >>` is reifier shorthand: spargebra emits `r rdf:reifies <<( s p o )>>` with a fresh blank node and does **not** assert the base triple (`spargebra-0.4.6/src/parser.rs:2044-2059`; pinned by `tests/sparql12_conformance.rs:86-104`); `s p o ~ <r> {| … |}` asserts it and attaches the reifier (`parser.rs:342-362`). A **named** reifier is what this design needs; the grammar accepts it in ground data (`parser.rs:1619-1621`; `term.rs:535`), `ground_update_delta` carries it through by clone (`src/store/engine.rs:1066-1071`) and probes it with `store.contains` (`:1039`), so it is **idempotent** where the bare form mints a new blank node every time, and **deletable**, since `DELETE DATA` rejects blank nodes (`term.rs:56`). No test uses `~ <iri>` or `{| |}` (`{|` appears only in a doc comment, `tests/sparql12_conformance.rs:13`). **The end-to-end path is unverified**; the first provenance task is a test that `INSERT DATA { <r> rdf:reifies <<( :s :p :o )>> }` executes, leaves one quad when run twice, is removed by `DELETE DATA`, and that Turtle 1.2 `{| |}` survives import.

### 6.2 Reifier and annotation set

The reifier is a deterministic IRI, `{base}/statement/{hash}`, a 128-bit digest of the canonical N-Quads of `(g s p o)`; oxigraph's own v1-to-v2 migration derives its reifier by hashing the `Triple` (`oxigraph-0.5.9/src/storage/rocksdb.rs:228`, `:246-258`), but as a blank node. Each change is a separate event node, so remove-then-re-add keeps both histories:

```
<{base}/statement/{h}> rdf:reifies <<( s p o )>> ; ots:graph <g> .   # once per statement
<{base}/statement/{h}> ots:change <{base}/statement/{h}/{seq}> .
<{base}/statement/{h}/{seq}> ots:op "add" ;                          # or "remove"
    prov:wasGeneratedBy <{base}/commit/{uuid}> ; ots:seq "…"^^xsd:integer ;
    prov:wasAssociatedWith <{base}/users/{uid}> ; prov:atTime "…"^^xsd:dateTime .
```

A derived triple adds `ots:derivedBy <rule or regime IRI>`. Two profiles: **minimal** (`ots:change`, `ots:op`, `prov:wasGeneratedBy`, `ots:seq`: four quads per event) and **full** (six, with actor and time denormalised for users who cannot join `urn:system:commit-log`, admin-only over `/store`, `src/server/routes.rs:2094`), plus two quads per statement. **Blank-node statements are not annotated** unless the dataset runs Skolem: a reifier hashed over `_:` labels is stable only inside one store, and after export and re-import under `Preserve` the labels change and the reifier no longer resolves; `has_bnode` gates it and the skip is recorded as a gap event.

### 6.3 Placement: a per-dataset Provenance-role graph

Annotations go to `urn:ots:statement-provenance:{dataset_id}`, registered with `GraphKind::Provenance`, the precedent being `urn:ots:property-states:{dataset_id}` (`src/property_states.rs:51-52`, `:163`).

| Concern | Behaviour today | Action |
|---|---|---|
| `/sparql` and `/store` visibility | registered graphs enter `get_accessible_graph_iris` with no role filter (`src/auth/db.rs:4139`, `:4185-4187`) and the injected `FROM` prologue (`inject_from_clauses`, `src/server/routes.rs:5120`; the authorised-scope rewrite is `:5194`); `/store` blocks only `urn:system:` (`:2094`) | none for `/sparql`: users can join annotations to data, and an unscoped `SELECT ?s ?p ?o` over the dataset **does** see them (below) |
| browse, facets and any path enumerating dataset graphs | role-blind graph lists | **in scope**: exclude Provenance-role graphs by default, include on explicit request |
| reasoning premise | `is_reasoning_source` excludes `Provenance` (`src/conformance.rs:70-81`); rdfs4a would otherwise type reifiers (`src/reasoning/rdfs.rs:326`) | none |
| entailment re-run | `materialized_datasets_for` is role-blind (`src/entailment.rs:100-108`) | exclude Provenance-role graphs (in scope) |
| SHACL | `pipeline_covers_graph` ignores role (`src/shacl_studio/gate.rs:466`); `sh:nodeKind` scores a triple term false on all kinds (`src/shacl/constraints.rs:207`) | skip Provenance-role graphs in the gate (in scope) |
| LDES | `tracked` is role-blind (`src/ldes/store.rs:70`); IRI reifiers would become entities (`src/ldes/capture.rs:51`) | exclude Provenance-role graphs (in scope) |
| version cuts | `create_version` takes the caller's graph selection | exclude Provenance-role graphs from the default selection (in scope) |
| JSON-LD export (ICDD already skips Provenance-role graphs, `src/containers/mod.rs:612-631`) | errors mid-stream after headers (`oxjsonld-0.2.5/src/from_rdf.rs:513`; `src/server/routes.rs:1312` before `:1314`) | 406 before headers for a graph flagged with triple terms |
| RDF Patch, diff, ETag | tokeniser (`src/rdf_patch.rs:103`), `"<< >>"` collapse (`src/data_models/diff.rs:303`), readers mapping `Term::Triple` to `""` (`src/dataset_versions/registry.rs:24`, `src/commit_log.rs:158`) | §5.1 item 2 and §3.5; the readers never see this graph |
| count index | inflation confined to this graph's own count | none |

The rationale is not query pollution: under the `FROM`-scoped union default graph, an unscoped `SELECT ?s ?p ?o` returns the annotation quads whichever graph they live in, and only `GRAPH`-scoped queries avoid them. What distinguishes the separate graph is everything else in the table: annotations are excluded from reasoning premises by an existing role check, from the write gate (`src/server/routes.rs:1445`) and from version cuts, their count stays out of the data graph's count, every checkpoint is not tripled, and one role filter in the browse paths keeps them out of the UI's graph lists. Rejected: **the data graph** (today's only RDF-star data is a demo insert, `src/saved_queries/seed_data.rs:727-731`), which loses all of those; and **`urn:system:commit-log`**, unregistered and blocked over `/store` (`:2094`), so the query users want would be admin-only.

### 6.4 Cost and opt-in

Encoder-derived, **not measured**: an IRI term is 17 bytes (`oxigraph-0.5.9/src/storage/binary_encoder.rs:628`; `numeric_encoder.rs:16`), a triple term 52 (`binary_encoder.rs:807`), a named-graph quad lands in six column families (`storage/rocksdb.rs:966`): 408 B of raw keys per plain all-IRI quad, 618 B for the `rdf:reifies` quad. Assuming the `ots:op` string and `ots:seq` integer literals take the inline small-string and integer encodings, a minimal event is on the order of 1.6 KB of raw keys and the two per-statement quads on the order of 1.0 KB, before compression and value bytes. The only documented disk figure, ~200–400 MB per million triples (`docs/triplestore-comparison.md:493`), is a comparison-table estimate. Annotating a 900k-quad graph adds ≈5.4M quads under the minimal profile (six per statement) and ≈7.2M under the full profile (eight); hence **opt-in per dataset** (`dataset_settings`, default off), `full`-extent rows only. Annotations are written by a change-log consumer after the data commit, in the same `spawn_blocking`, as a separate store write that is itself recorded and excluded by role from LDES, entailment, gating and version cuts.

### 6.5 The query this buys

```sparql
SELECT ?op ?commit ?actor ?time WHERE {
  GRAPH <urn:ots:statement-provenance:{dataset}> {
    ?r rdf:reifies <<( :subject :predicate "value" )>> ; ots:change ?e .
    ?e ots:op ?op ; prov:wasGeneratedBy ?commit .
    OPTIONAL { ?e prov:wasAssociatedWith ?actor ; prov:atTime ?time }
  }
} ORDER BY DESC(?time)
```

`<< :s :p :o >>` shorthand must **not** be used here: it mints a fresh blank reifier and matches nothing. Today this is answerable only at graph granularity (`src/provenance.rs:12-14`; `docs/datasets.md:76`).

## 7. API surface

All additive.

| Route | Notes |
|---|---|
| `GET /api/datasets/:id/changes?since=&limit=` | JSON rows without blobs plus `next`; rows filtered to `accessible_read_graphs` (`src/server/routes.rs:1003`), since dataset access does not imply private-graph access (`:3277`); unreadable rows are omitted, never counted |
| `PUT /api/datasets/:id/changes/cursors/:name {seq}`; `DELETE …/cursors/:name` | named cursor, pins retention (a PUT, not a side effect of a GET); carries owner and `expires_at` (§2.5); deletable by owner or admin |
| `GET /api/datasets/:id/changes/:seq[/inverse]` | `application/rdf-patch` with the §5.1 headers; 410 when compacted |
| `GET …/versions/:ver/patch` | the chain from `ver:baseVersion`; 409 when broken |
| `GET …/versions/:ver/diff/:other` | unchanged (`src/dataset_versions/routes.rs:29`); additive `source` field |
| `POST …/versions`; `GET …/versions/:ver` | optional `checkpoint`; additive `base_version`, `patch_range`, `content_hash`, `chain_broken`, `materialised`; `snapshot_graphs` and `source_map` populated for deltas as in §3.1 |
| `GET …/versions/:ver/data` and the other materialising reads | unchanged shape; 503 with `Retry-After` for an anonymous request on an unmaterialised delta (§3.3); 507 over the materialisation budget |
| `GET /api/datasets/:id/commits` | unchanged; `CommitRecord` gains optional `seq_from`/`seq_to` and `net_added`/`net_removed`, all `skip_serializing_if = "Option::is_none"` like `actor_iri` (`src/commit_log.rs:93-94`). **Existing commit nodes are not back-filled**: they have no `ver:seqTo`, so the four fields are absent, and `meta.first_seq_commit` records the first commit that has one; consumers order pre-change-log commits by `created` as today and later ones by `seq_to`. `added`/`removed` keep their present per-path meaning (§1.3) unchanged. A `since=` parameter needs `list_dataset_commits`, which lives in `crate::auth::handlers` (`src/server/mod.rs:35`, `:1178-1179`; `src/auth/handlers.rs:4187`), so it waits for approval |
| `GET /api/admin/changes/stats` | change-log size, rows and blob bytes per dataset, pinned ranges and their owners, materialisation quads and disk, checkpoint counts per dataset |

Conventions, from the read-boundary follow-up: read routes join `dataset_version_public_routes` under `optional_auth` (`src/dataset_versions/routes.rs:9-10`), take `Option<Extension<AuthenticatedUser>>` and hide existence with 404 as `execute_dataset_query` does (`src/server/routes.rs:4796`). The patch route is the anti-pattern: mounted under `optional_auth` (`src/server/mod.rs:1161-1163`) yet declaring a bare `Extension<AuthenticatedUser>` (`src/rdf_patch.rs:509`), so an anonymous POST answers 500; it moves to the protected group. Feed routes are GET, because `enforce_write_scope_for_mutation` returns 403 to a read-scoped token on any POST except a literal `/sparql` (`src/auth/middleware.rs:610`). Denials use `AppError::Forbidden`, since `audit_forbidden` records only 403s (`:477`); the rate tier is the `/sparql` group's (`make_rate_conf(1, 40)`, `src/server/mod.rs:811`); no `AuditEventType` covers a read (35 variants, `src/auth/audit.rs:21`) and adding one is a `src/auth/` edit. `docs/api-reference.md` documents none of `/commits`, `/provenance`, `/patch`, `/ldes` or `x-commit-message` (grep: 0 hits); the docs and `src/server/openapi.rs` update covers both.

## 8. Cost model, measurements, phasing, open questions

### 8.1 Numbers available

| Item | Figure | Source |
|---|---|---|
| RocksDB bulk load, 1M / 10M / 100M | 7.1 s / 59 s / 734 s; 0.14–0.17 Mt/s | `docs/performance.md:585-586` |
| PUT replace, 900k quads | 17.7 s / 18.8 s before, 32.3 s / 30.2 s as one transaction; "+70 %" accepted | `docs/performance.md:678`, `:680` |
| SHACL, 0.9M, 6 shapes | 10.8 s pre-rebuild; 0.64–0.93 s HTTP / 0.72 s in-process after | `docs/performance.md:624`, `:657` |
| SHACL, 9M, 6 shapes | 118 s pre-rebuild; **post-rebuild unmeasured** (no SHACL row at `:687-697`) | `:624` |
| mirror rebuild, 1.7M | ~29 s inline, on a laptop | `src/store/parallel_mirror.rs:88-90` |
| write-path budget | no benchmark may slow by more than 20 % | `p2-brief.md` |

Savings are structural, not measured: nine scan sites removed, two O(graph) LDES scans per write removed, O(delta) diffs, no synchronous copy on draft, pipeline and validate-and-commit versions. Added costs inside the write path, all unmeasured: two SQLite statements and the critical section per write (measurement 3); the PUT-replace pre-image on an already-regressed benchmark (measurement 4); N-Quads serialisation and gzip of payloads up to the payload cap (measurement 9); and, outside the write path but hitting every reader, the store-wide mirror and cache invalidation of a read-triggered materialisation (measurement 10).

**RocksDB options.** The brief's Tier A item also asks to "wire the RocksDB options (block cache, bloom filters, compaction)". Not possible through oxigraph 0.5.9's public API: `StoreOptions` has only `with_max_open_files`, `with_unlimited_max_open_files` and `with_fd_reserve` (`oxigraph-0.5.9/src/store.rs:111-146`); cache, filters and compaction are hard-coded in `open_read_write`/`db_options` (`rocksdb_wrapper.rs:138`, `:320`). No bloom-filter API is called anywhere in oxigraph (grep: 0 hits); the only filter is a side effect of `rocksdb_options_optimize_for_point_lookup(options, 128)` (`rocksdb_wrapper.rs:395-396`), applied to column families with `use_iter: false`, which is `id2str` alone (`storage/rocksdb.rs:108`; every other family is `use_iter: true`, `:114-168`). Changing them is a fork, a stop condition; the wiring is recorded as not achievable in scope. Sizing guidance is in §8.6.

### 8.2 Measurements required before implementation

1. **Before-image cost**: `quads_for_pattern(..).collect()` plus set difference at 900k and 9M; sets `OTS_CHANGE_CAPTURE_MAX_SCAN`.
2. **Probe cost**: a 1M-quad payload through `insert_quads_and_reindex_into` (probes, `src/store/engine.rs:1292`) versus `bulk_insert_quads` (none, `:1631`).
3. **Intent-row and critical-section overhead** at the mixed phase (`docs/performance.md:655`), including the `on_store` → `on_transaction` switch of §4.3 and the async-thread registry rows; per-test overhead of the in-memory change log.
4. **PUT-replace pre-image**: the `:678` benchmark with a `Transaction::quads_for_pattern` read before `tx.clear_graph`; the 20 % budget decides `full` row versus `counts` row.
5. **Transaction RSS** at 50 000 and 200 000 staged quads, setting `OTS_APPLY_DELTA_TX_MAX_QUADS`; **restore and materialisation**: copy+`MOVE` versus tier-1 inverse patch at 0.9M and 9M, base copy plus chain apply at 0.9M with 1k, 10k and 100k changed quads, both directions.
6. **Annotation storage** (`du` on two data directories) and the **blank-node census** over the seed bundles.
7. **Crash injection**: `kill -9` between the pending row and the WriteBatch commit, and between the row and an SST ingest.
8. **Tests**: named reifier end to end, idempotent re-insert, `DELETE DATA` of it, Turtle 1.2 `{| |}` through import, blank-node `Transaction::remove` after re-parse, concurrent double `create_version`.
9. **Payload serialisation**: N-Quads plus gzip of 50k, 250k and 900k quads inside `graph_store_put` and of a 9M import inside `bulk_insert_quads`, against the 20 % budget; sets `OTS_CHANGE_CAPTURE_MAX_PAYLOAD`, and `du` of `changes.db` after the seed bundles and after 24 hours of the mixed benchmark (feeds §8.6).
10. **Read-triggered materialisation**: query p95 across the store before, during and after a cold read of a 0.9M-quad delta version at 1.7M and 9M store sizes, with the mirror inline and background; decides whether rule (iv) of §3.3 is needed.
11. **LDES blank-node owner lookup**: capture cost per row on the IFC seed bundle versus the two graph scans it replaces; sets `OTS_LDES_BNODE_DEPTH` and `OTS_LDES_BNODE_SUBJECTS_MAX`.

### 8.3 Relationship to SHACL→SQL: deferred pending measurement

The maintainer's steer replaces the author's draft position ("not a good fit"): **the change log is built first; SHACL→SQL is deferred until one experiment has run; bulk revalidation after a shape or model update is a real, recurring workload.**

*The workload.* Beyond the per-write gate, two whole-dataset patterns exist. (i) After instance changes a pipeline re-runs over everything, because there is no incremental validation: `check_write_gates` and `validate_on_write` validate the entire future graph (`src/shacl_studio/gate.rs:111`; `src/server/routes.rs:1419`), `execute_pipeline` has only the manual handler and the 60 s scheduler as callers (`src/shacl_studio/scheduler.rs:17-19`), and `trigger_on_write` is persisted and round-tripped (`src/shacl_studio/models.rs:282`; `src/shacl_studio/store.rs:104`; echoed at `handlers.rs:1083`, `:1196`, `:1290`) but never consulted to start a run. The change log bounds (i): only the focus nodes touched since the pipeline's last `seq` need re-checking, and the Studio trigger becomes a feed consumer. (ii) After a **shape or model update**, every target of every affected shape must be revalidated regardless of which instances changed; the change log cannot shrink that, and it is exactly the "many targets, bulk, Core fragment" case in which a SQL compilation could pay. That instance data is routinely incorrect after a model update, so that (ii) recurs with every model version, is the maintainer's assertion; nothing in code, docs or telemetry establishes the frequency, which is why the analytical-mirror telemetry below is part of the decision.

*The gating experiment.* Harness: `examples/scale_otl.rs` (usage `scale_otl <assets> <data_dir> [writers] [readers] [seconds]`, `:17`; 1M assets ≈ 9M quads, six property shapes at `:69`, in-process `shacl::engine::validate` at `:194`), plus the same store over HTTP through a Studio pipeline (`POST /api/shacl/pipelines/:id/run`, `spawn_blocking` at `src/shacl_studio/handlers.rs:1341`), the path users take. Two configurations: (A) the mirror source, `OTS_PARALLEL_QUERY_MAX_TRIPLES` raised above 9M (default cap 2M, `src/store/parallel_mirror.rs:56`; the RAM-derived cap admits 9M only at roughly 36.9 GB); (B) the RocksDB snapshot source with the run index at its defaults (`OTS_SHACL_RUN_INDEX_MAX_QUADS` = memory/8/300 clamped 250k..8M, `OTS_SHACL_RUN_INDEX_MIN_PROBES` 20 000, `src/shacl/view.rs:141-153`) and disabled. Record per run: wall time, source taken, peak RSS, indexed versus dropped (predicate, direction, graph) pairs, `duration_ms` and `results_count` from `pipeline_runs` (`src/auth/db.rs:949-962`). Then the **gate-sandbox cost**: one Graph Store POST of 1 000 quads into the 9M graph with a gating pipeline, which dumps and re-parses the whole graph as Turtle before validating (`seed_existing_graph`, `src/shacl_studio/gate.rs:55`). Finally the **shape-update revalidation**, workload (ii): edit one property shape and re-run.

*Decision thresholds*, proposed for the maintainer to confirm. Linear scaling from 0.72 s in-process predicts about 7 s at 9M on the mirror (`docs/performance.md:767-768` claims linearity). SHACL→SQL is **taken up** if configuration (B), the default at 9M, exceeds 60 s on the whole-dataset run *and* (A) cannot be reached within the deployment's RAM budget, or if the shape-update revalidation exceeds 60 s on whichever source is in use. It is **closed** if (B) comes in under 20 s: a second validator whose semantics must match the engine byte-for-byte is then a liability without a payoff. Between 20 and 60 s the decision waits for the analytical-mirror telemetry to show how often (ii) runs. The gate-sandbox number is reported separately: a compilation does not touch that path, and if the sandbox dominates the fix is the change log (validate the merged delta's focus nodes), not SQL.

### 8.4 Phasing and approval

| Phase | Content | Effort |
|---|---|---|
| 1 | `changes.db`, intent rows with `row`/`seq` split, epochs, capture in the five free and three probe primitives, `counts` rows above the payload cap and for snapshot targets, `unknown` and store-scoped rows elsewhere, nested-guard suppression, `WriteContext` inside blocking closures, `on_store` → `on_transaction` on the three SPARQL primitives, the `seq` critical section, boot reconciliation, off switch, exact `adjust` everywhere with a `scan_count()` test, `ver:seqTo` and `net_added`/`net_removed` | 2.5 weeks |
| 2 | `/changes` feed, cursors with owner and TTL, per-change patch and inverse, admin stats, patch extensions, `apply_delta` behind the patch endpoint, text-index and entailment fed from rows | 1.5 weeks |
| 3 | Delta versions: predicates and `ver:graphMap`, resolver with the anonymous-caller rule, per-version build mutex, admission control, sweep, chain diffs, content hash, checkpoint policy, per-dataset lock, GC | 2 weeks |
| 4 | Budgeted before-image capture for the five scan primitives (gated on measurements 1 and 4), two-tier inverse restore, LDES from rows, Studio incremental trigger | 1.5 weeks |
| 5 | Skolem opt-in; provenance opt-in with the role fixes (entailment, gate, LDES, browse, version selection) and the JSON-LD 406; the §8.3 experiment | 1.5 weeks |

About 9 weeks against the painpoints document's 4-week Tier A estimate, which assumed the commit log could be the feed (§1.3 shows it cannot); phases 1–2 stand on their own.

**Approval items, to be answered before phase 1 starts.**

1. **Scope beyond the brief.** The brief lists `src/store/`, `src/server/{routes,openapi,error}.rs`, `src/entailment.rs`, `src/ldes/`, `src/shacl_studio/gate.rs` and `docs/`, all used. This design also needs `src/dataset_versions/` (phases 3–4), `src/commit_log.rs` (phase 1: `ver:seqTo`, the four optional fields), `src/rdf_patch.rs` (phase 2), `src/provenance.rs` (phase 5) and route mounting in `src/server/mod.rs` (phase 2), which the brief neither lists nor excludes. Each is named against its phase so that approval can be partial.
2. **Out of scope by design**: `src/auth/` (hence the separate SQLite file, `dataset_settings`, no `since=` on `/commits`, no read audit event, no dataset-delete wiring) and `src/backup/` (hence copy-on-publish as the backup rule).
3. **The import-path limitation** of §1.1 and §3.2 is accepted as is, or Q10 is answered.

### 8.5 Open questions

1. **Backup completeness.** Unmaterialised deltas are not in a backup; is copy-on-publish enough, or should `changes.db` join the payload (a `src/backup/` change)?
2. **The streaming load fast path.** Unhooked, it yields `unknown` rows on every boot seed and recovery; acceptable, or route small payloads through `parse_quads` (`src/store/engine.rs:1260`)?
3. **Commit-log atomicity.** With the three SPARQL primitives on `on_transaction` (§4.3), the commit triples could join the data transaction on the transactional paths; worth doing once the change log is authoritative?
4. **Net-delta accounting** — *decided*: `added`/`removed` keep their per-path meaning; `net_added`/`net_removed` are added as optional fields (§7). Kept here only to record the decision.
5. **Shared graphs.** Keep the phase-3 exclusion, or lock all owners in sorted order later (the sorted-order rule already applies to the §4.3 critical section)?
6. **Materialisation budget.** Is the §8.6 default for `OTS_MATERIALISATION_MAX_QUADS` right, and is 507 right when exceeded?
7. **Commit messages.** Only `/sparql` reads `x-commit-message` (`src/server/routes.rs:186`); should every path carry one?
8. **Reasoner provenance.** Attribute derived-triple events to the regime IRI, the triggering commit, or both?
9. **SHACL thresholds.** Are 20 s and 60 s at 9M right, and whose RAM budget defines "cannot reach configuration (A)"?
10. **Import archives.** Should a replace-import archive whose pre-image was captured exactly (below the scan cap) be cut as a Published *delta* on the previous archive, relaxing "publish ⇒ physical" for that one producer? It removes the copy for small graphs at the price of Q1's backup gap for those archives; above the cap nothing changes either way.

### 8.6 Memory and disk sizing

Guidance, not measurement; every figure here is derived and item 9 replaces it.

- **`changes.db` growth.** Per row: fixed columns of a few hundred bytes plus, for `full` rows, the gzipped N-Quads of the delta. N-Quads text of an all-IRI quad is a few hundred bytes uncompressed; the compressed size per quad for the seed bundles is unmeasured. Upper bound per row: `OTS_CHANGE_CAPTURE_MAX_PAYLOAD` quads (proposed 250 000), above which no blob is stored. Upper bound per deployment: rows within `OTS_CHANGE_RETENTION_DAYS` (proposed 90) below the lowest pin, plus everything pinned; the admin stats route (§7) reports both. Rows for snapshot, staging and Entailment-role graphs never carry payloads and are pruned after 24 hours.
- **Materialisations.** Each live materialisation is a full copy of its covered graphs in RocksDB, at the only documented rate, ~200–400 MB per million triples (`docs/triplestore-comparison.md:493`, a comparison-table estimate). Proposed default `OTS_MATERIALISATION_MAX_QUADS` = the mirror's default cap, 2M (`DEFAULT_MAX_TRIPLES`, `src/store/parallel_mirror.rs:56`), on the ground that a materialised version larger than the mirror cannot be served from the mirror in any case; the sum of live materialisations is bounded by that cap times the number of concurrently used delta versions, released after `OTS_MATERIALISATION_TTL_SECS` (proposed 3600).
- **Checkpoints versus retention.** A checkpoint costs one full copy at the rate above and is retained until GC (§3.7); a delta costs its rows until the chain is compacted (§2.5). The checkpoint policy (§3.2: every 100 deltas or 25 % of the base) therefore trades one copy against at most a quarter of the base's size in rows; both figures are unmeasured and are the knobs to turn if disk is the constraint.
- **Memory.** The intent row and finalisation add no per-quad memory beyond the payload the primitive already holds; `apply_delta` stages at most `OTS_APPLY_DELTA_TX_MAX_QUADS` in one transaction (measurement 5 gives RSS per staged quad); the parallel mirror's RAM-derived cap is unchanged by this design.
