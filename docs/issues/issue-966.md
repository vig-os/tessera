---
type: issue
state: open
created: 2026-11-24T10:39:13Z
updated: 2026-11-24T10:39:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/966
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-24T11:59:37.625Z
---

# [Issue 966]: [DICOM export: full re-export of ingested studies (not only SEG), pixel-identical, with UID policy](https://github.com/vig-os/tessera/issues/966)

## Motivation
Products ingested from DICOM must go back out as DICOM: owner downloads, referrals to clinics or hospitals that are not on the platform, second reads by external radiologists, and EPD-style exchange in human use later. The reader-side export adapters in #583 list `to_dicom_seg` but no full image re-export.

## Proposal
- `tessera export dicom` / `to_dicom`: rebuild Part-10 files per instance from pixel arrays + header rows (+ the identity envelope when the key holder authorises it).
- UID policy: keep the original SOP Instance UID on a pixel-identical, identified export; mint new UIDs on a de-identified export; keep (0028,2110) Lossy Image Compression = "00".
- Transfer syntax choice: explicit VR little endian by default; lossless compressed options.
- Shares the round-trip suite of the migration-record issue.

## Acceptance
- [ ] CT/MR/CR/DX series round-trip pixel-identical; [ ] opens in two independent viewers; [ ] de-identified export carries no identity tags.

Refs #583, #301, #656. Consumer: nerdmachines/vetgraph.
