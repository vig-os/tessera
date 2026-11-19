---
type: issue
state: open
created: 2026-11-18T20:57:35Z
updated: 2026-11-18T20:57:44Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/908
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:06.286Z
---

# [Issue 908]: [feat(io): ingress tap on StreamWriter pushes — lossy, zero-copy live view of rows the writer already holds (companion to #794)](https://github.com/vig-os/tessera/issues/908)

Context: the DAQ discussion for the kit's acquisition sessions (2026-11-18). This is a companion to #794's commit tap.

## Problem

#794 taps **committed** blocks. That is right for durable progress, QA and integrity, but too late for a live view:
- the event comes only after encode + fsync + commit;
- at low rates it arrives one row group (2¹⁶ rows) later, about 1.8 h at 10 Hz (#907).

A live view, such as a strip chart, oscilloscope or detector map, wants "now" at 10–60 Hz.

The rows are already in the writer: `OpenTable`'s `pending` buffer, `TableStreamWriter`'s two row groups, and the job queue. A consumer that wants them today has to run a **second ring in front of the writer**. At GB/s that duplicates the hottest buffer in the system. The writer should expose what it already holds.

## Proposal

`StreamWriter::ingress_tap(lane, IngressTapPolicy) -> IngressTap`, a second tap point next to #794:

- **Fires per pushed batch,** before encode.
- **Lossy and never blocking:** `LatestWins` or `DropOldest`, with bounded capacity and per-tap drop counters. The same shape as #794's `TapPolicy`.
- **No copy:** events carry **shared references** to the batch the producer pushed (`Arc` around the pushed columns), never a copy. If the internal batch type can't be shared without a copy, this issue includes making it shareable.
- **Optional decimation in the tap,** e.g. every-Nth row or a row-range slice, so a 60 Hz view never receives full GB/s batches. The heavier operators stay in `tessera-live` (#794 B).
- **Labelled not durable and not verified.** An event says so in its type: `IngressEvent { lane, seq, rows, columns, durable: false }`. Consumers needing truth use #794 and #795.
- **Works in every writer state,** including ARMED (#909). That is how "monitor without recording" works with no second pipeline.

## Acceptance

- [ ] A parked ingress tap leaves `push` throughput and `peak_in_flight_*` unchanged, and its drop counter rises. Same regression shape as #794.
- [ ] A heap test proves the batch isn't copied: the tap holds a reference, and heap growth is bounded by `capacity × batch` refcounts, not bytes.
- [ ] `content_hash` is identical with and without ingress taps attached (property test).
- [ ] At 10 Hz, an event arrives within one push of the data, independent of row-group fill.

## Out of scope

Transport and wire format: the consumer owns the socket, as in #794. Async API: sync per ADR-0034.

Related: #794, #907, #909, #203, #633, #719, ADR-0028 §5.

