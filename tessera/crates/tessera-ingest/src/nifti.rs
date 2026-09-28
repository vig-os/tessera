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
    source_digest: Option<&str>,
    extra_sources: &[tessera_core::provenance::Source],
) -> Result<(Manifest, Vec<BlockPayload>)> {
    let mut spec = ArraySpec::new(img.shape.clone(), img.data.dtype())
        .with_rescale(img.rescale_slope, img.rescale_intercept);
    spec.world_frame = img.world_frame.clone();
    let (block_ref, payload) = array::array_block("volume", &spec, &img.data)?;

    let mut b = ProductBuilder::new("recon", name, "NIfTI recon volume", timestamp);
    b.add_block_ref(block_ref);
    // `ingested_from` carries a source `content_hash` (blake3 of the `.nii`) when the caller supplies
    // it — the integrity link to the source-of-record, independent of the `source` reference.
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

    /// A synthetic little-endian NIfTI-1 fixture builder. It writes **every** header field itself, so a
    /// fixture can never carry real (PHI-bearing) data, and it covers the whole surface the #396
    /// regression tests drive: dims 1..7, sform *and* qform, `vox_offset`, and the time axis. The
    /// defaults reproduce the original 3-D int16 sform-only fixture byte-for-byte, so the conformance
    /// test below is unaffected.
    struct Synth {
        dim: [i16; 8],
        /// `pixdim[0]` is the qform handedness `qfac`; `pixdim[1..4]` the (2,3,4) mm spacing.
        pixdim: [f32; 8],
        datatype: i16,
        vox_offset: f32,
        sform_code: i16,
        srows: [[f32; 4]; 3],
        qform_code: i16,
        quatern: [f32; 3],
        qoffset: [f32; 3],
        xyzt_units: u8,
        toffset: f32,
    }

    fn put_i16(h: &mut [u8], off: usize, v: i16) {
        h[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put_i32(h: &mut [u8], off: usize, v: i32) {
        h[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put_f32(h: &mut [u8], off: usize, v: f32) {
        h[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    impl Synth {
        /// `dims` are in **NIfTI order** (`[nx, ny, nz, nt, …]`); `dim[0]` (ndim) is its length.
        fn new(dims: &[i16]) -> Self {
            let mut dim = [1i16; 8];
            dim[0] = i16::try_from(dims.len()).unwrap();
            dim[1..=dims.len()].copy_from_slice(dims);
            Synth {
                dim,
                pixdim: [1.0, 2.0, 3.0, 4.0, 0.0, 0.0, 0.0, 0.0],
                datatype: 4, // int16
                vox_offset: 352.0,
                sform_code: 1,
                // diagonal sform: spacing (2,3,4), offset (10,20,30) in RAS.
                srows: [[2., 0., 0., 10.], [0., 3., 0., 20.], [0., 0., 4., 30.]],
                qform_code: 0,
                quatern: [0.0; 3],
                qoffset: [10.0, 20.0, 30.0],
                xyzt_units: 0,
                toffset: 0.0,
            }
        }
        fn sform(mut self, code: i16) -> Self {
            self.sform_code = code;
            self
        }
        fn no_sform(mut self) -> Self {
            self.sform_code = 0;
            self
        }
        /// Set the qform: its code, the `quatern_b/c/d` triple, and the `pixdim[0]` handedness.
        fn qform(mut self, code: i16, quatern: [f32; 3], qfac: f32) -> Self {
            self.qform_code = code;
            self.quatern = quatern;
            self.pixdim[0] = qfac;
            self
        }
        fn vox_offset(mut self, v: f32) -> Self {
            self.vox_offset = v;
            self
        }
        /// `pixdim[4]` (the t step), `toffset`, and the `xyzt_units` byte that names their unit.
        fn time_axis(mut self, dt: f32, toffset: f32, xyzt_units: u8) -> Self {
            self.pixdim[4] = dt;
            self.toffset = toffset;
            self.xyzt_units = xyzt_units;
            self
        }

        fn header(&self) -> Vec<u8> {
            let mut h = vec![0u8; 352]; // the 348-byte header + the 4-byte magic
            put_i32(&mut h, 0, 348); // sizeof_hdr
            for (k, d) in self.dim.iter().enumerate() {
                put_i16(&mut h, 40 + 2 * k, *d);
            }
            put_i16(&mut h, 70, self.datatype);
            put_i16(&mut h, 72, 16); // bitpix — int16 fixtures only
            for (k, v) in self.pixdim.iter().enumerate() {
                put_f32(&mut h, 76 + 4 * k, *v);
            }
            put_f32(&mut h, 108, self.vox_offset);
            put_f32(&mut h, 112, 1.0); // scl_slope = 1
            h[123] = self.xyzt_units;
            put_f32(&mut h, 136, self.toffset);
            put_i16(&mut h, 252, self.qform_code);
            put_i16(&mut h, 254, self.sform_code);
            for (i, v) in self.quatern.iter().enumerate() {
                put_f32(&mut h, 256 + 4 * i, *v);
            }
            for (i, v) in self.qoffset.iter().enumerate() {
                put_f32(&mut h, 268 + 4 * i, *v);
            }
            for (r, row) in self.srows.iter().enumerate() {
                for (c, v) in row.iter().enumerate() {
                    put_f32(&mut h, 280 + 16 * r + 4 * c, *v);
                }
            }
            h[344..348].copy_from_slice(b"n+1\0"); // magic
            h
        }

        fn bytes(&self, voxels: &[i16]) -> Vec<u8> {
            let mut b = self.header();
            for v in voxels {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b
        }

        fn write(&self, path: &std::path::Path, voxels: &[i16]) {
            std::fs::write(path, self.bytes(voxels)).unwrap();
        }
    }

    /// Write a minimal little-endian NIfTI-1 `.nii`: int16, dims [nx,ny,nz], a diagonal sform.
    fn write_synth_nifti(path: &std::path::Path, nx: i16, ny: i16, nz: i16, voxels: &[i16]) {
        Synth::new(&[nx, ny, nz]).write(path, voxels);
    }

    /// Compare a world point within float32-header rounding (the qform quaternion round-trips through
    /// `f32` fields, so the reconstructed matrix is not bit-exact).
    #[track_caller]
    fn approx(got: [f64; 3], want: [f64; 3]) {
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 1e-5, "{got:?} != {want:?}");
        }
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
        let (m, _payloads) = to_recon_product(
            &img,
            "brain-01",
            "2024-01-01T00:00:00Z",
            "brain.nii",
            None,
            &[],
        )
        .unwrap();
        m.verify().unwrap();
        assert_eq!(m.product, "recon");
        assert_eq!(m.sources[0].reference, "brain.nii");

        // a non-NIfTI file is rejected.
        let bad = dir.path().join("bad.nii");
        std::fs::write(&bad, vec![0u8; 400]).unwrap();
        assert!(read_nifti(&bad).is_err());
    }

    /// **B1 (#396)** — `vox_offset` is header-controlled and was used as a slice start *before* any
    /// bounds check, so a malformed file panicked with "range start index out of range" instead of
    /// returning the typed error FEATURE-MATRIX §A promises ("never panic"). Ingest reads untrusted
    /// third-party files, so a header-driven panic is the wrong failure mode. Includes the §P2 audit of
    /// the *other* header-derived indices in this decoder — the dim product is attacker-controlled too.
    #[test]
    fn malformed_header_indices_are_typed_errors_never_panics() {
        let dir = tempfile::tempdir().unwrap();
        let voxels: Vec<i16> = (0..8).collect();
        let invalid = |name: &str, s: Synth| {
            let p = dir.path().join(name);
            s.write(&p, &voxels);
            match read_nifti(&p).map(|img| img.shape) {
                Err(Error::Invalid(msg)) => msg,
                Ok(shape) => panic!("{name}: expected Error::Invalid, decoded shape {shape:?}"),
                Err(other) => panic!("{name}: expected Error::Invalid, got {other:?}"),
            }
        };

        // the panic in the issue: `vox_offset = 1e9` with an 8-voxel payload.
        let msg = invalid("far.nii", Synth::new(&[2, 2, 2]).vox_offset(1e9));
        assert!(
            msg.contains("vox_offset"),
            "error must name the field: {msg}"
        );
        // a NaN / infinite / negative float offset must not become a wrapped or saturated index.
        for bad in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            invalid("odd.nii", Synth::new(&[2, 2, 2]).vox_offset(bad));
        }

        // the same class one level up — a dim product that overflows `u64` must be rejected, never
        // wrap into a small allocation.
        invalid("overflow.nii", Synth::new(&[32767; 5]));
        // …and one that fits but exceeds the file is a short read, not a 70 TB allocation.
        invalid("short.nii", Synth::new(&[32767, 32767, 32767]));
        // a zero or negative extent is not a NIfTI dimension.
        invalid("zero.nii", Synth::new(&[2, 0, 2]));
        invalid("neg.nii", Synth::new(&[2, -1, 2]));
    }

    /// **B2 (#396)** — `.nii.gz` is the majority of NIfTI on disk (FSL, SPM and dcm2niix all emit it by
    /// default) and used to fail with the actively misleading `sizeof_hdr != 348 (not NIfTI-1 …)`,
    /// blaming the file for not being NIfTI when it is. It must decompress transparently and seal to the
    /// **same** block `content_hash` as its gunzipped twin.
    #[test]
    fn gzipped_nifti_seals_to_the_same_content_hash_as_its_plain_twin() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let voxels: Vec<i16> = (0..4 * 3 * 2).map(|k| k as i16 - 50).collect();
        let synth = Synth::new(&[4, 3, 2]);

        let plain = dir.path().join("brain.nii");
        synth.write(&plain, &voxels);
        let gz = dir.path().join("brain.nii.gz");
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&synth.bytes(&voxels)).unwrap();
        std::fs::write(&gz, enc.finish().unwrap()).unwrap();

        let a = read_nifti(&plain).unwrap();
        let b = read_nifti(&gz).unwrap();
        assert_eq!(a.shape, b.shape);
        assert_eq!(a.world_frame, b.world_frame);
        assert!(matches!(b.data, ArrayData::I16(ref v) if v == &voxels));

        // the acceptance criterion: the sealed products are byte-identical (same `source` string, so
        // the only possible difference is the decoded volume).
        let seal = |img: &NiftiImage| {
            to_recon_product(
                img,
                "brain-01",
                "2024-01-01T00:00:00Z",
                "brain.nii",
                None,
                &[],
            )
            .unwrap()
            .0
        };
        assert_eq!(seal(&a).content_hash, seal(&b).content_hash);
    }

    /// **B3 (#396)** — only the sform was read, so a file with `sform_code == 0` and `qform_code > 0`
    /// (older SPM/FSL outputs, some dcm2niix paths) ingested with `world_frame: None`: Tessera declared
    /// the array to be in bare index space when it demonstrably is not. Precedence must be
    /// sform → qform → none, and the `pixdim[0] < 0` handedness must flip the k column.
    #[test]
    fn falls_back_to_the_qform_quaternion_when_the_sform_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let voxels: Vec<i16> = (0..8).collect();
        // A 90° rotation about the RAS x axis: quaternion (a,b,c,d) = (cos45, sin45, 0, 0), whose
        // unscaled `nifti_quatern_to_mat44` matrix is [[1,0,0],[0,0,-1],[0,1,0]]. With pixdim (2,3,4)
        // and qoffset (10,20,30) the RAS rows are [2,0,0,10] / [0,0,-4,20] / [0,3,0,30]; Tessera
        // reorders the columns [i,j,k]→[k,j,i] and flips RAS→LPS.
        let rot_x90 = [std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0];
        let read = |name: &str, s: Synth| {
            let p = dir.path().join(name);
            s.write(&p, &voxels);
            read_nifti(&p).unwrap()
        };

        let img = read(
            "qform-only.nii",
            Synth::new(&[2, 2, 2]).no_sform().qform(1, rot_x90, 1.0),
        );
        let wf = img
            .world_frame
            .expect("a qform-only file still HAS a geometry — it must not be dropped");
        assert_eq!(wf.convention, "LPS");
        assert_eq!(wf.space, "scanner"); // qform_code = 1
        approx(wf.voxel_to_world([1.0, 1.0, 1.0]), [-12.0, -16.0, 33.0]);
        approx(wf.spacing(), [4.0, 3.0, 2.0]);

        // `pixdim[0] < 0` (qfac) flips the k column — the sign convention that is easy to get wrong.
        let wf = read(
            "qform-left.nii",
            Synth::new(&[2, 2, 2]).no_sform().qform(1, rot_x90, -1.0),
        )
        .world_frame
        .unwrap();
        approx(wf.voxel_to_world([1.0, 1.0, 1.0]), [-12.0, -24.0, 33.0]);
        approx(wf.spacing(), [4.0, 3.0, 2.0]);

        // precedence: with both present the sform wins (the diagonal fixture → (-12,-23,34)).
        let wf = read("both.nii", Synth::new(&[2, 2, 2]).qform(1, rot_x90, 1.0))
            .world_frame
            .unwrap();
        approx(wf.voxel_to_world([1.0, 1.0, 1.0]), [-12.0, -23.0, 34.0]);

        // a degenerate sform is not a usable frame — fall through to the qform rather than sealing a
        // singular affine (which `ArraySpec::validate` would reject far downstream).
        let mut degenerate = Synth::new(&[2, 2, 2]).qform(1, rot_x90, 1.0);
        degenerate.srows = [[0.; 4]; 3];
        let wf = read("degenerate-sform.nii", degenerate)
            .world_frame
            .unwrap();
        approx(wf.voxel_to_world([1.0, 1.0, 1.0]), [-12.0, -16.0, 33.0]);

        // neither code set → index space, unchanged (ADR-0029 feature-by-presence).
        let img = read("none.nii", Synth::new(&[2, 2, 2]).no_sform());
        assert!(img.world_frame.is_none());
    }

    /// **B4 (#396)** — `world_frame.space` was hard-coded `"scanner"`, mislabelling every aligned /
    /// Talairach / MNI volume. NIfTI's `sform_code`/`qform_code` value *is* the frame identity, and
    /// `"atlas:<id>"` is already in the ADR-0030 vocabulary for the template cases.
    #[test]
    fn xform_code_names_the_world_space() {
        let dir = tempfile::tempdir().unwrap();
        let voxels: Vec<i16> = (0..8).collect();
        for (code, space) in [
            (1i16, "scanner"),
            (2, "aligned"),
            (3, "atlas:talairach"),
            (4, "atlas:mni152"),
            (5, "atlas:other"),
        ] {
            let s = dir.path().join(format!("sform-{code}.nii"));
            Synth::new(&[2, 2, 2]).sform(code).write(&s, &voxels);
            let wf = read_nifti(&s).unwrap().world_frame.unwrap();
            assert_eq!(wf.space, space, "sform_code {code}");
            // and it stays inside the ADR-0032 pinned frame vocabulary.
            assert!(
                tessera_core::referencing::Referenced::from_world_frame(&wf).frame_is_canonical(),
                "{space} must be a pinned frame"
            );

            // the qform branch names the frame from ITS OWN code, not the sform's.
            let q = dir.path().join(format!("qform-{code}.nii"));
            Synth::new(&[2, 2, 2])
                .no_sform()
                .qform(code, [0.0; 3], 1.0)
                .write(&q, &voxels);
            assert_eq!(read_nifti(&q).unwrap().world_frame.unwrap().space, space);
        }

        // an xform code outside the NIfTI enum cannot be named, and guessing `"scanner"` IS the bug —
        // so it is a typed error, not a mislabelled frame.
        let bad = dir.path().join("sform-9.nii");
        Synth::new(&[2, 2, 2]).sform(9).write(&bad, &voxels);
        assert!(matches!(read_nifti(&bad), Err(Error::Invalid(_))));
    }

    /// **B5 (#396)** — `ndim` was validated as `1..=7` but only dims 1–3 were read, so a 4-D fMRI
    /// series with `dim[4] = 200` ingested volume 0 and **silently discarded the other 199**: the file
    /// passed, sealed and verified with 99.5 % of the data gone. That violates ADR-0025 (lossless at the
    /// door); the fix declares `[t,z,y,x]` and carries the time axis via ADR-0032 `axis_referencing`.
    #[test]
    fn reads_every_volume_of_a_4d_series() {
        let dir = tempfile::tempdir().unwrap();
        let (nx, ny, nz, nt) = (2i16, 2i16, 2i16, 200i16);
        let per_volume = usize::try_from(nx * ny * nz).unwrap();
        let voxels: Vec<i16> = (0..per_volume * usize::try_from(nt).unwrap())
            .map(|k| k as i16)
            .collect();
        let p = dir.path().join("fmri.nii");
        // xyzt_units = NIFTI_UNITS_MM | NIFTI_UNITS_SEC; TR = 2.5 s from pixdim[4], toffset = 1 s.
        Synth::new(&[nx, ny, nz, nt])
            .time_axis(2.5, 1.0, 2 | 8)
            .write(&p, &voxels);

        let img = read_nifti(&p).unwrap();
        assert_eq!(img.shape, vec![200, 2, 2, 2], "4-D declares [t,z,y,x]");
        // volume 199 is present, byte-for-byte — the silent-truncation regression.
        let ArrayData::I16(ref got) = img.data else {
            panic!("native int16 preserved")
        };
        assert_eq!(got.len(), voxels.len(), "every volume is read");
        assert_eq!(&got[per_volume * 199..], &voxels[per_volume * 199..]);
        // the spatial frame still rides along, addressing the trailing [z,y,x] axes (the same
        // convention as the `[3,z,y,x]` deformation_field of ADR-0030 §5).
        let wf = img.world_frame.clone().unwrap();
        assert_eq!(wf.voxel_to_world([1.0, 1.0, 1.0]), [-12.0, -23.0, 34.0]);

        // …and a rank-4 volume still seals into a verifying recon product.
        let (m, _payloads) = to_recon_product(
            &img,
            "fmri-01",
            "2024-01-01T00:00:00Z",
            "fmri.nii",
            None,
            &[],
        )
        .unwrap();
        m.verify().unwrap();

        // `ndim = 4, dim[4] = 1` is the 3-D volume it says it is — no gratuitous rank bump (which
        // would move the `content_hash` of every single-frame 4-D file in existence).
        let flat = dir.path().join("single-frame.nii");
        Synth::new(&[2, 2, 2, 1]).write(&flat, &voxels[..per_volume]);
        assert_eq!(read_nifti(&flat).unwrap().shape, vec![2, 2, 2]);
    }
}
