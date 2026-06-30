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

/// Schema-driven **sensitivity tier** for a metadata field (ADR-0040 §1 — the spike that
/// teaches the engine to *reason* about PHI without yet redacting/encrypting). One of four,
/// in increasing identifiability:
/// - [`Public`](Sensitivity::Public) — safe in clear (e.g. modality vocab, calibration
///   coefficients, scan geometry); ships unchanged.
/// - [`Coded`](Sensitivity::Coded) — a controlled-vocab code, intrinsically non-identifying but
///   carrying clinical meaning (e.g. DICOM `Modality` = `CT`).
/// - [`Sensitive`](Sensitivity::Sensitive) — clinical content that is not directly identifying
///   on its own but is access-controlled (e.g. impression text); the future field-redactor's
///   default-keep-in-clear tier.
/// - [`Identifying`](Sensitivity::Identifying) — **direct PHI** (the seed list is DICOM
///   PS3.15's confidentiality profile — Patient Name, Patient ID, MRN, Birth Date, …,
///   plus UIDs that bind to the patient/study). The redactor / field-encryption phases
///   (deferred) operate on the fields the schema marks at this tier.
///
/// Pure data (`Copy` + serde) — the wasm-targeted `tessera-core` stays host-free; no PHI logic
/// lives in this enum, only the classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    /// Safe in clear — no clinical or identifying content.
    #[default]
    Public,
    /// A controlled-vocabulary code (e.g. DICOM `Modality`). Not identifying.
    Coded,
    /// Clinical content, not directly identifying on its own (access-controlled but not a name).
    Sensitive,
    /// Direct PHI — DICOM PS3.15 confidentiality-profile fields (Patient Name/ID/MRN, DOB,
    /// linking UIDs). The redact / field-encryption phases (deferred) operate on this tier.
    Identifying,
}

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
    /// Whether a valid product of the owning schema **must** carry this field — absence is a hard
    /// **block** at ingest ([`SchemaRegistry::validate`]).
    #[serde(default)]
    pub required: bool,
    /// Field severity below `required`: a **recommended** field's absence is a non-fatal **warn**
    /// ([`SchemaRegistry::missing_recommended`]) — FAIR-completeness nudge, never a block. Mutually
    /// meaningful with `required` (a `required` field is implicitly more than recommended; this flags
    /// the warn tier). Composes through `imaging_base` like every other field.
    #[serde(default)]
    pub recommended: bool,
    /// Schema-driven PHI classification (ADR-0040 §1). The engine uses this tier to *reason* about
    /// identifiability — e.g. the ingest warn surfaces an `identifying` field present in
    /// metadata in the clear, the hook the future field-encryption / redact phases replace.
    /// `#[serde(default)]` keeps existing on-disk schemas back-compat (absent ⇒ `Public`).
    #[serde(default)]
    pub sensitivity: Sensitivity,
}

impl FieldSpec {
    /// A required, undimensioned metadata field (absent ⇒ ingest **blocks**). Defaults to
    /// [`Sensitivity::Public`]; chain [`Self::with_sensitivity`] to classify PHI.
    pub fn required(id: &str, description: &str, dtype: &str) -> Self {
        FieldSpec {
            id: id.into(),
            description: description.into(),
            dtype: dtype.into(),
            unit: None,
            vocabulary: None,
            default: None,
            required: true,
            recommended: false,
            sensitivity: Sensitivity::Public,
        }
    }

    /// An optional metadata field (absent ⇒ silent). Defaults to [`Sensitivity::Public`].
    pub fn optional(id: &str, description: &str, dtype: &str) -> Self {
        FieldSpec {
            required: false,
            ..FieldSpec::required(id, description, dtype)
        }
    }

