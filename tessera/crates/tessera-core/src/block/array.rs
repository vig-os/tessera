//! Array block — dense N-D chunked, sharded storage. zarrs backend (feature `array-zarr`).
//!
//! Defaults encode the benchmark findings (fd5 #192/#194): **native dtype** (no float32
//! upcast for CT/PET), **cubic chunks** (fast orthogonal/ROI access), **zstd** codec, and
//! optional **sharding** (cloud range-reads without the unsharded many-files problem).

use serde::{Deserialize, Serialize};

use super::{Block, BlockKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArraySpec {
    pub shape: Vec<u64>,
    /// Native dtype, e.g. "int16". Do NOT upcast CT/PET to float32 (2.6× bigger, no gain).
    pub dtype: String,
    /// Cubic chunks by default — 18–24× faster orthogonal-plane access than slice chunks.
    pub chunks: Vec<u64>,
    /// Shard shape; `Some` collapses thousands of chunk objects into a few shard files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shards: Option<Vec<u64>>,
    /// Compression codec; zstd by default (smaller AND faster than gzip).
    pub codec: String,
    /// Physical-unit recovery for native-int storage (CT → HU, PET → Bq/mL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rescale_slope: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rescale_intercept: Option<f64>,
}

impl ArraySpec {
    /// Construct with benchmark-backed defaults: cubic 64³ chunks, zstd, no rescale.
    pub fn new(shape: Vec<u64>, dtype: impl Into<String>) -> Self {
        ArraySpec {
            shape,
            dtype: dtype.into(),
            chunks: vec![64, 64, 64],
            shards: None,
            codec: "zstd".into(),
            rescale_slope: None,
            rescale_intercept: None,
        }
    }

    /// Record the DICOM rescale so physical units are recoverable from native ints.
    pub fn with_rescale(mut self, slope: f64, intercept: f64) -> Self {
        self.rescale_slope = Some(slope);
        self.rescale_intercept = Some(intercept);
        self
    }

    /// Validate the spec: dtype must be in the supported allowlist (int16 is *recommended*
    /// for CT/PET, not required — any [`crate::dtype::DType`] is allowed), and the chunk
    /// grid must match the array rank.
    pub fn validate(&self) -> crate::Result<()> {
        if !crate::dtype::DType::is_supported(&self.dtype) {
            return Err(crate::Error::Invalid(format!(
                "unsupported array dtype '{}' (allowed: {:?})",
                self.dtype,
                crate::dtype::DType::ALL.map(|d| d.as_str())
            )));
        }
        if self.chunks.len() != self.shape.len() {
            return Err(crate::Error::Invalid(format!(
                "chunk rank {} != array rank {}",
                self.chunks.len(),
                self.shape.len()
            )));
        }
        Ok(())
    }
}

pub struct ArrayBlock {
    pub name: String,
    pub spec: ArraySpec,
}

impl ArrayBlock {
    pub fn new(name: impl Into<String>, spec: ArraySpec) -> Self {
        ArrayBlock {
            name: name.into(),
            spec,
        }
    }
}

impl Block for ArrayBlock {
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> BlockKind {
        BlockKind::Array
    }
    fn spec_json(&self) -> crate::Result<serde_json::Value> {
        Ok(serde_json::to_value(&self.spec)?)
    }
    fn digest(&self) -> crate::Result<String> {
        self.spec.validate()?;
        // Spike: digest the spec. Real impl digests the encoded zarrs shards (Merkle of chunks).
        Ok(crate::hash::digest(&serde_json::to_vec(&self.spec)?))
    }
}

#[cfg(feature = "array-zarr")]
impl ArrayBlock {
    /// Write the array payload via zarrs (sharded, cubic-chunked, zstd). Not yet implemented.
    pub fn write_zarr(&self, _store_path: &std::path::Path) -> crate::Result<()> {
        Err(crate::Error::Unimplemented(
            "ArrayBlock::write_zarr (zarrs backend)",
        ))
    }
}
