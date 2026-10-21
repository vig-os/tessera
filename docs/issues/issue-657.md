---
type: issue
state: open
created: 2026-10-21T00:31:07Z
updated: 2026-10-21T00:31:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/657
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-21T07:01:43.094Z
---

# [Issue 657]: [feat(format): recipe-only (view) product: sealed query/derivation recipe with input edges and no data blocks](https://github.com/vig-os/tessera/issues/657)

Relates ADR-0028 (derived sidecars are content-addressed + recipe-stamped; materialised views listed), ADR-0054 (derived caches, built_from), ADR-0033/0058 (derivation and provenance edges), #654 (streaming TableProvider).

## Problem
A consumer defines analysis **views** (DataFusion SQL + UDF versions + parameters) over one or many sealed products. The recipe is the reproducible, shareable unit ("preset X v2 on these N scans with calibration v3"), but today a recipe exists only as a stamp on a materialised derived product. A view cannot be sealed, shared, verified or described on its own before (or without) materialising its rows, which may be large and are rebuildable anyway.

## Ask
A product kind with **no data blocks**: a canonical recipe (query text, engine/UDF versions, parameters) + `derived_from` edges to the sealed input identities (or a Collection), sealed and content-addressed like any product.
- Materialisations reference it (`recipe = <recipe product id>`, `built_from = input seals`) and are stale when either differs.
- `tessera inspect` / the reader can execute it through the streaming TableProvider (#654).

## Acceptance
Seal recipe → same id for the same canonical recipe and inputs; executing it equals the materialised product; changing an input seal or a parameter yields a new id; a materialisation stamped with an old recipe id is detected as stale.
