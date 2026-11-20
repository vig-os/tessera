---
type: issue
state: open
created: 2026-11-20T03:55:36Z
updated: 2026-11-20T03:55:36Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/935
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:07.582Z
---

# [Issue 935]: [schema: metadata number rules do not check the field's dtype (an int32 field accepts 1.5)](https://github.com/vig-os/tessera/issues/935)

Found in the review of PR #934 (#895). `check_metadata` enforces `encoding` and `minimum`/`maximum` but not the declared dtype: an `int32` field accepts `1.5`, and an `int32`/`uint8` field accepts values outside the dtype's own range unless the schema also declares a range. Rust and the Python reference reader agree on this (the docstring says so), and #934 pins it as a schema case.

Decide whether the dtype should be a binding rule (integer dtypes refuse fractions and out-of-dtype values; unsigned refuse negatives). If yes, it moves no seals, but it does refuse metadata that verifies today.

Refs: #897
