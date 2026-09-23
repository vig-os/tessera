# ADR-0055 — Whole-file encryption: `tessera share --to` / `tessera open` (age/SOPS-like)

**Status:** Proposed (2026-08-11, spike). Builds on **ADR-0037** (signing/trust store, the "sign the
envelope not the seal" lesson, `keygen`, archival verify-offline-forever principle), **ADR-0040 §3**
(static `age`/X25519 per-*field* encryption to a recipient set — the *same* envelope core reused here),
**ADR-0041** (provider-mediated field decryption — the revocable rung above per-field), **ADR-0036**
(content-addressed store), **ADR-0007** (encryption-at-rest of the *disk* stays a non-goal — this is the
share/leak threat model). Sibling spike doc: `docs/spikes/whole-file-encryption.md`.

> Mechanism, not legal certification. Technical controls only; whether an output is "anonymised" for
> Swiss FADP/nDSG + GDPR is the DPO's determination, not a tool checkbox.

## Context — the missing axis

Tessera already has a confidentiality *ladder* for identifying data *inside* a shared product:

- **Rung 1 — per-field static age** (ADR-0040 §3): `sensitive`/`identifying` fields are `age`/X25519
  envelope-encrypted to a recipient set; `public`/`coded` fields stay in the clear. The product stays a
  *structurally valid, seal-verifiable* `.tsra` — cleartext blocks remain range-readable, so cloud
  prune-before-fetch (ADR-0002 §4) still works. "Anonymous-by-default if leaked."
- **Rung 2 — provider-mediated field decryption** (ADR-0041): adds revocation, audit, time-gating.

Both answer *"share this product broadly, but keep the identifying columns confidential."* Neither
answers a different, common need:

> **"Send this *entire* dataset privately to one named collaborator — nobody else should read *any* of
> it — the way I'd `age`-encrypt or `sops` a file before dropping it in shared storage."**

That is **whole-file** encryption: the sealed `.tsra` (identity + provenance + every block, PHI or not)
becomes one opaque ciphertext blob addressed to a recipient. It is *not* a de-identification tool and
*not* a substitute for per-field — it is the transport-privacy axis. The live gap: today the only way to
send a whole product privately is out-of-band `age`/`gpg`, which Tessera neither drives nor records.

## Three concepts, deliberately untangled

The word "encryption" hides three orthogonal intents. This ADR is **A**; it must compose with, not
duplicate, B and C.

| | Intent | Granularity | Reversible? | Who can read | Owner |
| --- | --- | --- | --- | --- | --- |
| **A. Whole-file to recipient** | **grant** (share privately) | the entire `.tsra` | yes (recipient decrypts) | named recipient(s) only | **this ADR** |
| **B. Per-field sensitivity** | grant (structured) | `sensitive`/`identifying` fields | yes | recipients of those fields | ADR-0040/0041 |
| **C. Crypto-shred** | **revoke** (erasure) | a key's scope | **no** (key destroyed) | *nobody* after shred | ADR-0040 §2.2 |

Load-bearing distinctions:

- **A and C are opposite intents** on the same machinery. A grants a recipient the ability to read; C
  destroys the ability to read (including in leaked copies). "Shred-on-share" is a *de-identification*
  layer (C over B), **not** recipient-encryption (A). You can *stack* them: shred/redact PHI first
  (C/B), then age-wrap the residual for a recipient (A). They are layers, not alternatives.
- **A is coarser than B by design.** Per-field keeps a structurally-valid product (cloud
  prune-before-fetch preserved); whole-file yields an opaque blob (prune-before-fetch **lost** — you
  fetch and decrypt the whole thing to read any of it). Choose A when the recipient set is the whole
  product's audience; choose B when you share broadly but gate some columns.

## Decision

### §1 — Encryption is strictly OUTSIDE the seal (decrypt-then-verify)

The sealed `.tsra` is the plaintext identity (`id` / `content_hash` / `manifest_hash`). Whole-file
encryption is an **outer transport wrapper**: `seal → age-encrypt → .tsra.age`. On receipt:
`age-decrypt → the original byte-identical .tsra → verify`. The seal is computed over plaintext and
never changes; encryption cannot move any hash. age's payload AEAD (ChaCha20-Poly1305) means a tampered
ciphertext fails to decrypt — it can never yield a "valid-looking" plaintext.

This is the **same lesson as ADR-0037's "sign the envelope, not the seal"**: identity is a property of
the plaintext product; signing and encryption are outer envelopes that travel *with* it. Verification
always reduces to the existing offline `manifest.verify()` on the decrypted plaintext.

### §1a — Encrypt-**then**-sign: signature binds the outer ciphertext AND the recipients

The AEAD protects against outsider tampering but **not** against a *legitimate recipient* re-encrypting
the plaintext to a third party, nor against sender-spoofing (age recipient keys are public — *anyone*
can encrypt any product to Alice). A three-lens adversarial pass (`docs/spikes/whole-file-encryption.md`)
surfaced three attacks that "sign the inner `.tsra` only" leaves open:

- **Signature demotion (H1):** a recipient strips the inner sidecar and re-encrypts the plaintext to a
  fourth party, who sees an unsigned "valid" product with no evidence it was ever signed.
- **No sender authentication (H2):** the transport layer proves nothing about *who* sent it.
- **Surreptitious forwarding / Davis attack (H3):** Bob signs a product *for Alice*; Alice re-encrypts
  to Charlie, who wrongly assumes Bob addressed *him*.

**Decision:** `tessera share` signs the outer envelope (**encrypt-then-sign**) in addition to whatever
inner signature the product already carries. The `.tsra.age` gets its own detached
`.tsra.age.sig.json`, an ADR-0037 envelope whose bound payload is extended with the **encryption
context**: `{alg, key_id, ciphertext_hash, recipients: [age-pubkey…], signer, signed_at}`. This:

- lets a recipient prove provenance to a DPO/regulator **without decrypting** (verify the outer sig +
  `ciphertext_hash` over the `.tsra.age` bytes) — closes H1;
- authenticates the sender at the transport layer — `open` fails closed if the outer sig doesn't verify
  against the recipient's trust store — closes H2;
- binds the **intended recipient set** into the signed statement, so a forwarded copy is detectably
  *not* addressed to the new reader — closes H3.

Inner signing (the product's own `.sig.json`, made at seal time) still rides inside for
decrypt-then-verify attribution of the *content*; the outer sig authenticates the *transfer*.

### §2 — One envelope core, reused from ADR-0040 §3

Whole-file and per-field MUST share a single implementation of the age/X25519 primitive: a random
**data-encryption key (DEK)** encrypts the payload (ChaCha20-Poly1305 via the `age` format), and the
DEK is **wrapped per recipient X25519 public key** (age stanza per recipient — the sops multi-recipient
model). Per-field (ADR-0040 §3) wraps small field values; whole-file wraps the whole `.tsra` byte
stream. Same crate, same key format, same trust-store lookup — no second crypto path to audit. Use the
**`age` crate / format** (not raw primitives) so the ciphertext is inspectable by the standard `age`
CLI too (interop, and a hedge if Tessera is ever unavailable — archival principle). age generates a
fresh file key + per-chunk nonces per encryption, so there is no DEK/nonce-reuse hazard across the two
call sites (confirmed in the adversarial pass).

**Domain separation (M5):** a whole-file ciphertext and a per-field ciphertext are otherwise both "an
age file." Prepend a 1-byte context tag to each *plaintext* before wrapping (`0x01` = whole `.tsra`,
`0x02` = field value) and reject a mismatch on decrypt, so one can never be confused for the other even
though they share the primitive.

### §3 — X25519 recipient keys live in the trust store, cryptographically bound to the signing key

- **Do not reuse the ed25519 signing key for encryption.** Signing (ed25519, ADR-0037) proves *who
  made this*; encryption (X25519) controls *who can read this*. Different keys, different lifetimes and
  rotation policies. (`age` uses X25519; ed25519→X25519 conversion exists but conflating the two couples
  unrelated rotations — keep them separate.)
- Extend the trust store (ADR-0037: `.tessera/trust/` repo + `~/.config/tessera/trust` user) so an
  entry can carry an **`age` recipient public key** alongside the `signer`/`.pub`. `tessera keygen`
  already accepts `--age-recipient`; add `--age` to emit an X25519 keypair, and
  `tessera trust add <name> --age-recipient ageXXXX…` to register a collaborator's encryption key by
  **name** — so `share --to alice` resolves a real key, never a pasted recipient string.
- **Authenticated enrollment (H4).** A name→X25519 binding by DPO fiat is unauthenticated glue: an
  attacker who can write the trust-store repo can attach *their* X25519 to Alice's name, silently
  redirecting future `share --to alice`. So enrollment **requires a proof**: the age recipient key must
  be signed by Alice's paired **ed25519** key (the identity already anchored in the trust store), and
  `trust add` stores that proof. `share --to` **re-verifies** the proof at lookup and refuses a recipient
  whose binding doesn't check out. This reuses the ed25519 trust anchor to authenticate the X25519 key,
  without merging the two keys.

### §4 — CLI surface

- **`tessera share <file.tsra> --to <name>[,<name>…] [--out <file.tsra.age>]`** — resolve each name to
  its (authenticated, §3) X25519 recipient key, age-encrypt the sealed `.tsra` to that recipient set,
  write `.tsra.age`, and **sign the outer envelope** (§1a) with the sharer's ed25519 key →
  `.tsra.age.sig.json` binding `{ciphertext_hash, recipients, signer, signed_at}`. `--recipient ageXXXX`
  allows an unregistered one-off. The product's own inner sidecar (if any) is encrypted *inside* and
  travels with the plaintext, so decrypt-then-verify still recovers content attribution.
  **Recovery recipient (M7):** `share` warns if no lab-wide **recovery** X25519 recipient is included —
  losing the sole recipient identity silently turns a *grant* into an irreversible *shred* (the opposite
  intent). A configurable default recovery recipient is recommended for any custodial workflow.
- **`tessera open <file.tsra.age> [--out <file.tsra>] [--identity <key>]`** — **first verify the outer
  signature** against the recipient's trust store and **fail closed** if it is absent or doesn't check
  out (this authenticates the *sender* — age recipient keys are public, so an unsigned `.tsra.age` proves
  nothing about who sent it, H2); then age-decrypt with the user's X25519 identity (trust-store default,
  `--identity` override), recover the byte-identical `.tsra`, and run the existing `verify` (seal + inner
  signature). A corrupted/forged ciphertext never yields a "valid-looking" product.
