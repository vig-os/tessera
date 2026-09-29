---
type: issue
state: open
created: 2026-09-29T05:40:39Z
updated: 2026-09-29T05:53:48Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/494
comments: 1
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:24.241Z
---

# [Issue 494]: [perf(table): evaluate a run-end integer scheme for high-repeat columns (real-data justified only)](https://github.com/vig-os/tessera/issues/494)

## The observation

From the #493 investigation, on **real** DUPLET listmode (`/events_2p`, 4M rows), one column is
meaningfully worse than HDF5 shuffle+gzip:

| column | raw | shuffle+gzip4 | tessera | tessera vs gzip |
|---|---|---|---|---|
| `ms` | 15.3 MiB | **49.2 KiB** | 101.0 KiB | **0.49×** |

`ms` is a coarse millisecond clock over 4M events: monotonic, **3 distinct deltas, 99.6% of them
zero** — i.e. long runs of an identical value. That is a textbook run-end / RLE shape, and it is the
only real-data column where tessera loses by more than noise (everything else is 0.99–1.07×, and the
file overall is 1.03× *smaller* than shuffle+gzip).

## What is NOT the answer

**Delta + bit-packing would be far worse** — 976.6 KiB, because 2 bits/value × 4M rows beats nothing
when 99.6% of the deltas are already zero. Measured, not assumed.

## Why this is low priority

The deficit is **52 KiB out of 73.4 MiB — 0.07% of the file**. Any encoder change is a
**format/determinism event**: it must pass the both-profiles gate (#474), keep scheme selection a
pure function of the data, hold cross-arch byte-identity, and must not move dev goldens without an
owner decision. That cost is not obviously repaid by 0.07%.

## If it is ever picked up

- Justify it on **real high-repeat columns**, never on a synthetic fixture — #493 is precisely the
  cautionary tale of a fixture whose result did not transfer.
- Check whether a run-end/RLE integer scheme is already reachable in the btrblocks scheme set and
  simply not selected for this shape, before adding anything new.
- Measure the read-path cost too: a run-end column changes random-access and projection behaviour,
  and `decode_rows` / `decode_column` latency matters more than 52 KiB.
- Confirm it does not regress the columns that currently win (`ms` is 2213× on the synthetic pure
  sequence via `vortex.sequence`, so whatever changes must not disturb that selection).

Refs: #493, #487
---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 05:53 AM_

Scheme introspection from #493 narrows this: the `ms` column is currently **`vortex.dict(u32)`**
(119.81 kB before the container), not a run-end scheme.

Dict is defensible for 3 distinct values, but it stores a code per row and therefore cannot exploit
the **runs** — and 99.6 % of this column's deltas are zero. Deflate collapses exactly those runs,
which is why shuffle+gzip reaches 49.2 KiB where tessera seals 101.0 KiB.

So the question for this issue is narrower than "add a codec": **is a run-end/RLE integer scheme
already selectable in the btrblocks set, and simply not chosen over dict for this shape?** If so this
may be a selection question rather than a new encoding — but it is still a format/determinism event,
and still only worth 0.07 % of the file.

