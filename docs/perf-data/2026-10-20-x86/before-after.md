# Issue #468 on an x86 workstation: before and after (2026-10-20)

Host: AMD Ryzen 9 7900X3D, Docker Desktop on WSL 2 (`host.json`). Every
store in a 4 GiB, 4-CPU container. Before: develop `462b206c`; after:
`757c2489`. Medians from the harness (`campaign.py`, 100 measured explore
mixes, 10 runs per feature query), run back to back in one window.

**1k, BSBM explore, one client** — median ms

| Query | Before (`462b206c`) | After | Oxigraph 0.5.11 | After / Oxigraph |
| --- | --: | --: | --: | --: |
| Q1 | 1.01 | 1.07 | 1.16 | 0.92 |
| Q2 | 1.62 | 1.72 | 1.81 | 0.95 |
| Q3 | 1.24 | 1.27 | 1.28 | 0.99 |
| Q4 | 1.17 | 1.29 | 1.56 | 0.83 |
| Q5 | 3.23 | 3.39 | 6.70 | 0.51 |
| Q7 | 5.02 | 5.65 | 2.31 | 2.44 |
| Q8 | 7.38 | 8.12 | 1.69 | 4.81 |
| Q9 | 1.21 | 1.30 | 0.66 | 1.97 |
| Q10 | 1.76 | 1.84 | 1.41 | 1.31 |
| Q11 | 1.01 | 1.01 | 0.77 | 1.32 |
| Q12 | 1.17 | 1.25 | 0.95 | 1.32 |
| **QMpH** | **48,629** | **44,133** | **63,030** | **1.43×** slower |

**1k, SPARQL 1.1 feature mix** — median ms (runs)

| Query | Before (`462b206c`) | After | Oxigraph 0.5.11 | After / Oxigraph |
| --- | --: | --: | --: | --: |
| F01-path-closure | 5.88 | 3.20 | 6.81 | 0.47 |
| F02-path-sequence-inverse | 2.13 | 1.17 | 2.53 | 0.46 |
| F03-path-alternative | 144 | 133 | 212 | 0.63 |
| F04-path-oneormore-join | 3.35 | 2.44 | 2.19 | 1.12 |
| F05-agg-group-avg | 361 | 279 | 266 | 1.05 |
| F06-agg-having | 24.1 | 20.4 | 8.91 | 2.28 |
| F07-agg-count-distinct | 18.9 | 18.8 | 22.2 | 0.85 |
| F08-optional-unbound | 3.88 | 6.83 | 9.83 | 0.70 |
| F09-minus | 7.39 | 12.5 | 68.9 | 0.18 |
| F10-not-exists | 49.1 | 55.5 | 79,625 (1) | 0.00 |
| F11-subquery-topk | 32.1 | 40.2 | 70.4 | 0.57 |
| F12-subquery-agg-join | 508 | 376 | 352 | 1.07 |
| F13-values-union | 10.4 | 9.62 | 2.32 | 4.15 |
| F14-count-all | 149 | 1.04 | 176 | 0.01 |

**10k, BSBM explore, one client** — median ms

| Query | Before (`462b206c`) | After | Oxigraph 0.5.11 | After / Oxigraph |
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
| **QMpH** | **10,859** | **16,586** | **19,331** | **1.17×** slower |

**10k, SPARQL 1.1 feature mix** — median ms (runs)

| Query | Before (`462b206c`) | After | Oxigraph 0.5.11 | After / Oxigraph |
| --- | --: | --: | --: | --: |
| F01-path-closure | 29.2 | 24.6 | 16.2 | 1.52 |
| F02-path-sequence-inverse | 5.61 | 4.60 | 2.36 | 1.95 |
| F03-path-alternative | 3,039 | 1,968 | 2,234 | 0.88 |
| F04-path-oneormore-join | 22.0 | 20.5 | 14.9 | 1.38 |
| F05-agg-group-avg | 7,192 | 4,084 | 4,511 | 0.91 |
| F06-agg-having | 106 | 105 | 99.1 | 1.06 |
| F07-agg-count-distinct | 454 | 314 | 293 | 1.07 |
| F08-optional-unbound | 148 | 117 | 91.8 | 1.28 |
| F09-minus | 1,682 | 1,337 | 1,246 | 1.07 |
| F10-not-exists | 445 | 463 | timeout | — |
| F11-subquery-topk | 1,412 | 743 | 598 | 1.24 |
| F12-subquery-agg-join | 9,189 | 7,396 | 6,436 | 1.15 |
| F13-values-union | 6.86 | 5.42 | 2.56 | 2.11 |
| F14-count-all | 1,433 | 2.41 | 1,347 | 0.00 |

