---
type: issue
state: open
created: 2026-11-18T20:57:36Z
updated: 2026-11-18T20:57:46Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/909
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:05.832Z
---

# [Issue 909]: [feat(io): StreamWriter states ARMED → RECORDING → seal / disarm, with in-writer pre-trigger retention and a write preflight() report](https://github.com/vig-os/tessera/issues/909)

Context: the DAQ discussion for the kit's acquisition sessions (2026-11-18). An instrument stream should go through **one** pipeline whether it's being watched, armed for a trigger, or recorded, with no second buffer for any of them. A write also needs a preflight, the way a device command has a safety envelope.

## Proposal A: writer states

```
 open ──▶ ARMED ──record──▶ RECORDING ──stop──▶ seal ──▶ .tsra
            │  ▲                 │
            │  └─────pause───────┘   (optional, per lane or session)
            └──disarm──▶ discarded (RAII: staging cleared, no journal entry, nothing sealed)
```

**ARMED:**
- **Everything that can fail is done up front:** the session is created and staging provisioned (optionally `fallocate`d), lanes and schemas opened and validated, the producer key loaded (#795) and the window allocated.
- **Batches flow:** the ingress tap (#908) serves live views, and nothing is committed or sealed.
- **Optional pre-trigger retention:** the last `N` seconds or rows per lane stay in staging, *in the writer's own buffer and staging*, not a second copy. Retention past the RAM window spills to staging on disk (at 18 GB/s, 5 s is about 90 GB). Fragments that age out are dropped, RAII as today.

**`record`:**
- The retained pre-trigger rows become the product's first rows, and later pushes follow.
- `seq` and `content_hash` cover only what was recorded, so the bytes are deterministic given the recorded rows, regardless of how long the writer was ARMED.
- The record instant and the pre-trigger depth go into sealed provenance (`Generation`).

**`disarm`** discards everything; nothing is sealed. "Monitor only" is ARMED with no record.

## Proposal B: `preflight()` → a report

A write has an envelope too. Before ARM succeeds, `StreamWriter::preflight(plan)` checks a declared plan (rate per lane, duration, expected compression ratio, pre-trigger depth) and returns a typed report:

| Check | Basis |
|---|---|
| **Free disk:** ≥ planned bytes ÷ ratio, plus scratch `≈ 2·T + J·S` | ADR-0026's disk paragraph |
| **Sustained write speed:** of the staging filesystem ≥ planned rate × margin, from a short measured probe | |
| **Staging filesystem:** disk-backed, not tmpfs; output on the same filesystem (commit by rename) | ADR-0064 §8.1 already refuses tmpfs for its memory proof |
| **Encode capacity:** `W` × measured per-core encode rate ≥ planned rate | the `WriteConfig::balanced` model |
| **RAM:** the window (`J`, `B`) plus the encoders' working sets fit the budget | the #677 bound and ADR-0064 §8's `p_block` |
| **Schema and lanes:** validate, and every `requires_generation` field is present | |
| **Producer key:** loadable, if checkpoints are on (#795) | |

**Live counterparts:** during RECORDING, `StreamStats` exposes free-disk headroom, `producer_stalls` and window saturation, so a host can warn on a trend and stop and seal cleanly before the disk fills.

Backpressure stays the caller's choice. `push` blocks as today; a host that can't block (a detector) decides fail-the-run or drop-and-count. tessera only reports.

## Acceptance

- [ ] **States are enforced and typed:** ARMED → RECORDING → seal; ARMED → disarm leaves nothing (no staging, no journal line). Invalid transitions are typed errors.
- [ ] **Pre-trigger bytes:** a product recorded after 10 s ARMED with 2 s pre-trigger has the same `content_hash` as a batch write of exactly those rows.
- [ ] **Pre-trigger heap:** retention beyond the RAM window is on disk, and a heap test proves the heap is bounded by the window, not by the depth.
- [ ] **`preflight()` refusals:** it refuses tmpfs staging, insufficient disk and insufficient encode capacity, each with a typed reason. A passing plan records the report in unsealed `aux/` (wall-clock and host facts, per ADR-0042).
- [ ] **No change to identity rules:** sealed bytes depend only on recorded rows and pinned config.

Related: #203, #677, #794, #795, #907, #908, ADR-0026, ADR-0042, ADR-0058, ADR-0064, ADR-0065.

