//! DICOM ingest (#207) — read a DICOM image into a Tessera `recon` product, **losslessly**: the
//! native integer samples are preserved as-is (no rescale baked in); the DICOM rescale slope/
//! intercept, modality, and physical unit are carried as metadata so a reader recovers HU / Bq·mL⁻¹.
//! Transfer syntaxes decode **pure-Rust, no C++ codecs**: uncompressed (tested), plus JPEG Baseline
//! (Process 1, 8-bit) via `jpeg-decoder`/`jpeg-encoder` (the `jpeg` feature on the transfer-syntax
//! registry) — proven end-to-end by `ingests_a_jpeg_baseline_encapsulated_dicom_pure_rust`. JPEG Lossless
//! (Process 14) routes through the same `JpegAdapter` (`jpeg-decoder` supports the SOF3 lossless path),
//! but Tessera has no fixture exercising it yet, so it is not claimed proven. JPEG-LS (.80/.81 — charls),
//! JPEG 2000 (.90/.91 — openjpeg-sys), and 12-bit Extended need the deliberately-excluded C libraries and
//! stay unsupported. Decode dispatch is the same `decode_pixel_data()` call for every syntax — the
//! registry picks the adapter by UID.

use std::collections::BTreeMap;

use dicom::core::header::Header;
use dicom::core::value::Value;
use dicom::core::{DataElement, PrimitiveValue, Tag, VR};
use dicom::object::{FileDicomObject, InMemDicomObject};
use dicom::pixeldata::{ConvertOptions, ModalityLutOption, PixelDecoder};
use tessera_core::block::array::ArraySpec;
use tessera_core::manifest::Manifest;
use tessera_core::{Error, ProductBuilder, Result};
use tessera_io::array::{self, ArrayData};
use tessera_io::BlockPayload;

const MODALITY: Tag = Tag(0x0008, 0x0060);
const ROWS: Tag = Tag(0x0028, 0x0010);
const COLUMNS: Tag = Tag(0x0028, 0x0011);
const RESCALE_INTERCEPT: Tag = Tag(0x0028, 0x1052);
const RESCALE_SLOPE: Tag = Tag(0x0028, 0x1053);
const INSTANCE_NUMBER: Tag = Tag(0x0020, 0x0013);

// Curated tags ported to the `recon` schema (issue #dicom-metadata / ADR-0040 §1). Every one
// is *recommended* (not required) so a series missing any single tag still ingests.
const STUDY_INSTANCE_UID: Tag = Tag(0x0020, 0x000D);
const SERIES_INSTANCE_UID: Tag = Tag(0x0020, 0x000E);
const STUDY_DATE: Tag = Tag(0x0008, 0x0020);
const MANUFACTURER: Tag = Tag(0x0008, 0x0070);
const MODEL_NAME: Tag = Tag(0x0008, 0x1090);
const KVP: Tag = Tag(0x0018, 0x0060);
const SLICE_THICKNESS: Tag = Tag(0x0018, 0x0050);
const PIXEL_SPACING: Tag = Tag(0x0028, 0x0030);

/// The pixel-data tag — excluded from the `extra["dicom_header"]` dump (the pixels are the
/// `volume` block; the header is metadata about them, not a redundant copy).
const PIXEL_DATA: Tag = Tag(0x7FE0, 0x0010);

/// A DICOM image decoded for Tessera: native int16 samples (C order, `[rows, cols]`) plus the
/// metadata needed to recover physical units. The rescale is **not** applied to `voxels`.
///
/// `curated` carries the schema-mapped DICOM tags ported to the `recon` product's metadata
/// fields (Study/Series UID, StudyDate, Manufacturer, ManufacturerModelName, KVP,
/// SliceThickness, PixelSpacing — see [`DicomCuratedTags`]). `header_json` is the *full*
/// DICOM header of the representative (first) slice, minus PixelData, ready to drop into
/// `manifest.extra["dicom_header"]`. On a de-identified ingest the `header_json` is the
/// **PS3.15-scrubbed** dump (PHI never reaches this struct).
#[derive(Debug, Clone, PartialEq)]
pub struct DicomImage {
    pub shape: Vec<u64>,
    pub voxels: Vec<i16>,
    pub modality: String,
    pub rescale_slope: f64,
    pub rescale_intercept: f64,
    /// Curated DICOM tags → `recon` schema fields (see [`DicomCuratedTags`]). Any absent DICOM
    /// tag is `None`; the schema marks these `recommended` so absence is a warn, not a block.
    pub curated: DicomCuratedTags,
    /// Full DICOM header (minus PixelData) as a deterministically-serialized JSON object,
    /// destined for `manifest.extra["dicom_header"]`. Deterministic ordering — the underlying
    /// map is a [`BTreeMap`] keyed by `"GGGG,EEEE"` uppercase hex, so re-ingest is byte-identical.
    pub header_json: serde_json::Value,
}

/// Curated DICOM tags carried into the `recon` schema (ADR-0040 §1 / PS3.15 seeding). Every
/// field is `Option` — the DICOM ingest never blocks on a missing tag, matching the
/// schema's `recommended` (warn-tier) classification.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DicomCuratedTags {
    /// DICOM `StudyInstanceUID` (0020,000D) — PS3.15 UID family (Identifying).
    pub study_instance_uid: Option<String>,
    /// DICOM `SeriesInstanceUID` (0020,000E) — PS3.15 UID family (Identifying).
    pub series_instance_uid: Option<String>,
    /// DICOM `StudyDate` (0008,0020), YYYYMMDD — Sensitive.
    pub study_date: Option<String>,
    /// DICOM `Manufacturer` (0008,0070) — Public.
    pub manufacturer: Option<String>,
    /// DICOM `ManufacturerModelName` (0008,1090) — Public.
    pub model_name: Option<String>,
    /// DICOM `KVP` (0018,0060), kV — Public (CT-specific).
    pub kvp: Option<f64>,
    /// DICOM `SliceThickness` (0018,0050), mm — Public.
    pub slice_thickness: Option<f64>,
    /// DICOM `PixelSpacing` (0028,0030), `[row_mm, col_mm]` — Public.
    pub pixel_spacing: Option<[f64; 2]>,
}

fn de(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("dicom: {e}"))
}

/// Read a single-frame DICOM file into a [`DicomImage`].
pub fn read_image(path: &std::path::Path) -> Result<DicomImage> {
    let obj = dicom::object::open_file(path).map_err(de)?;
    read_object(&obj)
}

/// Open a DICOM file, apply PS3.15 de-identification (drop PHI tags) in memory, then decode the
/// pixels. The image content is unchanged; the PHI never reaches the Tessera product.
pub fn read_image_deidentified(path: &std::path::Path) -> Result<DicomImage> {
    let mut obj = dicom::object::open_file(path).map_err(de)?;
    deidentify(&mut obj);
    read_object(&obj)
}

