---
type: issue
state: open
created: 2026-11-24T23:17:01Z
updated: 2026-11-24T23:17:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/977
comments: 0
labels: chore, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:46.528Z
---

# [Issue 977]: [nits from the #951 review (nullable columns in Python)](https://github.com/vig-os/tessera/issues/977)

From the fresh review of PR #951 (merged):
1. `tessera-io/src/table.rs:358`: the `canonicalise` doc still says "the zero-copy export carries the column as given". Nullable columns no longer reach the export through `to_le_bytes`, so the doc is out of date.
2. The masked array's `.data` is read-only, because it comes from `frombuffer`. That matches the old plain-ndarray behaviour, but the Python docs don't say so.

Refs: #804
