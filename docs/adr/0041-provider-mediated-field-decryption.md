# ADR-0041 — Provider-mediated field decryption (revocable, audited, crypto-shred at scale)

**Status:** Proposed (2026-07-01, spike for #241). Builds on **ADR-0040** (sensitivity tiers + §2.2
crypto-shred + §3 field-level confidentiality + §4 determinism), **ADR-0037** (the additive-sidecar
precedent for the detached signature envelope), **ADR-0036** (content-addressed object store and the
audit shape), and **ADR-0007** (encryption-at-rest stays a non-goal — this ADR is share/leak threat
model, not disk).

> Mechanism, not legal certification. This ADR specifies *technical* controls for revocable,
> time-gated, audited access to identifying fields in a shared `.tsra`. Whether a given workflow
> satisfies **Swiss FADP/nDSG** + **GDPR** (clinical PET/CT = special-category personal data) is the
> **DPO's** determination, not a tool checkbox. The honest limit (§4) is the point of this spike.

## Context — rung 2 of the confidentiality ladder

ADR-0040 §3 specifies static `age`/X25519 envelope encryption of `sensitive`/`identifying` fields
to a *set of recipient public keys* (sops model). It is the right **rung 1**: simplest, offline,
"coded/anonymous by default if leaked," and it pairs cleanly with the existing trust-store discipline
(ADR-0037). It also leaves four real gaps an authorized adversary or a withdrawing patient walks right
through:

1. **No revocation.** Once a recipient holds the `age` private key, every past *and future* copy of
   the ciphertext stays decryptable forever. Removing the recipient from re-encryptions only blocks
   *new* envelopes; copies already shared are out of reach.
2. **No audit.** Decryption happens entirely on the recipient's host — there is no record of *who*
   decrypted *what* and *when*. A breach is invisible.
3. **No time-gating.** "Until study close-out" or "business hours only" or "until consent expires
   on 2027-01-01" cannot be expressed.
4. **No reach to leaked copies.** A consent withdrawal (ADR-0040 §2.2 — the strongest erasure) requires
   that the bytes the recipient holds become *undecryptable*. With static `age`, deleting the patient's
   record from your custodial store doesn't touch the copies already on a collaborator's laptop.

**Rung 2 — provider-mediated decryption — closes (1)–(4) by moving the KEK off the recipient's
host into an online key provider** whose policy-gated `unwrap` call is the single chokepoint for
deriving plaintext. The provider can refuse, log, or stop existing forever; the format does not
change, and the same sidecar shape ADR-0040 §4 mandates for `age` carries the wrapped DEK.

It does **not** address what an authorized recipient does with the plaintext after a valid decrypt
(§4). That is the analog hole, and naming it honestly is half the deliverable.

The framing this ADR adopts is the three-rung ladder ADR-0040 §3 implies:

| Rung | Where the decryption key lives | Online at decrypt? | What it buys over the rung below |
|---|---|---|---|
| **1 — Static `age`** (ADR-0040 §3) | On the recipient's host (X25519 private key) | No | Baseline confidentiality; "coded if leaked." |
| **2 — Provider-mediated** (this ADR) | KEK in an external provider (KMS / Vault / broker); only the *wrapped* DEK is shipped | **Yes** (online handshake) | Time-gating, revocation, audit, crypto-shred-at-scale, consent linkage. |
| **3 — TEE / confidential computing** | KEK released only inside an attested enclave | Yes (+ attestation) | Partial in-process copy-defense (memory walls, output policy); still analog-hole bound. |

This ADR specifies rung 2. Rung 3 is out of scope but explicitly named so the ladder is honest about
where copy-defense actually begins (§4).

## Decision

### §1 — Envelope architecture: DEK-per-field, KEK in the provider, sidecar carries the wrap

The cryptographic shape:

1. **Per-field (or per-tier) DEK.** Each `sensitive` / `identifying` field value is symmetrically
   encrypted by a freshly-generated **data-encryption key** (DEK) — concretely a 256-bit key driving an
   AEAD (XChaCha20-Poly1305 or AES-256-GCM). The field's *ciphertext* is what would otherwise have been
   the `age` payload in ADR-0040 §3, but generated under a DEK Tessera controls rather than under a
   per-recipient X25519 envelope. (Per-tier DEK — one DEK for every `identifying` field in a product
   — is a coarser variant; the trade is in §1.1.)
