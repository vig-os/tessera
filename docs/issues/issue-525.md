---
type: issue
state: open
created: 2026-09-29T15:28:05Z
updated: 2026-09-29T15:28:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/525
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:04.503Z
---

# [Issue 525]: [fix(export): RO-Crate writes a blake3 digest under the sha256 key](https://github.com/vig-os/tessera/issues/525)

`tessera-core/src/export.rs:55/68` emits the merkle `content_hash` (a `blake3:…` value) under the RO-Crate/schema.org `sha256` property, and a test pins it (`export.rs:215`: `ent["sha256"] == "blake3:abcd1234"`). Any consumer validating `sha256` will fail or report corruption.

Fix: emit the blake3 value under a correctly-labelled property (a PropertyValue or `identifier` carrying the algorithm), or compute a real SHA-256 when one is claimed. Correct the test.

Found by the BagIt/OCFL review.
