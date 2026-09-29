---
type: issue
state: open
created: 2026-09-28T20:18:38Z
updated: 2026-09-29T06:53:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/459
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:29.970Z
---

# [Issue 459]: [fix(core): `WorldFrame::is_nondegenerate` accepts NaN and parallel-column affines — promote the NIfTI conditioning check](https://github.com/vig-os/tessera/issues/459)

Raised by the independent review of PR #447 (#396). The NIfTI decoder now carries a **local** stricter
check; this issue is about whether the **format-wide** gate should be tightened to match, which is a
format decision rather than a decoder bug.

## The gap

`WorldFrame::is_nondegenerate` (`tessera/crates/tessera-core/src/block/array.rs`) is the ADR-0030
post-Accept gate that `ArraySpec::validate` enforces on every array product:

```rust
pub fn is_nondegenerate(&self) -> bool {
    self.spacing().iter().all(|&s| s > 0.0)
}
```

`spacing()` is the three column norms, so the gate accepts two affines it should not:

1. **Non-finite entries.** A `NaN` column norm compares `false` against `> 0.0`, so a NaN affine is
   reported as *degenerate* rather than as corrupt — the two get the same treatment, although one is
   "there is no usable geometry here" and the other is "these bytes are damaged". An `inf` entry gives
   an infinite norm, which passes `> 0.0` outright.
2. **Parallel columns.** Three columns can each have a healthy non-zero norm while being linearly
   dependent — e.g. `[[2,2,0,…],[0,0,0,…],[0,0,4,…]]`. The determinant is 0, so there is no invertible
   voxel→world map, but every norm is non-zero and the gate passes. A sealed product can therefore
   carry an affine that no consumer can invert.

## What PR #447 did, and why it stopped there

`nifti.rs` grew two local helpers rather than changing core:

- `affine_is_finite` — a non-finite sform/qform is now a **typed error**, not a fall-through, because
  "unreadable" is a different claim from "absent".
- `affine_conditioning` — `|det| / Π‖column‖`, which is **scale-invariant**: 1 for an orthogonal frame,
  → 0 as columns become parallel. That matters because an absolute `det` threshold would reject a
  legitimate micron-spacing affine (`det` ~1e-9) while accepting a nearly-parallel metre-scale one.
  Singular-but-finite falls through to the qform; unusable with nothing to fall back to is an error.

Tightening `is_nondegenerate` itself was deliberately **not** done in that PR: it is the gate every array
product passes through, so narrowing it changes what DICOM, raw, pyramid and deformation-field products
are allowed to seal. That is a format decision needing its own review, and it risks rejecting products
that seal today.

## Decision to make

- [ ] Should `is_nondegenerate` require every entry **finite**? (Very likely yes — a NaN affine is never
      meaningful, and today it is merely mislabelled rather than caught.)
- [ ] Should it require **non-singular**, via the scale-invariant conditioning ratio? (Probably yes, with
      the epsilon stated in ADR-0030 so it is a format constant rather than a decoder's private choice.)
- [ ] If both, promote `affine_conditioning` from `nifti.rs` into `WorldFrame` so there is **one**
      definition, and have the NIfTI decoder call it — DRY, and the epsilon stops being per-decoder.
- [ ] Audit whether any existing product in `tessera/corpus/` would newly fail. Expected: none (the
      corpus affines are diagonal), but it must be checked, not assumed — a newly-failing corpus entry
      would make this a corpus event.
- [ ] Decide whether a singular affine should be **rejected at seal** or **accepted and flagged**. Reject
      is consistent with ADR-0030 already gating non-degeneracy; but note it turns a previously-sealable
      product into an error, so it is a breaking validation change (pre-1.0, so acceptable — state it).

## References

- `tessera/crates/tessera-core/src/block/array.rs` — `WorldFrame::is_nondegenerate`, `spacing`,
  `ArraySpec::validate`
- `tessera/crates/tessera-ingest/src/nifti.rs` — `affine_is_finite`, `affine_conditioning`,
  `affine_is_usable` (the local versions to promote)
- ADR-0030 (named frames / the non-degeneracy gate) · PR #447 · #396

