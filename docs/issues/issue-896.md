---
type: issue
state: open
created: 2026-11-17T16:16:44Z
updated: 2026-11-17T16:16:44Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/896
comments: 0
labels: chore, priority:low, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-18T09:43:22.690Z
---

# [Issue 896]: [chore: I-JSON refusal follow-ups from the #894 review (builder consumed on refusal, error context)](https://github.com/vig-os/tessera/issues/896)

Follow-ups from the adversarial review of PR #894 (#886). These are nits, not blockers:
- **Python `Builder` consumed on refusal:** `pack` calls `take()` before `seal()`, so a refused `pack` leaves the builder empty. The user can't fix the value and retry. Refuse before consuming, or restore the builder on error.
- **WriteSession errors don't name the block:** a refusal says `spec.fill_value` without saying which block.
- **Rust and Python error classes differ:** for integers of 400+ digits, Rust says "out of range" and Python says "inexact". The accept/refuse verdict agrees in both.
- **Reviewer trap:** cargo hashes path dependencies relative to the workspace, so a shared `CARGO_TARGET_DIR` reused across checkouts can serve stale rlibs for the base build. Add this to the contributor docs.
(The "error prints the parsed double, not the token" and "`--meta` error names `product[0].metadata`" nits are being handled in #894's fix round where trivial; anything left over gets ticked here.)

