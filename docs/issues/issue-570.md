---
type: issue
state: open
created: 2026-10-16T21:39:09Z
updated: 2026-10-16T21:39:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/570
comments: 0
labels: feature, security
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:46.839Z
---

# [Issue 570]: [feat: whole-file crypto-shred (erasure by key destruction) — ADR-0055 share is a grant, not erasure; define an at-rest erasure mode](https://github.com/vig-os/tessera/issues/570)

## Context

Dogfooding from MorePET/anatomist#86 (ADR-0007, PR MorePET/anatomist#103) and #91, at tessera `dev` `fa581f5`, with synthetic data only.

anatomist needs **erasure by key destruction for whole products**: each dog's ingest and every product derived from it are encrypted to one per-dog key, and destroying that key must make every copy unreadable. Tessera's documents don't define that verb today:

- **ADR-0040 §2.2** defines crypto-shred (encrypt, then delete the key; "the only erasure that reaches copies off your hosts") for *encrypted fields* (§3, not built).
- **ADR-0047** implements crypto-shred only for the DICOM *identity envelope* (`aux/identity/`). Product content stays plaintext, and the path is reachable only through `tessera ingest dicom --crypto-shred` (Python: #566).
- **ADR-0055** (whole-file, #378) is explicitly *not erasure* (§5: "`share --to` is a *grant*. Crypto-shred (C) is the opposite verb and stays ADR-0040 §2.2 — do not overload `share`").

So a consumer that wants whole-content erasure has to build it out of a primitive that tessera says is not meant for it.

## Ask

Decide (ADR-0055 amendment or a new ADR) whether tessera supports **at-rest whole-file encryption with erasure semantics**, distinct from `share`:

1. **A distinct verb or mode**, e.g. `tessera seal-at-rest <file> --erasure-key <age-pubkey>`, producing a `.tsra.age` whose documented meaning is "readable only with this key; destroying the key erases every copy".
2. **Recipient discipline:** exactly one recipient (the erasure key). Refuse extra recipients, or, if a recovery or escrow recipient is allowed, make the shred obligation explicit: erasure holds only once *every* listed recipient's private key is destroyed. `inspect` on the ciphertext (no decryption) should report the recipient count and whether the file is an erasure-mode file.
3. **Documented shred semantics:** what survives (the outer signature, `ciphertext_hash`, the recipient public key in the header — the L8 graph leak), what does not (sealed content, lineage inside), and what erasure cannot reach (plaintext already `open`ed, per L10).
4. Optionally, a `tessera shred-key`-style helper that destroys an identity file and records a non-identifying tombstone (key id, date), so audits can show the erasure happened.

Until this is decided, anatomist's ADR-0007 treats per-dog encryption as gated on this issue together with #378.

## Linked

#378 (ADR-0055 implementation), #566 (ADR-0047 from Python), #407 (signed `aux/`, lineage outside the ciphertext), ADR-0040 §2.2, ADR-0047, ADR-0055 §5.

