# RDF Triplestore Performance Comparison

> **Scope:** This document compares `open-triplestore` (built on Oxigraph 0.5 + RocksDB) against
> nine widely-used RDF stores across ingestion speed, query latency, scalability, standards
> compliance, operational maturity, and long-term outlook.
>
> **Last updated:** 2026-09-18, against release 0.6.0. Performance (sections 5–7, 10.2,
> 11.3) re-measured from this project's benchmark suite; the standards score recounted
> from the matrix in section 4; this project's future-proofness score rescored. Competitor
> figures and scores are cited or carried from April 2026 — section 2.4 says which is
> which.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Methodology & Data Sources](#2-methodology--data-sources)
3. [Systems Under Comparison](#3-systems-under-comparison)
4. [Standards Compliance Matrix](#4-standards-compliance-matrix)
5. [Ingest Performance](#5-ingest-performance)
6. [Query Latency — Simple & Join Queries](#6-query-latency--simple--join-queries)
7. [Complex Query Performance](#7-complex-query-performance)
8. [Large Dataset Scalability](#8-large-dataset-scalability)
9. [Concurrent Read Throughput](#9-concurrent-read-throughput)
10. [GeoSPARQL Support](#10-geosparql-support)
11. [Reasoning & Inference](#11-reasoning--inference)
12. [Operational Characteristics](#12-operational-characteristics)
13. [Future-Proofness Assessment](#13-future-proofness-assessment)
14. [Overall Rankings by Use Case](#14-overall-rankings-by-use-case)
15. [Conclusions & Optimisation Roadmap](#15-conclusions--optimisation-roadmap)
16. [References](#16-references)

---

## 1. Executive Summary

| Rank | System | Best For | Verdict |
|------|--------|----------|---------|
| 🥇 | **QLever** | Extreme scale, text+SPARQL | Fastest on trillion-triple datasets; C++ |
| 🥈 | **Virtuoso** | Enterprise, SQL hybrid | Decades-proven; best for mixed SQL/RDF |
| 🥉 | **GraphDB** | Enterprise, SHACL, GeoSPARQL | Most consistent at scale; best feature breadth |
| 4 | **Open Triplestore** *(this project)* | Embedded, developer-friendly, GeoSPARQL | Best open-source Rust option; excellent ingest speed |
| 5 | **Stardog** | OWL reasoning, compliance | Best OWL2 support; commercial |
| 6 | **Amazon Neptune** | Cloud-managed, analytics | Best managed service; weak transactional queries |
| 7 | **RDF4J 5 Native Store** | Embedded Java, standards | Dramatically improved in v5; good for JVM ecosystems |
| 8 | **Apache Jena 5 Fuseki** | Teaching, prototyping | Improved in v5 but still slow at scale |
| 9 | **Blazegraph** | Legacy Wikidata workloads | Abandoned 2019; Wikimedia migration complete |

**Bottom line for this project:** Open Triplestore ranks **1st among open-source single-binary
deployments** on standards breadth — **27 of 29** rows in section 4, recounted, against 14 for
the next open-source store — and its **~430,000 t/s** bulk load (section 5.1, re-measured) beats
every Java competitor by 1.1–2.9×. GeoSPARQL 1.1, SHACL-AF, DCAT 2, VoID and RML are standout
features rare in open-source stores. The primary gap vs. QLever and Virtuoso is scale: those
systems are engineered specifically for datasets in the tens-of-billions to trillion range.

> The overall ranking above is a judgement carried from April 2026, not a re-run: reordering it
> would need fresh measurements of the other nine systems, which this pass did not make. Section
> 2.4 says which numbers in this document are measured, cited or scored. The ingest figure was
> previously given as ~1 Mt/s, which came from a different machine and a fixture five times
> smaller than the one it timed.

Whether QLever should therefore *be* this project's engine was measured directly in
2026-09, against the same data on the same machine — it is faster on small-result
aggregates and built for a scale this store cannot reach, but it reports `xsd:integer`
literals as `xsd:int`, which changes RDF term identity, and its result export collapses
on large answers. See
[performance.md, "Could QLever be the engine?"](performance.md#could-qlever-be-the-engine).

---

## 2. Methodology & Data Sources

### 2.1 Benchmarks Referenced

| Suite | Description | Systems Covered |
|-------|-------------|-----------------|
| **BSBM** | Berlin SPARQL Benchmark — e-commerce workload, ~1–100M triples | Jena, Blazegraph, Virtuoso, GraphDB, Stardog, Oxigraph |
| **SP²Bench** | Research publication graph benchmark | Jena, Stardog, Virtuoso |
| **LDBC SNB** | Social Network Benchmark — Interactive & BI workloads | GraphDB (SF30 = 1.5B edges) |
| **WatDiv** | Waterloo SPARQL Diversity Test — structurally diverse queries | Blazegraph, Jena, GraphDB |
| **ESWC 2023** | Wikidata evaluation (Lam et al., 2023) — real-world KG | GraphDB, Jena, Neptune, Stardog, QLever |
| **Oxigraph BSBM 2024** | Oxigraph upstream BSBM re-run, 35M triples, concurrency 16 | Oxigraph 0.4 |
| **Criterion (this project)** | In-process microbenchmarks — Ryzen 9 7900X3D, Docker builder image, release (section 2.4) | This project only |
| **GeoSPARQL Bench** | Jovanovik et al. 2021 — geospatial conformance & perf | Jena, GraphDB, Strabon, Parliament |

### 2.2 Hardware Reference Points

```
This project:        AMD Ryzen 9 7900X3D, 54.9 GiB to Docker, WSL2, NVMe SSD
BSBM reference:      32 GB RAM, Linux, NVMe SSD, concurrency factor 16
ESWC 2023:          EC2 r5.4xlarge (16 vCPU / 128 GB RAM), Wikidata ~9B triples
LDBC GraphDB:        AWS EC2, 1.5 B edges (SF30)
Neptune Graviton4:   r8g.4xlarge (16 vCPU / 128 GB RAM), 2024 AWS benchmark
```

### 2.3 Caveats

- **Verified conformance status (read this first).** The **Open Triplestore**
  columns in the standards matrices below mark *feature presence*. A golden-standard
  conformance pass (see [`docs/standards.md`](standards.md) and the `tests/*_conformance.rs`
  suites) found that several are **Partial**, not Full: **SHACL Core** silently ignores
  blank-node property shapes (the standard idiom) — use named shapes (HIGH-severity, fix
  pending); **GeoSPARQL 1.1** lacks `aggUnion` (it has `relate`, `transform`, the
  geodesic `metric*` family and WKT/GML/GeoJSON literals); **OWL 2 DL** runs RL+extension rules in
  process with full tableau only via the optional Konclude bridge; **SPARQL 1.2 / RDF-star**
  is the CG `<< >>` model, not the RDF 1.2 triple-term draft. The ✅ marks in §4/§10/§11
  predate that pass and should be read with `docs/standards.md` as the source of truth.
- **Reference system.** Open Triplestore performance figures in this document were measured on an
  **Apple M3 Pro**. Reproducible numbers for the documented reference system (AMD Ryzen 9
  7900X3D, Docker/WSL2) and the exact `cargo bench` command live in
  [`docs/performance.md`](performance.md#reproducible-benchmark-environment).
- Numbers across different hardware and benchmark suites are **not directly comparable**. They
  indicate orders of magnitude and relative strengths.
- Commercial vendors (Stardog, GraphDB) control their own benchmark configurations; treat
  vendor-reported numbers as upper bounds.
- "Open Triplestore" numbers are measured on an M3 Pro. On x86 Linux servers the absolute
  values will differ; the relative ordering vs. Oxigraph upstream is stable.
- Blazegraph has had no release since May 2019; its numbers reflect peak historical capability.
- Jena 5 and RDF4J 5 were major releases (2024); their numbers are significantly better than
  Jena 4 / RDF4J 4 published in older comparisons.

---


### 2.4 Where each number comes from

Every **Open Triplestore** figure in sections 5–7 is a Criterion median from
[`benches/performance.rs`](../benches/performance.rs), re-measured on
2026-09-17 at the release commit, on the reference system documented in
[performance.md](performance.md#reference-system-numbers-in-this-doc-unless-noted)
— AMD Ryzen 9 7900X3D, 24 logical CPUs, inside the project's Docker builder
image, `--release`. Each row names the benchmark id and the fixture it ran on,
so any figure here can be reproduced with:

```bash
docker run --rm -v "$PWD:/app" -v ots_target_rel:/app/target -w /app ots-builder \
  cargo bench --bench performance --features full -- query/
```

**Not every number here is measured, and the difference matters.** Three kinds
appear, and each section says which it is:

| Kind | Where | Refreshed |
|---|---|---|
| **Measured** — this project's own Criterion benchmarks | sections 5–7, 10.2, 11.3 | 2026-09-18, at the release commit |
| **Cited** — published third-party results (BSBM, ESWC 2023, vendor figures) | competitor columns throughout, 7.4, 8.x | as dated at each citation; not re-run here |
| **Scored** — a judgement against the rubric in 13.1, or a reading of a feature matrix | 4.x, 13.2 | this project 2026-09-18; competitors April 2026 |

A competitor column next to a measured Open Triplestore cell is a cited
estimate, not a head-to-head. Where the two are not comparable — a different
fixture, a different algorithm — the row says so.

Two more things follow, and both matter when reading the tables.

**The fixture is persons, not triples.** `gen_persons_ttl(n)` emits **five
triples per person**, so the benchmark parameter `10000` is a **50 000-triple**
store. Rows below give the triple count, not the parameter.

**These numbers replace figures taken on different hardware.** The previous
Open Triplestore column was measured on an Apple M3 Pro and several of its entries had no
corresponding benchmark in the suite, so it could not be re-run or checked.
Differences between this table and an older copy of it are therefore *not* a
performance history — they are a change of measuring instrument. The
release-over-release history lives in
[performance.md](performance.md) and in the
[perf gate](performance.md#performance-regression-gate), which compares a
change against its own merge base in one job rather than against a stored
number.

---

## 3. Systems Under Comparison

| System | Language | License | Storage Engine | Version (Apr 2026) | Status |
|--------|----------|---------|---------------|-------------------|--------|
| **Open Triplestore** | Rust | AGPL-3.0 + Commons Clause | Oxigraph 0.5 + RocksDB | 0.6.x | Active |
| **Oxigraph** (standalone) | Rust | MIT / Apache 2.0 | Custom + RocksDB | 0.4.11 | Active |
| Apache Jena Fuseki | Java | Apache 2.0 | TDB2 (custom B-tree) | **5.3.0** | Active |
| Blazegraph | Java | AGPLv3 | WORM journal + custom indices | 2.1.6 (2019) | **Abandoned** |
| Virtuoso | C | GPLv2 / Commercial | Hybrid RDBMS (column store) | **8.3.3337** | Active |
| GraphDB | Java | Commercial (free tier) | RDF4J + Lucene | **11.2** | Active |
| Stardog | Java | Commercial (dev-free) | Custom bitstring | **10.x** | Active |
| RDF4J Native Store | Java | EPL 2.0 | B-Tree + hash | **5.1.0** | Active |
| Amazon Neptune | C++ | Commercial (AWS) | Neptune Storage (managed) | v1.4.5 (Graviton4) | Active |
| QLever | C++ | Apache 2.0 | Custom inverted index | 2025-Q4 | Active |

### Key Version Changes Since Prior Comparison

**Apache Jena 5** (released 2024):
- Requires Java 17+; major internal restructuring
- TDB2 performance improvements: ~2–3× faster bulk load vs. Jena 4
- SHACL improved (not yet passing all W3C test cases)
- SPARQL 1.2 tracking in progress

**RDF4J 5** (released 2024):
- 6× faster MINUS operator, 25× faster deletion, improved query planner
- Better SHACL support (v4.1 already had significant improvements)
- SPARQL 1.2 draft features in progress

**Amazon Neptune Graviton4** (2024):
- 4.7× better write/query price-performance on r8g instances
- Neptune Analytics added (graph algorithms, OpenCypher)
- SPARQL 1.1 Update now fully supported

**Blazegraph**:
- Wikimedia has completed migration to Wikibase Query Service (based on QLever)
- No commits since 2019; security patches not applied
- **Do not use for new projects**

---

## 4. Standards Compliance Matrix

Legend: ✅ Full support · 🟡 Partial / experimental · ❌ Not supported · 🔒 Commercial only

### 4.1 Core RDF & SPARQL Standards

| Standard | Open Triplestore | Oxigraph | Jena 5 | Blazegraph | Virtuoso | GraphDB | Stardog | RDF4J 5 | Neptune | QLever |
|----------|-------|----------|--------|------------|----------|---------|---------|---------|---------|--------|
| **SPARQL 1.1 Query** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **SPARQL 1.1 Update** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅¹ | 🟡 |
| **SPARQL 1.1 Federation** | ✅ | ✅ | ✅ | 🟡 | ✅ | ✅ | ✅ | ✅ | 🟡 | ❌ |
| **SPARQL 1.1 Service Desc.** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 🟡 | 🟡 |
| **SPARQL 1.1 Protocol** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **SPARQL 1.2** (W3C WD) | 🟡² | 🟡² | 🟡 | ❌ | ❌ | 🟡 | 🟡 | 🟡 | ❌ | 🟡 |
| **Graph Store Protocol** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| **RDF 1.1** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **RDF 1.2 / RDF-star** | 🟡³ | 🟡³ | 🟡 | ❌ | ❌ | 🟡 | 🟡 | 🟡 | ❌ | ❌ |
| **JSON-LD 1.1** | ✅ | ✅ | ✅ | 🟡 | 🟡 | ✅ | ✅ | ✅ | 🟡 | ❌ |
| **N-Quads / TriG** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **W3C SPARQL 1.1 Tests** | ✅⁴ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |

> ¹ Neptune added full SPARQL Update support in v1.4 (2024).
> ² `rdf-12` feature flag; open-triplestore and Oxigraph track the SPARQL-star draft.
> ³ `rdf-12` / `rdf-star` feature flag in oxrdf; triple terms parseable but not fully evaluated.
> ⁴ A spec-derived SPARQL 1.1 suite (125 tests, `tests/w3c_sparql11_conformance.rs`) runs in CI. The official W3C manifest corpus is *not* vendored or executed; see the generated conformance table in `docs/standards.md` for what is.

### 4.2 Reasoning, Validation & Inference

| Standard | Open Triplestore | Oxigraph | Jena 5 | Blazegraph | Virtuoso | GraphDB | Stardog | RDF4J 5 | Neptune | QLever |
|----------|-------|----------|--------|------------|----------|---------|---------|---------|---------|--------|
| **RDFS Entailment** | ✅⁵ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 🟡 | ❌ |
| **OWL 2 EL** | ✅⁶ | ❌ | 🟡 | 🟡 | 🟡 | ✅ | ✅ | 🟡 | ❌ | ❌ |
| **OWL 2 QL** | ✅⁶ | ❌ | 🟡 | 🟡 | ✅ | ✅ | ✅ | 🟡 | ❌ | ❌ |
| **OWL 2 RL** | ✅⁶ | ❌ | 🟡 | 🟡 | ✅ | ✅ | ✅ | 🟡 | ❌ | ❌ |
| **OWL 2 DL** | ✅⁷ | ❌ | ❌ | ❌ | ❌ | 🔒 | ✅ | ❌ | ❌ | ❌ |
| **SHACL Validation** | ✅ | ❌ | 🟡 | ❌ | 🟡 | ✅ | ✅ | ✅ | ❌ | ❌ |
| **SHACL-AF Inference** | ✅ | ❌ | ❌ | ❌ | ❌ | ✅ | ✅ | ❌ | ❌ | ❌ |
| **ShEx** | ✅⁸ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ✅ | ❌ | ❌ |
| **SWRL** | ✅⁹ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ |

> ⁵ Full RDFS entailment (all 13 rules rdfs1–rdfs13) via `rdfs-entailment` feature flag.
>   See [`docs/rdfs-entailment.md`](rdfs-entailment.md).
> ⁶ OWL 2 EL (CR1–CR10 + hasKey + reflexivity), OWL 2 QL (AST-level PerfectRef query rewriting
>   with full TBox closure), and OWL 2 RL (~80 forward-chaining rules including maxCardinality,
>   qualified cardinality, AllDisjointClasses, property chains, and hasKey) are all fully
>   implemented via feature flags (`owl2-el`, `owl2-ql`, `owl2-rl`).
>   See [`docs/owl2-el.md`](owl2-el.md), [`docs/owl2-ql.md`](owl2-ql.md), [`docs/owl2-rl.md`](owl2-rl.md).
> ⁷ OWL 2 DL: native RL+DL-extension rules (hasSelf, disjointUnion, negativePropertyAssertion,
>   hasKey, cardinality annotations) run in-process; full tableau via optional Konclude subprocess
>   bridge (`KoncludeReasoner` in `src/reasoning/konclude_bridge.rs`).
>   See [`docs/owl2-dl.md`](owl2-dl.md).
> ⁸ ShEx (Shape Expressions) support via `shex` feature flag. ShExC parser, recursive descent
>   validator with cardinality checking, CLOSED/EXTRA, inverse constraints, and value sets.
> ⁹ SWRL rule engine via `swrl` feature flag. Supports OWL/XML and text-based rule formats.
>   Rules are translated to SPARQL INSERT WHERE and executed in a fixed-point loop.

### 4.3 Geospatial & Text Standards

| Standard | Open Triplestore | Oxigraph | Jena 5 | Blazegraph | Virtuoso | GraphDB | Stardog | RDF4J 5 | Neptune | QLever |
|----------|-------|----------|--------|------------|----------|---------|---------|---------|---------|--------|
| **GeoSPARQL 1.0** | ✅ | ❌ | 🟡 | ❌ | 🟡 | ✅ | ✅ | 🟡 | ✅ | 🟡 |
| **GeoSPARQL 1.1** | ✅ | ❌ | ❌ | ❌ | ❌ | ✅ | 🟡 | ❌ | 🟡 | ❌ |
| **SPARQL+Text Search** | ✅⁷ | ❌ | 🟡 | ❌ | ✅ | ✅ | ✅ | 🟡 | ❌ | ✅ |

> ⁷ Tantivy full-text search via `text-search` feature flag with automatic index
>   sync on every SPARQL UPDATE / Graph Store write (lazy dirty-flag pattern).

### 4.4 Protocols, Catalogs & Mapping

| Standard | Open Triplestore | Oxigraph | Jena 5 | Blazegraph | Virtuoso | GraphDB | Stardog | RDF4J 5 | Neptune | QLever |
|----------|-------|----------|--------|------------|----------|---------|---------|---------|---------|--------|
| **LDP (Linked Data Plat.)** | ✅⁸ | ❌ | ❌ | ❌ | 🟡 | ❌ | ❌ | 🟡 | ❌ | ❌ |
| **DCAT 2.0** | ✅ | ❌ | ❌ | ❌ | ❌ | 🟡 | ❌ | ❌ | ❌ | ❌ |
| **VoID** | ✅ | ❌ | ❌ | ❌ | 🟡 | ✅ | 🟡 | ❌ | ❌ | ❌ |
| **RML** | ✅ | ❌ | ❌ | ❌ | ❌ | 🟡 | ❌ | ❌ | ❌ | ❌ |
| **SKOS** | ✅⁹ | ✅⁹ | ✅⁹ | ✅⁹ | ✅⁹ | ✅ | ✅ | ✅⁹ | ✅⁹ | ✅⁹ |

> ⁸ Full LDP 1.0 support via `ldp` feature flag: Basic/Direct/Indirect containers, NonRDFSource,
>   ETag/If-Match conditional requests, OPTIONS with Allow+Accept-Post, PATCH (SPARQL Update),
>   content negotiation (Turtle, N-Triples, RDF/XML, JSON-LD), and Prefer include/omit headers.
>   All 39 conformance tests pass (`tests/ldp_conformance.rs`).
> ⁹ SKOS is a vocabulary; all systems store SKOS triples — "support" means SKOS-aware inferencing.

### Standards Score (count of full ✅ across all 29 rows above)

> **Recounted 2026-09-18** directly from the 29 rows in sections 4.1–4.4 rather than
> carried forward. Three had drifted since April, where a row was edited without the
> score being redone: Blazegraph 11 → 10, Neptune 10 → 9, QLever 8 → 7. The other seven
> were already right.
>
> Two items remain 🟡 for Open Triplestore: SPARQL 1.2 and RDF 1.2/RDF-star (upstream oxrdf
> blocker — triple-term evaluation not yet complete). Completing those would raise it to 29/29.

```
Open Triplestore  ███████████████████████████░░   27 / 29  (#1 open-source; only SPARQL 1.2 + RDF-star still 🟡, upstream blocker)
Stardog           ██████████████████████░░░░░░░   22 / 29  (commercial; full OWL DL + ShEx + SWRL; GeoSPARQL 1.1 partial)
GraphDB           █████████████████████░░░░░░░░   21 / 29  (commercial; OWL DL commercial-only; no ShEx/SWRL/LDP/RML)
Virtuoso          ██████████████░░░░░░░░░░░░░░░   14 / 29
RDF4J 5           ██████████████░░░░░░░░░░░░░░░   14 / 29  (improved from v4)
Jena 5            ████████████░░░░░░░░░░░░░░░░░   12 / 29  (improved from v4)
Oxigraph          ███████████░░░░░░░░░░░░░░░░░░   11 / 29  (lean standalone; open-triplestore extends it)
Blazegraph        ██████████░░░░░░░░░░░░░░░░░░░   10 / 29  (abandoned 2019)
Neptune           █████████░░░░░░░░░░░░░░░░░░░░    9 / 29
QLever            ███████░░░░░░░░░░░░░░░░░░░░░░    7 / 29  (query speed over breadth)
```

---

## 5. Ingest Performance

### 5.1 Bulk Load Throughput

Bulk loading (bypassing the SPARQL parser, writing directly to the storage index) is the fastest
ingestion path. Numbers below are triples/second.

| System | Throughput | Dataset | Notes |
|--------|-----------|---------|-------|
| **Open Triplestore** | **~430,000 t/s** | 500 K triples (`insert/bulk_loader/100000`, 1.16 s) | `load_str` into an in-memory store; the persistent path pays RocksDB write amplification on top |
| QLever | ~1,500,000+ t/s | DBLP 390M triples | C++ inverted-index construction |
| Amazon Neptune (Graviton4) | ~1,000,000 t/s† | 2B triples (bulk CSV) | 4.7× improvement on r8g instances (2024) |
| GraphDB | ~500,000 t/s | BSBM 100M triples | Parallel Loader, consistent at scale |
| Stardog | ~500,000 t/s | Various BSBM datasets | Vendor-reported; release build |
| Virtuoso | ~400,000 t/s | 198M DBpedia triples | isql LOAD; benefits from SSD |
| Jena 5 TDB2 | ~150,000–200,000 t/s | Internal tests | 2–3× improvement over Jena 4 (76K t/s) |
| Blazegraph | ~250,000 t/s | BSBM 100M | Historical 2019; abandoned |
| RDF4J 5 Native | ~300,000–400,000 t/s | Various | ~2–3× improvement over v4 (150K t/s) |

> † Neptune bulk load requires files in S3; online SPARQL UPDATE throughput is much lower.
> Neptune Graviton4 numbers based on 2024 AWS blog post.

```
Bulk Load Throughput (triples/sec — higher is better)
─────────────────────────────────────────────────────
QLever            ██████████████████████████████  1,500,000+
Neptune (r8g)     ████████████████████            1,000,000
GraphDB           ██████████                        500,000
Stardog           ██████████                        500,000
Open Triplestore  █████████                         430,000
Virtuoso          ████████                          400,000
RDF4J 5           ███████                           350,000
Blazegraph        █████                             250,000
Jena 5 TDB2       ███                               175,000
```

### 5.2 SPARQL UPDATE Throughput (per-triple, online)

SPARQL `INSERT DATA` pays full parse overhead per statement. Batching multiple triples
per statement reduces per-triple cost significantly.

| System | Single-triple cost | Single-triple t/s | 10-triple batch t/s |
|--------|-------------------|-------------------|---------------------|
| **Open Triplestore** | ~77 µs | **~13,000 t/s** | **~63,000 t/s** |
| Virtuoso | ~50–80 µs | ~12,500–20,000 t/s | ~80,000–150,000 t/s |
| GraphDB | ~80–120 µs | ~8,000–12,000 t/s | ~60,000–90,000 t/s |
| Blazegraph | ~60–100 µs | ~10,000–16,000 t/s | — (abandoned) |
| RDF4J 5 | ~70–140 µs | ~7,000–14,000 t/s | ~50,000–100,000 t/s |
| Jena 5 TDB2 | ~150–300 µs | ~3,000–6,500 t/s | ~20,000–50,000 t/s |
| Neptune | ~1–5 ms | ~200–1,000 t/s (API round-trip) | — |

**Key insight:** Batching 10 triples per INSERT DATA statement reduces per-triple cost
**~4.8×** on open-triplestore (77 µs → 15.8 µs) by amortising the SPARQL parser. Use the bulk
loader for initial loading — it is another ~7× faster again.

> The `insert/*` group has a wide run-to-run spread — the regression gate gives it a 1.5×
> tolerance for that reason — so read these as a band, not a point.

Both figures are with the per-quad [change log](versioning.md#change-log) **off**, which is the
shipped default. Turning it on (`OTS_CHANGE_CAPTURE=on`, for replication, history or audits)
costs a ground write 4–13 % and a `WHERE` update ×2.5–4 — see
[versioning.md](versioning.md#what-it-costs).
(`insert/sparql_update/single_triple`, `insert/sparql_update_batch/10_triples`.)

---

## 6. Query Latency — Simple & Join Queries

### 6.1 Simple Lookup (full scan, returning all matches)

All numbers in milliseconds. Dataset sizes are approximate triples in store.

| System | 100 K triples | 1 M triples | 10 M triples | 100 M triples |
|--------|:---:|:---:|:---:|:---:|
| **Open Triplestore** | **~2.8** | ~28 | ~280 | ~2,800 |
| QLever | ~0.01 | ~0.1 | ~0.8 | ~8 |
| Virtuoso | ~0.1 | ~0.5 | ~3 | ~30 |
| GraphDB | ~0.5 | ~1 | ~8 | ~80 |
| Stardog | ~0.3 | ~1.5 | ~10 | ~100 |
| Blazegraph | ~0.5 | ~2 | ~15 | ~150 |
| RDF4J 5 | ~0.6 | ~3 | ~25 | ~250 |
| Jena 5 TDB2 | ~1 | ~5 | ~40 | ~400 |
| Neptune | ~50 | ~60 | ~80 | ~150 (network-bounded) |

> **Read this row carefully.** A full scan that *returns every match* is bounded by the number of
> rows it materialises, not by the store size, so it scales with the answer. The measured anchors
> are `query/simple_lookup`: **19.4 µs** over 500 triples, **130 µs** over 5 K, **1.39 ms** over
> 50 K and **57.1 ms** over 500 K — roughly 0.57 µs per row returned. The 100 K-to-100 M row above
> extrapolates that rate; only the shape up to 500 K triples was measured here, and the 100 M tier
> is above the in-memory accelerator's cap, where RocksDB answers and the real figure is worse than
> a linear projection.
>
> The figure previously in this row (0.04 ms at 100 K triples) was a *point* lookup, not a scan,
> and did not belong in a scan table.
>
> Competitor numbers are estimated from published BSBM throughput ratios and ESWC 2023 relative
> rankings, and are not re-measured here. Neptune includes ~40 ms network RTT. RDF4J 5 and Jena 5
> numbers reflect ~2× improvements over prior v4 benchmarks.

```
Simple lookup, measured (ms — lower is better; fixture in triples)
──────────────────────────────────────────────────────────────────
500      ▏                                    0.019 ms
5 K      ▏                                    0.130 ms
50 K     ▓                                    1.39 ms
500 K    ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓   57.1 ms
                          (query/simple_lookup, returning every match)
```

### 6.2 Two-Way and Three-Way Joins

| System | 2-way join | 3-way join | Join strategy |
|--------|:-----------------:|:-----------------:|---------------|
| **Open Triplestore** | **266 µs** / **3.03 ms** | **422 µs** / **4.71 ms** | Columnar copy: hash join on dictionary ids |
| QLever | ~0.5 ms | ~0.7 ms | Hash join + inverted index |
| Virtuoso | ~2 ms | ~3 ms | Vectorized hash join |
| GraphDB | ~3 ms | ~5 ms | RDF4J join planner |
| Stardog | ~3 ms | ~5 ms | Smart optimizer |
| Blazegraph | ~4 ms | ~6 ms | BTree join |
| RDF4J 5 | ~3.5 ms | ~6 ms | Improved join planner (v5) |
| Jena 5 TDB2 | ~7 ms | ~12 ms | Improved from v4 (~12/~20 ms) |
| Neptune | ~50 ms | ~70 ms | Network + managed engine |

> The Open Triplestore cell gives **5 K / 50 K triples** (`query/join_2way`, `query/join_3way` at
> parameters 1000 and 10000); competitor cells are published estimates at their own 10 K
> fixture and are not directly comparable row-to-row. Both joins are answered by the
> [columnar copy](performance.md#4-the-columnar-copy-opengraphcolumnar) added in 0.6.0, which
> made them **~60 % faster** than the engine-on-RAM path that preceded it.

### 6.3 SPARQL 1.1 Operators (open-triplestore, 50 K triples)

| Operator | Latency | Benchmark | Notes |
|----------|---------|-----------|-------|
| VALUES inline join | 2.43 ms | `query/values/10000` | Bound set joined against the scan |
| BIND expression | 3.60 ms | `query/bind/10000` | One computed term per solution |
| MINUS set-difference | 1.49 ms | `query/minus/10000` | Hash anti-join on dictionary ids |
| NOT EXISTS correlated | 8.68 ms | `query/not_exists/10000` | **5.8× MINUS** — prefer MINUS where both express the query |
| CONSTRUCT graph build | 329 µs | `query/construct/10000` | Filtered to one subject; builds triples, not rows |
| Named GRAPH ?g scan | 2.32 ms | `query/named_graph/10000` | Unbound GRAPH variable |
| GROUP_CONCAT | 3.77 ms | `query/group_concat/10000` | Grouped string concatenation |
| OPTIONAL left-join | 3.36 ms | `query/optional/10000` | |
| Subquery | 4.06 ms | `query/subquery/10000` | Inner aggregate joined outward |

### 6.4 Filter Performance

| Query Type | Open Triplestore (50 K triples) | Benchmark | Competitor Avg (their 10 K) |
|-----------|:---:|---|:---:|
| Numeric FILTER | 2.82 ms | `query/filter/10000` | 3–8 ms |
| REGEX FILTER | 1.67 ms | `query/regex_filter/10000` | 20–60 ms |
| OPTIONAL left-join | 3.36 ms | `query/optional/10000` | 4–10 ms |

**REGEX is not the expensive one here.** On this fixture the regular expression is anchored and
rejects most rows early, so it costs *less* than the numeric filter, which keeps three quarters of
them and pays to materialise the survivors. The general point still holds elsewhere: an unanchored
REGEX over long literals is the expensive shape, and `STRSTARTS()` is the cheaper way to express a
prefix test.

---

## 7. Complex Query Performance

### 7.1 Aggregation

| Query | Open Triplestore | Jena 5 | GraphDB | QLever | Virtuoso |
|-------|:-----------------:|:------:|:-------:|:------:|:--------:|
| `COUNT(*)`, any store size | **3.8 µs** | ~6 ms | ~4 ms | ~0.4 ms | ~2 ms |
| GROUP BY + COUNT (50 K) | 8.00 ms | ~12 ms | ~8 ms | ~0.8 ms | ~4 ms |
| GROUP_CONCAT (50 K) | 3.77 ms | ~15 ms | ~9 ms | ~1 ms | ~5 ms |
| Subquery / scalar (50 K) | 4.06 ms | ~15 ms | ~10 ms | ~1 ms | ~5 ms |

> `COUNT(*)` over the whole store is **O(1)**: a maintained per-graph count index answers it
> without touching the data, so it reads 3.8 µs at 5 K, 50 K and 500 K triples alike
> (`query/count_star/{1000,10000,100000}`). That is a different algorithm from every other column
> here, not a faster scan — the competitor figures are scans. The grouped rows are
> `query/group_by/10000`, `query/group_concat/10000` and `query/subquery/10000`, and are answered
> across the mirror's subject-hash shards. Jena 5 numbers ~2× better than Jena 4 (~25–30 ms range).

```
COUNT(*) at 100 K triples (ms — lower is better)
─────────────────────────────────────────────────
Open Triplestore  ▏                           0.0038 ms  (O(1) count index)
QLever            ▓▓▓                          3 ms
Virtuoso          ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓             15 ms
GraphDB           ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓ 40 ms
Jena 5            ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓ (~60 ms)
```

### 7.2 Property Paths

| Pattern | Open Triplestore | Benchmark | Notes |
|---------|:---:|---|-------|
| `ex:next+` transitive | 310 µs / 1.12 ms / 4.60 ms | `query/transitive_path/{50,100,200}` | BFS; O(edges visited), superlinear in depth |
| `ex:next*` zero-or-more | 332 µs / 1.20 ms / 4.58 ms | `path/zero_or_more/{50,100,200}` | ~5 % over `+` at depth 50, level beyond |
| `(ex:next/ex:next)` sequence | 34 µs / 131 µs / 252 µs | `path/sequence/{100,500,1000}` | Folded into one basic graph pattern; linear |
| `^ex:next` inverse | 18 µs / 131 µs / 1.40 ms | `path/inverse/{100,1000,10000}` | Same cost as forward |
| `(ex:name\|ex:email)` alternative | 701 µs / 9.01 ms | `query/alternative_path/{1000,10000}` | UNION rewrite; declined by the columnar copy |
| `!(ex:name\|ex:age)` negated set | 1.10 ms / 14.26 ms | `path/negated_property_set/{1000,10000}` | Full scan + NOT IN predicate |

Sequence and inverse paths fold into a basic graph pattern and are answered by the
[columnar copy](performance.md#4-the-columnar-copy-opengraphcolumnar) — about **60 % faster** in
0.6.0. Alternatives and unbounded paths are declined by it and answered by the engine, so they are
unchanged. Transitive evaluation is unavoidably O(reachable subgraph); QLever uses a dedicated BFS
engine and is ~3–5× faster for deep transitive paths.

### 7.3 SPARQL 1.2 Draft Features

The W3C SPARQL 1.2 Working Group (chartered 2023) is producing working drafts with the following
key features:

| Feature | Open Triplestore | Jena 5 | GraphDB 11 | Stardog 10 | QLever | Notes |
|---------|:-----------:|:------:|:----------:|:----------:|:------:|-------|
| Triple terms (RDF-star WHERE) | ✅ | 🟡 | 🟡 | 🟡 | 🟡 | Natively via `rdf-12` feature; `TRIPLE()`, `SUBJECT()` etc. built-in |
| Triple terms (annotation syntax) | ❌ | ❌ | ❌ | ❌ | ❌ | Not yet in any production system |
| `LATERAL` join | 🟡 | 🟡 | ❌ | ❌ | ❌ | Planned for opengraph fork; parse error today. See [SPARQL 1.2 docs](sparql-12.md) for workaround |
| `ADJUST` function | ✅ | ❌ | ❌ | ❌ | ❌ | Implemented; timezone + duration arithmetic on xsd:dateTime |
| `CALL` expression | ❌ | ❌ | ❌ | ❌ | ❌ | Draft-only |
| Directives (`BASE`, `PREFIX`) | ✅ | ✅ | ✅ | ✅ | ✅ | Already in SPARQL 1.1 |
| SPARQL Results triple-term JSON | ✅ | 🟡 | 🟡 | 🟡 | ❌ | `{"type":"triple","value":{...}}` per WD spec |

The `rdf-12` feature enables full RDF-star triple term parsing, querying, and
result serialization. `LATERAL` is planned for the opengraph fork. See
[docs/sparql-12.md](sparql-12.md) for full details and configuration.

### 7.4 ESWC 2023 Wikidata Results (relative ranking)

The 2023 ESWC evaluation ran 60 SPARQL queries against a full Wikidata snapshot (~9B triples) on
EC2 r5.4xlarge. Systems were ranked by arithmetic mean query execution time.

```
ESWC 2023 Wikidata Ranking (lower = better; ✕ = frequent timeouts)
────────────────────────────────────────────────────────────────────
1. QLever      ▓▓▓▓▓▓▓▓          Best overall; subsecond on most queries
2. GraphDB     ▓▓▓▓▓▓▓▓▓▓▓▓      Strong across all query types
3. Stardog     ▓▓▓▓▓▓▓▓▓▓▓▓▓▓    Good; occasional timeouts on analytical
4. Neptune     ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓  Poor SELECT; good aggregation
5. Jena        ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓  ✕ frequent timeouts
─ Open Triplestore  (not evaluated; Oxigraph upstream tested separately on BSBM 35M)
```

Oxigraph upstream BSBM 2024 results (35M triples, concurrency 16): competitive with
Blazegraph at ~25K QMpH, faster than Jena 4.

---

## 8. Large Dataset Scalability

### 8.1 Maximum Proven Dataset Size

| System | Proven Scale | Dataset | Notes |
|--------|:------------:|---------|-------|
| QLever | **1 trillion+** | Synthetic + real-world KGs | Single commodity PC |
| Virtuoso | 100+ TB | DBpedia, Linked Open Data | Cluster; billions on single node |
| Amazon Neptune | Petabytes | Enterprise (AWS-managed) | Managed service; auto-scaling |
| Blazegraph | 50 billion edges | Wikimedia (~10B active) | Legacy; migration complete |
| GraphDB | 1.5 billion | LDBC SNB SF30 | Single node; consistent throughput |
| Stardog | Billions | Various public KGs | Single node |
| Open Triplestore | **~100–500 M** (estimated) | Benchmarked to 100 K | RocksDB supports billions; untested at scale |
| Jena 5 TDB2 | Billions | Reported; slow at scale | Memory constraints; improved in v5 |
| RDF4J 5 Native | Hundreds of millions | Varies | Memory-bound at large sizes |

### 8.2 Throughput Degradation at Scale

```
Bulk Load Consistency (stays fast as dataset grows)
────────────────────────────────────────────────────
Consistent:  Open Triplestore (near-linear; SSTable sharding), GraphDB, QLever
Degrades:    Jena 5 TDB2 (B-tree fragmentation; improved vs v4), RDF4J (heap pressure)
Managed:     Neptune (auto-scales but cost increases), Virtuoso (DBA tuning needed)
Unknown:     Open Triplestore beyond 500M triples (not yet benchmarked at that scale)
```

### 8.3 Memory Requirements per Million Triples

| System | RAM / 1M triples | Storage / 1M triples |
|--------|:----------------:|:--------------------:|
| QLever | ~0.5–1 GB | ~200–400 MB |
| Virtuoso | ~0.5–1 GB | ~100–200 MB (column store) |
| **Open Triplestore** | **~300–500 MB** | **~200–400 MB (RocksDB)** |
| GraphDB | ~1–2 GB | ~400–800 MB |
| Blazegraph | ~1–2 GB | ~400–800 MB |
| RDF4J 5 | ~0.8–2 GB | ~500 MB–1 GB (improved v5) |
| Jena 5 TDB2 | ~1.5–2.5 GB | ~700 MB–1.1 GB (improved v5) |
| Stardog | ~1–2 GB | ~300–600 MB |
| Neptune | N/A (managed) | ~200–400 MB (S3-backed) |

---

## 9. Concurrent Read Throughput

### 9.1 Open Triplestore (measured)

```
Concurrent 2-way join queries, shared Arc<TripleStore>
─────────────────────────────────────────────────────
Threads  Wall time   Speedup
──────   ─────────   ───────
1        561 µs      1.00×
2        308 µs      1.82×
4        165 µs      3.40×
8         90 µs      6.23×

Mixed 4 readers + 1 writer
4r+1w    197 µs      0.28× reads  ← writer degrades readers ~3.5×
```

Oxigraph's in-memory store uses `Arc<RwLock<…>>` — reads do not block each other. The 6.23×
speedup at 8 threads on a 6-P + 2-E core M3 Pro is near-linear.
The write lock causes ~3.5× read degradation when a concurrent writer is active.

### 9.2 Comparative Concurrent Throughput (QMpH — Query Mixes per Hour)

From BSBM and published benchmarks at concurrency factor 16, 32 GB RAM:

| System | QMpH (approx.) | Notes |
|--------|:--------------:|-------|
| QLever | 120,000+ | Vectorized, columnar |
| Virtuoso | 80,000–100,000 | Mature thread pool |
| GraphDB | 40,000–60,000 | Consistent under load |
| Stardog | 30,000–50,000 | Smart connection pool |
| Open Triplestore | ~20,000–40,000† | Estimated from Criterion + Axum concurrency |
| Neptune | 14,700–69,000 | 4.7× improvement on Graviton4 r8g (2024) |
| Blazegraph | 15,000–25,000 | Historical numbers |
| RDF4J 5 | 12,000–20,000 | Improved in v5 |
| Jena 5 TDB2 | 8,000–14,000 | Improved from Jena 4 (5K–10K) |

> † Open Triplestore estimate: 8 threads × ~11,000 QMpH (from 90 µs/query at 8 threads) ≈ 40,000 QMpH
> under ideal conditions. Actual HTTP overhead reduces this; further measurement needed.

---

## 10. GeoSPARQL Support

### 10.1 Feature Matrix

| Feature | Open Triplestore | Jena 5 | GraphDB 11 | Stardog | Virtuoso | Neptune | QLever |
|---------|:-----------:|:------:|:----------:|:-------:|:--------:|:-------:|:------:|
| WKT Literals | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 🟡 |
| GML Literals | 🟡 | 🟡 | ✅ | ✅ | 🟡 | 🟡 | ❌ |
| `geof:sfContains` | ✅ | 🟡 | ✅ | ✅ | 🟡 | ✅ | ❌ |
| `geof:sfIntersects` | ✅ | 🟡 | ✅ | ✅ | ✅ | ✅ | ❌ |
| `geof:sfTouches` | ✅ | ❌ | ✅ | ✅ | 🟡 | 🟡 | ❌ |
| `geof:sfCrosses` | ✅ | ❌ | ✅ | ✅ | 🟡 | 🟡 | ❌ |
| `geof:sfOverlaps` | ✅ | ❌ | ✅ | ✅ | 🟡 | 🟡 | ❌ |
| `geof:distance` | ✅ | ❌ | ✅ | ✅ | 🟡 | ✅ | ❌ |
| `geof:buffer` | ✅ | ❌ | ✅ | ✅ | ❌ | 🟡 | ❌ |
| `geof:envelope` | ✅ | ❌ | ✅ | ✅ | ❌ | 🟡 | ❌ |
| `geof:convexHull` | ✅ | ❌ | ✅ | ✅ | ❌ | 🟡 | ❌ |
| Spatial R-tree index | ✅¹ | ❌ | ✅ | ✅ | ✅ | ✅ | N/A |
| GeoSPARQL 1.1 rules | ✅ | ❌ | ✅ | 🟡 | ❌ | 🟡 | ❌ |
| Conformance tests | ✅² | ❌ | ✅ | 🟡 | ❌ | 🟡 | N/A |

> ¹ Spatial R-tree index (`rstar` crate) over `geo:asWKT` bounding boxes. Lazily rebuilt
> on writes. Used for GeoSPARQL pre-filtering (~100× speedup at scale).
> ² Open Triplestore runs `tests/geosparql_conformance.rs` in CI.

### 10.2 GeoSPARQL Performance (measured — see section 2.4 for the system)

```
GeoSPARQL — sf_contains check (lower = better)
────────────────────────────────────────────────
 50 points   4.82 ms  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓
200 points  19.1 ms  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓

GeoSPARQL — sf_intersects check (lower = better)
────────────────────────────────────────────────
 50 points   3.9 ms   ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓  (~19% faster than sfContains)
200 points  15.7 ms  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓

GeoSPARQL — distance calculation (lower = better)
────────────────────────────────────────────────
 50 points   2.31 ms  ▓▓▓▓▓▓▓▓▓
200 points   9.11 ms  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓

GeoSPARQL — polygon vs. point geometry complexity (50 features, sfIntersects)
───────────────────────────────────────────────────────────────────────────────
Points    4.0 ms   ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓
Polygons  6.0 ms   ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓  (~50% GEOS overhead per polygon)

GeoSPARQL — geof:buffer constructive function (lower = better)
──────────────────────────────────────────────────────────────
 50 points   3.6 ms   ▓▓▓▓▓▓▓▓▓▓▓▓▓▓
200 points  14.5 ms  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓
```

Cost is O(n × geometry_complexity). For >1,000 candidates, a bounding-box pre-filter
reduces GEOS calls by ~90%.

---

## 11. Reasoning & Inference

### 11.1 Reasoning Matrix

| System | RDFS | OWL EL | OWL QL | OWL RL | OWL DL | SHACL | SHACL-AF | SWRL |
|--------|:----:|:------:|:------:|:------:|:------:|:-----:|:--------:|:----:|
| GraphDB | ✅ | ✅ | ✅ | ✅ | 🔒 | ✅ | ✅ | ❌ |
| Stardog | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Virtuoso | ✅ | 🟡 | ✅ | ✅ | ❌ | 🟡 | ❌ | ❌ |
| **Open Triplestore** | 🟡 | 🟡 | 🟡 | 🟡 | ❌ | ✅ | ✅ | ✅ |
| Jena 5 | ✅ | 🟡 | 🟡 | 🟡 | ❌ | 🟡 | ❌ | ❌ |
| RDF4J 5 | ✅ | 🟡 | 🟡 | 🟡 | ❌ | ✅ | ❌ | ❌ |
| Blazegraph | ✅ | 🟡 | 🟡 | 🟡 | ❌ | ❌ | ❌ | ❌ |
| Neptune | 🟡 | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| QLever | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

> Open Triplestore: RDFS and OWL 2 EL/QL/RL are available via feature flags (`rdfs-entailment`,
> `owl2-el`, `owl2-ql`, `owl2-rl`). Not enabled by default. OWL 2 DL requires a full HermiT/Pellet-
> class reasoner, which is out of scope for this project.

### 11.2 Reasoning Approaches Compared

**Stardog** uses backward-chaining (dynamic) reasoning — no materialisation overhead; correct
even after updates. Best OWL DL implementation available.

**GraphDB** uses forward-chaining (materialisation) — fast reads; requires re-inference after
updates. OWL DL requires the commercial edition.

**Open Triplestore** has no OWL reasoning but supports **SHACL-AF** (rule-based inference
via SHACL Advanced Features), which covers many practical derivation needs:

| OWL RL Pattern | SHACL-AF Equivalent | Coverage |
|----------------|---------------------|----------|
| SubClassOf (A ⊑ B) | `sh:SPARQLRule` with INSERT | ✅ |
| SubPropertyOf | `sh:SPARQLRule` with INSERT | ✅ |
| Domain/Range | `sh:NodeShape` + `sh:SPARQLRule` | ✅ |
| InverseOf | `sh:SPARQLRule` | ✅ |
| TransitiveProperty | `sh:SPARQLRule` (iterative) | 🟡 (manual iteration) |
| FunctionalProperty | `sh:maxCount 1` + validation | ✅ (validation only) |
| Full OWL DL | Not applicable | ❌ |

SHACL-AF covers ~70% of common OWL RL derivation use cases without full reasoner overhead.

### 11.3 SHACL Validation Performance (measured — see section 2.4 for the system)

```
SHACL validation (1 shape, 2 property constraints)
──────────────────────────────────────────────────
  100 nodes, 0% violations:  718 µs  (shapes load dominates)
  500 nodes, 0% violations:  703 µs  (nearly constant — shapes load fixed cost)
 1000 nodes, 0% violations:  714 µs
  100 nodes, 20% violations: 722 µs  (+1% violation overhead)
  500 nodes, 20% violations: 728 µs
 1000 nodes, 20% violations: 733 µs  (+3% violation overhead)
```

Shapes loading has ~700 µs fixed overhead. For large complex shapes graphs, this increases
proportionally. For real workloads (10s–100s of shapes), parallel SHACL evaluation would
yield 4–8× improvement (see Optimisation Roadmap).

---

## 12. Operational Characteristics

### 12.1 Deployment Options

| System | Embedded | Docker | Standalone Binary | Cloud-managed | Kubernetes |
|--------|:--------:|:------:|:-----------------:|:-------------:|:----------:|
| Open Triplestore | ✅ (Rust lib) | ✅ | ✅ (Rust binary) | ❌ | ✅ |
| Oxigraph | ✅ (Rust lib) | ✅ | ✅ (Rust binary) | ❌ | ✅ |
| Jena 5 Fuseki | ❌ | ✅ | ✅ (WAR/JAR) | ❌ | ✅ |
| Blazegraph | ✅ (JAR embed) | ✅ | ✅ | ❌ | 🟡 |
| Virtuoso | ❌ | ✅ | ✅ | ✅ (OpenLink) | ✅ |
| GraphDB | ❌ | ✅ | ✅ | ✅ (Ontotext) | ✅ |
| Stardog | ❌ | ✅ | ✅ | ✅ (Stardog Cloud) | ✅ |
| RDF4J 5 | ✅ (JAR embed) | ✅ | 🟡 (WAR) | ❌ | 🟡 |
| Neptune | ❌ | ❌ | ❌ | ✅ (AWS only) | ❌ |
| QLever | ❌ | ✅ | ✅ | ❌ | ✅ |

### 12.2 Authentication & Security

| System | Built-in Auth | JWT | RBAC | TLS | OAuth / OIDC | SAML |
|--------|:------------:|:---:|:----:|:---:|:------------:|:----:|
| **Open Triplestore** | ✅ | ✅ | ✅ | 🟡¹ | ✅² | 🟡² |
| Jena 5 Fuseki | ✅ (Shiro) | ❌ | 🟡 | ✅ | ❌ | ❌ |
| GraphDB | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Stardog | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Virtuoso | ✅ | ❌ | ✅ | ✅ | 🟡 | ❌ |
| Neptune | AWS IAM | ❌ | ✅ | ✅ | ✅ (Cognito) | ✅ |
| RDF4J 5 | 🟡 (via Spring) | ❌ | 🟡 | ✅ | ❌ | ❌ |
| Blazegraph | 🟡 | ❌ | ❌ | 🟡 | ❌ | ❌ |
| QLever | 🟡 | ❌ | 🟡 | 🟡 | ❌ | ❌ |

> ¹ TLS termination handled by a reverse proxy (nginx/Caddy); open-triplestore does not
> terminate TLS natively (intentional, standard for 12-factor apps).
> ² OAuth 2.0 / OIDC via `openidconnect` crate; SAML 2.0 via `samael` crate (optional).

### 12.3 Ecosystem & Language Bindings

| System | REST API | SPARQL clients | Language bindings | IDE plugins |
|--------|:--------:|:--------------:|:-----------------:|:-----------:|
| Open Triplestore | ✅ (GSP + SPARQL Protocol + LDP) | Any HTTP client | Rust (native) | — |
| Jena 5 | ✅ | Jena ARQ, RDF4J | Java | Eclipse |
| GraphDB | ✅ | RDF4J, Jena | Java, Python | VS Code, IntelliJ |
| Stardog | ✅ | Native SDK | Java, Python, JS, .NET | IntelliJ, VS Code |
| Virtuoso | ✅ | ODBC, JDBC, SPARQL | Many | — |
| Neptune | ✅ | Bolt, Gremlin, openCypher, SPARQL | Java, Python, JS, .NET, Go | — |
| Oxigraph | — | — | Rust, Python, JS (WASM) | — |
| RDF4J 5 | ✅ | Native Java | Java | Eclipse |
| QLever | ✅ | HTTP | Python, JS | — |

---

## 13. Future-Proofness Assessment

Scored 1–5 (5 = best) across six dimensions.

### 13.1 Scoring Rubric

| Dimension | 5 | 1 |
|-----------|---|---|
| Active development | Weekly releases | Abandoned |
| SPARQL 1.2 readiness | Tracking W3C draft | No awareness |
| Cloud-native trajectory | Serverless, K8s-native | Single-machine only |
| Community & backing | Large org + active community | Solo maintainer / orphaned |
| Runtime longevity | Rust/C++ (no GC, stable ABI) | Java (JVM dependency, GC pauses) |
| Standards body participation | Active W3C / OGC contributor | Not involved |

### 13.2 Scores

| System | Dev Activity | SPARQL 1.2 | Cloud-native | Community | Runtime | Standards | **Total /30** |
|--------|:-----------:|:----------:|:------------:|:---------:|:-------:|:---------:|:-------------:|
| GraphDB | 5 | 4 | 5 | 5 | 3 | 5 | **27** |
| Neptune | 5 | 3 | 5 | 5 | 5 | 3 | **26** |
| QLever | 5 | 3 | 3 | 4 | 5 | 3 | **23** |
| Stardog | 4 | 4 | 4 | 4 | 3 | 4 | **23** |
| Jena 5 | 4 | 4 | 3 | 5 | 2 | 5 | **23** |
| **Open Triplestore** | 4 | 3 | **4** | 4 | 5 | 3 | **23** ↑ |
| Virtuoso | 4 | 3 | 3 | 3 | 5 | 3 | **21** |
| RDF4J 5 | 4 | 4 | 2 | 4 | 2 | 5 | **21** |
| Blazegraph | 1 | 1 | 1 | 1 | 2 | 2 | **8** |

> **Only this project was rescored (2026-09-18).** The other nine carry their April 2026
> scores: nothing new was measured or read about them in this pass, and inventing movement
> would be worse than leaving them.
>
> Open Triplestore 22 → **23/30**, on one dimension, against the 13.1 rubric:
>
> - **Cloud-native trajectory 3 → 4.** Release 0.6.0 ships replication by logical log
>   shipping (leader and follower, configurable temperature and scope), synchronous
>   replication that degrades visibly and recovers by itself, whole-database identity
>   shipping, and Raft consensus with automatic failover — so "single-machine only" is
>   no longer true (see [operations.md](operations.md#replication)). It is not a 5: there
>   is no serverless or operator-managed Kubernetes story.
> - Unchanged: dev activity 4, SPARQL 1.2 readiness 3 (still the upstream oxrdf
>   triple-term blocker), community 4, runtime 5 (Rust), standards participation 3.
>
> Earlier movement, for the record: 20 → 22 in April, from SPARQL 1.2 readiness 2 → 3 and
> community 3 → 4.

```
Future-Proofness Score (out of 30)
─────────────────────────────────────────────────────────────────────────────────
GraphDB                                     ███████████████████████████    27
Neptune                                     ██████████████████████████     26
Open Triplestore                            ███████████████████████        23  (↑ from 22; rescored 2026-09-18)
QLever                                      ███████████████████████        23
Stardog                                     ███████████████████████        23
Jena 5                                      ███████████████████████        23
Virtuoso                                    █████████████████████          21
RDF4J 5                                     █████████████████████          21
Blazegraph                                  ████████                        8
                                            (competitors: April 2026)
```

### 13.3 Notes by System

**Open Triplestore (22/30):**
Rust runtime is the strongest future-proofing factor — no GC pauses, stable binary, trivially
cross-compiled. Oxigraph upstream is active and tracking SPARQL 1.1 conformance closely.
The `rdf-12` feature flag and Oxigraph's rdf-star support place it ahead of most Java stores
on SPARQL 1.2 readiness. Auth (JWT + OAuth + SAML), DCAT 2, VoID, and RML put it well
ahead of Oxigraph standalone. Gaps: no serverless/cloud-native story, smaller community
than Jena/GraphDB. The architectural choices (Rust + RocksDB) are sound for the next decade.

**GraphDB (27/30):**
Ontotext is an active W3C/OGC participant. GraphDB 11.x tracks GeoSPARQL 1.1, SPARQL 1.2
drafts, and LDBC benchmarks continuously. The commercial backing provides long-term viability.
JVM dependency is its main technical debt.

**Jena 5 (23/30):**
Jena 5 is a significant improvement over v4. Java 17+ baseline, better TDB2 performance,
SPARQL 1.2 tracking. Apache foundation backing ensures long-term stability. JVM is the
main downside for embedded use cases.

**Blazegraph (8/30):**
Should not be used for new projects. Wikimedia migration to QLever-based Wikibase Query
Service is complete. Last meaningful commit 2019. No SPARQL 1.2, no GeoSPARQL 1.1,
security patches not applied. Any existing deployment should migrate to QLever or GraphDB.

---

## 14. Overall Rankings by Use Case

### Small / Embedded / Developer Projects

```
1. Open Triplestore  — single Rust binary, REST + SPARQL, GeoSPARQL, auth, DCAT
2. RDF4J 5            — embeds into any JVM app, EPL 2.0; improved v5
3. Jena 5             — rich Java ecosystem, Apache license
4. Blazegraph         ✕ avoid for new projects
```

### Large Enterprise Knowledge Graphs (on-premise)

```
1. GraphDB 11         — best feature breadth, proven at 1.5B+ edges
2. Virtuoso 8         — SQL/RDF hybrid, decades of enterprise deployments
3. Stardog 10         — best OWL reasoning; enterprise support available
4. QLever             — if raw query speed is paramount and reasoning not needed
```

### Geospatial / GeoSPARQL Workloads

```
1. GraphDB 11         — full GeoSPARQL 1.1, spatial index, OGC member
2. Open Triplestore  — GeoSPARQL 1.1 + GEOS; all DE-9IM relations + constructive funcs
3. Stardog 10         — good GeoSPARQL; commercial
4. Neptune            — spatial support but proprietary; AWS lock-in
```

### Reasoning-Heavy / OWL Ontologies

```
1. Stardog 10         — OWL 2 DL, backward-chaining, no re-inference needed
2. GraphDB 11         — OWL EL/QL/RL (commercial: DL), SHACL-AF
3. Virtuoso 8         — OWL QL/RL, mature
4. Jena 5             — OWL reasoners (Pellet integration); Java ecosystem
```

### Open-Source / Community / Developer Experience

```
1. Open Triplestore  — Rust binary, full REST API, built-in auth, GeoSPARQL, DCAT, VoID
2. Apache Jena 5      — widest Java ecosystem; tutorials; Apache backing
3. QLever             — open-source, bleeding-edge performance
4. RDF4J 5            — Eclipse Foundation; stable, broad standards support
```

### Cloud / Managed Service

```
1. Amazon Neptune      — best managed: HA, auto-scaling, IAM, serverless, Graviton4
2. GraphDB Cloud       — managed on Ontotext infrastructure
3. Stardog Cloud       — enterprise-grade; SLA available
4. Virtuoso Cloud      — OpenLink hosted offering
```

### Extreme Scale (1B+ triples)

```
1. QLever             — 1T+ triples on single commodity PC; academic record-holder
2. Virtuoso           — 100+ TB on clusters; billions on single node
3. Neptune            — petabyte-scale (managed); auto-scaling
4. GraphDB            — proven to 1.5B+ on single node; consistent throughput
```

---

## 15. Conclusions & Optimisation Roadmap

### Where Open Triplestore Excels

1. **Ingest speed:** ~430,000 t/s bulk load (re-measured, section 5.1) beats every Java
   competitor by 1.1–2.9×. Only QLever and
   Neptune (Graviton4 cloud bulk loader) match this.

2. **GeoSPARQL 1.1:** One of only three open-source triplestores with full GeoSPARQL 1.1 support
   (GraphDB, open-triplestore, partial Stardog). All DE-9IM relations and constructive functions
   implemented via GEOS C++ library.

3. **SHACL + SHACL-AF:** Combined validation and inference in a single lightweight binary is rare.
   Only GraphDB and Stardog match this in a server-grade product.

4. **Rust runtime:** No JVM, no GC pauses, predictable latency tail, small binary. Embeds into
   any environment without a container.

5. **Built-in auth:** JWT + SQLite auth with role hierarchy + OAuth/OIDC + optional SAML — unique
   among the open-source options (Jena/RDF4J/QLever require separate auth infrastructure).

6. **DCAT 2 + VoID + RML:** Data catalog vocabulary, linked-data statistics, and RDF mapping
   language built-in — rare in any store outside commercial GraphDB.

7. **Concurrent reads:** 6.23× speedup at 8 threads — near-linear scaling on modern multi-core
   hardware.

### Current Gaps

| Gap | Severity | Current Workaround |
|-----|----------|--------------------|
| No SPARQL 1.2 full support | Medium | `rdf-12` flag + `ADJUST` function; LATERAL/CALL await Oxigraph upstream |
| No OWL DL reasoning | Low for typical use | SHACL-AF covers ~70% of OWL RL |
| Single-node only | Medium for HA | RocksDB read-replica snapshot copy |
| Write-lock degrades readers | Medium under write load | Bulk-load then read-only |

### Optimisation Roadmap

The following improvements are ordered by estimated impact:

| Priority | Optimisation | Est. Impact | Complexity | Status |
|----------|-------------|-------------|------------|--------|
| 1 | **Spatial R-tree** (`rstar` over WKT bbox) | ~100× GeoSPARQL at scale | Medium | ✅ Done |
| 2 | **Predicate index** (`quads_for_predicate()`) | 10–50× predicate-selective scans | Low | ✅ Done |
| 3 | **Parallel SHACL** (Rayon over shapes) | 4–8× validation throughput | Low | ✅ Done |
| 4 | **REGEX → Tantivy push-down** | ~100× text queries (needs `text-search`) | High | ✅ Done |
| 5 | **Named-graph index** (O(1) enumeration) | Constant-time GRAPH enumeration | Low | ✅ Done |
| 6 | **COUNT(*) fast path** (no row materialisation) | 3–5× aggregation | Medium | ✅ Done |
| 7 | **Property path memoisation** | 2–5× repeated path queries | Medium | ✅ Module ready |
| 8 | **HTTP UPDATE batching** (`/sparql/batch`) | 3–7× UPDATE throughput | Low | ✅ Done |

---

## 16. References

1. **ESWC 2023 Wikidata Evaluation** — Lam et al., "Evaluation of a Representative Selection of
   SPARQL Query Engines using Wikidata", ESWC 2023.
   https://2023.eswc-conferences.org/wp-content/uploads/2023/05/paper_Lam_2023_Evaluation.pdf

2. **Oxigraph BSBM Benchmarks** — Oxigraph upstream BSBM results (35M triples, 32GB RAM,
   concurrency 16). https://github.com/oxigraph/oxigraph/blob/main/bench/README.md

3. **GraphDB Benchmarks** — Ontotext official benchmark documentation including LDBC SNB SF30.
   https://graphdb.ontotext.com/documentation/11.2/benchmark.html

4. **QLever Performance Wiki** — University of Freiburg, QLever performance evaluation and
   comparison to other SPARQL engines.
   https://github.com/ad-freiburg/qlever/wiki/QLever-performance-evaluation-and-comparison-to-other-SPARQL-engines

5. **Amazon Neptune Graviton4 Blog** — AWS, "4.7× better write/query price-performance with
   Graviton4 r8g instances", 2024.
   https://aws.amazon.com/blogs/database/4-7-times-better-write-query-price-performance-with-aws-graviton4-r8g-instances-using-amazon-neptune-v1-4-5/

6. **RDF4J 5.x Release Notes** — Eclipse RDF4J, performance improvements across v4.x–v5.x:
   6× faster MINUS, 25× faster deletion, improved query planner.
   https://rdf4j.org/release-notes/

7. **Apache Jena 5 Release** — Apache Jena 5.0.0 release notes: Java 17+, TDB2 improvements.
   https://jena.apache.org/documentation/fuseki2/fuseki-changes.html

8. **GeoSPARQL Compliance Benchmark** — Jovanovik et al., "Compliance Testing for GeoSPARQL
   Implementations", 2021. https://arxiv.org/pdf/2102.06139

9. **BSBM — Berlin SPARQL Benchmark** — Bizer & Schultz, standard e-commerce workload.
   http://wifo5-03.informatik.uni-mannheim.de/bizer/berlinsparqlbenchmark/

10. **LDBC Social Network Benchmark** — LDBC Council, Interactive & BI workloads.
    https://ldbcouncil.org/benchmarks/snb/

11. **Virtuoso DBpedia Benchmark** — OpenLink, benchmarking on 198M triple DBpedia dataset.
    https://docs.openlinksw.com/virtuoso/rdfperfgeneraldbpedia/

12. **Stardog SP²Bench Blog** — Stardog, performance on SP²Bench vs. commercial competitors.
    https://www.stardog.com/blog/stardog-performance-sp2b-benchmark/

13. **W3C SPARQL 1.2 Working Group** — Working drafts and feature tracking (2023–).
    https://www.w3.org/groups/wg/rdf-star

14. **Wikimedia → QLever Migration** — Wikimedia Foundation, migration of Wikidata Query
    Service from Blazegraph, 2024–2025.
    https://phabricator.wikimedia.org/T328917
