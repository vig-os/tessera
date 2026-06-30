# ADR-0037 — Signing trust model

Status: **Proposed** (2026-06-29). Builds on S16 (`signing.rs` envelope), ADR-0033 (WORM roles),
ADR-0036 (versioning / publish-seal). Synthesizes `docs/spikes/signing-trust-model.md` (three
fresh-context lenses: feasibility · user-stories · adversarial). **Two bugs the spike found are fixed
in the same change as this ADR** (see Decision §0).

## Context — one dataset, four hands

The trust model has to serve a *chain of custody*, not a single signer. Follow a high-rate PET/CT
acquisition through four personas:

1. **Instrument scientist** runs the scanner. Each acquisition emits a **listmode** product (a huge
   event *table*, millions of rows/s) and a reconstructed **recon** product (an *image* volume), sealed
   as `.tsra` at the acquisition host. They sign so anyone downstream can prove *"born on scanner
   SN-7421, unaltered since."* The host is often **air-gapped** — no live OIDC, no transparency log at
   sign time. → demands an **offline-signable** scheme (local ed25519), and the key lives on a shared
   appliance (→ ssh-agent/HSM is the eventual want, not a hex file).

2. **Data scientist** pulls those products and works the data in Python (numpy/polars), producing
   **derived** products (ROI tables, parametric maps). Before trusting an input they want
   `verify-sig data.tsra` to answer *"yes, this is the version the instrument lab published"* — which
   only works if they have a **trust store**, not an out-of-band pubkey. They sign their derived
   products with their **own** key, `signer` = their ORCID iD (attribution).

3. **Data custodian** owns the institutional **S3/MinIO** store and is responsible for *both* the raw
   and derived datasets. They **gate ingress** on signatures (verify against an institutional trust
   policy), WORM-lock raw=Compliance / derived=Governance (ADR-0033), and **distribute via OCI**
   (`tessera push`) — which must keep the signature *with* the product. On accession they may
   **counter-sign** (*"ingested into the archive on 2026-04-12, checks passed"*). Their pains are the
   load-bearing ones: trust-store hygiene, **key rotation** when staff leave, and the **auditor's
   10-year-offline** verification.

4. **Researcher** analyzes across the cohort and **publishes** — `tessera publish` (history-free) into
   InvenioRDM/Zenodo with a **DOI**, signed attributable to their ORCID. The trust anchor here is the
   **DOI registry** attesting the depositor's identity; Tessera's job is to *be verifiable* and record
   `signer`. A reviewer/auditor must verify the published `.tsra` **offline**, possibly decades later.

The through-line: **the signature must travel with the product across every hop (host → S3 → OCI →
DOI), verify offline forever, and bind *who* signed — not just *that the bytes are intact*.** ORCID is
**attribution**, never a trust anchor (it has no signing PKI; the data custodian's trust policy /
the DOI registry is the anchor).

## Decision

### §0 — Two bugs fixed now (correctness, not scope)

1. **Sign the envelope, not the seal.** The signature now covers the JCS-canonical
   `{alg, key_id, manifest_hash, signer, signed_at, key_format}` (`alg = ed25519-env-v1`), not the bare
   `manifest_hash`. This closes a **false-attribution** hole: previously a *valid* signature could be
   relabeled with any `signer` (e.g. a famous ORCID) and still verify. Every envelope field — the
   attribution `signer`, the time-of-signing `signed_at`, and the key encoding `key_format` — is now
   bound, so none can be rewritten on an otherwise-valid signature. Regression-tested.
2. **Carry the sidecar through distribution.** `tessera push` now attaches `<file>.tsra.sig.json` as a
   second OCI layer (`application/vnd.tessera.signature.v1+json`) and `tessera pull` recovers it
   (sha256-verified). Signing was previously *operationally unreachable* across the registry hop. The
   `registry-roundtrip` flake check proves the sidecar survives byte-identical.

### §1 — The archival trust principle (the load-bearing rule)

**Verification in year 30 must depend only on bits *in* the product (or its sidecar) plus bits the
verifier *already holds locally*.** Any mechanism that needs a live service at verify time (ORCID
lookup, OCSP, Rekor, a current TUF root) is admissible **only if** its evidence is *snapshotted at
sign-time and embedded*. This is the single criterion that ranks the mechanisms below.

### §2 — The mechanism ladder

