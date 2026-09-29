---
type: issue
state: open
created: 2026-07-02T12:13:53Z
updated: 2026-09-28T17:59:49Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/289
comments: 0
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:57.525Z
---

# [Issue 289]: [Evaluate virtual / external-reference chunks (kerchunk / Icechunk pattern)](https://github.com/vig-os/tessera/issues/289)

Icechunk chunk refs may point to a **virtual URL** (a byte range in an external file), alongside inline and `$ROOT/chunks/{id}`. Same mechanism as kerchunk/VirtualiZarr.

Relevance to tessera:
1. **Validates the OME-NGFF store-facade** over a sealed `.tsra` (chunk-key → in-container range read; the chunk index already is the offset/length map).
2. Enables **cloud-read-without-copy** / referencing vendor files by range rather than duplicating bytes.

Deliverable: prototype the reference set over a `.tsra` and confirm a real Zarr renderer round-trips it. Refs: #286
