# Tables generated from raw/ (2026-10-15, x86). The curated tables are in docs/performance.md, "Delta versions at scale".

## loads
- load-base09-data.json: 899999 quads in 8.36 s (107675/s), store 900156, dir 178.69 MiB
- load-base09-s250.json: 250002 quads in 2.63 s (95107/s), store 1200162, dir 271.74 MiB
- load-base09-s50.json: 50004 quads in 0.51 s (98217/s), store 950160, dir 218.75 MiB
- load-base17-data.json: 899999 quads in 7.99 s (112628/s), store 900156, dir 178.69 MiB
- load-base17-other.json: 800001 quads in 7.42 s (107835/s), store 1700157, dir 399.69 MiB
- load-base9m-data.json: 8999999 quads in 85.35 s (105449/s), store 9000156, dir 2988.9 MiB
- load-base9m-v09.json: 900000 quads in 7.63 s (117882/s), store 9900156, dir 2862.43 MiB

## item 1 scan
| graph quads | cap | off | on | ratio | added | on row extent | peak RSS on (MiB) | collect | diff |
|--:|--:|--:|--:|--:|--:|--|--|--:|--:|
| 50004 | 250000 | 183 ms (172 ms–192 ms) | 442 ms (429 ms–478 ms) | ×2.42 | 259 ms | full,full | [38.06, 20.25, 0.75, 0.19, 0.0] | 183 ms | 17 ms |
| 250002 | 250000 | 674 ms (664 ms–694 ms) | 680 ms (660 ms–681 ms) | ×1.01 | 5.6 ms | unknown,unknown | [0.56, 0.0, 0.19, 0.0, 0.19] | 754 ms | 140 ms |
| 899999 | 250000 | 2.59 s (2.48 s–2.62 s) | 2.61 s (2.52 s–2.66 s) | ×1.01 | 23 ms | unknown,unknown | [0.38, 0.19, 0.19, 0.0, 0.19] | 2.93 s | 618 ms |
| 50004 | 20000000 | 188 ms (182 ms–192 ms) | 440 ms (430 ms–470 ms) | ×2.34 | 252 ms | full,full | [38.44, 20.25, 0.56, 0.19, 0.19] | 191 ms | 17 ms |
| 250002 | 20000000 | 708 ms (676 ms–751 ms) | 1.88 s (1.82 s–1.94 s) | ×2.65 | 1.17 s | full,full | [196.88, 104.25, 99.75, 95.44, 95.44] | 738 ms | 135 ms |
| 899999 | 20000000 | 2.58 s (2.56 s–2.71 s) | 7.29 s (7.20 s–7.43 s) | ×2.82 | 4.70 s | full,full | [696.56, 361.31, 361.31, 361.13, 361.31] | 2.82 s | 670 ms |
| 8999999 | 20000000 | 25.4 s (24.9 s–26.3 s) | 74.9 s (74.4 s–78.6 s) | ×2.95 | 49.5 s | full,full | [7077.19, 3721.13, 3734.44] | 27.4 s | 11.0 s |

## item 2 probe
- bulk_insert_quads_off: 11.6 s (9.25 s–11.7 s)
- bulk_insert_quads_on: 12.9 s (12.6 s–14.2 s)
- load_named_off: 13.5 s (13.2 s–15.4 s)
- load_named_on: 13.0 s (12.8 s–14.0 s)
- rows: {"bulk_insert_quads_on": [{"added_n": 999999, "extent": "counts", "graph": "https://example.org/otl/probe", "removed_n": 0, "seq": 10, "state": "committed"}], "load_named_on": [{"added_n": 999999, "extent": "counts", "graph": "https://example.org/otl/probe", "removed_n": 0, "seq": 11, "state": "committed"}]}

## item 4 put
- put09-capdefault: off 56.7 s (55.1 s–59.3 s), on 58.4 s (56.2 s–61.9 s), ratio ×1.03, peak off [2552.61, 1327.69, 1341.59] on [2348.09, 1504.02, 1327.49], rows ['unknown', 'unknown', 'unknown'], first PUT 6.27 s
- put09-scanned: off 57.0 s (55.4 s–58.6 s), on 60.8 s (60.5 s–61.5 s), ratio ×1.07, peak off [2554.81, 1326.0, 1342.0] on [2688.24, 1672.97, 1337.68], rows ['full', 'full', 'full'], first PUT 6.32 s

