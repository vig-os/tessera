---
type: issue
state: open
created: 2026-11-04T04:59:10Z
updated: 2026-11-04T04:59:10Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/766
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:49.178Z
---

# [Issue 766]: [io: WriteSession follow-ups from #765 review (error names the block, parse cost doc, O(n²) dup-name check)](https://github.com/vig-os/tessera/issues/766)

Follow-ups from the fresh review of PR #765 (#759, merged 26159de). No blockers; these are nits.

1. **Name the block in a parse-back error.** `write.rs:1109-1112` and `write.rs:242-246`: the error from `Committed::of` (e.g. serde_json's recursion limit) surfaces as the bare JSON error. Wrap it as `Error::Invalid` with `block '{name}': …`, so a failure in a long ingest can be located.
2. **Document the extra per-commit parse.** `write.rs:244`: every commit now parses its journal line once more. The CPU cost is proportional to entry size, which is small next to encoding but new per leaf. State it in the rustdoc.
3. **The duplicate-name check is O(n²) over a session.** `write.rs:821` does a linear scan per append. This predates #765, but #759 is about scaling with leaf count, and at the consumer's 573 leaves (or 10⁴+) the scan is quadratic. Use a `HashSet<name>` beside `blocks`, or prove it irrelevant with a measurement.

Acceptance: (1) a test that asserts the block name appears in the error; (3) either a set-based check with a test, or a recorded measurement showing it is negligible.

