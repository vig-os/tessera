//! Array block — dense N-D chunked, sharded storage. zarrs backend (feature `array-zarr`).
//!
//! Defaults encode the benchmark findings (fd5 #192/#194): **native dtype** (no float32
//! upcast for CT/PET), **cubic chunks** (fast orthogonal/ROI access), **zstd** codec, and
//! optional **sharding** (cloud range-reads without the unsharded many-files problem).

use serde::{Deserialize, Serialize};

use super::{Block, BlockKind};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Spatial referencing (ADR-0030): the voxel→world affine + named frame. Optional — absent means
    /// the array is in **index space** (feature-by-presence, ADR-0029). This is the `affine_nd`
    /// instance of the general `(transform, unit, frame)` descriptor (ADR-0032).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_frame: Option<WorldFrame>,
}

/// A voxel→world spatial frame (ADR-0030). The affine maps an index vector **in declared-axis order**
/// to world coordinates; spacing/orientation/origin live **only** here (spacing is derived — §2). The
/// world handedness is **LPS canonical** (§6); RAS sources are normalised at the door (ADR-0025).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldFrame {
    /// 3×4 **row-major** voxel→world affine `[R | t]` (the implicit last row is `[0,0,0,1]`): the first
    /// four entries are row 0 `[r00 r01 r02 t0]`, then row 1, then row 2.
    pub affine: [f64; 12],
    /// World handedness convention — `"LPS"` canonical (ADR-0030 §6). Stored so an affine is never
    /// ambiguous and the source frame is reconstructible.
    pub convention: String,
    /// World-coordinate unit (UCUM), e.g. `"mm"`.
    pub unit: String,
    /// Named target frame: `"patient"` | `"scanner"` | `"aligned"` | `"atlas:<id>"`.
    pub space: String,
}

impl WorldFrame {
    /// Per-axis voxel spacing = the column norms of the 3×3 rotation+scale block. **Derived** from the
    /// affine, never stored separately (ADR-0030 §2 — single source of truth for geometry).
    pub fn spacing(&self) -> [f64; 3] {
        let a = &self.affine;
        let col = |c: usize| (a[c].powi(2) + a[4 + c].powi(2) + a[8 + c].powi(2)).sqrt();
        [col(0), col(1), col(2)]
    }

    /// A frame is valid iff every axis has a non-zero spacing (non-degenerate affine) — the ADR-0030
    /// "affine non-degenerate" gate.
    pub fn is_nondegenerate(&self) -> bool {
        self.spacing().iter().all(|&s| s > 0.0)
    }
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
            world_frame: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_frame_spacing_is_derived_from_affine_columns() {
        // diag(2,3,4) scale + translation (10,20,30), LPS mm.
        let wf = WorldFrame {
            affine: [
                2.0, 0.0, 0.0, 10.0, //
                0.0, 3.0, 0.0, 20.0, //
                0.0, 0.0, 4.0, 30.0,
            ],
            convention: "LPS".into(),
            unit: "mm".into(),
            space: "patient".into(),
        };
        assert_eq!(wf.spacing(), [2.0, 3.0, 4.0]); // column norms = per-axis spacing (ADR-0030 §2)
        assert!(wf.is_nondegenerate());
        // zero out column 0 → degenerate (zero spacing on an axis)
        let mut bad = wf.clone();
        bad.affine[0] = 0.0;
        assert_eq!(bad.spacing()[0], 0.0);
        assert!(!bad.is_nondegenerate());
    }

    #[test]
    fn array_spec_world_frame_is_additive_and_optional() {
        let mut spec = ArraySpec::new(vec![64, 64, 64], "int16");
        // default = index space (feature-by-presence) and the key is omitted from JSON, so existing
        // arrays serialize byte-identically (no corpus regen).
        assert!(spec.world_frame.is_none());
        let json = serde_json::to_value(&spec).unwrap();
        assert!(json.get("world_frame").is_none());
        // attach a frame and round-trip through JSON.
        spec.world_frame = Some(WorldFrame {
            affine: [
                1.0, 0.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, 0.0, //
                0.0, 0.0, 1.0, 0.0,
            ],
            convention: "LPS".into(),
            unit: "mm".into(),
            space: "patient".into(),
        });
        let back: ArraySpec = serde_json::from_value(serde_json::to_value(&spec).unwrap()).unwrap();
        assert_eq!(back, spec);
    }
}
