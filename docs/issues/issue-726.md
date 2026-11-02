---
type: issue
state: open
created: 2026-11-02T06:09:28Z
updated: 2026-11-02T06:09:28Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/726
comments: 0
labels: bug, area:testing
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-02T08:51:34.755Z
---

# [Issue 726]: [test(io): single-source the S3 endpoint contract; make s3-range-read fail when its filter matches no tests](https://github.com/vig-os/tessera/issues/726)

These are the nits from the fresh review of PR #724 (#707, head ca89c72). The review found no blockers.

1. **Two copies of the S3 endpoint contract can drift.** `tessera-cli/tests/cloud.rs:44-66` duplicates
   `tessera-io/src/cloud.rs` `s3_endpoint`, because an integration test can't reach `#[cfg(test)]` items. The CLI copy has 2
   tests and tessera-io has 5; the CLI copy doesn't test the bad-marker or non-UTF-8 cases. Make it a single source,
   e.g. a `#[path]` module shared by both or a small test-support crate. Otherwise mirror all 5 tests and add a guard that
   fails if the two copies differ.
2. **Could `s3-range-read` pass with zero tests?** If its nextest filter (`cloud`) ever matched no tests (a test
   renamed, or a feature not enabled), the check would pass without running anything. Check whether nextest
   `--no-tests=fail` is in effect for this check. If not, set it, or assert the expected test count by name. Show it
   failing with a filter that matches nothing.

