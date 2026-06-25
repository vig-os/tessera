//! Table block — columnar storage. arrow/parquet backend (feature `table-arrow`).
//!
//! Listmode events, spectra, ROIs. Columnar (never row-major compound — see fd5 #193: a
//! single-column projection on compound costs a full-table read). Per-column codecs, and an
//! optional secondary index for fast random per-event `take` (Lance-style).

use serde::{Deserialize, Serialize};

use super::{Block, BlockKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    /// Arrow-ish dtype string, e.g. "i2", "u4", "f4".
    pub dtype: String,
    /// Per-column codec — columnar layout lets each column compress optimally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSpec {
    pub columns: Vec<Column>,
    pub rows: u64,
    /// Optional secondary index column enabling O(1)-ish random row `take`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_index: Option<String>,
}

pub struct TableBlock {
    pub name: String,
    pub spec: TableSpec,
}

impl TableBlock {
    pub fn new(name: impl Into<String>, spec: TableSpec) -> Self {
        TableBlock { name: name.into(), spec }
    }
}

impl Block for TableBlock {
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> BlockKind {
        BlockKind::Table
    }
    fn spec_json(&self) -> crate::Result<serde_json::Value> {
        Ok(serde_json::to_value(&self.spec)?)
    }
    fn digest(&self) -> crate::Result<String> {
        // Spike: digest the spec. Real impl digests the encoded column chunks.
        Ok(crate::hash::digest(&serde_json::to_vec(&self.spec)?))
    }
}

#[cfg(feature = "table-arrow")]
impl TableBlock {
    /// Write the columnar payload via arrow/parquet. Not yet implemented.
    pub fn write_parquet(&self, _path: &std::path::Path) -> crate::Result<()> {
        Err(crate::Error::Unimplemented("TableBlock::write_parquet (arrow backend)"))
    }
}
