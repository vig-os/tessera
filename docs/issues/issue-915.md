---
type: issue
state: closed
created: 2026-11-19T05:25:25Z
updated: 2026-11-24T18:10:27Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/915
comments: 1
labels: docs, priority:high, area:core, adr
assignees: none
milestone: none
projects: none
parent: 897
children: none
synced: 2026-11-25T10:52:06.318Z
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

---

# [Comment #1]() by [gerchowl]()

_Posted on November 24, 2026 at 06:10 PM_

Done. The design note landed through the merged PR #919 (1c548eb). The owner's dated amendment of 2026-11-22 landed through the merged PR #958 (3b022960); it records the A3/A4/A5 miss and the decision to keep v1 as the default. The opt-in v2 question stays open on #828. Remaining wording nits are tracked in #922.

