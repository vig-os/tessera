---
type: issue
state: closed
created: 2026-07-01T14:23:50Z
updated: 2026-09-28T17:54:36Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/269
comments: 1
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:59.957Z
---

# [Issue 269]: [feat(ingest): PHI hygiene gaps — --deidentify opt-in + incomplete vs PS3.15; --source-label filename side-channel; --spec has no source_label](https://github.com/vig-os/tessera/issues/269)

**Found by ingest, auditor, blob-archivist, explorer reviewers.** The design (ADR-0040) is right; the implementation under-delivers on clinical data.

## 1. `--deidentify` is opt-in and silent when omitted (ingest)
Without the flag, PatientName/PatientID/BirthDate/Age seal into `extra/dicom_header` in the clear — no warning. For a FAIR clinical tool this polarity is wrong. **Default it ON** with `--keep-identified` opt-out, or at minimum print a loud banner when a DICOM input is sealed without de-id.

## 2. `--deidentify` is incomplete vs its PS3.15 claim (ingest, detailed)
Help says "Apply PS3.15 de-identification". Actual: strips Name/ID/DOB only. **Retained in the clear:** `(0008,0081) InstitutionAddress`, `(0008,1010) StationName`, `(0008,1050) PerformingPhysiciansName`, `(0018,1000) DeviceSerialNumber`, `(0032,1032) RequestingPhysician`, `(0032,1060) RequestedProcedureDescription`. Study/Series UIDs **retained verbatim** (PS3.15 requires *replacement* with derived UIDs); `study_date` retained (requires removal/offset). Either broaden the tag list to the PS3.15 Basic Profile + UID replacement + date offset, OR downgrade the help to "partial de-id (Name/ID/DOB only — see ADR-0040)".

## 3. `--source-label` filename side-channel (blob-archivist B2, auditor B2)
`--source-label` scrubs `sources[].reference` but the raw vendor filename is still sealed into `blocks[0].spec.filename` and rendered by `tree` (`file testscan.l64`). A `PATIENT_SMITH_2025.l64` source name is sealed unredacted. Extend `--source-label` (or add `--blob-filename`) to rewrite/redact `spec.filename`; warn when the basename matches a PHI heuristic.

## 4. `--spec` engine has no `source_label` (ingest)
The declarative `--spec` path — the *recommended* production path (MIGRATION.md) — has no `source_label` on `[[product]]`, so it seals absolute PHI paths verbatim (leakier than the quick per-format CLI). Add `source_label` to the product spec schema.

## 5. Sensitivity tiers are informational-only (auditor)
Tiers (`public/coded/sensitive/identifying`) are declared but nothing enforces them. Add a lint (`schema --strict` / `inspect --lint`) that fails non-zero when an `identifying` field would be emitted in cleartext by `export`, or a manifest string matches a PHI regex.
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit fb6b6cd (loud PHI warning), 63448e3 (--source-label redacts sealed blob filename), c00a8cc (offline crypto-shred de-identification ADR-0047, #279).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

