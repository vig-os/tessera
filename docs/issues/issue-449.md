---
type: issue
state: closed
created: 2026-09-28T18:50:31Z
updated: 2026-09-28T23:30:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/449
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:32.870Z
---

# [Issue 449]: [feat(ingest): read big-endian NIfTI-1 (3 of 6 nibabel test files are BE) — normalise byte order at the door](https://github.com/vig-os/tessera/issues/449)

Found while fixing #396. Not a regression — big-endian NIfTI-1 has never been supported — but the
prevalence measured on the way makes it worth its own issue rather than a footnote.

## It is not a rare case

**Three of the six `.nii`/`.nii.gz` files in nibabel's own test data are big-endian.** Verified by
reading `sizeof_hdr` (offset 0) directly:

| File | `sizeof_hdr` as little-endian | Meaning |
|---|---|---|
| `anatomical.nii` | 1543569408 | 348 byte-swapped → **big-endian NIfTI-1** |
| `reoriented_anat_moved.nii` | 1543569408 | big-endian NIfTI-1 |
| `resampled_anat_moved.nii` | 1543569408 | big-endian NIfTI-1 |
| `functional.nii` | 348 | little-endian NIfTI-1 |
| `standard.nii.gz` | 348 | little-endian NIfTI-1 |
| `row_major.dconn.nii` | 540 | **NIfTI-2** (CIFTI), a separate gap |

Big-endian NIfTI is what ANALYZE-era and SPARC/PowerPC-written data looks like, and it is exactly the
class of file an archival format is asked to read *because* nothing modern can. Refusing it is a poor
fit for a format whose pitch is "read it offline, forever".

## What PR #447 already did

Only the diagnosis. The old error blamed the file:

```text
nifti: sizeof_hdr != 348 (not NIfTI-1, or big-endian — unsupported)
```

It now names the format actually found:

```text
nifti: big-endian NIfTI-1 is not supported (sizeof_hdr is 348 byte-swapped) — convert the file to
       little-endian first
nifti: this is NIfTI-2 (sizeof_hdr = 540), not NIfTI-1 — unsupported (CIFTI .dconn.nii /
       .dtseries.nii files are NIfTI-2)
```

That is strictly a message change — both are still `Error::Invalid`.

## Fix

The byte order is **detected already** (`sizeof_hdr` reading as byte-swapped 348 is the canonical NIfTI
sniff, which is why the header carries a known constant at offset 0). What is missing is reading through
it. `nifti.rs` funnels every scalar read through four helpers — `i16le`, `i32le`, `f32le`, `f64le` — so
the change is contained: thread a byte order through `parse_header` and have those four swap when it is
big-endian. Voxel decode needs the same in `read_voxels`' per-dtype closures.

**Normalise at the door (ADR-0025):** decode to native and store little-endian, exactly as the
`.npy` lane already specifies ("big-endian normalised to native", ADR-0056 §11). The byte order is a
transport detail of the source, not a property of the data, so it must **not** reach the sealed bytes —
a big-endian and a little-endian file of the same volume should seal to the *same* `content_hash`. That
is the acceptance test worth writing.

## Non-goal, deliberately

`.hdr`/`.img` pairs (magic `ni1` instead of `n+1`) stay unsupported and are a separate question — they
are two files, so they interact with the collection model rather than with the decoder. The current
error message for them is already accurate ("only single-file .nii is supported").

## Acceptance criteria

- [ ] A big-endian NIfTI-1 fixture (synthetic, byte-swapped header + payload) decodes to the **same**
      shape, voxels and `world_frame` as its little-endian twin.
- [ ] Both seal to the **same block `content_hash`** — byte order never reaches the sealed bytes.
- [ ] The qform/sform geometry is byte-swapped too (they are `f32` fields, easy to miss).
- [ ] `datatype`, `vox_offset`, `scl_*`, `pixdim`, `xyzt_units` (a single byte — no swap) all covered.
- [ ] The three real big-endian files listed above decode, once #448 vendors fixtures — or at minimum
      the synthetic twin test stands alone.
- [ ] No hash moves for little-endian files: the swap is behind a flag that is off for them.

## References

- `tessera/crates/tessera-ingest/src/nifti.rs` — `i16le`/`i32le`/`f32le`/`f64le`, `sizeof_hdr_error`
- #396 / PR #447 (where this was measured) · #448 (real fixtures) · #446 (`xyzt_units` spatial unit)
- ADR-0025 (normalise at the door) · ADR-0056 §11 (the `.npy` lane already commits to this behaviour)

