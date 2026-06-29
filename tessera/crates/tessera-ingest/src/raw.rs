//! Raw **headerless binary** ingest (#208) — a flat little-endian voxel buffer plus a caller-supplied
//! shape & dtype → a Tessera `recon` product. The escape hatch for the long tail of vendor dumps that
//! carry no self-describing header (a `.dat`/`.bin`/`.raw` blob): the operator knows the geometry, and
//! Tessera carries it **losslessly** (native dtype, no re-encode of the values).

use std::path::Path;

use tessera_core::block::array::ArraySpec;
use tessera_core::manifest::Manifest;
use tessera_core::{Error, ProductBuilder, Result};
use tessera_io::array::{self, ArrayData};
use tessera_io::BlockPayload;

fn he(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("raw: {e}"))
}

/// Read a raw little-endian binary file as an array of `numpy_code` dtype (`i2`/`i4`/`i8`, `u2`/`u4`/`u8`,
/// `f4`/`f8`) with the given C-order `shape`. Errors if the element count doesn't match the shape product
/// (a guard against a wrong dtype/shape silently mis-reading the buffer).
pub fn read_raw(path: &Path, shape: Vec<u64>, numpy_code: &str) -> Result<(ArraySpec, ArrayData)> {
    let bytes = std::fs::read(path).map_err(he)?;
    let data = ArrayData::from_le_bytes(numpy_code, &bytes)?;
    let expected: u64 = shape.iter().product();
    let got = u64::try_from(data.len()).map_err(|_| he("element count overflow"))?;
    if got != expected {
        return Err(he(format!(
            "{got} elements != shape product {expected} (wrong dtype or shape?)"
        )));
    }
    let spec = ArraySpec::new(shape, data.dtype());
    Ok((spec, data))
}

/// Read a raw binary volume and seal it as a Tessera `recon` product, with an `ingested_from` provenance
/// edge to the source file. `extra_sources` flow in AFTER `ingested_from` (the declarative ingest engine
/// threads `derived_from` + `ingested_via_spec` edges here so the chain verifier picks up the parent's
/// `manifest_hash`).
pub fn to_recon_product(
    path: &Path,
    shape: Vec<u64>,
    numpy_code: &str,
    name: &str,
    timestamp: &str,
    extra_sources: &[tessera_core::provenance::Source],
) -> Result<(Manifest, Vec<BlockPayload>)> {
    let (spec, data) = read_raw(path, shape, numpy_code)?;
    let (block_ref, payload) = array::array_block("volume", &spec, &data)?;
    let mut b = ProductBuilder::new("recon", name, "raw binary volume", timestamp);
    b.add_block_ref(block_ref);
    b.add_source(tessera_core::provenance::Source::new(
        "ingested_from",
        path.display().to_string(),
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

    #[test]
    fn reads_raw_binary_with_shape_and_dtype_and_rejects_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("vol.raw");
        // 2×2×2 int16 volume, little-endian.
        let voxels: Vec<i16> = (0..8).map(|k| k as i16 * 7 - 100).collect();
        let bytes: Vec<u8> = voxels.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(&f, &bytes).unwrap();

        let (spec, data) = read_raw(&f, vec![2, 2, 2], "i2").unwrap();
        assert_eq!(spec.shape, vec![2, 2, 2]);
        assert_eq!(spec.dtype, "int16");
        assert!(matches!(data, ArrayData::I16(ref v) if v == &voxels));

        // a wrong shape (element-count mismatch) is rejected, not silently mis-read.
        assert!(read_raw(&f, vec![2, 2, 4], "i2").is_err());
        // a wrong dtype width (4 i32 ≠ 8 i16-worth of bytes) is rejected.
        assert!(read_raw(&f, vec![2, 2, 2], "i4").is_err());

        // seals a verifying recon product.
        let (m, _p) = to_recon_product(
            &f,
            vec![2, 2, 2],
            "i2",
            "raw-01",
            "2024-01-01T00:00:00Z",
            &[],
        )
        .unwrap();
        m.verify().unwrap();
        assert_eq!(m.product, "recon");
    }
}
