---
type: issue
state: closed
created: 2026-11-19T05:25:28Z
updated: 2026-11-25T04:10:34Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/917
comments: 2
labels: chore, priority:high
assignees: none
milestone: none
projects: none
parent: 897
children: none
synced: 2026-11-25T10:52:05.366Z
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

---

# [Comment #2]() by [gerchowl]()

_Posted on November 25, 2026 at 04:10 AM_

Released: v0.1.0-alpha.7, the ADR-0066 format break (A′ plus #720), cut through the normal path.
- Release PR #932 was merged as d24c1f9a. The signed annotated tag `v0.1.0-alpha.7` (aa8d8bdf) points at that commit.
- Release run 45542380349 succeeded: 4 platforms, and a pre-release with 13 assets.
- The notes are exactly the `[0.1.0-alpha.7]` section of the root CHANGELOG (versioned per #982). They name the format break, the refused alpha layout (no migration verb), which hashes moved (per #916 / PR #980), and that signatures made before the break no longer verify where the seal moved.
- Promotion PR #985 (tree = tag tree; parents `main` ee277ad3 and the tag commit) waits for the owner's approval. It supersedes #927.
- Bridge work (#454 → #811) may start now, per the plan.

