---
type: issue
state: open
created: 2026-06-25T11:39:36Z
updated: 2026-09-28T17:59:34Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/198
comments: 0
labels: priority:blocking, area:testing, spike
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:06.722Z
---

# [Issue 198]: [[P0][spike] S15 remainder: cross-version/arch determinism + vendored-reader prototype](https://github.com/vig-os/tessera/issues/198)

**Phase P0 — closes the only open §C correctness risk.** ROADMAP: tessera/docs/ROADMAP.md

S13 (bit-exact) + S15 same-version determinism already PASS. Remaining: byte-identical across codec releases / CPU arch. Hedge = pin codec versions + ship a vendored reader.

**Done-gate:** vendored reader decodes a pinned-version file; cross-arch delta characterised. (FEATURE-MATRIX §C.)
