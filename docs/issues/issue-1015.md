---
type: issue
state: open
created: 2026-11-27T02:31:51Z
updated: 2026-11-27T08:59:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1015
comments: 0
labels: chore, docs, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:30:55.046Z
---

# [Issue 1015]: [nits from the #1006 review (injective member names)](https://github.com/vig-os/tessera/issues/1015)

From the round-2 review of PR #1006 (#994):
1. Some doc comments still describe member files as raw `<reference>.tsra` or `<id>.tsra`, without the encoding. They are in `tessera-cli/src/collection.rs:9,269,354,665`, `tessera-cli/src/main.rs:875,2314` and `tessera-ingest/src/engine.rs:1101`. Each should give the encoded name and refer to `tessera_core::names::file_name`.
2. `tessera-io/src/collection.rs:68` is an unsplit string literal 121 characters long. Two edited lines in ADR-0049 are 125 and 204 characters long. Wrap them.

Refs: #994
