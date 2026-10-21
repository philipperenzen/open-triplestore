### Load

| Store | Scale | Load path | Load time (s) | Triples/s | Peak memory (MiB) | Store size (MiB) | HTTP POST load (s) |
| --- | --- | --- | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | HTTP Graph Store POST into a dataset graph, 100 MB N-Triples chunks | 17.2 | 21,779 | 731 | 176 | — |
| Oxigraph | 1k | `oxigraph load` (offline bulk loader) + `oxigraph optimize` | 7.1 | 52,545 | 330 | 69 | — |
| Open Triplestore | 10k | HTTP Graph Store POST into a dataset graph, 100 MB N-Triples chunks | 150.5 | 23,692 | 1,860 | 1,680 | — |
| Oxigraph | 10k | `oxigraph load` (offline bulk loader) + `oxigraph optimize` | 32.1 | 110,973 | 2,319 | 636 | — |

### BSBM explore, single client — 1k

| Query | Open Triplestore | Oxigraph |
| --- | --: | --: |
| Q1 | 1.07 / 1.64 | 1.16 / 3.09 |
| Q2 | 1.72 / 2.61 | 1.81 / 2.67 |
| Q3 | 1.27 / 1.86 | 1.28 / 3.23 |
| Q4 | 1.29 / 1.82 | 1.56 / 4.68 |
| Q5 | 3.39 / 5.00 | 6.70 / 13.0 |
| Q7 | 5.65 / 10.3 | 2.31 / 3.74 |
| Q8 | 8.12 / 15.4 | 1.69 / 2.70 |
| Q9 | 1.30 / 2.12 | 0.66 / 0.93 |
| Q10 | 1.84 / 2.69 | 1.41 / 2.25 |
| Q11 | 1.01 / 1.53 | 0.77 / 1.11 |
| Q12 | 1.25 / 1.99 | 0.95 / 1.34 |
| **QMpH** | **44,133** | **63,030** |

Cells: median / p95 latency in ms (100 measured mixes). QMpH: explore query mixes per hour.

### BSBM explore, single client — 10k

| Query | Open Triplestore | Oxigraph |
| --- | --: | --: |
| Q1 | 3.79 / 26.0 | 2.71 / 23.3 |
| Q2 | 3.74 / 5.32 | 2.37 / 3.39 |
| Q3 | 6.22 / 32.3 | 5.18 / 31.2 |
| Q4 | 6.67 / 40.4 | 5.10 / 36.6 |
| Q5 | 52.5 / 97.3 | 53.1 / 91.8 |
| Q7 | 5.39 / 7.94 | 3.57 / 6.10 |
| Q8 | 3.41 / 5.29 | 2.29 / 3.69 |
| Q9 | 1.43 / 2.11 | 0.72 / 1.08 |
| Q10 | 2.91 / 4.63 | 1.92 / 3.17 |
| Q11 | 1.52 / 2.22 | 0.77 / 1.11 |
| Q12 | 1.83 / 2.88 | 1.05 / 1.59 |
| **QMpH** | **16,586** | **19,331** |

Cells: median / p95 latency in ms (100 measured mixes). QMpH: explore query mixes per hour.

### SPARQL 1.1 feature mix — 1k

| Query | Open Triplestore | Oxigraph |
| --- | --: | --: |
| F01-path-closure | 3.20 | 6.81 |
| F02-path-sequence-inverse | 1.17 | 2.53 |
| F03-path-alternative | 133 | 212 |
| F04-path-oneormore-join | 2.44 | 2.19 |
| F05-agg-group-avg | 279 | 266 |
| F06-agg-having | 20.4 | 8.91 |
| F07-agg-count-distinct | 18.8 | 22.2 |
| F08-optional-unbound | 6.83 | 9.83 |
| F09-minus | 12.5 | 68.9 |
| F10-not-exists | 55.5 | 79.6 s (n=1) |
| F11-subquery-topk | 40.2 | 70.4 |
| F12-subquery-agg-join | 376 | 352 |
| F13-values-union | 9.62 | 2.32 |
| F14-count-all | 1.04 | 176 |

Cells: median latency in ms of 10 runs after 2 warm-up runs (n = fewer runs, see method).

### SPARQL 1.1 feature mix — 10k

| Query | Open Triplestore | Oxigraph |
| --- | --: | --: |
| F01-path-closure | 24.6 | 16.2 |
| F02-path-sequence-inverse | 4.60 | 2.36 |
| F03-path-alternative | 1968 | 2234 |
| F04-path-oneormore-join | 20.5 | 14.9 |
| F05-agg-group-avg | 4084 | 4511 |
| F06-agg-having | 105 | 99.1 |
| F07-agg-count-distinct | 314 | 293 |
| F08-optional-unbound | 117 | 91.8 |
| F09-minus | 1337 | 1246 |
| F10-not-exists | 463 | timeout |
| F11-subquery-topk | 743 | 598 |
| F12-subquery-agg-join | 7396 | 6436 |
| F13-values-union | 5.42 | 2.56 |
| F14-count-all | 2.41 | 1347 |

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
| Open Triplestore | 1k | 731 | 903 | 911 | — | — |
| Oxigraph | 1k | 330 | 50 | 57 | — | — |
| Open Triplestore | 10k | 1,860 | 1,456 | 1,498 | — | — |
| Oxigraph | 10k | 2,319 | 149 | 261 | — | — |

### Result-count cross-check

No store returned a different result count for any query instance.