2. **The DEK is wrapped by a KEK held in an external provider.** The DEK never leaves the producer's
   process in the clear before being wrapped by the provider's `wrap(kek_ref, dek)` operation. The
   resulting **wrapped-DEK** (an opaque ciphertext from the provider) plus a provider reference and a
   policy handle land in the sidecar.
3. **The sidecar is the same additive sidecar ADR-0040 §4 establishes for field encryption.** The
   seal continues to commit to the *plaintext* (or its per-field commitment — see §6), and the
   wrapped-DEK envelope rides *outside* the seal. This is the **same architectural precedent as the
   detached signature** of ADR-0037 §0.2 (the `.sig.json` carried as a second OCI layer): an additive,
   determinism-preserving sidecar.

**Why the sidecar placement is forced, not chosen.** Provider `wrap` outputs are *non-deterministic*
ciphertext: every modern KMS embeds a fresh IV/nonce and an authentication tag; calling `wrap` twice
on the same DEK with the same KEK produces two different ciphertexts. AEAD field-ciphertexts are
likewise nonced. If either entered `content_hash` / `manifest_hash`, two byte-identical inputs would
produce two different sealed `.tsra` files — breaking writer-determinism (ADR-0034, the S15 release
gate). The seal therefore *cannot* commit to wrapped-DEK or field-ciphertext bytes; it commits to
plaintext (or a deterministic per-field commitment), and the encryption material rides additively.
This is the same constraint ADR-0040 §4 identified for `age`, and the same resolution.

#### §1.1 — DEK-per-field vs DEK-per-tier (recommendation: per-tier with per-field nonce)

| Option | Pro | Con |
|---|---|---|
| **DEK-per-field** | Maximum granularity; revoke a single field by burning its KEK reference. | N field-decrypts → N round-trips to the provider on every open; sidecar bloat. |
| **DEK-per-tier (one DEK per `sensitivity` tier per product, per-field nonce)** | One `unwrap` per tier per open (typically 1 for `identifying`, 0–1 for `sensitive`); sidecar stays small; audit log entries scale with *intent* not field count. | Granularity is the tier, not the field. Per-field revocation requires a tier-wide rewrap. |

