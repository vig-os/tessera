---
type: issue
state: open
created: 2026-11-26T23:03:35Z
updated: 2026-11-27T08:59:28Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1010
comments: 0
labels: chore, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:30:56.350Z
---

# [Issue 1010]: [nits from the #1004 review (Collection sources)](https://github.com/vig-os/tessera/issues/1010)

From the fresh round-2 review of PR #1004 (#997, merged). It found no blockers.
1. A whitespace-only `fork` token (`" "`) passes, because the check is `is_none_or(|t| t.is_empty())`. If whitespace should count as empty, use `trim().is_empty()`, and make SPEC's "non-empty" say which is meant.
2. `probe_to_stored_bypasses_fork_rule` returns early once `compute_manifest_hash` refuses, so it never reaches `to_stored`. Make it assert at the `to_stored` door itself; `all_writer_doors_refuse` covers that door today.
3. `CHANGELOG.md:18` is 150 characters; rewrap it.
4. Reading a forged collection through `Collection::verify()` now raises `Invalid`. That is documented as the writer's check, and the only caller is `to_stored`. Keep `verify()` out of reader paths, or split it.

Refs: #997
