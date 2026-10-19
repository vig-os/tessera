---
type: issue
state: open
created: 2026-10-18T12:50:11Z
updated: 2026-10-18T12:50:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/616
comments: 0
labels: feature, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:47.433Z
---

# [Issue 616]: [[ADR-0061 P2c] feat(ingest): keyed UID replacement and the Modified Dates shift](https://github.com/vig-os/tessera/issues/616)

Phase **P2c** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~300 lines.

- **Scope:** UID derivation over the full §4.1 coverage with the DICOM-root guard; the per-run K derivation;
  the subject key, Δ and its refusals; the Modified Dates shift; Retain UIDs. Pure, not wired.
- **Acceptance:** at object level: §11.1.1 (object-level form); §11.1.2; §11.1.3(a) and (c)-(f); §11.1.4(b)-(d);
  §11.1.12(d) and (f) under Modified; §11.1.13(c); §11.1.14; §11.1.15 under the keyed options.
- **Depends on:** P2b-ii.
- **Split if needed:** the Δ derivation and the date shift as a second PR after UID replacement.

## Dependencies

- P2b-ii → #615

Do not dispatch before its predecessors merge.

