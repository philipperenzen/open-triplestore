# Store comparison harness

Runs the same SPARQL workloads against Open Triplestore and five open-source
triplestores, one store at a time, under the same Docker memory and CPU limits.
The method and the results are in
[`docs/performance-comparison.md`](../../docs/performance-comparison.md); the
measured data is under [`docs/perf-data/`](../../docs/perf-data/).

| File | What it does |
|---|---|
| `fetch.sh` | Downloads the BSBM tools and the Fuseki jar (SHA-256 checked), generates the BSBM N-Triples and the seeded query instances. |
| `Dockerfile.ots` | Builds Open Triplestore with `cargo build --release --features full` in a Linux image, so it runs under the same limits as the others. |
| `compose/*.yml` | One compose file per store, images pinned by digest, memory/CPU limits from `BENCH_MEM`/`BENCH_CPUS`. |
| `config/` | Fuseki's service configuration (query, update, Graph Store, SHACL) and the RDF4J repository template. |
| `queries/features/*.rq` | The SPARQL 1.1 feature mix: property paths, aggregates, OPTIONAL/MINUS/NOT EXISTS, subqueries, VALUES/UNION. |
| `shapes/bsbm-shapes.ttl` | SHACL Core shapes over the BSBM data. |
| `bench.py` | The load generator (`client`), the query-instance generator (`params`) and the report (`report`). Standard library only. |
| `campaign.py` | The orchestrator: quiet-window check, start, load, settle, workloads, memory sampling, teardown. |

## Licences of what the harness fetches

Nothing third-party is vendored here. At run time the harness fetches:

- **BSBM tools 0.2** (Freie Universität Berlin; GPL-3.0) from SourceForge: the
  data generator and the explore query templates. Its output data and the
  queries it is used to build are not part of this repository.
- **Apache Jena Fuseki 6.2.0** (Apache-2.0) from Maven Central.
- The store images below. Every store is open source under a licence that puts
  no condition on publishing benchmark results.

| Store | Licence | Image (pinned by digest in `compose/`) |
|---|---|---|
| Open Triplestore | this repository | built locally by `Dockerfile.ots` |
| Oxigraph server 0.5.11 | MIT OR Apache-2.0 | `ghcr.io/oxigraph/oxigraph:0.5.11` |
| Apache Jena Fuseki 6.2.0 (TDB2) | Apache-2.0 | Maven Central jar on `eclipse-temurin:21-jdk` |
| QLever (commit caba7eb) | Apache-2.0 | `adfreiburg/qlever:commit-caba7eb0d1` |
| Virtuoso Open Source 7.2.17 | GPL-2.0 | `openlink/virtuoso-opensource-7:7.2.17` |
| Eclipse RDF4J 6.1.0 (Native Store) | EDL-1.0 (BSD-3-Clause) | `eclipse/rdf4j-workbench:6.1.0-tomcat` |

## Running it

```bash
W=$HOME/bench-work                     # anything outside the repository
scripts/bench-compare/fetch.sh "$W" 1000 10000

# Open Triplestore at the commit under test (40 min on an M1 Pro; heavy, so
# through the build slot when other builds share the machine):
docker build -f scripts/bench-compare/Dockerfile.ots -t ots-bench:$(git rev-parse --short HEAD) .

python3 scripts/bench-compare/campaign.py --data-root "$W/data" --cache "$W/cache" \
  --out "$W/results/raw" --ots-image ots-bench:$(git rev-parse --short HEAD) \
  --stores ots,oxigraph,fuseki,qlever,virtuoso,rdf4j --scales 1k,10k --require-quiet 10k

python3 scripts/bench-compare/bench.py report --results "$W/results/raw" --out "$W/results/summary" \
  --stores ots,oxigraph,fuseki,qlever,virtuoso,rdf4j
```

`campaign.py` waits before each store until no `rustc`/`cargo` process runs,
the 1-minute load average is under `--max-load` (default 6) and at least
15 GiB of disk is free, polling every `--poll` seconds. A scale listed in
`--require-quiet` is skipped (and reported) when no quiet window comes within
`--max-wait`; other scales run anyway and are flagged `not_quiet` in their
`meta.json`. Smoke test with `--max-wait 0 --seconds 5 --feature-runs 1`.

## Fairness rules the scripts enforce

- Every store container gets the same `mem_limit`/`memswap_limit` (4 GiB) and
  `cpus` (4). JVM heaps are 3 GiB; Virtuoso's buffers follow OpenLink's sizing
  for 4 GB; QLever's query memory is 3 GB.
- One store at a time. The compose project is torn down with its volume
  before the next store starts.
- The same data file, loaded with each store's documented bulk path (offline
  loaders where the store has one), the same 60 s settle after the load, the
  same query instances in the same order, the same warm-up (20 explore mixes;
  2 runs per feature query) and the same number of measured runs (100 mixes;
  10 runs per feature query; 5 SHACL runs).
- Result caches are off where a store has one (Open Triplestore
  `OTS_QUERY_CACHE=off`, QLever `-c 0GB`), and Open Triplestore's per-IP rate
  limiter is off. No other tuning beyond the memory settings above.
- The load generator runs in its own container (2 CPUs) on the store's Docker
  network and keeps one HTTP connection per client.
- Every answer is counted (solutions, or triples for CONSTRUCT/DESCRIBE) and
  parsed strictly; the report compares the counts store by store per query
  instance and lists every difference in `crosscheck.csv`.
