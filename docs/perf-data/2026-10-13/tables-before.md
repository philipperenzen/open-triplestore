### Load

| Store | Scale | Load path | Load time (s) | Triples/s | Peak memory (MiB) | Store size (MiB) | HTTP POST load (s) |
| --- | --- | --- | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | HTTP Graph Store POST into a dataset graph, 100 MB N-Triples chunks | 15.6 | 24,014 | 786 | 179 | — |
| Open Triplestore | 10k | HTTP Graph Store POST into a dataset graph, 100 MB N-Triples chunks | 149.3 | 23,879 | 3,843 | 1,778 | — |

### BSBM explore, single client — 1k

| Query | Open Triplestore |
| --- | --: |
| Q1 | 0.69 / 0.95 |
| Q2 | 1.09 / 1.49 |
| Q3 | 0.82 / 1.12 |
| Q4 | 0.85 / 1.17 |
| Q5 | 2.03 / 3.15 |
| Q7 | 3.51 / 4.34 |
| Q8 | 5.84 / 7.23 |
| Q9 | 41.5 / 45.7 |
| Q10 | 1.60 / 2.55 |
| Q11 | 0.69 / 1.04 |
| Q12 | 0.80 / 1.27 |
| **QMpH** | **24,923** |

Cells: median / p95 latency in ms (100 measured mixes). QMpH: explore query mixes per hour.

### BSBM explore, single client — 10k

| Query | Open Triplestore |
| --- | --: |
| Q1 | 44.8 / 71.4 |
| Q2 | 5.01 / 8.10 |
| Q3 | 39.9 / 63.4 |
| Q4 | 21.5 / 53.8 |
| Q5 | 110 / 175 |
| Q7 | 9.98 / 44.0 |
| Q8 | 6.55 / 13.6 |
| Q9 | 42.6 / 46.2 |
| Q10 | 44.3 / 48.7 |
| Q11 | 2.51 / 5.49 |
| Q12 | 2.36 / 3.62 |
| **QMpH** | **5,358** |

Cells: median / p95 latency in ms (100 measured mixes). QMpH: explore query mixes per hour.

### SPARQL 1.1 feature mix — 1k

| Query | Open Triplestore |
| --- | --: |
| F01-path-closure | 2.93 |
| F02-path-sequence-inverse | 0.91 |
| F03-path-alternative | 93.6 |
| F04-path-oneormore-join | 2.76 |
| F05-agg-group-avg | 320 |
| F06-agg-having | 9.04 |
| F07-agg-count-distinct | 146 |
| F08-optional-unbound | 2.84 |
| F09-minus | 5.28 |
| F10-not-exists | 5047 |
| F11-subquery-topk | 16.7 |
| F12-subquery-agg-join | 357 |
| F13-values-union | 8.55 |
| F14-count-all | 45.2 |

Cells: median latency in ms of 10 runs after 2 warm-up runs (n = fewer runs, see method).

### SPARQL 1.1 feature mix — 10k

| Query | Open Triplestore |
| --- | --: |
| F01-path-closure | 17.6 |
| F02-path-sequence-inverse | 3.67 |
| F03-path-alternative | 3492 |
| F04-path-oneormore-join | 18.4 |
| F05-agg-group-avg | 4948 |
| F06-agg-having | 79.1 |
| F07-agg-count-distinct | 320 |
| F08-optional-unbound | 117 |
| F09-minus | 1211 |
| F10-not-exists | timeout |
| F11-subquery-topk | 1326 |
| F12-subquery-agg-join | 5226 |
| F13-values-union | 2.37 |
| F14-count-all | 2102 |

Cells: median latency in ms of 10 runs after 2 warm-up runs (n = fewer runs, see method).

### Concurrent clients — 1k

| Store | QMpH, 1 client | QMpH, 4 clients | QMpH, 8 clients | p95 ms, 1 | p95 ms, 4 | p95 ms, 8 | Failed queries |
| --- | --: | --: | --: | --: | --: | --: | --: |
| Open Triplestore | 28,740 | 182,160 | 285,300 | 41.9 | 7.89 | 16.9 | 0 |

### Concurrent clients — 10k

| Store | QMpH, 1 client | QMpH, 4 clients | QMpH, 8 clients | p95 ms, 1 | p95 ms, 4 | p95 ms, 8 | Failed queries |
| --- | --: | --: | --: | --: | --: | --: | --: |
| Open Triplestore | 5,640 | 29,220 | 53,880 | 102 | 77.5 | 101 | 0 |

### SHACL validation

| Store | Scale | Median (s) | p95 (s) | Runs | Results reported |
| --- | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | 0.53 | 0.55 | 5 | 5,245 |

### Peak container memory per phase (MiB)

| Store | Scale | load | explore | features | concurrent | shacl |
| --- | --: | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | 786 | 1,215 | 1,168 | 1,188 | 1,191 |
| Open Triplestore | 10k | 3,843 | 3,282 | 3,353 | 3,404 | 4,092 |

### Result-count cross-check

No store returned a different result count for any query instance.

