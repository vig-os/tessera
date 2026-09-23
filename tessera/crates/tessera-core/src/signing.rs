//! Detached **signatures** over a product's `manifest_hash` (S16 / #209 — integrity + tamper-evidence,
//! source-rooted attestation).
//!
//! The seal (`manifest_hash`) transitively commits to `id_inputs`, every block digest, and all metadata
//! (see [`crate::manifest`]), so binding it in the signature attests the **whole** product with one
//! short signature. The signature is over the **envelope** — the JCS-canonical `{alg, key_id,
//! manifest_hash, signer, signed_at, key_format}` — *not* the bare seal, so the attribution identity
//! (`signer`), the key (`key_id`), the time-of-signing (`signed_at`) and the key encoding (`key_format`)
//! are all cryptographically bound and cannot be rewritten on an otherwise-valid signature. A signature
//! is **additive** — it lives beside the manifest, never inside it, so signing (or re-signing) never
//! changes the product's identity.
//!
//! ## Scheme-agnostic envelope
//! [`Signature`] is `(alg, key_id, signer, signed_at, key_format, sig)` — a closed-over-`alg` envelope so
//! new schemes are additive: **ed25519** here (pure-Rust → the sign/verify path compiles to wasm for
//! browser verification), with `ssh-ed25519` (reuse a researcher's `~/.ssh` key — a host-side key
//! *loader*, same curve + same envelope) and keyless/sigstore as later backends. `signer` carries the **identity** (e.g. an ORCID iD) independently of the key bytes —
//! attribution is FAIR metadata, not the crypto. Key material may be held at rest under age/sops; that
//! is a host-side loader concern, not part of this verify-able core.

use ed25519_dalek::{Signature as Ed25519Sig, Signer, Verifier};
use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;

/// Re-exported Ed25519 key types so downstream crates sign/verify without a direct `ed25519-dalek`
/// dependency (the crypto boundary lives here, in the core).
pub use ed25519_dalek::{SigningKey, VerifyingKey};

/// The `alg` tag for the Ed25519 backend. **v1** signs the *envelope* (`alg` + `key_id` +
/// `manifest_hash` + `signer` + `signed_at` + `key_format`), not the bare seal — so every envelope field
/// is cryptographically bound and cannot be rewritten on an otherwise-valid signature (false-attribution
/// fix). Note an OpenSSH `id_ed25519` produces the **same** `alg`/`key_id`: `ssh-ed25519` is a host-side
/// key *loader*, not a distinct scheme — the curve, the envelope, and verification are identical.
pub const ALG_ED25519_ENV: &str = "ed25519-env-v1";

/// The exact fields a signature binds: the seal **plus** the envelope identity (`alg`, `key_id`,
/// `signer`) **plus** `signed_at` + `key_format`. Signing the JCS-canonical bytes of this (not the bare
/// `manifest_hash`) is what stops an attacker rewriting any of them on an otherwise-valid signature —
/// in particular `signer` (false attribution) and `signed_at` (so the time-of-signing is trustworthy
/// for later revocation reasoning).
#[derive(Serialize)]
struct SignedPayload<'a> {
    alg: &'a str,
    key_id: &'a str,
    manifest_hash: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    signer: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signed_at: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key_format: Option<&'a str>,
}

/// Optional attributes attached + bound to a signature beyond the key + seal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignOpts {
    /// Signer **identity** (ORCID iD / institutional URI) — FAIR attribution.
    pub signer: Option<String>,
    /// RFC-3339 time of signing — bound, so a later revocation policy can reason about "signed before".
    pub signed_at: Option<String>,
    /// Encoding tag for `key_id` (e.g. `"raw-hex"` for a keygen key, `"ssh-ed25519"` for an OpenSSH key).
    pub key_format: Option<String>,
}

/// The canonical (RFC 8785 JCS) bytes a signature is computed over — the [`SignedPayload`].
fn signed_bytes(env: &Signature, manifest_hash: &str) -> crate::Result<Vec<u8>> {
    crate::canonical::to_bytes(&SignedPayload {
        alg: &env.alg,
        key_id: &env.key_id,
        manifest_hash,
        signer: env.signer.as_deref(),
        signed_at: env.signed_at.as_deref(),
        key_format: env.key_format.as_deref(),
    })
}

