---
type: issue
state: open
created: 2026-10-18T11:53:23Z
updated: 2026-10-18T11:54:15Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/604
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:51.492Z
---

# [Issue 604]: [[ADR-0060 P3d] feat(cli): attested and unattested actors in `inspect`](https://github.com/vig-os/tessera/issues/604)

Phase **P3d** of [ADR-0060](https://github.com/vig-os/tessera/blob/dev/docs/adr/0060-agentic-actors-and-prov-mapping.md) §11 (Accepted, merged in #586). Parent: #564.


- **Scope:** `inspect`'s marks (§6.4, §10.4), reusing P3's lookup.
- **Acceptance:** `inspect` shows `attested (key …)` only for the pinned approver, `unattested` for an
  `acted_on_behalf_of` principal who didn't sign, and `unattested` for every row with no signature or an
  empty trust store → a trycmd case.
- **Depends on:** P3.

## Dependencies

#587 → #573 → #574 → #596 → #597 → #598 → #599 → #600 → #601 → #602 → #603 → P3d → P3c

Do not dispatch before its predecessors merge.

