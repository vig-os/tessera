---
type: issue
state: closed
created: 2026-08-19T14:15:30Z
updated: 2026-09-28T21:22:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/396
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:40.449Z
---

# [Issue 396]: [fix(ingest): five NIfTI decoder bugs — panic on malformed vox_offset, .nii.gz, qform, hard-coded space, silent 4-D truncation](https://github.com/vig-os/tessera/issues/396)

Found by reading the as-built decoder during the #386 generic-ingest spike (recorded in ADR-0056 §11).
These are **bugs in shipped code**, not open design questions, so they are filed separately from #386 —
they should be fixable and mergeable without waiting on the generic-ingest ADR.

All five are in `tessera/crates/tessera-ingest/src/nifti.rs`.

## B1 — Panic on a malformed `vox_offset` (severity: highest)

`vox_offset` is read from the file header and used as a **slice start before any bounds check**:

```rust
let vox_offset = f32_to_usize(f32le(&b, 108));   // nifti.rs:81 — attacker/corruption controlled
...
let d = &b[vox_offset..];                         // nifti.rs:89 — panics if vox_offset > b.len()
macro_rules! read_vec { ... if d.len() < n * $w { return Err(...) } ... }  // check comes AFTER
```

The only guard upstream is `b.len() < 352`. A file declaring `vox_offset = 1e9` with a small payload
slices out of range and **panics** ("range start index out of range"). The length check inside
`read_vec!` is ordered after the slice, so it never runs.

This violates a stated invariant: FEATURE-MATRIX §A lists the error taxonomy as "typed
`#[non_exhaustive]`, `Integrity{what,exp,act}`, **never panic**". Ingest reads untrusted third-party
files, so a header-driven panic is the wrong failure mode — it should be `Error::Invalid`.

Fix: bounds-check `vox_offset` against `b.len()` immediately after parsing it, and prefer
`b.get(vox_offset..).ok_or_else(...)` over direct slicing throughout.

## B2 — `.nii.gz` is unsupported, and fails with a misleading error

`read_nifti` does a plain `std::fs::read` (`nifti.rs:53`) and then checks `sizeof_hdr == 348`
(`nifti.rs:57`). A gzipped NIfTI therefore fails with:

```text
nifti: sizeof_hdr != 348 (not NIfTI-1, or big-endian — unsupported)
```

`.nii.gz` is the **majority of NIfTI on disk** in practice (FSL, SPM and dcm2niix all emit it by
default), so this is a format-completeness gap rather than polish, and the error actively misleads —
it blames the file for not being NIfTI when it is.

Fix: sniff the gzip magic (`1f 8b`) and decompress transparently. Failing that, at minimum detect it
and say `"gzipped NIfTI (.nii.gz) is not yet supported — gunzip first"`.

## B3 — qform is ignored; geometry silently dropped when `sform_code == 0`

Only the sform is read:

```rust
let sform_code = i16le(&b, 254);                  // nifti.rs:117
let world_frame = (sform_code > 0).then(|| { ... });   // nifti.rs:118 — else None
```

`qform_code` (offset 252) and the quaternion fields (`quatern_b/c/d`, `qoffset_x/y/z`, `pixdim[0]`
handedness) are never touched. Files with `sform_code == 0` and `qform_code > 0` — older SPM/FSL
outputs, and some dcm2niix paths — carry their geometry **only** in the qform. Today they ingest with
`world_frame: None`, i.e. Tessera declares the array to be in bare index space when it demonstrably
is not, with no warning.

Fix: precedence sform (`sform_code > 0`) → else qform (`qform_code > 0`, quaternion→matrix) → else
no `world_frame`. Record which one was used.

## B4 — `world_frame.space` is hard-coded `"scanner"`

```rust
space: "scanner".into(),   // nifti.rs:141
```

NIfTI's `sform_code`/`qform_code` value *is* the frame identity: 1 = scanner-anatomical,
2 = aligned (co-registered to another file), 3 = Talairach, 4 = MNI-152. Stamping every volume
`"scanner"` mislabels the aligned/Talairach/MNI cases, which is exactly the metadata ADR-0030's named
frame exists to carry — `"atlas:<id>"` is already in the vocabulary for the MNI/Talairach cases.

