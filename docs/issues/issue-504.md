---
type: issue
state: open
created: 2026-09-29T07:10:59Z
updated: 2026-09-29T07:10:59Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/504
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:22.111Z
---

# [Issue 504]: [feat(cli): bench compare --input — run the head-to-head on your own HDF5 acquisition](https://github.com/vig-os/tessera/issues/504)

## Why

`tessera bench compare` (#388) measures synthetic fixtures. The most credible version of the pitch is
one a reader can check on **their own** acquisition, so the comparison should accept an HDF5 compound
dataset as input.

A first cut was written during #497 and **pulled before merge** because it only worked for the
benchmark's own 3-column shape. Recording what it has to handle so the next attempt is not
half-working either.

## What it must do

- **Arbitrary numeric columns.** A real GE `/events_2p` is `ms u32`, `en_* f32`, `ax_* u8`,
  `tx_* u16`, `vtx_* f32`. The pulled version assumed `u64 t` + two `f32` and would have failed on
  every one of those: `expect_rows` rejects `u8`/`u16`, the destructuring asserts `t must be u64`,
  and the projected-column read is hard-coded to a column named `e0`.
- **The SAME column set written to every format.** The pulled version would have handed Tessera all N
  columns while the HDF5 side wrote only three — the exact asymmetry the #487 review caught in the
  row-ROI comparison. Whatever is read must be what was written, for every format.
- **A projected column chosen from the data**, not a hard-coded name.
- **A test** with a small synthetic multi-dtype HDF5 input (u8/u16/u32/f32 together), so the dtype
  coverage is proven rather than assumed.
- No PHI in anything committed: the file is read in place, and only synthetic fixtures are ever
  checked in.

## Not urgent

The two synthetic fixtures already span the informative range (periodic/adversarial and
continuous/listmode-like), and #493 published the real-data numbers separately. This makes the claim
self-serve; it does not change what the claim is.

Refs: #388, #497, #493
