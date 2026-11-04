---
type: issue
state: closed
created: 2026-11-03T20:45:31Z
updated: 2026-11-04T04:58:49Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/759
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:51.686Z
---

# [Issue 759]: [fix(io): WriteSession keeps every committed leaf's spec as a serde_json::Value tree until seal (~9.7 KB/leaf of duplicated column specs)](https://github.com/vig-os/tessera/issues/759)

Found by the ADR-0064 PR G proof (#715). The diagnosis is `docs/spikes/715-rss-proof-diagnosis.md` on `test/715-ksb-rss-proof` @ 252af87, measured with heaptrack on ksb (jobs `tessera-715-r2-heap*`).

`WriteSession::blocks` (`tessera-io/src/write.rs:227`) keeps every committed leaf's `BlockRef` until the seal. Each entry is pushed at `write.rs:880` from the clone at `write.rs:763`, while the caller drops the original (`stream.rs:1151`). Every entry carries its spec as a `serde_json::Value`/`IndexMap` tree. On the consumer's 12-column schema, the live heap grows 12.8 KB per leaf, and RssAnon about 25.4 KB per leaf once allocator retention is included:
- (a) ~3.1 KB is the manifest's per-leaf content, which is needed until the seal;
- (b) **~9.7 KB is the Value/IndexMap form**, mostly 12 column specs identical in every leaf;
- (c) ~12.6 KB is allocator retention from that churn.

This is O(leaves) heap that the format doesn't need. It is the main reason ADR-0064 §8.1 criterion 2 (plateau) fails at W=1: 59.5 → 71.7 MiB over 64 → 573 blocks.

**Fix:** keep per-leaf state in the compact form the manifest needs, either as canonical bytes or as typed structs with the table schema shared (Arc'd) across the table's leaves, instead of a cloned `Value` tree per leaf. The sealed bytes must not change.

**Tests:**
- A counting-allocator test that the held heap per committed leaf is ≤ the serialized manifest entry plus a stated constant, shown failing before the fix.
- The manifest and `content_hash` are unchanged on the existing corpus.

Refs: #715
