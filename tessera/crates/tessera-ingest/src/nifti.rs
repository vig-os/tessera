//! NIfTI-1 ingest — read a `.nii` neuroimaging volume into a Tessera `recon` product (#208), lossless
//! (native dtype preserved) with the spatial `world_frame` derived from the NIfTI **sform** affine and
//! the `scl_slope`/`scl_inter` rescale carried as the value transform.
//!
//! NIfTI-1 single-file: a fixed **348-byte header** then the voxel data at `vox_offset` (352 for `.nii`).
//! NIfTI stores voxels **x-fastest** and its sform affine is **RAS+**; Tessera arrays are C-order
//! (last axis fastest) and **LPS canonical** (ADR-0030 §6). So the volume is declared with axes
//! `[z,y,x]` (x fastest — matches NIfTI's storage byte-for-byte, no transpose) and the affine is
//! reordered to `[k,j,i]` columns + converted RAS→LPS (negate the world x,y rows) at the door.

use tessera_core::block::array::{ArraySpec, WorldFrame};
use tessera_core::manifest::Manifest;
use tessera_core::{Error, ProductBuilder, Result};
use tessera_io::array::{self, ArrayData};
use tessera_io::BlockPayload;

fn he(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("nifti: {e}"))
}

/// A decoded NIfTI volume: shape in Tessera `[z,y,x]` order, native voxels, the LPS `world_frame`, and
/// the intensity rescale.
pub struct NiftiImage {
    pub shape: Vec<u64>,
    pub data: ArrayData,
    pub world_frame: Option<WorldFrame>,
    pub rescale_slope: f64,
    pub rescale_intercept: f64,
}

fn i16le(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}
fn i32le(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn f32le(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
/// NIfTI stores `vox_offset` (and dims, via floats) as numbers that are really integers; convert the
/// float byte-offset to `usize` (the one inherently-lossy NIfTI field — guarded to ≥ 0).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn f32_to_usize(v: f32) -> usize {
    v.max(0.0) as usize
}
/// A NIfTI `i16` size/count field → `u64`; negative is invalid.
fn dim_u64(v: i16) -> Result<u64> {
    u64::try_from(v).map_err(|_| he("negative NIfTI dimension"))
}

/// Read a NIfTI-1 `.nii` file (little-endian) into a [`NiftiImage`]. Supports the common datatypes
/// (uint8/int16/uint16/int32/float32/float64). Errors on a non-NIfTI-1 / big-endian / unsupported file.
pub fn read_nifti(path: &std::path::Path) -> Result<NiftiImage> {
    let b = std::fs::read(path).map_err(he)?;
    if b.len() < 352 {
        return Err(he("file shorter than a NIfTI-1 header"));
    }
    if i32le(&b, 0) != 348 {
        return Err(he(
            "sizeof_hdr != 348 (not NIfTI-1, or big-endian — unsupported)",
        ));
    }
    // magic "n+1\0" at offset 344 marks a single-file .nii.
    if &b[344..347] != b"n+1" {
        return Err(he(
            "magic is not 'n+1' (only single-file .nii is supported)",
        ));
    }
    let ndim = i16le(&b, 40);
    if !(1..=7).contains(&ndim) {
        return Err(he("dim[0] (ndim) out of range"));
    }
    // Tessera C-order [z,y,x] (x fastest) == NIfTI storage; declare shape reversed from NIfTI [x,y,z].
    let dim_at = |k: usize| dim_u64(i16le(&b, 40 + 2 * k));
    let nx = dim_at(1)?;
    let ny = if ndim >= 2 { dim_at(2)? } else { 1 };
    let nz = if ndim >= 3 { dim_at(3)? } else { 1 };
    let shape = vec![nz, ny, nx];
    let n = usize::try_from(nx * ny * nz).map_err(|_| he("dim product overflow"))?;

    let datatype = i16le(&b, 70);
    let vox_offset = f32_to_usize(f32le(&b, 108));
    let scl_slope = f32le(&b, 112) as f64;
    let scl_inter = f32le(&b, 116) as f64;
    let (rescale_slope, rescale_intercept) = if scl_slope != 0.0 {
        (scl_slope, scl_inter)
    } else {
        (1.0, 0.0)
    };

    let d = &b[vox_offset..];
    macro_rules! read_vec {
        ($w:expr, $variant:ident, $from:expr) => {{
            if d.len() < n * $w {
                return Err(he("voxel data shorter than dim product"));
            }
            ArrayData::$variant((0..n).map(|i| $from(&d[i * $w..])).collect())
        }};
    }
    let data = match datatype {
        // Tessera's dtype floor is 16-bit (ADR — 8-bit out of scope), so NIfTI uint8 (e.g. masks)
        // widens losslessly to uint16.
        2 => read_vec!(1, U16, |s: &[u8]| u16::from(s[0])),
        4 => read_vec!(2, I16, |s: &[u8]| i16::from_le_bytes([s[0], s[1]])),
        512 => read_vec!(2, U16, |s: &[u8]| u16::from_le_bytes([s[0], s[1]])),
        8 => read_vec!(4, I32, |s: &[u8]| i32::from_le_bytes([
            s[0], s[1], s[2], s[3]
        ])),
        16 => read_vec!(4, F32, |s: &[u8]| f32::from_le_bytes([
            s[0], s[1], s[2], s[3]
        ])),
        64 => read_vec!(8, F64, |s: &[u8]| f64::from_le_bytes([
            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]
        ])),
        other => return Err(he(format!("unsupported NIfTI datatype code {other}"))),
    };

    // sform (preferred) → LPS world_frame. srow_x/y/z at 280/296/312, each f32[4] = [Mi,Mj,Mk,off].
    let sform_code = i16le(&b, 254);
    let world_frame = (sform_code > 0).then(|| {
        let srow = |off: usize| {
            [
                f32le(&b, off) as f64,
                f32le(&b, off + 4) as f64,
                f32le(&b, off + 8) as f64,
                f32le(&b, off + 12) as f64,
            ]
        };
        let sx = srow(280);
        let sy = srow(296);
        let sz = srow(312);
        // Reorder columns [i,j,k]→[k,j,i] (Tessera [z,y,x]) and RAS→LPS (negate world x,y rows).
        WorldFrame {
            affine: [
                -sx[2], -sx[1], -sx[0], -sx[3], // world L = -RAS x
                -sy[2], -sy[1], -sy[0], -sy[3], // world P = -RAS y
                sz[2], sz[1], sz[0], sz[3], // world S =  RAS z
            ],
            convention: "LPS".into(),
            unit: "mm".into(),
            space: "scanner".into(),
        }
    });

    Ok(NiftiImage {
        shape,
        data,
        world_frame,
        rescale_slope,
        rescale_intercept,
    })
}

