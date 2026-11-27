//! Tessera **WASM bindings** (#210) — the in-browser verification surface. `tessera-core` is pure-Rust
//! with no C deps, so the spine (manifest seal · MMR `content_hash` · inclusion/consistency proofs ·
//! ed25519 signature verify · referencing) compiles to wasm and runs in a browser/Node with no server.
//!
//! **Scope (the Arrow boundary).** These bindings verify + locate; they do **not** decode columnar/array
//! payloads in wasm (Vortex/zarrs are native). The intended split is **Arrow-JS as the RS↔TS boundary**:
//! the wasm core confirms a product's integrity/authenticity and reads its manifest, and the heavy
//! columnar read is done in JS via Arrow. So a browser app can prove a `.tsra` is authentic + unaltered
//! before touching its bytes.
//!
//! **Inputs (ADR-0066).** A product's manifest is two members: `manifest.json`, stored as exactly its
//! JCS bytes, and `manifest.seal`, the 71 bytes `blake3:<hex>` over them. Every verifier here takes
//! both as the **bytes** the container holds (`Uint8Array`; `manifest_seal` is `undefined` when the
//! member is missing, which is the pre-ADR-0066 alpha layout and is refused), and verifies through
//! [`Manifest::from_stored_members`], the same function the native reader uses: the strict UTF-8 and
//! I-JSON parse, the canonical check, the seal over the stored bytes, then `id` and `content_hash`.
//!
//! Bytes, not strings: the seal is over the stored bytes, and a string is not them. A default
//! `TextDecoder` drops a leading byte-order mark and maps invalid UTF-8 to U+FFFD, so a verifier
//! handed decoded text would check a different document than the one stored, and accept a manifest
//! the native reader refuses (`corpus/seal-cases.json` pins both: `recon_int16/byte-order-mark` and
//! `recon_int16/invalid-utf8-under-the-seal-of-its-replacement`).

use wasm_bindgen::prelude::*;

use tessera_core::manifest::Manifest;
use tessera_core::signing::{self, Signature};
use tessera_core::Error;

fn read(manifest_json: &[u8], manifest_seal: Option<&[u8]>) -> tessera_core::Result<Manifest> {
    Manifest::from_stored_members(manifest_json, manifest_seal)
}

/// Verify a product's **three-hash seal** from its stored `manifest.json` and `manifest.seal`:
/// `true` iff the bytes are their own JCS form, I-JSON, carry no in-band seal, hash to the seal, and
/// `id` and `content_hash` hold. (A full integrity check of the *payloads* additionally re-hashes the
/// blocks — done host-side; in the browser the bytes arrive via Arrow.) Fields from a newer format
/// revision verify; a key spliced into a sealed manifest, a respelling (`64.0` for `64`, a pretty
/// layout) and a duplicate key don't.
#[wasm_bindgen]
pub fn verify_manifest(manifest_json: &[u8], manifest_seal: Option<Vec<u8>>) -> bool {
    read(manifest_json, manifest_seal.as_deref()).is_ok()
}

/// A stored manifest's verdict in the vocabulary of `corpus/seal-cases.json`: `"ok"` (it verifies),
/// `"tampered"` (an integrity failure: the seal, `id` or `content_hash` doesn't match, or
/// `content_hash` is missing), or `"refused"` (not read at all: the alpha layout or a malformed
/// seal, non-canonical bytes, not I-JSON, the seal in band, another declared form, an unsupported
/// major version, a block `class` other than `"derived"`, a block listed twice or without a digest, a
/// chunk index whose block the manifest doesn't list).
/// `content_hash` is over the primary blocks only (ADR-0028 §4). The verdict the native reader and the
/// Python reference reader reach on the same pair.
#[wasm_bindgen]
pub fn seal_verdict(manifest_json: &[u8], manifest_seal: Option<Vec<u8>>) -> String {
    match read(manifest_json, manifest_seal.as_deref()) {
        Ok(_) => "ok",
        Err(Error::Integrity { .. }) => "tampered",
        Err(_) => "refused",
    }
    .to_owned()
}

/// A collection's stored-byte verdict, with the vocabulary of [`seal_verdict`].
/// Takes `collection.json` and optional `collection.seal` bytes. Checks the seal, declared form,
/// identity and kind-tagged member MMR through the native collection reader, including sealed
/// `sources` and newer fields (SPEC §4, ADR-0067). Does not resolve members or edge targets.
#[wasm_bindgen]
pub fn collection_seal_verdict(collection_json: &[u8], collection_seal: Option<Vec<u8>>) -> String {
    match tessera_core::Collection::from_stored_members(collection_json, collection_seal.as_deref())
    {
        Ok(_) => "ok",
        Err(Error::Integrity { .. }) => "tampered",
        Err(_) => "refused",
    }
    .to_owned()
}

/// Verify an **ed25519 signature** over a product: checks the [`Signature`] envelope (JSON) against the
/// manifest's seal and the signer's hex public key. `true` iff the signature is valid and the product
/// is unaltered since signing: the stored manifest is verified against its seal first
/// ([`verify_manifest`]), since the signature binds only the seal. Any malformed input returns `false`
/// (never throws).
#[wasm_bindgen]
pub fn verify_signature(
    manifest_json: &[u8],
    manifest_seal: Option<Vec<u8>>,
    signature_json: &str,
    pubkey_hex: &str,
) -> bool {
    let (Ok(m), Ok(sig), Ok(vk)) = (
        read(manifest_json, manifest_seal.as_deref()),
        Signature::from_json(signature_json),
        signing::verifying_key_from_hex(pubkey_hex),
    ) else {
        return false;
    };
    signing::verify_manifest(&m, &sig, &vk)
}

