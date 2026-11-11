---
type: issue
state: open
created: 2026-11-11T03:05:18Z
updated: 2026-11-11T03:36:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/826
comments: 2
labels: feature, priority:medium, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:29.717Z
---

# [Issue 826]: [perf(io): .tsra writer is 2.4–4.5× slower per core than HDF5 shuffle+zstd on real list-mode and scales only ~2× to 8 cores](https://github.com/vig-os/tessera/issues/826)

## Description

On real vendor list-mode, the `.tsra` table writer is **2.4–4.5× slower per core than HDF5 shuffle + zstd**. It scales only 1.6–2.1× from 1 to 8 cores. "Write raw, then `zstd -T`" reaches 49–57 M events/s at the same 8 cores (58 M on all 32 hardware threads), versus 10.8–12.7 M events/s for `.tsra`, a 4.5× gap. That row is **not** a competing format: it is an unstructured byte stream compressed afterwards, with no columns, no index or random access, and no seal, and it compresses slightly worse (3.44 vs 3.58 on GE). It is the ceiling for a "hot path writes flat, compress or encode afterwards" design. For live list-mode (a Siemens Quadra or 10× activity), the writer's per-core rate decides how many cores acquisition has to reserve. It also matters for simulation producers (gPET-sim writes ~2 M events per stage per run).

## Measurements

**Setup:**
- Ryzen 9 5950X (16 cores / 32 threads), rootless podman, `taskset` pinning, warm cache, `/usr/bin/time -v`.
- Median of 3 reps; every output verified bit-exact.
- `.tsra` = tessera `6a3078079` `TableMultiBlockSink` + `StreamWriter`.
- Profiles: single = 1 core; multi = 8 physical cores / 16 hardware threads; full = 0–31.
- Ratio = raw / compressed bytes.

GE Discovery MI Gen2, 54,161,949 real events (5 × int16 + `t_ms` u4, 758 MB raw):

| Profile | Writer | Wall (s) | M ev/s | CPU (s) | Peak RSS (MiB) | Ratio |
|---|---|---:|---:|---:|---:|---:|
| single | `.tsra` default | 8.11 | 6.68 | 6.73 | 66 | 3.58 |
| single | h5py shuffle + zstd | 3.39 | 15.98 | 3.35 | 55 | 3.74 |
| single | Parquet zstd | 4.29 | 12.63 | 4.22 | 82 | 2.82 |
| single | raw then `zstd` | 4.44 | 12.20 | 4.10 | 47 | 3.44 |
| multi | `.tsra` default | 5.01 | 10.81 | 9.81 | 412 | 3.58 |
| multi | h5py shuffle + zstd | 3.29 | 16.46 | 3.25 | 55 | 3.74 |
| multi | raw then `zstd -T` | 1.10 | 49.24 | 5.33 | 262 | 3.44 |
| full | `.tsra` default | 4.94 | 10.96 | 8.39 | 451 | 3.58 |
| full | raw then `zstd -T` | 0.93 | 58.24 | 4.51 | 412 | 3.44 |

Siemens Biograph Vision, 55,004,119 real events (11 columns, 825 MB raw):

| Profile | Writer | Wall (s) | M ev/s | CPU (s) | Ratio |
|---|---|---:|---:|---:|---:|
| single | `.tsra` default | 8.96 | 6.14 | 8.16 | 2.00 |
| single | h5py shuffle + zstd | 1.97 | 27.92 | 1.90 | 2.13 |
| multi | `.tsra` default | 4.34 | 12.67 | 11.38 | 2.00 |
| multi | raw then `zstd -T` | 0.96 | 57.30 | 3.46 | 2.08 |

## Observations