At 1k the accelerator answers most queries. The 1k harness run shows the
after build 9 % behind the before build on QMpH and F08/F09 up to 1.7 times
slower; an interleaved probe of the two builds side by side right after
(`probes/s4`, three rounds of 10 runs per query, both builds loaded) found
no difference beyond the spread (F08 6.4/7.3/6.2 against 5.9/5.9/6.4 ms,
F09 12.2/13.6/13.6 against 11.7/12.6/12.8 ms; the explore queries change sign
between rounds, Q5 +2, −7 and +22 %, Q8 −3, +21 and +3 %). The machine ran at
half its 2026-10-15 speed during this window.

## Paired probes, 10k (`probes/s3`, interleaved, both builds and Oxigraph loaded)

Median ms, 20 instances × 5 rounds.
`after-1g` is the after build through a second service of the same dataset
whose default graph is the data graph alone.

| Query | Oxigraph | Before | After | After-1g |
| --- | --: | --: | --: | --: |
| Q1 | 2.114 | 4.618 | 3.690 | 3.415 |
| Q2 | 1.883 | 3.558 | 3.318 | 3.314 |
| Q3 | 4.967 | 9.908 | 7.417 | 6.607 |
| Q4 | 4.011 | 8.526 | 6.031 | 5.746 |
| Q5 | 41.872 | 68.886 | 47.786 | 42.027 |
| Q7 | 2.736 | 4.761 | 3.755 | 3.977 |
| Q8 | 1.654 | 3.308 | 2.500 | 2.671 |
| Q9 | 0.666 | 1.513 | 1.219 | 1.447 |
| Q10 | 1.221 | 3.147 | 2.247 | 2.443 |
| Q11 | 0.589 | 1.690 | 1.281 | 1.449 |
| Q12 | 0.833 | 2.217 | 1.748 | 1.702 |

| | Oxigraph | Before | After |
| --- | --: | --: | --: |
| trivial `ASK` (ms) | 0.556 | 1.710 | 1.529 |
| F14 `COUNT(*)` (ms) | 1368.696 | 1493.416 | 4.230 |

`GET /livez` on the same builds: 0.65 and 0.60 ms; Oxigraph's own minimal
round trip is 0.56 ms.

## Diagnostics (2026-10-15, `probes/s1`, `probes/s2`, baseline image)

Before the fixes, at 10k, interleaved (median ms):

| Query | Oxigraph | dataset service | same dataset, data graph only |
| --- | --: | --: | --: |
| Q1 | 0.806 | 1.636 | 1.290 |
| Q2 | 0.862 | 1.475 | 1.472 |
| Q3 | 1.827 | 3.439 | 2.659 |
| Q4 | 1.549 | 3.411 | 2.389 |
| Q5 | 17.059 | 31.983 | 20.234 |
| Q7 | 0.886 | 1.928 | 1.477 |
| Q8 | 0.559 | 1.207 | 1.003 |
| Q9 | 0.223 | 0.584 | 0.582 |
| Q10 | 0.516 | 1.205 | 0.996 |
| Q11 | 0.242 | 0.608 | 0.549 |
| Q12 | 0.321 | 0.874 | 0.794 |

After `oxigraph optimize` (a full compaction) of the same Open Triplestore
store and a restart, Q5 took 37.8 ms over the service and 23.1 ms over the
data graph alone: the store's file layout is not the difference.

Per request, a trivial `ASK` (median ms): Oxigraph `GET /query` 0.173, `POST
/query` 0.202; Open Triplestore `GET /livez` 0.243, the dataset service with
a token 0.649, anonymous 0.593, `/sparql` with a token 0.612.
