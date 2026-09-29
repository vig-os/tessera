---
type: issue
state: closed
created: 2026-02-25T20:24:54Z
updated: 2026-09-28T17:53:29Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/111
comments: 1
labels: feature, effort:medium, area:imaging
assignees: gerchowl
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:12.469Z
---

# [Issue 111]: [[FEATURE] fd5.ingest.nifti — NIfTI loader](https://github.com/vig-os/tessera/issues/111)

## Parent

Epic: #108 (Phase 6: Ingest Layer)

## Summary

Implement `src/fd5/ingest/nifti.py` — a loader that reads NIfTI-1/NIfTI-2 files (`.nii`, `.nii.gz`) and produces sealed fd5 `recon` files via `fd5.create()`.

## Scope

NIfTI is simpler than DICOM — it already has a volume + affine in a single file. The loader's job is:

1. **Read volume** — load data array via nibabel
2. **Extract affine** — use the NIfTI sform/qform affine (4×4 matrix)
3. **Map metadata** — NIfTI headers have limited metadata (voxel sizes, data type, intent codes). Map what's available.
4. **Dimension order** — determine from NIfTI header (typically RAS or LPS convention)
5. **Provenance** — record source `.nii`/`.nii.gz` file with SHA-256 hash

### Dependency

```toml
[project.optional-dependencies]
nifti = ["nibabel>=5.0"]
```

## Proposed API

```python
def ingest_nifti(
    nifti_path: Path,
    output_dir: Path,
    *,
    product: str = "recon",
    name: str,
    description: str,
    timestamp: str | None = None,
    reference_frame: str = "RAS",
    study_metadata: dict | None = None,
) -> Path:
    """Read a NIfTI file and produce a sealed fd5 file."""
```

## Acceptance criteria

- [ ] Implements `Loader` protocol from `fd5.ingest._base`
- [ ] Produces valid fd5 files that pass `fd5 validate`
- [ ] Affine correctly extracted from NIfTI sform/qform
- [ ] 3D and 4D NIfTI files supported (static + dynamic)
- [ ] `.nii.gz` (compressed) handled transparently
- [ ] `ImportError` with clear message when nibabel not installed
- [ ] Provenance records source file SHA-256
- [ ] Tests with synthetic NIfTI data
- [ ] ≥ 90% coverage

## Dependencies

Depends on: `fd5.ingest._base` (#109)
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:53 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: Superseded by tessera-ingest nifti.rs; open bugs tracked in #396.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

