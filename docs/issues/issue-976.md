---
type: issue
state: open
created: 2026-11-24T20:59:37Z
updated: 2026-11-24T20:59:37Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/976
comments: 0
labels: chore, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:46.925Z
---

# [Issue 976]: [nits from the #972 review (parquet 58.4.2)](https://github.com/vig-os/tessera/issues/976)

1. `tessera/crates/tessera-ingest/src/decoder.rs:31` still shows `"=58.4.1"` in its doc example.
2. The commit message and PR body say "7 parquet fixtures". It is 6 `arrow-rs/parquet` fixtures plus 1 `arrow-rs/arrow-ipc` fixture.

Refs: #970
