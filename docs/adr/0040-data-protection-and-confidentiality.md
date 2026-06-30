# ADR-0040 — Data protection: schema-driven sensitivity tiers, consent-withdrawal erasure, field-level confidentiality

**Status:** Proposed (decide before any clinical `.tsra` leaves a host)

> Mechanism, not legal certification. This ADR specifies *technical* controls. Whether a given output is
> "anonymized" / "pseudonymized" for **Swiss FADP/nDSG** + **GDPR** (clinical PET/CT = special-category
> personal data) is the **DPO's** determination, not a tool checkbox.

## Context

A `.tsra` is built to be **shared** (FAIR). That is a different threat model than "data at rest": **ADR-0007**
deferred encryption to storage-layer SSE/dm-crypt, which protects the *disk* but does **nothing** for a
product that is exported, pushed to a registry, or leaked. The live runs make the gap concrete — a DICOM
series ingest embeds the **patient name in the `ingested_from` path** inside the sealed manifest.

Three framings shape the design:
1. **DICOM is a special case of the blob/junk tier** — a transitional, opaque vendor container Tessera
   normalises *away* over time. So identity/sensitivity policy must live in **Tessera's own schema**, not be
   re-derived from the vendor format on every ingest.
2. **PS3.15 is the seed, not the source of truth** — DICOM's confidentiality profile authoritatively maps
   *which DICOM attributes are identifying*; we **learn the classification from it** and encode it in Tessera.
3. **Drive the anonymisation tiers from the versioned product schema** — so the policy ("which fields are
   PHI") is itself **content-addressed, versioned, and auditable**: a schema version pins exactly which fields
   are identifying; changing it is a tracked schema bump, not an out-of-band config.

Today the metadata model has coded-vocabulary (`_vocabulary`/`_code`) + `required`/`recommended` severity
(ADR-0040 sibling work) but **no sensitivity classification** — the engine cannot reason about PHI at all.

## Decision

### §1 — Sensitivity tier on `FieldSpec` (schema-driven, versioned)
Add `sensitivity: public | coded | sensitive | identifying` to `FieldSpec`, declared in the product schema
and **composed through `imaging_base`** like every other field attribute. The tiers, with `study`/`modality`
as worked examples:
- **public** — safe in the clear, even in a leaked artifact (`modality`, `study` group label, voxel geometry).
- **coded** — a controlled-vocabulary code, not free text (`_vocabulary`/`_code`); low-risk but vocabulary-bound.
- **sensitive** — clinical but not directly identifying (tracer dose, some dates) — minimise/encrypt by policy.
- **identifying** — direct PHI (`patient_id`, `patient_name`, accession, raw acquisition path) — **never** in
  the clear in a shareable artifact.

The classification is **seeded from DICOM PS3.15**: the DICOM ingest maps PS3.15 "remove/replace" tags →
`identifying`, and the `recon` schema's sensitivity tiers mirror that map — so the policy that PS3.15 expresses
*per ingest* becomes a **durable, versioned property of the Tessera schema**. The engine can then warn on, redact,
or encrypt fields *by tier*, domain-agnostically.

### §2 — Consent withdrawal / right-to-erasure — two mechanisms
1. **Destructive `redact` (data you control).** The identifying raw lives as a `blob`; the de-identified `recon`
   is `derived_from` it. On consent withdrawal: **`forget` the identifying blob lineage + `gc`** its bytes
   (existing verbs, ADR-0036). The de-identified product survives with a **lineage tombstone** — its
   `derived_from` edge points at a `manifest_hash` whose **bytes no longer exist**: provable lineage, erased
   payload. Formalise as `tessera redact <study> --keep-derived`.
2. **Crypto-shred (data you don't control).** If identifying fields are encrypted (§3), **deleting the key**
   renders them permanently unrecoverable — including in copies already shared/leaked. A recognised GDPR
   erasure technique for distributed data; it is the *only* erasure that reaches copies off your hosts.

### §3 — Field-level confidentiality (the self-protecting product)
Fields tiered `sensitive`/`identifying` (§1) are stored as **`age` (X25519) envelope-encrypted** values,
encrypted to a **set of recipient public keys** (sops model — multiple recipients per envelope). `public`/`coded`
fields stay in the clear. Consequences:
- A leaked / publicly-pushed `.tsra` is **"coded/anonymous by default"** — identity is ciphertext only authorised
  recipients can open.
- **Granting access = the recipient supplies their `age` public key**; the custodian re-encrypts the envelopes to
  include it (your "end-user adds his pub key" flow). Revocation = re-encrypt without it + rotate (and §2.2 for
  already-shared copies).
- Revisits **ADR-0007** for the *share/leak* threat model — complementary to, not a replacement for, at-rest SSE.

### §4 — Determinism (the load-bearing constraint)
`age` is nonce'd → ciphertext is **non-deterministic**, so encrypted field *values* **cannot enter the
`content_hash` / `manifest_hash` seal** without breaking writer-determinism (a release gate). Therefore:
- The seal commits to the **plaintext** (or a per-field commitment/hash of it); the **ciphertext is an additive
  sidecar** — the *same architecture as the detached signature envelope* (ADR-0037): outside the seal, beside
  the product, so signing/identity/determinism are untouched.
- Open question: hash a per-field commitment so a recipient can verify the decrypted plaintext matches the seal
  (binding), vs. treat the cleartext-stripped manifest as canonical.

### §5 — Keys
**Signing = ed25519** (existing `keygen` + trust store, ADR-0037). **Confidentiality = age / X25519** — a
**distinct** keypair (don't conflate). Reuse the trust-store pattern for recipient public keys. ADR-0037's alpha
already floated `--age-recipient` for the *private key at rest*; this extends `age` to *field data*.

## Phasing
1. **§1 sensitivity tier** — cheap, unblocks engine reasoning + a PHI warn; PS3.15-seeded. *(spike first.)*
2. **PHI hygiene now** (separate, immediate): `--source-label` (drop the patient-name path from `ingested_from`)
   + wire `dicom-series --deidentify` (PS3.15 per-file; single `dicom` already has it).
3. **§2.1 `redact`** — formalise forget/gc into a consent-withdrawal verb.
4. **§3/§4 field encryption** — the big one: `age` integration + the sidecar/determinism design. §2.2 crypto-shred
   falls out of it.

## Non-goals / notes
- Not legal anonymisation certification (DPO owns that line).
- DICOM/PS3.15 is the **bridge + seed**, not the permanent source of truth — long-term the sensitivity map lives
  in native Tessera schemas as DICOM is normalised away.
- Numbering: ADR-0039 is reserved for the compute-node caching spike (#236).
