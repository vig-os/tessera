//! Offline crypto-shred de-identification envelope (ADR-0047 / #269).
//!
//! When a DICOM is ingested with `--crypto-shred --recipient <age-pubkey>`, the identifying material
//! that is stripped from the shared, de-identified product is not *destroyed* — it is gathered into an
//! [`IdentityDocument`], `age`-encrypted to the recipient public key(s) (X25519 + ChaCha20-Poly1305,
//! the sops model), and carried as an **aux member** [`AUX_IDENTITY_NAME`] *inside* the one shareable
//! `.tsra` but *outside* the seal (ADR-0042). Two operations follow:
//!
//! - **re-identify** — a holder of a recipient *private* key runs `tessera reidentify`, which calls
//!   [`decrypt_identity`] to recover the original identity. A key "un-SOPs" the product.
//! - **shred** — dropping the aux member (`tessera shred`) or destroying every copy of the private key
//!   renders the identifying material irrecoverable on every distributed copy (ADR-0040 §2.2).
//!
//! Host-only: this is the encrypt/decrypt path, never the wasm verify path (verifying a crypto-shred
//! product's seal + signature needs no key — the sealed product underneath is an ordinary
//! de-identified `recon`).

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use tessera_core::{Error, Result};

/// The aux-member path (relative to `aux/`) that carries the encrypted identity envelope — so the
/// full path inside the container is `aux/identity/identity.age` (ADR-0042/0047).
pub const AUX_IDENTITY_NAME: &str = "identity/identity.age";

/// Everything identifying that was removed from the shared, de-identified product, gathered so a
/// holder of a recipient private key can reconstruct it. JCS-canonicalized, then `age`-encrypted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityDocument {
    /// Envelope schema version (bump on any breaking shape change).
    pub v: u32,
    /// The full **pre-scrub** DICOM header (`"GGGG,EEEE"` → `{vr, value}`) — the complete identifying
    /// record, so re-identification is exact rather than best-effort over a curated subset.
    pub dicom_header: serde_json::Value,
    /// The curated `identifying`-tier schema fields echoed for convenience (patient_pseudonym, the
    /// study/series UIDs, study_date) — a small structured view over the header for quick re-linking.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub curated_identifying: serde_json::Map<String, serde_json::Value>,
}

impl IdentityDocument {
    /// The current envelope schema version.
    pub const VERSION: u32 = 1;
}

/// Generate a fresh `age` X25519 keypair for crypto-shred (ADR-0047). Returns
/// `(secret_key, recipient_public_key)` as strings: the secret is `AGE-SECRET-KEY-1…` (give to
/// `tessera reidentify --identity`, keep 0600), the public is `age1…` (give to `ingest --recipient`,
/// share freely).
pub fn generate_keypair() -> (String, String) {
    use age::secrecy::ExposeSecret;
    let id = age::x25519::Identity::generate();
    let public = id.to_public().to_string();
    let secret = id.to_string().expose_secret().to_string();
    (secret, public)
}

/// Parse an `age` recipient **public** key (`age1…`) from its string form.
pub fn parse_recipient(s: &str) -> Result<age::x25519::Recipient> {
    s.trim()
        .parse()
        .map_err(|e| Error::Invalid(format!("invalid age recipient public key: {e}")))
}

/// Encrypt an [`IdentityDocument`] to one or more `age` recipients → the ASCII-armored envelope bytes
/// destined for [`AUX_IDENTITY_NAME`]. Errors if `recipients` is empty (crypto-shred with no key would
/// silently destroy the identity — that is what plain `--deidentify` is for; refuse the ambiguity).
pub fn encrypt_identity(
    doc: &IdentityDocument,
    recipients: &[age::x25519::Recipient],
) -> Result<Vec<u8>> {
    if recipients.is_empty() {
        return Err(Error::Invalid(
            "crypto-shred requires at least one --recipient (age public key)".into(),
        ));
    }
    // JCS-canonical plaintext — deterministic bytes for the *plaintext* (the ciphertext is not, by
    // design: age nonces every encrypt, which is exactly why the envelope rides outside the seal).
    let plaintext = tessera_core::canonical::to_bytes(doc)?;

    let recips: Vec<&dyn age::Recipient> = recipients
        .iter()
        .map(|r| r as &dyn age::Recipient)
        .collect();
    let encryptor = age::Encryptor::with_recipients(recips.into_iter())
        .map_err(|e| Error::Invalid(format!("age encryptor: {e}")))?;

    // ASCII-armored so `unzip -p study.tsra aux/identity/identity.age` reveals an obvious, `age -d`-
    // compatible envelope rather than opaque binary.
    let mut out = Vec::new();
    let armor = age::armor::ArmoredWriter::wrap_output(&mut out, age::armor::Format::AsciiArmor)
        .map_err(|e| Error::Invalid(format!("age armor: {e}")))?;
    let mut writer = encryptor
        .wrap_output(armor)
        .map_err(|e| Error::Invalid(format!("age wrap: {e}")))?;
    writer.write_all(&plaintext)?;
    let armor = writer
        .finish()
        .map_err(|e| Error::Invalid(format!("age finish: {e}")))?;
    armor
        .finish()
        .map_err(|e| Error::Invalid(format!("age armor finish: {e}")))?;
    Ok(out)
}

