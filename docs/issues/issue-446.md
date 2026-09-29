---
type: issue
state: closed
created: 2026-09-28T18:34:30Z
updated: 2026-09-28T23:30:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/446
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:33.685Z
---

# [Issue 446]: [fix(ingest): NIfTI `xyzt_units` spatial unit is ignored — a micron/metre affine is stamped `"mm"`](https://github.com/vig-os/tessera/issues/446)

Found while fixing #396 (the five NIfTI decoder bugs). It is the *same class* as #396 B4
(`world_frame.space` hard-coded `"scanner"`) but a different field, so it is filed separately rather
than widening that PR: like B3/B4 it **moves `manifest_hash` for affected inputs**, and #396's agreed
set of hash-moving changes was scoped in its issue text.

## The bug

`tessera/crates/tessera-ingest/src/nifti.rs` stamps the world unit unconditionally:

```rust
convention: "LPS".into(),
unit: "mm".into(),          // ← always, whatever the header says
```

NIfTI's `xyzt_units` byte (offset 123) carries the **spatial** unit in bits 0–2 —
`1 = NIFTI_UNITS_METER`, `2 = NIFTI_UNITS_MM`, `3 = NIFTI_UNITS_MICRON` — and the sform/qform affine
plus `pixdim[1..3]` are expressed in **that** unit, not in mm. #396 taught the decoder to read the
*time* half of the same byte (for the 4-D `t` axis's `time_regular` referencing); the space half is
still ignored.

So a metre-unit file (rare but legal) or a **micron-unit file (routine in preclinical / µCT / light-sheet
imaging, which is exactly where dcm2niix-adjacent tools emit NIfTI)** seals an affine whose numbers are
microns while `world_frame.unit` claims `"mm"` — a 1000× geometry error that every downstream consumer
inherits silently. There is no warning: the affine is non-degenerate, so `ArraySpec::validate` passes.

## Fix

Normalise at the door (ADR-0025), the way the RAS→LPS flip already does — keep `unit: "mm"` canonical
and **scale the affine** by the header's unit factor (`METER → ×1000`, `MM → ×1`, `MICRON → ×0.001`), so
the stored geometry means what it says. `"mm"` is in `CANONICAL_UNITS`; `"um"`/`"m"` are not, which is a
second reason to normalise rather than to carry the source unit through.

A `xyzt_units` space code of 0 (unspecified) should stay `"mm"` — that is the de-facto NIfTI default and
what every tool assumes — but is worth a `WARN`, since it is an assumption rather than a fact.

## Acceptance criteria

- [ ] A fixture with `xyzt_units` space `= 3` (micron) and `pixdim = (2,3,4)` seals a `world_frame` whose
      `spacing()` is `(0.004, 0.003, 0.002)` mm, not `(4,3,2)`.
- [ ] The metre case (`= 1`) scales by 1000.
- [ ] `= 2` (mm) and `= 0` (unspecified) are byte-identical to today — no hash moves for the common case,
      and the existing conformance fixture is unaffected.
- [ ] The qform path scales too (it reads the same `pixdim`).

## Pitfall

This moves `manifest_hash` **and** `content_hash`-adjacent metadata for micron/metre inputs. That is the
point of the fix, but it must be called out in the PR, and the #396 verification (no NIfTI product in the
committed conformance corpus) should be re-checked rather than assumed.

## References

- `tessera/crates/tessera-ingest/src/nifti.rs` (`world_frame_from_ras`, `parse_time_axis` already reads
  the byte's time half)
- #396 B4 (the sibling hard-coded-metadata bug) · ADR-0025 (normalise at the door) · ADR-0030 (named
  frames) · ADR-0032 (`CANONICAL_UNITS`)

