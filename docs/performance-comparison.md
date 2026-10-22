# Performance comparison with other open-source triplestores

Open Triplestore measured against five open-source triplestores on the same
machine, under the same container limits, with the same data and the same
query instances: Oxigraph server (the engine this store embeds, upstream and
unpatched), Apache Jena Fuseki, QLever, Virtuoso Open Source and Eclipse RDF4J.

> **Measured 2026-10-03** on one Apple M1 Pro laptop (16 GB) in Docker, against
> Open Triplestore 0.7.0 at commit `570a7a30` (develop). Two BSBM scales: 1,000
> products (0.37 M triples) and 10,000 products (3.56 M triples). Every number
> below comes from [`docs/perf-data/2026-10-03/`](perf-data/2026-10-03/), produced
> by the harness in [`scripts/bench-compare/`](../scripts/bench-compare/). Read
> the [threats to validity](#7-threats-to-validity) before quoting a number.
>
> **Open Triplestore re-measured 2026-10-13** after the round-3 performance
> fixes ([§5.8](#58-open-triplestore-after-the-round-3-fixes-2026-10-13)): the
> 2026-10-03 commit and the fixed branch head, built the same way and run back to
> back, with upstream Oxigraph re-run in the same window to cross-check every
> answer. The other stores were **not** re-run; every number of theirs in this
> document is from 2026-10-03. Sections 5.1–5.7 are the 2026-10-03 campaign.

## Contents

1. [Summary](#1-summary)
2. [Stores](#2-stores)
3. [Method](#3-method)
4. [Hardware and software](#4-hardware-and-software)
5. [Results](#5-results)
6. [Where Open Triplestore loses](#6-where-open-triplestore-loses)
7. [Threats to validity](#7-threats-to-validity)
8. [What was not measured](#8-what-was-not-measured)
9. [Reproduce it](#9-reproduce-it)

## 1. Summary

- **After the round-3 fixes (2026-10-13, [§5.8](#58-open-triplestore-after-the-round-3-fixes-2026-10-13)).**
  Measured against the 2026-10-03 commit in the same window: the BSBM explore
  mix with one client runs at 16,570 query mixes per hour at 10k (5,358
  before) and 64,685 at 1k (24,923 before). Upstream Oxigraph, re-run in the
  same window, did 33,719 and 117,023, so Open Triplestore is now about 2×
  slower than its own engine at 10k, not 6×; DESCRIBE (Q9) went from 42.6 ms to
  1.0 ms. The correlated `NOT EXISTS` (F10) takes 366 ms at 10k (a timeout
  before) and 42 ms at 1k (5.0 s). The 10k load into 4 GiB peaked at 1.5–1.9 GiB
  in eight loads, none OOM-killed (3.8 GiB before); whole-dataset SHACL
  validation at 10k completed in 15.7 s, where the old build's container died
  in the same run. Every answer matched upstream Oxigraph's, instance by
  instance. The bullets below are the 2026-10-03 campaign.
- **No store is fastest everywhere.** On the BSBM explore mix with one client,
  upstream Oxigraph leads at 1k products (136,476 query mixes per hour) and
  Virtuoso at 10k (49,962). Open Triplestore is third at 1k (50,897) and third
  at 10k (7,229), ahead of Fuseki, RDF4J and QLever.
- **Against its own engine, Open Triplestore loses.** It answers the explore
  mix 2.7× slower than upstream Oxigraph 0.5.11 at 1k and 6× slower at 10k,
  with the same answers. At 10k its in-memory accelerator is off (the store is
  over its 2 M-triple cap in a 4 GiB container), and DESCRIBE takes 43 ms
  against Oxigraph's 0.24 ms. A follow-up probe points at this store's own
  layer rather than the evaluator; it is tracked in [#468](https://github.com/philipperenzen/open-triplestore/issues/468).
- **Where it leads:** at 1k, with the accelerator on, it has the lowest median
  latency of the six on BSBM Q2, Q4 and Q5 and on the property-path queries F01 and F02,
  and whole-dataset SHACL validation takes 0.33 s against Fuseki's 0.98 s.
- **Loading is its slowest workload:** 44 k triples/s over the Graph Store
  protocol at both scales, where the other stores loaded the 10k file at
  101–476 k triples/s (it indexes every literal for full-text search as it
  loads). In the 4 GiB container the 10k load was **OOM-killed in 2 of 6 attempts**
  ([#469](https://github.com/philipperenzen/open-triplestore/issues/469)); SHACL
  validation at 10k peaked at 4,071 of 4,096 MiB.
- **Correlated `FILTER NOT EXISTS` is a cliff** for Open Triplestore, Oxigraph
  and RDF4J: 4.4 s, 112 s and 89 s at 1k, a timeout at 10k, against 23–170 ms
  for Virtuoso, QLever and Fuseki ([#471](https://github.com/philipperenzen/open-triplestore/issues/471)).
- **Answers were checked, not assumed.** Every query instance's result count is
  compared across stores. All stores agree on every SELECT and CONSTRUCT
  instance except Virtuoso, which returned a short answer with HTTP 200 for
  24 query executions at each scale, out of about 190,000–220,000 run under 4
  and 8 concurrent clients. DESCRIBE answers differ by design (RDF4J and Virtuoso include
  incoming triples). The harness also found a correctness bug in this store's
  `/sparql` for admins who write `FROM` ([#470](https://github.com/philipperenzen/open-triplestore/issues/470)).

![BSBM explore, one client, 1k products: query mixes per hour](perf-data/2026-10-03/explore-qmph-1k.svg)

![BSBM explore, one client, 10k products: query mixes per hour](perf-data/2026-10-03/explore-qmph-10k.svg)

## 2. Stores

Only open-source stores whose licences put no condition on publishing
benchmark results. Each licence text was read before the store was included.

| Store | Version | Licence | Storage and configuration |
|---|---|---|---|
| **Open Triplestore** | 0.7.0, `570a7a30` | this repository | RocksDB (Oxigraph 0.5.11) + in-memory accelerator; release build, `--features full` |
| **Oxigraph server** | 0.5.11 | MIT OR Apache-2.0 | RocksDB; `oxigraph load` then `oxigraph optimize` |
| **Apache Jena Fuseki** | 6.2.0 (main) | Apache-2.0 | TDB2, `-Xmx3g`; query, update, Graph Store and SHACL services |
| **QLever** | commit `caba7eb` | Apache-2.0 | its own index; 3 GB query memory, result cache off, 4 parallel queries |
| **Virtuoso Open Source** | 7.2.17 | GPL-2.0 | buffers sized per OpenLink's guidance for 4 GB (340,000 / 250,000) |
| **Eclipse RDF4J server** | 6.1.0 | EDL-1.0 (BSD-3-Clause) | Native Store, default indexes `spoc,posc`, `-Xmx3g` |

**Not included.** GraphDB, Stardog, AllegroGraph and MarkLogic are proprietary
products whose licence or evaluation terms limit publishing benchmark results or
using the software for competitive evaluation — Stardog's licence, for one, rules
out evaluating the software "for the purpose of competing with Stardog" — and no
vendor permission was sought. Amazon Neptune is a managed service and cannot run
on the same machine under the same limits. The comparison matrix in
[triplestore-comparison.md](triplestore-comparison.md) still lists their
documented features.

## 3. Method

### Workloads

| | Workload | What runs | What is reported |
|---|---|---|---|
| a | **BSBM explore** | The 25-query explore mix of BSBM V3 (Q1–Q5, Q7–Q12), one client: 20 warm-up mixes, then 100 measured mixes | median and p95 latency per query, query mixes per hour (QMpH) |
| b | **Load** | The same N-Triples file into an empty store, with each store's documented bulk path; for Oxigraph and Fuseki also over HTTP | wall time, triples/s, peak memory, store size |
| c | **SPARQL 1.1 feature mix** | 14 fixed queries: property paths (`*`, `+`, sequence, inverse, alternative), aggregates (`GROUP BY`, `HAVING`, `COUNT DISTINCT`, `AVG`), `OPTIONAL`/`!BOUND`, `MINUS`, `NOT EXISTS`, subqueries, `VALUES`/`UNION`, `COUNT(*)` | median of 10 runs after 2 warm-up runs |
| d | **SHACL validation** | 4 node shapes, 22 property constraints over all products, offers, reviews and reviewers; stores with on-demand validation over HTTP | median and p95 of 5 runs after 1 warm-up, number of results |
| e | **Concurrent clients** | 1, 4 and 8 client processes each running explore mixes for 60 s | QMpH, p95 latency, failed queries |

The queries are in [`scripts/bench-compare/queries/features/`](../scripts/bench-compare/queries/features/),
the shapes in [`shapes/bsbm-shapes.ttl`](../scripts/bench-compare/shapes/bsbm-shapes.ttl).

**Data.** The [Berlin SPARQL Benchmark](http://wbsg.informatik.uni-mannheim.de/bizer/berlinsparqlbenchmark/)
generator (BSBM tools 0.2, GPL-3.0, fetched at run time and never vendored)
with forward chaining (`-fc`): 1,000 products → 374,911 triples (96 MB), and
10,000 products → 3,564,773 triples (919 MB). The generator is deterministic;
the SHA-256 of both files is in [`inputs.json`](perf-data/2026-10-03/inputs.json).

**Query instances.** BSBM's test driver is not used. The harness fills the
BSBM explore templates itself (`bench.py params`, seed `20261003`): products,
offers and reviews are drawn uniformly, a product type and features are taken
from one random product so Q1/Q3/Q4 usually match something, numeric bounds are
uniform in 1–500, and the current date is 2008-06-20. Warm-up, measured and
concurrent mixes use disjoint draws. Every store gets the identical query text
in the identical order. The QMpH numbers are therefore comparable *between the
stores in this document*, but not with published BSBM results.

**Scales.** 1k and 10k products, not the 100k and 1M a server would get: 1M
products is about 350 M triples, which neither fits a 16 GB laptop next to other
work nor the disk that was free; 100k (35 M triples, 9 GB of N-Triples) did not
fit the disk either. At 10k the data is still smaller than the 4 GiB memory
limit for most stores' indexes, so both scales measure the in-memory regime.

### Fairness rules

- **Same limits.** Every store container gets `mem_limit` and `memswap_limit`
  of 4 GiB and 4 CPUs. JVM heaps are 3 GiB; QLever's query memory is 3 GB;
  Virtuoso's buffers follow OpenLink's 4 GB sizing.
- **One store at a time.** Each store is started, loaded, measured and removed
  with its volume before the next starts. No two stores ever ran together.
- **Same data, same queries, same order, same counts.** The same N-Triples
  file, the same query instances, 20 warm-up mixes and 100 measured mixes, 2
  warm-up and 10 measured runs per feature query, 1 warm-up and 5 measured
  SHACL runs, 60 s per concurrency level.
- **Same settle time.** 60 s between the end of the load and the first query,
  for every store.
- **No result caches.** Open Triplestore runs with `OTS_QUERY_CACHE_ENABLED=off` and
  QLever with `-c 0GB`; the others have no query-result cache. Open
  Triplestore's per-IP rate limiter is off, its demo seed and vocabulary
  download are off. Nothing else is tuned.
- **The product's own path.** Open Triplestore is queried through a dataset's
  SPARQL service endpoint, the path a client uses; it pays for its
  authentication, scoping and HTTP stack like every other server pays for its
  own.
- **One query timeout.** The harness waits up to 300 s per request. Open
  Triplestore enforces its own 30 s server limit, so its timeouts happen at
  30 s.
- **Slow-query rule.** A feature query whose warm-up run takes more than 30 s
  gets one measured run (shown as `n=1`); a query that fails or times out is
  reported as such and not retried.
- **A separate client.** The load generator (`bench.py client`, Python
  standard library, one persistent HTTP connection per client process) runs in
  its own container with 2 CPUs on the store's Docker network.

### Quiet window

The laptop is shared with other build jobs. Before each store, the orchestrator
waited until no `rustc` or `cargo` process ran, the 1-minute load average was
under 6, and at least 15 GiB of disk was free, polling every 3 minutes. Runs at
10k were allowed only in such a window. The load average at the start of each
run is in the run files ([`runs/`](perf-data/2026-10-03/runs/)): 1.9–5.3 for
every run used here. The first Open Triplestore run at 1k had no quiet window
and was re-run; the re-run is reported.

### What counts as an answer

Every response is parsed strictly — SPARQL JSON for SELECT, N-Triples for
CONSTRUCT and DESCRIBE — and its solutions or triples are counted. A body that
does not parse is an error, not a count. The report then compares the counts
per query instance across stores:

- a store that gave **two different counts for the same query instance** is
  flagged `inconsistent`;
- otherwise its count is compared with the count a strict majority of the
  stores gave (at least three answering), and a store that differs is flagged
  `differs`;
- DESCRIBE is implementation-defined (SPARQL 1.1 §16.4), so its differences are
  listed as `describe-differs` and not treated as wrong.

Latency statistics include only answers with HTTP 200 and a count. Flagged
answers are listed in [§5.6](#56-result-count-cross-check); none is silently
dropped from the totals.

### Memory

Peak memory is the highest `docker stats` reading for the store's container
during each phase, sampled about once a second: the cgroup's usage minus
inactive file cache. Stores that read through the operating system's page cache
or memory maps (Oxigraph's RocksDB, QLever) look small by this measure; stores
with their own buffer pools (JVM heaps, Virtuoso) look large. Treat it as "how
close to the limit did the container get", not as a working-set size.
Since 2026-10-15 the harness also samples the container's cgroup
`memory.stat` twice a second and records each phase's peak anonymous memory
and file cache next to it (§5.9), which tells a process's own memory from page
cache the kernel would reclaim.

## 4. Hardware and software

| | |
|---|---|
| Machine | Apple M1 Pro (8 performance + 2 efficiency cores), 16 GB RAM, internal SSD |
| OS | macOS 26.6.2 |
| Container runtime | Docker 29.6.2 (Docker Desktop, Linux VM: 10 CPUs, 15.6 GiB, kernel 6.12.76-linuxkit), `linux/arm64` images |
| Store containers | 4 CPUs, 4 GiB memory and swap limit each |
| Load generator | `python:3.12-slim` container, 2 CPUs |
| Open Triplestore build | `cargo build --release --features full` (fat LTO, one codegen unit) in `rust:1.94-bookworm`, [`Dockerfile.ots`](../scripts/bench-compare/Dockerfile.ots) |

Image digests, the Fuseki jar checksum, the dataset checksums and the exact
commands are in [`inputs.json`](perf-data/2026-10-03/inputs.json) and per run
in [`runs/`](perf-data/2026-10-03/runs/).

## 5. Results

### 5.1 Load

| Store | Scale | Load path | Load time (s) | Triples/s | Peak memory (MiB) | Store size (MiB) | HTTP POST load (s) |
| --- | --- | --- | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | HTTP Graph Store POST into a dataset graph, 100 MB N-Triples chunks | 8.5 | 43,865 | 776 | 156 | — |
| Oxigraph | 1k | `oxigraph load` (offline bulk loader) + `oxigraph optimize` | 3.1 | 122,680 | 275 | 69 | 3.6 |
| Fuseki | 1k | `tdb2.tdbloader` (offline, default loader) | 5.5 | 68,166 | 1,006 | 74 | 3.3 |
| QLever | 1k | `qlever-index` (offline index build) | 1.1 | 331,486 | — | 15 | — |
| Virtuoso | 1k | `ld_dir` + `rdf_loader_run()` + `checkpoint` (bulk loader) | 1.8 | 205,206 | 474 | 260 | — |
| RDF4J | 1k | HTTP `POST /statements`, 100 MB N-Triples chunks | 3.6 | 103,825 | 562 | 88 | — |
| Open Triplestore | 10k | HTTP Graph Store POST into a dataset graph, 100 MB N-Triples chunks | 82.1 | 43,442 | 3,019 | 1,739 | — |
| Oxigraph | 10k | `oxigraph load` (offline bulk loader) + `oxigraph optimize` | 14.2 | 250,970 | 1,832 | 636 | 42.4 |
| Fuseki | 10k | `tdb2.tdbloader` (offline, default loader) | 19.8 | 180,203 | 2,930 | 704 | 26.7 |
| QLever | 10k | `qlever-index` (offline index build) | 7.5 | 475,938 | 628 | 140 | — |
| Virtuoso | 10k | `ld_dir` + `rdf_loader_run()` + `checkpoint` (bulk loader) | 14.8 | 240,749 | 2,926 | 510 | — |
| RDF4J | 10k | HTTP `POST /statements`, 100 MB N-Triples chunks | 35.3 | 100,871 | 1,385 | 626 | — |

Open Triplestore (into a dataset graph) and the RDF4J server have no offline
loader, so they load over HTTP; the others use their bulk loaders, which build the index without serving queries.
The last column puts Oxigraph and Fuseki on the same HTTP path (the same 100 MB
chunks, POST into an empty store): 42 s and 27 s at 10k, against Open
Triplestore's 82 s. Open Triplestore's load also indexes every literal for
full-text search and keeps a per-graph count index; its store size (1.7 GB at 10k) includes the text index.

At 10k the Open Triplestore load was **killed by the container's OOM killer in
2 of 6 attempts** ([attempt log](perf-data/2026-10-03/ots-10k-load-attempts.json));
the one whose chunk log was kept died on chunk 7 of 9, after about 2.3 M
triples, just past the point where the in-memory accelerator turns itself off
at its 2 M-triple cap. The table shows the successful re-run. See [§6](#6-where-open-triplestore-loses).

### 5.2 BSBM explore, one client

**1k products (374,911 triples)**

| Query | Open Triplestore | Oxigraph | Fuseki | QLever | Virtuoso | RDF4J |
| --- | --: | --: | --: | --: | --: | --: |
| Q1 | 0.50 / 0.89 | 0.41 / 1.22 | 1.32 / 1.75 | 56.8 / 59.5 | 1.46 / 1.72 | 1.07 / 2.27 |
| Q2 | 0.88 / 1.35 | 0.95 / 1.21 | 3.08 / 3.85 | 24.8 / 27.8 | 3.50 / 4.19 | 1.35 / 1.91 |
| Q3 | 0.58 / 0.94 | 0.48 / 1.30 | 1.47 / 1.82 | 67.0 / 73.4 | 1.72 / 2.00 | 1.04 / 1.57 |
| Q4 | 0.59 / 0.89 | 0.69 / 2.18 | 1.61 / 1.97 | 89.9 / 97.0 | 4.09 / 4.75 | 1.14 / 1.71 |
| Q5 | 1.51 / 2.70 | 3.61 / 7.43 | 74.6 / 101 | 73.2 / 80.2 | 1.94 / 2.19 | 3.56 / 5.85 |
| Q7 | 2.84 / 3.67 | 1.00 / 1.48 | 2.06 / 2.81 | 82.8 / 89.4 | 3.23 / 3.94 | 1.51 / 2.20 |
| Q8 | 4.93 / 6.65 | 0.71 / 1.09 | 1.99 / 2.87 | 44.8 / 76.8 | 2.35 / 2.80 | 1.56 / 2.46 |
| Q9 | 0.52 / 45.3 | 0.23 / 0.30 | 1.29 / 1.73 | 49.5 / 51.8 | 0.88 / 1.08 | 19.6 / 22.7 |
| Q10 | 1.02 / 1.94 | 0.62 / 0.98 | 1.56 / 1.99 | 156 / 164 | 2.93 / 3.34 | 1.60 / 3.03 |
| Q11 | 0.48 / 0.84 | 0.29 / 0.35 | 1.16 / 1.46 | 4.06 / 4.53 | 0.83 / 0.99 | 20.4 / 23.2 |
| Q12 | 0.61 / 1.02 | 0.37 / 0.48 | 1.68 / 2.09 | 12.9 / 13.2 | 2.43 / 2.57 | 1.60 / 2.73 |
| **QMpH** | **50,897** | **136,476** | **17,772** | **2,467** | **58,386** | **26,292** |

Cells: median / p95 latency in ms (100 measured mixes). QMpH: explore query mixes per hour.

**10k products (3,564,773 triples)**

| Query | Open Triplestore | Oxigraph | Fuseki | QLever | Virtuoso | RDF4J |
| --- | --: | --: | --: | --: | --: | --: |
| Q1 | 44.2 / 67.8 | 1.01 / 9.92 | 1.64 / 2.48 | 57.7 / 61.0 | 1.59 / 1.94 | 1.43 / 2.85 |
| Q2 | 2.94 / 5.68 | 1.08 / 1.44 | 3.36 / 4.46 | 24.2 / 27.6 | 3.72 / 4.37 | 1.46 / 2.06 |
| Q3 | 15.3 / 52.1 | 1.85 / 13.1 | 1.72 / 5.67 | 68.6 / 76.2 | 1.89 / 2.51 | 1.44 / 6.31 |
| Q4 | 9.91 / 51.1 | 2.13 / 14.6 | 2.01 / 3.53 | 91.0 / 99.1 | 4.35 / 5.21 | 1.61 / 2.79 |
| Q5 | 69.9 / 123 | 25.8 / 45.5 | 302 / 406 | 87.1 / 94.8 | 2.52 / 3.13 | 21.5 / 41.5 |
| Q7 | 2.95 / 41.8 | 1.40 / 2.54 | 2.86 / 5.19 | 95.6 / 108 | 3.85 / 5.89 | 2.17 / 3.60 |
| Q8 | 2.16 / 4.50 | 0.87 / 1.48 | 2.37 / 3.71 | 40.4 / 47.2 | 2.75 / 3.24 | 1.89 / 2.80 |
| Q9 | 42.8 / 50.5 | 0.24 / 0.29 | 1.42 / 1.96 | 45.1 / 50.7 | 0.92 / 1.08 | 181 / 193 |
| Q10 | 7.95 / 50.8 | 0.72 / 1.15 | 1.75 / 2.46 | 175 / 185 | 2.67 / 3.07 | 1.98 / 2.98 |
| Q11 | 1.86 / 4.56 | 0.29 / 0.33 | 1.31 / 1.74 | 4.11 / 4.60 | 0.88 / 1.01 | 179 / 186 |
| Q12 | 2.46 / 3.86 | 0.40 / 0.45 | 1.84 / 2.41 | 13.7 / 14.3 | 2.33 / 2.47 | 1.78 / 2.32 |
| **QMpH** | **7,229** | **43,681** | **5,359** | **2,339** | **49,962** | **3,627** |

Cells: median / p95 latency in ms (100 measured mixes). QMpH: explore query mixes per hour.

The second single-client measurement, the first level of the concurrency run
(§5.4), agrees within 20 % for every store but one: Open Triplestore reached 95,580 QMpH
there at 1k against 50,897 here. The difference is not explained; the
explore phase runs first after the settle, and its p95 for DESCRIBE (Q9, 45 ms
against a 0.5 ms median) suggests intermittent stalls in that phase.

QLever spends 25–175 ms on most explore queries at both scales (4–14 ms on the
single-resource Q11 and Q12); its server log showed 63 ms of query planning for
one Q7 instance. That suits large analytical queries (§5.3) more than BSBM's
many small lookups. RDF4J's DESCRIBE (Q9) and Q11 grow with the data (180 ms
at 10k), likely because its default indexes (`spoc,posc`) have no object-first
order for patterns such as `?s ?p <offer>`.

### 5.3 SPARQL 1.1 feature mix

All cells are medians in milliseconds of 10 runs; `n=1` marks a query that got one
measured run under the slow-query rule, `timeout` one that did not finish.

**1k products**

| Query | Open Triplestore | Oxigraph | Fuseki | QLever | Virtuoso | RDF4J |
| --- | --: | --: | --: | --: | --: | --: |
| F01-path-closure | 2.37 | 3.93 | 508 | 4.79 | 4.87 | 4.95 |
| F02-path-sequence-inverse | 0.66 | 1.36 | 2.50 | 6.92 | 1.03 | 1.45 |
| F03-path-alternative | 81.4 | 161 | 86.4 | 49.0 | 3.67 | 78.5 |
| F04-path-oneormore-join | 2.21 | 3.28 | 5.14 | 5.68 | 1.08 | 3.45 |
| F05-agg-group-avg | 280 | 360 | 85.5 | 27.9 | 25.6 | 67.1 |
| F06-agg-having | 6.13 | 10.2 | 14.3 | 4.33 | 3.55 | 13.1 |
| F07-agg-count-distinct | 125 | 23.5 | 16.6 | 8.38 | 14.7 | 19.5 |
| F08-optional-unbound | 2.43 | 15.8 | 4.89 | 4.51 | 1.65 | 10.8 |
| F09-minus | 4.36 | 33.2 | 13.6 | 9.34 | 2.99 | 35.3 |
| F10-not-exists | 4373 | 111.6 s (n=1) | 170 | 57.3 | 22.8 | 88.5 s (n=1) |
| F11-subquery-topk | 11.6 | 58.3 | 46.4 | 10.9 | 1.52 | 21.4 |
| F12-subquery-agg-join | 303 | 180 | 150 | 56.5 | 33.5 | 182 |
| F13-values-union | 7.67 | 0.85 | 2.73 | 9.42 | 1.96 | 60.6 |
| F14-count-all | 40.5 | 136 | 71.5 | 46.3 | 2.59 | 191 |

**10k products**

| Query | Open Triplestore | Oxigraph | Fuseki | QLever | Virtuoso | RDF4J |
| --- | --: | --: | --: | --: | --: | --: |
| F01-path-closure | 14.2 | 8.39 | 7620 | 8.15 | 10.7 | 13.8 |
| F02-path-sequence-inverse | 2.12 | 1.19 | 6.52 | 6.54 | 0.87 | 1.68 |
| F03-path-alternative | 2621 | 1326 | 1044 | 45.8 | 18.4 | 797 |
| F04-path-oneormore-join | 16.6 | 11.7 | 24.7 | 10.9 | 6.41 | 25.3 |
| F05-agg-group-avg | 3708 | 2175 | 920 | 203 | 91.3 | 700 |
| F06-agg-having | 64.7 | 55.3 | 189 | 16.4 | 35.4 | 141 |
| F07-agg-count-distinct | 244 | 189 | 204 | 24.1 | 196 | 181 |
| F08-optional-unbound | 104 | 70.1 | 42.9 | 9.18 | 14.9 | 140 |
| F09-minus | 827 | 533 | 145 | 24.7 | 34.1 | 464 |
| F10-not-exists | timeout | timeout | 1705 | 43.9 | 77.6 | timeout |
| F11-subquery-topk | 1258 | 635 | 497 | 15.8 | 10.1 | 358 |
| F12-subquery-agg-join | 4521 | 3158 | 1766 | 45.1 | 256 | 3096 |
| F13-values-union | 1.95 | 0.99 | 4.90 | 22.1 | 1.96 | 827 |
| F14-count-all | 1689 | 1473 | 637 | 44.9 | 21.5 | 2703 |

Every store that answered agreed on every count. At 10k QLever and Virtuoso are
the fastest on nearly every analytical query; Open Triplestore, Oxigraph and
RDF4J time out on the correlated `NOT EXISTS` (F10). Fuseki's `rdfs:subClassOf*`
closure (F01) takes 7.6 s at 10k where the others take 8–14 ms.

### 5.4 Concurrent clients

Each level runs for 60 s; QMpH counts the mixes completed inside the window.

**1k products**

| Store | QMpH, 1 client | QMpH, 4 clients | QMpH, 8 clients | p95 ms, 1 | p95 ms, 4 | p95 ms, 8 | Failed queries |
| --- | --: | --: | --: | --: | --: | --: | --: |
| Open Triplestore | 95,580 | 240,780 | 317,640 | 4.81 | 5.48 | 12.2 | 0 |
| Oxigraph | 157,860 | 590,280 | 721,740 | 2.84 | 2.98 | 3.76 | 0 |
| Fuseki | 18,120 | 47,280 | 27,960 | 72.6 | 116 | 438 | 0 |
| QLever | 2,460 | 9,600 | 9,960 | 154 | 168 | 306 | 0 |
| Virtuoso | 59,160 | 201,480 | 329,040 | 4.06 | 4.87 | 6.57 | 0 |
| RDF4J | 28,680 | 76,080 | 52,560 | 20.0 | 31.6 | 99.7 | 0 |

**10k products**

| Store | QMpH, 1 client | QMpH, 4 clients | QMpH, 8 clients | p95 ms, 1 | p95 ms, 4 | p95 ms, 8 | Failed queries |
| --- | --: | --: | --: | --: | --: | --: | --: |
| Open Triplestore | 8,460 | 39,300 | 64,740 | 58.5 | 56.1 | 91.0 | 0 |
| Oxigraph | 39,360 | 130,080 | 121,200 | 24.3 | 28.8 | 76.6 | 0 |
| Fuseki | 5,220 | 13,080 | 7,680 | 290 | 469 | 1638 | 0 |
| QLever | 2,340 | 8,820 | 7,680 | 174 | 187 | 439 | 0 |
| Virtuoso | 51,480 | 177,000 | 273,360 | 4.48 | 5.47 | 7.44 | 0 |
| RDF4J | 3,360 | 6,060 | 5,460 | 194 | 448 | 971 | 0 |

Virtuoso reaches the highest throughput at eight clients (273,360 QMpH at 10k,
×5.3 its single-client rate). Open Triplestore's rate grows ×7.7 from one to
eight clients at 10k, from a low single-client base. Oxigraph, Fuseki and RDF4J
lose throughput going from four to eight clients at 10k. Virtuoso's 24 short answers
(§5.6) all happened at four and eight clients.

### 5.5 SHACL validation

| Store | Scale | Median (s) | p95 (s) | Runs | Results reported |
| --- | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | 0.33 | 0.37 | 5 | 5,245 |
| Fuseki | 1k | 0.98 | 0.99 | 5 | 5,245 |
| Open Triplestore | 10k | 14.09 | 15.46 | 5 | 51,690 |
| Fuseki | 10k | 12.36 | 12.72 | 5 | 51,690 |

Only Open Triplestore and Fuseki validate over HTTP on demand: Open
Triplestore with `POST /api/datasets/{id}/validate?test=true` (validate and
answer without recording the run, like Fuseki), Fuseki with its `shacl`
service on the TDB2 dataset — the HTTP counterpart of the Jena `shacl validate`
measurement in [performance.md](performance.md#like-for-like-over-http--open-triplestore-vs-apache-jena-fuseki).
Both report the same 5,245 and 51,690 results. At 1k Open Triplestore validates
3× faster; at 10k, with the accelerator off, it is 14% slower than Fuseki and
its container peaked at 4,071 of 4,096 MiB.

### 5.6 Result-count cross-check

| Scale | Store | Query | Finding | Query instances |
| --- | --- | --- | --- | --: |
| 1k | RDF4J | Q9 | describe-differs | 1192 |
| 1k | Virtuoso | Q10 | inconsistent | 1 |
| 1k | Virtuoso | Q11 | inconsistent | 1 |
| 1k | Virtuoso | Q2 | inconsistent | 13 |
| 1k | Virtuoso | Q7 | inconsistent | 6 |
| 1k | Virtuoso | Q8 | inconsistent | 3 |
| 1k | Virtuoso | Q9 | describe-differs | 1192 |
| 10k | RDF4J | Q9 | describe-differs | 715 |
| 10k | Virtuoso | Q1 | inconsistent | 2 |
| 10k | Virtuoso | Q10 | inconsistent | 1 |
| 10k | Virtuoso | Q2 | inconsistent | 9 |
| 10k | Virtuoso | Q3 | inconsistent | 1 |
| 10k | Virtuoso | Q5 | inconsistent | 2 |
| 10k | Virtuoso | Q7 | inconsistent | 8 |
| 10k | Virtuoso | Q8 | inconsistent | 1 |
| 10k | Virtuoso | Q9 | describe-differs | 1197 |

- **Virtuoso, `inconsistent`:** for each flagged query instance, one execution
  under four or eight concurrent clients returned fewer solutions than every
  other execution of the same query (for example 10 rows instead of 19, as valid
  SPARQL JSON with HTTP 200). That happened 24 times in about 220,000
  executions at 1k and 24 times in about 190,000 at 10k, never with one client.
  Those answers are included in Virtuoso's concurrency numbers above.
- **`describe-differs`:** RDF4J and Virtuoso describe a resource with its
  incoming triples as well (24–33 triples for a reviewer), the other four with
  its outgoing triples (6). Both are allowed.
- Every other SELECT and CONSTRUCT instance got the same count from every store
  that ran it, and no instance lacked a strict majority. At 10k, 447 instances
  that only two stores reached inside the time-boxed concurrency phase were
  checked for consistency only.

### 5.7 Peak container memory

| Store | Scale | load | explore | features | concurrent | shacl |
| --- | --: | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | 776 | 1,209 | 1,178 | 1,198 | 1,211 |
| Oxigraph | 1k | 275 | 53 | 60 | 65 | — |
| Fuseki | 1k | 1,006 | 777 | 777 | 1,254 | 1,181 |
| QLever | 1k | — | 526 | 517 | 628 | — |
| Virtuoso | 1k | 474 | 1,090 | 1,090 | 1,284 | — |
| RDF4J | 1k | 562 | 970 | 621 | 1,409 | — |
| Open Triplestore | 10k | 3,019 | 2,284 | 2,153 | 2,164 | 4,071 |
| Oxigraph | 10k | 1,832 | 149 | 262 | 435 | — |
| Fuseki | 10k | 2,930 | 760 | 1,939 | 2,298 | 2,273 |
| QLever | 10k | 628 | 682 | 665 | 721 | — |
| Virtuoso | 10k | 2,926 | 2,998 | 3,125 | 3,280 | — |
| RDF4J | 10k | 1,385 | 1,175 | 1,063 | 1,326 | — |

### 5.8 Open Triplestore after the round-3 fixes (2026-10-13)

The round-3 performance work (issues
[#467](https://github.com/philipperenzen/open-triplestore/issues/467)–[#471](https://github.com/philipperenzen/open-triplestore/issues/471)
and [#527](https://github.com/philipperenzen/open-triplestore/issues/527)) was
measured with the same harness, data and query instances as above, on the same
laptop. To separate the code from the day, both Open Triplestore builds ran in
one window, back to back: **Before** is commit `570a7a30` (the 2026-10-03
commit), **After** is `9e49fa50` (develop `56a44aef`, the open sibling PR stack
#517–#522 and the fixes). Both were
built from the same `Dockerfile.ots` with thin LTO (the shipped fat-LTO link
needs about 14 GB, more than the Docker VM had free next to other containers),
so neither column is directly comparable with §5.1–5.7, which used fat LTO.
Upstream Oxigraph 0.5.11 was re-run in the same window, for the answer
cross-check below and as a reference; its 2026-10-03 numbers are shown next to
it. That it came out 13–23 % slower in this window than on 2026-10-03 is the
size of the day's effect on this machine. Data:
[`docs/perf-data/2026-10-13/`](perf-data/2026-10-13/).

**1k, BSBM explore, one client** — median ms

| Query | Before (`570a7a30`) | After | Oxigraph 0.5.11, same window | Oxigraph 0.5.11, 2026-10-03 |
| --- | --: | --: | --: | --: |
| Q1 | 0.69 | 0.66 | 0.49 | 0.41 |
| Q2 | 1.09 | 1.07 | 1.06 | 0.95 |
| Q3 | 0.82 | 0.78 | 0.62 | 0.48 |
| Q4 | 0.85 | 0.82 | 0.83 | 0.69 |
| Q5 | 2.03 | 2.18 | 4.04 | 3.61 |
| Q7 | 3.51 | 4.10 | 1.25 | 1.00 |
| Q8 | 5.84 | 6.88 | 0.90 | 0.71 |
| Q9 | 41.5 | 0.87 | 0.27 | 0.23 |
| Q10 | 1.60 | 1.50 | 0.75 | 0.62 |
| Q11 | 0.69 | 0.66 | 0.34 | 0.29 |
| Q12 | 0.80 | 0.83 | 0.45 | 0.37 |
| **QMpH** | **24,923** | **64,685** | **117,023** | **136,476** |

**1k, SPARQL 1.1 feature mix** — median ms

| Query | Before (`570a7a30`) | After | Oxigraph 0.5.11, same window | Oxigraph 0.5.11, 2026-10-03 |
| --- | --: | --: | --: | --: |
| F01-path-closure | 2.93 | 3.85 | 4.66 | 3.93 |
| F02-path-sequence-inverse | 0.91 | 1.08 | 1.81 | 1.36 |
| F03-path-alternative | 93.6 | 124 | 136 | 161 |
| F04-path-oneormore-join | 2.76 | 3.77 | 2.61 | 3.28 |
| F05-agg-group-avg | 320 | 291 | 180 | 360 |
| F06-agg-having | 9.04 | 9.20 | 5.88 | 10.2 |
| F07-agg-count-distinct | 146 | 11.8 | 16.1 | 23.5 |
| F08-optional-unbound | 2.84 | 3.75 | 7.87 | 15.8 |
| F09-minus | 5.28 | 6.53 | 35.8 | 33.2 |
| F10-not-exists | 5,047 | 42.3 | 118,901 | 111,630 |
| F11-subquery-topk | 16.7 | 18.9 | 63.6 | 58.3 |
| F12-subquery-agg-join | 357 | 364 | 217 | 180 |
| F13-values-union | 8.55 | 8.77 | 1.20 | 0.85 |
| F14-count-all | 45.2 | 87.8 | 141 | 136 |


**10k, BSBM explore, one client** — median ms

| Query | Before (`570a7a30`) | After | Oxigraph 0.5.11, same window | Oxigraph 0.5.11, 2026-10-03 |
| --- | --: | --: | --: | --: |
| Q1 | 44.8 | 3.41 | 1.57 | 1.01 |
| Q2 | 5.01 | 2.91 | 1.43 | 1.08 |
| Q3 | 39.9 | 5.35 | 2.70 | 1.85 |
| Q4 | 21.5 | 5.99 | 2.83 | 2.13 |
| Q5 | 110 | 62.1 | 31.4 | 25.8 |
| Q7 | 9.98 | 4.01 | 2.07 | 1.40 |
| Q8 | 6.55 | 2.73 | 1.36 | 0.87 |
| Q9 | 42.6 | 1.00 | 0.34 | 0.24 |
| Q10 | 44.3 | 2.34 | 1.06 | 0.72 |
| Q11 | 2.51 | 1.15 | 0.41 | 0.29 |
| Q12 | 2.36 | 1.46 | 0.56 | 0.40 |
| **QMpH** | **5,358** | **16,570** | **33,719** | **43,681** |

**10k, SPARQL 1.1 feature mix** — median ms

| Query | Before (`570a7a30`) | After | Oxigraph 0.5.11, same window | Oxigraph 0.5.11, 2026-10-03 |
| --- | --: | --: | --: | --: |
| F01-path-closure | 17.6 | 21.0 | 9.84 | 8.39 |
| F02-path-sequence-inverse | 3.67 | 5.03 | 2.00 | 1.19 |
| F03-path-alternative | 3,492 | 3,163 | 1,777 | 1,326 |
| F04-path-oneormore-join | 18.4 | 21.0 | 13.1 | 11.7 |
| F05-agg-group-avg | 4,948 | 4,588 | 2,821 | 2,175 |
| F06-agg-having | 79.1 | 90.7 | 64.0 | 55.3 |
| F07-agg-count-distinct | 320 | 305 | 212 | 189 |
| F08-optional-unbound | 117 | 126 | 76.8 | 70.1 |
| F09-minus | 1,211 | 1,138 | 685 | 533 |
| F10-not-exists | timeout | 366 | timeout | timeout |
| F11-subquery-topk | 1,326 | 1,468 | 671 | 635 |
| F12-subquery-agg-join | 5,226 | 5,834 | 5,461 | 3,158 |
| F13-values-union | 2.37 | 2.33 | 1.31 | 0.99 |
| F14-count-all | 2,102 | 2,846 | 1,500 | 1,473 |

**Load, SHACL and concurrency.**

| | Before (`570a7a30`) | After |
| --- | --: | --: |
| 10k load over the Graph Store protocol (s) | 149.3 | 105.9 |
| 10k load, peak container memory (MiB) | 3,843 | 1,744 |
| 1k load (s) / peak (MiB) | 15.6 / 786 | 11.5 / 617 |
| SHACL validation at 10k, median of 5 (s) | — (container gone) | 15.7 |
| SHACL validation at 10k, peak memory (MiB) | 4,092 | 3,958 |
| SHACL validation at 1k, median (s) | 0.53 | 0.27 |
| 10k QMpH with 1 / 4 / 8 clients | 5,640 / 29,220 / 53,880 | 16,020 / 65,220 / 61,620 |
| 1k QMpH with 1 / 4 / 8 clients | 28,740 / 182,160 / 285,300 | 66,060 / 230,940 / 239,520 |

The old build's container went away during whole-dataset SHACL validation at
10k (connection refused, its last log line the run index reaching its budget at
4,092 MiB sampled); the new build answered all six runs. Across six repeated 10k
loads of an earlier commit of the branch with the same load path and two more
campaign loads, none was OOM-killed and the sampled peaks were 1,513–1,883 MiB
([attempts](perf-data/2026-10-13/ots-10k-load-attempts.json)); on 2026-10-03
two of six were.

**Answers.** Every query instance that both builds and upstream Oxigraph ran got
the same result count from all three (5,847 instances at 1k, 7,702 at 10k,
including the correlated `NOT EXISTS`, which Oxigraph answered at 1k; at 10k only
the new build finished it, with 43,428 offers)
([`crosscheck.txt`](perf-data/2026-10-13/crosscheck.txt)).

**What changed, and what did not.**

- **DESCRIBE and the stalls.** Q9 went from 42.6 to 1.0 ms at 10k and from
  41.5 to 0.87 ms at 1k, and the p95 latencies at 10k fell from 44–71 ms to
  2–36 ms for Q1, Q3, Q4, Q7, Q9 and Q10. The 40 ms floor did not reproduce on develop before
  these fixes in a probe of the same endpoint, so which change removed it
  (`TCP_NODELAY`, now set on every connection, or changes merged between
  `570a7a30` and develop) was not isolated.
- **Correlated `NOT EXISTS`** is now a hash semi-join (§ vendored changes 10 and
  11 in [`vendor/README.md`](../vendor/README.md)): 366 ms at 10k, between
  Fuseki's 1.7 s and Virtuoso's 78 ms on 2026-10-03.
- **Per-query overhead**: the shared SPARQL parser is built once instead of on
  every parse; a dataset whose default graph merges its data graph with its
  shapes graph no longer hashes every match of a pattern that only one graph
  matches; a query that times out is cancelled instead of running on.
- **Still slower than upstream Oxigraph**: about 2× on the 10k explore mix,
  mostly Q5 (62 ms against 31 ms) and the analytical feature queries at 10k
  (F03, F05, F09, F11, F14: 1.6–2.2× Oxigraph in the same window). `COUNT(*)` over the whole dataset
  (F14) is slower than before at both scales (88 against 45 ms at 1k, 2.8 against
  2.1 s at 10k): the default graph now merges the data and shapes graphs as a set
  (develop's SPARQL fix for duplicate rows), which the old build did not. Several
  1k feature queries (F01, F02, F04, F08, F09) are 19–37 % slower than before;
  not investigated.

### 5.9 SHACL memory at 10k after the run budget (2026-10-15, AMD Ryzen 9 7900X3D, Docker Desktop on WSL 2)

**A different machine.** Everything above ran on the M1 Pro laptop; this
section ran on the x86 desktop of [performance.md](performance.md#reference-system-numbers-in-this-doc-unless-noted),
so compare its two columns with each other, not with the tables above. Same
harness, data (byte-identical inputs), shapes and 4 GiB / 4 CPU limits, Open
Triplestore only, SHACL only; before (`462b206c`, 0.8.0) and after
(`2342e3f9`, issue [#532](https://github.com/philipperenzen/open-triplestore/issues/532))
back to back. Data: [`docs/perf-data/2026-10-15-x86/`](perf-data/2026-10-15-x86/before-after.md).

| | Before (`462b206c`) | After (`2342e3f9`) |
| --- | --: | --: |
| SHACL validation at 10k, peak memory (MiB), two paired runs | 4,080 / 3,734 | 2,931 / 2,881 |
| — of which anonymous (MiB) | 3,573 / 3,279 | 2,399 / 2,350 |
| SHACL validation at 10k, median of 10 (s) | 7.60 | 6.98 |
| SHACL validation at 1k, peak (MiB) / median (s) | 1,082 / 0.158 | 959 / 0.137 |
| Results at 10k / 1k | 51,690 / 5,245 | the same, the same content |
| Three repeated 10k loads + validations, SHACL peak (MiB) | 4,021 (one attempt) | 3,018, 2,914, 3,079 |

The near-limit peak of §5.5 and §5.8 was anonymous memory, mostly the
per-run index (about 1.1 GiB) on top of the 1.3 GiB the server held, and
it grew run after run because nothing freed was handed back. The harness now
records anonymous memory and file cache per phase next to the `docker stats`
figure, and `--repeat N --max-mem-fraction 0.8` turns a run into a check that
fails above 80 % of the limit ([README](../scripts/bench-compare/README.md#the-repeated-load-check)).

### 5.10 The engine path over the accelerator's cap, on an x86 workstation (2026-10-20)

[#468](https://github.com/philipperenzen/open-triplestore/issues/468), after
§5.8. **A different machine from every other section:** an AMD Ryzen 9
7900X3D workstation running the stores in Docker Desktop on WSL 2 (24 CPUs
for the VM), shared with other jobs, which a machine-wide lock kept off it
during the runs. The containers had the same 4 GiB and 4 CPUs, the same data
(checksums as on 2026-10-13) and the same query instances. **Before** is
develop `462b206c` (0.8.0), **After** is `757c2489`; both built from
`Dockerfile.ots` with fat LTO and every source file touched before the
build, so the shared `target/` cache could not hand back an older binary.
Oxigraph 0.5.11 ran in the same window. The machine ran about half as fast
in that window as five days earlier, so only ratios within the window mean
anything. Data, the harness's tables and the probes:
[`docs/perf-data/2026-10-20-x86/`](perf-data/2026-10-20-x86/).

**10k (3.56M triples, accelerator off), BSBM explore, one client** — median ms

| Query | Before | After | Oxigraph 0.5.11 | After / Oxigraph |
| --- | --: | --: | --: | --: |
| Q1 | 5.72 | 3.79 | 2.71 | 1.40 |
| Q2 | 5.08 | 3.74 | 2.37 | 1.58 |
| Q3 | 8.78 | 6.22 | 5.18 | 1.20 |
| Q4 | 9.88 | 6.67 | 5.10 | 1.31 |
| Q5 | 87.0 | 52.5 | 53.1 | 0.99 |
| Q7 | 7.23 | 5.39 | 3.57 | 1.51 |
| Q8 | 4.52 | 3.41 | 2.29 | 1.49 |
| Q9 | 1.98 | 1.43 | 0.72 | 2.00 |
| Q10 | 4.08 | 2.91 | 1.92 | 1.52 |
| Q11 | 2.08 | 1.52 | 0.77 | 1.97 |
| Q12 | 2.60 | 1.83 | 1.05 | 1.75 |
| **QMpH** | **10,859** | **16,586** | **19,331** | **1.17× slower** |

At 1k (the accelerator on) the explore mix gave 48,629 QMpH before, 44,133
after and 63,030 for Oxigraph. The after build's deficit at 1k did not
reproduce in an interleaved run of both builds loaded side by side
([`before-after.md`](perf-data/2026-10-20-x86/before-after.md)); none of
the changes touches the accelerator's evaluators.

**10k, SPARQL 1.1 feature mix** — median ms

| Query | Before | After | Oxigraph 0.5.11 |
| --- | --: | --: | --: |
| F03-path-alternative | 3,039 | 1,968 | 2,234 |
| F05-agg-group-avg | 7,192 | 4,084 | 4,511 |
| F09-minus | 1,682 | 1,337 | 1,246 |
| F11-subquery-topk | 1,412 | 743 | 598 |
| F12-subquery-agg-join | 9,189 | 7,396 | 6,436 |
| F14-count-all | 1,433 | 2.41 | 1,347 |

The whole feature table is in the data directory. Every query instance got the
same result count from all three (2,632 instances at 1k, 2,967 at 10k).

**Where the time went.** Diagnosed on the before build, with Oxigraph and
the before build loaded side by side and queried in turn:

- **The merged default graph.** A dataset service reads the RDF merge of its
  data graph and its shapes graph, and the evaluator opened a scan on both for
  every pattern it looked up: one lookup per row in Q5's for-loop joins. Q5
  took 32.0 ms through the dataset's service, 20.2 ms through a second service
  of the same dataset over the data graph alone, and 17.1 ms in Oxigraph. Graphs
  without the pattern's predicate are now left out (vendored oxigraph change
  15); after it Q5 runs within 14 % of the data graph alone.
- **`COUNT(*)` of the dataset (F14)** was always scanned: the dataset endpoint
  prints a scoped query back from its syntax tree, which writes the count as a
  sub-select, and the fast count only knew the written form; with two graphs
  it also needed the merge. Both are now counted from the count index.
- **Per request**, a trivial `ASK` took 0.65 ms against Oxigraph's 0.20 ms, and
  `GET /livez` 0.24 ms. Of the 0.4 ms, about 0.06 ms is the token, the rest
  eleven identity-database lookups (the user, the endpoint ACL rules, the
  dataset, its service and graphs, the role twice), the query's parses and the
  streamed response. The lookups now use cached statements and the store
  parses a query once on its engine path (it parsed three times): 1.71 → 1.53
  ms in the 2026-10-20 window, against Oxigraph's 0.56 ms.
- **Not the file layout:** a full compaction of the before build's store
  (`oxigraph optimize`) left Q5 where it was.

The target of #468, within about 1.5× of Oxigraph on the 10k explore mix, is
met on this machine. The queries that are still 1.5–2× Oxigraph are the
shortest ones (Q9, Q11, Q12, under 2 ms), where the per-request cost above is
most of the time.

## 6. Where Open Triplestore loses

The bullets are the 2026-10-03 findings; where the round-3 fixes changed one,
its 2026-10-13 status follows it (§5.8).

- **Its own engine is faster than it is** — 2.7× on the explore mix at 1k, 6×
  at 10k. A follow-up probe ([`diagnostics.json`](perf-data/2026-10-03/diagnostics.json))
  ruled out the named-graph layout and the `FROM`/`FROM NAMED` clauses this
  store injects: upstream Oxigraph with the data in a named graph and the same
  clauses answers Q9 in 0.2 ms and Q1 in 1 ms; this store's dataset endpoint
  takes 42 ms and 5 ms. The cost grows with the data (Q9: 0.5 ms at 1k, 42 ms at
  10k), so something per query works in proportion to the store outside the
  evaluator's index lookups. Not yet isolated —
  [#468](https://github.com/philipperenzen/open-triplestore/issues/468).
  *2026-10-13:* about 2× at 10k and 1.8× at 1k against Oxigraph run in the same
  window; DESCRIBE 1.0 ms. The rest is Q5 and the analytical queries at 10k.
  *2026-10-20 (x86 workstation, §5.10):* 1.17× Oxigraph on the 10k explore mix
  (was 1.78×); what is left is about 0.3 ms per request outside the engine.
- **Above 2 M triples in a 4 GiB container the accelerator is off**, by design
  (`OTS_PARALLEL_QUERY_MAX_TRIPLES`, default 2,000,000 at this memory size; since
  round 3, 1,048,576 in 4 GiB, a quarter of the limit at about 1 KiB a triple), and
  every query goes to RocksDB. That is why the 10k explore mix drops from
  50,897 to 7,229 QMpH while Oxigraph and Virtuoso barely move. With more memory
  the cap rises; this comparison kept every store at 4 GiB.
- **Memory headroom.** The load and the accelerator build can coincide and
  exceed 4 GiB (2 of 6 loads OOM-killed), and SHACL at 10k came within 25 MiB of
  the limit — [#469](https://github.com/philipperenzen/open-triplestore/issues/469).
  *2026-10-13:* no OOM in eight 10k loads (peaks 1.5–1.9 GiB); SHACL at 10k
  still peaks at 3,958 MiB.
- **Loading** is 4–11× slower than the other stores' bulk loaders at 10k and
  2–3× slower than Oxigraph, Fuseki and RDF4J over HTTP; full-text indexing of every
  literal is part of the cost (BSBM's reviews and comments are long texts).
- **Correlated `NOT EXISTS`** (F10) — inherited from the evaluator; ~25× faster
  than upstream Oxigraph, still 25–190× slower than Fuseki, QLever and Virtuoso
  at 1k and a timeout at 10k —
  [#471](https://github.com/philipperenzen/open-triplestore/issues/471).
  *2026-10-13:* 42 ms at 1k, 366 ms at 10k.
- **Analytical queries at 10k** (F03, F05, F09, F11, F12, F14): 0.8–4.5 s where
  QLever and Virtuoso take 10–260 ms. Without the accelerator these run on the
  row-at-a-time evaluator over RocksDB.
- **`COUNT(DISTINCT)` at 1k** (F07): 125 ms against 8–24 ms for the others,
  slower than upstream Oxigraph's 24 ms; not investigated. *2026-10-13:* 11.8 ms
  (changes merged since `570a7a30`).

## 7. Threats to validity

- **One laptop, shared.** Other agents' jobs ran on the same machine between and
  sometimes during runs; the quiet-window rule bounds this but cannot rule it
  out. Each configuration ran once: there are no repeated campaigns and no
  confidence intervals, and the earlier
  [perf-gate measurements](performance.md#performance-regression-gate) show
  ±10–25 % swings between identical runs on shared machines.
- **Docker Desktop on macOS.** The stores run in a Linux VM; the input file is
  bind-mounted from the host, which slows the loads of all stores; CPU and I/O
  behave differently from a Linux server, and on `arm64` rather than the
  `amd64` most of these stores are tuned on.
- **Small scales.** 0.37 M and 3.56 M triples are the in-memory regime for most
  stores. Stores built for data much larger than RAM (QLever, Virtuoso) do not
  show their strengths here; results at 100 M+ triples could order the stores
  differently.
- **Not the BSBM test driver.** The parameters are drawn by this harness
  (§3), so the QMpH figures are comparable only within this document.
- **Default configurations.** Each store ran its documented default plus the
  memory settings above. Tuning (RDF4J's index set, Fuseki's loader choice,
  QLever's cache, Virtuoso's indexes) could change any row.
- **QLever without its cache.** Its result cache is part of its design; it is
  off here so that repeated feature queries are evaluated, which may cost QLever
  more than the others on the explore mix.
- **Different load paths.** Offline bulk loaders and HTTP loads are not the
  same operation; the HTTP column puts three stores on one path.
- **Memory readings** are sampled about once a second and exclude page cache
  (§3), so short peaks can be missed (QLever's 1-second 1k load was not sampled
  at all).
- **Timeouts.** Open Triplestore stops queries at its own 30 s limit; the other
  stores were given 300 s. A query that times out on one and not the other is
  marked as such.
- **One HTTP connection per client** in a Python client: per-request overhead is
  the same for every store, but it is part of every latency.
- **The 2026-10-13 re-run (§5.8)** used thin LTO for both Open Triplestore
  builds, ran on a busier laptop (other jobs, the host swapping; the load
  average at the start of each run is in its run file), and compares the after
  build with the other stores' 2026-10-03 numbers only through upstream
  Oxigraph, which was re-run in the same window. The datasets were regenerated
  with the same generator; their checksums match 2026-10-03.

## 8. What was not measured

- Proprietary stores (GraphDB, Stardog, AllegroGraph, MarkLogic) and Amazon
  Neptune — see §2.
- BSBM scales of 100k and 1M products; BSBM's business-intelligence and update
  use cases; SP²Bench.
- Writes: concurrent update throughput, SPARQL Update, transactional loads next
  to readers (Open Triplestore's own numbers for that are in
  [performance.md](performance.md#otl-scale-benchmark--asset-shaped-data-deep-shacl-concurrent-writers-2026-09)).
- SHACL on RDF4J (its ShaclSail validates at commit, with no on-demand
  validation over the server's HTTP API), and on Oxigraph, QLever and Virtuoso,
  which do not validate SHACL.
- Open Triplestore with its accelerator on at 10k, which needs more than the
  4 GiB every store was given.
- GeoSPARQL, reasoning, full-text search, cold-cache (first-query) latency,
  `amd64` hardware, and repeated runs for variance.

## 9. Reproduce it

```bash
W=$HOME/bench-work
scripts/bench-compare/fetch.sh "$W" 1000 10000
docker build -f scripts/bench-compare/Dockerfile.ots -t ots-bench:$(git rev-parse --short HEAD) .
# (--build-arg CARGO_PROFILE_RELEASE_LTO=thin when the Docker VM cannot spare ~14 GB for the fat-LTO link)
python3 scripts/bench-compare/campaign.py --data-root "$W/data" --cache "$W/cache" --out "$W/results/raw" \
  --ots-image ots-bench:$(git rev-parse --short HEAD) --stores ots,oxigraph,fuseki,qlever,virtuoso,rdf4j \
  --scales 1k,10k --require-quiet 10k
python3 scripts/bench-compare/bench.py report --results "$W/results/raw" --out "$W/results/summary" \
  --stores ots,oxigraph,fuseki,qlever,virtuoso,rdf4j --triples 1k=374911,10k=3564773
```

The harness, its fairness rules and the licences of everything it fetches are
described in [`scripts/bench-compare/README.md`](../scripts/bench-compare/README.md).
A full campaign takes about two hours on the machine above, most of it the
concurrency phases and the timeouts.
