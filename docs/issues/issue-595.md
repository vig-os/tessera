---
type: issue
state: open
created: 2026-10-18T10:09:16Z
updated: 2026-10-18T10:09:16Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/595
comments: 0
labels: documentation
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:54.887Z
---

# [Issue 595]: [docs(adr): ADR-0061 wording nits from its final review](https://github.com/vig-os/tessera/issues/595)

From the fresh review of ADR-0061's acceptance commit (PR #592 @aaffb71), no design change:
1. §11.1.15(c) says 'no text-VR element holds a copy of any UI value of the input', which is broader than the ADR guarantees (free-text descriptions are a stated residual). Scope it to the test fixture.
2. R10 'a pattern-D hit that r4 misclassified': 'r4' is ambiguous (revision 4 vs the round-4 audit).
3. 'The reviewers agreed with each (rounds 1-4)' overstates round 4, which didn't rule on the decisions.

Refs: #579
