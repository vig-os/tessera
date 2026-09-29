---
type: issue
state: closed
created: 2026-09-28T18:50:03Z
updated: 2026-09-28T23:30:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/448
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:33.246Z
---

# [Issue 448]: [test(ingest): vendor two small non-PHI real NIfTI fixtures — close #396's "test the qform against a real FSL file" criterion](https://github.com/vig-os/tessera/issues/448)

#396 / PR #447 fixed the qform bug (B3) but could not meet one of its own acceptance criteria:

> The quaternion→matrix conversion (B3) has a sign convention that is easy to get subtly wrong
> (`pixdim[0] < 0` flips the k column). Test against a real FSL-produced qform-only file, not only a
> synthetic one.

No such file was available in that environment and the worker brief forbids real data in fixtures, so
#447 pinned the conversion two independent synthetic ways instead. This issue closes the gap properly.

## The math has since been validated against a real file — just not in a committed test

Verified locally against `functional.nii` from nibabel's test data (a real FSL-written 4-D file), by
**forcing `sform_code` to 0 in a copy** so the decoder had to take the qform branch:

```text
functional.nii: ndim=4 dims=(17,21,3,20) dt=4(int16) qform_code=2 sform_code=2
                pixdim[0]=-1 (qfac)  pixdim[1..3]=(4,4,8)
                quatern=(b=0, c=1, d=0)  qoffset=(32,-40,0)  xyzt_units=10 (mm|sec)

decoder, sform branch (as shipped):  affine = [-0,-0, 4,-32,  -0,-4,-0, 40,   8, 0, 0, 0]
decoder, qform branch (sform zeroed): affine = [ 0,-0, 4,-32,   0,-4,-0, 40,   8, 0, 0, 0]
MAX |qform - sform| = 0        ← exact
```

That is a genuine third-party cross-check: **FSL wrote both transforms itself**, so reproducing its
sform from its quaternion bit-for-bit validates the conversion — and this file happens to exercise the
two parts the issue warned about:

- `pixdim[0] = -1`, i.e. the **qfac k-column flip**;
- `b² + c² + d² = 1` exactly, i.e. the **`a = 0` 180°-rotation branch** of
  `nifti_quatern_to_mat44`, not the ordinary `a = sqrt(1 - …)` path.

The same run also confirmed on real data that B4 and B5 are fixed: `functional.nii` decodes all
**21420** voxels (on `dev`: 1071, i.e. volume 0 of 20), and its `sform_code = 2` yields
`space = "aligned"` — a file `dev` would have mislabelled `"scanner"`. Its TR comes out as
`time_regular(0, 2.0)` from `pixdim[4]` + `xyzt_units`.

So this issue is about **committing** that evidence as a test, not about doubt in the math.

## Candidate fixtures (headers verified by reading the bytes, not assumed)

From `nipy/nibabel`, `nibabel/tests/data/`:

| File | Size | Header | Use |
|---|---|---|---|
| `standard.nii.gz` | **143 B** | LE NIfTI-1, `ndim=3` dims (4,5,7), `dt=2` (uint8), `sform_code=2`, `qform_code=0` | **B2** on a real `.nii.gz` + the real uint8→uint16 widening. Decodes to 140 voxels today. |
| `functional.nii` | 43 KB | LE NIfTI-1, `ndim=4` dims (17,21,3,20), `dt=4`, `qform_code=2`, `sform_code=2`, `qfac=-1` | **B3/B4/B5** — the cross-check above |
| `anatomical.nii`, `reoriented_anat_moved.nii`, `resampled_anat_moved.nii` | 4.6–68 KB | **big-endian** NIfTI-1 (`sizeof_hdr` reads 1543569408 = 348 byte-swapped) | negative tests — see the big-endian issue |
| `row_major.dconn.nii` | 1.9 KB | **NIfTI-2** (`sizeof_hdr = 540`), CIFTI | negative test |

**Important correction to the assumption this issue started from:** *no* `.nii` file in that directory
is strictly `sform_code == 0 && qform_code > 0`. The two smallest NIfTI-1 files are
`qform_code=0, sform_code=2` (`standard.nii.gz`) and `qform_code=2, sform_code=2`
(`functional.nii`). So either:

1. **reframe the criterion** as "a real FSL-written *qform* validated against an independent writer",
   which `functional.nii` satisfies exactly (and better than a qform-only file would, since the file
   carries FSL's own answer to compare against); or
2. find a genuinely qform-only real file elsewhere. Note that zeroing `sform_code` in a copy makes the
   fixture synthetic again in the only respect that matters, so it buys nothing over #447's existing
   synthetic test — option 1 is the real win.

Option 1 is recommended.

## Licence — checked, with one caveat to resolve

`nipy/nibabel`'s `COPYING` puts "the nibabel package, including all examples, code snippets and attached
documentation" under **MIT** (© 2009–2019 Matthew Brett et al.). Its "3rd party code and data" section
carves out specific files, and **none of the `.nii` files above is among them**:

- the PDDL-1.0 carve-outs are the **Philips PAR/REC** files (`phantom_EPI_*`, `Phantom_EPI_*`, `DTI.PAR`,
  `umass_anonymized.PAR`, …) — *not* the NIfTI files;
- `doc/source/someone.nii.gz` has its own MNI/ICBM `mni_icbm152_t1_tal_nlin_asym_09a` licence and should
  be **avoided** for that reason.

So the candidates fall under the repo-wide MIT grant by omission rather than by an explicit data-licence
statement. **Caveat to settle before vendoring:** MIT is a *software* licence and "the package … and
attached documentation" does not name test data outright. Two things to do:

- [ ] Confirm intent with upstream (a short issue on `nipy/nibabel` asking whether `nibabel/tests/data/*.nii`
      is covered by the MIT grant) **or** rely on the omission and document the reasoning.
- [ ] Either way, reproduce the MIT notice + copyright line next to the fixture, as MIT requires.

Neither file contains patient data — `functional.nii` is a 17×21×3 phantom/synthetic volume and
`standard.nii.gz` is a 4×5×7 uint8 test pattern — so there is no PHI concern, only an attribution one.

## Acceptance criteria

- [ ] `standard.nii.gz` (143 B) and `functional.nii` (43 KB) vendored under
      `tessera/crates/tessera-ingest/tests/data/nifti/` with a `PROVENANCE.md` carrying the upstream URL,
      commit SHA, MIT notice and copyright line.
- [ ] A test asserts the qform branch reproduces `functional.nii`'s own sform **exactly** (`sform_code`
      zeroed in an in-test copy of the bytes — the file on disk stays pristine).
- [ ] A test asserts `standard.nii.gz` decodes to 140 uint16 voxels with `space = "aligned"` — real-file
      coverage for B2 and B4.
- [ ] A test asserts `functional.nii` decodes all 21420 voxels with `axes = [t,z,y,x]` and
      `time_regular(0, 2.0)` — real-file coverage for B5.
- [ ] `typos` excludes the binary fixtures (`_typos.toml` — it spell-checks binaries; see the `.tsra`
      precedent) and the crane `src` filter in `flake.nix` includes the new `tests/data/` path, or the
      tests silently run zero cases in the nix gate (the trycmd precedent).
- [ ] The synthetic tests from #447 stay — they cover the `sform_code == 0` branch and the malformed
      headers, which no real file does.

## References

- PR #447 (the fix) · #396 (the five bugs) · the big-endian follow-up
- `tessera/crates/tessera-ingest/src/nifti.rs` — `qform_rows`, `parse_geometry`
- upstream: <https://github.com/nipy/nibabel/tree/master/nibabel/tests/data> · `COPYING`

