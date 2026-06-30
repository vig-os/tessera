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

/// A DICOM image decoded for Tessera: native int16 samples (C order, `[rows, cols]`) plus the
/// metadata needed to recover physical units. The rescale is **not** applied to `voxels`.
#[derive(Debug, Clone, PartialEq)]
pub struct DicomImage {
    pub shape: Vec<u64>,
    pub voxels: Vec<i16>,
    pub modality: String,
    pub rescale_slope: f64,
    pub rescale_intercept: f64,
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
    let rows = obj.element(ROWS).map_err(de)?.to_int::<u32>().map_err(de)? as u64;
    let cols = obj
        .element(COLUMNS)
        .map_err(de)?
        .to_int::<u32>()
        .map_err(de)? as u64;
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
    Ok(DicomImage {
        shape: vec![rows, cols],
        voxels,
        modality,
        rescale_slope,
        rescale_intercept,
    })
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
    Ok(DicomImage {
        shape: vec![z, first.shape[0], first.shape[1]],
        voxels,
        modality: first.modality,
        rescale_slope: first.rescale_slope,
        rescale_intercept: first.rescale_intercept,
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
    b.add_source(tessera_core::provenance::Source::new(
        "ingested_from",
        source,
    ));
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
        }
    }

    #[test]
    fn to_recon_product_carries_rescale_modality_and_provenance() {
        let img = sample_image();
        let (sealed, payloads) =
            to_recon_product(&img, "DP06-ct", "2024-01-01T00:00:00Z", "1.2.3.sop", &[]).unwrap();
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
        let (a, _) =
            to_recon_product(&img, "golden-ct", "2024-01-01T00:00:00Z", "sop-a", &[]).unwrap();
        // a different timestamp + source must NOT change the content hash (content-addressing)
        let (b, _) =
            to_recon_product(&img, "golden-ct", "2099-12-31T23:59:59Z", "sop-b", &[]).unwrap();
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
