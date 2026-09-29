---
type: issue
state: open
created: 2026-06-25T11:39:49Z
updated: 2026-09-28T17:59:38Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/207
comments: 1
labels: priority:high, epic, area:ingest
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:05.997Z
---

# [Issue 207]: [[P5][epic] tessera-ingest: DICOM (+ PS3.15, DICOMweb/DIMSE, egress)](https://github.com/vig-os/tessera/issues/207)

**Phase P5 → v0.2.** ROADMAP: tessera/docs/ROADMAP.md

DICOM files + DICOMweb + DIMSE; PS3.15 verify + re-attest; lossless tags; rescale/units normalise-at-door; egress (bidirectional VNA).

**Done-gate:** lossless DICOM roundtrip + egress; golden DICOM corpus. (Task #10/S9.)
---

# [Comment #1]() by [gerchowl]()

_Posted on June 26, 2026 at 12:15 AM_

First cut landed (commit 9a9c494): `tessera-ingest::dicom` — single-frame DICOM → `recon` product, **lossless** (raw native int16 via `ModalityLutOption::None`; rescale/modality/UCUM-unit as metadata, not baked in; `ingested_from` provenance). Pure-Rust decode (charls/gdcm/ul off). Hermetic tests synthesize a DICOM in-process (no PHI). ADR-0025. **Remaining for v0.2:** multi-slice series stacking (sort by ImagePositionPatient/InstanceNumber → 3-D volume), PS3.15 de-identification verify, golden DICOM corpus.

