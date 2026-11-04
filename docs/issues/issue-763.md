---
type: issue
state: open
created: 2026-11-04T01:25:43Z
updated: 2026-11-04T01:25:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/763
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:49.588Z
---

# [Issue 763]: [perf(io): attach aux members (identity envelope) to multi-GB archives in bounded memory](https://github.com/vig-os/tessera/issues/763)

## Context
MorePET/visionx-rs writes multi-GB stage-0 packs with `pack_streaming_to` (bounded memory) and must attach an ADR-0047 identity envelope as an aux member (MorePET/visionx-rs#138, MorePET/visionx-rs#162).

## Gap (at `4504efb`)
- `pack_streaming_to` (`tessera-io/src/container.rs:243`) takes no aux members.
- `write_aux_members` rebuilds the archive in memory:
  - `container.rs:754` collects `Vec<(String, Vec<u8>)>`;
  - `:766` calls `read_to_end` on every entry;
  - `:864–883` writes them back.

  Peak memory is about the archive size, so attaching an envelope to a 7–17 GB pack is not feasible in bounded memory.

## Ask
One of the following, with bounded memory independent of the blob sizes:
- `pack_streaming_to` accepts aux members, written outside the seal like `sign` and `--crypto-shred`;
- `write_aux_members` streams entries instead of buffering them.

