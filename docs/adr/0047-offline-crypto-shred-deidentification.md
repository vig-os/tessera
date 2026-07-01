# ADR-0047 — Offline crypto-shred de-identification (rung 1, DICOM identity envelope)

**Status:** Accepted (2026-07-01, implements #269). The concrete, buildable **rung 1** of the
confidentiality ladder — implements **ADR-0040 §2.2** (crypto-shred erasure) + **§3** (`age`/X25519
envelope encryption of identifying fields, offline, "coded-if-leaked") for the DICOM ingest path.
Deliberately does **not** build **ADR-0041** rung 2 (provider-mediated `unwrap`, KMS/broker, online
revocation/audit) — that is separate infrastructure. Reuses **ADR-0042** (aux members ride *inside*
the one shareable `.tsra`, *outside* the seal) as the carrier, so the whole thing is one self-contained
file with writer-determinism (ADR-0034) intact.

> Mechanism, not legal certification (ADR-0040/0041 caveat, restated). This ADR specifies a
> *technical* control: identifying material is recoverable only by a holder of the recipient private
> key, and destroying every copy of that key renders it permanently unrecoverable. Whether a given
> workflow is "GDPR pseudonymisation / FADP-compliant" is the **DPO's** call, not a tool checkbox.

## Context — "can a key un-SOP itself, and shred by key destruction?"

Two prior DICOM de-identification modes both have a gap:

1. **Destructive `--deidentify`** (PS3.15 tag strip). PHI is *gone at ingest* — irreversible. Good
   for "I never need re-identification," but a legitimate custodian who must re-link a finding to a
   patient later has no path back. It also depends on a **hand-maintained tag blocklist** ([`PHI_TAGS`]);
   a tag not on the list leaks.
2. **Default keep-PHI.** Ingesting without a flag sealed the *full raw DICOM header* (patient name,
   IDs, physicians, institution) into `extra["dicom_header"]` inside a shareable, content-addressed
   product. The reviewer-flagged footgun.

**Crypto-shred (this ADR) is the third mode and the general answer.** De-identify the *shared* view
completely, but carry the original identifying material as an **`age`-encrypted envelope** riding in
`aux/`:

- **A key un-SOPs itself.** A holder of a recipient private key runs `tessera reidentify` → the
  envelope decrypts → the original identity is recovered. Re-identification is a **key operation**,
  not a data-recovery operation.
- **Shred by key destruction.** Destroy every copy of the recipient private key (or run
  `tessera shred` to drop the envelope from the file) → the identifying material is
  cryptographically irrecoverable on **every distributed copy**. This is ADR-0040 §2.2's
  strongest erasure, offline.
- **Completeness by construction.** The envelope carries the **entire raw header**, and the sealed
  product keeps only the explicitly non-identifying curated fields (modality, rescale, kVp, slice
  thickness, pixel spacing, manufacturer, model). De-identification stops being "did we blocklist
  every PHI tag?" and becomes "allowlist the few safe fields; encrypt the rest." Strictly safer than
  a blocklist.

## Decision

### §1 — The identity envelope is one `age` file per product, in `aux/identity/`

On `tessera ingest dicom --crypto-shred --recipient <age-pubkey> [--recipient …]`:

1. Read the DICOM object. Build the **identity document** — a deterministic JSON object:
   ```json
   { "v": 1, "dicom_header": { "0010,0010": {"vr":"PN","value":["…"]}, … },
     "curated_identifying": { "patient_pseudonym": "…", "study_instance_uid": "…", … } }
   ```
   The `dicom_header` is the full pre-scrub header (the same dump that used to leak into `extra`);
   `curated_identifying` echoes the curated fields whose schema `sensitivity` is `identifying`.
2. Serialize it with the canonical JCS rules (ADR-0020) → deterministic plaintext bytes.
3. **Encrypt to the recipient public key(s)** with `age` (X25519 + ChaCha20-Poly1305, the exact
   primitive ADR-0040 §3 names; multi-recipient = the sops model). The `age` armored ciphertext is
   the envelope.
4. **De-identify the sealed product**: strip every [`PHI_TAGS`] element *and* drop every curated
   `identifying`-tier field from the manifest; keep only non-identifying curated fields. Set
   `PatientIdentityRemoved = YES`. The sealed product is byte-for-byte what a `--deidentify` product
   would be (determinism, corpus-neutral) — plus one difference: it carries the envelope in `aux/`.
5. **Attach the envelope as an aux member** `aux/identity/identity.age` via [`add_aux_members`]
   (ADR-0042) — inside the one shareable file, **outside** the seal. `content_hash` / `manifest_hash`
   are unchanged by its presence or absence (the crypto-shred invariant: dropping it is a no-op on the
   seal).

**Why aux, not a block or `extra`.** `age` ciphertext is non-deterministic (fresh ephemeral key +
nonce every encrypt). If it entered `content_hash` / `manifest_hash`, two ingests of the same input
would seal to different bytes — breaking S15 writer-determinism (ADR-0034). Aux members are outside
the seal *by construction* (ADR-0042), so the ciphertext rides for free and the de-identified product
underneath stays deterministic. This is the **same constraint and resolution** ADR-0040 §4 / ADR-0041
§1 identified — here it costs nothing because the aux mechanism already exists (#259/#268).

### §2 — `reidentify` recovers, `shred` destroys

- `tessera reidentify <file> --identity <age-keyfile> [--out <json>]` — read `aux/identity/identity.age`,
  decrypt with the private key, print (or write) the identity document. No key ⇒ the envelope is
  opaque; a wrong key ⇒ `age` decryption fails loudly. Reads are otherwise unchanged (the sealed
  product is a normal de-identified `recon`).
- `tessera shred <file>` — remove `aux/identity/` from the container via
  [`write_aux_members`]`(…, remove_prefixes=["identity/"])`. The seal is untouched (invariant above),
  so the file still verifies; the identifying material is now gone from *this* copy. Combined with
  destroying the private key, it is gone everywhere.

### §3 — What the sealed product commits to

The seal commits to the **de-identified** product only. It does **not** commit to the envelope (§1).
Consequences, all intentional:

- A crypto-shred `.tsra` and a plain `--deidentify` `.tsra` of the same input have the **same data
  identity** — identical `id` (lineage) and `content_hash` (block-merkle over the de-identified
  pixels + metadata). The `to_recon_product` output is *byte-identical*: the recipient list never
  reaches it. When driven through the CLI **spec engine**, the `manifest_hash` differs by exactly one
  field — the `ingested_via_spec` provenance edge, whose spec-hash records the recipient directive.
  That is **correct**: "who can re-identify this" is auditable provenance, not data. Every other
  source edge, all metadata, and all `extra` are identical; `shred`-ding the envelope leaves the
  de-identified data untouched (`content_hash` unchanged).
- `tessera verify` passes with or without the envelope. `inspect` / `tree` list `aux/identity/…`
  (the #268 aux listing) so a reader *sees* that a re-identification envelope is present without
  being able to open it.
- The envelope is **not** signed by the product seal. A tamperer can strip it (that is `shred`,
  a feature) or replace it. Replacement is detectable only to a key holder (decryption fails or the
  recovered `dicom_header` mismatches the retained non-identifying fields); binding the envelope into
  a signature is a future refinement (cross-check ADR-0037 §3 signatures-as-hashed-block).

### §4 — The honest limit

Same analog hole as ADR-0041 §4: once a key holder decrypts, what they do with the plaintext is
outside this control. And offline rung 1 has **no revocation and no audit** — anyone who ever holds
the private key can re-identify any copy forever (that is ADR-0041 rung 2's whole reason to exist).
The correct claim for rung 1 is: **"identifying material is unreadable to anyone without the
recipient private key, and destroying that key erases it from every copy."** Never "revocable" or
"audited" — those are rung 2.

## Non-goals

- **No provider / KMS / broker** (ADR-0041 rung 2). Offline `age` only.
- **No per-field granularity.** One envelope per product carries the whole identifying tier
  (ADR-0041 §1.1's per-tier recommendation, at its simplest).
- **No encryption-at-rest** (ADR-0007 stays a non-goal). This is a share/leak control.
- **Not legal certification** (ADR-0040/0041, restated).

## Consequences

- **New dep:** `age` (MIT/Apache-2.0) on `tessera-ingest` (encrypt) + `tessera-cli` (decrypt). Host-only,
  like `keygen`/`sign` — never in the wasm verify path (verifying a crypto-shred `.tsra`'s seal +
  signature needs no key; decryption is a separate, host-only operation).
- **New CLI verbs:** `reidentify`, `shred`; one new `ingest dicom`/`dicom-series` flag pair
  `--crypto-shred` + `--recipient`.
- **Raw-header leak fixed regardless of crypto-shred:** `extra["dicom_header"]` is now embedded
  **only** on a de-identified path (destructive `--deidentify` or `--crypto-shred`); the keep-PHI
  default no longer seals the full raw header. (User decision, 2026-07-01.)
- **Conformance corpus impact: zero.** The de-identified product underneath is deterministic and
  matches the existing `--deidentify` output; the envelope is aux (seal-neutral). No golden changes.
