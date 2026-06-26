//! DICOM ingest (#207) — read a DICOM image into a Tessera `recon` product, **losslessly**: the
//! native integer samples are preserved as-is (no rescale baked in); the DICOM rescale slope/
//! intercept, modality, and physical unit are carried as metadata so a reader recovers HU / Bq·mL⁻¹.
//! Uncompressed and RLE transfer syntaxes decode pure-Rust (no C++ codecs).

use dicom::core::Tag;
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
    use dicom::core::{DataElement, PrimitiveValue, VR};
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
}
