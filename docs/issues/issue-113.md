---
type: issue
state: closed
created: 2026-02-25T20:25:14Z
updated: 2026-09-28T17:53:34Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/113
comments: 1
labels: feature, effort:medium, area:core
assignees: gerchowl
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:11.798Z
---

# [Issue 113]: [[FEATURE] fd5 ingest CLI commands](https://github.com/vig-os/tessera/issues/113)

## Parent

Epic: #108 (Phase 6: Ingest Layer)

## Summary

Add `fd5 ingest` CLI subcommand group that exposes the ingest loaders via the command line.

## Proposed CLI

```bash
# DICOM series → fd5
fd5 ingest dicom /path/to/dicom/series/ --output ./output/ \
    --name "PET Recon" --description "Whole-body PET reconstruction"

# NIfTI → fd5
fd5 ingest nifti /path/to/volume.nii.gz --output ./output/ \
    --name "CT Volume" --description "Thorax CT scan"

# Raw binary → fd5
fd5 ingest raw /path/to/data.bin --output ./output/ \
    --product recon --dtype float32 --shape 128,128,64 \
    --name "Sim Output" --description "Monte Carlo simulation result"

# List available loaders
fd5 ingest list
```

### Common options

| Option | Description |
|--------|-------------|
| `--output` / `-o` | Output directory (default: current dir) |
| `--name` | Required: human-readable name |
| `--description` | Required: description for AI-readability |
| `--product` | Product type (default depends on loader) |
| `--timestamp` | Override timestamp (default: extracted from source or now) |
| `--study-type` | Optional: study type for study/ group |
| `--license` | Optional: license for study/ group |

### `fd5 ingest list`

Prints available loaders and their status (installed / missing dependency):

```
Available loaders:
  dicom   ✓ (pydicom 2.4.4)
  nifti   ✗ (requires nibabel — pip install fd5[nifti])
  raw     ✓ (built-in)
```

## Acceptance criteria

- [ ] `fd5 ingest dicom` calls `fd5.ingest.dicom.ingest_dicom()`
- [ ] `fd5 ingest nifti` calls `fd5.ingest.nifti.ingest_nifti()`
- [ ] `fd5 ingest raw` calls `fd5.ingest.raw.ingest_binary()`
- [ ] `fd5 ingest list` shows available loaders with dep status
- [ ] Graceful error if required dep not installed
- [ ] `fd5 ingest --help` shows all subcommands
- [ ] Tests via `CliRunner`
- [ ] ≥ 90% coverage

## Dependencies

Depends on: `fd5.ingest._base` (#109), at least one loader implemented
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:53 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: Tessera CLI has 'tessera ingest {dicom-series,dicom,ge-hdf5,nifti,blob,blob-series}' (crates/tessera-cli/src/main.rs). #386 tracks generic ingest CLI.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