/// The product `id` a stored `manifest.json` **claims** (the logical identity), or empty string when
/// the bytes are not UTF-8 or don't parse — a lightweight read for indexing, **unverified**: verify
/// with [`verify_manifest`] before trusting it.
#[wasm_bindgen]
pub fn product_id(manifest_json: &[u8]) -> String {
    std::str::from_utf8(manifest_json)
        .ok()
        .and_then(|text| Manifest::from_json(text).ok())
        .map(|m| m.id)
        .unwrap_or_default()
}

/// Recompute the primary-block Merkle root from a manifest's block digests, excluding derived blocks.
/// This does not verify the stored `content_hash`, seal or payload bytes. Returns an empty string if
/// the input is not UTF-8, cannot be parsed as a manifest, or a block digest is missing.
#[wasm_bindgen]
pub fn primary_content_root(manifest_json: &[u8]) -> String {
    std::str::from_utf8(manifest_json)
        .ok()
        .and_then(|text| Manifest::from_json(text).ok())
        .and_then(|m| m.recompute_content_hash().ok())
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
    use tessera_core::signing::{sign_manifest, verifying_key_hex, SignOpts, SigningKey};
    use tessera_core::ProductBuilder;

    fn stored() -> (Vec<u8>, Vec<u8>) {
        let mut b = ProductBuilder::new("recon", "DP", "wasm", "2024-01-01T00:00:00Z");
        b.add_block(&ArrayBlock::new(
            "volume",
            ArraySpec::new(vec![8, 8, 8], "int16"),
        ))
        .unwrap();
        let (bytes, seal) = b.seal().unwrap().to_stored().unwrap();
        (bytes, seal.into_bytes())
    }

    #[test]
    fn verify_manifest_accepts_sealed_rejects_tampered() {
        let (json, seal) = stored();
        assert!(verify_manifest(&json, Some(seal.clone())));
        assert!(!product_id(&json).is_empty());
        // a tampered manifest (flip a hex digit in content_hash) fails the seal.
        let text = String::from_utf8(json.clone()).unwrap();
        let tampered = text.replacen("blake3:", "blake3:0", 1).into_bytes();
        assert!(!verify_manifest(&tampered, Some(seal.clone())));
        assert_eq!(seal_verdict(&tampered, Some(seal.clone())), "tampered");
        // no seal member: the alpha layout, refused.
        assert!(!verify_manifest(&json, None));
        assert_eq!(seal_verdict(&json, None), "refused");
        assert!(!verify_manifest(b"not json", Some(seal)));
    }

    #[test]
    fn verify_signature_roundtrips_in_the_wasm_surface() {
        let (json, seal) = stored();
        let key = SigningKey::from_bytes(&[11; 32]);
        let m = read(&json, Some(&seal)).unwrap();
        let sig = sign_manifest(&m, &key, &SignOpts::default()).unwrap();
        let sig_json = serde_json::to_string(&sig).unwrap();
        let pub_hex = verifying_key_hex(&key.verifying_key());
        assert!(verify_signature(
            &json,
            Some(seal.clone()),
            &sig_json,
            &pub_hex
        ));
        // wrong key fails; malformed inputs return false (never panic).
        let other = verifying_key_hex(&SigningKey::from_bytes(&[22; 32]).verifying_key());
        assert!(!verify_signature(&json, Some(seal), &sig_json, &other));
        assert!(!verify_signature(b"x", None, "y", "z"));
    }

    /// A case's `manifest.json` bytes: the UTF-8 string they spell (`manifest`), or `manifest_hex` for
    /// bytes no JSON string holds (invalid UTF-8).
    fn manifest_bytes(c: &serde_json::Value) -> Vec<u8> {
        match c["manifest_hex"].as_str() {
            Some(hex) => (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("manifest_hex is hex"))
                .collect(),
            None => c["manifest"].as_str().unwrap().as_bytes().to_vec(),
        }
    }

    /// The wasm surface reaches every verdict of `corpus/seal-cases.json`, the file the native
    /// reader and the reference reader are held to (ADR-0066 step 4: all three agree). The
    /// `wasm-bindgen-smoke` check runs the same cases through the compiled wasm in Node.
    #[test]
    fn seal_verdict_agrees_with_every_seal_case() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/seal-cases.json");
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let cases = doc["cases"].as_array().unwrap();
        assert!(!cases.is_empty());
        let disagreements: Vec<String> = cases
            .iter()
            .filter_map(|c| {
                let verdict = if c["kind"] == "collection" {
                    collection_seal_verdict
                } else {
                    seal_verdict
                };
                let got = verdict(
                    &manifest_bytes(c),
                    c["seal"].as_str().map(|s| s.as_bytes().to_vec()),
                );
                (got != c["verdict"].as_str().unwrap())
                    .then(|| format!("{}: expected {}, got {got}", c["name"], c["verdict"]))
            })
            .collect();
        assert!(disagreements.is_empty(), "{disagreements:#?}");
    }
}