    /// A **recommended** metadata field: absent ⇒ a non-fatal warn (FAIR nudge), never a block. The
    /// middle tier between `required` (block) and `optional` (silent).
    pub fn recommended(id: &str, description: &str, dtype: &str) -> Self {
        FieldSpec {
            required: false,
            recommended: true,
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

    /// Builder: classify the field's PHI **sensitivity tier** (ADR-0040 §1). Defaults to
    /// [`Sensitivity::Public`]; mark direct PHI as [`Sensitivity::Identifying`] so the engine's
    /// schema-driven PHI reasoning (today: ingest warn; future: redact / field encryption) picks
    /// it up automatically.
    pub fn with_sensitivity(mut self, s: Sensitivity) -> Self {
        self.sensitivity = s;
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

    /// The schema's **recommended** fields (the warn tier) that this manifest does **not** carry and
    /// that have no default — the FAIR-completeness nudge the engine surfaces as a non-fatal warning.
    /// Never blocks (that's [`Self::validate`]'s job for `required`). Domain-agnostic: the policy is
    /// pure schema data, so it composes through `imaging_base` for every product alike.
    pub fn missing_recommended<'a>(&'a self, m: &Manifest) -> Vec<&'a FieldSpec> {
        self.fields
            .iter()
            .filter(|f| f.recommended && f.default.is_none() && !m.metadata.contains_key(&f.id))
            .collect()
    }

    /// All fields of this schema classified at the given sensitivity `tier` (ADR-0040 §1). The
    /// query the redact / field-encryption phases (deferred) consume to know **which fields** to
    /// touch — pure data, no PHI logic inside the core. Order matches schema declaration.
    pub fn fields_by_sensitivity(&self, tier: Sensitivity) -> Vec<&FieldSpec> {
        self.fields
            .iter()
            .filter(|f| f.sensitivity == tier)
            .collect()
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

    /// The recommended-but-absent fields for a manifest's declared schema (warn tier; see
    /// [`ProductSchema::missing_recommended`]). Empty for an unknown product (open-world).
    pub fn missing_recommended(&self, m: &Manifest) -> Vec<&FieldSpec> {
        self.get(&m.product)
            .map(|s| s.missing_recommended(m))
            .unwrap_or_default()
    }

    /// The fields of `m`'s declared product schema at the given sensitivity `tier` (ADR-0040 §1)
    /// — the schema-driven query the redact / field-encryption phases (deferred) use to know
    /// which fields to touch. Empty for an unknown product (open-world).
    pub fn fields_by_sensitivity(&self, m: &Manifest, tier: Sensitivity) -> Vec<&FieldSpec> {
        self.get(&m.product)
            .map(|s| s.fields_by_sensitivity(tier))
            .unwrap_or_default()
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

/// A composable **requirement-set** (ADR-0029 §5 trait/mixin): the shared imaging metadata every
/// modality-bearing product carries — defined **once** here and composed into `recon` / `dynamic_pet` /
/// `diffusion_mri` / `multicontrast_mri` rather than repeated per schema (DRY). `with` appends the
/// schema's own extra fields.
fn imaging_base(with: Vec<FieldSpec>) -> Vec<FieldSpec> {
    let mut fields = vec![FieldSpec::required("modality", "Imaging modality", "coded")
        .vocabulary("DICOM")
        .with_sensitivity(Sensitivity::Coded)];
    fields.extend(with);
    fields
}

/// The built-in product schemas. Each declares its required block(s) + key fields; all are
/// versioned `1.0` and evolve additively (new optional fields/blocks only). See ROADMAP P1 +
/// ADR-0029 §5 (the multi-dimensional `dynamic_pet`/`diffusion_mri`/`multicontrast_mri` set).
fn builtin_schemas() -> Vec<ProductSchema> {
    use BlockKind::{Array, Blob, Table};
    vec![
        ProductSchema {
            // A blob is opaque (no parsed metadata), so operator-supplied context is what makes a
            // preserved file Reusable — `study` is *recommended* (a warn nudge), never required (the
            // escape hatch must stay frictionless): you can always preserve junk now, label it later.
            fields: vec![FieldSpec::recommended(
                "study",
                "Study / exam this preserved file belongs to (FAIR grouping)",
                "string",
            )],
            blocks: vec![one(
                "data",
                Some(Blob),
                "The preserved source file, stored verbatim as opaque bytes (blake3-verified)",
            )],
            ..schema(
                "blob",
                "1.0",
                "Opaque preserved file — bytes stored bit-faithfully, not engine-parsed (the \"junk\" tier).",
            )
        },
        ProductSchema {
            // `rescale_*` are pure scan geometry → `Public`. The two trailing fields are seeded
            // from DICOM **PS3.15 Annex E** (Basic Application Confidentiality Profile, the
            // tag list any de-identifier must touch): a pseudonymised patient handle
            // (Patient Name / Patient ID family) and a study/series/SOP-instance UID
            // (UID family — the UIDs bind back to the patient/study, so PS3.15 requires
            // they be replaced / managed under the profile). Both are **optional** + tagged
            // `Identifying` so the schema-driven PHI machinery (today: an ingest warn; future:
            // redact / field encryption) picks them up without any per-product code change.
            fields: imaging_base(vec![
                FieldSpec::optional("rescale_slope", "Native→physical slope", "float64"),
                FieldSpec::optional("rescale_intercept", "Native→physical intercept", "float64"),
                FieldSpec::optional(
                    "patient_pseudonym",
                    "Pseudonymised patient handle (DICOM PS3.15 Patient Name / Patient ID family — direct PHI; supply a site-issued pseudonym, never the raw MRN)",
                    "string",
                )
                .with_sensitivity(Sensitivity::Identifying),
                FieldSpec::optional(
                    "acquisition_uid",
                    "Acquisition UID (DICOM PS3.15 UID family — Study/Series/SOPInstance UID; linking PHI under the confidentiality profile)",
                    "string",
                )
                .with_sensitivity(Sensitivity::Identifying),
            ]),
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
                Some(Array),
                "Histogram of an energy/lifetime/TOF quantity — a dense 1-D array by nature (ADR-0029 §6)",
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
            // ADR-0032 §6: the PET decay-correction reference — the named instant activity is
            // decay-corrected to (so a reader can recompute or un-correct). Optional metadata.
            fields: imaging_base(vec![FieldSpec::optional(
                "decay_correction_reference",
                "Named instant PET activity is decay-corrected to (e.g. injection / scan-start / acquisition-start)",
                "string",
            )]),
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
            fields: imaging_base(vec![]),
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
            fields: imaging_base(vec![]),
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
        // ADR-0030 §5 — the non-linear (deformable) registration carrier.
        ProductSchema {
            blocks: vec![one(
                "field",
                Some(Array),
                "A dense displacement/deformation vector field, e.g. `[3, z, y, x]` (one array block)",
            )],
            ..schema(
                "deformation_field",
                "1.0",
                "A non-linear (deformable) spatial transform — a per-voxel displacement field (ADR-0030 §5).",
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
            "deformation_field", // ADR-0030 §5 deformable registration carrier
            "blob",              // ADR-0038 opaque preservation tier
        ] {
            assert!(r.get(p).is_some(), "missing built-in schema '{p}'");
        }
        assert_eq!(r.products().count(), 14);
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
    fn dynamic_pet_carries_optional_decay_correction_reference() {
        use crate::block::array::{ArrayBlock, ArraySpec};
        use crate::block::table::{Column, TableBlock, TableSpec};
        let r = SchemaRegistry::builtin();
        // the schema declares the ADR-0032 §6 decay-correction reference as an optional field.
        let s = r.get("dynamic_pet").unwrap();
        let f = s
            .fields
            .iter()
            .find(|f| f.id == "decay_correction_reference")
            .expect("dynamic_pet declares decay_correction_reference");
        assert!(
            !f.required,
            "decay-correction reference is optional (feature-by-presence)"
        );
        // a product that sets it validates (the named instant activity is corrected to).
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
        let mut b = ProductBuilder::new("dynamic_pet", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.add_block(&timing).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "PT"}),
        );
        b.with_field("decay_correction_reference", serde_json::json!("injection"));
        let m = b.seal().unwrap();
        r.validate(&m).unwrap();
        assert_eq!(
            m.metadata.get("decay_correction_reference"),
            Some(&serde_json::json!("injection"))
        );
    }

    #[test]
    fn imaging_base_trait_set_is_shared_across_modality_schemas() {
        // ADR-0029 §5: the `modality` requirement is defined ONCE in `imaging_base()` and composed
        // into every modality-bearing schema — DRY trait/mixin. Assert every such schema carries the
        // identical shared field, and that `recon` additionally composes its own rescale fields on top.
        let r = SchemaRegistry::builtin();
        let shared = imaging_base(vec![]);
        assert_eq!(
            shared.len(),
            1,
            "imaging_base is exactly the shared modality field"
        );
        let modality = &shared[0];
        for product in ["recon", "dynamic_pet", "diffusion_mri", "multicontrast_mri"] {
            let s = r.get(product).expect("schema present");
            let f = s
                .fields
                .iter()
                .find(|f| f.id == "modality")
                .unwrap_or_else(|| panic!("{product} carries the shared modality field"));
            assert_eq!(
                f, modality,
                "{product}'s modality is the shared trait-set field, byte-for-byte"
            );
        }
        // recon = imaging_base + its own extras (composition appends, does not replace).
        let recon = r.get("recon").unwrap();
        assert_eq!(
            recon.fields.first().map(|f| f.id.as_str()),
            Some("modality")
        );
        assert!(recon.fields.iter().any(|f| f.id == "rescale_slope"));
        assert!(recon.fields.iter().any(|f| f.id == "rescale_intercept"));
    }

    #[test]
    fn recommended_field_warns_but_never_blocks() {
        let r = SchemaRegistry::builtin();
        // a blob with its required `data` block but no `study` → schema-VALID (study is recommended,
        // not required), yet surfaced as a recommendation (the warn tier).
        let mut b = ProductBuilder::new("blob", "junk", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(crate::block::BlockRef {
            name: "data".into(),
            kind: crate::block::BlockKind::Blob,
            digest: Some("blake3:00".into()),
            spec: serde_json::json!({"filename": "x.l64", "size": 1}),
        });
        let m = b.seal().unwrap();
        assert!(r.validate(&m).is_ok(), "recommended-absent must NOT block");
        let missing = r.missing_recommended(&m);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].id, "study");

        // supplying it (what `--meta study=…` does) clears the recommendation.
        let mut b2 = ProductBuilder::from_manifest(&m);
        b2.with_field("study", serde_json::json!("DUPLET-07"));
        let m2 = b2.seal().unwrap();
        assert!(r.missing_recommended(&m2).is_empty());
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

    // ─── ADR-0040 §1: sensitivity classification — plain-data tier + schema-driven query ───

    #[test]
    fn sensitivity_defaults_to_public_and_round_trips_through_serde() {
        // Default is `Public` (so an unannotated schema field — and any deserialized older
        // FieldSpec with the field absent — classifies as the safest tier, not PHI).
        assert_eq!(Sensitivity::default(), Sensitivity::Public);

        // snake_case wire form, all four variants round-trip.
        for (variant, wire) in [
            (Sensitivity::Public, "\"public\""),
            (Sensitivity::Coded, "\"coded\""),
            (Sensitivity::Sensitive, "\"sensitive\""),
            (Sensitivity::Identifying, "\"identifying\""),
        ] {
            let s = serde_json::to_string(&variant).unwrap();
            assert_eq!(s, wire, "{variant:?} serializes as {wire}");
            let v: Sensitivity = serde_json::from_str(&s).unwrap();
            assert_eq!(v, variant, "round-trip");
        }

        // A FieldSpec with NO `sensitivity` key deserializes (back-compat) → Public.
        let json = r#"{"id":"x","description":"y","dtype":"string"}"#;
        let f: FieldSpec = serde_json::from_str(json).unwrap();
        assert_eq!(f.sensitivity, Sensitivity::Public);

        // …and a FieldSpec round-trips a non-default tier intact through serde.
        let f = FieldSpec::optional("mrn", "PHI", "string")
            .with_sensitivity(Sensitivity::Identifying);
        let s = serde_json::to_string(&f).unwrap();
        let back: FieldSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back.sensitivity, Sensitivity::Identifying);
    }

    #[test]
    fn recon_seeds_ps3_15_identifying_fields_and_classifies_them() {
        // The spike's PS3.15-seeded example fields are present on `recon`, marked Identifying.
        let r = SchemaRegistry::builtin();
        let recon = r.get("recon").unwrap();
        for id in ["patient_pseudonym", "acquisition_uid"] {
            let f = recon
                .fields
                .iter()
                .find(|f| f.id == id)
                .unwrap_or_else(|| panic!("recon must seed PS3.15 example field '{id}'"));
            assert!(
                !f.required,
                "PS3.15 example field '{id}' is optional (escape hatch must stay frictionless)"
            );
            assert_eq!(
                f.sensitivity,
                Sensitivity::Identifying,
                "PS3.15 example field '{id}' is direct PHI → Identifying"
            );
        }
        // …and the rest of the recon fields are NOT identifying (Public/Coded only).
        for f in &recon.fields {
            if f.id == "patient_pseudonym" || f.id == "acquisition_uid" {
                continue;
            }
            assert_ne!(
                f.sensitivity,
                Sensitivity::Identifying,
                "non-seeded recon field '{}' must not classify as Identifying",
                f.id
            );
        }
    }

    #[test]
    fn fields_by_sensitivity_returns_the_right_tier() {
        let r = SchemaRegistry::builtin();
        // Build a real recon manifest so we can query through SchemaRegistry::fields_by_sensitivity.
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![8, 8, 8], "int16"));
        let mut b = ProductBuilder::new("recon", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "CT"}),
        );
        let m = b.seal().unwrap();

        // Identifying: exactly the two PS3.15-seeded recon fields.
        let phi = r.fields_by_sensitivity(&m, Sensitivity::Identifying);
        let phi_ids: Vec<&str> = phi.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(phi_ids, ["patient_pseudonym", "acquisition_uid"]);

        // Coded: modality (DICOM controlled vocab).
        let coded = r.fields_by_sensitivity(&m, Sensitivity::Coded);
        let coded_ids: Vec<&str> = coded.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(coded_ids, ["modality"]);

        // Public: rescale_slope + rescale_intercept (scan geometry, safe in clear).
        let public = r.fields_by_sensitivity(&m, Sensitivity::Public);
        let public_ids: Vec<&str> = public.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(public_ids, ["rescale_slope", "rescale_intercept"]);

        // Sensitive: none on recon.
        assert!(r
            .fields_by_sensitivity(&m, Sensitivity::Sensitive)
            .is_empty());

        // Open-world: an unknown product → empty (no schema to classify against).
        let m = ProductBuilder::new("my_custom", "x", "d", "2024-01-01T00:00:00Z")
            .seal()
            .unwrap();
        assert!(r
            .fields_by_sensitivity(&m, Sensitivity::Identifying)
            .is_empty());
    }
}
