---
type: issue
state: open
created: 2026-11-19T18:05:41Z
updated: 2026-11-19T18:05:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/928
comments: 0
labels: chore, priority:low, area:docs
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:08.313Z
---

# [Issue 928]: [docs(ci): promote-main wording nits after #926](https://github.com/vig-os/tessera/issues/928)

Nits from the review of PR #926 (merged 4c02cab). None block anything.

1. The promotion commit's message says "Merges $tag … into main", but `promotion-head.sh`'s header says "It merges nothing". The commit only carries the tag's tree with parents [main, tag]. Reword the message to match, e.g. "Promotes $tag to main (tree = $tag)".
2. The `tag` head kind now also checks the tag commit's tree and signature before creating the ref. That is stricter than before, and it is documented. The PR body called that path "unchanged"; record the stricter check in docs/RELEASING.md under the head kinds.
3. If the branch is created but the PR create fails, a re-run fails at `ls-remote` until someone deletes the branch. This is by design. Add the exact recovery command to docs/RELEASING.md.

Refs: #925

