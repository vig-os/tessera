---
type: issue
state: open
created: 2026-09-08T20:13:15Z
updated: 2026-09-28T17:59:26Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/419
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:36.528Z
---

# [Issue 419]: [Array blocks: complex64/128 for MRI k-space — plane-split vs interleave storage decision](https://github.com/vig-os/tessera/issues/419)

## Context

#418 closed the array dtype envelope at the numpy fixed-width numeric ladder + bool: `i1..i8`, `u1..u8`, `f2/f4/f8`, `b1` (sub-16-bit via transparent widening). `complex64/128` was **deliberately excluded** there because it is not a widening — it is a storage-design fork that deserves its own decision.

## The case

MRI raw k-space is canonically `complex64` (also: spectroscopy FIDs, diffraction patterns). If tessera is to hold MRI acquisitions pre-reconstruction, arrays need a complex story. numpy `c8`/`c16` round-trips would make `np.load`-style migration trivial.

## The fork

pcodec has no complex support; two representations, with different compression behaviour:

1. **Plane-split** — store `re` and `im` as two separate f32 sub-arrays inside the block's zarr store (the store already holds multiple keys). Each plane is smooth in the way pcodec exploits; likely the best ratio. Spec dtype `complex64`, reader interleaves on decode.
2. **Interleave** — a trailing axis of size 2 (`[..., 2]` f32). Simpler, shape-visible, but alternating re/im samples break pcodec's delta model → worse ratio.
3. (zarrs `data_type::complex64` exists — but that only names the dtype; the codec question above remains.)

Measure both on a real k-space volume before deciding — the #412/#418 lesson is that compression intuitions need numbers.

## Constraints from the settled conventions

- Determinism gate: the chosen layout must be byte-stable across arch/build (both candidates are — pure data reshuffles).
- The widened-dtype convention (#418) already establishes 'payload storage dtype ≠ logical spec dtype' — plane-split fits the same pattern (spec says `complex64`, store holds two f32 arrays).
- `ArrayData::C64(Vec<Complex32>)` — `num-complex` is already in-tree via zarrs.
- Conformance corpus needs a fixture with specials (NaN/inf components) once landed.

## Non-goals

`bfloat16` (not numpy-native) and `datetime64` (= i8 + epoch/unit metadata) stay excluded — documented in SPEC.md §array-dtypes.

Refs: #418