/// Decrypt an identity envelope (the bytes of [`AUX_IDENTITY_NAME`]) with a recipient **private** key
/// (`AGE-SECRET-KEY-1…`) → the recovered [`IdentityDocument`]. Accepts both armored and binary age
/// input (the [`age::armor::ArmoredReader`] passes binary through). A wrong key, a corrupt envelope,
/// or a non-identity secret all surface as a typed [`Error::Invalid`] rather than a panic.
pub fn decrypt_identity(envelope: &[u8], identity_secret: &str) -> Result<IdentityDocument> {
    let identity: age::x25519::Identity = identity_secret
        .trim()
        .parse()
        .map_err(|e| Error::Invalid(format!("invalid age identity (secret key): {e}")))?;

    let reader = age::armor::ArmoredReader::new(envelope);
    let decryptor =
        age::Decryptor::new(reader).map_err(|e| Error::Invalid(format!("age decryptor: {e}")))?;
    let mut plaintext = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|e| {
            Error::Invalid(format!(
                "age decrypt failed (wrong key or tampered envelope): {e}"
            ))
        })?;
    let mut buf = Vec::new();
    plaintext.read_to_end(&mut buf)?;
    serde_json::from_slice(&buf).map_err(Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use age::secrecy::ExposeSecret;

    fn keypair() -> (age::x25519::Recipient, String) {
        let id = age::x25519::Identity::generate();
        (id.to_public(), id.to_string().expose_secret().to_string())
    }

    fn doc() -> IdentityDocument {
        IdentityDocument {
            v: IdentityDocument::VERSION,
            dicom_header: serde_json::json!({
                "0010,0010": {"vr": "PN", "value": ["SMITH^JOHN"]},
                "0010,0020": {"vr": "LO", "value": ["MRN-000123"]},
            }),
            curated_identifying: serde_json::Map::new(),
        }
    }

    #[test]
    fn envelope_round_trips_with_the_right_key() {
        let (pk, sk) = keypair();
        let env = encrypt_identity(&doc(), &[pk]).unwrap();
        // Armored envelope is ASCII and self-identifying.
        assert!(
            env.starts_with(b"-----BEGIN AGE ENCRYPTED FILE-----"),
            "not armored"
        );
        // The ciphertext never contains the plaintext PHI.
        let as_text = String::from_utf8_lossy(&env);
        assert!(!as_text.contains("SMITH"), "PHI leaked into ciphertext");
        assert!(
            !as_text.contains("MRN-000123"),
            "PHI leaked into ciphertext"
        );
        // The right key recovers it exactly.
        let back = decrypt_identity(&env, &sk).unwrap();
        assert_eq!(back, doc());
    }

    #[test]
    fn a_different_key_cannot_decrypt() {
        let (pk, _sk) = keypair();
        let (_pk2, sk2) = keypair();
        let env = encrypt_identity(&doc(), &[pk]).unwrap();
        // A holder of a *different* private key is shut out — the crypto-shred guarantee.
        assert!(decrypt_identity(&env, &sk2).is_err());
    }

    #[test]
    fn multi_recipient_envelope_opens_for_each() {
        let (pk1, sk1) = keypair();
        let (pk2, sk2) = keypair();
        let env = encrypt_identity(&doc(), &[pk1, pk2]).unwrap();
        assert_eq!(decrypt_identity(&env, &sk1).unwrap(), doc());
        assert_eq!(decrypt_identity(&env, &sk2).unwrap(), doc());
    }

    #[test]
    fn no_recipient_is_refused() {
        assert!(encrypt_identity(&doc(), &[]).is_err());
    }

    #[test]
    fn a_bad_recipient_string_is_a_typed_error() {
        assert!(parse_recipient("not-an-age-key").is_err());
    }
}
