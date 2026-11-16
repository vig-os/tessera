---
type: issue
state: open
created: 2026-11-15T12:01:18Z
updated: 2026-11-15T12:01:18Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/868
comments: 0
labels: area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:40.220Z
---

# [Issue 868]: [follow-up(#832): lane/block-name binding, running_root cost, CHANGELOG, shared job credit](https://github.com/vig-os/tessera/issues/868)

Review nits on PR #867 (lanes on StreamWriter/WriteSession, head 242433b), deferred per the nits-to-follow-ups rule.

1. **Lane not tied to block name.** If one table's leaves are pushed on two lanes, `seal_order` silently reorders the partitions (`events/0001` before `events/0000`). Refuse it with a typed error, or document the rule, and add a test that fails today.
2. **`running_root` (write.rs ~1259–1266).** It calls `.expect()` twice in a public non-test fn. With more than one lane it also re-parses the JSON of every block on each call, which is O(blocks) per poll. Remove the expects, and cache or make it incremental.
3. **Root `CHANGELOG.md` Unreleased.** Add `Lane`, the `*_on` methods, and the forward-compatibility note: older builds cannot recover table-lane journals.
4. **Shared `max_in_flight` credit.** A slow lane can stall pushes on other lanes at gate 1. Document this in the module docs, or give each lane its own credit, measured against ADR-0065 §7.3's "no table waits for another".
5. **Unverified.** Re-serialising a recovered table-lane `BlockRef` through `serde_json::Value` may not reproduce the line byte for byte for unusual numbers. Add a property test.

Refs: #832
