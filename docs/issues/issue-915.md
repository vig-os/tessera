---
type: issue
state: open
created: 2026-11-19T05:25:25Z
updated: 2026-11-19T05:25:25Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/915
comments: 0
labels: docs, priority:high, area:core, adr
assignees: none
milestone: none
projects: none
parent: 897
children: none
synced: 2026-11-19T09:52:02.969Z
---

# [Issue 915]: [docs(design): tessera-table-v2 codec profile design note (ADR-0066 step 6, before #828)](https://github.com/vig-os/tessera/issues/915)

Part of the ADR-0066 implementation plan (docs/adr/0066-seal-encoding.md, merged in #900; epic #897). **Step 6 of 9: a design note for `tessera-table-v2`.** This note must be approved before #828 is implemented, which is step 7.

- **Crates:** none. The deliverable is a document under `docs/`, plus the ADR or ADR amendment it needs.
- **Depends on:** nothing. It may run in parallel with steps 1–5 and should finish before step 7 starts.
- **Scope:**
  - the per-product-type profile table from the #828 sweep;
  - how the profile is named in the manifest;
  - the speed/size trade-off the default takes;
  - the determinism requirement across arches (ADR-0026, #468, #472).
- **Acceptance:** the owner has reviewed and accepted the note. It states the numbers that step 7's acceptance will be measured against.

