---
type: issue
state: open
created: 2026-11-19T05:25:28Z
updated: 2026-11-22T20:45:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/917
comments: 1
labels: chore, priority:high
assignees: none
milestone: none
projects: none
parent: 897
children: none
synced: 2026-11-23T10:16:30.480Z
---

# [Issue 917]: [chore(release): cut the ADR-0066 format-break alpha (ADR-0066 step 9)](https://github.com/vig-os/tessera/issues/917)

Part of the ADR-0066 implementation plan (docs/adr/0066-seal-encoding.md, merged in #900; epic #897). **Step 9 of 9: cut the format-break alpha.**

- **Depends on:** step 8.
- **Scope:** the release notes name:
  - the format break;
  - that alpha-layout products are refused, with no migration verb;
  - which hashes moved;
  - that signatures made before the break no longer verify.
- **Acceptance:** the alpha is tagged and released through the normal release-plz → `git tag -s` → Release.yml path. Bridge work (#454 → #811) starts only after this release.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 22, 2026 at 08:45 PM_

Scope update (owner decision on #828, 2026-11-22): the format-break alpha ships ADR-0066 A′ (#940, merged) and #720 (PR #942) **without** #828. tessera-table-v2 has gone back to its design note (see #828). Step 8 (#916, the final corpus cross-check) follows once #942 merges.

