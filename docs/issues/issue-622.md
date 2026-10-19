---
type: issue
state: open
created: 2026-10-18T12:50:21Z
updated: 2026-10-18T12:50:21Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/622
comments: 0
labels: feature, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:45.054Z
---

# [Issue 622]: [[ADR-0061 P4b] feat(ingest,cli): select PS3.15 options](https://github.com/vig-os/tessera/issues/622)

Phase **P4b** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~300 lines.

- **Scope:** the `[product.deidentify_profile]` table with `options`; `--deid-option`; the exclusivity and
  unknown-option errors.
- **Acceptance:** end to end: §11.1.2; §11.1.4(a)-(e), with (e) reading Δ back from the v2 envelope;
  §11.1.9(b); §11.1.12.
- **Depends on:** P4.

## Dependencies

- P4 → #621

Do not dispatch before its predecessors merge.

