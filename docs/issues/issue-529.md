---
type: issue
state: open
created: 2026-09-29T15:28:12Z
updated: 2026-09-29T15:28:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/529
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:02.217Z
---

# [Issue 529]: [feat(format): reserved manifest.info block for BagIt reserved fields + a plain-text format declaration](https://github.com/vig-os/tessera/issues/529)

For the 'readable in 30 years with only unzip and a text editor' goal: reserve a `manifest.info` block with a convention mirroring BagIt's reserved bag-info fields (Bagging-Date, Source-Organization, External-Identifier, Contact-Email, …), and optionally a plain-text `TESSERA.txt` format declaration inside the zip (what this is, where the spec lives, how to verify it). Additive.
