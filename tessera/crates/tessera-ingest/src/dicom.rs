//! DICOM ingest (#207) — read a DICOM image into a Tessera `recon` product, **losslessly**: the
//! native integer samples are preserved as-is (no rescale baked in); the DICOM rescale slope/
//! intercept, modality, and physical unit are carried as metadata so a reader recovers HU / Bq·mL⁻¹.
//! Uncompressed and RLE transfer syntaxes decode pure-Rust (no C++ codecs).

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
    if paths.is_empty() {
        return Err(Error::Invalid("dicom: empty series".into()));
    }
    let mut slices: Vec<(i64, DicomImage)> = Vec::with_capacity(paths.len());
    for p in paths {
        let obj = dicom::object::open_file(p).map_err(de)?;
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
    Ok(DicomImage {
        shape: vec![slices.len() as u64, first.shape[0], first.shape[1]],
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
pub fn to_recon_product(
    img: &DicomImage,
    name: &str,
    timestamp: &str,
    source: &str,
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
            to_recon_product(&img, "DP06-ct", "2024-01-01T00:00:00Z", "1.2.3.sop").unwrap();
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