/// Build a sealed Tessera `recon` product from a decoded NIfTI volume, with the `world_frame`/rescale on
/// the array spec and an `ingested_from` provenance edge to the source `.nii`. `extra_sources` flow in
/// AFTER `ingested_from` (the declarative ingest engine threads `derived_from` + `ingested_via_spec`
/// edges here so the chain verifier picks up the parent's `manifest_hash`).
pub fn to_recon_product(
    img: &NiftiImage,
    name: &str,
    timestamp: &str,
    source: &str,
    extra_sources: &[tessera_core::provenance::Source],
) -> Result<(Manifest, Vec<BlockPayload>)> {
    let mut spec = ArraySpec::new(img.shape.clone(), img.data.dtype())
        .with_rescale(img.rescale_slope, img.rescale_intercept);
    spec.world_frame = img.world_frame.clone();
    let (block_ref, payload) = array::array_block("volume", &spec, &img.data)?;

    let mut b = ProductBuilder::new("recon", name, "NIfTI recon volume", timestamp);
    b.add_block_ref(block_ref);
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

    /// Write a minimal little-endian NIfTI-1 `.nii`: int16, dims [nx,ny,nz], a diagonal sform.
    fn write_synth_nifti(path: &std::path::Path, nx: i16, ny: i16, nz: i16, voxels: &[i16]) {
        let mut h = vec![0u8; 352];
        h[0..4].copy_from_slice(&348i32.to_le_bytes()); // sizeof_hdr
        h[40..42].copy_from_slice(&3i16.to_le_bytes()); // dim[0] = 3
        h[42..44].copy_from_slice(&nx.to_le_bytes()); // dim[1] = nx
        h[44..46].copy_from_slice(&ny.to_le_bytes()); // dim[2] = ny
        h[46..48].copy_from_slice(&nz.to_le_bytes()); // dim[3] = nz
        h[70..72].copy_from_slice(&4i16.to_le_bytes()); // datatype = int16
        h[72..74].copy_from_slice(&16i16.to_le_bytes()); // bitpix
        h[108..112].copy_from_slice(&352f32.to_le_bytes()); // vox_offset
        h[112..116].copy_from_slice(&1f32.to_le_bytes()); // scl_slope = 1
        h[254..256].copy_from_slice(&1i16.to_le_bytes()); // sform_code = 1
                                                          // diagonal sform: spacing (2,3,4), offset (10,20,30) in RAS.
        h[280..296].copy_from_slice(
            &[2f32, 0., 0., 10.]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        h[296..312].copy_from_slice(
            &[0f32, 3., 0., 20.]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        h[312..328].copy_from_slice(
            &[0f32, 0., 4., 30.]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        h[344..348].copy_from_slice(b"n+1\0"); // magic
        let mut bytes = h;
        for v in voxels {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn reads_nifti_volume_with_lps_world_frame_and_builds_recon() {
        let dir = tempfile::tempdir().unwrap();
        let nii = dir.path().join("brain.nii");
        let voxels: Vec<i16> = (0..2 * 2 * 2).map(|k| k as i16 - 100).collect();
        write_synth_nifti(&nii, 2, 2, 2, &voxels);

        let img = read_nifti(&nii).unwrap();
        // Tessera [z,y,x] shape, native int16, values byte-identical (x fastest = no transpose).
        assert_eq!(img.shape, vec![2, 2, 2]);
        assert!(matches!(img.data, ArrayData::I16(ref v) if v == &voxels));

        // the LPS world_frame: voxel [z=1,y=1,x=1] → world (-12,-23,34) (verified vs the RAS sform).
        let wf = img.world_frame.clone().unwrap();
        assert_eq!(wf.convention, "LPS");
        assert_eq!(wf.voxel_to_world([1.0, 1.0, 1.0]), [-12.0, -23.0, 34.0]);
        // spacing is the column norms = (4,3,2) for axes [z,y,x].
        assert_eq!(wf.spacing(), [4.0, 3.0, 2.0]);

        // builds a sealed, verifying recon product.
        let (m, _payloads) =
            to_recon_product(&img, "brain-01", "2024-01-01T00:00:00Z", "brain.nii", &[]).unwrap();
        m.verify().unwrap();
        assert_eq!(m.product, "recon");
        assert_eq!(m.sources[0].reference, "brain.nii");

        // a non-NIfTI file is rejected.
        let bad = dir.path().join("bad.nii");
        std::fs::write(&bad, vec![0u8; 400]).unwrap();
        assert!(read_nifti(&bad).is_err());
    }
}
