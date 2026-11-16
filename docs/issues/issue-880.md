---
type: issue
state: open
created: 2026-11-16T00:12:48Z
updated: 2026-11-16T00:12:48Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/880
comments: 0
labels: area:testing
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:35.343Z
---

# [Issue 880]: [chore: gitignore Python __pycache__ (reference reader runs leave worktrees dirty)](https://github.com/vig-os/tessera/issues/880)

Running `tessera/corpus/reference_reader/run.py` leaves an untracked `tessera/corpus/reference_reader/__pycache__/`. That makes every worktree that ran it dirty, so worktree cleanup refuses without `--force`; the PR #788 worktree was blocked this way on 2026-11-16. Add `__pycache__/` (and `*.pyc`) to the root `.gitignore`, or run the reader with `-B` / `PYTHONDONTWRITEBYTECODE=1` in the scripts and the flake check.
