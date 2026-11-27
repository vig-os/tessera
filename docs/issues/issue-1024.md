---
type: issue
state: closed
created: 2026-11-27T07:06:51Z
updated: 2026-11-27T09:54:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1024
comments: 0
labels: chore, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:30:53.558Z
---

# [Issue 1024]: [nits from the #1023 review (typed SchemaMismatch)](https://github.com/vig-os/tessera/issues/1024)

From the review of PR #1023 (#1021). It found no blockers.
1. **Guard against drift (do this one first).** `same_value_contract` now decides through `first_difference` rather than the derived `==`. If `ValueContract` gains a sixth field, the two would silently disagree. Destructure `ValueContract` exhaustively inside `first_difference`, so that adding a field is a compile error.
2. **Stale ADR.** ADR-0064 (around line 274) still shows `SchemaMismatch { leaf: String, detail: String }`. Update it to the typed shape and to `MalformedTableSpec`.
3. **Python bindings.** Python still gets only a message string, with no typed members. Decide whether the Python `TesseraError` should expose `kind`, `column` and `field`.
4. **`ContractField` and `ContractValue`** derive `thiserror::Error` only to get `Display`, so they now implement `std::error::Error`. Implement `Display` directly instead.
5. **`Display` shows Rust debug form** for scale and referencing (`Some(0.001)`, `Some(Referenced { .. })`). Render them readably.
6. **Ruling shape.** The ruling put `table` inside `SchemaMismatch`; the PR carries it in the outer `LogicalTable.name`. That nesting is acceptable, but note it in the ADR.

Refs: #1021
