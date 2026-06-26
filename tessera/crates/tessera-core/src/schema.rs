//! Product-schema registry + the fd5 field model (issues #199, #200).
//!
//! The engine is **schema-driven and domain-agnostic**: product schemas are *embedded data*
//! (these structs), not engine code. A [`ProductSchema`] declares the blocks and metadata
//! fields a product of that kind must carry; [`SchemaRegistry::validate`] checks a manifest
//! against its declared schema. Unknown products are permitted (open-world) so custom domains
//! work without forking the engine; a product that *claims* a known schema must satisfy it.
//!
//! ## The fd5 field model (carried once, in the schema — never per value)
//! Each [`FieldSpec`] pairs a short **stable id** (the storage key + rename-safe evolution
//! anchor, Iceberg-style) with a human/AI-readable **description**, a **dtype**, an optional
//! **unit** (UCUM), an optional controlled **vocabulary**, a **default**, and whether it is
//! **required**. Coded values use the fd5 [`Coded`] (`_vocabulary`/`_code`) convention.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::block::BlockKind;
use crate::manifest::Manifest;

/// A field's self-description — carried once, in the schema, so values stay lean and a reader
/// (or an AI) always has the field's meaning, unit, and dtype without external context (FAIR I1/I2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSpec {
    /// Short, stable storage id (the key). Renaming the human label never changes this — it is
    /// the rename-safe evolution anchor.
    pub id: String,
    /// Human + AI-readable description.
    pub description: String,
    /// Value dtype: a [`crate::dtype::DType`] name, or `"string"` / `"coded"` / `"json"` for metadata.
    pub dtype: String,
    /// UCUM physical unit, if any (e.g. "HU", "Bq/mL", "ps", "keV", "mm").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Controlled vocabulary the value is drawn from (e.g. "DICOM", "RadLex", "SNOMED", "UCUM").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vocabulary: Option<String>,
    /// Default value used when the field is absent (satisfies a `required` field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// Whether a valid product of the owning schema must carry this field.
    #[serde(default)]
    pub required: bool,
}

impl FieldSpec {
    /// A required, undimensioned metadata field.
    pub fn required(id: &str, description: &str, dtype: &str) -> Self {
        FieldSpec {
            id: id.into(),
            description: description.into(),
            dtype: dtype.into(),
            unit: None,
            vocabulary: None,
            default: None,
            required: true,
        }
    }

    /// An optional metadata field.
    pub fn optional(id: &str, description: &str, dtype: &str) -> Self {
        FieldSpec {
            required: false,
            ..FieldSpec::required(id, description, dtype)
        }
    }

    /// Builder: attach a UCUM unit.
    pub fn unit(mut self, unit: &str) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// Builder: attach a controlled vocabulary.
    pub fn vocabulary(mut self, vocab: &str) -> Self {
        self.vocabulary = Some(vocab.into());
        self
    }
}

/// A coded value drawn from a controlled vocabulary (fd5 `_vocabulary`/`_code`). e.g. a CT
/// modality `{ _vocabulary: "DICOM", _code: "CT", label: "Computed Tomography" }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coded {
    #[serde(rename = "_vocabulary")]
    pub vocabulary: String,
    #[serde(rename = "_code")]
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Coded {
    pub fn new(vocabulary: &str, code: &str) -> Self {
        Coded {
            vocabulary: vocabulary.into(),
            code: code.into(),
            label: None,
        }
    }
}

/// A block a schema expects: a logical `role`, the required shape (`kind`, `None` = either), and
/// how many are required (`0` = optional).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockRequirement {
    pub role: String,
    /// Required block shape; `None` accepts either array or table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<BlockKind>,
    /// Minimum number of matching blocks (0 = optional).
    pub min_count: u32,
    pub description: String,
}

impl BlockRequirement {
    fn matches(&self, b: &crate::block::BlockRef) -> bool {
        self.kind.is_none_or(|k| k == b.kind)
    }
}

