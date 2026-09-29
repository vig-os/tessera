---
type: issue
state: open
created: 2026-09-28T20:04:22Z
updated: 2026-09-29T06:53:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/454
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:31.673Z
---

# [Issue 454]: [retro-gate the vendor decoders behind cargo features (ADR-0057 Phase 1)](https://github.com/vig-os/tessera/issues/454)

ADR-0057 §5 declares a feature graph in which `dicom` and `hdf5` are opt-in and `static-hdf5` folds into `hdf5`. #386 landed the *generic* half of that graph (`arrow`/`parquet`/`csv`, default-ON) and deliberately left the vendor half alone, because retro-gating touches every vendor backend and ADR-0056 §12 calls it out of scope ("named for a future pass").

What is left:

- `dicom = ["dep:dicom", "dep:dicom-transfer-syntax-registry"]`, `hdf5 = ["dep:hdf5-metno", "dep:hdf5-metno-sys"]`
- `static-hdf5 = ["hdf5", "hdf5-metno/static", "hdf5-metno/zlib"]` so `static-hdf5` without `hdf5` becomes impossible by construction rather than a runtime error
- `full = ["default", "dicom", "hdf5"]`; the release channels set `full` + `static-hdf5` (ADR-0057 §4 ships ONE binary with everything)
- `BACKENDS_ENABLED` gains four `#[cfg]` attributes; `backends::backend_version`'s runtime libhdf5 query needs gating too
- the `EXPECTED_COUNTS` table in `tessera-ingest::corpus` gains the new configurations

Why it is worth doing: ADR-0057 §5's third and load-bearing reason for gating is that a feature-gated decoder makes *which decoders could have produced this artifact* a build-time fact. That is true for the generic lanes today and not yet for the vendor ones.

The mechanism is already proven by #386 — this is the same change, applied to the expensive decoders.