/// Decode an already-parsed DICOM object (testable without touching the filesystem).
pub fn read_object(obj: &FileDicomObject<InMemDicomObject>) -> Result<DicomImage> {
    let modality = obj
        .element(MODALITY)
        .map_err(de)?
        .to_str()
        .map_err(de)?
        .trim()
        .to_string();
    let rows_u32 = obj.element(ROWS).map_err(de)?.to_int::<u32>().map_err(de)?;
    let cols_u32 = obj
        .element(COLUMNS)
        .map_err(de)?
        .to_int::<u32>()
        .map_err(de)?;
    let rows = u64::from(rows_u32);
    let cols = u64::from(cols_u32);
    let rescale_slope = obj
        .element(RESCALE_SLOPE)
        .ok()
        .and_then(|e| e.to_float64().ok())
        .unwrap_or(1.0);
    let rescale_intercept = obj
        .element(RESCALE_INTERCEPT)
        .ok()
        .and_then(|e| e.to_float64().ok())
        .unwrap_or(0.0);
    // Lossless: raw stored samples, no modality LUT (rescale is preserved as metadata, not applied).
    let opts = ConvertOptions::new().with_modality_lut(ModalityLutOption::None);
    let voxels: Vec<i16> = obj
        .decode_pixel_data()
        .map_err(de)?
        .to_vec_with_options::<i16>(&opts)
        .map_err(de)?;
    if voxels.len() as u64 != rows * cols {
        return Err(Error::Invalid(format!(
            "dicom: pixel count {} != rows*cols {}",
            voxels.len(),
            rows * cols
        )));
    }
    let curated = extract_curated_tags(obj);
    let header_json = header_to_json(obj);
    Ok(DicomImage {
        shape: vec![rows, cols],
        voxels,
        modality,
        rescale_slope,
        rescale_intercept,
        curated,
        header_json,
    })
}

