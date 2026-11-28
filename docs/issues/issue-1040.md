---
type: issue
state: open
created: 2026-11-27T17:23:12Z
updated: 2026-11-27T22:11:17Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1040
comments: 0
labels: chore, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-28T09:46:59.141Z
---

# [Issue 1040]: [nits from the #1038 review (CLI exit codes)](https://github.com/vig-os/tessera/issues/1040)

From the round-3 review of PR #1038 (#1002, merged). The verdict was nits only.
1. `advance_ref` (`repo.rs:637`) returns a plain `Invalid` for an unstored lineage target, so it exits 1. The same case for a pin returns `UnstoredPinTarget` and exits 5. Add a typed variant for lineages so both exit 5.
2. The fork-token refusal is classified by matching the exact message text, which is copied in `error.rs` and `collection.rs:466`. `fork_validator_is_classified_…` would catch the two drifting apart, but a typed error variant is the sturdier fix.

Refs: #1002