- **Alpha (the default — "serviceable for a lab," ship next):**
  - `ed25519-env-v1` (done) + **`tessera keygen`** (ed25519 seed + `.pub`, `0600`; optional
    `--age-recipient` for at-rest encryption) — closes the deferred keygen verb.
  - A **file-based trust store** (`~/.config/tessera/trust/` + per-repo `.tessera/trust/`) with
    `tessera trust add/list/remove`; **`verify-sig` defaults to it** (`--require-signer <orcid>`
    optional), instead of requiring `--pubkey`.
  - `ssh-ed25519` (done) — `tessera sign` auto-detects an OpenSSH `~/.ssh/id_ed25519` and signs with it,
    recording `key_format = "ssh-ed25519"`. It is a host-side key **loader**, *not* a second `alg`: the
    curve, the signed envelope, and verification are identical, so `key_id` (the raw-pubkey hex) and the
    wasm verify path are unchanged. A raw-hex `keygen` seed records `key_format = "raw-hex"`.
  - Two **additive, optional** envelope fields shipped *before any sigs hit production data* (done):
    `signed_at` (RFC-3339, stamped by `tessera sign`) and `key_format` — both **bound** into the
    signature (see §0).
  - `signer` stays **attribution-only**; a `verify-orcid` fetch-and-pin probe makes the binding
    *auditable when online* but never a trust anchor.
- **Archival-grade (feature-gated, post-alpha):**
  - `alg = sigstore-bundle` (`sigstore-rs`) with the cosign bundle (Fulcio cert + Rekor inclusion proof
    + signed timestamp) **embedded** for self-contained offline verify — the publish-time upgrade for
    persona 4. Open problem to resolve before it lands: the **archival TUF-root snapshot** strategy.
    Behind a `sigstore` cargo feature so default + wasm cores stay lean.
  - A **SLSA-provenance side-statement** (`<file>.tsra.intoto.jsonl`, DSSE-signed by the alpha key) —
    turns the provenance edges Tessera already records into a verifiable third-party-readable claim
    (the biggest FAIR win). Hand-roll DSSE (~50 LoC); do **not** depend on alpha `in-toto-rs`.
  - `ssh-agent` / PIV / HSM (`alg = ssh-ed25519` via agent) on clinical demand — keeps the bare seed
    off a shared appliance (persona 1's real need).

### §3 — The one genuine format fork (decide before production sigs)

**Signatures-as-hashed-block vs detached sidecar.** The adversarial lens wants the signature to be a
**hashed block committed by the manifest** (not a strippable sidecar), plus an `attestation.required:
bool` manifest flag so a release channel can *hard-reject* unsigned/stripped products, and so
counter-signatures (persona 3) and PQ re-witnessing are first-class. This touches the seal → it is the
real fork. **Default for the alpha: keep the additive sidecar** (signing never changes identity — a
property worth preserving) and ship `attestation.required` + counter-signatures as a *later, opt-in*
manifest field. Re-open if a regulated adopter blocks on hard-reject.

### §4 — Rejected

ORCID-as-trust-anchor · running our own Fulcio/Rekor · X.509/PKI (CA archival is the wrong shape) ·
PGP/WoT (no upside over local ed25519) · depending on `in-toto-rs` as a primary crate.

## Why

- The personas span **air-gapped acquisition** (no online signer) → **offline auditor** (no live
  service in year 30). Local ed25519 + a trust store is the *only* mechanism that serves both ends
  today and keeps the verify path pure-Rust/wasm. Everything else is an *upgrade for one persona*
  (sigstore for the publisher, HSM for the clinical site), correctly feature-gated.
- Binding `signer` into the signature (§0.1) is non-negotiable the moment attribution is a FAIR claim:
  an unbound `signer` is worse than none (it *invites* the false-attribution attack).
- Carrying the sidecar (§0.2) is what turns "signing exists" into "signing is used" — without it the
  custodian can't gate and the publisher can't ship a verifiable artifact.

## Consequences

- `ed25519-env-v1` replaces the seal-only scheme (pre-1.0, no production sigs affected; the old
  `ed25519` tag is retired, not dual-verified). Verify is still pure-Rust → wasm browser verification
  holds.
- New OCI layer media type `application/vnd.tessera.signature.v1+json`; pull is backward-compatible
  (artifacts without a sig layer pull as before).
- The trust store + keygen + the two envelope fields are the next build increment (the "alpha tier").
- `publish`/`seal` emit *unsigned* products from the repo (the repo stores no signatures); the natural
  flow is publish → sign → push. Storing signatures *in the repo* (so `seal` can carry them) is a
  future enhancement, not in this ADR.
- **Open:** archival TUF-root snapshot for sigstore-bundle; the signatures-as-hashed-block +
  `attestation.required` decision (§3); revocation distribution (a signed revocation list + `signed_at`
  cutoff); counter-signature container shape (`<file>.tsra.sig.d/<key_id>.json`).
