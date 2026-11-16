---
type: issue
state: open
created: 2026-11-16T01:12:09Z
updated: 2026-11-16T01:12:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/881
comments: 0
labels: area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:34.763Z
---

# [Issue 881]: [follow-up(#812): stall-restart test is sleep-based; poison is not journalled, so recover seals a refused table](https://github.com/vig-os/tessera/issues/881)

Nits from the PR #876 re-review (head 13df09c).

1. **Timing-dependent test.** `sequenced.rs` `every_advance_of_next_restarts_the_stall_deadline` relies on 0.9 s sleeps against a 2 s deadline. A runner that stalls for more than about 1.1 s fails it with a false `Stalled`. Inject a clock (a `Clock` trait or a test-only `Instant` source) so the test is deterministic.
2. **The poison is not persisted.** A poisoned writer leaves its session directory and journal behind, and `WriteSession::recover` can then seal a product with part or none of the refused table. This is documented (SequencedTable docs ~:188, ADR-0065 §5). Decide whether the bridge (#810/#813) needs the refusal journalled, so that recover refuses as well.

Refs: #812
