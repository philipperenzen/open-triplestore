### Load

| Store | Scale | Load path | Load time (s) | Triples/s | Peak memory (MiB) | Store size (MiB) | HTTP POST load (s) |
| --- | --- | --- | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | HTTP Graph Store POST into a dataset graph, 100 MB N-Triples chunks | 16.7 | 22,475 | 778 | 156 | — |
| Oxigraph | 1k | `oxigraph load` (offline bulk loader) + `oxigraph optimize` | 7.1 | 52,545 | 330 | 69 | — |
| Open Triplestore | 10k | HTTP Graph Store POST into a dataset graph, 100 MB N-Triples chunks | 174.5 | 20,428 | 1,894 | 1,675 | — |
| Oxigraph | 10k | `oxigraph load` (offline bulk loader) + `oxigraph optimize` | 32.1 | 110,973 | 2,319 | 636 | — |

### BSBM explore, single client — 1k

| Query | Open Triplestore | Oxigraph |
| --- | --: | --: |
| Q1 | 1.01 / 1.85 | 1.16 / 3.09 |
| Q2 | 1.62 / 2.57 | 1.81 / 2.67 |
| Q3 | 1.24 / 2.03 | 1.28 / 3.23 |
| Q4 | 1.17 / 1.95 | 1.56 / 4.68 |
| Q5 | 3.23 / 4.94 | 6.70 / 13.0 |
| Q7 | 5.02 / 8.41 | 2.31 / 3.74 |
| Q8 | 7.38 / 11.4 | 1.69 / 2.70 |
| Q9 | 1.21 / 1.98 | 0.66 / 0.93 |
| Q10 | 1.76 / 2.62 | 1.41 / 2.25 |
| Q11 | 1.01 / 1.71 | 0.77 / 1.11 |
| Q12 | 1.17 / 1.84 | 0.95 / 1.34 |
| **QMpH** | **48,629** | **63,030** |

Cells: median / p95 latency in ms (100 measured mixes). QMpH: explore query mixes per hour.

### BSBM explore, single client — 10k

| Query | Open Triplestore | Oxigraph |
| --- | --: | --: |
| Q1 | 5.72 / 37.8 | 2.71 / 23.3 |
| Q2 | 5.08 / 7.16 | 2.37 / 3.39 |
| Q3 | 8.78 / 48.8 | 5.18 / 31.2 |
| Q4 | 9.88 / 65.8 | 5.10 / 36.6 |
| Q5 | 87.0 / 161 | 53.1 / 91.8 |
| Q7 | 7.23 / 11.4 | 3.57 / 6.10 |
| Q8 | 4.52 / 7.16 | 2.29 / 3.69 |
| Q9 | 1.98 / 2.82 | 0.72 / 1.08 |
| Q10 | 4.08 / 6.38 | 1.92 / 3.17 |
| Q11 | 2.08 / 3.08 | 0.77 / 1.11 |
| Q12 | 2.60 / 3.66 | 1.05 / 1.59 |
| **QMpH** | **10,859** | **19,331** |

Cells: median / p95 latency in ms (100 measured mixes). QMpH: explore query mixes per hour.

### SPARQL 1.1 feature mix — 1k

| Query | Open Triplestore | Oxigraph |
| --- | --: | --: |
| F01-path-closure | 5.88 | 6.81 |
| F02-path-sequence-inverse | 2.13 | 2.53 |
| F03-path-alternative | 144 | 212 |
| F04-path-oneormore-join | 3.35 | 2.19 |
| F05-agg-group-avg | 361 | 266 |
| F06-agg-having | 24.1 | 8.91 |
| F07-agg-count-distinct | 18.9 | 22.2 |
| F08-optional-unbound | 3.88 | 9.83 |
| F09-minus | 7.39 | 68.9 |
| F10-not-exists | 49.1 | 79.6 s (n=1) |
| F11-subquery-topk | 32.1 | 70.4 |
| F12-subquery-agg-join | 508 | 352 |
| F13-values-union | 10.4 | 2.32 |
| F14-count-all | 149 | 176 |

Cells: median latency in ms of 10 runs after 2 warm-up runs (n = fewer runs, see method).

### SPARQL 1.1 feature mix — 10k

| Query | Open Triplestore | Oxigraph |
| --- | --: | --: |
| F01-path-closure | 29.2 | 16.2 |
| F02-path-sequence-inverse | 5.61 | 2.36 |
| F03-path-alternative | 3039 | 2234 |
| F04-path-oneormore-join | 22.0 | 14.9 |
| F05-agg-group-avg | 7192 | 4511 |
| F06-agg-having | 106 | 99.1 |
| F07-agg-count-distinct | 454 | 293 |
| F08-optional-unbound | 148 | 91.8 |
| F09-minus | 1682 | 1246 |
| F10-not-exists | 445 | timeout |
| F11-subquery-topk | 1412 | 598 |
| F12-subquery-agg-join | 9189 | 6436 |
| F13-values-union | 6.86 | 2.56 |
| F14-count-all | 1433 | 1347 |

Cells: median latency in ms of 10 runs after 2 warm-up runs (n = fewer runs, see method).

### Concurrent clients — 1k

| Store | QMpH, 1 client | QMpH, 4 clients | QMpH, 8 clients | p95 ms, 1 | p95 ms, 4 | p95 ms, 8 | Failed queries |
| --- | --: | --: | --: | --: | --: | --: | --: |

### Concurrent clients — 10k

| Store | QMpH, 1 client | QMpH, 4 clients | QMpH, 8 clients | p95 ms, 1 | p95 ms, 4 | p95 ms, 8 | Failed queries |
| --- | --: | --: | --: | --: | --: | --: | --: |

### SHACL validation

| Store | Scale | Median (s) | p95 (s) | Runs | Results reported |
| --- | --: | --: | --: | --: | --: |

### Peak container memory per phase (MiB)

| Store | Scale | load | explore | features | concurrent | shacl |
| --- | --: | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | 778 | 895 | 975 | — | — |
| Oxigraph | 1k | 330 | 50 | 57 | — | — |
| Open Triplestore | 10k | 1,894 | 1,516 | 1,551 | — | — |
| Oxigraph | 10k | 2,319 | 149 | 261 | — | — |

### Result-count cross-check

No store returned a different result count for any query instance.

