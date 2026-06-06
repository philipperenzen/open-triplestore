# Benchmark harness & charts

Reproducibility artifacts for [`docs/performance.md`](../performance.md). All
results in that document were produced on the reference system documented there
(AMD Ryzen 9 7900X3D, 24 vCPU / 30.9 GiB, Docker Desktop on WSL2, release build,
Oxigraph 0.4.11).

## 1. In-process micro-benchmarks (Criterion)

The engine-level numbers come from [`benches/performance.rs`](../../benches/performance.rs):

```bash
docker build --target builder -t ots-builder .
docker run --rm -v "$PWD:/app" -v ots_target_rel:/app/target -w /app ots-builder \
  cargo bench --bench performance --features full
```

97 benchmarks across insert / query / paths / update / geosparql / shacl /
concurrent. Criterion writes `target/criterion/**/estimates.json` (machine
readable) and an HTML report at `target/criterion/report/index.html`.

## 2. Cross-store comparison (Open Triplestore vs Apache Jena Fuseki)

Same host, same 500k-triple dataset, identical SPARQL over HTTP. Files here:

| File | Purpose |
|---|---|
| `gen-data.sh` | deterministic dataset generator (100k persons → 500k triples) |
| `compare-fuseki.sh` | orchestrator: both servers on one Docker network |
| `compare-client.sh` | in-container client: loads data, times paced queries |

```bash
cd docs/benchmarks
./gen-data.sh > data.nt                 # 43 MB N-Triples
bash compare-fuseki.sh                  # prints the latency table
```

Notes:
- Fuseki is `stain/jena-fuseki` (TDB2). Open Triplestore is run from the
  `ots-builder` image's release binary; data is loaded into a **public dataset
  graph** so the ACL-scoped `/sparql` endpoint can read it.
- Queries are **compute-bound** (≤10-row results) so timing reflects engine
  work, not result serialization. Requests are **paced** (1.2 s apart) to stay
  under Open Triplestore's per-IP SPARQL rate limiter — otherwise rapid-fire
  requests return `429` and corrupt the median.
- The comparison exercises Open Triplestore's full multi-tenant HTTP stack (ACL
  `FROM` rewrite + Axum + rate limiter); the in-process numbers in §1 are the raw
  engine speed.

## 3. Charts

SVGs are generated from a small dependency-free script:

```bash
node gen_charts.mjs chart_data.json .
```

`chart_data.json` holds the plotted values (sourced from the runs above); edit it
and re-run to regenerate the `*.svg` files embedded in `performance.md`.
