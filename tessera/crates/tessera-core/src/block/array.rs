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
    /// Named axes in storage order (fd5 convention) — e.g. `["z","y","x"]`. Length == rank.
    /// Carries the meaning of each dimension so a reader/AI never has to guess axis order.
    pub axes: Vec<String>,
    /// Shard shape; `Some` collapses thousands of chunk objects into a few shard files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shards: Option<Vec<u64>>,
    /// Compression codec; **pcodec** by default — the settled volume codec (lossless, −21% CT /
    /// −33% PET vs zstd). `zstd` remains a decades-stable fallback name. See `tessera-io::array`.
    pub codec: String,
    /// No-data / fill value (fd5 `fill_value`) for sparse or masked regions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_value: Option<serde_json::Value>,
    /// UCUM physical unit of the (rescaled) sample values, e.g. "HU", "Bq/mL", "1/cm".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Physical-unit recovery for native-int storage (CT → HU, PET → Bq/mL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rescale_slope: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rescale_intercept: Option<f64>,
}

/// Default axis names for a given rank: 3-D → `[z,y,x]`, 2-D → `[y,x]`, else `[dim0,dim1,…]`.
pub fn default_axes(rank: usize) -> Vec<String> {
    match rank {
        2 => vec!["y".into(), "x".into()],
        3 => vec!["z".into(), "y".into(), "x".into()],
        4 => vec!["t".into(), "z".into(), "y".into(), "x".into()],
        n => (0..n).map(|i| format!("dim{i}")).collect(),
    }
}

impl ArraySpec {
    /// Construct with benchmark-backed defaults: cubic 64³ chunks, **pcodec**, named axes per
    /// rank, no rescale. Axes default to `[z,y,x]` for the common 3-D volume.
    pub fn new(shape: Vec<u64>, dtype: impl Into<String>) -> Self {
        let rank = shape.len();
        ArraySpec {
            shape,
            dtype: dtype.into(),
            chunks: vec![64; rank.max(1)],
            axes: default_axes(rank),
            shards: None,
            codec: "pcodec".into(),
            fill_value: None,
            unit: None,
            rescale_slope: None,
            rescale_intercept: None,
        }
    }

    /// Record the DICOM rescale so physical units are recoverable from native ints. `unit` is
    /// the UCUM unit of the rescaled values (e.g. "HU" for CT, "Bq/mL" for PET activity).
    pub fn with_rescale(mut self, slope: f64, intercept: f64) -> Self {
        self.rescale_slope = Some(slope);
        self.rescale_intercept = Some(intercept);
        self
    }

    /// Set the UCUM physical unit of the (rescaled) values.
    pub fn with_unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// Override the default axis names (must match the array rank).
    pub fn with_axes(mut self, axes: Vec<String>) -> Self {
        self.axes = axes;
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
        if self.axes.len() != self.shape.len() {
            return Err(crate::Error::Invalid(format!(
                "axes rank {} != array rank {}",
                self.axes.len(),
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
        // A *spec-only* digest: an `ArrayBlock` carries no samples, so this hashes the canonical
        // spec — the digest for a block whose data is not yet attached. A real array product is
        // built via `tessera_io::array::array_block`, which digests the encoded payload bytes
        // (Zarr v3 + pcodec) and supplies the `BlockRef` through `ProductBuilder::add_block_ref`.
        Ok(crate::hash::digest(&serde_json::to_vec(&self.spec)?))
    }
}
