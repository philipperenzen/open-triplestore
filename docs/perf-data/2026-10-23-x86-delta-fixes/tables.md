# Delta-versions fixes, measured (2026-10-23)

Paired runs of develop at `e6f55c62` ("base") against the fix branch ("after"),
alternating run by run, every run on a fresh copy of a base store loaded with
capture off. Host, container limits and commands: `host.json`, `series.sh`.
Medians, ranges in brackets; × is capture on ÷ capture off unless stated.

## Item 5: checkpoint cut and builds (capture on, `restore … --skip-tier2`), n = 3

| | base | after |
|---|--:|--:|
| checkpoint cut, 0.9M quads | 43.4 s (43.2–44.1) | **15.6 s** (15.4–15.9) |
| checkpoint cut, 0.3M quads | 8.59 s (8.29–8.74) | 4.68 s (4.49–4.75) |
| 0.9M ÷ 0.3M (3× the data) | ×5.0 | ×3.3 |
| cold build forward (v2), 0.9M | 45.5 s (43.7–45.8) | 17.0 s (16.8–17.6) |
| cold build backward (v3), 0.9M | 45.2 s (44.6–45.3) | 17.2 s (17.2–21.3) |
| delta cuts (k = 1 000) | 7–11 ms | 6–10 ms |
| tier-1 restore (hash check dominates) | 2.95 s (2.95–3.56) | 3.30 s (2.94–3.79) |

## Items 2 and 9: `bulk_insert_quads`, 1M quads into a new named graph

Separate processes per side (`bulk 111112 off|on`), n = 3:

| | off | on | × |
|---|--:|--:|--:|
| base | 9.96 s (9.90–10.07) | 11.21 s (11.13–11.31) | ×1.13 |
| after | 9.87 s (9.81–9.89) | 10.29 s (10.14–10.33) | **×1.04** |

In one process, off and on alternating (`probe 1000000 5`), n = 5:

| path | base off | base on | × | after off | after on | × |
|---|--:|--:|--:|--:|--:|--:|
| `bulk_insert_quads` | 11.87 s | 13.60 s | ×1.15 | 11.83 s | 12.64 s | **×1.07** |
| named-graph load (probes either way) | 13.62 s | 14.16 s | ×1.04 | 13.98 s | 14.61 s | ×1.05 |

## Item 3: the mixed phase (`scale_otl 100000 --skip-load`, 4 writers, 4 readers, 20 s), n = 3

| | base off | base on | × | after off | after on | × |
|---|--:|--:|--:|--:|--:|--:|
| quads written per second | 5 200 (4 975–5 325) | 2 300 (2 175–2 350) | ×0.44 | 5 200 (4 900–5 300) | 5 125 (4 900–5 250) | **×0.99** |
| write p95 | 554 ms (515–571) | 1 048 ms (994–1 178) | ×1.89 | 598 ms (509–689) | 517 ms (491–558) | ×0.86 |
| reads per second | 56 571 | 61 086 | ×1.08 | 55 706 | 55 190 | ×0.99 |

A first round (`raw/superseded/`) measured the branch at `c4f241f1`, before
bulk ingestions shared the section: capture on 2 400 quads/s against 5 325 off,
×0.45, no better than base. An instrumented build then timed one write's
phases (four writers, capture on): 147 ms staging the SST files, 583 ms waiting
for the section, 248 ms holding it — the ingestion alone — and 1 ms finalising
its rows. With one writer the ingestion took 202 ms, about what it takes with
capture off, where RocksDB runs four of them side by side.

## Criterion, `concurrent/{,capture_}writes/threads/4` (`--features full`), two passes alternating

| | pass 1 | pass 2 |
|---|--:|--:|
| base `concurrent/writes` | 831 µs | 848 µs |
| base `concurrent/capture_writes` | 2.58 ms | 2.49 ms |
| after `concurrent/writes` | 833 µs | 834 µs |
| after `concurrent/capture_writes` | 2.60 ms | 2.65 ms |

Not changed (×3.1 on both sides): the benchmark's writes are in-memory
`INSERT DATA` transactions, whose commit is cheap; what they queue on is the
log's one SQLite connection (the intent row and the finalisation of every
write), not the section.

## Item 7: `kill -9` (after only), 60 kills, a fresh store every 10

No violations, run twice (after `c4f241f1` in `raw/superseded/`, after the
shared section in `raw/`): every batch whole or absent, no lying row, dense
sequence numbers, every intact chain replays to its graph.
