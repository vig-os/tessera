//! Sign / verify a sealed **`.tsra` container** end-to-end (S16 / #209). Ties [`tessera_core::signing`]
//! (the detached-signature envelope over `manifest_hash`) to a product on disk via a **`<file>.sig.json`
//! sidecar** — additive, beside the container, never inside it (signing never changes identity).
//!
//! The sidecar is the conventional distribution unit: ship `data.tsra` + `data.tsra.sig.json`; a verifier
//! opens the container, reads its sealed manifest, and checks the signature against the signer's public
//! key — confirming the **entire** product is unaltered since signing (because `manifest_hash`
//! transitively commits to every block digest + all metadata).

use std::path::{Path, PathBuf};

use tessera_core::signing::{self, SignOpts, Signature, SigningKey, VerifyingKey};
use tessera_core::{Error, Result};

use crate::container::Reader;

/// The conventional sidecar path for a container: `<file>.tsra` → `<file>.tsra.sig.json`.
pub fn sidecar_path(tsra: &Path) -> PathBuf {
    let mut s = tsra.as_os_str().to_os_string();
    s.push(".sig.json");
    PathBuf::from(s)
}

/// Sign a sealed `.tsra` and write the `<file>.tsra.sig.json` sidecar. Reads the container's sealed
/// manifest, signs its `manifest_hash` (ed25519) with the given [`SignOpts`] (signer identity / `signed_at`
/// / `key_format`, all bound into the signature), and returns the written [`Signature`].
pub fn sign_tsra(tsra: &Path, key: &SigningKey, opts: &SignOpts) -> Result<Signature> {
    let reader = Reader::open(tsra)?;
    let env = signing::sign_manifest(reader.manifest(), key, opts)?;
    let json = serde_json::to_vec_pretty(&env).map_err(|e| Error::Container(e.to_string()))?;
    std::fs::write(sidecar_path(tsra), json)?;
    tracing::info!(
        target: "tessera::sign",
        file = %tsra.display(),
        key_id = %env.key_id,
        signer = env.signer.as_deref().unwrap_or("-"),
        "signed container"
    );
    Ok(env)
}

/// Read the sidecar [`Signature`] for a container (`<file>.tsra.sig.json`).
pub fn read_signature(tsra: &Path) -> Result<Signature> {
    let bytes = std::fs::read(sidecar_path(tsra))?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Container(e.to_string()))
}

/// Verify a sealed `.tsra` against its sidecar signature + the signer's Ed25519 public key. Opens the
/// container, reads its sealed manifest, and checks the signature — `true` only if the product is
/// unaltered since signing and the key matches. Any tamper (a swapped block, edited metadata, a forged
/// sidecar) fails.
pub fn verify_tsra(tsra: &Path, key: &VerifyingKey) -> Result<bool> {
    let reader = Reader::open(tsra)?;
    let env = read_signature(tsra)?;
    Ok(signing::verify_manifest(reader.manifest(), &env, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{self, ArrayData};
    use crate::container::pack;
    use tessera_core::block::array::ArraySpec;
    use tessera_core::ProductBuilder;

    fn sealed_tsra(dir: &Path, name: &str, n: i16) -> PathBuf {
        let mut spec = ArraySpec::new(vec![8, 8, 8], "int16");
        spec.codec = "pcodec".into();
        let data = ArrayData::I16((0..512).map(|k| k as i16 + n).collect());
        let (bref, payload) = array::array_block("volume", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("recon", "DP", "signed", "2024-01-01T00:00:00Z");
        b.add_block_ref(bref);
        let manifest = b.seal().unwrap();
        let out = dir.join(name);
        pack(&manifest, &[payload], &out).unwrap();
        out
    }

    #[test]
    fn sign_and_verify_a_tsra_via_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let key = SigningKey::from_bytes(&[42; 32]);
        let tsra = sealed_tsra(dir.path(), "data.tsra", 0);

        // sign → sidecar written → verifies against the public key.
        let env = sign_tsra(
            &tsra,
            &key,
            &SignOpts {
                signer: Some("https://orcid.org/0000-0002-1825-0097".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(sidecar_path(&tsra).exists());
        assert_eq!(
            env.signer.as_deref(),
            Some("https://orcid.org/0000-0002-1825-0097")
        );
        assert!(verify_tsra(&tsra, &key.verifying_key()).unwrap());

        // the read-back sidecar equals the returned envelope.
        assert_eq!(read_signature(&tsra).unwrap(), env);

        // a different signer's key fails.
        let other = SigningKey::from_bytes(&[7; 32]);
        assert!(!verify_tsra(&tsra, &other.verifying_key()).unwrap());

        // a different product (changed block) with the original signature copied over fails — the
        // attacker can't move a valid signature onto a different product.
        let tampered = sealed_tsra(dir.path(), "tampered.tsra", 1);
        std::fs::copy(sidecar_path(&tsra), sidecar_path(&tampered)).unwrap();
        assert!(!verify_tsra(&tampered, &key.verifying_key()).unwrap());
    }
}
