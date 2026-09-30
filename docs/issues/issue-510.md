---
type: issue
state: closed
created: 2026-09-29T11:04:41Z
updated: 2026-09-29T12:31:12Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/510
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:08.057Z
---

# [Issue 510]: [docs: agent instructions still say to commit unsigned — the owner now requires signed commits](https://github.com/vig-os/tessera/issues/510)

Since squash merges were disabled on 2026-09-29, every branch commit lands on `dev` as-is. The owner has decided that all commits, agents' included, are signed with the registered SSH signing key.

`tessera/CLAUDE.md:91-92` and `tessera/docs/AUTONOMOUS-GOAL.md:68` still tell agents that signing is unavailable and to pass `-c commit.gpgsign=false`. That now contradicts the policy and would mislead the next agent. Update both to require signed commits.
