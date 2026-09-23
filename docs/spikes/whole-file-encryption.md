# Spike — whole-file encryption (`tessera share --to` / `open`)

Design exploration behind **ADR-0055**. Question: can Tessera drive a "send this *entire* sealed
`.tsra` privately to a named collaborator" flow (age/SOPS-like) that fits the existing signing/trust +
per-field-encryption model without weakening either? Method: draft the mechanism, then a fresh-context
**cryptography/secure-systems adversarial pass** tried to break every load-bearing claim.

## The three concepts (the framing that unlocked it)

"Encryption" conflates three orthogonal intents; the design only became clean once they were named:

- **A — whole-file to recipient** (grant, reversible): the *whole* `.tsra` → one opaque `.tsra.age`
  for named recipients. **This spike.**
- **B — per-field sensitivity** (grant, structured): PHI fields age-encrypted, product stays
  structurally valid + range-readable. ADR-0040 §3 / ADR-0041.
- **C — crypto-shred** (revoke, irreversible): destroy a key → bytes unreadable everywhere, incl.
  leaked copies. ADR-0040 §2.2.

A and C are *opposite intents on the same machinery*; A and B are *complementary* (you stack C/B then
A). "Shred-on-share" is a de-id layer (C over B), not recipient-encryption (A).

## Load-bearing decisions

1. **Encrypt outside the seal, decrypt-then-verify.** Identity = plaintext property; encryption is an
   outer wrapper (same lesson as ADR-0037 "sign the envelope"). Cannot move a hash.
2. **Encrypt-then-sign the outer envelope**, binding `{ciphertext_hash, recipients}`. (The weak first
   draft signed only the inner product — see H1–H3.)
3. **One age/X25519 envelope core** shared with per-field (B); age = reviewed, interoperable,
   CLI-decryptable as an archival hedge.
4. **X25519 keys distinct from ed25519** signing keys, but the X25519 enrollment is **authenticated by
   the paired ed25519** (see H4).

## Adversarial pass — findings → verdict

| # | Sev | Attack | Verdict / fix |
| --- | --- | --- | --- |
| H1 | HIGH | Recipient strips inner sig, re-encrypts → downstream sees unsigned "valid" product; also can't prove provenance to DPO without decrypting | **Fix:** outer encrypt-then-sign (`.tsra.age.sig.json`); provenance provable without decrypt |
| H2 | HIGH | age recipient keys are public → anyone can encrypt to Alice; no sender auth | **Fix:** `open` fails closed without a valid outer signature vs trust store |
| H3 | HIGH | Davis / surreptitious forwarding: Bob signs for Alice, Alice re-encrypts to Charlie who assumes Bob addressed him | **Fix:** bind intended `recipients` into the signed outer envelope |
| H4 | HIGH | Trust-store repo write attaches attacker X25519 to Alice's name → future `share --to alice` leaks | **Fix:** enroll age key only with a proof it's signed by Alice's paired ed25519; re-verify at lookup |
| M5 | MED | Whole-file vs field ciphertext are both "an age file" (confusion) | **Fix:** 1-byte plaintext context tag (`0x01` tsra / `0x02` field), reject mismatch |
| M6 | MED | age is randomized → `.tsra.age` not content-addressable, won't dedup in ADR-0036 store | **Doc:** address by outer `ciphertext_hash`; inner plaintext keeps stable CAS after `open` |
| M7 | MED | X25519 identity loss silently turns grant (A) into shred (C) | **Fix:** warn + recommend a lab-wide recovery recipient on `share` |
| L8 | LOW | age header lists recipient keys → collaboration-graph leak | **Doc:** inherent |
| L9 | LOW | No age "add-recipient" → re-share needs plaintext round-trip | **Doc:** open→share again |
| L10 | LOW | No whole-file consent-withdrawal (once opened, plaintext is out) | **Doc:** use C before sharing; revocable = ADR-0041 rung |

**Held cleanly:** DEK/nonce reuse (age fresh key+nonces per encryption); "decrypt-then-verify cannot
move a hash" (AEAD); the A/B/C taxonomy.

**Verdict:** the outer/inner boundary is right; the first draft's "sign inside only" was the weak
choice. Fixing H1 (outer signature binding recipients) collaterally closes H2 and H3; H4 (authenticated
enrollment) is the second must-fix. Both are prerequisites for Proposed → Accepted.