/// One product schema — the contract for a `product` kind. Versioned for additive evolution
/// (adding optional fields/blocks bumps the minor; ids never change).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductSchema {
    pub product: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub fields: Vec<FieldSpec>,
    #[serde(default)]
    pub blocks: Vec<BlockRequirement>,
}

impl ProductSchema {
    /// Validate a manifest against this schema: every required block role must have enough
    /// matching blocks, and every required field must be present in `metadata` (or carry a
    /// default). Returns a typed [`crate::Error::Invalid`] naming the first violation.
    pub fn validate(&self, m: &Manifest) -> crate::Result<()> {
        for req in &self.blocks {
            if req.min_count == 0 {
                continue;
            }
            let count = m.blocks.iter().filter(|b| req.matches(b)).count();
            if count < req.min_count as usize {
                return Err(crate::Error::Invalid(format!(
                    "schema '{}' requires {}× block role '{}' ({}), found {count}",
                    self.product,
                    req.min_count,
                    req.role,
                    req.kind
                        .map(|k| format!("{k:?}"))
                        .unwrap_or_else(|| "any".into()),
                )));
            }
        }
        for f in &self.fields {
            if f.required && f.default.is_none() && !m.metadata.contains_key(&f.id) {
                return Err(crate::Error::Invalid(format!(
                    "schema '{}' requires metadata field '{}' ({})",
                    self.product, f.id, f.description
                )));
            }
        }
        Ok(())
    }
}

/// The registry of built-in product schemas. Domain-agnostic: lookups for unknown products
/// return `None` and validation is then permissive (custom/extension products are allowed).
#[derive(Debug, Clone)]
pub struct SchemaRegistry {
    schemas: BTreeMap<String, ProductSchema>,
}

impl SchemaRegistry {
    /// The built-in Tessera product schemas (medical imaging + simulation lineage).
    pub fn builtin() -> Self {
        let mut schemas = BTreeMap::new();
        for s in builtin_schemas() {
            schemas.insert(s.product.clone(), s);
        }
        SchemaRegistry { schemas }
    }

    pub fn get(&self, product: &str) -> Option<&ProductSchema> {
        self.schemas.get(product)
    }

    pub fn products(&self) -> impl Iterator<Item = &str> {
        self.schemas.keys().map(String::as_str)
    }

    /// Validate a manifest against its declared product schema. Unknown product → `Ok` (a
    /// custom/extension product is permitted). A known product must satisfy its schema.
    pub fn validate(&self, m: &Manifest) -> crate::Result<()> {
        match self.get(&m.product) {
            Some(schema) => schema.validate(m),
            None => Ok(()),
        }
    }
}

impl Default for SchemaRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

/// Convenience: one required block of a given role/kind.
fn one(role: &str, kind: Option<BlockKind>, description: &str) -> BlockRequirement {
    BlockRequirement {
        role: role.into(),
        kind,
        min_count: 1,
        description: description.into(),
    }
}

fn schema(product: &str, version: &str, description: &str) -> ProductSchema {
    ProductSchema {
        product: product.into(),
        version: version.into(),
        description: description.into(),
        fields: Vec::new(),
        blocks: Vec::new(),
    }
}

