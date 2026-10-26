---
type: issue
state: open
created: 2026-10-21T00:31:07Z
updated: 2026-10-25T09:11:23Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/657
comments: 1
labels: feature, priority:medium, effort:large, area:core, adr
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-26T07:24:24.913Z
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
---

# [Comment #1]() by [gerchowl]()

_Posted on October 25, 2026 at 09:11 AM_

**Triage (architect, 2026-10-25): accepted. It adds a new product kind, a format change, so an ADR comes first. Blocked on #654.**

The ADR (extending ADR-0028/0054/0058) has to settle:
- **Canonical recipe form:** query text plus engine and UDF versions plus parameters. Canonicalisation (JCS) and what is identity versus annotation.
- **The no-data-blocks kind:** how readers that don't know it behave. This interacts with #573: today an unknown field fails verify.
- **`derived_from` edges:** to input seals, or to a Collection.
- **Staleness:** a materialisation is stale when its `recipe` id or any `built_from` seal differs.
- **Engine-version drift:** whether a recipe sealed against engine N still executes on N+1, and how that is reported.

Execution goes through the #654 provider, so implementation waits for it. The ADR can be drafted earlier, once the #654 ADR has fixed the provider's contract.


