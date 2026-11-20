---
type: issue
state: open
created: 2026-11-20T03:55:37Z
updated: 2026-11-20T03:55:37Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/936
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:07.322Z
---

# [Issue 936]: [api: mark FieldSpec #[non_exhaustive] (or give it a builder) before the API freeze](https://github.com/vig-os/tessera/issues/936)

PR #934 (#895) adds public fields `encoding`, `minimum` and `maximum` to `FieldSpec`. This breaks every struct-literal construction, and the next rule will break them again. Before the API freeze, decide whether to mark `FieldSpec` (and its sibling schema structs) `#[non_exhaustive]` and add a constructor or builder.

Refs: #897
