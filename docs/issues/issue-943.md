---
type: issue
state: open
created: 2026-11-21T01:31:27Z
updated: 2026-11-21T01:31:27Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/943
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-21T09:10:52.874Z
---

# [Issue 943]: [readers trust .cidx aggregates without checking them against the block they index](https://github.com/vig-os/tessera/issues/943)

Found in the review of PR #942 (#720). A product can carry a derived `volume.cidx` whose stats lie (min = max = 7 for a recon_int16 volume whose real range is -1024..3071). `tessera verify` and the reference reader both pass it, because the seal covers the index bytes as written. `tessera stats` then reports the lie as `source chunk-index-aggregate (exact)`.

Since #720, derived blocks are outside `content_hash`. A lying index is still tamper-evident through `manifest_hash`, but nothing checks it against the data it indexes. PR #942 closes the copy path (`commit --add-block` rebuilds indexes and does not copy them). The general question is still open: should `verify`, or a `verify --deep`, recompute derived blocks from their primary block? Or should `stats` label index-sourced values as writer-attested rather than exact? ADR-0059 is the place to decide.

Refs: #720
