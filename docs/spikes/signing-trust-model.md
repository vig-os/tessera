# Spike — signing trust model (feasibility · user stories · adversarial)

Status: **Spike** (2026-06-29). Three fresh-context lenses (mechanisms/feasibility · user-stories/UX-DX
· adversarial red-team) on "how should Tessera establish *trust* in a signature, not just verify the
math." Informs a future ADR. **No code changed by the spike** — but it surfaced two real bugs (below).

## What exists today

A detached ed25519 signature over the product's `manifest_hash` (the seal), in a scheme-agnostic
envelope `Signature { alg, key_id, signer, sig }`, written as a `<file>.tsra.sig.json` **sidecar**
(additive — signing never changes identity). `signer` is an *optional* identity (e.g. an ORCID iD).
CLI: `tessera sign --key <hex-seed> [--signer <orcid>]` / `verify-sig --pubkey <hex>`. There is **no
keygen, no trust store, no timestamp, no revocation** — the verifier obtains the right key out-of-band
(BYO-trust).

## ⚠️ Two real bugs the spike found (independent of which trust model we pick)

1. **The signature covers only `manifest_hash`, not the envelope.** `signer`/`key_id` are *not* in the
   signed bytes (`signing.rs` signs `manifest_hash.as_bytes()` verbatim). So an attacker can take a
   *valid* `(key_id_A, sig_A)` and rewrite `signer: "<famous ORCID>"` — and `verify-sig` still returns
   true. **False attribution on a cryptographically-valid signature.** Fix: sign a JCS-canonical
   `{manifest_hash, key_id, signer}` and bump `alg` → `ed25519-env-v1`. One-line change + a new `alg`.
2. **`tessera push`/`pull` and `publish`/`seal` drop the sidecar.** Signing is *supported* but
   *operationally unreachable* across Tessera's own distribution paths — a registry can't gate on a
   signature it never receives. Fix: carry the `.sig.json` as an additional OCI blob
   (`application/vnd.tessera.signature.v1+json`) and copy it through `publish`/`seal`.

## Convergent verdict (all three lenses agree)

- **ORCID-as-trust is rejected.** ORCID has no signing PKI; nothing maps an ORCID iD → a key. Keep
  `signer` as **FAIR attribution metadata** and stop implying it is a trust statement. The spec must
  say this explicitly (the adversarial lens shows the current UX actively invites false-attribution).
- **The crypto + envelope are sound and forward-compatible.** The work is almost entirely **harness
  around the keys** (lifecycle + trust + distribution), not algorithm changes.
- **No single model works alone** for a 50-year *archival* FAIR context. The discriminator is: *does
  verification in year 30 depend only on bits inside the product + bits the verifier already has
  locally?* Everything that needs a live service (ORCID lookup, OCSP, Rekor, a current TUF root)
  survives archivally **only by snapshotting it at sign-time and embedding it** — i.e. turning the
  live mechanism into a static one anyway.

## Mechanisms (feasibility lens)

