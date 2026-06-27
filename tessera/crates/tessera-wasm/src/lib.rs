//! Tessera **WASM bindings** (#210) — the in-browser verification surface. `tessera-core` is pure-Rust
//! with no C deps, so the spine (manifest seal · MMR `content_hash` · inclusion/consistency proofs ·
//! ed25519 signature verify · referencing) compiles to wasm and runs in a browser/Node with no server.
//!
//! **Scope (the Arrow boundary).** These bindings verify + locate; they do **not** decode columnar/array
//! payloads in wasm (Vortex/zarrs are native). The intended split is **Arrow-JS as the RS↔TS boundary**:
//! the wasm core confirms a product's integrity/authenticity and reads its manifest, and the heavy
//! columnar read is done in JS via Arrow. So a browser app can prove a `.tsra` is authentic + unaltered
//! before touching its bytes.

use wasm_bindgen::prelude::*;

use tessera_core::manifest::Manifest;
use tessera_core::signing::{self, Signature};

/// Verify a product's **three-hash seal** from its manifest JSON: recomputes `content_hash` (the MMR over
/// the block digests) and `manifest_hash` (the JCS seal) and checks `id`. Returns `true` iff the manifest
/// is internally consistent + sealed. (A full integrity check of the *payloads* additionally re-hashes the
/// blocks — done host-side; in the browser the bytes arrive via Arrow.)
#[wasm_bindgen]
pub fn verify_manifest(manifest_json: &str) -> bool {
    Manifest::from_json(manifest_json)
        .map(|m| m.verify().is_ok())
        .unwrap_or(false)
}

/// Verify an **ed25519 signature** over a product: checks the [`Signature`] envelope (JSON) against the
/// manifest's `manifest_hash` and the signer's hex public key. `true` iff the signature is valid and the
/// product is unaltered since signing. Any malformed input returns `false` (never throws).
#[wasm_bindgen]
pub fn verify_signature(manifest_json: &str, signature_json: &str, pubkey_hex: &str) -> bool {
    let (Ok(m), Ok(sig), Ok(vk)) = (
        Manifest::from_json(manifest_json),
        serde_json::from_str::<Signature>(signature_json),
        signing::verifying_key_from_hex(pubkey_hex),
    ) else {
        return false;
    };
    signing::verify_manifest(&m, &sig, &vk)
}

/// The product `id` from a manifest JSON (the logical identity), or empty string on parse failure — a
/// lightweight read for indexing without a full verify.
#[wasm_bindgen]
pub fn product_id(manifest_json: &str) -> String {
    Manifest::from_json(manifest_json)
        .map(|m| m.id)
        .unwrap_or_default()
}

/// The tessera-wasm crate version (for a JS app to report the verifier build).
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::block::array::{ArrayBlock, ArraySpec};
    use tessera_core::signing::{sign_manifest, verifying_key_hex, SigningKey};
    use tessera_core::ProductBuilder;

    fn sealed_json() -> String {
        let mut b = ProductBuilder::new("recon", "DP", "wasm", "2024-01-01T00:00:00Z");
        b.add_block(&ArrayBlock::new(
            "volume",
            ArraySpec::new(vec![8, 8, 8], "int16"),
        ))
        .unwrap();
        b.seal().unwrap().to_json().unwrap()
    }

    #[test]
    fn verify_manifest_accepts_sealed_rejects_tampered() {
        let json = sealed_json();
        assert!(verify_manifest(&json));
        assert!(!product_id(&json).is_empty());
        // a tampered manifest (flip a hex digit in content_hash) fails the seal recompute.
        let tampered = json.replacen("blake3:", "blake3:0", 1);
        assert!(!verify_manifest(&tampered));
        assert!(!verify_manifest("not json"));
    }

    #[test]
    fn verify_signature_roundtrips_in_the_wasm_surface() {
        let json = sealed_json();
        let key = SigningKey::from_bytes(&[11; 32]);
        let m = Manifest::from_json(&json).unwrap();
        let sig = sign_manifest(&m, &key, None).unwrap();
        let sig_json = serde_json::to_string(&sig).unwrap();
        let pub_hex = verifying_key_hex(&key.verifying_key());
        assert!(verify_signature(&json, &sig_json, &pub_hex));
        // wrong key fails; malformed inputs return false (never panic).
        let other = verifying_key_hex(&SigningKey::from_bytes(&[22; 32]).verifying_key());
        assert!(!verify_signature(&json, &sig_json, &other));
        assert!(!verify_signature("x", "y", "z"));
    }
}
