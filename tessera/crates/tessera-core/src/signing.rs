//! Detached **signatures** over a product's `manifest_hash` (S16 / #209 — integrity + tamper-evidence,
//! source-rooted attestation).
//!
//! The seal (`manifest_hash`) transitively commits to `id_inputs`, every block digest, and all metadata
//! (see [`crate::manifest`]), so a signature over the `manifest_hash` string attests the **whole**
//! product with one short signature. A signature is **additive** — it lives beside the manifest, never
//! inside it, so signing (or re-signing) never changes the product's identity.
//!
//! ## Scheme-agnostic envelope
//! [`Signature`] is `(alg, key_id, signer, sig)` — a closed-over-`alg` envelope so new schemes are
//! additive: **ed25519** here (pure-Rust → the sign/verify path compiles to wasm for browser
//! verification), with `ssh-ed25519` (reuse a researcher's `~/.ssh` key) and keyless/sigstore as later
//! backends. `signer` carries the **identity** (e.g. an ORCID iD) independently of the key bytes —
//! attribution is FAIR metadata, not the crypto. Key material may be held at rest under age/sops; that
//! is a host-side loader concern, not part of this verify-able core.

use ed25519_dalek::{Signature as Ed25519Sig, Signer, Verifier};
use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;

/// Re-exported Ed25519 key types so downstream crates sign/verify without a direct `ed25519-dalek`
/// dependency (the crypto boundary lives here, in the core).
pub use ed25519_dalek::{SigningKey, VerifyingKey};

/// The `alg` tag for the Ed25519 backend.
pub const ALG_ED25519: &str = "ed25519";

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

/// Sign a `manifest_hash` with an Ed25519 key, attaching an optional `signer` identity (e.g. an ORCID
/// iD). The signature is over the `manifest_hash` **string bytes** verbatim (the seal — so it attests the
/// whole product). Pair with [`verify`].
pub fn sign_ed25519(manifest_hash: &str, key: &SigningKey, signer: Option<String>) -> Signature {
    let sig = key.sign(manifest_hash.as_bytes());
    Signature {
        alg: ALG_ED25519.into(),
        key_id: hex(key.verifying_key().as_bytes()),
        signer,
        sig: hex(&sig.to_bytes()),
    }
}

/// Verify a [`Signature`] envelope over `manifest_hash` against an Ed25519 `key`. Returns `false` (never
/// panics) on any mismatch: wrong `alg`, a `key_id` that doesn't match `key` (so an envelope can't claim
/// a different key than the one it's checked against), malformed hex/length, or a bad signature. Tamper
/// with the `manifest_hash`, the `sig`, or the key and verification fails.
pub fn verify(manifest_hash: &str, env: &Signature, key: &VerifyingKey) -> bool {
    if env.alg != ALG_ED25519 {
        return false;
    }
    if env.key_id != hex(key.as_bytes()) {
        return false;
    }
    let Some(raw) = unhex(&env.sig) else {
        return false;
    };
    let Ok(bytes) = <[u8; 64]>::try_from(raw.as_slice()) else {
        return false;
    };
    let sig = Ed25519Sig::from_bytes(&bytes);
    key.verify(manifest_hash.as_bytes(), &sig).is_ok()
}

/// Sign a **sealed product**: attest its `manifest_hash` with an Ed25519 key + optional `signer`
/// identity. Errors if the manifest is unsealed (no `manifest_hash`). The product-level entry point —
/// a verifier needs only the manifest and the public key.
pub fn sign_manifest(
    m: &Manifest,
    key: &SigningKey,
    signer: Option<String>,
) -> crate::Result<Signature> {
    let mh = m.manifest_hash.as_deref().ok_or_else(|| {
        crate::Error::Invalid("cannot sign an unsealed manifest (no manifest_hash)".into())
    })?;
    Ok(sign_ed25519(mh, key, signer))
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

    #[test]
    fn sign_then_verify_roundtrips() {
        let key = test_key(7);
        let mh = "blake3:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
        let env = sign_ed25519(
            mh,
            &key,
            Some("https://orcid.org/0000-0002-1825-0097".into()),
        );
        assert_eq!(env.alg, ALG_ED25519);
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
        let env = sign_ed25519(mh, &key, None);

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
    }

    #[test]
    fn envelope_serializes_round_trips_and_omits_absent_signer() {
        let key = test_key(1);
        let env = sign_ed25519("blake3:cccc", &key, None);
        let j = serde_json::to_value(&env).unwrap();
        assert_eq!(j["alg"], "ed25519");
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
            Some("https://orcid.org/0000-0002-1825-0097".into()),
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
        assert!(sign_manifest(&unsealed, &key, None).is_err());
        assert!(!verify_manifest(&unsealed, &env, &key.verifying_key()));
    }

    #[test]
    fn hex_roundtrips_and_rejects_bad_input() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff, 0xa5]), "000fffa5");
        assert_eq!(unhex("000fffa5").unwrap(), vec![0x00, 0x0f, 0xff, 0xa5]);
        assert!(unhex("abc").is_none()); // odd length
        assert!(unhex("zz").is_none()); // non-hex
    }
}
