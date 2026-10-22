# SHACL run memory budget: before and after (2026-10-15, x86)

Issue [#532](https://github.com/philipperenzen/open-triplestore/issues/532).
Host: AMD Ryzen 9 7900X3D, Docker Desktop on WSL 2 (`host.json`); inputs and
images in `inputs.json`. Every run: an empty store, the BSBM data loaded over
the Graph Store protocol, a 60 s settle, one warm-up and five measured
`POST /api/datasets/bench/validate?test=true`, in a container limited to
4 GiB and 4 CPUs. Peaks per phase: the `docker stats` figure (usage minus
inactive file cache) and, from the cgroup's `memory.stat` sampled twice a
second, anonymous memory and active file cache (`runs/*.json`,
`summary.json`). Before = `462b206c` (develop, 0.8.0), after = `2342e3f9`.

## What the peak was made of (before, first run of `base-10k-1`)

From `memstat-shacl-base-10k-1.csv` aligned with the server's SHACL log lines:

| Moment | Anonymous memory (MiB) |
| --- | --: |
| Before the run (server, RocksDB, what the load left) | 1,315 |
| After the class sets and focus lists | about 1,600 (+290) |
| After the run index (1,791,365 quads) | 2,657, rising to 2,833 as the last scans finish (+1,100) |
| End of evaluation (51,690 results) | 2,863 (+30) |
| Report converted to a JSON value tree and written | 2,962 (+170 over what the run had freed) |
| End of the sixth run | 3,573 |

The anonymous memory, not the page cache, made the peak: active file cache
stayed at 500–650 MiB. The run index was the largest single part (about
600 bytes a quad against the 300 its quad cap assumed). Nothing was given back
between runs: each of the six runs started from the high-water mark of the
one before, 2.8 GiB after the first.

## Peak memory and validation time

| Run | Peak, docker stats (MiB) | Peak, anonymous (MiB) | Active file (MiB) | Median of 5 (s) | Results | Results digest |
| --- | --: | --: | --: | --: | --: | --- |
| before, 10k #1 | 4,080 | 3,573 | 523 | 7.84 | 51,690 | `1c6b8ff25f98` |
| after, 10k #1 | 2,931 | 2,399 | 499 | 6.89 | 51,690 | `1c6b8ff25f98` |
| before, 10k #2 | 3,734 | 3,279 | 509 | 7.50 | 51,690 | `1c6b8ff25f98` |
| after, 10k #2 | 2,881 | 2,350 | 499 | 7.37 | 51,690 | `1c6b8ff25f98` |
| before, 1k | 1,082 | 1,047 | 22 | 0.158 | 5,245 | `239d2d5acec7` |
| after, 1k | 959 | 927 | 22 | 0.137 | 5,245 | `239d2d5acec7` |

The ten measured 10k runs of each build: median 7.60 s before, 6.98 s after.
The digest covers the content of every result (canonical JSON, sorted, blank
node labels left out): the same report before and after, at both scales.

After, each 10k run logs its account, e.g. (`runs/new-10k-1-10k.json`):

```text
SHACL run memory: budget 1024 MiB; class sets and targets 109 MiB, index 605 MiB (1444351 quads, 4 pairs from the store), closure memo 0 MiB, results 177 MiB (51690); process anon MiB after start 1245, class sets 1481, targets 1520, index 2246, results 2438
```

The index held 1.40–1.59 M quads in an estimated 587–657 MiB (three or four
(graph, predicate) pairs answered from the store; which ones depends on the
order the parallel scans finish in); giving the freed memory back took
157–386 ms at the end of each run.

## The repeated-load check

`campaign.py --stores ots --scales 10k --workloads shacl --repeat 3 --max-mem-fraction 0.8`
(`repeat-check-new-10k.json`; the before image with `--repeat 1`,
`repeat-check-base-10k.json`):

| Build | Attempt | Load peak (MiB) | SHACL peak, docker stats (MiB) | SHACL peak, anonymous (MiB) | Verdict (80 % = 3,277 MiB) |
| --- | --: | --: | --: | --: | --- |
| after | 1 | 1,710 | 3,018 | 2,486 | pass |
| after | 2 | 1,853 | 2,914 | 2,386 | pass |
| after | 3 | 1,735 | 3,079 | 2,386 | pass |
| before | 1 | 1,920 | 4,021 | 3,509 | fail |

The three attempts after the change gave the same answer (51,690 results, one
digest).
