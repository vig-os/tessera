---
type: issue
state: open
created: 2026-11-17T21:51:21Z
updated: 2026-11-19T05:26:00Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/901
comments: 1
labels: bug, priority:medium, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:10.495Z
---

# [Issue 901]: [bug(integrity): a respelled number (64.0 for 64) keeps the seal and splits readers (Rust refuses, Python/JS accept)](https://github.com/vig-os/tessera/issues/901)

Found by the #897 seal-encoding spike (PR #900; reproducible with its `run.sh`).

Under the current seal, which hashes `JCS(parsed manifest)`, every byte-level respelling of a manifest keeps the seal: 59 of 59 tested. That is expected for a re-serialising seal. One class also splits readers: a sealed integer field respelled as `64.0` verifies in all 7 of 7 cases tested.
- Rust refuses the block, because its typed integer fields reject `64.0`. So Rust fails closed.
- The Python reference reader reads a float.
- JavaScript reads `64`.

The Rust verifier is not bypassed, but the reference reader disagrees with Rust, contrary to the alpha.6 claim that both readers agree.

**Root-cause fix:** ADR-0066 option A′ (hash the stored bytes and refuse non-canonical bytes), if accepted. Otherwise, make the reference reader refuse a non-integral spelling wherever the type table declares an integer, plus a shared seal case. Decide after ADR-0066.
---

# [Comment #1]() by [gerchowl]()

_Posted on November 19, 2026 at 05:26 AM_

This issue is step 4 (closed by #914) of the ADR-0066 implementation plan (epic #897; full order in the plan comment there). Its scope and acceptance criteria are in docs/adr/0066-seal-encoding.md, under Implementation plan.

