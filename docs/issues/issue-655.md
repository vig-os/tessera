---
type: issue
state: open
created: 2026-10-21T00:01:04Z
updated: 2026-10-21T00:01:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/655
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-21T07:01:44.261Z
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
