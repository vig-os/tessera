---
type: issue
state: open
created: 2026-10-18T12:50:03Z
updated: 2026-10-18T12:50:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/611
comments: 0
labels: feature, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:49.326Z
---

# [Issue 611]: [[ADR-0061 P1] feat(ingest): vendored PS3.15 / PS3.6 tables and the policy generator](https://github.com/vig-os/tessera/issues/611)

Phase **P1** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~300 lines.

- **Scope:** the stdlib-Python generator over E.1-1, E.3.10-1 and the PS3.6 6-1/7-1/8-1 registry; the R3, R4,
  R6, R7 and R11 lists, the R9/R10 searches with each candidate's class, and the §3.0 override set; rules R1-R11
  with their citations; the TSVs with provenance headers; a static parser; the hermetic drift test. No
  behaviour change.
- **Acceptance:** §11.1.10 green with every §2 / §2.1 count; the source sha256 equals §2's; a hand edit to a
  TSV fails the drift test.
- **Depends on:** none.

## Dependencies

None.

Do not dispatch before its predecessors merge.

