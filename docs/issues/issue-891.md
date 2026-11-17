---
type: issue
state: open
created: 2026-11-16T10:58:25Z
updated: 2026-11-16T10:58:25Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/891
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-17T10:09:18.274Z
---

# [Issue 891]: [format: explicit canonical defaults are a second sealed spelling (decide canonical form before 1.0)](https://github.com/vig-os/tessera/issues/891)

Found in #883 (#873) fix round 2.

An explicit canonical default (`study: null`, `members[].kind: "product"`, `sequence_key: []` …) and the omitted field seal as two distinct objects (two `manifest_hash` values) with the same `content_hash`. Non-canonical spellings of a default (e.g. `{"product": null}`) are refused since #883. Readers agree on both spellings and nothing is hidden, so this is version-id malleability, not a verify bypass.

SPEC §4 currently accepts explicit defaults, and tracked fixtures emit them (seal case `study-null-resealed`, the #801 drafts' `sequence_key`/`keys`/`references: []`), so refusing them would break existing products.

Owner decision before 1.0: (a) keep accepting and document that a product's identity is `content_hash` and `manifest_hash` is spelling-sensitive; or (b) define one canonical form (omit defaults), refuse the explicit spelling for new seals behind a format version, and migrate the fixtures.

Refs: #873
