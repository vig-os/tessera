//! Integration test for the `tessera sign` / `verify-sig` verbs — drives the real binary end-to-end
//! (resolved via `CARGO_BIN_EXE_tessera`, so it runs in the hermetic flake check with no cargo).
//! Process-level + exit-code based (not a trycmd snapshot, which can't capture the temp-path output).

use std::path::{Path, PathBuf};
use std::process::Command;

use tessera_core::block::array::ArraySpec;
use tessera_core::signing::{verifying_key_hex, SigningKey};
use tessera_core::ProductBuilder;
use tessera_io::array::{self, ArrayData};
use tessera_io::pack;

fn tessera() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tessera"))
}

/// Build a sealed `.tsra` fixture at `out`.
fn sealed_tsra(out: &Path) {
    let mut spec = ArraySpec::new(vec![8, 8, 8], "int16");
    spec.codec = "pcodec".into();
    let data = ArrayData::I16((0..512).map(|k| k as i16).collect());
    let (bref, payload) = array::array_block("volume", &spec, &data).unwrap();
    let mut b = ProductBuilder::new("recon", "DP", "signed", "2024-01-01T00:00:00Z");
    b.add_block_ref(bref);
    let manifest = b.seal().unwrap();
    pack(&manifest, &[payload], out).unwrap();
}

#[test]
fn cli_sign_then_verify_sig_roundtrips_and_rejects_wrong_key() {
    let dir = tempfile::tempdir().unwrap();
    let tsra = dir.path().join("data.tsra");
    sealed_tsra(&tsra);

    // a deterministic ed25519 keypair, written as hex key files (the CLI's interchange form).
    let sk = SigningKey::from_bytes(&[17u8; 32]);
    let key_file = dir.path().join("signer.key");
    let pub_file = dir.path().join("signer.pub");
    std::fs::write(&key_file, hex32(&sk.to_bytes())).unwrap();
    std::fs::write(&pub_file, verifying_key_hex(&sk.verifying_key())).unwrap();

    // sign → exit 0 + sidecar written.
    let sign = tessera()
        .args(["sign"])
        .arg(&tsra)
        .arg("--key")
        .arg(&key_file)
        .args(["--signer", "https://orcid.org/0000-0002-1825-0097"])
        .status()
        .unwrap();
    assert!(sign.success(), "sign should exit 0");
    let sidecar = PathBuf::from(format!("{}.sig.json", tsra.display()));
    assert!(sidecar.exists(), "sidecar must be written");

    // verify-sig with the right public key → exit 0.
    let ok = tessera()
        .args(["verify-sig"])
        .arg(&tsra)
        .arg("--pubkey")
        .arg(&pub_file)
        .status()
        .unwrap();
    assert!(
        ok.success(),
        "verify-sig should exit 0 for a valid signature"
    );

    // verify-sig with a DIFFERENT public key → non-zero exit (tamper-evident).
    let wrong_pub = dir.path().join("wrong.pub");
    let other = SigningKey::from_bytes(&[99u8; 32]);
    std::fs::write(&wrong_pub, verifying_key_hex(&other.verifying_key())).unwrap();
    let bad = tessera()
        .args(["verify-sig"])
        .arg(&tsra)
        .arg("--pubkey")
        .arg(&wrong_pub)
        .status()
        .unwrap();
    assert!(
        !bad.success(),
        "verify-sig must fail (non-zero) for the wrong key"
    );
}

/// Lowercase-hex of 32 bytes (the key-file form).
fn hex32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
