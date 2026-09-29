---
type: issue
state: open
created: 2026-06-25T11:39:51Z
updated: 2026-09-28T17:59:39Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/208
comments: 0
labels: epic, area:ingest
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:05.646Z
---

# [Issue 208]: [[P5][epic] tessera-ingest: vendor raw (GE-HDF5 · Siemens · raw .dat/.BLF · NIfTI)](https://github.com/vig-os/tessera/issues/208)

**Phase P5 → v0.3.** ROADMAP: tessera/docs/ROADMAP.md

Per-vendor reader plugins; normalise proprietary-at-door; decode→re-encode lossless. Siemens = binary + padded-ASCII footer; GE = HDF5 + proprietary compression; raw DUPLET `.dat`/`.BLF`.

**Done-gate:** each decodes→re-encodes open + lossless. (FEATURE-MATRIX §G.)
