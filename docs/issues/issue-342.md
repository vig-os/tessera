---
type: issue
state: closed
created: 2026-07-07T08:37:55Z
updated: 2026-09-28T17:55:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/342
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:47.824Z
---

# [Issue 342]: [feat(ingest): seal-time identity inheritance raw→derived — patient/exam/study/instrument stranded on the parent](https://github.com/vig-os/tessera/issues/342)

## Problem (both tiers, DUPLET first-user review)

Intrinsic **identity** metadata is captured on ONE product but never propagates to its derivatives, so a scientist handed a single derived `.tsra` cannot tell what study/patient/instrument it belongs to.

**Listmode chain (`DP01-singles`):** the raw `.dat` blob carries 16 acquisition fields —
```
acq_patient_id="ANON9297"  acq_exam_number="9297"
acq_start_time="2023-09-22 16:30:15"  acq_duration="420"
acq_acq_mode_config="acq.cfg.LYSO4x9_6_SIPMGEN1_230223"
acq_crystalmap/ctc/energy/gain="<cal files>" ...
```
…but the derived `singles`/`coin`/`events`/`time-markers` members inherit only `study` + `coincidence_mode`. `tessera inspect events-3p` → `meta: coincidence_mode, study` — no patient, exam, scanner, acquisition window.

**DICOM two-tier (`DP01`):** the recon volume has full DICOM identity (`study_instance_uid`, `series_instance_uid`, `modality`, `manufacturer`, `model_name`, `study_date`, `exam`) — but the **parent blob-series manifest** is anemic (`meta: series, study` only), even though those values are constant across the whole preserved series. `tessera inspect blob-12-pt` tells a downstream user almost nothing; they must `extract` a DICOM file and parse it.

## Proposal

A **seal-time step** that DAG-walks `derived_from` and copies a defined set of *intrinsic identity* fields from ancestor → descendant `meta`, unless the descendant already overrides them:

- **Intrinsic identity** (belongs to the data, not the transform): `study`, `patient_id`, `exam`, `study_instance_uid`, `study_date`, instrument (`manufacturer`/`model_name`/serial), acquisition window/times. These SHOULD inherit.
- **Transform-specific** (recon params, quantization scales, coincidence window): these belong to each product's own generation record (#324), NOT inherited.
- For the DICOM blob-series specifically: surface the series-constant DICOM identity (`modality`, `study_instance_uid`, `series_instance_uid`, `study_date`, `manufacturer`, `model_name`, `exam`, `sop_class_uid`, `file_count`) onto the **blob** manifest at ingest, so the cold tier is discoverable via `inspect` without dereferencing DICOM bytes.

Distinct from #324 (which records *how* a product was made — producer+settings). This issue is about *what identity* it carries. Together they make a single derived `.tsra` self-describing.

Found during: DUPLET first-user run; two independent fresh-context FAIR reviews converged on this as the #1 root cause.
Refs #324, #223 (raw→derived boundary), #301, #305.
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:55 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: ADR-0058 accepted + landed (commit 7d6da0a 'feat(core+ingest): land ADR-0058 generation-provenance on dev (supersedes #346) (#415)'). ADR-0058 status line explicitly names #342 as implemented.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