- Inspection: `tessera inspect <file.tsra.age>` reports the recipient count + outer-signature status
  from the envelope + `.sig.json` **without** decrypting — a recipient can prove provenance to a
  DPO/regulator without revealing the plaintext (H1).

### §5 — What whole-file encryption costs (state it honestly)

- **Loses cloud prune-before-fetch.** A `.tsra.age` is opaque: you cannot range-read block statistics
  to skip non-matching products (ADR-0002 §4 / #225). Reading anything means fetching + decrypting the
  whole blob. This is the price of the coarse grain — call it out at `share` time and in docs. For
  cohort-scale selective access, **per-field (B) is the right tool, not A.**
- **No revocation / audit / time-gating** — same limits as static age (the ADR-0041 gaps). Whole-file
  is rung-1-grade for those axes; if you need revocable/audited access, that is ADR-0041 territory (and
  a future "provider-mediated whole-file" is the analogous rung 2, deferred).
- **Not erasure.** `share --to` is a *grant*. Crypto-shred (C) is the opposite verb and stays ADR-0040
  §2.2 — do not overload `share` with revocation semantics.
- **Not content-addressable (M6).** age is randomized (fresh file key + nonces), so two `share` runs of
  the same product to the same recipients yield *different* bytes. A `.tsra.age` therefore does **not**
  dedup in the ADR-0036 `objects/` store and is addressed by its own outer hash (`ciphertext_hash`), not
  the product's `content_hash`. The inner plaintext keeps its stable content-addressing after `open`.
- **Recipient-graph leakage (L8).** age lists all recipient X25519 keys in the header in the clear, so a
  `.tsra.age` reveals the collaboration graph. Inherent to the format; documented, not fixed.
- **Re-share needs a plaintext round-trip (L9).** age has no "add-recipient" primitive; granting a new
  reader means `open` then `share` again (and re-signing the new outer envelope) — no in-place mutation.
- **Consent withdrawal has no whole-file technical answer (L10).** Once `open`ed, the plaintext is out;
  whole-file grant cannot reach it. Revocable access is ADR-0041 territory (a future provider-mediated
  whole-file rung); for erasure use crypto-shred/redact (C) *before* sharing.

## Alternatives considered

- **Reuse the ed25519 key for encryption (ed25519→X25519).** Rejected §3: couples signing and
  encryption rotation; a compromised/rotated read key shouldn't force re-signing and vice-versa.
- **Encrypt inside the seal (a "ciphertext block" tier).** Rejected §1: would make identity depend on
  the encryption key, breaking reproducible content-addressing and offline verify; contradicts the
  ADR-0037 envelope lesson. Encryption stays an outer wrapper.
- **Roll our own DEK+wrap instead of `age`.** Rejected §2: `age` is a reviewed, interoperable format;
  the standard `age` CLI can decrypt as an archival hedge. No bespoke crypto.
- **Whole-file only, drop per-field.** Rejected: loses cloud prune-before-fetch for the common
  "share-broadly-gate-columns" case. A and B are complementary, not competing.
- **Sign the inner `.tsra` only (no outer signature).** Rejected §1a: leaves signature demotion (H1),
  no sender authentication (H2), and surreptitious forwarding (H3) open. Encrypt-then-sign the outer
  envelope, binding the recipient set, closes all three.

## Consequences

- A named, trust-store-driven private-transfer path (`share --to`/`open`) with no out-of-band `age`.
- One audited crypto path (DEK + X25519 recipient-wrap) serves both per-field and whole-file.
- Identity/verification semantics are unchanged: decrypt → the exact sealed bytes → existing verify.
- Opaqueness (lost prune-before-fetch) is the explicit trade for coarse-grain privacy; per-field remains
  the structure-preserving option.

## Scope for the follow-up issue (implementation, when Accepted)

1. Envelope core: factor the age/X25519 DEK+recipient-wrap primitive so ADR-0040 §3 and this ADR call
   one implementation (`age` crate), with the §2 domain-separation context tag.
2. Trust store: X25519 recipient keys by name; `keygen --age`; `trust add --age-recipient` **with the
   §3 authenticated-enrollment proof** (age key signed by the paired ed25519), re-verified on lookup.
3. `tessera share --to` (outer encrypt-then-sign + recovery-recipient warning) / `tessera open`
   (verify-outer-sig-fail-closed → decrypt → verify) / `inspect` on `.tsra.age` (recipients +
   outer-sig status without decrypt).
4. Signature interplay: outer `.tsra.age.sig.json` binds `{ciphertext_hash, recipients, signer}`; the
   product's inner sidecar rides inside for content attribution (decrypt-then-verify).
5. Tests (each maps to a finding): round-trip byte-identity (seal→age→open→verify); wrong-identity
   refusal; multi-recipient; tamper-the-ciphertext-fails; **outer-sig-missing → `open` fails closed
   (H2); forwarded-to-unlisted-recipient detected (H3); trust-store age-key swap rejected (H4);
   field↔product context-tag mismatch rejected (M5)**; and a `.tsra.age` decryptable by the stock
   `age` CLI (interop).
6. Docs: the A/B/C table; "loses prune-before-fetch — use per-field for cohorts"; the key-loss=shred
   warning + recovery recipient (M7); recipient-graph leakage (L8).

## Threat model — adversarial pass summary

A cryptography-lens review (`docs/spikes/whole-file-encryption.md`) attacked the design. The
outer/inner boundary, the A/B/C taxonomy, and "decrypt-then-verify cannot move a hash" held; age's
fresh-key-per-encryption cleared the DEK/nonce-reuse concern. Four HIGH findings drove the §1a
outer-signature and §3 authenticated-enrollment decisions (H1 demotion, H2 sender-spoof, H3 Davis
forwarding, H4 trust-store key swap); MEDIUM/LOW findings (M5 domain separation, M6 non-CAS, M7
key-loss=shred, L8 graph leak, L9 re-share, L10 no whole-file revocation) are addressed or explicitly
documented above. **Both HIGH must-fixes (outer signature, authenticated enrollment) are prerequisites
for moving this ADR from Proposed to Accepted.**