/// The built-in product schemas. Each declares its required block(s) + key fields; all are
/// versioned `1.0` and evolve additively (new optional fields/blocks only). See ROADMAP P1 +
/// ADR-0029 §5 (the multi-dimensional `dynamic_pet`/`diffusion_mri`/`multicontrast_mri` set).
fn builtin_schemas() -> Vec<ProductSchema> {
    use BlockKind::{Array, Table};
    vec![
        ProductSchema {
            fields: vec![
                FieldSpec::required("modality", "Imaging modality", "coded").vocabulary("DICOM"),
                FieldSpec::optional("rescale_slope", "Native→physical slope", "float64"),
                FieldSpec::optional("rescale_intercept", "Native→physical intercept", "float64"),
            ],
            blocks: vec![one(
                "volume",
                Some(Array),
                "Reconstructed image volume (z,y,x), native dtype",
            )],
            ..schema(
                "recon",
                "1.0",
                "A reconstructed image volume (CT/PET/μ-map).",
            )
        },
        ProductSchema {
            fields: vec![FieldSpec::required(
                "coincidence_mode",
                "Acquisition mode (singles / prompt-coincidence / extended-coincidence)",
                "string",
            )],
            blocks: vec![one(
                "events",
                Some(Table),
                "Per-event columnar table (timestamps, energies, positions)",
            )],
            ..schema(
                "listmode",
                "1.0",
                "Raw per-event list-mode acquisition data.",
            )
        },
        ProductSchema {
            blocks: vec![one(
                "sinogram",
                Some(Array),
                "Projection / sinogram array (angle, radial, plane)",
            )],
            ..schema("sinogram", "1.0", "Projection-space (sinogram) data.")
        },
        ProductSchema {
            fields: vec![FieldSpec::optional(
                "domain",
                "Histogram domain (energy / lifetime / time-of-flight)",
                "string",
            )],
            blocks: vec![one(
                "spectrum",
                None,
                "Histogram of an energy/lifetime/TOF quantity",
            )],
            ..schema(
                "spectrum",
                "1.0",
                "An energy / positronium-lifetime / TOF histogram.",
            )
        },
        ProductSchema {
            blocks: vec![one(
                "roi",
                None, // representation by nature (ADR-0029 §4): raster label N-D **array** OR a
                "Region(s) of interest — a raster label array, or a parametric / contour / stats table",
            )],
            ..schema(
                "roi",
                "1.0",
                "Regions of interest over an image product (representation chosen by the ROI's nature).",
            )
        },
        ProductSchema {
            blocks: vec![one(
                "transform",
                None,
                "Spatial transform: affine (table) or deformation field (array)",
            )],
            ..schema(
                "transform",
                "1.0",
                "A spatial transform / registration result.",
            )
        },
        ProductSchema {
            blocks: vec![one(
                "calibration",
                None,
                "Calibration coefficients / lookup (normalization, attenuation, dead-time)",
            )],
            ..schema("calibration", "1.0", "Scanner calibration data.")
        },
        ProductSchema {
            fields: vec![
                FieldSpec::optional(
                    "simulator",
                    "Simulation toolkit (e.g. GATE/Geant4)",
                    "string",
                ),
                FieldSpec::optional("seed", "RNG seed for reproducibility", "int64"),
            ],
            blocks: vec![one(
                "output",
                None,
                "Simulation output (hits table or scored volume)",
            )],
            ..schema("sim", "1.0", "Monte-Carlo simulation output.")
        },
        ProductSchema {
            fields: vec![
                FieldSpec::optional("vendor", "Device vendor", "string"),
                FieldSpec::optional("model", "Device model", "string"),
            ],
            blocks: vec![one(
                "raw",
                None,
                "Raw vendor/device payload (normalized at ingest, preserved verbatim)",
            )],
            ..schema(
                "device_data",
                "1.0",
                "Raw device data from an existing system (GE/Siemens/…).",
            )
        },
        // ── ADR-0029 §5 multi-dimensional acquisition schemas (additive) ──
        ProductSchema {
            fields: vec![
                FieldSpec::required("modality", "Imaging modality", "coded").vocabulary("DICOM")
            ],
            blocks: vec![
                one(
                    "volume",
                    Some(Array),
                    "4-D dynamic volume (t,z,y,x), native dtype — one N-D array, the t_c lever sets per-frame↔TAC locality",
                ),
                one(
                    "frame_timing",
                    Some(Table),
                    "Per-frame start + duration (s, monotonic) and decay-correction reference",
                ),
            ],
            ..schema(
                "dynamic_pet",
                "1.0",
                "A dynamic (4-D) PET acquisition: one time-series volume + a frame-timing table.",
            )
        },
        ProductSchema {
            fields: vec![
                FieldSpec::required("modality", "Imaging modality", "coded").vocabulary("DICOM")
            ],
            blocks: vec![
                one(
                    "volume",
                    Some(Array),
                    "4-D diffusion volume (dir,z,y,x), native dtype",
                ),
                one(
                    "gradients",
                    Some(Table),
                    "Per-direction b-value (s/mm²) + unit b-vector (bval/bvec)",
                ),
            ],
            ..schema(
                "diffusion_mri",
                "1.0",
                "A diffusion MRI acquisition: per-direction volumes + b-values/vectors.",
            )
        },
        ProductSchema {
            fields: vec![
                FieldSpec::required("modality", "Imaging modality", "coded").vocabulary("DICOM")
            ],
            blocks: vec![one(
                "volume",
                Some(Array),
                "One image volume per contrast (≥1 — heterogeneous matrix/voxel, e.g. T1/T2/FLAIR)",
            )],
            ..schema(
                "multicontrast_mri",
                "1.0",
                "A multi-contrast MRI study: N separately-acquired contrast volumes (model B).",
            )
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::array::{ArrayBlock, ArraySpec};
    use crate::ProductBuilder;

    #[test]
    fn registry_has_all_builtins() {
        let r = SchemaRegistry::builtin();
        for p in [
            "recon",
            "listmode",
            "sinogram",
            "spectrum",
            "roi",
            "transform",
            "calibration",
            "sim",
            "device_data",
            // ADR-0029 §5 multi-dimensional acquisitions
            "dynamic_pet",
            "diffusion_mri",
            "multicontrast_mri",
        ] {
            assert!(r.get(p).is_some(), "missing built-in schema '{p}'");
        }
        assert_eq!(r.products().count(), 12);
    }

    #[test]
    fn roi_accepts_representation_by_nature_array_or_table() {
        // ADR-0029 §4: an ROI's representation is chosen by its nature — irregular → a raster label
        // **array**; primitive/contour/stats → a **table**. The `roi` schema accepts either.
        use crate::block::array::{ArrayBlock, ArraySpec};
        use crate::block::table::{Column, TableBlock, TableSpec};
        let r = SchemaRegistry::builtin();
        // irregular ROI → a raster uint16 label-map array
        let mask = ArrayBlock::new("mask", ArraySpec::new(vec![8, 8, 8], "uint16"));
        let mut b = ProductBuilder::new("roi", "roi-a", "tumour mask", "2024-01-01T00:00:00Z");
        b.add_block(&mask).unwrap();
        r.validate(&b.seal().unwrap()).unwrap();
        // primitive ROI → a parametric table (type + params)
        let params = TableBlock::new(
            "rois",
            TableSpec {
                columns: vec![Column {
                    name: "radius".into(),
                    dtype: "f4".into(),
                    codec: None,
                }],
                rows: 1,
                row_index: None,
            },
        );
        let mut b = ProductBuilder::new("roi", "roi-b", "sphere ROI", "2024-01-01T00:00:00Z");
        b.add_block(&params).unwrap();
        r.validate(&b.seal().unwrap()).unwrap();
    }

    #[test]
    fn registration_is_a_transform_product_with_new_frame_and_provenance() {
        // ADR-0030 §5: a registration is a `transform` product whose array carries the **new** (target)
        // world_frame, with a provenance edge recording source→target + the recipe. No new mechanism —
        // it composes the `transform` schema + `world_frame` + the `sources` DAG.
        use crate::block::array::{ArrayBlock, ArraySpec, WorldFrame};
        use crate::provenance::Source;
        let r = SchemaRegistry::builtin();
        let mut spec = ArraySpec::new(vec![8, 8, 8], "int16");
        spec.world_frame = Some(WorldFrame {
            affine: [
                1.0, 0.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, 0.0, //
                0.0, 0.0, 1.0, 0.0,
            ],
            convention: "LPS".into(),
            unit: "mm".into(),
            space: "atlas:mni152".into(), // the TARGET frame the registration resampled into
        });
        let vol = ArrayBlock::new("registered", spec);
        let mut b = ProductBuilder::new(
            "transform",
            "reg-01",
            "rigid reg → MNI",
            "2024-01-01T00:00:00Z",
        );
        b.add_block(&vol).unwrap();
        b.add_source(Source::new(
            "registered_from",
            "blake3:source-manifest-hash",
        ));
        b.with_field(
            "recipe",
            serde_json::json!({"kind": "rigid", "to_space": "atlas:mni152"}),
        );
        let m = b.seal().unwrap();

        // valid `transform`; carries the target frame + the typed provenance edge; verifies.
        r.validate(&m).unwrap();
        assert_eq!(m.product, "transform");
        assert_eq!(m.sources.len(), 1);
        assert_eq!(m.sources[0].role, "registered_from");
        let stored: ArraySpec = serde_json::from_value(m.blocks[0].spec.clone()).unwrap();
        assert_eq!(stored.world_frame.unwrap().space, "atlas:mni152");
        assert!(m.verify().is_ok());
    }

    #[test]
    fn dynamic_pet_requires_volume_and_frame_timing() {
        use crate::block::array::{ArrayBlock, ArraySpec};
        use crate::block::table::{Column, TableBlock, TableSpec};
        let r = SchemaRegistry::builtin();
        let vol = ArrayBlock::new("vol", ArraySpec::new(vec![4, 8, 8, 8], "int16"));
        let timing = TableBlock::new(
            "frame_timing",
            TableSpec {
                columns: vec![Column {
                    name: "start_s".into(),
                    dtype: "f8".into(),
                    codec: None,
                }],
                rows: 4,
                row_index: None,
            },
        );
        // volume alone → rejected (frame_timing table required)
        let mut b = ProductBuilder::new("dynamic_pet", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "PT"}),
        );
        let m = b.seal().unwrap();
        assert!(r.validate(&m).is_err());
        // volume + frame_timing → valid
        let mut b = ProductBuilder::new("dynamic_pet", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.add_block(&timing).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "PT"}),
        );
        let m = b.seal().unwrap();
        r.validate(&m).unwrap();
    }

    #[test]
    fn recon_validates_with_volume_and_required_field() {
        let r = SchemaRegistry::builtin();
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![64, 64, 64], "int16"));
        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "CT"}),
        );
        let m = b.seal().unwrap();
        r.validate(&m).unwrap();
    }

    #[test]
    fn recon_without_volume_block_is_rejected() {
        let r = SchemaRegistry::builtin();
        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        b.with_field("modality", serde_json::json!("CT"));
        // a recon needs a table? no — it needs an *array* volume; add a table instead → fail
        let m = b.seal().unwrap();
        assert!(r.validate(&m).is_err(), "recon with no volume must fail");
    }

    #[test]
    fn recon_missing_required_field_is_rejected() {
        let r = SchemaRegistry::builtin();
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![64, 64, 64], "int16"));
        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        let m = b.seal().unwrap(); // no `modality`
        match r.validate(&m) {
            Err(crate::Error::Invalid(msg)) => assert!(msg.contains("modality"), "{msg}"),
            other => panic!("expected missing-field error, got {other:?}"),
        }
    }

    #[test]
    fn unknown_product_is_permitted_open_world() {
        let r = SchemaRegistry::builtin();
        let m = ProductBuilder::new("my_custom_domain", "x", "d", "2024-01-01T00:00:00Z")
            .seal()
            .unwrap();
        r.validate(&m).unwrap();
    }
}
