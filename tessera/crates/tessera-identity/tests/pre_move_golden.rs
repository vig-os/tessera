//! The envelope format did not move with the code (#762).
//!
//! `fixtures/pre-move-envelope.age` and `fixtures/pre-move-plaintext.json` were written by
//! `tessera_ingest::identity::encrypt_identity` at `26159de`, before the primitives moved into this
//! crate (by a one-off test against that commit, not kept). The tests pass on either side of the move;
//! they guard against the format changing from here on. An `age` ciphertext is not reproducible (a fresh file key and ephemeral share per call), so the
//! envelope cannot be pinned byte for byte. What can be pinned is everything the code decides: the
//! armor, the recipient stanza type, and the plaintext bytes, which are the document's JCS form.

use std::io::Read;

use tessera_identity::{decrypt_identity, encrypt_identity, parse_recipient, IdentityDocument};

/// A throwaway key generated for this fixture only. It protects nothing but the dummy document below.
const FIXTURE_SECRET: &str =
    "AGE-SECRET-KEY-12NUTPPNR5RZ52WMXZGVCLGEG9DPF8VDN9XNNT7FAVU3DMU3LTE3S5YYEUJ"; // gitleaks:allow

const PRE_MOVE_ENVELOPE: &[u8] = include_bytes!("fixtures/pre-move-envelope.age");
const PRE_MOVE_PLAINTEXT: &[u8] = include_bytes!("fixtures/pre-move-plaintext.json");

/// The document the fixture encrypts. Its keys are written out of JCS order on purpose, so the golden
/// plaintext also pins that serialization sorts them.
fn fixture_doc() -> IdentityDocument {
    let mut curated = serde_json::Map::new();
    curated.insert("study_date".into(), serde_json::json!("20260101"));
    curated.insert(
        "patient_pseudonym".into(),
        serde_json::json!("pseud-Ünïcode"),
    );
    IdentityDocument {
        v: IdentityDocument::VERSION,
        dicom_header: serde_json::json!({
            "0010,0020": {"vr": "LO", "value": ["MRN-000123"]},
            "0010,0010": {"value": ["SMITH^JOHN"], "vr": "PN"},
        }),
        curated_identifying: curated,
    }
}

fn fixture_identity() -> age::x25519::Identity {
    FIXTURE_SECRET.parse().unwrap()
}

/// Decrypt with `age` alone, bypassing this crate, so the bytes compared are the envelope's own.
fn raw_plaintext(envelope: &[u8]) -> Vec<u8> {
    let decryptor = age::Decryptor::new(age::armor::ArmoredReader::new(envelope)).unwrap();
    let identity = fixture_identity();
    let mut reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .unwrap();
    let mut out = Vec::new();
    reader.read_to_end(&mut out).unwrap();
    out
}

#[test]
fn the_pre_move_envelope_decrypts_to_the_fixture_document() {
    assert_eq!(
        decrypt_identity(PRE_MOVE_ENVELOPE, FIXTURE_SECRET).unwrap(),
        fixture_doc()
    );
    assert_eq!(raw_plaintext(PRE_MOVE_ENVELOPE), PRE_MOVE_PLAINTEXT);
}

#[test]
fn a_new_envelope_carries_the_pre_move_plaintext_bytes() {
    let recipient = parse_recipient(&fixture_identity().to_public().to_string()).unwrap();
    let envelope = encrypt_identity(&fixture_doc(), &[recipient]).unwrap();
    assert_eq!(raw_plaintext(&envelope), PRE_MOVE_PLAINTEXT);
}

#[test]
fn a_new_envelope_has_the_pre_move_framing() {
    let recipient = fixture_identity().to_public();
    let envelope = encrypt_identity(&fixture_doc(), &[recipient]).unwrap();
    for env in [PRE_MOVE_ENVELOPE, envelope.as_slice()] {
        let text = std::str::from_utf8(env).unwrap();
        assert!(text.starts_with("-----BEGIN AGE ENCRYPTED FILE-----\n"));
        assert!(text.ends_with("-----END AGE ENCRYPTED FILE-----\n"));
        // De-armored, the header's first stanza is the X25519 recipient (not scrypt, not a plugin).
        let mut binary = Vec::new();
        age::armor::ArmoredReader::new(env)
            .read_to_end(&mut binary)
            .unwrap();
        assert!(binary.starts_with(b"age-encryption.org/v1\n-> X25519 "));
    }
}