/// A detached signature envelope over a `manifest_hash`. Scheme-agnostic (`alg`); `key_id` identifies
/// the verifying key (here, the hex of the 32-byte Ed25519 public key); `signer` is the optional
/// human/institutional identity (e.g. `"https://orcid.org/0000-0002-1825-0097"`); `sig` is the hex
/// detached signature. Serialized beside the product (e.g. `manifest.sig.json`), never inside the
/// manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// Signature scheme tag — `"ed25519"` (`"ssh-ed25519"`, sigstore, … are additive).
    pub alg: String,
    /// Verifying-key identifier — hex of the Ed25519 public key (an ssh fingerprint for `ssh-ed25519`).
    pub key_id: String,
    /// Optional signer **identity** (ORCID iD / institutional URI) — FAIR attribution, distinct from the
    /// key bytes. Omitted from JSON when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
    /// Optional RFC-3339 time of signing (bound into the signature).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_at: Option<String>,
    /// Optional `key_id` encoding tag (e.g. `"raw-hex"` / `"ssh-ed25519"`; bound into the signature).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_format: Option<String>,
    /// The detached signature, hex-encoded.
    pub sig: String,
}

/// Lowercase-hex encode (matches the `blake3:<hex>` digest convention; no extra dep).
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

/// Decode lowercase/upper hex → bytes; `None` on odd length or a non-hex digit.
fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i < b.len() {
        let hi = (b[i] as char).to_digit(16)?;
        let lo = (b[i + 1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Some(out)
}

/// The hex of an Ed25519 **public** key — the `key_id` and the contents of a `.pub` key file.
pub fn verifying_key_hex(key: &VerifyingKey) -> String {
    hex(key.as_bytes())
}

/// The hex of an Ed25519 **signing** key's 32-byte seed — the contents of a private key file (the
/// inverse of [`signing_key_from_hex`]). Used by `tessera keygen` to persist a freshly generated key.
pub fn signing_key_hex(key: &SigningKey) -> String {
    hex(&key.to_bytes())
}

/// Load an Ed25519 **signing** key from a 64-char hex 32-byte seed (leading/trailing whitespace
/// tolerated). Interoperable with any tool that emits the raw 32-byte ed25519 private scalar as hex.
pub fn signing_key_from_hex(s: &str) -> crate::Result<SigningKey> {
    let raw = unhex(s.trim())
        .ok_or_else(|| crate::Error::Invalid("signing key is not valid hex".into()))?;
    let bytes = <[u8; 32]>::try_from(raw.as_slice())
        .map_err(|_| crate::Error::Invalid("signing key must be 32 bytes".into()))?;
    Ok(SigningKey::from_bytes(&bytes))
}

/// Load an Ed25519 **verifying** (public) key from a 64-char hex 32-byte key. Validates the point.
pub fn verifying_key_from_hex(s: &str) -> crate::Result<VerifyingKey> {
    let raw = unhex(s.trim())
        .ok_or_else(|| crate::Error::Invalid("public key is not valid hex".into()))?;
    let bytes = <[u8; 32]>::try_from(raw.as_slice())
        .map_err(|_| crate::Error::Invalid("public key must be 32 bytes".into()))?;
    VerifyingKey::from_bytes(&bytes)
        .map_err(|e| crate::Error::Invalid(format!("invalid ed25519 public key: {e}")))
}

/// Sign a `manifest_hash` with an Ed25519 key, attaching an optional `signer` identity (e.g. an ORCID
/// iD). The signature is over the **envelope** (`alg` + `key_id` + `manifest_hash` + `signer`), JCS-
/// canonicalized — so the seal *and* the attribution identity are bound, and neither can be rewritten
/// on a valid signature. Pair with [`verify`]. Errors only if canonicalization fails (never in practice).
pub fn sign_ed25519(
    manifest_hash: &str,
    key: &SigningKey,
    opts: &SignOpts,
) -> crate::Result<Signature> {
    // Build the envelope first (everything but `sig`), then sign its canonical bytes and fill `sig` in.
    let mut env = Signature {
        alg: ALG_ED25519_ENV.into(),
        key_id: hex(key.verifying_key().as_bytes()),
        signer: opts.signer.clone(),
        signed_at: opts.signed_at.clone(),
        key_format: opts.key_format.clone(),
        sig: String::new(),
    };
    let bytes = signed_bytes(&env, manifest_hash)?;
    env.sig = hex(&key.sign(&bytes).to_bytes());
    Ok(env)
}

/// Verify a [`Signature`] envelope over `manifest_hash` against an Ed25519 `key`. Returns `false` (never
/// panics) on any mismatch: wrong `alg`, a `key_id` that doesn't match `key`, malformed hex/length, or a
/// bad signature. Because the signature is over the **envelope**, tampering with `manifest_hash`, `sig`,
/// `key_id`, **or `signer`** all fail verification (the false-attribution fix).
pub fn verify(manifest_hash: &str, env: &Signature, key: &VerifyingKey) -> bool {
    if env.alg != ALG_ED25519_ENV {
        return false;
    }
    let key_id = hex(key.as_bytes());
    if env.key_id != key_id {
        return false;
    }
    let Ok(bytes) = signed_bytes(env, manifest_hash) else {
        return false;
    };
    let Some(raw) = unhex(&env.sig) else {
        return false;
    };
    let Ok(sig_bytes) = <[u8; 64]>::try_from(raw.as_slice()) else {
        return false;
    };
    let sig = Ed25519Sig::from_bytes(&sig_bytes);
    key.verify(&bytes, &sig).is_ok()
}

/// Sign a **sealed product**: attest its `manifest_hash` with an Ed25519 key + optional `signer`
/// identity. Errors if the manifest is unsealed (no `manifest_hash`). The product-level entry point —
/// a verifier needs only the manifest and the public key.
pub fn sign_manifest(m: &Manifest, key: &SigningKey, opts: &SignOpts) -> crate::Result<Signature> {
    let mh = m.manifest_hash.as_deref().ok_or_else(|| {
        crate::Error::Invalid("cannot sign an unsealed manifest (no manifest_hash)".into())
    })?;
    sign_ed25519(mh, key, opts)
}

/// Verify a [`Signature`] against a sealed product (its `manifest_hash`) + the Ed25519 public key.
/// `false` for an unsealed manifest or any signature/identity mismatch. Because `manifest_hash`
/// transitively commits to every block digest + all metadata, a valid signature here attests that the
/// **entire** product is unaltered since signing.
pub fn verify_manifest(m: &Manifest, env: &Signature, key: &VerifyingKey) -> bool {
    m.manifest_hash
        .as_deref()
        .is_some_and(|mh| verify(mh, env, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic test keypair from a fixed 32-byte seed (no rng dependency).
    fn test_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    /// A `SignOpts` carrying just a `signer` identity (the common test shape).
    fn signed_by(who: &str) -> SignOpts {
        SignOpts {
            signer: Some(who.into()),
            ..Default::default()
        }
    }

    #[test]
    fn sign_then_verify_roundtrips() {
        let key = test_key(7);
        let mh = "blake3:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
        let env = sign_ed25519(
            mh,
            &key,
            &signed_by("https://orcid.org/0000-0002-1825-0097"),
        )
        .unwrap();
        assert_eq!(env.alg, ALG_ED25519_ENV);
        assert_eq!(
            env.signer.as_deref(),
            Some("https://orcid.org/0000-0002-1825-0097")
        );
        assert!(verify(mh, &env, &key.verifying_key()));
    }

    #[test]
    fn tampering_is_rejected() {
        let key = test_key(3);
        let mh = "blake3:aaaa";
        let env = sign_ed25519(mh, &key, &SignOpts::default()).unwrap();

        // a different manifest_hash (the product was altered) → fail.
        assert!(!verify("blake3:bbbb", &env, &key.verifying_key()));
        // a different key → fail (key_id mismatch, caught before the crypto).
        assert!(!verify(mh, &env, &test_key(4).verifying_key()));
        // a corrupted signature → fail.
        let mut bad = env.clone();
        bad.sig.replace_range(0..2, "00");
        assert!(!verify(mh, &bad, &key.verifying_key()));
        // a wrong alg → fail (not silently accepted).
        let mut wrong_alg = env.clone();
        wrong_alg.alg = "ssh-ed25519".into();
        assert!(!verify(mh, &wrong_alg, &key.verifying_key()));
        // malformed hex → fail, not panic.
        let mut malformed = env.clone();
        malformed.sig = "zz".into();
        assert!(!verify(mh, &malformed, &key.verifying_key()));
        // REWRITING `signer` on an otherwise-valid signature → fail (the false-attribution fix:
        // the signature binds the envelope identity, not just the seal).
        let mut relabeled = sign_ed25519(mh, &key, &signed_by("alice")).unwrap();
        relabeled.signer = Some("https://orcid.org/0000-0002-1825-0097".into());
        assert!(!verify(mh, &relabeled, &key.verifying_key()));
        // …and rewriting the bound `signed_at` / `key_format` likewise fails.
        let mut restamped = sign_ed25519(
            mh,
            &key,
            &SignOpts {
                signed_at: Some("2024-01-01T00:00:00Z".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(verify(mh, &restamped, &key.verifying_key()));
        restamped.signed_at = Some("1999-01-01T00:00:00Z".into());
        assert!(!verify(mh, &restamped, &key.verifying_key()));
    }

    #[test]
    fn envelope_serializes_round_trips_and_omits_absent_signer() {
        let key = test_key(1);
        let env = sign_ed25519("blake3:cccc", &key, &SignOpts::default()).unwrap();
        let j = serde_json::to_value(&env).unwrap();
        assert_eq!(j["alg"], "ed25519-env-v1");
        assert!(
            j.get("signer").is_none(),
            "absent signer is omitted from JSON"
        );
        let back: Signature = serde_json::from_value(j).unwrap();
        assert_eq!(back, env);
        assert!(verify("blake3:cccc", &back, &key.verifying_key()));
    }

    #[test]
    fn sign_and_verify_a_sealed_product_end_to_end() {
        use crate::block::array::{ArrayBlock, ArraySpec};
        use crate::ProductBuilder;
        let key = test_key(9);

        let mut b = ProductBuilder::new("recon", "DP", "signed product", "2024-01-01T00:00:00Z");
        b.add_block(&ArrayBlock::new(
            "volume",
            ArraySpec::new(vec![64, 64, 64], "int16"),
        ))
        .unwrap();
        let m = b.seal().unwrap();

        let env = sign_manifest(
            &m,
            &key,
            &signed_by("https://orcid.org/0000-0002-1825-0097"),
        )
        .unwrap();
        assert!(verify_manifest(&m, &env, &key.verifying_key()));

        // tamper the product (a different block) → new manifest_hash → the old signature no longer verifies.
        let mut b2 = ProductBuilder::new("recon", "DP", "signed product", "2024-01-01T00:00:00Z");
        b2.add_block(&ArrayBlock::new(
            "volume",
            ArraySpec::new(vec![64, 64, 32], "int16"),
        ))
        .unwrap();
        let tampered = b2.seal().unwrap();
        assert_ne!(m.manifest_hash, tampered.manifest_hash);
        assert!(!verify_manifest(&tampered, &env, &key.verifying_key()));

        // an unsealed manifest cannot be signed.
        let unsealed = Manifest::new("recon", "x", "d", "2024-01-01T00:00:00Z");
        assert!(sign_manifest(&unsealed, &key, &SignOpts::default()).is_err());
        assert!(!verify_manifest(&unsealed, &env, &key.verifying_key()));
    }

    #[test]
    fn hex_key_loaders_roundtrip_and_reject_bad_input() {
        let key = test_key(5);
        let seed_hex = hex(key.as_bytes()); // the 32-byte seed as hex
        let loaded = signing_key_from_hex(&format!("  {seed_hex}\n")).unwrap(); // whitespace tolerated
        assert_eq!(loaded.to_bytes(), key.to_bytes());
        // public-key load + the `.pub`/key_id hex roundtrip.
        let pub_hex = verifying_key_hex(&key.verifying_key());
        let vk = verifying_key_from_hex(&pub_hex).unwrap();
        assert_eq!(vk.to_bytes(), key.verifying_key().to_bytes());
        // a signature made with the loaded key verifies under the loaded public key.
        let env = sign_ed25519("blake3:dddd", &loaded, &SignOpts::default()).unwrap();
        assert!(verify("blake3:dddd", &env, &vk));
        // bad inputs are rejected, not panicked.
        assert!(signing_key_from_hex("nothex").is_err());
        assert!(signing_key_from_hex("abcd").is_err()); // wrong length
        assert!(verifying_key_from_hex("00").is_err());
    }

    #[test]
    fn hex_roundtrips_and_rejects_bad_input() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff, 0xa5]), "000fffa5");
        assert_eq!(unhex("000fffa5").unwrap(), vec![0x00, 0x0f, 0xff, 0xa5]);
        assert!(unhex("abc").is_none()); // odd length
        assert!(unhex("zz").is_none()); // non-hex
    }
}
