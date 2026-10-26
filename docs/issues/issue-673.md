---
type: issue
state: open
created: 2026-10-25T11:19:01Z
updated: 2026-10-25T11:19:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/673
comments: 0
labels: question, priority:low, area:workspace
assignees: none
milestone: none
projects: none
parent: 654
children: none
synced: 2026-10-26T07:24:19.008Z
---

# [Issue 673]: [question: give tessera-query its own version line at 1.0 (ADR-0062 / ADR-0052)](https://github.com/vig-os/tessera/issues/673)

Open question from ADR-0062 §2 (#654), for the owner.

`tessera-query` exposes DataFusion types, so every DataFusion major is a breaking `tessera-query` release. Under
ADR-0052 §2's unified workspace version that bumps the whole workspace. Should `tessera-query` leave the unified
version at the 1.0 cut and keep its own version line (an exception to ADR-0052 §2)?

**Recommendation:** yes, at 1.0; unified until then (pre-1.0 increments are harmless).

