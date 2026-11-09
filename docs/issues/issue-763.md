---
type: issue
state: open
created: 2026-11-04T01:25:43Z
updated: 2026-11-08T20:11:16Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/763
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-09T09:04:25.509Z
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

---

# [Comment #1]() by [gerchowl]()

_Posted on November 8, 2026 at 08:11 PM_

Consumer status: on MorePET/visionx-rs, the stage-0 raw pack is now complete except for the identity envelope. That covers the typed allowlisted header tree in the manifest and a CI leak oracle, on visionx-rs main at 04b9084.

The remaining piece, the crypto-shredded ADR-0047 envelope inside the raw pack, is tracked in MorePET/visionx-rs#186. It is blocked on this issue. We aren't working around it; until this lands, the extension point stays documented and unimplemented.

