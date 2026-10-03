### Load

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

### BSBM explore, single client — 1k

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

### BSBM explore, single client — 10k

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

### SPARQL 1.1 feature mix — 1k

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

Cells: median latency in ms of 10 runs after 2 warm-up runs (n = fewer runs, see method).

### SPARQL 1.1 feature mix — 10k

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

Cells: median latency in ms of 10 runs after 2 warm-up runs (n = fewer runs, see method).

### Concurrent clients — 1k

| Store | QMpH, 1 client | QMpH, 4 clients | QMpH, 8 clients | p95 ms, 1 | p95 ms, 4 | p95 ms, 8 | Failed queries |
| --- | --: | --: | --: | --: | --: | --: | --: |
| Open Triplestore | 95,580 | 240,780 | 317,640 | 4.81 | 5.48 | 12.2 | 0 |
| Oxigraph | 157,860 | 590,280 | 721,740 | 2.84 | 2.98 | 3.76 | 0 |
| Fuseki | 18,120 | 47,280 | 27,960 | 72.6 | 116 | 438 | 0 |
| QLever | 2,460 | 9,600 | 9,960 | 154 | 168 | 306 | 0 |
| Virtuoso | 59,160 | 201,480 | 329,040 | 4.06 | 4.87 | 6.57 | 0 |
| RDF4J | 28,680 | 76,080 | 52,560 | 20.0 | 31.6 | 99.7 | 0 |

### Concurrent clients — 10k

| Store | QMpH, 1 client | QMpH, 4 clients | QMpH, 8 clients | p95 ms, 1 | p95 ms, 4 | p95 ms, 8 | Failed queries |
| --- | --: | --: | --: | --: | --: | --: | --: |
| Open Triplestore | 8,460 | 39,300 | 64,740 | 58.5 | 56.1 | 91.0 | 0 |
| Oxigraph | 39,360 | 130,080 | 121,200 | 24.3 | 28.8 | 76.6 | 0 |
| Fuseki | 5,220 | 13,080 | 7,680 | 290 | 469 | 1638 | 0 |
| QLever | 2,340 | 8,820 | 7,680 | 174 | 187 | 439 | 0 |
| Virtuoso | 51,480 | 177,000 | 273,360 | 4.48 | 5.47 | 7.44 | 0 |
| RDF4J | 3,360 | 6,060 | 5,460 | 194 | 448 | 971 | 0 |

### SHACL validation

| Store | Scale | Median (s) | p95 (s) | Runs | Results reported |
| --- | --: | --: | --: | --: | --: |
| Open Triplestore | 1k | 0.33 | 0.37 | 5 | 5,245 |
| Fuseki | 1k | 0.98 | 0.99 | 5 | 5,245 |
| Open Triplestore | 10k | 14.09 | 15.46 | 5 | 51,690 |
| Fuseki | 10k | 12.36 | 12.72 | 5 | 51,690 |

### Peak container memory per phase (MiB)

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

### Result-count cross-check

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

