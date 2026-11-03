---
type: issue
state: open
created: 2026-11-02T10:27:11Z
updated: 2026-11-02T10:27:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/731
comments: 0
labels: priority:low, area:testing
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-03T08:59:46.873Z
---

# [Issue 731]: [test(ci): flake nextest checks pass when the filter matches fewer tests than intended](https://github.com/vig-os/tessera/issues/731)

Follow-up nits from the review of PR #729 (#726), head c09c09c.

1. **A shrunken filter still passes.** `--no-tests=fail` (flake.nix:533) catches a filter that matches zero tests, but not one that matches fewer than intended. For example, `s3-range-read`'s `cloud` filter could lose the range-read tests and still pass on the 5 contract tests. Assert the expected test set by name against a literal count for `s3-range-read` and `sql-tests`, e.g. from `cargo nextest list --message-format json`. Prove it with a fail-before: drop one test and the check fails.
2. `tessera-io/src/cloud/s3_contract.rs:5,9`: two module-doc lines run past 100 columns; reflow them.

Refs: #726
