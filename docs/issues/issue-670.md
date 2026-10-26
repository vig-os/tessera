---
type: issue
state: open
created: 2026-10-25T11:18:57Z
updated: 2026-10-25T11:18:57Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/670
comments: 0
labels: priority:low, effort:medium, spike, area:integrity
assignees: none
milestone: none
projects: none
parent: 654
children: none
synced: 2026-10-26T07:24:20.462Z
---

# [Issue 670]: [spike(format): per-segment sealed digests so projected reads can be verified (ADR-0062 §7)](https://github.com/vig-os/tessera/issues/670)

Deferred by ADR-0062 §7 (#654). The format verifies payloads only at block granularity (`BlockRef.digest`).
`VerifyBlocks` therefore reads every touched block whole, whatever the projection and zone pruning would have
skipped, and `SealOnly` leaves the payload bytes it reads unverified.

**Question:** should the format carry a sealed digest per Vortex segment (or per column per row group), so a
projected, pruned read can be verified without fetching the whole block? It would be its own format event
(it moves `content_hash` if sealed), in the spirit of ADR-0059's "one event". Needs a design: what is hashed
(stored segment bytes, not decoded values), where it lives (footer-adjacent sealed block vs `.cidx`), its cost,
and how a reader maps Vortex segment requests onto it.

