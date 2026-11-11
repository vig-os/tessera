---
type: issue
state: open
created: 2026-11-11T07:20:31Z
updated: 2026-11-11T07:20:31Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/840
comments: 0
labels: documentation
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:23.386Z
---

# [Issue 840]: [docs: tidy two nits left from the #715 RSS proof records](https://github.com/vig-os/tessera/issues/840)

Nits from the fresh review of #839 at 7f0e2ac. Not blocking.

- In FEATURE-MATRIX, a sentence starts lowercase: '…PR #750). the ksb RSS proof…'.
- #715 asked for one job that exits non-zero on failure. Criteria 1–5 were instead graded by merging shards offline, which the PR discloses. Add a single `run.py evaluate` command line that reproduces the full verdict to the ADR-0026 results note, so anyone can rerun the grading in one step.

Refs: #715
