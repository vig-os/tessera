---
type: issue
state: open
created: 2026-10-18T12:50:08Z
updated: 2026-10-18T12:50:08Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/614
comments: 0
labels: feature, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:48.133Z
---

# [Issue 614]: [[ADR-0061 P2b] feat(ingest): date, URI, name, identifier and free-text rules](https://github.com/vig-os/tessera/issues/614)

Phase **P2b** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~300 lines.

- **Scope:** §3.0 precedence and the record's `overridden` list; R4/R6 date-valued removal (Basic) and
  retention (Full); R7; R9; R10; R11.
- **Acceptance:** at object level: §11.1.4(a); §11.1.12(a), (b), (e) and (f) for Basic and Full; §11.1.13(a);
  §11.1.15 under Basic, Full and the non-keyed options.
- **Depends on:** P2a-ii.
- **Split if needed:** R9-R11 as a second PR after the date and URI rules.

## Dependencies

- P2a-ii → #613

Do not dispatch before its predecessors merge.

