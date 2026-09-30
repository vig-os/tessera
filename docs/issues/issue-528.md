---
type: issue
state: open
created: 2026-09-29T15:28:11Z
updated: 2026-09-29T15:28:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/528
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:02.915Z
---

# [Issue 528]: [feat(format): Payload-Oxum-style pre-check — byte and member counts verified before the Merkle walk](https://github.com/vig-os/tessera/issues/528)

BagIt's Payload-Oxum (octet count plus file count) is an O(1) sanity gate before hashing. Add an optional manifest field (total payload bytes plus member count) that the reader checks first: it catches truncation or tail corruption on cloud/OCI reads before fetching gigabytes. Additive and optional; an absent field means unchanged behaviour.
