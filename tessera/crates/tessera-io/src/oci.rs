//! Distribute a sealed `.tsra` as an **OCI artifact** (#209) — so a Tessera product rides standard
//! container registries (Harbor, GHCR, zot, ECR) with their auth, replication, GC, and content-addressing
//! for free.
//!
//! This module builds the **OCI 1.1 artifact manifest** for a `.tsra` (pure function of the bytes); the
//! push/pull of the manifest + blobs to a registry is the transport layer (validated against a local
//! `zot`/`registry:2` in CI). An artifact manifest is an `image.manifest.v1` with:
//! - an **`artifactType`** of [`ARTIFACT_TYPE`] (so registries/tools recognise it as a Tessera product,
//!   not a runnable image);
//! - the OCI **empty config** ([`EMPTY_CONFIG_MEDIA_TYPE`]) — an artifact has no runnable config;
//! - one **layer** = the `.tsra` blob ([`TSRA_MEDIA_TYPE`]), addressed by its **sha256** (registries are
//!   sha256-addressed; this is *additional* to Tessera's internal blake3 identity, which is preserved
//!   inside the container).
//!
//! Annotations carry the product `id` + `manifest_hash` so the Tessera identity is discoverable from the
//! registry without unpacking.

use sha2::{Digest, Sha256};

/// The OCI **artifact type** marking a Tessera product manifest.
pub const ARTIFACT_TYPE: &str = "application/vnd.tessera.product.v1+json";
/// The media type of the `.tsra` container layer.
pub const TSRA_MEDIA_TYPE: &str = "application/vnd.tessera.tsra.v1";
/// The OCI standard empty-config media type (an artifact has no runnable config).
pub const EMPTY_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.empty.v1+json";
/// The OCI image-manifest media type.
pub const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
/// The media type of the detached-signature sidecar (`<file>.tsra.sig.json`) carried as a 2nd layer,
/// so `push`/`pull` keep the signature with the product through a registry.
pub const SIGNATURE_MEDIA_TYPE: &str = "application/vnd.tessera.signature.v1+json";

/// The OCI empty descriptor blob — the 2 bytes `{}` (sha256 `44136f…aff8a`), referenced by the config.
pub const EMPTY_BLOB: &[u8] = b"{}";

/// `sha256:<hex>` digest of `bytes` — the OCI content-address form.
pub fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Build the OCI 1.1 **artifact manifest** (a serde JSON value) for a sealed `.tsra`. `tsra` is the
/// container bytes; `product_id` / `manifest_hash` are recorded as annotations so the Tessera identity is
/// visible at the registry. The manifest references the `.tsra` as its single layer (sha256-addressed)
/// and the OCI empty config.
///
/// Push procedure (transport layer): PUT the empty-config blob ([`EMPTY_BLOB`]) and the `.tsra` blob, then
/// PUT this manifest under a tag — a standard OCI push (`oras push`, or the distribution API).
pub fn artifact_manifest(tsra: &[u8], product_id: &str, manifest_hash: &str) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": 2,
        "mediaType": MANIFEST_MEDIA_TYPE,
        "artifactType": ARTIFACT_TYPE,
        "config": {
            "mediaType": EMPTY_CONFIG_MEDIA_TYPE,
            "digest": sha256_digest(EMPTY_BLOB),
            "size": EMPTY_BLOB.len(),
        },
        "layers": [{
            "mediaType": TSRA_MEDIA_TYPE,
            "digest": sha256_digest(tsra),
            "size": tsra.len(),
            "annotations": {
                "org.opencontainers.image.title": "product.tsra",
            },
        }],
        "annotations": {
            "vnd.tessera.id": product_id,
            "vnd.tessera.manifest_hash": manifest_hash,
            "org.opencontainers.image.title": product_id,
        },
    })
}

/// Like [`artifact_manifest`] but carrying the detached-signature sidecar as a **second layer**
/// ([`SIGNATURE_MEDIA_TYPE`]) — so `tessera push` keeps `<file>.tsra.sig.json` with the product through
/// the registry and `tessera pull` recovers it. Both blobs must be uploaded before this manifest.
pub fn artifact_manifest_signed(
    tsra: &[u8],
    sig: &[u8],
    product_id: &str,
    manifest_hash: &str,
) -> serde_json::Value {
    let mut m = artifact_manifest(tsra, product_id, manifest_hash);
    if let Some(layers) = m["layers"].as_array_mut() {
        layers.push(serde_json::json!({
            "mediaType": SIGNATURE_MEDIA_TYPE,
            "digest": sha256_digest(sig),
            "size": sig.len(),
            "annotations": { "org.opencontainers.image.title": "product.tsra.sig.json" },
        }));
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_blob_has_the_canonical_oci_empty_digest() {
        // the OCI 1.1 standard empty descriptor digest (sha256 of `{}`).
        assert_eq!(
            sha256_digest(EMPTY_BLOB),
            "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        assert_eq!(EMPTY_BLOB.len(), 2);
    }

    #[test]
    fn artifact_manifest_is_a_valid_oci_artifact_for_the_tsra() {
        let tsra = b"PK\x03\x04 ... pretend .tsra bytes ...";
        let m = artifact_manifest(tsra, "blake3:abc123", "blake3:def456");

        assert_eq!(m["schemaVersion"], 2);
        assert_eq!(m["mediaType"], MANIFEST_MEDIA_TYPE);
        assert_eq!(m["artifactType"], ARTIFACT_TYPE);
        // config = the OCI empty config.
        assert_eq!(m["config"]["mediaType"], EMPTY_CONFIG_MEDIA_TYPE);
        assert_eq!(m["config"]["size"], 2);
        // exactly one layer = the .tsra, sha256-addressed with the right size.
        let layers = m["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0]["mediaType"], TSRA_MEDIA_TYPE);
        assert_eq!(layers[0]["digest"], sha256_digest(tsra));
        assert_eq!(layers[0]["size"], tsra.len());
        // Tessera identity is discoverable from registry annotations without unpacking.
        assert_eq!(m["annotations"]["vnd.tessera.id"], "blake3:abc123");
        assert_eq!(
            m["annotations"]["vnd.tessera.manifest_hash"],
            "blake3:def456"
        );
    }

    #[test]
    fn layer_digest_is_content_addressed_and_changes_with_the_bytes() {
        let a = artifact_manifest(b"one", "id", "mh");
        let b = artifact_manifest(b"two", "id", "mh");
        assert_ne!(a["layers"][0]["digest"], b["layers"][0]["digest"]);
        // deterministic: same bytes → same digest.
        assert_eq!(
            artifact_manifest(b"one", "id", "mh")["layers"][0]["digest"],
            a["layers"][0]["digest"]
        );
    }
}
