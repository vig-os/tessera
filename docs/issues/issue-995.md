---
type: issue
state: open
created: 2026-11-26T17:44:04Z
updated: 2026-11-26T17:44:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/995
comments: 0
labels: chore, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:31:03.978Z
---

# [Issue 995]: [nits from the #990 review (injective lineage refs)](https://github.com/vig-os/tessera/issues/995)

From the fresh review of PR #990 (#975):
1. `repo.rs:236-250`: `lock()` creates `tmp/` and the lock file and empties `tmp/` before checking the layout, so a refused old-layout repository still gets touched. This is harmless, since `tmp/` only holds debris, but the docs saying "nothing was written" overstate it, and the test's `ref_state` ignores `tmp/`. Either check the layout before `lock()` touches anything, or reword the docs and test.
2. The refusal message reads "has the layout the one from before #975". Reword it, and update the quote in `docs/book/src/versioning.md` to match.
3. `repo.rs:7`: the module doc still says `refs/<lineage>`; it should say `refs/<encoded lineage>`.
4. `has_entries` refuses an empty, unmarked repository if `refs/` holds any stray file, such as macOS `.DS_Store`. Decide whether to ignore dotfiles.

Refs: #975
