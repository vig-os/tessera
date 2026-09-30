---
type: issue
state: open
created: 2026-09-29T10:28:33Z
updated: 2026-09-29T10:28:33Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/508
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:08.914Z
---

# [Issue 508]: [In-tree decoder identity without per-release churn](https://github.com/vig-os/tessera/issues/508)

## Problem

`.npy` is the first ingest lane whose decoder is **our own code** (ADR-0056 §12 declined `ndarray-npy`:
"NPY is a header parse plus a memcpy"). Its sealed `ingest_decoder` record is therefore just
`{"name": "tessera/npy"}` — no version, no feature digest — because the two available components are both
wrong to record:

- **A feature digest** would be taken over an empty pre-image, which is *identical for every build* and so
  a false claim of sameness. `decode_path` already rejects that shape.
- **Our crate version** would move the `manifest_hash` of every `.npy` product on **every release**, with
  nothing having changed about how the bytes were read. That is exactly the churn ADR-0052 §1 removed and
  that #477 was careful not to reintroduce — `build.rs` excludes our version from the pre-image for this
  reason, in those words.

So the record is honest but **incomplete in one respect**: it does not pin *which* in-tree parser ran. If
the `.npy` header parse changes behaviour, two products sealed by different parsers carry byte-identical
decoder records.

This is not `.npy`-specific. `nifti` and the `blob` family are also in-tree, and any future hand-written
decoder inherits the same gap.

## What would close it

An identity for an in-tree decoder that moves **when the parser changes** and not when the version does.
The obvious candidate is a digest over the parser's own sources, emitted by `build.rs` alongside the
existing per-lane pre-images:

```
"ingest_decoder": { "name": "tessera/npy", "source": "blake3:…" }
```

Open questions, none of them settled:

- **Which files.** `npy.rs` alone is too narrow (it calls shared canonicalisation); the whole crate is too
  wide (it would move on a CSV-only edit, re-creating cross-lane coupling — #477's defect in a new place).
  A declared per-lane source set, gated the way `IN_DIGEST` is, is the shape that matches how the
  third-party side was solved.
- **Churn profile.** Better than per-release, but every refactor of a named file still moves it. Whether
  that is acceptable depends on whether a reader wants "the parser changed" or "the parser's *behaviour*
  changed" — and only the first is mechanically available.
- **Comments and formatting.** A digest over raw bytes moves on a comment edit. Tokenising first is more
  faithful and much more machinery.
- **Whether it belongs in the seal at all**, versus `aux/`. ADR-0042's line is "sealed = what changed the
  bytes", and the parser *is* what changed the bytes — but so is the compiler, which is not sealed either.

## Scope

Beta, not alpha. The current record is correct as far as it goes and the corpus gate already catches a
behaviour change in the parser via `content_hash`; this closes a provenance gap, not a correctness one.

Refs: #386

