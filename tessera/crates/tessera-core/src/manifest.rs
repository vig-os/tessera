//! The manifest — the self-describing, hashable spine of a Tessera product.

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
        // Normalise to UTC so equivalent instants in different offsets share one id.
        let timestamp = crate::identity::normalize_timestamp(&timestamp.into());
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

    /// Parse a manifest, refusing one whose major spec version this reader can't handle.
    pub fn from_json(s: &str) -> crate::Result<Self> {
        let m: Manifest = serde_json::from_str(s)?;
        m.check_version()?;
        Ok(m)
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
                crate::Error::Invalid(format!("bad tessera_version: {}", self.tessera_version))
            })?;
        if major > SUPPORTED_MAJOR {
            return Err(crate::Error::Invalid(format!(
                "unsupported tessera_version {} (reader supports major <= {SUPPORTED_MAJOR})",
                self.tessera_version
            )));
        }
        Ok(())
    }
}
