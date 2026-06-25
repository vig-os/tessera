//! The manifest — the self-describing, hashable spine of a Tessera product (see ADR-0020).
//!
//! Three hashes, three jobs:
//! - [`id`](Manifest::id) — *logical identity*: `blake3` over canonical-JSON of [`id_inputs`].
//!   Rename- and re-encode-stable; survives re-ingest of the same logical product.
//! - [`content_hash`](Manifest::content_hash) — *data fingerprint*: Merkle root over the ordered
//!   block digests. Changes iff payload bytes change.
//! - [`manifest_hash`](Manifest::manifest_hash) — *the seal*: `blake3` over canonical-JSON of the
//!   whole manifest (this field excluded). Covers metadata + sources + block refs (which carry the
//!   block digests), so it transitively commits to all payloads — tamper anything, it changes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::block::BlockRef;
use crate::provenance::Source;

/// Format/spec version this build writes.
pub const TESSERA_VERSION: &str = "0.0.0";

/// Highest major spec version this reader understands. A manifest with a higher major
/// version is refused rather than silently mis-read.
pub const SUPPORTED_MAJOR: u64 = 0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Format/spec version.
    pub tessera_version: String,
    /// Stable, content-derived logical identity (see [`crate::identity`]).
    pub id: String,
    /// Declared identity inputs (key→value) that `id` is hashed over — recorded so identity is
    /// transparent and independently verifiable. Default keys: `product`/`name`/`timestamp`.
    pub id_inputs: BTreeMap<String, String>,
    /// Product schema name, e.g. "recon", "listmode", "spectrum".
    pub product: String,
    pub name: String,
    pub description: String,
    /// RFC 3339 timestamp, normalized to UTC.
    pub timestamp: String,
    /// Embedded JSON Schema for the product (opaque to the core).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    /// Typed storage blocks composing this product (order is significant for the Merkle root).
    #[serde(default)]
    pub blocks: Vec<BlockRef>,
    /// Provenance DAG: where this product came from.
    #[serde(default)]
    pub sources: Vec<Source>,
    /// Optional study/grouping id (fd5 `study`) — ties together the products of one exam
    /// (a study's CT + PET + listmode share a `study`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub study: Option<String>,
    /// Values for the product schema's declared metadata fields (fd5 field model), keyed by each
    /// field's stable `id`. Schema-governed — distinct from `extra` (non-standard).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
    /// Extension namespace (fd5 `extra/`) for non-standard / vendor metadata. Preserved through
    /// round-trips and covered by `manifest_hash`, but never schema-validated and never allowed
    /// to collide with core keys (it is a nested object).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
    /// Merkle root over block digests; `Some` once sealed, `None` while building.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// The seal: `blake3` over canonical-JSON of this manifest with `manifest_hash` itself
    /// omitted. `Some` once sealed, `None` while building.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
}