## item 9 payload
### payload09-cap1m (max_payload 1000000)
| quads | N-Quads MB | gzip MB | B/quad gz | serialise | gzip | decode | PUT empty off | on | ratio | added | blob |
|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| 49995 | 7.1 | 0.51 | 10.12 | 7.1 ms | 3.4 ms | 56 ms | 487 ms (316 ms–566 ms) | 583 ms (514 ms–597 ms) | ×1.2 | 96 ms | [506169, 0] |
| 249993 | 35.7 | 2.53 | 10.13 | 65 ms | 14 ms | 265 ms | 1.90 s (1.79 s–2.06 s) | 2.34 s (2.20 s–2.40 s) | ×1.23 | 441 ms | [2532533, 0] |
| 900000 | 128.6 | 9.12 | 10.13 | 209 ms | 54 ms | 1.02 s | 7.67 s (6.54 s–7.72 s) | 8.40 s (8.38 s–8.62 s) | ×1.1 | 735 ms | [9118535, 0] |
### payload09-capdefault (max_payload 250000)
| quads | N-Quads MB | gzip MB | B/quad gz | serialise | gzip | decode | PUT empty off | on | ratio | added | blob |
|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| 900000 | 128.6 | 9.12 | 10.13 | 203 ms | 52 ms | 982 ms | 7.38 s (6.26 s–7.40 s) | 7.56 s (7.55 s–7.68 s) | ×1.02 | 175 ms | [0, 0] |

### bulk 9M
- off 1: 46.9 s (191713 q/s), peak +4503.9 MiB, rows , blob [0, 0], log growth 0.0 MiB
- off 2: 46.4 s (193943 q/s), peak +4517.85 MiB, rows , blob [0, 0], log growth 0.0 MiB
- on 1: 57.1 s (157571 q/s), peak +4698.04 MiB, rows counts, blob [0, 0], log growth 0.06 MiB
- on 2: 57.0 s (157817 q/s), peak +4697.7 MiB, rows counts, blob [0, 0], log growth 0.06 MiB
- on-payload 1: 59.1 s (152273 q/s), peak +7846.36 MiB, rows full, blob [90633447, 0], log growth 173.84 MiB
- on-payload 2: 60.1 s (149773 q/s), peak +8042.57 MiB, rows full, blob [90633447, 0], log growth 173.84 MiB

## item 5 txrss
| quads | mode | capture | ms (median) | peak above start MiB (each) | B/quad |
|--:|--|--|--:|--|--:|
| 50000 | atomic | off | 1.41 s | [115.69, 114.94] | 2418 |
| 50000 | atomic | on | 1.65 s | [134.47, 137.06, 136.88] | 2870 |
| 200000 | atomic | off | 8.43 s | [453.39, 453.66] | 2378 |
| 200000 | atomic | on | 7.41 s | [528.84, 530.29, 529.9] | 2778 |
| 500000 | atomic | on | 24.2 s | [1321.38, 1321.82, 1321.49] | 2771 |
| 1000000 | atomic | on | 60.4 s | [2641.31, 2640.96, 2641.59] | 2770 |
| 1000000 | chunked | on | 33.8 s | [1696.82, 1696.98] | 1779 |

## item 5 restore
### restore09-k1000: graph 899999, inverse ops 3020, cut checkpoint 39.3 s, cut delta [6.66, 7.2], deltas True/True
- build backward_v3: 39.9 s (39.7 s–40.7 s) peak [41.6, 17.26, 13.11]
- build forward_v2: 42.4 s (38.7 s–42.5 s) peak [61.93, 34.88, 12.14]
- tier 1 total 3.43 s (3.38 s–3.47 s), apply 49 ms (44 ms–56 ms), hash 3.37 s (3.31 s–3.41 s), peak [0.75, 0.19, 0.0]
- tier 2 87.2 s (85.4 s–89.6 s), peak [3220.38, 867.88, 2099.01], log growth [9.91, 11.79, 7.76]
### restore09-k10000: graph 899999, inverse ops 27000, cut checkpoint 39.6 s, cut delta [42.16, 40.64], deltas True/True
- build backward_v3: 40.2 s (39.9 s–40.3 s) peak [27.18, 17.91, 11.5]
- build forward_v2: 41.9 s (41.2 s–42.3 s) peak [69.94, 19.95, 12.12]
- tier 1 total 3.59 s (3.42 s–3.72 s), apply 601 ms (559 ms–618 ms), hash 2.88 s (2.81 s–2.93 s), peak [15.75, 0.0, 0.0]
- tier 2 87.7 s (87.1 s–91.2 s), peak [3251.2, 847.38, 2101.25], log growth [8.08, 11.33, 11.33]
### restore09-k100000: graph 899999, inverse ops 300020, cut checkpoint 45.6 s, cut delta [307.56, 718.21], deltas True/True
- build backward_v3: 47.8 s (45.8 s–53.9 s) peak [307.91, 293.75, 291.52]
- build forward_v2: 58.3 s (44.4 s–71.5 s) peak [306.83, 218.73, 224.46]
- tier 1 total 18.9 s (18.5 s–19.0 s), apply 14.2 s (14.0 s–14.3 s), hash 3.25 s (3.17 s–3.54 s), peak [763.45, 868.19, 893.15]
- tier 2 148.1 s (119.7 s–198.0 s), peak [2881.12, 2794.51, 2894.53], log growth [6.64, 11.59, 11.59]

