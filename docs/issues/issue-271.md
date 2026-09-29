---
type: issue
state: closed
created: 2026-07-01T14:25:14Z
updated: 2026-09-28T17:54:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/271
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:59.276Z
---

# [Issue 271]: [feat(ingest): DICOM completeness — populate world_frame (affine) + surface rescale to manifest meta](https://github.com/vig-os/tessera/issues/271)

Found by explorer + ingest reviewers. (1) `world_frame` voxel→world affine not populated from ImagePositionPatient/ImageOrientationPatient/PixelSpacing/SliceThickness (ADR-0030) → `tsra slice --world` never works on real DICOM ingests. (2) `rescale_slope`/`rescale_intercept` are decoded (stats prints the HU formula) but show `—` in `tsra schema` — surface them into manifest `metadata`, not only the array spec. (Curated tags DO populate on fresh ingests since #255; reviewers saw blanks because ct-lung.tsra predates #255.)
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit fdb457e 'feat(ingest): populate DICOM series world_frame + surface rescale (#271) (#280)'.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

