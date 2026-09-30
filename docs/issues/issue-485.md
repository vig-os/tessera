---
type: issue
state: closed
created: 2026-09-29T02:35:19Z
updated: 2026-09-29T19:13:43Z
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
synced: 2026-09-30T07:57:12.602Z
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

---

## Scope amendment (2026-09-29)

The **Not in scope** section above is superseded in two specific ways. It was written before the
#487 / #497 / #503 reviews, and those reviews established the reasons:

1. **The adapter contract DOES change** — each adapter now exposes `VARIANTS` (a sensible default
   *and* its standard tuning), each with a printed settings string. This is required by the rule that
   every format be shown at both settings; it cannot be satisfied by the current one-`CODEC`-per-adapter
   contract.
2. **A second table fixture IS added** — #497's *continuous* (listmode-like) fixture alongside the
   existing *periodic* one, both reported, each labelled.

### Why these are not optional, and why they must land together

An audit of the harness found it already carries the fairness omission that #487 and #503 each caught
once, in three places — all favouring Tessera on size:

- `adapters/hdf5.py:40`, `:67` — `shuffle=False` with gzip-4, on both volume and table. #487 measured
  this: adding shuffle took HDF5's tuned table 5.9 MiB → 155.3 KiB and inverted the headline.
- `adapters/parquet.py:45` — `compression="zstd"` with no BYTE_STREAM_SPLIT and the dictionary left on.
  #503 measured this: 39.3 → 23.4 MiB, turning a claimed 2.0× win into 1.19×.
- `adapters/zarr_.py:48`, `:76` — bare `ZstdCodec`, no shuffle.

**And the existing table fixture is #497's periodic one** (`common.py:55-56`, `% 7` and `% 5`) — the
fixture established as adversarial for value-distribution codecs, where HDF5 shuffle+gzip wins 6.2×.

So the harness today pairs a fixture that favours deflate with a deflate configuration that has
shuffle disabled. **The two errors partially cancel.** Fixing the shuffle omission alone would swing
the table hard toward HDF5 on a fixture already chosen against us; adding the continuous fixture alone
would swing it back. Either single fix relocates the bias rather than removing it — so no commit or
published result in this work may show shuffle-fixed, periodic-only numbers.

That is the scope argument: this is not a methodology upgrade to correct numbers, it is a correction
to numbers that are wrong now, and the correction is only sound if both halves land together.

### Also corrected here

#503 recorded that Parquet page CRCs could not be timed because "parquet-rs 58's writer emits none".
That is a property of **parquet-rs**, not of Parquet. Verified in this harness's environment:

```
pyarrow 24.0.0 | write_page_checksum in write_table: True
                 page_checksum_verification in read_table: True
```

So the integrity row here **can** measure Parquet, and will. The output states both facts — parquet-rs
writes none, pyarrow writes and verifies them — so the asymmetry is attributed to the implementation
rather than to the format.

Refs: #388

