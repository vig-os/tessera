---
type: issue
state: open
created: 2026-09-29T02:35:19Z
updated: 2026-09-29T02:35:19Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/485
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:25.464Z
---

# [Issue 485]: [bench(ecosystems): backport median+spread, cold-cache and seal/verify into the #143 cross-ecosystem harness](https://github.com/vig-os/tessera/issues/485)

## Context

`tessera/bench/ecosystems/` (#143) compares `.tsra` against HDF5, Zarr, NeXus, NIfTI, DICOM, ROOT and
Parquet through one driver, with real-library adapters and bit-exact read-back asserted before any
timing counts. It is the broadest comparison we have.

`tessera bench compare` (#388) is being built to the stricter methodology the AX pitch needs. Three of
those improvements are not specific to the two formats #388 covers and would make the 7-format
comparison better too. Filed separately so #388 stays scoped.

## Ask

- **Median + spread instead of min-of-N.** `common.best()` takes the minimum wall-clock over N runs
  ("the least-contended run on a busy box"). That is a defensible choice for a throughput ceiling, but
  it hides variance and cannot show when two formats are within noise of each other. Report the median
  with a spread (min/max, or IQR) and the N, as #388 does.
- **Cold-cache reads.** The harness is warm-only by explicit design, so it measures decode+parse
  throughput rather than what a first read costs. Add a cold variant via
  `posix_fadvise(POSIX_FADV_DONTNEED)` per file before the timed run — no root needed, unlike
  `drop_caches` — and label every row warm or cold. Note that the eviction is best-effort.
- **Seal / verify cost.** Nothing currently times what integrity costs. Add tessera's seal and
  `verify` alongside the closest mechanism each other format offers (HDF5's `fletcher32` filter,
  Parquet's page CRCs), and state precisely what each does and does not prove — corruption detection
  is not whole-file tamper-evidence.

## Not in scope

Changing the datasets or the adapter contract. The point is to strengthen the method on the existing
matrix, not to re-litigate what it measures.

## References

- #388 (`tessera bench compare`, where this methodology lands first) · #143 (the harness) · #390 (the
  AX audit that asked for numbers)
- `tessera/bench/ecosystems/common.py` (`best()`, the warm-only note), `run.py`

Refs: #388
