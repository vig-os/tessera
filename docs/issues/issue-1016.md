---
type: issue
state: closed
created: 2026-11-27T02:36:04Z
updated: 2026-11-27T07:14:39Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1016
comments: 0
labels: chore, area:testing
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:30:54.411Z
---

# [Issue 1016]: [nits from the #1013 review (derived-block corpus golden)](https://github.com/vig-os/tessera/issues/1016)

From the review of PR #1013 (#981, merged):
1. `check_table.py` ends with `assert checked > 0`, so it passes even when most fixtures are missing. Assert `checked == 13` instead, and assert the fixtures by name (guard the referent, not a proxy).
2. The wasm smoke compares `content_hash` against the manifest's own field, not against a root wasm computed. The real coverage comes from `seal_verdict`, so the assertion is mislabelled. Make it compare against the wasm-computed primary root.
3. The reference reader README rewrap leaves a ragged line (`producer` is a Producer object or a).

Refs: #981
