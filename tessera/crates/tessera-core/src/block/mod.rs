//! Storage blocks — the shape-dispatched payloads of a product.
//!
//! A product holds N blocks. Each block is one of two *shapes*, chosen by the data:
//! - [`array`] — dense N-D chunked arrays (volumes, sinograms) → zarrs backend.
//! - [`table`] — columnar tables (events, spectra, ROIs) → arrow/parquet backend.
//!
//! The core records each block as a [`BlockRef`] in the manifest and rolls its digest into
//! the product Merkle root. Payload read/write lives behind the backend features.

pub mod array;
pub mod table;

use serde::{Deserialize, Serialize};

/// Which engine/shape a block uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// Dense N-D chunked array (zarrs). Volumes, sinograms, μ-maps.
    Array,
    /// Columnar table (arrow/parquet). Events, spectra, ROIs.
    Table,
}

/// A reference to a block, as recorded in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockRef {
    pub name: String,
    pub kind: BlockKind,
    /// Per-block content digest (rolled into the product Merkle root at seal time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Shape-specific descriptor (dtype, chunks, schema, codec, ...) for self-description.
    #[serde(default)]
    pub spec: serde_json::Value,
}

/// Behaviour every block backend provides to the product builder.
pub trait Block {
    fn name(&self) -> &str;
    fn kind(&self) -> BlockKind;
    /// JSON descriptor of this block's storage spec (embedded in the manifest).
    fn spec_json(&self) -> crate::Result<serde_json::Value>;
    /// Content digest over this block's encoded spec/payload.
    fn digest(&self) -> crate::Result<String>;
    /// Build the manifest reference for this block.
    fn block_ref(&self) -> crate::Result<BlockRef> {
        Ok(BlockRef {
            name: self.name().to_string(),
            kind: self.kind(),
            digest: Some(self.digest()?),
            spec: self.spec_json()?,
        })
    }
}
