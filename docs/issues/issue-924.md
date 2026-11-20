---
type: issue
state: open
created: 2026-11-19T15:35:13Z
updated: 2026-11-19T15:35:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/924
comments: 0
labels: chore, priority:low, area:testing, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:09.142Z
---

# [Issue 924]: [test(ingest): honest pre-fix tests + cover fail-closed branches of the TOML underflow walk](https://github.com/vig-os/tessera/issues/924)

Follow-up from the round-4 re-review of #894 (merged 2bccbf5). Neither item blocks the merge.

1. **Some new tests did not fail before the fix.** The plain `[[x]]` and plain inline-table underflow cases in `tessera-ingest/src/sealable.rs` already passed at b580323f, before the round-4 fix. They were added as fix tests, but they are really coverage. Only the dotted-key and implicit-table cases show the bug. Relabel those two (or merge them into one coverage test) so the fix tests stay honest.
2. **Fail-closed branches have no tests.** These internal mismatch errors in the TOML underflow walk are never exercised:
   - "no key … in the tree"
   - "array shorter/longer than the tree"
   - "expected a table/array"

   If someone changed one of them to `Ok(())`, which is the original fail-open bug class, no test would fail. Add a unit test per branch: feed the walker a tree and span text that disagree, and check that it refuses.

Refs: #886