## item 10 coldread
### coldread17-mirror: store 2600192, built 899999 in 51.7 s (peak 262.05 MiB), mirror True published-before True settle 29.03 s, republish 55.39 s
| phase | lookup n | p50 | p95 | max | scan n | p50 | p95 | max |
|--|--:|--:|--:|--:|--:|--:|--:|--:|
| before | 4926906 | 0.0 ms | 0.0 ms | 9.7 ms | 4011 | 4.7 ms | 6.9 ms | 18 ms |
| during_build | 2069308 | 0.1 ms | 0.2 ms | 16 ms | 2036 | 25 ms | 31 ms | 42 ms |
| until_mirror_republished | 2170178 | 0.1 ms | 0.2 ms | 473 ms | 2200 | 24 ms | 30 ms | 42 ms |
| after | 4238186 | 0.0 ms | 0.0 ms | 5.5 ms | 3562 | 5.3 ms | 7.6 ms | 19 ms |
### coldread99-mirror: store 10800192, built 900000 in 56.6 s (peak 286.42 MiB), mirror True published-before True settle 187.13 s, republish 233.61 s
| phase | lookup n | p50 | p95 | max | scan n | p50 | p95 | max |
|--|--:|--:|--:|--:|--:|--:|--:|--:|
| before | 4041825 | 0.0 ms | 0.0 ms | 15 ms | 740 | 26 ms | 33 ms | 49 ms |
| during_build | 2025283 | 0.1 ms | 0.2 ms | 31 ms | 184 | 309 ms | 349 ms | 430 ms |
| until_mirror_republished | 9474600 | 0.1 ms | 0.1 ms | 1.42 s | 767 | 296 ms | 346 ms | 1.85 s |
| after | 4132068 | 0.0 ms | 0.0 ms | 12 ms | 757 | 26 ms | 32 ms | 46 ms |
### coldread99-nomirror: store 10800192, built 900000 in 48.9 s (peak 222.96 MiB), mirror False published-before False settle 0.0 s, republish 0.0 s
| phase | lookup n | p50 | p95 | max | scan n | p50 | p95 | max |
|--|--:|--:|--:|--:|--:|--:|--:|--:|
| before | 895617 | 0.1 ms | 0.1 ms | 5.1 ms | 70 | 287 ms | 311 ms | 339 ms |
| during_build | 2276728 | 0.1 ms | 0.1 ms | 17 ms | 165 | 295 ms | 318 ms | 376 ms |
| until_mirror_republished | 45 | 0.1 ms | 0.2 ms | 0.2 ms | 0 | 0.0 ms | 0.0 ms | 0.0 ms |
| after | 915680 | 0.1 ms | 0.1 ms | 3.7 ms | 68 | 292 ms | 313 ms | 340 ms |