impl Manifest {
    pub fn new(
        product: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Self {
        let product = product.into();
        let name = name.into();
        let description = description.into();
        // Normalise to UTC so equivalent instants in different offsets share one id.
        let timestamp = crate::identity::normalize_timestamp(&timestamp.into());
        let id_inputs = crate::identity::default_id_inputs(&product, &name, &timestamp);
        // Infallible: a string→string map always canonicalizes (see compute_id).
        let id =
            crate::identity::compute_id(&id_inputs).expect("string id_inputs always canonicalize");
        Manifest {
            tessera_version: TESSERA_VERSION.to_string(),
            id,
            id_inputs,
            product,
            name,
            description,
            timestamp,
            schema: None,
            blocks: Vec::new(),
            sources: Vec::new(),
            study: None,
            metadata: BTreeMap::new(),
            extra: BTreeMap::new(),
            content_hash: None,
            manifest_hash: None,
        }
    }

    /// True once the product has been sealed (carries a manifest hash).
    pub fn is_sealed(&self) -> bool {
        self.manifest_hash.is_some()
    }

    /// Pretty JSON, for humans / display. NOT the hashed form — see [`canonical_bytes`].
    pub fn to_json(&self) -> crate::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Canonical (RFC 8785 JCS) bytes of this manifest with `manifest_hash` excluded — the
    /// exact bytes [`manifest_hash`](Self::manifest_hash) is computed over.
    pub fn canonical_bytes(&self) -> crate::Result<Vec<u8>> {
        let mut bare = self.clone();
        bare.manifest_hash = None;
        crate::canonical::to_bytes(&bare)
    }

    /// Recompute the logical `id` from `id_inputs`.
    pub fn recompute_id(&self) -> crate::Result<String> {
        crate::identity::compute_id(&self.id_inputs)
    }

    /// Recompute the content Merkle root from the block digests. Errors if any block ref is
    /// missing its digest.
    pub fn recompute_content_hash(&self) -> crate::Result<String> {
        let mut digests = Vec::with_capacity(self.blocks.len());
        for b in &self.blocks {
            match &b.digest {
                Some(d) => digests.push(d.clone()),
                None => return Err(crate::Error::MissingDigest(b.name.clone())),
            }
        }
        Ok(crate::hash::merkle_root(&digests))
    }

    /// Recompute the seal (`manifest_hash`) over the canonical bytes.
    pub fn compute_manifest_hash(&self) -> crate::Result<String> {
        Ok(crate::hash::digest(&self.canonical_bytes()?))
    }

    /// Parse a manifest, refusing one whose major spec version this reader can't handle.
    /// Does not verify hashes — use [`from_json_verified`](Self::from_json_verified) for that.
    pub fn from_json(s: &str) -> crate::Result<Self> {
        let m: Manifest = serde_json::from_str(s)?;
        m.check_version()?;
        Ok(m)
    }

    /// Parse + version-check + full integrity [`verify`](Self::verify).
    pub fn from_json_verified(s: &str) -> crate::Result<Self> {
        let m = Self::from_json(s)?;
        m.verify()?;
        Ok(m)
    }

    /// Verify all three hashes against their recomputed values (and the spec version). Any
    /// mismatch is a typed [`crate::Error::Integrity`] naming the field. A building (unsealed)
    /// manifest verifies its `id` only; a sealed one verifies all three.
    pub fn verify(&self) -> crate::Result<()> {
        self.check_version()?;
        check("id", &self.id, &self.recompute_id()?)?;
        if let Some(ch) = &self.content_hash {
            check("content_hash", ch, &self.recompute_content_hash()?)?;
        }
        if let Some(mh) = &self.manifest_hash {
            check("manifest_hash", mh, &self.compute_manifest_hash()?)?;
        }
        Ok(())
    }

    /// Error if `tessera_version`'s major exceeds [`SUPPORTED_MAJOR`] (forward-incompat),
    /// or if it is unparseable. Never panics.
    pub fn check_version(&self) -> crate::Result<()> {
        let major = self
            .tessera_version
            .split('.')
            .next()
            .and_then(|x| x.parse::<u64>().ok())
            .ok_or_else(|| {
                crate::Error::UnsupportedVersion(format!(
                    "unparseable tessera_version: {}",
                    self.tessera_version
                ))
            })?;
        if major > SUPPORTED_MAJOR {
            return Err(crate::Error::UnsupportedVersion(format!(
                "{} (reader supports major <= {SUPPORTED_MAJOR})",
                self.tessera_version
            )));
        }
        Ok(())
    }
}

/// Compare an expected vs recomputed hash, raising a typed integrity error on mismatch.
fn check(what: &'static str, expected: &str, actual: &str) -> crate::Result<()> {
    if expected != actual {
        return Err(crate::Error::Integrity {
            what,
            expected: expected.to_string(),
            actual: actual.to_string(),
        });
    }
    Ok(())
}
