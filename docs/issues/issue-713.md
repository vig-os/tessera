---
type: issue
state: open
created: 2026-10-30T01:11:57Z
updated: 2026-11-02T03:45:12Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/713
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: 677
children: none
synced: 2026-11-02T08:51:36.381Z
---

# [Issue 713]: [feat(io): StreamWriter::push_table, a disk-staged streaming partition (ADR-0064 PR E)](https://github.com/vig-os/tessera/issues/713)

PR E of ADR-0064's implementation plan (§5, §8). The #677 writer.

**Scope:**
- `StreamWriter::push_table(name, spec, groups)`: partitions at `BLOCK_ROWS`, names/marks leaves (PR D), stages each
  block's input to disk in the session's `staging/` (fragment format of `TableMultiBlockSink`), dispatches each block
  to a free worker as soon as it is complete; names and `last` are known from `spec.rows` up front; block `b + 1`'s
  staging file is created only after block `b` is handed to a worker (ADR-0064 §5.2-§5.3); a `with_block_rows`
  test seam. Each leaf is a job under the ADR-0026 window.
- `StreamStats::{peak_staged_input_blocks, peak_staged_input_bytes}` counting a block from the creation of its
  staging file (partial included) until its worker deletes it.
- `table_job_streaming`: unchanged at ≤ `BLOCK_ROWS`; above, a typed error naming `push_table`; `#[deprecated]`.

**Acceptance (fails before):** the barrier test of ADR-0064 §8 (all `W` workers held; `peak_staged_input_blocks ==
W + 1`, a partial block's bytes counted, no more after further input is offered; product equals the batch partition on
release); `push_table` seals a canonical manifest equal to `TableMultiBlockSink`'s (minus `producer`, `timestamp`, `id`,
`id_inputs`, `manifest_hash`; ADR-0064 §6) and the same `content_hash` for `W ∈ {1, 2, 4}`, shown failing once against a `push_table` that misnames a leaf and once against one that drops
the marker,
including nullable columns (the consumer's nullable `ms`); ≤ `BLOCK_ROWS` rows is byte-identical to
`table_job_streaming`.

---
Design: ADR-0064 "Logical tables over numbered leaves" (`docs/adr/0064-logical-tables-over-numbered-leaves.md`,
Proposed, PR for #677). Implement only after the ADR is Accepted. Sub-issue of #677; order A → B → C1, C2 → D → E → F → G → H
(D lands only after B, C1 and C2, so no writer on `dev` writes a form a reader on `dev` can't read).


