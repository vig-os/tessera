---
type: issue
state: open
created: 2026-11-24T18:53:38Z
updated: 2026-11-24T18:54:36Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/968
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:49.681Z
---

# [Issue 968]: [Cryptography for 1.0: choose post-quantum-ready algorithms now (pre-1.0 format break accepted), with algorithm IDs and multi-recipient key slots](https://github.com/vig-os/tessera/issues/968)

## Motivation
tessera's encryption and signing came from first choices: `age` with X25519 + ChaCha20-Poly1305 for the identity envelope (ADR-0047), per-field encryption (ADR-0040/0041) and whole-file encryption (ADR-0055); ed25519 signatures (ADR-0037); blake3 identity. **tessera is alpha, pre-1.0, and format breaks are accepted**, so the right algorithms can be chosen before 1.0 instead of migrating later.

The driver is **harvest-now-decrypt-later**. Products hold identity data and clinical records for 10–20 years, so ciphertext captured today must stay confidential after a cryptographically relevant quantum computer exists. The same applies to **crypto-shredding**: destroying the key only erases data if nobody can break the key wrap from a stored copy of the ciphertext.

## Scope: decide per role, from requirements, not from what is in the code
| Role | Requirement | Candidates to evaluate (not decided) |
|---|---|---|
| Key encapsulation (wrapping data keys to recipients) | post-quantum confidentiality for 20+ years; hybrid during the transition, as BSI/ANSSI recommend (to verify) | ML-KEM-768/1024 (FIPS 203), hybrid ML-KEM + X25519 (e.g. X-Wing), inside HPKE (RFC 9180) or another envelope; `age` only if its post-quantum recipients meet the bar |
| Payload encryption (AEAD) | 256-bit keys, nonce-misuse resistance for large and parallel block writes | AES-256-GCM-SIV, XChaCha20-Poly1305, a chunked STREAM construction for large blocks |
| Signatures | verifiable for decades; multi-signature per product | ML-DSA (FIPS 204), SLH-DSA (FIPS 205) for long-lived roots, hybrid with a classical signature where verifiers need it (qualified seals #963) |
| Hashes / identity | collision resistance with a quantum margin; parallel tree hashing | blake3-256, SHA-256/SHA-3 fixity (#526); decide together with the seal-encoding spike #897 |

## Format requirements (independent of the choice)
- **Algorithm identifiers on every envelope, signature and hash**, so the next change is an explicit, versioned event (crypto agility).
- **Multi-recipient key slots**: a data key wrapped to N recipients, each slot naming its KEM; threshold (k-of-n) slots as a slot type (see the kit key-lifecycle spike).
- **Re-wrap without re-encrypting the payload**: add, remove or rotate a recipient by rewriting key slots only.
- **Crypto-shred proof**: the shred guarantee is stated per KEM, and test vectors show that a product stays unreadable without the key.

## Acceptance
- [ ] ADR choosing each role's algorithm with the threat model and sources (NIST FIPS 203/204/205; national agency guidance), checked at the date of writing
- [ ] envelope, slot and signature format with algorithm IDs; test vectors
- [ ] migration of existing alpha products (or an explicit statement that alpha products are re-ingested)
- [ ] performance on large products (multi-GB, parallel blocks)

Refs ADR-0037, ADR-0040, ADR-0047, ADR-0055, #526, #897, #963, #964, #378. Consumer: nerdmachines/vetgraph.
---

# [Comment #1]() by [gerchowl]()

_Posted on November 24, 2026 at 06:54 PM_

Related: kit post-quantum plan nerdmachines/kit#352; key lifecycle spike nerdmachines/kit#353; leak detection #969.