## item 11 ldes
| depth | m | graph quads | scan path | rows path | write | scan members | rows members | publish_all |
|--:|--:|--:|--:|--:|--:|--|--|--:|
| 1 | 100 | 100000 | 664 ms (661 ms–758 ms) | 5.3 ms (5.2 ms–6.1 ms) | 1.9 ms (1.9 ms–2.0 ms) | [100, 100, 100] | [100, 100, 100] | 1.16 s |
| 4 | 100 | 220000 | 1.68 s (1.63 s–1.90 s) | 9.2 ms (8.6 ms–9.4 ms) | 2.5 ms (2.1 ms–2.5 ms) | [100, 100, 100] | [100, 100, 100] | 2.25 s |
| 8 | 100 | 380000 | 3.19 s (3.17 s–3.31 s) | 15 ms (14 ms–16 ms) | 2.6 ms (2.2 ms–3.9 ms) | [100, 100, 100] | [100, 100, 100] | 3.46 s |
| 9 | 100 | 420000 | 3.34 s (3.23 s–3.64 s) | 5.7 ms (5.6 ms–5.8 ms) | 2.2 ms (2.0 ms–10 ms) | [100, 100, 100] | [0, 0, 0] | 3.80 s |
| 4 | 4000 | 220000 | 2.20 s (2.04 s–2.20 s) | 382 ms (362 ms–382 ms) | 85 ms (66 ms–85 ms) | [4000, 4000] | [4000, 4000] | 2.27 s |
| 4 | 6000 | 220000 | 2.31 s (2.16 s–2.31 s) | 2.19 s (2.18 s–2.19 s) | 154 ms (126 ms–154 ms) | [6000, 6000] | [20000, 20000] | 2.25 s |

## item 6 annotations
- off: data 900000, annotations 0 (written 0), load 8.15 s, consumer 88 ms, dir 261.47 MiB (rocksdb 250.58, log 10.89), after optimize {'dir_mib': 286.64, 'optimize_ms': 884.17, 'rocksdb_mib': 275.75}
- minimal: data 900000, annotations 4500000 (written 4500000), load 11.9 s, consumer 177.3 s, dir 3974.91 MiB (rocksdb 3963.9, log 11.0), after optimize {'dir_mib': 3845.37, 'optimize_ms': 6784.78, 'rocksdb_mib': 3834.37}
- full: data 900000, annotations 5400000 (written 5400000), load 19.3 s, consumer 201.3 s, dir 4385.28 MiB (rocksdb 4374.27, log 11.0), after optimize {'dir_mib': 4222.73, 'optimize_ms': 9879.46, 'rocksdb_mib': 4211.73}

## census: 25 files, 984 quads, 284 with a blank node (0.29), max steps to owner 5
- clinical-reference/shapes.ttl: 39 quads, 35 with bnode, 10 bnodes, max steps 5 {'1': 6, '2': 1, '3': 1, '4': 1, '5': 1}, lists 4 cells longest 4, orphans 0
- dqv-quality/shapes.ttl: 81 quads, 59 with bnode, 15 bnodes, max steps 3 {'1': 12, '2': 2, '3': 1}, lists 2 cells longest 2, orphans 0
- layered-reference/shapes.ttl: 19 quads, 17 with bnode, 5 bnodes, max steps 3 {'1': 3, '2': 1, '3': 1}, lists 2 cells longest 2, orphans 0
- nen2660-relations/instances.ttl: 55 quads, 16 with bnode, 8 bnodes, max steps 1 {'1': 8}, lists 0 cells longest 0, orphans 0
- nen2660-relations/shapes.ttl: 59 quads, 34 with bnode, 8 bnodes, max steps 1 {'1': 8}, lists 0 cells longest 0, orphans 0
- opm-profile/shapes.ttl: 151 quads, 123 with bnode, 42 bnodes, max steps 4 {'1': 22, '2': 9, '3': 6, '4': 5}, lists 11 cells longest 3, orphans 0