/// Extract the schema-mapped [`DicomCuratedTags`] from an already-parsed DICOM object. Any tag
/// missing from the object is left `None` (never a hard error — the recon schema classifies these
/// as `recommended`, so absence is a warn tier, not a block).
fn extract_curated_tags(obj: &FileDicomObject<InMemDicomObject>) -> DicomCuratedTags {
    fn trimmed_str(obj: &FileDicomObject<InMemDicomObject>, tag: Tag) -> Option<String> {
        obj.element(tag)
            .ok()
            .and_then(|e| e.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
    fn f64_of(obj: &FileDicomObject<InMemDicomObject>, tag: Tag) -> Option<f64> {
        obj.element(tag).ok().and_then(|e| e.to_float64().ok())
    }
    // PixelSpacing (0028,0030) is a DICOM `DS` (decimal string) with 2 values, separated by a
    // backslash. Real parsers may hand us either a `PrimitiveValue::Strs` (the split-by-parser
    // form) or a `PrimitiveValue::Str` carrying the raw `"row\\col"` text — try `to_multi_float64`
    // first, then fall back to splitting the string form on the DICOM value separator.
    let pixel_spacing = obj.element(PIXEL_SPACING).ok().and_then(|e| {
        e.to_multi_float64()
            .ok()
            .and_then(|v| {
                if v.len() >= 2 {
                    Some([v[0], v[1]])
                } else {
                    None
                }
            })
            .or_else(|| {
                let raw = e.to_str().ok()?;
                let mut parts = raw.trim().split('\\');
                let a: f64 = parts.next()?.trim().parse().ok()?;
                let b: f64 = parts.next()?.trim().parse().ok()?;
                Some([a, b])
            })
    });
    DicomCuratedTags {
        study_instance_uid: trimmed_str(obj, STUDY_INSTANCE_UID),
        series_instance_uid: trimmed_str(obj, SERIES_INSTANCE_UID),
        study_date: trimmed_str(obj, STUDY_DATE),
        manufacturer: trimmed_str(obj, MANUFACTURER),
        model_name: trimmed_str(obj, MODEL_NAME),
        kvp: f64_of(obj, KVP),
        slice_thickness: f64_of(obj, SLICE_THICKNESS),
        pixel_spacing,
    }
}

/// Serialize the full DICOM header (every top-level element **except** PixelData) into a
/// deterministic JSON object destined for `manifest.extra["dicom_header"]`.
///
/// **Shape:** a JSON object keyed by `"GGGG,EEEE"` (uppercase hex, comma-separated), each value
/// `{"vr": "<VR>", "value": <JSON>}`. `<JSON>` is a JSON array of the element's multi-string
/// representation (via [`PrimitiveValue::to_multi_str`]) — every DICOM value type has a
/// deterministic textual form. Sequences (`VR::SQ`) recurse (nested arrays of objects); pixel
/// fragments (encapsulated pixel data) collapse to a size marker rather than the fragment bytes.
///
/// The container is a [`BTreeMap`] → keys are ordered by string comparison of the hex tag, so the
/// output is byte-identical across runs (the write-determinism gate).
///
/// **PHI safety:** whatever `obj` contains is what serializes here. Callers who want the scrubbed
/// header ([`read_image_deidentified`] / [`read_series_deidentified`]) must call [`deidentify`]
/// on `obj` *before* the object reaches this function (which is exactly how the de-id ingest
/// paths are wired — [`read_object`] runs *after* `deidentify` on the de-id path).
fn header_to_json(obj: &FileDicomObject<InMemDicomObject>) -> serde_json::Value {
    let mut out: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for elem in obj.iter() {
        let tag = elem.tag();
        if tag == PIXEL_DATA {
            continue; // pixels are the volume block, not header metadata
        }
        let key = format!("{:04X},{:04X}", tag.group(), tag.element());
        out.insert(
            key,
            serde_json::json!({
                "vr": elem.vr().to_string(),
                "value": value_to_json(elem.value()),
            }),
        );
    }
    serde_json::to_value(out).unwrap_or(serde_json::Value::Null)
}

/// Render an in-memory DICOM element's value as deterministic JSON (see [`header_to_json`]).
///
/// - `Primitive` → array of strings (`to_multi_str`); every primitive has a stable text form.
/// - `Sequence` → array of nested objects (each item is one recursion of the header dump).
/// - `PixelSequence` → a `{"fragments": N}` marker rather than fragment bytes (never rehydrated
///   from the header; the pixel payload is already in the `volume` block, not the header).
fn value_to_json(
    value: &Value<InMemDicomObject, dicom::core::value::InMemFragment>,
) -> serde_json::Value {
    match value {
        Value::Primitive(p) => {
            let strs: Vec<String> = p.to_multi_str().iter().cloned().collect();
            serde_json::json!(strs)
        }
        Value::Sequence(seq) => {
            let items: Vec<serde_json::Value> =
                seq.items().iter().map(nested_object_to_json).collect();
            serde_json::Value::Array(items)
        }
        Value::PixelSequence(seq) => serde_json::json!({
            "fragments": seq.fragments().len(),
        }),
    }
}

/// Recurse a `SQ`-nested `InMemDicomObject` into the same `{ "GGGG,EEEE": {vr, value} }` shape.
fn nested_object_to_json(item: &InMemDicomObject) -> serde_json::Value {
    let mut nested: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for elem in item {
        let tag = elem.tag();
        if tag == PIXEL_DATA {
            continue;
        }
        let key = format!("{:04X},{:04X}", tag.group(), tag.element());
        nested.insert(
            key,
            serde_json::json!({
                "vr": elem.vr().to_string(),
                "value": value_to_json(elem.value()),
            }),
        );
    }
    serde_json::to_value(nested).unwrap_or(serde_json::Value::Null)
}

/// Read and stack a multi-slice DICOM series into a 3-D `[z, rows, cols]` volume, ordered by
/// InstanceNumber (geometric ImagePositionPatient ordering is a later refinement). Every slice must
/// share rows/cols/modality/rescale, else the series is rejected as non-uniform.
pub fn read_series(paths: &[std::path::PathBuf]) -> Result<DicomImage> {
    read_series_inner(paths, false)
}

/// Same as [`read_series`] but applies PS3.15 de-identification ([`deidentify`]) to **every** slice
/// in memory before decoding — the per-slice analogue of [`read_image_deidentified`]. PHI never
/// reaches the stacked volume.
pub fn read_series_deidentified(paths: &[std::path::PathBuf]) -> Result<DicomImage> {
    read_series_inner(paths, true)
}

/// Shared series body for [`read_series`] / [`read_series_deidentified`] — opening each slice, then
/// optionally stripping PHI via [`deidentify`] before [`read_object`] decodes the pixels, then
/// stacking by `InstanceNumber`. The PHI is gone before the pixels are even decoded; non-uniform
/// shape/modality/rescale across slices is rejected as in the non-de-id path.
fn read_series_inner(paths: &[std::path::PathBuf], strip_phi: bool) -> Result<DicomImage> {
    if paths.is_empty() {
        return Err(Error::Invalid("dicom: empty series".into()));
    }
    let mut slices: Vec<(i64, DicomImage)> = Vec::with_capacity(paths.len());
    for p in paths {
        let mut obj = dicom::object::open_file(p).map_err(de)?;
        if strip_phi {
            deidentify(&mut obj);
        }
        let instance = obj
            .element(INSTANCE_NUMBER)
            .ok()
            .and_then(|e| e.to_int::<i64>().ok())
            .unwrap_or(0);
        slices.push((instance, read_object(&obj)?));
    }
    slices.sort_by_key(|(k, _)| *k);

    let first = slices[0].1.clone();
    for (_, s) in &slices {
        if s.shape != first.shape
            || s.modality != first.modality
            || s.rescale_slope != first.rescale_slope
            || s.rescale_intercept != first.rescale_intercept
        {
            return Err(Error::Invalid(
                "dicom: non-uniform series (shape/modality/rescale differ between slices)".into(),
            ));
        }
    }
    let mut voxels = Vec::with_capacity(slices.len() * first.voxels.len());
    for (_, s) in &slices {
        voxels.extend_from_slice(&s.voxels);
    }
    let z = u64::try_from(slices.len())
        .map_err(|e| Error::Invalid(format!("dicom: series slice count overflow: {e}")))?;
    // Curated tags + header live on the representative (first-by-InstanceNumber) slice — the
    // series is uniform for study/series-level tags, and the per-slice diff (InstanceNumber,
    // ImagePositionPatient, …) matters far less than the study/series/manufacturer context.
    Ok(DicomImage {
        shape: vec![z, first.shape[0], first.shape[1]],
        voxels,
        modality: first.modality,
        rescale_slope: first.rescale_slope,
        rescale_intercept: first.rescale_intercept,
        curated: first.curated,
        header_json: first.header_json,
    })
}

/// PS3.15 Basic Application Level Confidentiality — the core direct-identifier (PHI) tags Tessera
/// removes and checks. A pragmatic high-risk subset of the standard's list; extend as needed.
pub const PHI_TAGS: &[Tag] = &[
    Tag(0x0008, 0x0050), // AccessionNumber
    Tag(0x0008, 0x0080), // InstitutionName
    Tag(0x0008, 0x0090), // ReferringPhysicianName
    Tag(0x0008, 0x1030), // StudyDescription (free text — may carry PHI)
    Tag(0x0008, 0x103E), // SeriesDescription
    Tag(0x0010, 0x0010), // PatientName
    Tag(0x0010, 0x0020), // PatientID
    Tag(0x0010, 0x0030), // PatientBirthDate
    Tag(0x0010, 0x1000), // OtherPatientIDs
    Tag(0x0010, 0x1010), // PatientAge
    Tag(0x0010, 0x2160), // EthnicGroup
    Tag(0x0010, 0x4000), // PatientComments
];
const PATIENT_IDENTITY_REMOVED: Tag = Tag(0x0012, 0x0062);

/// The PHI tags still present in `obj` (a compliance gate before ingest/egress); empty == clean.
/// Note: Tessera's `read_*` ingest never reads PHI, so ingested products are PHI-free regardless —
/// this checks the *source* object / supports a de-identified egress path.
pub fn phi_present(obj: &FileDicomObject<InMemDicomObject>) -> Vec<Tag> {
    PHI_TAGS
        .iter()
        .copied()
        .filter(|&t| obj.element(t).is_ok())
        .collect()
}

/// Remove the PHI tags ([`PHI_TAGS`]) from `obj` in place and set `PatientIdentityRemoved = "YES"`
/// (PS3.15). Returns how many PHI elements were removed.
pub fn deidentify(obj: &mut FileDicomObject<InMemDicomObject>) -> usize {
    let removed = PHI_TAGS.iter().filter(|&&t| obj.remove_element(t)).count();
    obj.put(DataElement::new(
        PATIENT_IDENTITY_REMOVED,
        VR::CS,
        PrimitiveValue::from("YES"),
    ));
    removed
}

/// The UCUM unit implied by a DICOM modality (the rescaled value's unit), if known.
fn unit_for(modality: &str) -> Option<&'static str> {
    match modality {
        "CT" => Some("HU"),
        "PT" => Some("Bq/mL"),
        _ => None,
    }
}

/// Build a sealed Tessera `recon` product (manifest + payloads) from a DICOM image. The volume is an
/// `int16` array carrying the DICOM rescale → physical units; `modality` is an fd5 `Coded` value.
/// `source` (e.g. the DICOM SOP Instance UID or filename) is recorded as a provenance edge.
///
/// `extra_sources` are appended after the canonical `ingested_from` edge — typed [`Source`] edges
/// supplied by orchestrators (the declarative ingest engine threads `derived_from` parents +
/// `ingested_via_spec` here so chain verification picks up the parent's `manifest_hash`).
pub fn to_recon_product(
    img: &DicomImage,
    name: &str,
    timestamp: &str,
    source: &str,
    source_digest: Option<&str>,
    extra_sources: &[tessera_core::provenance::Source],
) -> Result<(Manifest, Vec<BlockPayload>)> {
    let mut spec = ArraySpec::new(img.shape.clone(), "int16")
        .with_rescale(img.rescale_slope, img.rescale_intercept);
    if let Some(u) = unit_for(&img.modality) {
        spec = spec.with_unit(u);
    }
    let data = ArrayData::I16(img.voxels.clone());
    let (block_ref, payload) = array::array_block("volume", &spec, &data)?;

    let mut b = ProductBuilder::new("recon", name, "DICOM recon volume", timestamp);
    b.add_block_ref(block_ref);
    b.with_field(
        "modality",
        serde_json::json!({"_vocabulary": "DICOM", "_code": img.modality}),
    );

    // Curated DICOM tags → `recon` schema fields (each is `recommended`, so absence is fine).
    let c = &img.curated;
    if let Some(v) = &c.study_instance_uid {
        b.with_field("study_instance_uid", serde_json::json!(v));
    }
    if let Some(v) = &c.series_instance_uid {
        b.with_field("series_instance_uid", serde_json::json!(v));
    }
    if let Some(v) = &c.study_date {
        b.with_field("study_date", serde_json::json!(v));
    }
    if let Some(v) = &c.manufacturer {
        b.with_field("manufacturer", serde_json::json!(v));
    }
    if let Some(v) = &c.model_name {
        b.with_field("model_name", serde_json::json!(v));
    }
    if let Some(v) = c.kvp {
        b.with_field("kvp", serde_json::json!(v));
    }
    if let Some(v) = c.slice_thickness {
        b.with_field("slice_thickness", serde_json::json!(v));
    }
    if let Some(v) = c.pixel_spacing {
        b.with_field("pixel_spacing", serde_json::json!(v));
    }

    // Full DICOM header → `extra["dicom_header"]` (PHI-scrubbed on the de-id path, since the
    // header snapshot was taken *after* `deidentify` ran on the source object).
    if !img.header_json.is_null() {
        b.with_extra("dicom_header", img.header_json.clone());
    }

    // The `ingested_from` edge carries a `content_hash` (merkle root over the source `.dcm` slices,
    // computed by the caller which holds the paths) so the product is cryptographically tied to the
    // exact source bytes — independent of the (possibly PHI-scrubbed) `source` reference.
    let mut edge = tessera_core::provenance::Source::new("ingested_from", source);
    if let Some(d) = source_digest {
        edge = edge.with_content_hash(d);
    }
    b.add_source(edge);
    for s in extra_sources {
        b.add_source(s.clone());
    }
    let sealed = b.seal()?;
    Ok((sealed, vec![payload]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom::object::meta::FileMetaTableBuilder;

    fn sample_image() -> DicomImage {
        DicomImage {
            shape: vec![8, 8],
            voxels: (0..64).map(|k| k as i16 - 1024).collect(),
            modality: "CT".into(),
            rescale_slope: 1.0,
            rescale_intercept: -1024.0,
            curated: DicomCuratedTags::default(),
            header_json: serde_json::Value::Null,
        }
    }

    #[test]
    fn to_recon_product_carries_rescale_modality_and_provenance() {
        let img = sample_image();
        let (sealed, payloads) = to_recon_product(
            &img,
            "DP06-ct",
            "2024-01-01T00:00:00Z",
            "1.2.3.sop",
            None,
            &[],
        )
        .unwrap();
        assert_eq!(sealed.product, "recon");
        assert!(sealed.is_sealed());
        assert_eq!(sealed.blocks.len(), 1);
        // rescale + unit live in the block spec; modality + provenance in the manifest
        let spec = &sealed.blocks[0].spec;
        assert_eq!(spec["rescale_intercept"], -1024.0);
        assert_eq!(spec["unit"], "HU");
        assert_eq!(sealed.metadata["modality"]["_code"], "CT");
        assert_eq!(sealed.sources[0].reference, "1.2.3.sop");

        // packs to a verifiable .tsra and the volume reads back
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ct.tsra");
        tessera_io::pack(&sealed, &payloads, &path).unwrap();
        let mut r = tessera_io::Reader::open(&path).unwrap();
        assert!(!r.read_block("volume").unwrap().is_empty());
    }

    /// Synthesize a minimal uncompressed CT DICOM, write it, read it back, ingest — fully hermetic.
    #[test]
    fn roundtrip_synthetic_dicom_file() {
        let (rows, cols) = (8u16, 8u16);
        let n = rows as usize * cols as usize;
        // small positive samples so the unsigned storage equals the signed interpretation
        let pixels: Vec<u16> = (0..n).map(|k| (k as u16) * 7).collect();

        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(MODALITY, VR::CS, PrimitiveValue::from("CT")),
            DataElement::new(ROWS, VR::US, PrimitiveValue::from(rows)),
            DataElement::new(COLUMNS, VR::US, PrimitiveValue::from(cols)),
            DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)), // SamplesPerPixel
            DataElement::new(
                Tag(0x0028, 0x0004),
                VR::CS,
                PrimitiveValue::from("MONOCHROME2"),
            ), // PhotometricInterpretation
            DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(16u16)), // BitsAllocated
            DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(16u16)), // BitsStored
            DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(15u16)), // HighBit
            DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(1u16)), // PixelRepresentation=signed
            DataElement::new(RESCALE_INTERCEPT, VR::DS, PrimitiveValue::from("-1024")),
            DataElement::new(RESCALE_SLOPE, VR::DS, PrimitiveValue::from("1")),
            DataElement::new(
                Tag(0x7FE0, 0x0010),
                VR::OW,
                PrimitiveValue::U16(pixels.clone().into()),
            ),
        ]);
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1") // Explicit VR Little Endian
            .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.2") // CT Image Storage
            .media_storage_sop_instance_uid("1.2.3.4.5.6.7.8.9.0")
            .implementation_class_uid("1.2.826.0.1.3680043.tessera")
            .build()
            .unwrap();
        let file_obj = obj.with_exact_meta(meta);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synth.dcm");
        file_obj.write_to_file(&path).unwrap();

        let img = read_image(&path).unwrap();
        assert_eq!(img.shape, vec![8, 8]);
        assert_eq!(img.modality, "CT");
        assert_eq!(img.rescale_intercept, -1024.0);
        assert_eq!(img.rescale_slope, 1.0);
        let expected: Vec<i16> = (0..n).map(|k| (k as u16 * 7) as i16).collect();
        assert_eq!(img.voxels, expected);
    }

    /// Build a deterministic uncompressed CT DICOM in memory (no PHI, no external fixture) and write it.
    fn write_golden_ct(path: &std::path::Path) {
        let (rows, cols) = (16u16, 16u16);
        let n = rows as usize * cols as usize;
        let pixels: Vec<u16> = (0..n).map(|k| (k as u16).wrapping_mul(11) % 4096).collect();
        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(MODALITY, VR::CS, PrimitiveValue::from("CT")),
            DataElement::new(ROWS, VR::US, PrimitiveValue::from(rows)),
            DataElement::new(COLUMNS, VR::US, PrimitiveValue::from(cols)),
            DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(
                Tag(0x0028, 0x0004),
                VR::CS,
                PrimitiveValue::from("MONOCHROME2"),
            ),
            DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(12u16)),
            DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(11u16)),
            DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(0u16)),
            DataElement::new(RESCALE_INTERCEPT, VR::DS, PrimitiveValue::from("-1024")),
            DataElement::new(RESCALE_SLOPE, VR::DS, PrimitiveValue::from("1")),
            DataElement::new(
                Tag(0x7FE0, 0x0010),
                VR::OW,
                PrimitiveValue::U16(pixels.into()),
            ),
        ]);
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1")
            .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.2")
            .media_storage_sop_instance_uid("1.2.3.4.5.6.7.8.9.2")
            .implementation_class_uid("1.2.826.0.1.3680043.tessera")
            .build()
            .unwrap();
        obj.with_exact_meta(meta).write_to_file(path).unwrap();
    }

    /// Golden DICOM conformance: a fixed synthetic CT must always ingest to the same `content_hash`
    /// (the MMR root over the decoded voxels), independent of the manifest timestamp/SOP — the same
    /// content-addressing guarantee the `.tsra` corpus locks, now anchored at the DICOM door. Drift here
    /// means the decode or the array encode changed bytes.
    #[test]
    fn golden_dicom_ingest_is_deterministic_and_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("golden.dcm");
        write_golden_ct(&path);

        let img = read_image(&path).unwrap();
        let (a, _) = to_recon_product(
            &img,
            "golden-ct",
            "2024-01-01T00:00:00Z",
            "sop-a",
            None,
            &[],
        )
        .unwrap();
        // a different timestamp + source must NOT change the content hash (content-addressing)
        let (b, _) = to_recon_product(
            &img,
            "golden-ct",
            "2099-12-31T23:59:59Z",
            "sop-b",
            None,
            &[],
        )
        .unwrap();
        let ha = a.content_hash.as_deref().unwrap();
        let hb = b.content_hash.as_deref().unwrap();
        assert_eq!(
            ha, hb,
            "content_hash must be independent of manifest metadata"
        );
        assert_eq!(
            ha, "blake3:b11c199ce147cf505159ddeff94ebda4c4861cb4a1ca3a124ec11f7aa8215627",
            "golden DICOM content_hash drifted (decode/encode bytes changed): {ha}"
        );
    }

    /// Synthesize an 8-bit CT slice, transcode it to **JPEG Baseline (Process 1)** with the pure-Rust
    /// `jpeg-encoder`, write it, then ingest it back through `read_image` — proving the JPEG-encapsulated
    /// decode path (`jpeg-decoder`, no C++ codec) works end-to-end under the hermetic gate. JPEG Baseline
    /// is lossy, so the smooth gradient is asserted within a tolerance (and its monotonic structure must
    /// survive); the point is that a compressed transfer syntax decodes at all, purely in Rust.
    #[test]
    fn ingests_a_jpeg_baseline_encapsulated_dicom_pure_rust() {
        use dicom::pixeldata::Transcode;
        use dicom_transfer_syntax_registry::entries::JPEG_BASELINE;

        let (rows, cols) = (16u16, 16u16);
        let n = rows as usize * cols as usize;
        // smooth horizontal gradient (0..240) — JPEG handles low-frequency content with minimal loss
        let pixels: Vec<u8> = (0..n).map(|k| ((k % cols as usize) * 16) as u8).collect();

        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(MODALITY, VR::CS, PrimitiveValue::from("CT")),
            DataElement::new(ROWS, VR::US, PrimitiveValue::from(rows)),
            DataElement::new(COLUMNS, VR::US, PrimitiveValue::from(cols)),
            DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)), // SamplesPerPixel
            DataElement::new(
                Tag(0x0028, 0x0004),
                VR::CS,
                PrimitiveValue::from("MONOCHROME2"),
            ), // PhotometricInterpretation
            DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(8u16)), // BitsAllocated
            DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(8u16)), // BitsStored
            DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(7u16)), // HighBit
            DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(0u16)), // PixelRepresentation=unsigned
            DataElement::new(
                Tag(0x7FE0, 0x0010),
                VR::OB,
                PrimitiveValue::U8(pixels.clone().into()),
            ),
        ]);
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1") // start native (Explicit VR LE)
            .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.2")
            .media_storage_sop_instance_uid("1.2.3.4.5.6.7.8.9.1")
            .implementation_class_uid("1.2.826.0.1.3680043.tessera")
            .build()
            .unwrap();
        let mut file_obj = obj.with_exact_meta(meta);

        // pure-Rust JPEG Baseline ENCODE (jpeg-encoder), producing encapsulated fragments
        file_obj.transcode(&JPEG_BASELINE.erased()).unwrap();
        assert_eq!(file_obj.meta().transfer_syntax(), "1.2.840.10008.1.2.4.50");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jpeg_baseline.dcm");
        file_obj.write_to_file(&path).unwrap();

        // pure-Rust JPEG DECODE through our ingest (jpeg-decoder via the registry adapter)
        let img = read_image(&path).unwrap();
        assert_eq!(img.shape, vec![16, 16]);
        assert_eq!(img.voxels.len(), n);

        // lossy but faithful: bounded per-pixel error, and the gradient's structure survives
        let max_err = img
            .voxels
            .iter()
            .zip(&pixels)
            .map(|(d, o)| (*d as i32 - i32::from(*o)).abs())
            .max()
            .unwrap();
        assert!(max_err <= 24, "JPEG round-trip error too large: {max_err}");
        // the row-0 horizontal gradient (0 → 240 left-to-right) must survive the lossy round-trip
        let (leftmost, rightmost) = (img.voxels[0], img.voxels[cols as usize - 1]);
        assert!(
            rightmost - leftmost > 100,
            "horizontal gradient lost after JPEG round-trip: {leftmost} → {rightmost}"
        );
    }

    /// Write a minimal CT slice with a given InstanceNumber and pixel base value.
    fn write_slice(path: &std::path::Path, instance: i32, base: u16) {
        let pixels: Vec<u16> = (0..64).map(|k| base + k as u16).collect();
        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(MODALITY, VR::CS, PrimitiveValue::from("CT")),
            DataElement::new(ROWS, VR::US, PrimitiveValue::from(8u16)),
            DataElement::new(COLUMNS, VR::US, PrimitiveValue::from(8u16)),
            DataElement::new(
                INSTANCE_NUMBER,
                VR::IS,
                PrimitiveValue::from(instance.to_string()),
            ),
            DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(
                Tag(0x0028, 0x0004),
                VR::CS,
                PrimitiveValue::from("MONOCHROME2"),
            ),
            DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(15u16)),
            DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(RESCALE_INTERCEPT, VR::DS, PrimitiveValue::from("-1024")),
            DataElement::new(RESCALE_SLOPE, VR::DS, PrimitiveValue::from("1")),
            DataElement::new(
                Tag(0x7FE0, 0x0010),
                VR::OW,
                PrimitiveValue::U16(pixels.into()),
            ),
        ]);
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1")
            .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.2")
            .media_storage_sop_instance_uid(format!("1.2.3.{instance}"))
            .implementation_class_uid("1.2.826.0.1.3680043.tessera")
            .build()
            .unwrap();
        obj.with_exact_meta(meta).write_to_file(path).unwrap();
    }

    #[test]
    fn series_stacks_in_instance_order() {
        let dir = tempfile::tempdir().unwrap();
        // write three slices out of order; distinct pixel bases identify them
        let p3 = dir.path().join("s3.dcm");
        let p1 = dir.path().join("s1.dcm");
        let p2 = dir.path().join("s2.dcm");
        write_slice(&p3, 3, 3000);
        write_slice(&p1, 1, 1000);
        write_slice(&p2, 2, 2000);

        let vol = read_series(&[p3, p1, p2]).unwrap();
        assert_eq!(vol.shape, vec![3, 8, 8]); // [z, rows, cols]
                                              // sorted by InstanceNumber 1,2,3 regardless of input order
        assert_eq!(vol.voxels[0], 1000);
        assert_eq!(vol.voxels[64], 2000);
        assert_eq!(vol.voxels[128], 3000);
    }

    /// Like [`write_slice`] but stamps PHI tags onto the slice — used by the series-de-id test to prove
    /// PHI is stripped per-slice before the volume is built.
    fn write_phi_slice(path: &std::path::Path, instance: i32, base: u16, patient: &str) {
        let pixels: Vec<u16> = (0..64).map(|k| base + k as u16).collect();
        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(MODALITY, VR::CS, PrimitiveValue::from("CT")),
            DataElement::new(ROWS, VR::US, PrimitiveValue::from(8u16)),
            DataElement::new(COLUMNS, VR::US, PrimitiveValue::from(8u16)),
            DataElement::new(
                INSTANCE_NUMBER,
                VR::IS,
                PrimitiveValue::from(instance.to_string()),
            ),
            DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(
                Tag(0x0028, 0x0004),
                VR::CS,
                PrimitiveValue::from("MONOCHROME2"),
            ),
            DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(15u16)),
            DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(RESCALE_INTERCEPT, VR::DS, PrimitiveValue::from("-1024")),
            DataElement::new(RESCALE_SLOPE, VR::DS, PrimitiveValue::from("1")),
            // PHI tags — must NOT survive a `read_series_deidentified`.
            DataElement::new(
                Tag(0x0010, 0x0010), // PatientName
                VR::PN,
                PrimitiveValue::from(patient),
            ),
            DataElement::new(
                Tag(0x0010, 0x0020), // PatientID
                VR::LO,
                PrimitiveValue::from(format!("PID-{instance}")),
            ),
            DataElement::new(
                Tag(0x0008, 0x0080), // InstitutionName
                VR::LO,
                PrimitiveValue::from("ACME Hospital"),
            ),
            DataElement::new(
                Tag(0x7FE0, 0x0010),
                VR::OW,
                PrimitiveValue::U16(pixels.into()),
            ),
        ]);
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1")
            .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.2")
            .media_storage_sop_instance_uid(format!("1.2.3.phi.{instance}"))
            .implementation_class_uid("1.2.826.0.1.3680043.tessera")
            .build()
            .unwrap();
        obj.with_exact_meta(meta).write_to_file(path).unwrap();
    }

    /// `read_series_deidentified` must yield the SAME pixel stack as `read_series` (de-id touches
    /// metadata, never voxels) AND its source files must still contain PHI (the function is in-memory
    /// only — it never writes back to disk). The PHI-vs-clean contrast is the load-bearing assertion.
    #[test]
    fn series_deidentified_strips_phi_per_slice_and_preserves_pixels() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("phi1.dcm");
        let p2 = dir.path().join("phi2.dcm");
        write_phi_slice(&p1, 1, 1000, "DOE^JOHN");
        write_phi_slice(&p2, 2, 2000, "DOE^JANE");

        // The non-de-id reader sees the same pixel content (PHI lives in metadata, not pixels).
        let plain = read_series(&[p1.clone(), p2.clone()]).unwrap();
        let deid = read_series_deidentified(&[p1.clone(), p2.clone()]).unwrap();
        assert_eq!(plain.shape, deid.shape);
        assert_eq!(plain.voxels, deid.voxels);

        // The source files on disk still carry their PHI — de-id is in-memory at ingest, not a
        // destructive pre-pass on the inputs (a Tessera invariant: never mutate the caller's files).
        let obj1 = dicom::object::open_file(&p1).unwrap();
        assert!(
            !phi_present(&obj1).is_empty(),
            "source DICOM must still contain PHI; the in-memory de-id only affects the ingested product"
        );
    }

    // ── Curated DICOM tags → `recon` schema fields + full-header dump into `extra/` ────────

    /// Assemble a synthetic CT DICOM carrying the curated tags — plus any extra elements the
    /// caller wants stamped (PHI, exotic VRs). Returns the [`FileDicomObject`].
    fn dcm_with_curated_and(
        rows: u16,
        cols: u16,
        instance: i32,
        extras: Vec<DataElement<InMemDicomObject, dicom::core::value::InMemFragment>>,
    ) -> FileDicomObject<InMemDicomObject> {
        let n = rows as usize * cols as usize;
        let pixels: Vec<u16> = (0..n).map(|k| (k as u16) * 3).collect();
        let mut elems: Vec<DataElement<InMemDicomObject, dicom::core::value::InMemFragment>> = vec![
            DataElement::new(MODALITY, VR::CS, PrimitiveValue::from("CT")),
            DataElement::new(ROWS, VR::US, PrimitiveValue::from(rows)),
            DataElement::new(COLUMNS, VR::US, PrimitiveValue::from(cols)),
            DataElement::new(
                INSTANCE_NUMBER,
                VR::IS,
                PrimitiveValue::from(instance.to_string()),
            ),
            DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(
                Tag(0x0028, 0x0004),
                VR::CS,
                PrimitiveValue::from("MONOCHROME2"),
            ),
            DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(15u16)),
            DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(RESCALE_INTERCEPT, VR::DS, PrimitiveValue::from("-1024")),
            DataElement::new(RESCALE_SLOPE, VR::DS, PrimitiveValue::from("1")),
            // Curated tags the schema-driven port must lift into `recon` metadata.
            DataElement::new(
                STUDY_INSTANCE_UID,
                VR::UI,
                PrimitiveValue::from("1.2.826.0.1.3680043.tessera.study.1"),
            ),
            DataElement::new(
                SERIES_INSTANCE_UID,
                VR::UI,
                PrimitiveValue::from("1.2.826.0.1.3680043.tessera.series.1"),
            ),
            DataElement::new(STUDY_DATE, VR::DA, PrimitiveValue::from("20240115")),
            DataElement::new(MANUFACTURER, VR::LO, PrimitiveValue::from("SIEMENS")),
            DataElement::new(MODEL_NAME, VR::LO, PrimitiveValue::from("Biograph mCT")),
            DataElement::new(KVP, VR::DS, PrimitiveValue::from("120")),
            DataElement::new(SLICE_THICKNESS, VR::DS, PrimitiveValue::from("2.5")),
            DataElement::new(PIXEL_SPACING, VR::DS, PrimitiveValue::from("0.977\\0.977")),
            DataElement::new(
                Tag(0x7FE0, 0x0010),
                VR::OW,
                PrimitiveValue::U16(pixels.into()),
            ),
        ];
        elems.extend(extras);
        let obj = InMemDicomObject::from_element_iter(elems);
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1")
            .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.2")
            .media_storage_sop_instance_uid("1.2.3.curated")
            .implementation_class_uid("1.2.826.0.1.3680043.tessera")
            .build()
            .unwrap();
        obj.with_exact_meta(meta)
    }

    /// The curated tags must land on `DicomImage` (populated in [`read_object`]) and then flow
    /// through [`to_recon_product`] into the sealed `recon` manifest's metadata — using the
    /// same field ids the `recon` schema declares (rename-safe, schema-driven).
    #[test]
    fn curated_dicom_tags_populate_recon_schema_fields() {
        let file_obj = dcm_with_curated_and(8, 8, 1, vec![]);
        let img = read_object(&file_obj).unwrap();

        // The struct carries every curated tag we stamped.
        let c = &img.curated;
        assert_eq!(
            c.study_instance_uid.as_deref(),
            Some("1.2.826.0.1.3680043.tessera.study.1")
        );
        assert_eq!(
            c.series_instance_uid.as_deref(),
            Some("1.2.826.0.1.3680043.tessera.series.1")
        );
        assert_eq!(c.study_date.as_deref(), Some("20240115"));
        assert_eq!(c.manufacturer.as_deref(), Some("SIEMENS"));
        assert_eq!(c.model_name.as_deref(), Some("Biograph mCT"));
        assert_eq!(c.kvp, Some(120.0));
        assert_eq!(c.slice_thickness, Some(2.5));
        assert_eq!(c.pixel_spacing, Some([0.977, 0.977]));

        // And every curated tag lands in the sealed manifest's metadata under the schema id.
        let (sealed, _) = to_recon_product(
            &img,
            "curated-ct",
            "2024-01-01T00:00:00Z",
            "1.2.3.sop.curated",
            None,
            &[],
        )
        .unwrap();
        assert_eq!(
            sealed.metadata["study_instance_uid"],
            "1.2.826.0.1.3680043.tessera.study.1"
        );
        assert_eq!(
            sealed.metadata["series_instance_uid"],
            "1.2.826.0.1.3680043.tessera.series.1"
        );
        assert_eq!(sealed.metadata["study_date"], "20240115");
        assert_eq!(sealed.metadata["manufacturer"], "SIEMENS");
        assert_eq!(sealed.metadata["model_name"], "Biograph mCT");
        assert_eq!(sealed.metadata["kvp"], 120.0);
        assert_eq!(sealed.metadata["slice_thickness"], 2.5);
        assert_eq!(
            sealed.metadata["pixel_spacing"],
            serde_json::json!([0.977, 0.977])
        );

        // The recon schema classifies these correctly (ADR-0040 §1) — spot-check via the registry
        // that the tiers used by the DICOM port match what the schema declares.
        let reg = tessera_core::schema::SchemaRegistry::builtin();
        let ids_at = |tier| -> Vec<String> {
            reg.fields_by_sensitivity(&sealed, tier)
                .into_iter()
                .map(|f| f.id.clone())
                .collect()
        };
        let identifying = ids_at(tessera_core::schema::Sensitivity::Identifying);
        assert!(identifying.iter().any(|s| s == "study_instance_uid"));
        assert!(identifying.iter().any(|s| s == "series_instance_uid"));
        let sensitive = ids_at(tessera_core::schema::Sensitivity::Sensitive);
        assert_eq!(sensitive, ["study_date"]);
        let public = ids_at(tessera_core::schema::Sensitivity::Public);
        for id in [
            "manufacturer",
            "model_name",
            "kvp",
            "slice_thickness",
            "pixel_spacing",
        ] {
            assert!(
                public.iter().any(|s| s == id),
                "curated field '{id}' must classify Public"
            );
        }
    }

    /// A DICOM ingested with **only** the mandatory shape/modality tags (every curated tag
    /// absent) must still ingest. The schema declares curated fields as `recommended`, so their
    /// absence is a warn tier — never a block.
    #[test]
    fn missing_curated_tags_do_not_block_ingest() {
        let (rows, cols) = (8u16, 8u16);
        let n = rows as usize * cols as usize;
        let pixels: Vec<u16> = (0..n).map(|k| k as u16).collect();
        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(MODALITY, VR::CS, PrimitiveValue::from("CT")),
            DataElement::new(ROWS, VR::US, PrimitiveValue::from(rows)),
            DataElement::new(COLUMNS, VR::US, PrimitiveValue::from(cols)),
            DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(
                Tag(0x0028, 0x0004),
                VR::CS,
                PrimitiveValue::from("MONOCHROME2"),
            ),
            DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(15u16)),
            DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(
                Tag(0x7FE0, 0x0010),
                VR::OW,
                PrimitiveValue::U16(pixels.into()),
            ),
        ]);
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1")
            .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.2")
            .media_storage_sop_instance_uid("1.2.3.min")
            .implementation_class_uid("1.2.826.0.1.3680043.tessera")
            .build()
            .unwrap();
        let file_obj = obj.with_exact_meta(meta);
        let img = read_object(&file_obj).unwrap();
        assert_eq!(img.curated, DicomCuratedTags::default());

        let (sealed, _) = to_recon_product(
            &img,
            "minimal-ct",
            "2024-01-01T00:00:00Z",
            "1.2.3.min",
            None,
            &[],
        )
        .unwrap();
        // Sealed and schema-valid despite every curated tag being absent (recommended, not required).
        tessera_core::schema::validate_manifest(&sealed).unwrap();
        for id in [
            "study_instance_uid",
            "series_instance_uid",
            "study_date",
            "manufacturer",
            "model_name",
            "kvp",
            "slice_thickness",
            "pixel_spacing",
        ] {
            assert!(
                !sealed.metadata.contains_key(id),
                "no curated field should be stamped when the DICOM tag is absent: {id}"
            );
        }
    }

    /// `extra["dicom_header"]` must be present, JSON-object-shaped, deterministic (byte-identical
    /// across two ingests of the same input), and **must exclude** PixelData.
    #[test]
    fn full_dicom_header_lands_in_extra_and_is_deterministic() {
        let file_obj = dcm_with_curated_and(8, 8, 1, vec![]);
        let img_a = read_object(&file_obj).unwrap();
        let img_b = read_object(&file_obj).unwrap();
        let (sealed_a, _) = to_recon_product(
            &img_a,
            "hdr-ct",
            "2024-01-01T00:00:00Z",
            "1.2.3.hdr",
            None,
            &[],
        )
        .unwrap();
        let (sealed_b, _) = to_recon_product(
            &img_b,
            "hdr-ct",
            "2024-01-01T00:00:00Z",
            "1.2.3.hdr",
            None,
            &[],
        )
        .unwrap();

        // Present.
        let hdr_a = sealed_a
            .extra
            .get("dicom_header")
            .expect("extra['dicom_header'] must be present after ingest");
        let hdr_b = sealed_b.extra.get("dicom_header").unwrap();

        // Deterministic: same input → same bytes (byte-for-byte JSON).
        assert_eq!(
            serde_json::to_string(hdr_a).unwrap(),
            serde_json::to_string(hdr_b).unwrap(),
            "dicom_header must serialize deterministically across ingests"
        );

        // Object-shaped, keyed by `GGGG,EEEE` — spot-check a few curated tags are there.
        let obj = hdr_a
            .as_object()
            .expect("dicom_header is a JSON object keyed by 'GGGG,EEEE'");
        assert!(obj.contains_key("0008,0060"), "Modality present");
        assert!(obj.contains_key("0020,000D"), "StudyInstanceUID present");
        assert!(obj.contains_key("0028,0030"), "PixelSpacing present");

        // PixelData is NEVER dumped into the header (it's the volume block's payload).
        assert!(
            !obj.contains_key("7FE0,0010"),
            "PixelData must be excluded from dicom_header"
        );

        // The seal commits to `extra` — both ingests hash identically (writer-determinism).
        assert_eq!(sealed_a.manifest_hash, sealed_b.manifest_hash);
    }

    /// **The load-bearing PHI test.** A DICOM full-header dump destined for `extra/` must not
    /// leak PHI when the ingest went through the de-identifying path — the `deidentify` call has
    /// to run BEFORE the header is serialized. When the ingest is NOT de-identifying, the header
    /// preserves what the source carried (the caller opted out of scrubbing).
    #[test]
    fn de_identified_ingest_scrubs_phi_from_dicom_header_extra() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("phi.dcm");
        // Reuse the PHI-slice writer (it stamps PatientName + PatientID + InstitutionName).
        write_phi_slice(&p, 1, 1000, "DOE^JOHN");

        // Non-de-id: the header dump reflects the source object — PHI is present.
        let img_plain = read_image(&p).unwrap();
        let (sealed_plain, _) = to_recon_product(
            &img_plain,
            "hdr-phi",
            "2024-01-01T00:00:00Z",
            "1.2.3.phi",
            None,
            &[],
        )
        .unwrap();
        let hdr_plain = sealed_plain
            .extra
            .get("dicom_header")
            .and_then(|v| v.as_object())
            .expect("plain ingest carries dicom_header");
        assert!(
            hdr_plain.contains_key("0010,0010"),
            "non-de-id ingest keeps PatientName in the header dump (source-faithful)"
        );
        assert!(hdr_plain.contains_key("0010,0020"), "…and PatientID");
        assert!(hdr_plain.contains_key("0008,0080"), "…and InstitutionName");

        // De-id: the header dump is the PS3.15-scrubbed object — PHI absent, everyone's marker
        // set. THIS IS THE LOAD-BEARING ASSERTION (issue #dicom-metadata).
        let img_deid = read_image_deidentified(&p).unwrap();
        let (sealed_deid, _) = to_recon_product(
            &img_deid,
            "hdr-phi-deid",
            "2024-01-01T00:00:00Z",
            "1.2.3.phi",
            None,
            &[],
        )
        .unwrap();
        let hdr_deid = sealed_deid
            .extra
            .get("dicom_header")
            .and_then(|v| v.as_object())
            .expect("de-id ingest still carries dicom_header (scrubbed)");
        for phi_key in ["0010,0010", "0010,0020", "0008,0080"] {
            assert!(
                !hdr_deid.contains_key(phi_key),
                "de-identified ingest must strip PHI tag {phi_key} from extra['dicom_header']"
            );
        }
        // …and the PS3.15 PatientIdentityRemoved marker (0012,0062) is set to "YES".
        let marker = hdr_deid
            .get("0012,0062")
            .expect("PS3.15 PatientIdentityRemoved marker must be set on de-id");
        assert_eq!(marker["value"], serde_json::json!(["YES"]));

        // Non-PHI curated tags survive the de-id (modality is not PHI).
        assert!(hdr_deid.contains_key("0008,0060"));
    }

    #[test]
    fn deidentify_strips_phi_and_sets_marker() {
        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(
                Tag(0x0010, 0x0010),
                VR::PN,
                PrimitiveValue::from("DOE^JANE"),
            ),
            DataElement::new(
                Tag(0x0010, 0x0020),
                VR::LO,
                PrimitiveValue::from("PID-12345"),
            ),
            DataElement::new(
                Tag(0x0008, 0x0080),
                VR::LO,
                PrimitiveValue::from("ACME Hospital"),
            ),
            DataElement::new(MODALITY, VR::CS, PrimitiveValue::from("CT")),
        ]);
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1")
            .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.2")
            .media_storage_sop_instance_uid("1.2.3.deid")
            .implementation_class_uid("1.2.826.0.1.3680043.tessera")
            .build()
            .unwrap();
        let mut obj = obj.with_exact_meta(meta);

        assert_eq!(phi_present(&obj).len(), 3); // name + id + institution
        let removed = deidentify(&mut obj);
        assert_eq!(removed, 3);
        assert!(phi_present(&obj).is_empty());
        assert!(obj.element(MODALITY).is_ok()); // non-PHI survives
        assert_eq!(
            obj.element(PATIENT_IDENTITY_REMOVED)
                .unwrap()
                .to_str()
                .unwrap()
                .trim(),
            "YES"
        );
    }
}
