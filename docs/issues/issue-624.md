---
type: issue
state: open
created: 2026-10-18T12:50:24Z
updated: 2026-10-18T12:50:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/624
comments: 0
labels: feature, docs, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:44.197Z
---

# [Issue 624]: [[ADR-0061 P5] docs(ingest): legacy detection, the de-identification book page and disclosure](https://github.com/vig-os/tessera/issues/624)

Phase **P5** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~300 lines.

- **Scope:** the `inspect` / `verify` legacy warning; the book (`ingest.md`, `provenance-signing.md`, a
  de-identification page with the option table, §2.1 R1-R11, §3.0, the PET recipe, the §8 modality list and
  the custodian-keyspace erasure caveat); the FEATURE-MATRIX row moves to ✓; the security note and advisory
  (D4); the ADR-0047 amendment banner (§1 step 4, §4) and an ADR-0035 note (§9.5); the scheduled
  edition-drift workflow.
- **Acceptance:** §11.1.11 against a synthetic old-style manifest built in the test ((0012,0062) = `YES`, no
  record), because P3c has removed the old code path; `guardrails-gates` green.
- **Depends on:** P3d, P4c.

**Follow-up issues** (filed with the plan, not built here): Clean Descriptors (which would address the
free-text description residual); Clean Pixel Data / Recognizable Visual Features; DICOM egress resolving the
IOD-conformance qualifiers; the schema `dicom_tag` binding (blocked on consumer-schema registration); date
aggregation as a Modified Dates method; per-subject custodian keys; URL rewriting instead of R7 removal, if a
consumer needs retrieve locations.

## Dependencies

- P3d → #620
- P4c → #623

Do not dispatch before its predecessors merge.

