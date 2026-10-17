---
type: issue
state: open
created: 2026-08-11T08:06:28Z
updated: 2026-10-16T21:39:18Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/378
comments: 2
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:51.599Z
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
---

# [Comment #1]() by [gerchowl]()

_Posted on October 16, 2026 at 05:15 PM_

Dogfooding anatomist spike (MorePET/anatomist#91), tessera `dev` `fa581f5`, synthetic data only ("fictus" pseudonyms, no real patients).

**Anatomist's shape of ADR-0055:** per-dog data key — a dog's ingest product (CT) and every derived product (trees, frames, reports) are encrypted to **one** X25519 recipient that stands for that dog; destroying that recipient private key crypto-shreds the whole lineage for that dog, on every copy. Not the "share with a named human collaborator" picture the ADR foregrounds, but the same primitives.

**Emulation (ADR-0055 isn't shipped yet):** `age` CLI + an ed25519 outer-sig envelope (JSON `{alg, hash_alg, ciphertext_hash, ciphertext_bytes, recipients, signer, key_id, signed_at, context_tag:0x01}`, signed with the key from `tessera keygen`) end-to-end, two dogs, ingest+derived each.

What held up end-to-end:
- `tessera sign` + `verify-sig` on the plaintext product (inner attribution) → OK.
- age-encrypt the sealed `.tsra` → opaque blob, `tessera verify` on it fails with "Could not find EOCD" (expected).
- Outer sig over `ciphertext_hash` verifies **without the age key**.
- age-decrypt → **byte-identical** `.tsra` (ADR-0055 §1 as stated).
- Inner `verify` / `verify-sig` / `provenance` all pass on the decrypted product.
- Shredding the per-dog age private key: `age -d` fails for every ciphertext of that dog; the outer sig still verifies, so "nothing was tampered since signing" is still provable.

**Gaps that forced emulation (asks for the implementation):**

1. **`tessera trust add --age-recipient`** doesn't exist yet (`TrustAction` only takes ed25519 pubkeys). ADR-0055 §3 asks for this with the authenticated-enrollment proof (age key signed by the paired ed25519) — critical, because the age emulation here has no authenticated name→recipient binding.
2. **`tessera share --to` / `tessera open`** aren't there. Standalone `age` works for the data path but the outer `.sig.json` has to be hand-rolled; there's no `tessera inspect <file.tsra.age>` that reads recipients + outer-sig status without decrypting (§4), which is the auditor's "prove provenance without seeing content" surface.
3. **Writer-side aux API from Python.** `tessera ingest dicom --crypto-shred` is the only path today that attaches an `aux/identity/*.age` envelope. The `Builder` pyo3 surface is `add_array/add_table/add_source/set_field/pack` — a Python producer has no `add_aux_member`, so the ADR-0047 **crypto-shred pattern can't ride a Python-written product at all**. This matters because anatomist's steps write from Python; the DICOM ingest path is upstream of them. Filing this separately (see follow-up).
4. **Chain-verify without the key.** `derived_from` and parent `manifest_hash` live **inside** the ciphertext, so once you age-encrypt the whole file an auditor can confirm "this ciphertext hasn't been tampered since signing" via the outer sig, but can't confirm "this derived product descends from <parent id>" without decrypting. Cross-linked with #407 — anatomist needs the signed-aux appendix (or a projection of the lineage summary into the outer envelope) for the "no-decrypt lineage audit" use case.

**Key-management questions for anatomist's ADR (not asks on tessera, logging for cross-link):**
- Where keys live on the ksb host (per-dog X25519 identity file mounted into the pod from a host-only secret store; signing key stays on the operator host, not the pod).
- Offline ksb job (`--no-egress`): the per-dog key is a pre-mounted file; online KMS/broker (#241) is the follow-on, not M2.
- Per-dog vs per-study: per-dog is the current pick (one recipient covers every product of that dog's lifetime).
- Derived inheriting the parent's key: trivially, the step encrypts to the same per-dog recipient — but age has no "add-recipient" primitive (§5 L9), so a rotation needs a plaintext round-trip of every product of that dog.
- Who signs with which key: ed25519 operator key; the ksb pod does not sign (no secrets in the pod).
- Shredding and `supersedes`: destroying the per-dog key breaks every reference to that dog's products; audit trails that only recorded manifest_hashes stay valid, but any audit trail that quoted plaintext content is now orphaned.

Repro inputs available on request; synthetic throughout, no patient data.


---

# [Comment #2]() by [gerchowl]()

_Posted on October 16, 2026 at 09:39 PM_

From MorePET/anatomist#86 (ADR-0007): ADR-0055 §5 says `share --to` is a grant, not erasure. anatomist's per-dog crypto-shredding therefore can't rest on #378 alone. The erasure semantics are requested separately in #570 (at-rest mode, one recipient or an explicit obligation to destroy every recipient's key, inspect showing the recipient count). anatomist's per-dog encryption is now gated on #378 plus #570.