**Recommended default: DEK-per-tier with per-field nonces.** The clinical workload reads tens of
identifying fields together (a recon's PHI block is opened or it is not); per-field is needlessly
chatty against the provider, and per-tier audit entries are *more* informative ("X opened the
identifying tier of recon-2025-04-12") than N near-simultaneous per-field entries. Per-field DEK
remains available as a `policy.granularity = field` opt-in for the rare case where individual fields
have distinct policy windows.

### §2 — Decrypt-as-a-service vs unwrap-DEK (recommendation: unwrap-DEK)

The provider can play one of two roles. They are mutually exclusive *per envelope*; the choice is
not whether to support both but which is the default.

**Option A — Decrypt-as-a-service ("envelope decrypt").** Recipient ships the wrapped-DEK *and* the
field-ciphertext to the provider; provider unwraps the DEK, decrypts the field, and returns the
plaintext. The DEK never leaves the provider.

**Option B — Unwrap-DEK ("envelope unwrap").** Recipient ships only the wrapped-DEK; provider returns
the unwrapped DEK in memory; the recipient decrypts the field locally.

| Axis | A (decrypt-as-a-service) | B (unwrap-DEK) — recommended default |
|---|---|---|
| **Provider sees plaintext?** | Yes — provider is in the PHI path. | No — provider sees only key material. |
| **Blast radius if provider is compromised** | All field plaintexts that pass through. | All DEKs that flow through (still bad, but one indirection removed; plaintext was never centralized at the provider). |
| **Latency / throughput** | Round-trip per field (or per batch endpoint where supported). | One `unwrap` per tier per session; subsequent fields decrypt locally at AEAD speed. |
| **Bandwidth** | Ciphertext crosses the provider boundary both directions. | Only key material (small + fixed size). |
| **Offline-impossible-by-design** | Strict — every decrypt is an HTTPS call. | Strict — but only at "session start," then a tier-DEK can be held in memory for the session. (This is the lever §3's policy/TTL pulls.) |
| **Audit fidelity** | Per-field, per-call. | Per-tier-unwrap; field-level access is *not* logged at the provider (it happens locally). |
| **Compliance posture (FADP/GDPR)** | Provider becomes a sub-processor of PHI (DPA, BAA-equivalent, residency). | Provider is a sub-processor of *key material* (almost universally a lighter contract surface). |
| **Vendor abstraction** | Some KMSes (AWS KMS `Decrypt`, GCP KMS `decrypt`, Vault Transit `decrypt`) implement it directly — *but only on ciphertexts they themselves wrapped*; that locks payload format to the vendor. | The abstract `wrap/unwrap` is the lowest common denominator across all three vendors. |

**Verdict: unwrap-DEK is the default.** The compliance, blast-radius, and abstraction wins are all on
the same side. Decrypt-as-a-service is available as an opt-in (`envelope.mode = "decrypt"`) for
deployments where the regulator specifically requires the provider to be the sole PHI-handler — but
those deployments are already paying the latency and contract-surface tax and know they are. The
*key* leakage concern with B (an unwrapped DEK in process memory) is the same concern as the analog
hole (§4): once a recipient is authorized, in-process exfiltration of *something* is unavoidable.
Routing plaintext through the provider does not change that; it just shifts which secret is in flight.

### §3 — Provider shape: a small abstract interface, not a baked-in vendor

The format must not depend on a specific provider. The `.tsra` sidecar carries an **opaque provider
reference** (a URL or URN) and a **wrapped-DEK blob** the provider knows how to interpret; the
Tessera client speaks a tiny trait. ADR-0039 takes the same discipline with caching ("infrastructure,
not a `tessera` subcommand"); this ADR does the same with key providers.

**The minimal abstract interface** (illustrative Rust, not normative — exact shape pinned at
implementation):

```rust
trait KeyProvider {
    /// Wrap a freshly-generated DEK under the provider's KEK.
    /// Returns the opaque wrapped-DEK ciphertext + provider-side metadata
    /// (kek_version, key_arn, etc.) that the sidecar serialises verbatim.
    fn wrap(&self, kek_ref: &KekRef, dek: &Dek, aad: &[u8]) -> Result<WrappedDek>;

    /// Unwrap the DEK; the policy check happens provider-side using
    /// caller identity + the policy_handle attached to the envelope.
    /// Returns the cleartext DEK in memory (Option B above) — or, for
    /// `mode = "decrypt"` deployments, a separate `decrypt` entry-point
    /// returns plaintext directly without ever surfacing the DEK.
    fn unwrap(&self, kek_ref: &KekRef, wrapped: &WrappedDek, aad: &[u8])
        -> Result<Dek>;

    /// Probe policy *without* unwrapping — drives `tessera verify-access`,
    /// a no-side-effect "would this call succeed right now?" check.
    fn policy_check(&self, kek_ref: &KekRef, policy_handle: &PolicyHandle)
        -> Result<PolicyDecision>;
}
```

`aad` (additional authenticated data) **must** bind the call to the product identity — at minimum
`manifest_hash` and `field_path`. This is what stops a wrapped-DEK from one product being replayed to
unwrap a tier-DEK of another (cross-product replay) and is the smallest piece of cryptographic
discipline that earns provider-mediated decryption its security claim over rung 1.

**Recommended provider shape: a thin Tessera key-broker that fronts a real KMS (AWS KMS / GCP KMS /
HashiCorp Vault Transit).** Decompose:

| Provider | Pro | Con | Verdict |
|---|---|---|---|
| **AWS KMS / GCP KMS direct** | Mature, regulator-recognised, native `Encrypt`/`Decrypt`/`GenerateDataKey`, log to CloudTrail/Cloud Audit out of the box. | Vendor lock; cross-cloud collaboration awkward; AAD support varies (KMS has *encryption context* — usable, but vendor-specific). | **Yes, as a backing KMS** behind the broker — `KeyProvider` impl per vendor. |
| **HashiCorp Vault Transit** | Self-hostable; one API across clouds; rich policy language; audit log built in. | Operational burden (you run Vault); single-org bias (cross-institution sharing needs federation work). | **Yes, as a backing KMS** behind the broker. The default choice for single-institution multi-cloud. |
| **Tessera-native key-broker (no backing KMS)** | Total control; can encode Tessera-specific policy (sensitivity-tier-aware, consent-database-linked); offline-friendlier. | Tessera owns rotation, durability, HA, audit — every problem KMSes have already solved, badly re-solved. **NO**. | **Rejected as a *bare* provider**, but a **thin broker layer in front of a real KMS is the recommended shape** — it concentrates Tessera-specific policy (sensitivity-tier mapping, consent-link, audit-format normalization) without re-implementing key durability. The broker is *application code*, the KMS is *infrastructure*. |

The recommendation is therefore the *composition*: a **Tessera key-broker** (a small service in the
custodian's network, implementing the policy-and-audit surface Tessera wants) **fronted by a real KMS
or Vault Transit** for actual key material and the heavy crypto. The broker is what `KeyProvider`
talks to over a stable API; swapping the backing KMS is a broker-config change, not a `.tsra` change.

**Crucial non-goal: this is *infrastructure*, not baked into the format.** The `.tsra` carries an
opaque `provider` URL/URN + a wrapped-DEK + a `policy_handle`. It does **not** embed a provider
client, vendor SDK bindings, or any KMS-specific protocol. A reader running on a host that cannot
reach the provider sees the envelope, fails to unwrap, and treats the field as undecryptable —
exactly the right behavior (and exactly the same shape as ADR-0039 §0's "the format never depends on
a cache being present"). Provider-aware clients are a separate concern; the format is provider-blind.

### §4 — The honest limit: the analog hole

Provider-mediated decryption gates **key release and key access over time**. It does **not** gate
what an authorized recipient does with the plaintext once the DEK is in their process and the field
is decrypted in their address space. Once the recipient holds plaintext, they can:

- Screenshot it.
- Copy it to a notebook variable, then `pickle` the notebook.
- `print()` it into a log file.
- Export the DataFrame to CSV and email it.
- Photograph the monitor with a phone. (The literal "analog hole.")
- Memorize a 9-character patient ID and type it elsewhere.

The provider sees `unwrap` succeed; it has no visibility into any of the above. Revoking the KEK
*later* renders future copies undecryptable and burns leaked ciphertext copies (the crypto-shred win
of §5), but it **cannot un-leak plaintext that was already extracted during a valid session**.

**This is intentional naming, not a limitation to fix.** The threat model of provider-mediated
decryption is "revocable, time-gated, audited access for authorized recipients," not "copy-proof
distribution." Conflating the two is the failure mode this spike exists to prevent — every prior
generation of DRM has run aground on the same rock.

**Rung 3 — TEE / confidential computing — partially addresses copying** by decrypting only inside an
attested enclave that enforces output-side policy (e.g. "the unsealed plaintext can only flow into a
specific signed analysis binary; raw export is denied"). This is a real escalation against
*programmatic* exfiltration (the `print()` and CSV-export paths above). It does **not** address the
*physical* exfiltration paths (screenshot, photograph, memorization) — those bypass the enclave
boundary by definition. The cost (attested hardware, enclave-aware analysis code, attestation
infrastructure, debugging difficulty) is high; the marginal copy-defense win is real but bounded.
TEE is the **right** rung 3 — it is in scope for a *future* ADR, not this one.

**What this means for the marketing and the docs:** never claim a provider-mediated `.tsra` is
"copy-proof," "leak-proof," or "anonymous after decryption." The correct claim is **"identifying
fields are unreadable to anyone the key provider has not authorized, including holders of leaked
copies and recipients whose authorization has been revoked or has expired."** That is a strong claim
and a true one; it is also the largest true claim available. Hold the line.

### §5 — What rung 2 buys (each tied to the gap it closes)

- **Time-gating** (closes gap 3). The `policy_handle` carries a window (until-date, business-hours,
  study-active). The provider denies `unwrap` outside it without any client-side cooperation.
- **Revocation** (closes gap 1). The custodian deletes (or disables) the KEK or its policy binding;
  every subsequent `unwrap` denies. Already-cached *plaintext* is unaffected (the analog-hole caveat
  of §4); already-distributed *ciphertext* becomes inert.
- **Audit** (closes gap 2). Every `unwrap` call is a provider-side audit entry: caller identity,
  timestamp, `kek_ref`, `manifest_hash` (via AAD binding from §3), policy decision. This is the
  single biggest accountability win — and it costs nothing on the Tessera side because the provider
  already does it.
- **Crypto-shred at scale** (closes gap 4 + ADR-0040 §2.2 — the strongest erasure). Deleting the
  KEK renders *every distributed ciphertext copy* of the field permanently unwrappable. This is the
  consent-withdrawal endgame ADR-0040 §2.2 named: a recognised GDPR erasure technique for distributed
  data, *finally reaching the copies you do not control*. Static `age` (rung 1) cannot do this — the
  X25519 private key is in the recipient's hands.
- **Consent linkage.** The `policy_handle` can reference a consent record in an external
  consent-management system; the provider checks consent status as part of `policy_check` /
  `unwrap`. Consent withdrawal → policy turns red → next `unwrap` denies → effective erasure across
  every host that holds a copy. This is the load-bearing operationalization of "consent is the source
  of truth, not the schema."

### §6 — Plaintext-binding commitment (the per-field hash question)

ADR-0040 §4 left one question open: does the recipient need to verify that the *decrypted* plaintext
matches what the seal committed to? The clean answer for rung 2 is **yes, via a per-field
commitment**:

- For each encrypted field, the seal commits to `H(plaintext)` (a deterministic blake3 of the
  canonical-encoded plaintext value — JCS rules from ADR-0020, so the commitment is binary-stable).
- The sidecar carries the ciphertext + wrapped-DEK as additive material.
- On `unwrap` + decrypt, the recipient recomputes `H(decrypted)` and checks it equals the sealed
  commitment.

This gives **plaintext binding without breaking determinism**: the seal is over deterministic plaintext
hashes (writer-determinism preserved), and a compromised provider that returns garbage cannot pass it
off as the original value. It also preserves the existing seal/`verify`/`signature` path unchanged
(the sidecar is outside the seal; the per-field hashes are inside it as ordinary deterministic bytes).

Open question (deferred to implementation, §8): whether commitments are stored as one
`commitments` block alongside the encrypted field-tier block, or inline in the field-spec as a
declared property. Cross-check against the signatures-as-hashed-block fork question in ADR-0037 §3
— both questions touch "what does the seal commit to that is not user-visible plaintext?" and an
unforced reuse of one solution for the other is worth examining at implementation time.

### §7 — Illustrative sidecar JSON schema sketch (NOT normative)

The sidecar's shape — to make the design concrete; the implementation ADR will pin the exact field
names and AAD construction. Filename convention follows ADR-0037: `<file>.tsra.fcrypt.json` (sibling
of the seal, sibling of `.tsra.sig.json`). For OCI distribution it rides as a third layer alongside
the signature sidecar (media type `application/vnd.tessera.field-encryption.v1+json`).

```json
{
  "alg": "fcrypt-env-v1",
  "manifest_hash": "blake3:7a3f…",
  "envelopes": [
    {
      "scope": { "kind": "tier", "tier": "identifying" },
      "field_dek": {
        "alg": "xchacha20-poly1305",
        "wrapped_dek": "base64url:…",
        "kek_ref": "arn:aws:kms:eu-central-1:…:key/abcd-…",
        "kek_version": 7,
        "provider": {
          "kind": "kms",
          "vendor": "aws-kms",
          "endpoint": "https://kms.eu-central-1.amazonaws.com",
          "broker": "https://broker.example.org/v1"
        },
        "policy_handle": "tessera-policy://broker.example.org/study-2025-04/identifying",
        "aad_binding": ["manifest_hash", "scope.tier"],
        "mode": "unwrap"
      },
      "fields": [
        {
          "path": "patient_id",
          "ciphertext": "base64url:…",
          "nonce": "base64url:…",
          "commitment": "blake3:…"
        },
        {
          "path": "patient_name",
          "ciphertext": "base64url:…",
          "nonce": "base64url:…",
          "commitment": "blake3:…"
        }
      ]
    }
  ],
  "signed_at": "2026-07-01T14:32:11Z"
}
```

Notes — **all illustrative**, all to be pinned in the implementation ADR:

- `aad_binding` enumerates which fields of the envelope are folded into the AAD passed to `wrap` /
  `unwrap` — anchoring the call to product identity (§3, cross-product replay defense).
- `commitment` is the per-field plaintext hash from §6 — *redundant* with what the seal already
  commits to (the seal binds the same plaintext hash), but echoing it in the sidecar means a verifier
  can check the binding without re-opening the seal block. Implementation may dedupe.
- `mode: "unwrap"` is the default (§2); `"decrypt"` is the opt-in for deployments where the
  provider is the sole PHI handler.
- `provider.broker` is optional — when present, clients talk to the broker (which then talks to the
  backing KMS); when absent, clients speak the vendor KMS API directly. Both are valid
  `KeyProvider` impls.

### §8 — Non-goals

- **Not legal certification.** Whether a workflow that uses provider-mediated decryption is "GDPR
  pseudonymisation" / "FADP-compliant" / "research-exempt" is the DPO's call. This ADR specifies
  *technical* controls only; ADR-0040's same caveat applies and is reiterated here.
- **Not copy-prevention.** Explicitly. §4 is the load-bearing honesty.
- **Not a provider implementation.** This ADR specifies the abstract interface and the envelope
  shape. The broker service, the per-vendor `KeyProvider` impls, the audit-log normalization, and the
  consent-system integration are all separate implementation work — almost certainly their own ADRs
  per integration.
- **Not encryption-at-rest.** ADR-0007 stays a non-goal at the *format* layer; rung 2 is a
  *share/leak* threat-model control. The two are complementary (a host that loses a disk still wants
  dm-crypt) and stay decoupled.
- **Not a substitute for rung 1.** Static `age` (ADR-0040 §3) remains the right answer for offline
  / air-gapped sites and for shared `.tsra`s that explicitly want "decryptable by holder, period."
  Rung 2 is the right answer for online clinical/research workflows where revocation and audit are
  the load-bearing requirements. The product schema's `sensitivity` tier (ADR-0040 §1) determines
  *which* fields get encrypted; deployment policy determines *which rung* encrypts them.

### §9 — Threat-model table

For each rung, *defends-against* (D) vs *does NOT defend* (N). "Authorized recipient copying" is the
analog hole and is the only column where rung 3 begins to bite.

| Attack / event | Rung 1: static `age` | Rung 2: provider-mediated | Rung 3: TEE / confidential computing |
|---|---|---|---|
| **Leaked / publicly-pushed `.tsra` (recipient never authorized)** | D — ciphertext is opaque | D — same | D — same |
| **Recipient was authorized once, key now compromised / staff left** | N — recipient's X25519 private key still decrypts forever | **D — KEK disabled; no future `unwrap` succeeds; ciphertext goes dark on every host** | D — enclave attestation refuses to release key to non-attested code |
| **Consent withdrawal (must reach already-distributed copies)** | N — same reason as above | **D — KEK deletion = crypto-shred at scale; every copy becomes inert** | D — same as rung 2 (TEE depends on rung-2 key release semantics) |
| **Time-gated access ("until 2026-12-31 only")** | N — no concept | **D — policy denies `unwrap` outside window** | D — same |
| **Audit of who decrypted what, when** | N — happens client-side, no record | **D — every `unwrap` is a provider audit entry, AAD-bound to `manifest_hash`** | D — same + attestation evidence |
| **Provider compromise (KMS breach)** | N/A (no provider) | N — adversary can unwrap any DEK they get the ciphertext for (unwrap-DEK mode); rotation + monitoring help, do not prevent | **D (partial)** — attestation refuses to release key to non-attested binaries even with provider co-operation |
| **Authorized recipient screenshots / exports plaintext to CSV / pickles notebook** | N — full plaintext in process | **N — full plaintext in process (§4, the analog hole)** | **D (partial)** — enclave can deny `print` / file-write of decrypted data to non-attested sinks; cannot stop a screen photograph |
| **Cross-product wrapped-DEK replay (sidecar swapped between products)** | N/A (no wrap) | **D — AAD binds to `manifest_hash`; replay fails** (§3, this is what AAD earns) | D — same |
| **Provider unavailable / network partition at read time** | D (offline-by-design) | N — `unwrap` fails → field is undecryptable until provider returns | N — same, plus attestation infra must also be reachable |
| **Decades-later offline verify (the ADR-0037 archival principle)** | D — verify-sig only needs trust store + pubkey | N for the *encrypted fields* — the provider must still exist (this is a *known cost* of rung 2; the seal + signature still verify offline forever, only decryption requires the provider) | N — same, plus attestation root must be archived |

Two reads of the table: **rung 2 wins exactly where rung 1 lost (rows 2–5), and loses exactly where
copying happens (row 7).** That asymmetry is the spike's whole point.

## Phasing

This is rung 2 of ADR-0040's ladder; it depends on rung 1 landing first.

1. **ADR-0040 §1 sensitivity tiers** — already in the queue ("spike first" per ADR-0040 phasing).
   Rung 2 needs the tier vocabulary to know which fields to encrypt.
2. **ADR-0040 §3/§4 field encryption (rung 1, static `age`)** — the additive-sidecar discipline
   lands here. Rung 2 reuses the same sidecar shape (and is the second integration test for the
   sidecar contract).
3. **Rung 2: `KeyProvider` trait + a Vault-Transit reference impl + the broker skeleton.** The trait
   is the smallest unit of new code; the Vault impl proves the abstraction; the broker is a separate
   service iterated independently.
4. **AWS KMS / GCP KMS impls.** Mechanical once the trait is settled.
5. **Per-field commitment** (§6) — likely co-lands with rung 1 because the question is identical;
   the answer may be reused verbatim.
6. **Rung 3 (TEE)** — a future ADR, *not* in this one. The honest analog-hole framing in §4 is what
   this ADR commits to *now*; the TEE design lands when a clinical adopter blocks on output-side
   policy enforcement.

## Open questions

- **Provider client lifecycle.** Where does the `KeyProvider` instance live? Per-`Reader` (one
  unwrap per session = matches §1.1 per-tier ergonomics) is the obvious choice; per-process pooled
  is the optimization. The trait is unaffected; the wiring is.
- **Caching unwrapped DEKs in memory.** Holding a tier-DEK for the lifetime of an open `Reader` is
  the natural cost amortization; the question is whether the *policy* (windowed access) ever invalidates
  a DEK mid-session. The conservative default is **re-check at the start of every coarse-grained
  operation** (`open` boundary); the broker can offer a `ttl_ms` hint the client respects.
- **Per-field commitment storage location.** §6 — alongside the encrypted block as a `commitments`
  block, vs. inline in the field-spec, vs. echoed in the sidecar. Three options, each with a clean
  story; pick at implementation time based on how ADR-0037 §3's signatures-as-hashed-block question
  resolves (the two questions rhyme and may share an answer).
- **Audit-log normalization across vendors.** AWS CloudTrail vs GCP Cloud Audit vs Vault audit
  device emit *different shapes* of the same event. The broker should normalize to one Tessera audit
  schema so a custodian doesn't write three queries; the normalization schema is its own design.
- **Consent-system integration shape.** What does a `policy_handle` *look like* against a real
  consent-management system (REDCap consent module, OpenConsent, a homegrown table)? The handle is
  opaque to Tessera; the broker translates. The first such integration will define the convention.
- **Quota / rate-limit failure modes.** A clinical site that does cohort-wide reads can hit KMS rate
  limits. Per-tier DEK (§1.1) makes this much better than per-field, but the workload-shape question
  is real. Not a blocker for the design; a sizing question for the deployment guide.
- **Bootstrap & rotation.** KEK rotation should be a no-op for sealed `.tsra`s (the wrapped-DEK is
  just rewrappable by the provider on the fly); confirm that AWS KMS / GCP KMS / Vault Transit all
  support automatic-rewrap-on-rotate without sidecar regeneration. If not, the sidecar grows a
  `kek_version` field and re-wrap is a tracked operation. (Probably the latter; flagged.)
- **Recovery / break-glass.** A clinical regulator may demand "the provider can never permanently
  lock the data." This collides directly with "crypto-shred is the strongest erasure." The
  tension is real and is *policy*, not protocol: the broker may hold a sealed escrow KEK with a
  multi-party-quorum unwrap. Out of scope here; flagged.
- **Why this isn't rolled into ADR-0040.** Kept separate because the *static* rung is itself a big
  spike with its own deferred sub-questions (§4 commitment, §5 recipient-key UX). Bolting rung 2 onto
  ADR-0040 would have made one ADR carry two threat models at two operational complexities. Each
  rung gets its own ADR; the ladder framing in §0 stitches them.

## Why

- **Rung 1 has four named gaps; rung 2 closes all four** (§9 rows 2–5) with no format change and no
  surrender of writer-determinism — the additive-sidecar precedent ADR-0040 §4 + ADR-0037 §0.2
  already established makes rung 2 a *reuse*, not an invention.
- **The format stays provider-blind.** A `.tsra` carries an opaque URL + a wrapped-DEK; the same
  archive verifies, signs, ships over OCI, and round-trips MinIO unchanged. The Tessera client speaks
  a four-method trait. Providers are infra (ADR-0039's discipline), not format.
- **Unwrap-DEK over decrypt-as-a-service is the smaller compliance surface and the lower latency
  default** (§2). The narrow case where regulators demand provider-as-sole-PHI-handler is preserved
  as an opt-in.
- **Naming the analog hole protects the user.** Every other generation of "encrypted distribution"
  has been sold as copy-proof and has failed in exactly this seam. Tessera ships rung 2 as "revocable,
  audited, crypto-shred-capable" — never as "copy-proof" — and points at rung 3 (TEE) as the *partial*
  next step. The honesty *is* the deliverable.

## Consequences

- **No new format surface.** The container, the manifest, the seal, the signature envelope, and
  ADR-0040's sidecar contract — all unchanged. A rung-2 `.tsra` and a rung-1 `.tsra` are
  byte-indistinguishable except for the sidecar's `alg` field and the presence of `kek_ref` /
  `provider` / `policy_handle`.
- **One new sidecar media type for OCI distribution**:
  `application/vnd.tessera.field-encryption.v1+json` (a third layer alongside the seal and the
  ADR-0037 `.sig.json`). `tessera push` / `tessera pull` carries it for free using the same
  attach-and-fetch machinery (ADR-0037 §0.2).
- **One new abstract interface**: `KeyProvider` (§3). Per-vendor impls land independently and are
  feature-gated (`kms-aws`, `kms-gcp`, `vault-transit`) so the default crate stays lean and the
  wasm-verify path is untouched (verification of the *seal and signature* of a rung-2 `.tsra` does
  *not* need a `KeyProvider` — it only verifies the bits the seal commits to; decryption is the
  separate operation that needs the provider).
- **One new optional service**: the Tessera key-broker. It is *operator-deployed*, not a
  `tessera` subcommand (ADR-0039's discipline); its own repo / image; configured against a backing
  KMS. The custodian runs it; the format does not know about it beyond the URL in the sidecar.
- **Plaintext-binding via per-field commitment** (§6) makes encrypted-field verification possible
  without the verifier ever decrypting — a property a regulator-facing audit script directly relies on.
- **Decryption of rung-2 fields is online-only by design.** This is the trade for revocation. The
  seal and signature still verify offline forever (ADR-0037's archival principle); only the encrypted
  *field values* need the provider. A decades-later auditor verifies *integrity and signature*
  offline, but reading PHI requires a live (or replicated) provider — and that is correct: the whole
  point is that PHI access is controllable, including 30 years from now.
- **Conformance corpus impact: zero.** Encrypted-field fixtures are deferred to rung-2 *implementation*
  and live alongside their own broker fixture; the existing corpus (ADR-0024 etc.) is unencrypted by
  design and stays the writer-determinism baseline.
