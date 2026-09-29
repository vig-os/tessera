---
type: issue
state: open
created: 2026-08-11T08:06:28Z
updated: 2026-09-28T17:59:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/378
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:43.666Z
---

# [Issue 378]: [Implement ADR-0055 — whole-file encryption (tessera share --to / open)](https://github.com/vig-os/tessera/issues/378)

Implementation follow-up for **ADR-0055** (Proposed). Blocked on moving the ADR from Proposed →
Accepted; the two HIGH must-fixes from the adversarial spike (outer encrypt-then-sign + authenticated
X25519 enrollment) are prerequisites.

## Scope (from ADR-0055 §"Scope for the follow-up issue")

1. **Envelope core** — factor the `age`/X25519 DEK + recipient-wrap primitive so ADR-0040 §3 (per-field)
   and this ADR call one implementation (`age` crate), with the §2 domain-separation context tag
   (`0x01`=tsra / `0x02`=field).
2. **Trust store** — X25519 recipient keys by name; `keygen --age`; `trust add --age-recipient` with the
   §3 **authenticated-enrollment proof** (age key signed by the paired ed25519), re-verified on lookup.
3. **CLI** — `tessera share --to <name>` (age-encrypt → `.tsra.age` + **outer encrypt-then-sign**
   `.tsra.age.sig.json` binding `{ciphertext_hash, recipients}` + recovery-recipient warning);
   `tessera open` (verify-outer-sig-**fail-closed** → decrypt → existing verify); `inspect` on `.tsra.age`
   (recipients + outer-sig status without decrypt).
4. **Tests** (each maps to a spike finding) — round-trip byte-identity; wrong-identity refusal;
   multi-recipient; tamper-ciphertext-fails; outer-sig-missing → open fails closed (H2);
   forwarded-to-unlisted-recipient detected (H3); trust-store age-key swap rejected (H4);
   field↔product context-tag mismatch rejected (M5); `.tsra.age` decryptable by the stock `age` CLI (interop).
5. **Docs** — the A/B/C table; "loses prune-before-fetch → use per-field for cohorts"; key-loss=shred
   warning + recovery recipient (M7); recipient-graph leakage (L8).

## Deferred (noted in ADR)

Provider-mediated **revocable** whole-file (the ADR-0041 analog rung 2); manylinux-independent.

See `docs/adr/0055-whole-file-encryption.md` + `docs/spikes/whole-file-encryption.md`. Sibling of
ADR-0040/0041 (the per-field ladder).