Fix: map the code onto `WorldFrame.space` (`1 → "scanner"`, `2 → "aligned"`, `3 → "atlas:talairach"`,
`4 → "atlas:mni152"`).

## B5 — 4-D/5-D volumes silently ingest as volume 0 only

`ndim` is validated as `1..=7` (`nifti.rs:68`) but only dims 1–3 are read:

```rust
let nx = dim_at(1)?; let ny = ...dim_at(2)...; let nz = ...dim_at(3)...;   // nifti.rs:74-76
let shape = vec![nz, ny, nx];
let n = usize::try_from(nx * ny * nz)?;                                     // nifti.rs:78
```

`n` covers a single volume, and `read_vec!` takes the **first** `n` voxels. A 4-D fMRI series with
`dim[4] = 200` therefore ingests **volume 0 and silently discards the other 199** — the file passes,
seals, and verifies, while 99.5% of the data is gone. There is no error and no warning. 4-D is the
second-most-common neuroimaging shape after 3-D anatomical (fMRI time series, DWI direction stacks).

Fix (either is acceptable, the first is better): declare `[t,z,y,x]` and carry the time/direction
axis via ADR-0032 `axis_referencing`; **or** hard-error naming `dim[4]` until 4-D is supported.
Silently dropping volumes is the one option that must not ship.

## Scope

- [ ] **P0 — B1**: bounds-check `vox_offset`; typed error, no panic. Regression test with a header
      declaring an out-of-range offset.
- [ ] **P0 — B5**: stop silently discarding volumes (error at minimum, 4-D support preferred).
- [ ] **P0 — B3**: qform fallback with correct precedence.
- [ ] **P1 — B4**: `space` derived from the sform/qform code.
- [ ] **P1 — B2**: transparent `.nii.gz`.
- [ ] **P2**: audit the remaining header-derived indices in this file for the same B1 pattern.

## Pitfalls

- **B3/B4 change `manifest_hash` for affected files** — a volume that previously ingested with
  `world_frame: None` will now carry a frame. That is a correctness fix, but it is a hash change for
  those inputs; the committed conformance corpus uses a synthetic sform-only fixture
  (`write_synth_nifti`, `nifti.rs:190`) so the corpus itself should be unaffected. Verify before
  merging rather than assuming.
- **B5's 4-D form is a shape change**, so it interacts with ADR-0032 axis referencing — do not invent
  an axis convention here; reuse `time_regular`/`time_irregular`.
- The quaternion→matrix conversion (B3) has a sign convention that is easy to get subtly wrong
  (`pixdim[0] < 0` flips the k column). Test against a real FSL-produced qform-only file, not only a
  synthetic one.
- B2 must not open a decompression-bomb path — cap the decompressed size.

## Acceptance criteria

- [ ] A `.nii` with an out-of-range `vox_offset` returns `Error::Invalid`; no panic. Test asserts it.
- [ ] A 4-D NIfTI either ingests all volumes or errors; a test proves volume 199 is not silently lost.
- [ ] A qform-only file (`sform_code = 0`, `qform_code = 1`) produces a `world_frame` matching the
      quaternion, verified against a known voxel→world mapping.
- [ ] `sform_code = 4` produces an MNI-named space, not `"scanner"`.
- [ ] A `.nii.gz` ingests to the same `content_hash` as its gunzipped twin.
- [ ] Existing `reads_nifti_volume_with_lps_world_frame_and_builds_recon` still passes unchanged.

## References

- `tessera/crates/tessera-ingest/src/nifti.rs` (all five)
- ADR-0056 §11 (where these were recorded) · #386 generic ingest
- ADR-0025 (lossless at the door — B5 violates it) · ADR-0030 (named frames — B3/B4)
- ADR-0032 (axis referencing — the B5 4-D fix) · FEATURE-MATRIX §A ("never panic" — B1)
