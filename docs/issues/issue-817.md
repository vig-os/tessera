---
type: issue
state: open
created: 2026-11-11T01:18:52Z
updated: 2026-11-11T01:19:34Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/817
comments: 0
labels: docs, area:docs
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:33.075Z
---

# [Issue 817]: [docs(capi): Writing from C/C++ chapter as docs-as-tests, and tessera.hpp (ADR-0065)](https://github.com/vig-os/tessera/issues/817)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR for #800). **Blocked until the owner accepts ADR-0065**; do not start before. Toward #800.

## What
Consumer documentation (ADR-0065 §12, §14):

- a book chapter "Writing from C/C++": lifecycle, ownership, errors, threading, sequence keys, schemas, packaging; its code is the #815 consumer (docs-as-tests, included by ANCHOR regions);
- `tessera.hpp`: a header-only RAII wrapper over the C ABI (no ABI of its own), used by the chapter;
- the FEATURE-MATRIX row moved from ○ to ◑/✓ with its gates.

## Acceptance
- The chapter's code compiles and runs in the H consumer check; mdBook builds.

Size: ≤ ~400 lines. Depends on: #815.


