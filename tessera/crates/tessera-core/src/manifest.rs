//! The manifest — the self-describing, hashable spine of a Tessera product.

use serde::{Deserialize, Serialize};

use crate::block::BlockRef;
use crate::provenance::Source;

/// Format/spec version this build writes.
pub const TESSERA_VERSION: &str = "0.0.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Format/spec version.
    pub tessera_version: String,
    /// Stable, content-derived identity (see [`crate::identity::compute_id`]).
    pub id: String,
    /// Product schema name, e.g. "recon", "listmode", "spectrum".
    pub product: String,
    pub name: String,
    pub description: String,
    /// ISO 8601 timestamp with explicit timezone.
    pub timestamp: String,
    /// Embedded JSON Schema for the product (opaque to the core).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    /// Typed storage blocks composing this product (order is significant for the hash).
    #[serde(default)]
    pub blocks: Vec<BlockRef>,
    /// Provenance DAG: where this product came from.
    #[serde(default)]
    pub sources: Vec<Source>,
    /// Merkle root over block digests; `Some` once sealed, `None` while building.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
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
        let timestamp = timestamp.into();
        let id = crate::identity::compute_id(&[product.as_str(), name.as_str(), timestamp.as_str()]);
        Manifest {
            tessera_version: TESSERA_VERSION.to_string(),
            id,
            product,
            name,
            description,
            timestamp,
            schema: None,
            blocks: Vec::new(),
            sources: Vec::new(),
            content_hash: None,
        }
    }

    /// True once the product has been sealed with a content hash.
    pub fn is_sealed(&self) -> bool {
        self.content_hash.is_some()
    }

    pub fn to_json(&self) -> crate::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn from_json(s: &str) -> crate::Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}
