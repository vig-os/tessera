---
type: issue
state: open
created: 2026-10-18T12:50:22Z
updated: 2026-10-18T12:50:22Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/623
comments: 0
labels: feature, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:44.564Z
---

# [Issue 623]: [[ADR-0061 P4c] feat(ingest,cli): custodian keyspace and header surface](https://github.com/vig-os/tessera/issues/623)

Phase **P4c** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~300 lines.

- **Scope:** `key_file`, `--deid-key`, `keygen --deid`; the custodian keyspace with the §4.3 `shred` warning and
  the `inspect --json` keyspace output; `header = "none"`; `also_remove`; `timestamp = "from-source"`.
- **Acceptance:** §11.1.3(b) and §11.1.16 end to end.
- **Depends on:** P4b.
- **Split if needed:** `header` / `also_remove` / `timestamp` as a second PR after the keyspace.

## Dependencies

- P4b → #622

Do not dispatch before its predecessors merge.

