---
type: issue
state: open
created: 2026-09-29T15:28:07Z
updated: 2026-09-29T15:36:38Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/526
comments: 2
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:04.158Z
---

# [Issue 526]: [ADR: hash-algorithm agility — explicit hash_alg tag + optional SHA-256/512 fixity co-hash (keep blake3 as identity)](https://github.com/vig-os/tessera/issues/526)

Preservation practice (BagIt RFC 8493 §2.4, OCFL 1.1 `digestAlgorithm` and `fixity`) expects SHA-256/512. tessera hard-wires blake3 (`hash.rs`, and `blake3:` prefixes throughout) with no algorithm field.

**Keep blake3 as the identity hash.** It was chosen for parallel, tree-structured hashing that matches the per-block Merkle (hash.rs:3), and that stays.

Add:
1. An explicit `hash_alg` tag, so a future migration of the primary hash is an explicit event rather than a break.
2. An OPTIONAL per-block SHA-256/512 *fixity* co-hash (parallel across blocks; SHA-NI roughly 1–2 GB/s per core), either computed at seal time or only on BagIt/OCFL export.

Decide whether (1)/(2) join the single chunk-index format event (#347 spike), to avoid a second corpus regeneration.
---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 03:28 PM_

**Owner direction (2026-09-29):**
- **blake3 stays the default and favoured identity hash.** Its parallel, tree-structured hashing over the per-block Merkle is the point and must not regress.
- **SHA-256 only at the TOP level, and only when required**, never inside the tree.
- **Preferred shape:** hold both in parallel at the top level. The manifest carries the blake3 `content_hash` (identity) plus an OPTIONAL top-level `sha256` fixity value, computed in the SAME single streaming pass at seal time (both hashers fed from one read, so no second pass over the data). The blake3 tree path is unchanged.
- An explicit `hash_alg` label makes a future primary-hash migration an explicit event.

Open design points for the spike:
- The top-level SHA-256 has to be over something well-defined and reproducible from the file: the concatenated block payloads in manifest order, or a sha256 Merkle-list over the blocks. A sealed file can't hash itself.
- Whether the SHA-256 is computed by default at seal time or only on request / BagIt-OCFL export (#527).
- The exact cost on a large file (SHA-NI throughput vs blake3's parallel throughput) should be measured, not assumed.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 29, 2026 at 03:36 PM_

Owner decision: the per-block SHA-256 co-digest ships in ADR-0059's format event, OFF by default (seal flag or BagIt/OCFL export).

