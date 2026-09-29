---
type: issue
state: closed
created: 2026-07-03T09:46:41Z
updated: 2026-09-28T23:30:35Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/325
comments: 2
labels: bug, priority:high
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:49.596Z
---

# [Issue 325]: [perf(ingest): hdf-compound STREAMING path ~68× slower than batch (7min vs 6s for events_3p)](https://github.com/vig-os/tessera/issues/325)

## Context — DUPLET first-user run (ADR-0051 HDF5 decomposition)

Ingesting `/proc_data/events_3p` (10 M rows, **447 MB**) from the DP01 `.h5` took **6 m 59 s**:

```
real  6m59s   user  0m28s   sys  0m43s
```

user+sys = ~72 s of CPU, but **~6 min of wall-clock is I/O wait** — the reader is disk-I/O-bound at **~1 MB/s**, not compute-bound. Nested-path resolution itself works (`--dataset /proc_data/events_3p` succeeds), so this is purely throughput.

## Impact
At ~1 MB/s the ADR-0051 per-table decomposition is impractical for the big tables:

| table | size | projected time @1 MB/s |
|---|---|---|
| events_2p | 5 GB | ~1.4 h |
| coin_3p | 7 GB | ~2 h |
| coin_2p | 18 GB | ~5 h |
| **singles** | **81 GB** | **~22 h** |

So only the small tables (events_3p, time_markers, coin_counters) are ingestable in reasonable time; the giants block on this.

## Likely causes to investigate
- The generic compound reader parses **row-by-row** (`slab_to_columns`) — 447 MB / 10 M rows ≈ 45 µs/row; for 7.9 B `singles` rows that's ~90 h of parse alone.
- Reading a mid-file HDF5 compound dataset off a **spinning HDD** with a strided/chunked layout → many seeks. Measure: is it the parse loop or the H5Dread?
- The batch path reads the whole dataset into RAM first; the streaming path (`ADR-0026`) may or may not help throughput (it helps memory).

## Asks
- Profile read vs parse vs encode on `events_3p`; identify the ~1 MB/s bottleneck.
- Bulk-read the compound (large hyperslabs) + vectorized column transpose instead of per-row.
- Confirm the streaming path's throughput on a giant; document expected wall-clock for `singles`.

Found during: DUPLET DP01 listmode decomposition (ADR-0051). Blocks full-scale #305.
---

# [Comment #1]() by [gerchowl]()

_Posted on July 3, 2026 at 09:52 AM_

## Correction — it's the STREAMING path, not disk (my first read was wrong)

Re-measured, same `/proc_data/events_3p` (10 M rows, 469 MB):

| path | time | throughput |
|---|---|---|
| raw `h5py` read (`d[:]`) | **0.5 s** | **897 MB/s** |
| tessera ingest, `streaming = "batch"` | **6.2 s** | ~75 MB/s (read+transpose+Vortex) |
| tessera ingest, `streaming = "auto"` → **stream** | **7 min** | ~1 MB/s |

So: the **disk is fine (897 MB/s)**, **batch is fast (6 s)**, and the **streaming path is ~68× slower**
(7 min). My earlier "disk-I/O-bound / row-by-row parse" diagnosis was wrong — the batch path proves the
parse+encode is ~6 s. The regression is entirely in the **streaming slab reader** (`stream_compound` /
`STREAM_SLAB_ROWS` hyperslab pattern → many small `H5Sselect_hyperslab`+`H5Dread` calls, ~68× overhead
vs one bulk read). And `streaming = "auto"` engages it for **any dataset > 256 MB**, so it bites every
non-trivial table by default.

### Revised impact
- Batch-ingestable (fits RAM): `events_2p` 5 GB, `coin_3p` 7 GB, `coin_2p` 18 GB — minutes each at ~75 MB/s.
- `singles` 81 GB needs streaming (or ≥81 GB RAM) → blocked until the streaming slab reader is fixed.

### Fix
Make `stream_compound` read **large hyperslabs** (e.g. tens of MB / hundreds of k rows per H5Dread),
not `STREAM_SLAB_ROWS`-sized micro-reads; and/or raise the auto threshold. Target: streaming within ~2×
of batch. Add a throughput regression test.

---

# [Comment #2]() by [gerchowl]()

_Posted on July 3, 2026 at 10:02 AM_

## Correction #2 — the real bug is the DEFAULT worker count, and this host has 2 TB RAM

Same `events_3p`, forced `streaming = "stream"`:

| run | time |
|---|---|
| default (no knobs) | **7 min** |
| `--workers 8` | **45 s** |
| `--auto` | **41 s** |
| `streaming = "batch"` | **6 s** |

So streaming isn't fundamentally broken — with `--workers`/`--auto` it's ~40 s (and the content_hash is
**identical** to batch, determinism intact). The pathological 7 min was **default worker count ≈ 1**:
the `ingest --spec` / `ingest ge-hdf5` path does **not** default to `available_parallelism()` for the
streaming encode pool unless you pass `--workers`/`--auto`. That's the bug — **streaming ingest should
default to a sane worker count**, not 1.

Batch is still ~7× faster than streamed-with-workers (6 s vs 40 s) — worth a look — but not urgent.

### Host context (revises the "giants impractical" claim entirely)
This box has **2.0 TiB RAM (1.9 TiB free) + 88 cores**. So `singles` (81 GB), `coin_2p` (18 GB) all
fit in RAM → **`streaming = "batch"` ingests every table** at ~75 MB/s (singles ≈ 18 min). No streaming
required here at all. My "22 h / hours" projections were wrong on every axis.

### Net asks (unchanged core, re-prioritized)
1. **Streaming should default to `available_parallelism()` workers** (the actual bug — a fresh
   `ingest` with no flags shouldn't be 68× slow).
2. (lower) batch vs streamed-with-workers is still 7× — investigate the slab pipeline overhead.

