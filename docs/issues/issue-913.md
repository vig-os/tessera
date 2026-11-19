---
type: issue
state: open
created: 2026-11-19T05:25:23Z
updated: 2026-11-19T05:25:23Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/913
comments: 0
labels: feature, priority:high, area:core, area:integrity
assignees: none
milestone: none
projects: none
parent: 897
children: none
synced: 2026-11-19T09:52:04.242Z
---

# [Issue 913]: [feat(core): stored-bytes API and canonical-bytes check (ADR-0066 step 3)](https://github.com/vig-os/tessera/issues/913)

Part of the ADR-0066 implementation plan (docs/adr/0066-seal-encoding.md, merged in #900; epic #897). **Step 3 of 9.**

**Goal:** add the stored-bytes API and the canonical-bytes check to `tessera-core`. The change is additive.

- **Scope:**
  - `canonical::check_canonical(bytes)` parses strictly, refusing duplicate keys, lone surrogates, I-JSON violations and excess depth. It then refuses bytes that differ from their own JCS form, naming the first differing byte.
  - `Manifest::from_stored(bytes, seal)`, `Collection::from_stored(bytes, seal)`, and `to_stored()` on each, returning `(bytes, seal)` with no `manifest_hash` in the bytes.
  - Typed errors for non-canonical bytes and for a seal mismatch.
- **Depends on:** nothing.
- **Acceptance:**
  - unit tests over the spike's cases: T4–T7 and S8 refused as non-canonical; S5b refused by I-JSON, with a test showing the byte comparison alone would pass it; S7 (`64.0`) refused;
  - for all 13 tracked manifests, `blake3(to_stored().0)` equals the recorded seal.

