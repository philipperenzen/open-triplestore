### Load

| Store | Scale | Load path | Load time (s) | Triples/s | Peak memory (MiB) | Store size (MiB) | HTTP POST load (s) |
| --- | --- | --- | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | HTTP Graph Store POST into a dataset graph, 100 MB N-Triples chunks | 11.5 | 32,505 | 617 | 174 | — |
| Oxigraph | 1k | `oxigraph load` (offline bulk loader) + `oxigraph optimize` | 4.2 | 89,095 | 103 | 69 | 6.0 |
| Open Triplestore | 10k | HTTP Graph Store POST into a dataset graph, 100 MB N-Triples chunks | 105.9 | 33,650 | 1,744 | 1,855 | — |
| Oxigraph | 10k | `oxigraph load` (offline bulk loader) + `oxigraph optimize` | 22.3 | 159,920 | 2,231 | 636 | 65.6 |

### BSBM explore, single client — 1k

| Query | Open Triplestore | Oxigraph |
| --- | --: | --: |
| Q1 | 0.66 / 1.00 | 0.49 / 1.48 |
| Q2 | 1.07 / 1.43 | 1.06 / 1.39 |
| Q3 | 0.78 / 1.18 | 0.62 / 1.60 |
| Q4 | 0.82 / 1.35 | 0.83 / 2.55 |
| Q5 | 2.18 / 3.28 | 4.04 / 7.69 |
| Q7 | 4.10 / 5.28 | 1.25 / 2.01 |
| Q8 | 6.88 / 8.19 | 0.90 / 1.47 |
| Q9 | 0.87 / 1.30 | 0.27 / 0.42 |
| Q10 | 1.50 / 2.40 | 0.75 / 1.27 |
| Q11 | 0.66 / 1.05 | 0.34 / 0.55 |
| Q12 | 0.83 / 1.10 | 0.45 / 0.60 |
| **QMpH** | **64,685** | **117,023** |

Cells: median / p95 latency in ms (100 measured mixes). QMpH: explore query mixes per hour.

### BSBM explore, single client — 10k

| Query | Open Triplestore | Oxigraph |
| --- | --: | --: |
| Q1 | 3.41 / 21.6 | 1.57 / 11.6 |
| Q2 | 2.91 / 3.99 | 1.43 / 2.02 |
| Q3 | 5.35 / 28.9 | 2.70 / 16.4 |
| Q4 | 5.99 / 35.5 | 2.83 / 18.9 |
| Q5 | 62.1 / 115 | 31.4 / 57.3 |
| Q7 | 4.01 / 8.01 | 2.07 / 3.72 |
| Q8 | 2.73 / 5.39 | 1.36 / 2.16 |
| Q9 | 1.00 / 1.66 | 0.34 / 0.55 |
| Q10 | 2.34 / 4.06 | 1.06 / 1.71 |
| Q11 | 1.15 / 1.95 | 0.41 / 0.55 |
| Q12 | 1.46 / 2.13 | 0.56 / 0.74 |
| **QMpH** | **16,570** | **33,719** |

Cells: median / p95 latency in ms (100 measured mixes). QMpH: explore query mixes per hour.

### SPARQL 1.1 feature mix — 1k

| Query | Open Triplestore | Oxigraph |
| --- | --: | --: |
| F01-path-closure | 3.85 | 4.66 |
| F02-path-sequence-inverse | 1.08 | 1.81 |
| F03-path-alternative | 124 | 136 |
| F04-path-oneormore-join | 3.77 | 2.61 |
| F05-agg-group-avg | 291 | 180 |
| F06-agg-having | 9.20 | 5.88 |
| F07-agg-count-distinct | 11.8 | 16.1 |
| F08-optional-unbound | 3.75 | 7.87 |
| F09-minus | 6.53 | 35.8 |
| F10-not-exists | 42.3 | 118.9 s (n=1) |
| F11-subquery-topk | 18.9 | 63.6 |
| F12-subquery-agg-join | 364 | 217 |
| F13-values-union | 8.77 | 1.20 |
| F14-count-all | 87.8 | 141 |

Cells: median latency in ms of 10 runs after 2 warm-up runs (n = fewer runs, see method).

### SPARQL 1.1 feature mix — 10k

| Query | Open Triplestore | Oxigraph |
| --- | --: | --: |
| F01-path-closure | 21.0 | 9.84 |
| F02-path-sequence-inverse | 5.03 | 2.00 |
| F03-path-alternative | 3163 | 1777 |
| F04-path-oneormore-join | 21.0 | 13.1 |
| F05-agg-group-avg | 4588 | 2821 |
| F06-agg-having | 90.7 | 64.0 |
| F07-agg-count-distinct | 305 | 212 |
| F08-optional-unbound | 126 | 76.8 |
| F09-minus | 1138 | 685 |
| F10-not-exists | 366 | timeout |
| F11-subquery-topk | 1468 | 671 |
| F12-subquery-agg-join | 5834 | 5461 |
| F13-values-union | 2.33 | 1.31 |
| F14-count-all | 2846 | 1500 |

Cells: median latency in ms of 10 runs after 2 warm-up runs (n = fewer runs, see method).

### Concurrent clients — 1k

| Store | QMpH, 1 client | QMpH, 4 clients | QMpH, 8 clients | p95 ms, 1 | p95 ms, 4 | p95 ms, 8 | Failed queries |
| --- | --: | --: | --: | --: | --: | --: | --: |
| Open Triplestore | 66,060 | 230,940 | 239,520 | 6.82 | 7.21 | 21.4 | 0 |
| Oxigraph | 119,640 | 515,160 | 627,660 | 3.61 | 3.31 | 4.47 | 0 |

### Concurrent clients — 10k

| Store | QMpH, 1 client | QMpH, 4 clients | QMpH, 8 clients | p95 ms, 1 | p95 ms, 4 | p95 ms, 8 | Failed queries |
| --- | --: | --: | --: | --: | --: | --: | --: |
| Open Triplestore | 16,020 | 65,220 | 61,620 | 56.4 | 57.3 | 109 | 0 |
| Oxigraph | 31,980 | 97,380 | 98,640 | 28.1 | 38.0 | 84.5 | 0 |

### SHACL validation

| Store | Scale | Median (s) | p95 (s) | Runs | Results reported |
| --- | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | 0.27 | 0.29 | 5 | 5,245 |
| Open Triplestore | 10k | 15.74 | 16.09 | 5 | 51,690 |

### Peak container memory per phase (MiB)

| Store | Scale | load | explore | features | concurrent | shacl |
| --- | --: | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | 617 | 897 | 1,023 | 1,001 | 1,062 |
| Oxigraph | 1k | 103 | 52 | 59 | 62 | — |
| Open Triplestore | 10k | 1,744 | 1,369 | 1,371 | 1,417 | 3,958 |
| Oxigraph | 10k | 2,231 | 149 | 263 | 474 | — |

### Result-count cross-check

No store returned a different result count for any query instance.

