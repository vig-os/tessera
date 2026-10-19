---
type: issue
state: open
created: 2026-10-18T12:50:15Z
updated: 2026-10-18T12:50:15Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/618
comments: 0
labels: feature, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:46.608Z
---

# [Issue 618]: [[ADR-0061 P3b] feat(ingest): seal the Basic profile](https://github.com/vig-os/tessera/issues/618)

Phase **P3b** of [ADR-0061](https://github.com/vig-os/tessera/blob/dev/docs/adr/0061-dicom-deidentification-profile.md) §12 (Accepted, merged in #592). Parent: #579. Estimated size: ~350 lines.

- **Scope:** the `engine.rs` dispatch to `deid::apply` (Basic, per-run keyspace, selected by
  `deidentify = true` / `recipients`); curated metadata from `obj'`; the record and the attestation; the §9.1
  gate with its surface (the `pseudonym_fields` product key and `--pseudonym-field`); the §9.3 gate. The
  attestation and every gate it relies on land together.
- **Acceptance:** §11.1.1 (end-to-end form, Basic); §11.1.3(f) through one spec; §11.1.4(f); §11.1.5(c) and (d);
  §11.1.8 through `engine::run` and the CLI; an S15 re-ingest is byte-identical.
- **Depends on:** P2c, P3a.

## Dependencies

- P2c → #616
- P3a → #617

Do not dispatch before its predecessors merge.

