---
type: issue
state: open
created: 2026-11-11T01:18:42Z
updated: 2026-11-11T04:19:39Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/810
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:37.012Z
---

# [Issue 810]: [feature(io): validate-before-pack seal, session with_schema and with_ingest_transform (ADR-0065 §8-§9, #801)](https://github.com/vig-os/tessera/issues/810)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR #819). **Blocked until the owner accepts ADR-0065**; do not start before. Implements the seal half of #801 (gaps G4, G5).

## What
- `WriteSession::seal` (`tessera-io/src/write.rs:1065`) gains a validate-before-pack path: the sealed manifest is checked against its product schema (embedded or supplied) **before** `pack_streaming` writes the archive, so a refused product leaves no output file. Today no seal path validates.
- Session-header `with_schema` and `with_ingest_transform` setters, persisted like `with_generation`, applied at seal through `ProductBuilder::with_schema` / `with_ingest_transform` (`tessera-core/src/product.rs:113`, `:141`).
- (`StreamWriter::finish_with` and `abort` are #831.)

## Acceptance
- A session missing a required field / block refuses at seal and writes no file (fails before).
- A streamed product with transforms and a supplied schema is byte-identical to the batch `ProductBuilder` product of the same inputs.
- Header persistence survives `recover`.

Size: ≤ ~300 lines. Depends on: #809.