## item 7 crash
- iterations 60, violations 0, orphan SSTs seen 1880
  - bulk in flight: data all, row unknown: 1
  - bulk in flight: data none, row unknown: 45
  - bulk: chain closed (sequence 12 changed <urn:crash:g:bulk> in a way the log could not record): 10
  - bulk: chain closed (sequence 4 is a write whose graphs were not known): 20
  - bulk: chain closed (sequence 9 is a write whose graphs were not known): 30
  - ops in flight: data all, row unknown: 3
  - ops in flight: data none, row unknown: 57
  - ops: chain closed (sequence 11 changed <urn:crash:g:ops> in a way the log could not record): 10
  - ops: chain closed (sequence 2 changed <urn:crash:g:ops> in a way the log could not record): 10
  - ops: chain closed (sequence 3 changed <urn:crash:g:ops> in a way the log could not record): 10
  - ops: chain closed (sequence 9 is a write whose graphs were not known): 30
  - put in flight: data all, row unknown: 7
  - put in flight: data none, row unknown: 53
  - put: chain closed (sequence 14 changed <urn:crash:g:put> in a way the log could not record): 10
  - put: chain closed (sequence 4 is a write whose graphs were not known): 20
  - put: chain closed (sequence 8 changed <urn:crash:g:put> in a way the log could not record): 10
  - put: chain closed (sequence 9 is a write whose graphs were not known): 20
  - sst in flight: data all, row unknown: 1
  - sst in flight: data none, row unknown: 59
  - sst: chain closed (sequence 13 changed <urn:crash:g:sst> in a way the log could not record): 10
  - sst: chain closed (sequence 3 changed <urn:crash:g:sst> in a way the log could not record): 10
  - sst: chain closed (sequence 4 is a write whose graphs were not known): 10
  - sst: chain closed (sequence 9 is a write whose graphs were not known): 30
  - stream in flight: data none, row unknown: 60
  - stream: chain closed (sequence 15 is a write whose graphs were not known): 10
  - stream: chain closed (sequence 4 is a write whose graphs were not known): 20
  - stream: chain closed (sequence 9 is a write whose graphs were not known): 30
  - txn in flight: data all, row unknown: 4
  - txn in flight: data none, row unknown: 51
  - txn: chain closed (sequence 10 changed <urn:crash:g:txn> in a way the log could not record): 10
  - txn: chain closed (sequence 4 is a write whose graphs were not known): 20
  - txn: chain closed (sequence 9 is a write whose graphs were not known): 30
  - orphans (run, before, MiB, after): [(0, 40, 20.42, 0), (1, 40, 20.42, 0), (2, 40, 20.42, 0), (3, 37, 17.97, 0), (4, 32, 11.1, 0), (5, 16, 14.91, 0), (6, 14, 13.35, 0), (7, 34, 15.6, 0), (8, 36, 15.05, 0), (9, 32, 11.1, 0), (10, 40, 20.42, 0), (11, 32, 14.83, 0), (12, 40, 20.42, 0), (13, 35, 16.47, 0), (14, 40, 20.42, 0), (15, 32, 11.11, 0), (16, 32, 14.83, 0), (17, 32, 11.11, 0), (18, 18, 7.69, 0), (19, 36, 13.84, 0), (20, 32, 14.83, 0), (21, 40, 20.42, 0), (22, 39, 18.86, 0), (23, 40, 20.42, 0), (24, 16, 14.9, 0), (25, 31, 10.85, 0), (26, 32, 11.11, 0), (27, 24, 16.75, 0), (28, 19, 7.97, 0), (29, 39, 17.83, 0), (30, 40, 20.42, 0), (31, 40, 20.42, 0), (32, 35, 16.47, 0), (33, 32, 11.1, 0), (34, 33, 11.11, 0), (35, 32, 11.11, 0), (36, 24, 9.27, 0), (37, 37, 16.34, 0), (38, 32, 11.11, 0), (39, 32, 14.83, 0), (40, 29, 10.75, 0), (41, 32, 14.83, 0), (42, 16, 14.91, 0), (43, 28, 10.05, 0), (44, 35, 13.4, 0), (45, 23, 14.41, 0), (46, 16, 14.91, 0), (47, 32, 14.83, 0), (48, 25, 9.52, 0), (49, 32, 11.11, 0), (50, 16, 14.91, 0), (51, 25, 16.74, 0), (52, 32, 14.83, 0), (53, 38, 17.83, 0), (54, 33, 15.34, 0), (55, 40, 20.42, 0), (56, 37, 19.62, 0), (57, 28, 10.05, 0), (58, 16, 14.91, 0), (59, 40, 20.42, 0)]

## item 3 mixed
- off 1: written 8400/s p95 389.0 ms; reads 47950/s p95 0.13 ms; shacl 1.00s; group_by 950 ms; log 
- off 2: written 8550/s p95 399.1 ms; reads 45225/s p95 0.14 ms; shacl 1.09s; group_by 1202 ms; log 
- off 3: written 9175/s p95 294.2 ms; reads 47543/s p95 0.13 ms; shacl 1.03s; group_by 966 ms; log 
- on 1: written 2400/s p95 1049.4 ms; reads 56461/s p95 0.11 ms; shacl 1.03s; group_by 1007 ms; log 6381568	/data/run/changes
- on 2: written 2300/s p95 1102.8 ms; reads 56530/s p95 0.11 ms; shacl 0.99s; group_by 939 ms; log 6111232	/data/run/changes
- on 3: written 4200/s p95 719.8 ms; reads 52519/s p95 0.12 ms; shacl 1.13s; group_by 938 ms; log 11100160	/data/run/changes