- **CPU per event:** `.tsra` spends ~2× (GE) to ~4× (Siemens) the CPU of HDF5 shuffle + zstd, for a slightly *worse* ratio. The cost is in the encode path (BtrBlocks scheme sampling plus Pco), not in I/O. `raw_bin` alone writes 85–95 M ev/s.
- **Scaling:** `.tsra` uses 9.8–11.4 s of CPU for 4.3–5.0 s of wall time at 8 cores. The parallel speedup is 1.6–2.1×, so something serialises: possibly the single committer, the per-fragment fsync (#655) or the scheme search per block.
- **Memory:** peak RSS rises from 66 MiB to 412–704 MiB in multi. That is within the 1 GiB default window but much more than HDF5's ~55 MiB.

## Questions for the architect

1. Profile the single-core encode. How much of the time goes to scheme sampling versus Pco versus hashing (blake3)? Could a fixed per-column scheme cache for a product type (decided once on the first block) cut the sampling cost while staying deterministic?
2. What stops scaling beyond ~2×: committer, fsync or channel? Does batch mode (#655) change it?
3. Should a live "hot" profile write flat columns (raw or shuffle + zstd-1, still sealed and column-addressable), with the full encode done afterwards in the background or by the archival re-encode (#796)? Writing raw alone does 85–95 M ev/s on one core, and raw → `zstd -T` 49–57 M ev/s at 8 cores; that is the headroom such a profile could aim for.

## Fixtures

The same internal fixture set as #822 (`tessera-vendor-2026-11`, not public: ask the maintainer). The writer harness is the bench's `bench/writer/` (frozen tree `1a1ef75c…`), available on request.

## Related

#655 (batch mode, no per-fragment fsync), #681 (Vortex writer holds segments until the footer), #802 (deterministic bytes regardless of producer-thread scheduling), #796 (archival profile), #822 (size gap).

## Changelog Category

Changed

---

# [Comment #1]() by [gerchowl]()

_Posted on November 11, 2026 at 03:11 AM_

**Breakdown of the gap (from CPU / wall in the tables above).** The raw → `zstd` wall is the *sum* of writing raw and compressing it. A live encoder skips the intermediate write and re-read, so at equal codec and cores it should be **at least as fast**. The 4.5× gap is therefore entirely tessera-specific, and it splits into two factors:

| GE, 54 M events | Wall (s) | CPU (s) | Cores busy (CPU / wall) | Ratio |
|---|---:|---:|---:|---:|
| `.tsra`, 1 core | 8.11 | 6.73 | 0.8 | 3.58 |
| raw → zstd, 1 core | 4.44 | 4.10 | 0.9 | 3.44 |
| `.tsra`, 8 cores | 5.01 | 9.81 | **≈ 2.0** | 3.58 |
| raw → `zstd -T`, 8 cores | 1.10 | 5.33 | **≈ 4.8** | 3.44 |

1. **~1.8× per-core encode cost (codec choice):** BtrBlocks scheme sampling plus Pco cost 6.7 CPU-s, against 4.1 CPU-s for zstd. On GE this buys a slightly better ratio (3.58 vs 3.44). On Siemens it doesn't (2.00 vs 2.08), so more CPU for less.
2. **~2.5× parallelism:** with 8 cores pinned, the writer keeps only ~2 busy, while `zstd -T` keeps ~5 busy. The encode workers are starved or serialised downstream. Candidates are the single producer that builds and pushes `TableData`, the in-order committer, and the per-fragment fsync (#655). This factor is independent of the format and should be fixable.

1.8 × 2.5 ≈ 4.5. Target: a hot profile with a zstd-class codec per column and saturated workers should match or beat raw → `zstd -T` (≈ 49–57 M ev/s at 8 cores) while staying columnar and sealed. The dense encode moves to the archival re-encode (#796).


---

# [Comment #2]() by [gerchowl]()

_Posted on November 11, 2026 at 03:36 AM_

Measured follow-up in #828: a per-column codec sweep on the same fixtures, with every round trip verified. Plain Pco per column gives 3.18 B/ev on GE (`.tsra` 3.91) and 6.84 B/ev on Siemens (`.tsra` 7.51). The best codec per column gives 3.09 and 6.83; Siemens is then at its order-0 entropy. A fast fixed profile gives 3.15 / 6.89 B/ev at ≈25 / 49 M ev/s single-core codec time (indicative). Row-group size is not the cause (+0.2–0.3 %), so the gap points at scheme selection.