| Mechanism | Build now (Rust) | Offline year-30 verify | Revocation | Maturity | Envelope fit |
|---|---|---|---|---|---|
| **Local keypair + trust-store** (`ed25519-dalek`, `ssh-key`) | ~1 wk | **Yes, trivial** | signed revocation list + `signed_at` | production | perfect (already the design) |
| **ORCID-bound** | fetch-and-pin: ~2 d · own Fulcio: months | only via embedded snapshot | author edits ORCID | HTTP+JSON | additive `signer_attestation`; **no crypto binding exists** |
| **sigstore keyless** (`sigstore-rs`) | 2–4 wk | **only with embedded bundle + pinned TUF root** | moot (10-min certs) + Rekor proof | 0.x but real | new `alg=sigstore-bundle`; sig 250 B → 5–20 KB; verify wasm-feasible but heavy |
| **age key-at-rest** (`age`) | ~2 d | N/A (host-side) | N/A | production | doesn't touch envelope |
| **SLSA / in-toto** (DSSE hand-roll) | ~1 wk | inherits signer | inherits signer | crate alpha → hand-roll DSSE | **separate `.intoto.jsonl` sidecar**, signed by the local key |
| **X.509 / PKI** | yes, but CAdES-LT thin | no (heavy LTV) | OCSP/CRL = online | thin archival | rejected |
| **PGP (sequoia)** | yes | yes | revocation cert | production | rejected (ecosystem liability, no upside over #1) |

## User stories (UX/DX lens)

Personas: scanner/DAQ · ingest operator · curator/steward · institutional publisher (DOI) · consumer/
analyst · auditor/regulator · registry. The highest-leverage missing piece is a **trust store +
`verify-sig` defaulting to it** — without it every verify story degrades to "ship the pubkey
out-of-band," which silently becomes "nobody verifies." Friction differs sharply: a **research lab**
needs only trust-store + `--signer` ORCID; a **clinical site** must not keep a bare hex seed on disk
(needs ssh-agent/PIV/HSM — a later `alg`); a **public publisher** anchors trust in the DOI registry,
not in Tessera. The **auditor** (offline, 10-year-old product, rotated key) is the story that *forces*
`signed_at` + an archived historical key bundle.

## Adversarial findings (red-team lens) — top mitigations regardless of model

1. **Sign the envelope, not the seal alone** (closes the false-attribution bug #1 above).
2. **Bind a trusted timestamp + transparency record to every signature** — without it, revocation
   reasoning is impossible (can't tell "signed before compromise" from "after"). `signed_at` now;
   RFC-3161 TSA or Rekor inclusion proof later, *persisted in the artifact*.
3. **Make the signature a hashed block, not a replaceable sidecar**, and add an
   `attestation.required: bool` manifest flag so a release channel can hard-reject unsigned/stripped
   products. Supports countersignatures (a 2026 ed25519 sig re-witnessed by a 2040 PQ sig).
   *(Honourable mention: `verify_chain` skips external leaves — a signed product can still lie about
   which DICOM it was ingested from; the spec must say a signature attests "what the producer claimed,"
   not upstream truth.)*

## Recommendation

**Alpha (ship now, days — "serviceable for a lab"):**
- Fix bug #1 (sign the envelope, `alg=ed25519-env-v1`) and bug #2 (carry the sidecar across
  push/publish) — these are correctness, not scope.
- `tessera keygen` (ed25519 seed + `.pub`, `0600`, optional `--age-recipient` for at-rest encryption);
  `alg=ssh-ed25519` so an existing `~/.ssh/id_ed25519` can sign.
- A file-based **trust store** (`~/.config/tessera/trust/` + per-repo `.tessera/trust/`) and
  `tessera trust add/list/remove`; `verify-sig` defaults to it (`--require-signer <orcid>` optional).
- Two **optional, additive** envelope fields shipped *before any sigs hit production data*:
  `signed_at` (RFC-3339) and `key_format`.
- Keep `signer` as attribution; ship a `verify-orcid` fetch-and-pin probe but **never** treat ORCID as
  a trust anchor.

**Archival-grade (post-alpha, feature-gated):**
- `alg=sigstore-bundle` (`sigstore-rs`) with the cosign bundle **embedded** (Fulcio cert + Rekor
  inclusion proof + signed timestamp) for self-contained offline verify — *open problem: archival
  TUF-root snapshot strategy.* Behind a `cosign`/`sigstore` cargo feature so the default + wasm cores
  stay lean.
- A **SLSA-provenance side-statement** (`.tsra.intoto.jsonl`, DSSE-signed by the alpha key): the
  biggest single FAIR win available — turns the provenance edges Tessera already records into a
  verifiable third-party-readable claim. Hand-roll DSSE (~50 LoC), don't depend on alpha `in-toto-rs`.
- The **signatures-as-hashed-block** + `attestation.required` change is a *format* change (touches the
  seal) → must be an ADR decision, taken before sigs are on production data.

**Reject:** ORCID-as-trust-anchor · running our own Fulcio/Rekor · X.509/PKI · PGP/WoT · depending on
`in-toto-rs` as a primary crate.

## What becomes an ADR (the decisions to ratify)

1. **The archival trust principle** — "verification depends only on bits in the product + bits the
   verifier holds locally"; everything live must be snapshot-embedded.
2. **Envelope-signing + the two optional fields** (`signed_at`, `key_format`) — additive, do it now.
3. **Sidecar vs hashed-block** for the signature (and `attestation.required`) — the one genuine format
   fork; decide before production sigs.
4. **The mechanism ladder**: ed25519+trust-store (alpha) → sigstore-bundle + SLSA (feature-gated) →
   ssh-agent/HSM (on clinical demand). ORCID stays attribution-only.

Source lenses captured verbatim from the spike subagents (feasibility · UX/DX · adversarial); this doc
is the synthesis.
