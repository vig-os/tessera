---
type: issue
state: open
created: 2026-07-02T12:13:54Z
updated: 2026-09-28T17:59:51Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/290
comments: 0
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:57.198Z
---

# [Issue 290]: [Icechunk interop bridge (import snapshot → .tsra; export lineage → Icechunk repo)](https://github.com/vig-os/tessera/issues/290)

Reach the versioned-Zarr ecosystem (Earthmover Icechunk/Arraylake; geo/climate/ML) **without adopting Icechunk as the format**.

- **import:** an Icechunk snapshot → a sealed `.tsra` product (arrays; seal + optionally sign on the way in).
- **export:** a tsra CoW lineage → an Icechunk repo (arrays only; Vortex tables + blobs stay tsra-native).

Depends on the storage-format ADR decision. Refs: #286
