---
type: issue
state: open
created: 2026-10-21T00:01:04Z
updated: 2026-10-25T09:11:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/655
comments: 1
labels: feature, priority:high, effort:medium, semver:minor, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-26T07:24:25.831Z
---

# [Issue 655]: [feat(ingest): batch ingest mode for the stream writer: no per-fragment fsync/raw staging when the source is re-readable](https://github.com/vig-os/tessera/issues/655)

## Problem
The streaming writer (ADR-0026, `tessera-io/src/accumulate.rs`) stages uncompressed fragments and fsyncs every 2¹⁶ rows. That is the right durability model for **live** acquisition (crash recovery to the last fragment). For **batch** conversion of an existing, immutable, re-readable source file it is pure cost: for a ~19 GB source, about 90 GB of scratch space and ~37k fsyncs. The writer, not the decoder, then sets throughput. The docs also report only ~3× scaling with the encode pool.

## Ask
- A batch mode (explicit opt-in) where the bounded ring → parallel encode pool feeds compaction directly, with no durable raw staging and no per-fragment fsync. Same canonical bytes and same `content_hash` as the live mode (worker-count and mode independent; gate it in the determinism corpus).
- Document a scratch-space budget for each mode.
- Find and document where the encode pool saturates (committer, codec, IO).

## Acceptance
Same sealed product in both modes (hash-equal); measured scratch and throughput for both modes in the performance book.
---

# [Comment #1]() by [gerchowl]()

_Posted on October 25, 2026 at 09:11 AM_

**Triage (architect, 2026-10-25): decided, ready to implement. No format change, so no new ADR; the PR amends the ADR-0026 text.**

Decisions:
- **Opt-in:** an explicit batch mode on the stream writer's config, which defaults to the current live mode. Batch skips the durable raw staging and the per-fragment fsync, and feeds the encode pool straight into compaction. The rustdoc and the CLI flag say plainly that a crash in batch mode loses the in-progress product. That is the trade-off, and it's only acceptable because the source is re-readable.
- **Identity:** same canonical bytes and the same `content_hash` in both modes and at any worker count. This is gated in the determinism corpus (Gate A style): one input, both modes, several worker counts, hash-equal.
- **Measurements, published in the performance book for both modes:**
  - the scratch budget per mode;
  - throughput on a large source, run as a ksb job, not on the seat;
  - where the encode pool saturates (committer, codec or IO). That number comes from a profile, not a guess. If the saturation point needs its own fix, it becomes a separate issue.

Order: second in the next wave, after #632.


