//! NIfTI-1 ingest — read a `.nii` / `.nii.gz` neuroimaging volume into a Tessera `recon` product
//! (#208), lossless (native dtype preserved, and **every** volume of a 4-D series) with the spatial
//! `world_frame` derived from the NIfTI **sform** — or, when that is absent, the **qform** quaternion —
//! and the `scl_slope`/`scl_inter` rescale carried as the value transform.
//!
//! NIfTI-1 single-file: a fixed **348-byte header** then the voxel data at `vox_offset` (352 for `.nii`).
//! A `.nii.gz` is that same stream gzipped; it is the majority of NIfTI on disk (FSL, SPM and dcm2niix
//! all emit it by default) and is decompressed transparently, capped at the size the header itself
//! declares so the path is not a decompression bomb.
//!
//! NIfTI stores voxels **x-fastest** and its sform/qform affine is **RAS+**; Tessera arrays are C-order
//! (last axis fastest) and **LPS canonical** (ADR-0030 §6). So the volume is declared with NIfTI's axes
//! **reversed** — `[z,y,x]` for a 3-D volume, `[t,z,y,x]` for a 4-D series (x fastest — matches NIfTI's
//! storage byte-for-byte, no transpose) — and the affine is reordered to `[k,j,i]` columns + converted
//! RAS→LPS (negate the world x,y rows) at the door (ADR-0025).
//!
//! **Never panic** (FEATURE-MATRIX §A): every index this decoder derives from the header — `vox_offset`,
//! the dim product, the datatype width, the gunzip size — is range- and overflow-checked before use.
//! Ingest reads untrusted third-party files, so a header-driven panic is the wrong failure mode; the
//! failure is always a typed [`Error::Invalid`] (#396).

use tessera_core::block::array::{ArraySpec, WorldFrame};
use tessera_core::manifest::Manifest;
use tessera_core::referencing::Referenced;
use tessera_core::{Error, ProductBuilder, Result};
use tessera_io::array::{self, ArrayData};
use tessera_io::BlockPayload;

fn he(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("nifti: {e}"))
}

/// The NIfTI-1 single-file prefix: the 348-byte header plus the 4-byte `n+1\0` magic. The smallest byte
/// count [`read_nifti`] can parse, and the lowest legal `vox_offset`.
const HEADER_LEN: usize = 352;

/// Ceiling on the **declared** size of a transparently-gunzipped `.nii.gz` — `vox_offset` + the voxel
/// bytes the header asks for, which is also what bounds the decompression. A bomb therefore cannot claim
/// more than a legitimate file of the same declared shape.
///
/// It is deliberately *not* an expansion-ratio cap and not a peak-memory cap: decoding widens some
/// dtypes (NIfTI uint8 → Tessera uint16), so a file at this ceiling peaks at a multiple of it. The name
/// says "declared" for that reason. A volume larger than this must be gunzipped on the side and read
/// through the plain path (`.nii` is range-readable; a gzip stream is not).
const MAX_DECLARED_BYTES: u64 = 8 << 30; // 8 GiB

/// NIfTI's axis letters for `dim[1..=7]`: `x,y,z` spatial, `t` the time/frame axis, then the `u,v,w`
/// vector axes.
const NIFTI_AXES: [&str; 7] = ["x", "y", "z", "t", "u", "v", "w"];

/// Which NIfTI transform a [`NiftiImage::world_frame`] was reconstructed from. ADR-0056 §11 asks for
/// *which one was used* to be recorded: the sform and the qform can disagree, and a consumer chasing a
/// geometry discrepancy needs to know which one Tessera trusted.
///
/// Recorded in the **sealed provenance bag** (ADR-0056 §6a / ADR-0058 `Generation.config`, key
/// [`GEOMETRY_SOURCE_KEY`]) rather than as a new manifest field: it is a *recipe* fact — how the product
/// was made — so the bag is its designed home and no format surface is added. Being inside the seal is
/// the point; a `tracing` line is not a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometrySource {
    /// `srow_x/y/z` (`sform_code > 0`) — the preferred, fully general affine.
    Sform,
    /// The `quatern_b/c/d` + `qoffset_*` + `pixdim` quaternion (`qform_code > 0`) — used when the sform
    /// is absent or degenerate.
    Qform,
}

impl GeometrySource {
    /// The value recorded in the sealed provenance bag (see [`to_recon_product`]).
    pub fn as_str(self) -> &'static str {
        match self {
            GeometrySource::Sform => "sform",
            GeometrySource::Qform => "qform",
        }
    }
}

/// The well-known [`tessera_core::provenance::Generation`] key under which this decoder records which
/// NIfTI transform the geometry came from (ADR-0056 §6a — a *recipe* fact, so it rides the provenance
/// bag and adds no format surface).
const GEOMETRY_SOURCE_KEY: &str = "nifti_geometry_source";

/// A decoded NIfTI volume: shape and axis names in Tessera storage order (`[z,y,x]`, or `[t,z,y,x]` for
/// a 4-D series), native voxels, the LPS `world_frame` and which transform it came from, the per-axis
/// referencing for a non-spatial axis, and the intensity rescale.
pub struct NiftiImage {
    pub shape: Vec<u64>,
    /// Axis names in `shape` order — [`NIFTI_AXES`] reversed, so rank 3 is `[z,y,x]` and rank 4
    /// `[t,z,y,x]` (matching `tessera_core::block::array::default_axes`).
    pub axes: Vec<String>,
    pub data: ArrayData,
    /// The voxel→world frame. For a rank > 3 array the 3×4 affine addresses the **trailing three**
    /// (spatial) axes, the same convention as the `[3,z,y,x]` deformation field of ADR-0030 §5.
    pub world_frame: Option<WorldFrame>,
    /// Which NIfTI transform `world_frame` was reconstructed from; `None` when the file carries neither.
    pub geometry_source: Option<GeometrySource>,
    /// ADR-0032 per-axis referencing (one entry per axis) — `Some` only for a 4-D+ series whose header
    /// names both a time step (`pixdim[4]`) and a time unit (`xyzt_units`). Spatial axes stay bare
    /// storage indices.
    pub axis_referencing: Option<Vec<Option<Referenced>>>,
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
/// A NIfTI `f32` header field widened to the `f64` Tessera stores geometry in.
fn f64le(b: &[u8], o: usize) -> f64 {
    f64::from(f32le(b, o))
}
/// NIfTI stores `vox_offset` as a float that is really an integer. Convert it to a byte index without
/// panicking or wrapping: NaN and negatives clamp to 0 and anything past `usize::MAX` saturates (Rust's
/// float→int casts saturate), so every out-of-range value lands on one the caller's bounds check
/// rejects rather than on a plausible-looking index.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn f32_to_usize(v: f32) -> usize {
    v.max(0.0) as usize
}

/// Diagnose a `sizeof_hdr` that is not 348, so the error names the format actually found instead of
/// blaming the file for not being NIfTI — the same misleading-error complaint as B2's `.nii.gz` case.
/// Both alternatives are real rather than theoretical: **three of the six `.nii` fixtures in nibabel's
/// own test data are big-endian**, and every CIFTI `.dconn.nii`/`.dtseries.nii` is a NIfTI-2.
///
/// Neither is *supported* — this only makes the refusal say which one it is, so an operator knows
/// whether to byte-swap, convert, or look elsewhere.
fn sizeof_hdr_error(sizeof_hdr: i32) -> Error {
    /// 348 with its bytes reversed — what a big-endian NIfTI-1 header reads as here.
    const SWAPPED_348: i32 = i32::from_le_bytes(348i32.to_be_bytes());
    /// The NIfTI-2 header size, either way round.
    const NIFTI2: i32 = 540;
    const SWAPPED_540: i32 = i32::from_le_bytes(540i32.to_be_bytes());
    he(match sizeof_hdr {
        SWAPPED_348 => "big-endian NIfTI-1 is not supported (sizeof_hdr is 348 byte-swapped) — \
                        convert the file to little-endian first"
            .to_string(),
        NIFTI2 | SWAPPED_540 => "this is NIfTI-2 (sizeof_hdr = 540), not NIfTI-1 — unsupported \
                                 (CIFTI .dconn.nii / .dtseries.nii files are NIfTI-2)"
            .to_string(),
        other => format!("sizeof_hdr = {other}, not 348 — not a NIfTI-1 file"),
    })
}

/// Bytes per voxel for a supported NIfTI datatype code. Rejecting an unsupported code *here* — before
/// any size arithmetic — keeps the width in every overflow check a real one.
fn dtype_width(datatype: i16) -> Result<usize> {
    Ok(match datatype {
        2 => 1,       // uint8
        4 | 512 => 2, // int16 | uint16
        8 | 16 => 4,  // int32 | float32
        64 => 8,      // float64
        other => return Err(he(format!("unsupported NIfTI datatype code {other}"))),
    })
}

/// Tessera storage-order axis names for a rank-`rank` NIfTI array: [`NIFTI_AXES`] truncated to the rank
/// and reversed, because NIfTI stores x fastest while Tessera declares the fastest axis last.
fn axis_names(rank: usize) -> Vec<String> {
    NIFTI_AXES
        .iter()
        .take(rank)
        .rev()
        .map(|a| (*a).to_string())
        .collect()
}

/// The NIfTI `sform_code`/`qform_code` → the ADR-0030 named frame the affine lands in. The code **is**
/// the frame identity: 1 scanner-anatomical, 2 aligned (co-registered to another file), 3 Talairach,
/// 4 MNI-152, 5 another template. Every result is drawn from the ADR-0032 pinned frame vocabulary
/// (`tessera_core::referencing::CANONICAL_FRAMES`, plus the `atlas:<id>` refinement).
///
/// A code outside that enum is an error rather than a guess: hard-coding `"scanner"` over an unnameable
/// frame is exactly the bug this replaces, and an affine whose target frame is unknown is not usable
/// geometry.
fn xform_space(code: i16) -> Result<&'static str> {
    Ok(match code {
        1 => "scanner",
        2 => "aligned",
        3 => "atlas:talairach",
        4 => "atlas:mni152",
        5 => "atlas:other",
        other => {
            return Err(he(format!(
                "unknown sform/qform code {other} (1 scanner · 2 aligned · 3 talairach · 4 mni152 · \
                 5 other-template) — the world frame cannot be named, so the geometry is not stamped"
            )))
        }
    })
}

/// The parsed, fully validated NIfTI-1 header: everything the voxel read and the product build need,
/// with every header-derived index already range- and overflow-checked.
struct Header {
    /// Tessera storage order (NIfTI's dims reversed); rank ≥ 3.
    shape: Vec<u64>,
    axes: Vec<String>,
    /// Voxel count — the `shape` product.
    count: usize,
    /// Bytes per voxel.
    width: usize,
    datatype: i16,
    vox_offset: usize,
    /// `(slope, intercept)` — the `scl_*` pair, already defaulted to the identity.
    rescale: (f64, f64),
    world_frame: Option<WorldFrame>,
    geometry_source: Option<GeometrySource>,
    axis_referencing: Option<Vec<Option<Referenced>>>,
}

impl Header {
    /// The byte length the file must have for the declared volume to be present — and the cap on a
    /// transparent gunzip, so the bomb guard is the header's own claim rather than a magic constant.
    fn data_end(&self) -> Result<usize> {
        self.count
            .checked_mul(self.width)
            .and_then(|bytes| bytes.checked_add(self.vox_offset))
            .ok_or_else(|| he("declared voxel byte count overflows usize"))
    }
}

/// NIfTI dims → a Tessera shape + axis names + voxel count. `dim[1..=ndim]` are the real extents
/// (`dim[k] for k > ndim` is meaningless), declared **reversed** because NIfTI stores x fastest.
///
/// The declared rank keeps the higher NIfTI axes only while they carry more than one element, and never
/// drops below 3. So a 3-D volume stays `[z,y,x]`; a `ndim = 4, dim[4] = 1` file stays the 3-D volume it
/// is (a gratuitous rank bump would move the `content_hash` of every single-frame 4-D file); and a real
/// 4-D series becomes `[t,z,y,x]` **with all of its volumes** — they used to be silently discarded,
/// which ADR-0025 (lossless at the door) forbids.
fn parse_shape(b: &[u8], ndim: usize) -> Result<(Vec<u64>, Vec<String>, usize)> {
    let mut dims = [1u64; 8];
    for (k, slot) in dims.iter_mut().enumerate().take(ndim + 1).skip(1) {
        let extent = i16le(b, 40 + 2 * k);
        // Zero and negative are not NIfTI extents; both used to slip through into a 0-voxel or
        // wrapped-size array.
        if extent < 1 {
            return Err(he(format!(
                "dim[{k}] = {extent} — a NIfTI extent must be ≥ 1"
            )));
        }
        *slot = u64::try_from(extent).map_err(|_| he("negative NIfTI dimension"))?;
    }
    let rank = (4..=ndim).rev().find(|&k| dims[k] > 1).unwrap_or(3).max(3);
    let shape: Vec<u64> = (1..=rank).rev().map(|k| dims[k]).collect();
    let count = shape
        .iter()
        .try_fold(1u64, |acc, &d| acc.checked_mul(d))
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| {
            let d: Vec<String> = shape.iter().map(u64::to_string).collect();
            he(format!(
                "dim product overflows a voxel count (shape [{}])",
                d.join(", ")
            ))
        })?;
    Ok((shape, axis_names(rank), count))
}

/// The sform's three RAS rows — `srow_x/y/z` at 280/296/312, each `f32[4] = [Mi, Mj, Mk, offset]`.
fn sform_rows(b: &[u8]) -> [[f64; 4]; 3] {
    let row = |o: usize| {
        [
            f64le(b, o),
            f64le(b, o + 4),
            f64le(b, o + 8),
            f64le(b, o + 12),
        ]
    };
    [row(280), row(296), row(312)]
}

/// The qform's three RAS rows, reconstructed from the quaternion exactly as the reference
/// `nifti_quatern_to_mat44` does:
///
/// * `a = sqrt(1 − (b² + c² + d²))`, or — when that radicand is negligible — the renormalised unit
///   `(b,c,d)` with `a = 0` (a 180° rotation);
/// * the rotation's `i`/`j`/`k` columns are scaled by `pixdim[1..=3]`, a non-positive spacing clamped to
///   1 as the reference does;
/// * **`pixdim[0] < 0` (`qfac`) negates the `k` column** — the left-handed case, and the sign convention
///   that is easy to get subtly wrong;
/// * the translation is `qoffset_x/y/z`.
fn qform_rows(b: &[u8]) -> [[f64; 4]; 3] {
    let (mut qb, mut qc, mut qd) = (f64le(b, 256), f64le(b, 260), f64le(b, 264));
    let radicand = 1.0 - (qb * qb + qc * qc + qd * qd);
    let qa = if radicand < 1.0e-7 {
        let norm = (qb * qb + qc * qc + qd * qd).sqrt();
        if norm > 0.0 {
            qb /= norm;
            qc /= norm;
            qd /= norm;
        }
        0.0
    } else {
        radicand.sqrt()
    };
    // pixdim[k] at 76 + 4k; pixdim[0] is qfac, negative = left-handed (k column flipped).
    let spacing = |k: usize| {
        let d = f64le(b, 76 + 4 * k);
        if d > 0.0 {
            d
        } else {
            1.0
        }
    };
    let qfac = if f64le(b, 76) < 0.0 { -1.0 } else { 1.0 };
    let scale = [spacing(1), spacing(2), spacing(3) * qfac];
    let rotation = [
        [
            qa * qa + qb * qb - qc * qc - qd * qd,
            2.0 * (qb * qc - qa * qd),
            2.0 * (qb * qd + qa * qc),
        ],
        [
            2.0 * (qb * qc + qa * qd),
            qa * qa + qc * qc - qb * qb - qd * qd,
            2.0 * (qc * qd - qa * qb),
        ],
        [
            2.0 * (qb * qd - qa * qc),
            2.0 * (qc * qd + qa * qb),
            qa * qa + qd * qd - qc * qc - qb * qb,
        ],
    ];
    let offset = [f64le(b, 268), f64le(b, 272), f64le(b, 276)];
    let mut rows = [[0.0f64; 4]; 3];
    for (i, row) in rows.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().take(3).enumerate() {
            *cell = rotation[i][j] * scale[j];
        }
        row[3] = offset[i];
    }
    rows
}

/// Is every entry of the affine finite? A NaN/inf entry is **corruption**, not absent geometry:
/// `WorldFrame::is_nondegenerate` only checks that the column norms are non-zero, and a NaN norm
/// compares false against `> 0.0` in a way that silently reads as "degenerate".
fn affine_is_finite(wf: &WorldFrame) -> bool {
    wf.affine.iter().all(|v| v.is_finite())
}

/// How far the 3×3 block is from singular, **scale-invariantly**: `|det| / Π‖column‖`. That ratio is 1
/// for an orthogonal frame and → 0 as the columns become parallel, so it does not mistake a
/// micron-spacing affine (tiny `det`, perfectly good geometry) for a degenerate one — which an absolute
/// `det` threshold would. Parallel columns are the case `WorldFrame::is_nondegenerate` misses: their
/// norms are non-zero, yet there is no invertible voxel→world map.
fn affine_conditioning(wf: &WorldFrame) -> f64 {
    let a = &wf.affine;
    let at = |r: usize, c: usize| a[r * 4 + c];
    let det = at(0, 0) * (at(1, 1) * at(2, 2) - at(1, 2) * at(2, 1))
        - at(0, 1) * (at(1, 0) * at(2, 2) - at(1, 2) * at(2, 0))
        + at(0, 2) * (at(1, 0) * at(2, 1) - at(1, 1) * at(2, 0));
    let scale: f64 = wf.spacing().iter().product();
    if scale > 0.0 {
        det.abs() / scale
    } else {
        0.0 // a zero-norm column: degenerate by definition
    }
}

/// A usable voxel→world affine: non-degenerate per ADR-0030 **and** genuinely invertible. Kept local to
/// this decoder rather than tightening `WorldFrame::is_nondegenerate`, which is the format-wide
/// `ArraySpec::validate` gate — narrowing that would change what every other product may seal, which is
/// a format decision, not a NIfTI bug fix (raised as a follow-up instead).
fn affine_is_usable(wf: &WorldFrame) -> bool {
    wf.is_nondegenerate() && affine_conditioning(wf) > 1.0e-6
}

/// RAS rows `[Mi, Mj, Mk, offset]` → the Tessera LPS `world_frame`: reorder the columns
/// `[i,j,k] → [k,j,i]` (Tessera declares the fastest axis last) and convert RAS→LPS by negating the
/// world x and y rows (ADR-0030 §6).
fn world_frame_from_ras(rows: [[f64; 4]; 3], code: i16) -> Result<WorldFrame> {
    let [x, y, z] = rows;
    Ok(WorldFrame {
        affine: [
            -x[2], -x[1], -x[0], -x[3], // world L = -RAS x
            -y[2], -y[1], -y[0], -y[3], // world P = -RAS y
            z[2], z[1], z[0], z[3], // world S =  RAS z
        ],
        convention: "LPS".into(),
        unit: "mm".into(),
        space: xform_space(code)?.into(),
    })
}

/// The voxel→world frame, with NIfTI's own precedence: **sform** (`sform_code > 0`, the general affine),
/// else **qform** (`qform_code > 0`, the quaternion), else none — the array really is in index space
/// (ADR-0029 feature-by-presence).
///
/// The qform fallback matters: older SPM/FSL outputs and some dcm2niix paths carry geometry *only* in
/// the quaternion, and ignoring it made Tessera declare the array to be in bare index space when it
/// demonstrably is not, with no warning.
///
/// A **degenerate** sform (singular 3×3 block — no invertible geometry) is not a usable frame, so it
/// falls through to the qform rather than sealing an affine `ArraySpec::validate` would reject much
/// later, and errors when there is nothing to fall through to.
///
/// An sform whose *code* cannot be named (see [`xform_space`]) is **not** a fall-through case: it errors
/// even when a valid qform is present. The file names the sform as its primary transform, and quietly
/// substituting a differently-coded qform would be the same silent-substitution sin in reverse — the
/// operator needs to see that the preferred transform is uninterpretable.
fn parse_geometry(b: &[u8]) -> Result<(Option<WorldFrame>, Option<GeometrySource>)> {
    let (sform_code, qform_code) = (i16le(b, 254), i16le(b, 252));
    // A *negative* code is a malformed header, not "absent" — `code > 0` alone read -1 as absent and
    // silently dropped the geometry.
    for (name, code) in [("sform_code", sform_code), ("qform_code", qform_code)] {
        if code < 0 {
            return Err(he(format!(
                "{name} is negative ({code}) — malformed header"
            )));
        }
    }
    if sform_code > 0 {
        let wf = world_frame_from_ras(sform_rows(b), sform_code)?;
        // Non-finite is corruption: refuse rather than fall through to a different transform, since
        // "the sform is unreadable" is not the same claim as "there is no sform".
        if !affine_is_finite(&wf) {
            return Err(he(
                "the sform contains a non-finite (NaN/inf) entry — corrupt header, not absent geometry",
            ));
        }
        if affine_is_usable(&wf) {
            return Ok((Some(wf), Some(GeometrySource::Sform)));
        }
        tracing::warn!(
            target: "tessera::ingest",
            sform_code,
            conditioning = affine_conditioning(&wf),
            "nifti: sform_code is set but the sform is singular (no invertible geometry) — falling back to the qform"
        );
    }
    if qform_code > 0 {
        let wf = world_frame_from_ras(qform_rows(b), qform_code)?;
        if !affine_is_finite(&wf) {
            return Err(he(
                "the qform quaternion/pixdim contains a non-finite (NaN/inf) entry — corrupt header",
            ));
        }
        if !affine_is_usable(&wf) {
            return Err(he(format!(
                "qform_code {qform_code} reconstructs a singular affine (no invertible geometry)"
            )));
        }
        return Ok((Some(wf), Some(GeometrySource::Qform)));
    }
    if sform_code > 0 {
        return Err(he(
            "the sform is singular (no invertible geometry) and there is no qform to fall back to",
        ));
    }
    Ok((None, None))
}

/// ADR-0032 per-axis referencing for a 4-D+ series: the `t` axis's index→elapsed-seconds mapping, from
/// `pixdim[4]` (the step) and `toffset` (the start), both scaled by the `xyzt_units` time code.
///
/// `None` for a 3-D volume, and for a 4-D one whose header names no time unit or no positive step — that
/// axis then stays a bare storage index (feature-by-presence) rather than carrying a guessed cadence. A
/// DWI direction stack lands there: its `dim[4]` counts gradient directions, not seconds, and such files
/// leave `pixdim[4]` / the time unit unset. ADR-0032's `time_regular` is reused verbatim — this decoder
/// invents no axis convention of its own.
fn parse_time_axis(b: &[u8], rank: usize) -> Option<Vec<Option<Referenced>>> {
    let t_axis = rank.checked_sub(4)?; // rank 4 → axis 0, rank 5 → axis 1, …
                                       // xyzt_units bits 3..5 are the time unit: 8 = NIFTI_UNITS_SEC, 16 = MSEC, 24 = USEC.
    let scale = match b[123] & 0x38 {
        8 => 1.0,
        16 => 1.0e-3,
        24 => 1.0e-6,
        _ => return None,
    };
    let step = f64le(b, 76 + 4 * 4) * scale; // pixdim[4]
    if step <= 0.0 || !step.is_finite() {
        return None;
    }
    let start = f64le(b, 136) * scale; // toffset
    let mut per_axis = vec![None; rank];
    per_axis[t_axis] = Some(Referenced::time_regular(start, step));
    Some(per_axis)
}

/// Parse + validate the 348-byte NIfTI-1 header (little-endian, single-file `n+1`).
fn parse_header(b: &[u8]) -> Result<Header> {
    if b.len() < HEADER_LEN {
        return Err(he("file shorter than a NIfTI-1 header"));
    }
    let sizeof_hdr = i32le(b, 0);
    if sizeof_hdr != 348 {
        return Err(sizeof_hdr_error(sizeof_hdr));
    }
    // magic "n+1\0" at offset 344 marks a single-file .nii.
    if &b[344..347] != b"n+1" {
        return Err(he(
            "magic is not 'n+1' (only single-file .nii is supported)",
        ));
    }
    let ndim = i16le(b, 40);
    if !(1..=7).contains(&ndim) {
        return Err(he(format!("dim[0] (ndim) = {ndim} is outside 1..=7")));
    }
    let ndim = usize::try_from(ndim).map_err(|_| he("dim[0] (ndim) out of range"))?;
    let (shape, axes, count) = parse_shape(b, ndim)?;

    let datatype = i16le(b, 70);
    let width = dtype_width(datatype)?;

    // `vox_offset` is a float that is really a byte index. A fractional value used to be truncated in
    // silence — it is a malformed header, so say so rather than guess which byte was meant.
    let declared_offset = f32le(b, 108);
    if !declared_offset.is_finite() || declared_offset.fract() != 0.0 {
        return Err(he(format!(
            "vox_offset {declared_offset} is not a whole byte count"
        )));
    }
    let vox_offset = f32_to_usize(declared_offset);
    // The voxel data cannot start inside the header: `vox_offset = 0` would silently decode header bytes
    // as voxels. A single-file `n+1` declares ≥ 352 (nibabel rejects the same case).
    if vox_offset < HEADER_LEN {
        return Err(he(format!(
            "vox_offset {declared_offset} is inside the {HEADER_LEN}-byte header (a single-file .nii declares ≥ {HEADER_LEN})"
        )));
    }

    // NIfTI spells "no rescale" as `scl_slope == 0`, not as a degenerate scale.
    let scl_slope = f64le(b, 112);
    let rescale = if scl_slope == 0.0 {
        (1.0, 0.0)
    } else {
        (scl_slope, f64le(b, 116))
    };

    let (world_frame, geometry_source) = parse_geometry(b)?;
    let axis_referencing = parse_time_axis(b, shape.len());
    Ok(Header {
        shape,
        axes,
        count,
        width,
        datatype,
        vox_offset,
        rescale,
        world_frame,
        geometry_source,
        axis_referencing,
    })
}

/// Decode the voxel payload that starts at `vox_offset` into its native [`ArrayData`]. Both
/// header-derived indices are checked *before* they are used: `vox_offset` may point past the end of the
/// file, and the voxel byte count may overflow `usize`.
fn read_voxels(h: &Header, b: &[u8]) -> Result<ArrayData> {
    let d = b.get(h.vox_offset..).ok_or_else(|| {
        he(format!(
            "vox_offset {} is past the end of the file ({} bytes)",
            h.vox_offset,
            b.len()
        ))
    })?;
    let need = h
        .count
        .checked_mul(h.width)
        .ok_or_else(|| he("declared voxel byte count overflows usize"))?;
    if d.len() < need {
        return Err(he(format!(
            "voxel data shorter than dim product ({} of {need} bytes after vox_offset {})",
            d.len(),
            h.vox_offset
        )));
    }
    let (n, w) = (h.count, h.width);
    // The stride comes from `dtype_width` via `h.width` — never repeated per arm, so a width can never
    // disagree with the type that reads it (the bound checked above is `n * w`).
    macro_rules! read_vec {
        ($variant:ident, $from:expr) => {
            ArrayData::$variant((0..n).map(|i| $from(&d[i * w..])).collect())
        };
    }
    Ok(match h.datatype {
        // Tessera's dtype floor is 16-bit (ADR — 8-bit out of scope), so NIfTI uint8 (e.g. masks)
        // widens losslessly to uint16.
        2 => read_vec!(U16, |s: &[u8]| u16::from(s[0])),
        4 => read_vec!(I16, |s: &[u8]| i16::from_le_bytes([s[0], s[1]])),
        512 => read_vec!(U16, |s: &[u8]| u16::from_le_bytes([s[0], s[1]])),
        8 => read_vec!(I32, |s: &[u8]| i32::from_le_bytes([s[0], s[1], s[2], s[3]])),
        16 => read_vec!(F32, |s: &[u8]| f32::from_le_bytes([s[0], s[1], s[2], s[3]])),
        64 => read_vec!(F64, |s: &[u8]| f64::from_le_bytes([
            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]
        ])),
        // `dtype_width` already rejected every other code, so this arm is unreachable in practice — but
        // it is an error rather than a panic.
        other => return Err(he(format!("unsupported NIfTI datatype code {other}"))),
    })
}

/// Gzip magic: `1f 8b` opens every gzip member, so a `.nii.gz` is detected from its bytes rather than
/// from a filename the caller may not control.
fn is_gzip(b: &[u8]) -> bool {
    b.starts_with(&[0x1f, 0x8b])
}

/// Decompress at most `limit` bytes of a gzip stream **without** verifying its trailer — a *peek*, used
/// only to read the 352-byte header so the real read can be bounded by the file's own declared size.
/// Never use it for data that will be sealed; [`gunzip_verified`] is the one that checks integrity.
fn gunzip_peek(raw: &[u8], limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::MultiGzDecoder::new(raw)
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|e| he(format!("gzip decompression failed: {e}")))?;
    Ok(out)
}

/// Decompress the first `want` bytes of a gzip stream **and verify the member trailer(s)**.
///
/// The verification is the whole point, and it is not free: flate2 checks a member's CRC32/ISIZE trailer
/// only when the deflate stream reaches **EOF**, so stopping at `want` bytes never reads the trailer at
/// all. Measured on the previous implementation, a `.nii.gz` with a flipped CRC byte — or a flipped
/// ISIZE byte — decompressed and **sealed with a valid `content_hash`**. That is worse than
/// gunzip-then-ingest, because the trailer is the *only* integrity check a NIfTI source carries: a raw
/// `.nii` has none at all. So the remainder is drained into a sink purely to force the check, which also
/// rejects container-level trailing garbage (even one stray byte) that the old read accepted.
///
/// Concatenated members are **accepted**: RFC 1952 defines a gzip file as a series of members and
/// `gunzip` concatenates them, so refusing would reject files other tools legitimately produce. Every
/// member's trailer is verified, and decompressed content beyond the declared volume is ignored exactly
/// as trailing bytes are in a plain `.nii` (where they are extensions).
///
/// `cap` bounds the **total** decompressed size so a bomb cannot make the drain run unbounded. The drain
/// discards as it goes, so memory stays bounded by `want`, not by `cap`.
fn gunzip_verified(raw: &[u8], want: usize, cap: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut dec = flate2::read::MultiGzDecoder::new(raw);
    let mut out = Vec::new();
    (&mut dec)
        .take(want as u64)
        .read_to_end(&mut out)
        .map_err(|e| he(format!("gzip decompression failed: {e}")))?;
    // Read past `want` so the decoder hits EOF and validates CRC32 + ISIZE. `room + 1` makes "filled
    // the remaining budget exactly" distinguishable from "there was still more".
    let room = cap.saturating_sub(out.len() as u64);
    let extra = std::io::copy(&mut (&mut dec).take(room + 1), &mut std::io::sink())
        .map_err(|e| he(format!("gzip integrity check failed: {e}")))?;
    if extra > room {
        return Err(he(format!(
            "gzipped NIfTI decompresses to more than the {cap}-byte declared-size ceiling — gunzip it \
             and read the plain .nii instead"
        )));
    }
    Ok(out)
}

/// Read a NIfTI-1 `.nii` — or a gzipped `.nii.gz` — file (little-endian) into a [`NiftiImage`].
/// Supports the common datatypes (uint8/int16/uint16/int32/float32/float64) and **every** volume of a
/// 4-D/5-D series. Errors (never panics) on a non-NIfTI-1 / big-endian / unsupported / malformed file.
pub fn read_nifti(path: &std::path::Path) -> Result<NiftiImage> {
    let raw = std::fs::read(path).map_err(he)?;
    let bytes = if is_gzip(&raw) {
        // Two passes over the same stream. The first is a peek at the 352-byte header, so the *header*
        // bounds the second one — the bomb guard is the file's own claim, not an arbitrary constant.
        let need = parse_header(&gunzip_peek(&raw, HEADER_LEN as u64)?)?.data_end()?;
        if need as u64 > MAX_DECLARED_BYTES {
            return Err(he(format!(
                "gzipped NIfTI declares {need} bytes, over the {MAX_DECLARED_BYTES}-byte \
                 declared-size ceiling — gunzip it and read the plain .nii instead"
            )));
        }
        gunzip_verified(&raw, need, MAX_DECLARED_BYTES)?
    } else {
        raw
    };
    // Re-parsed from the bytes that were actually integrity-checked, so nothing downstream rests on the
    // unverified peek above.
    let header = parse_header(&bytes)?;
    let data = read_voxels(&header, &bytes)?;
    tracing::debug!(
        target: "tessera::ingest",
        rank = header.shape.len(),
        voxels = header.count,
        geometry = ?header.geometry_source,
        "nifti: decoded {} ({} voxels)",
        path.display(),
        header.count,
    );
    Ok(NiftiImage {
        shape: header.shape,
        axes: header.axes,
        data,
        world_frame: header.world_frame,
        geometry_source: header.geometry_source,
        axis_referencing: header.axis_referencing,
        rescale_slope: header.rescale.0,
        rescale_intercept: header.rescale.1,
    })
}

/// Build a sealed Tessera `recon` product from a decoded NIfTI volume, with the `world_frame`/rescale on
/// the array spec and an `ingested_from` provenance edge to the source `.nii`. `extra_sources` flow in
/// AFTER `ingested_from` (the declarative ingest engine threads `derived_from` + `ingested_via_spec`
/// edges here so the chain verifier picks up the parent's `manifest_hash`).
///
/// For a 4-D+ series the axis names come from the decoder (`[t,z,y,x]`), the `t` axis carries its
/// ADR-0032 `axis_referencing` when the header named one, and the 3×4 `world_frame` addresses the
/// trailing three spatial axes — the same convention as the `[3,z,y,x]` deformation field of ADR-0030 §5.
pub fn to_recon_product(
    img: &NiftiImage,
    name: &str,
    timestamp: &str,
    source: &str,
    source_digest: Option<&str>,
    extra_sources: &[tessera_core::provenance::Source],
) -> Result<(Manifest, Vec<BlockPayload>)> {
    let mut spec = ArraySpec::new(img.shape.clone(), img.data.dtype())
        .with_axes(img.axes.clone())
        .with_rescale(img.rescale_slope, img.rescale_intercept);
    spec.world_frame = img.world_frame.clone();
    spec.axis_referencing = img.axis_referencing.clone();
    let (block_ref, payload) = array::array_block("volume", &spec, &img.data)?;

    let mut b = ProductBuilder::new("recon", name, "NIfTI recon volume", timestamp);
    b.add_block_ref(block_ref);
    // Which transform the geometry came from is a recipe fact, recorded in the sealed provenance bag
    // (ADR-0056 §6a). Absent for an index-space volume — feature-by-presence, so a file with no
    // geometry gains no bag.
    if let Some(src) = img.geometry_source {
        b.with_generation(
            tessera_core::provenance::Generation::default()
                .with(GEOMETRY_SOURCE_KEY, serde_json::Value::from(src.as_str())),
        );
    }
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
        bitpix: i16,
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
                bitpix: 16,
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
        /// Override the datatype code (the `bitpix` the decoder ignores follows it).
        fn datatype(mut self, code: i16, bitpix: i16) -> Self {
            self.datatype = code;
            self.bitpix = bitpix;
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
            put_i16(&mut h, 72, self.bitpix);
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

        /// Header + an opaque little-endian payload — for the non-int16 datatypes.
        fn write_raw(&self, path: &std::path::Path, payload: &[u8]) {
            let mut b = self.header();
            b.extend_from_slice(payload);
            std::fs::write(path, b).unwrap();
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

    /// **Review blocker (#447)** — `.nii.gz` integrity. flate2 verifies a gzip member's CRC32/ISIZE
    /// trailer only when the deflate stream reaches **EOF**, so reading just the bytes the header asks
    /// for (`take(need)`) never touches the trailer: a corrupted `.nii.gz` whose payload still inflates
    /// was sealed with a perfectly valid `content_hash`. Measured on the old code — a flipped CRC byte
    /// AND a flipped ISIZE byte both returned `Ok`. That is *worse* than gunzip-then-ingest, and the
    /// trailer is the **only** integrity check a NIfTI source carries (a raw `.nii` has none), so the
    /// remainder must be drained to force the check.
    #[test]
    fn gzipped_nifti_integrity_is_verified_not_merely_inflated() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let voxels: Vec<i16> = (0..8).collect();
        let synth = Synth::new(&[2, 2, 2]);
        let gz = |plain: &[u8]| {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(plain).unwrap();
            e.finish().unwrap()
        };
        let good = gz(&synth.bytes(&voxels));
        let read = |name: &str, raw: &[u8]| {
            let p = dir.path().join(name);
            std::fs::write(&p, raw).unwrap();
            read_nifti(&p).map(|i| i.data.len())
        };

        // the baseline still works
        assert_eq!(read("ok.nii.gz", &good).unwrap(), 8);

        // a flipped CRC32 byte and a flipped ISIZE byte (the last 8 bytes are CRC32 ‖ ISIZE)
        for (name, off) in [("crc.nii.gz", 8), ("isize.nii.gz", 4)] {
            let mut bad = good.clone();
            let i = bad.len() - off;
            bad[i] ^= 0xFF;
            assert!(
                matches!(read(name, &bad), Err(Error::Invalid(_))),
                "{name}: a corrupt gzip trailer must not seal"
            );
        }
        // truncated mid-payload, and container-level trailing garbage (even one stray byte)
        assert!(matches!(
            read("trunc.nii.gz", &good[..good.len() / 2]),
            Err(Error::Invalid(_))
        ));
        for (name, tail) in [
            ("junk.nii.gz", &b"JUNKJUNK"[..]),
            ("stray.nii.gz", &b"\0"[..]),
        ] {
            let mut bad = good.clone();
            bad.extend_from_slice(tail);
            assert!(
                matches!(read(name, &bad), Err(Error::Invalid(_))),
                "{name}: trailing bytes after the gzip member are malformed"
            );
        }
        // a flipped byte in the deflate body was already caught, and still is.
        let mut body = good.clone();
        let i = body.len() / 2;
        body[i] ^= 0xFF;
        assert!(matches!(read("body.nii.gz", &body), Err(Error::Invalid(_))));

        // DECISION pinned: concatenated members are accepted. RFC 1952 defines a gzip file as a
        // *series* of members and `gunzip` concatenates them, so rejecting would refuse files other
        // tools produce; every member's trailer is still verified, and content past the declared
        // volume is ignored exactly as trailing bytes are in a plain `.nii` (extensions).
        let mut two = good.clone();
        two.extend_from_slice(&good);
        assert_eq!(read("two.nii.gz", &two).unwrap(), 8);
    }

    /// **Review nit (#447)** — a non-finite affine is corruption, not "absent geometry", so it is a
    /// typed error rather than a silent fall-through. `WorldFrame::is_nondegenerate` only checks that
    /// the column norms are non-zero, which NaN/inf and *parallel* columns both slip past.
    #[test]
    fn non_finite_geometry_is_rejected_and_singular_geometry_falls_through() {
        let dir = tempfile::tempdir().unwrap();
        let voxels: Vec<i16> = (0..8).collect();
        let read = |name: &str, s: Synth| {
            let p = dir.path().join(name);
            s.write(&p, &voxels);
            read_nifti(&p)
        };

        // NaN / inf anywhere in the sform → error, even though a qform is available.
        for (name, bad) in [("nan", f32::NAN), ("inf", f32::INFINITY)] {
            let mut s = Synth::new(&[2, 2, 2]).qform(1, [0.0; 3], 1.0);
            s.srows[1][2] = bad;
            assert!(
                matches!(
                    read(&format!("sform-{name}.nii"), s),
                    Err(Error::Invalid(_))
                ),
                "a {name} sform entry must be a typed error, not a fall-through"
            );
        }
        // NaN in the quaternion → error too (the qform is the chosen transform here).
        assert!(matches!(
            read(
                "quat-nan.nii",
                Synth::new(&[2, 2, 2])
                    .no_sform()
                    .qform(1, [f32::NAN, 0.0, 0.0], 1.0)
            ),
            Err(Error::Invalid(_))
        ));

        // SINGULAR but finite (two parallel columns → zero determinant, non-zero column norms) is
        // unusable geometry rather than corruption, so it falls through to the qform like the
        // all-zero sform does.
        let mut parallel = Synth::new(&[2, 2, 2]).qform(1, [0.0; 3], 1.0);
        parallel.srows = [[2., 2., 0., 10.], [0., 0., 0., 20.], [0., 0., 4., 30.]];
        let img = read("parallel.nii", parallel).unwrap();
        assert_eq!(img.geometry_source, Some(GeometrySource::Qform));
    }

    /// **Review nits (#447)** — two header fields that were accepted by silent coercion: a *negative*
    /// xform code read as "absent" (`code > 0`), and a *fractional* `vox_offset` truncated to an
    /// integer. Both are malformed headers, so both are typed errors.
    #[test]
    fn negative_xform_codes_and_a_fractional_vox_offset_are_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let voxels: Vec<i16> = (0..8).collect();
        let err = |name: &str, s: Synth| {
            let p = dir.path().join(name);
            s.write(&p, &voxels);
            match read_nifti(&p).map(|i| i.shape) {
                Err(Error::Invalid(msg)) => msg,
                other => panic!("{name}: expected Error::Invalid, got {other:?}"),
            }
        };
        assert!(err("neg-sform.nii", Synth::new(&[2, 2, 2]).sform(-1)).contains("sform"));
        assert!(err(
            "neg-qform.nii",
            Synth::new(&[2, 2, 2]).no_sform().qform(-1, [0.0; 3], 1.0)
        )
        .contains("qform"));
        assert!(err("frac.nii", Synth::new(&[2, 2, 2]).vox_offset(352.5)).contains("vox_offset"));

        // …but an unnameable code on the transform that is NOT used is ignored: the sform wins, the
        // qform is never consulted, and refusing a file over an unused field would be over-strict.
        // (The mirror case — an unnameable *sform* — errors even with a valid qform, by design.)
        let p = dir.path().join("odd-qform.nii");
        Synth::new(&[2, 2, 2])
            .qform(99, [0.0; 3], 1.0)
            .write(&p, &voxels);
        let img = read_nifti(&p).unwrap();
        assert_eq!(img.geometry_source, Some(GeometrySource::Sform));
    }

    /// **Review nit (#447)** — #396 asks for *which* transform was used to be **recorded**; a debug
    /// trace is not a record. It rides the ADR-0056 §6a / ADR-0058 sealed provenance recipe bag
    /// (`Generation.config`), which adds no format surface and is inside the seal.
    #[test]
    fn the_geometry_source_is_recorded_in_the_sealed_provenance_bag() {
        let dir = tempfile::tempdir().unwrap();
        let voxels: Vec<i16> = (0..8).collect();
        let seal = |name: &str, s: Synth| {
            let p = dir.path().join(name);
            s.write(&p, &voxels);
            let img = read_nifti(&p).unwrap();
            to_recon_product(&img, "x", "2024-01-01T00:00:00Z", name, None, &[])
                .unwrap()
                .0
        };
        let key = "nifti_geometry_source";
        let m = seal("sform.nii", Synth::new(&[2, 2, 2]));
        assert_eq!(
            m.generation.as_ref().unwrap().config.get(key),
            Some(&serde_json::json!("sform"))
        );
        m.verify().unwrap();
        let m = seal(
            "qform.nii",
            Synth::new(&[2, 2, 2]).no_sform().qform(1, [0.0; 3], 1.0),
        );
        assert_eq!(
            m.generation.as_ref().unwrap().config.get(key),
            Some(&serde_json::json!("qform"))
        );
        // an index-space file records no geometry, so the bag stays absent (feature-by-presence).
        let m = seal("none.nii", Synth::new(&[2, 2, 2]).no_sform());
        assert!(m.generation.is_none());
    }

    /// A header that is not little-endian NIfTI-1 must say **which** format it is. The old catch-all
    /// ("not NIfTI-1, or big-endian") is the same misleading-error complaint as B2's `.nii.gz` case: it
    /// blames the file. Both alternatives are real — three of the six `.nii` fixtures in nibabel's own
    /// test data are big-endian, and every CIFTI `.dconn.nii` is a NIfTI-2. Still unsupported; just named.
    #[test]
    fn a_non_little_endian_nifti1_header_is_named_not_blamed_on_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let named = |name: &str, sizeof_hdr: [u8; 4]| {
            let mut h = Synth::new(&[2, 2, 2]).header();
            h[0..4].copy_from_slice(&sizeof_hdr);
            let p = dir.path().join(name);
            std::fs::write(&p, h).unwrap();
            match read_nifti(&p).map(|img| img.shape) {
                Err(Error::Invalid(msg)) => msg,
                other => panic!("{name}: expected Error::Invalid, got {other:?}"),
            }
        };
        assert!(
            named("be.nii", 348i32.to_be_bytes()).contains("big-endian"),
            "a byte-swapped 348 is a big-endian NIfTI-1, and the error should say so"
        );
        for (name, hdr) in [
            ("n2.nii", 540i32.to_le_bytes()),
            ("n2-be.nii", 540i32.to_be_bytes()),
        ] {
            assert!(named(name, hdr).contains("NIfTI-2"), "{name}");
        }
        // anything else is simply not NIfTI, and the message quotes what was found.
        assert!(named("junk.nii", 1234i32.to_le_bytes()).contains("1234"));
    }

    /// The datatype ladder, and the guard that makes the §P2 bounds audit hold: `dtype_width` is the
    /// **only** bytes-per-voxel table in the decoder (the decode arms take their stride from it), so a
    /// width can never disagree with the type that reads it — which is what would turn the one checked
    /// `n * width` bound back into an out-of-range read. Each code is exercised here at its real width.
    /// NIfTI uint8 widens losslessly to uint16 (Tessera's 16-bit dtype floor).
    #[test]
    fn decodes_every_supported_datatype_at_its_declared_width() {
        let dir = tempfile::tempdir().unwrap();
        // one voxel per file: (datatype, bitpix, payload, expected Tessera dtype + value)
        let cases: Vec<(i16, i16, Vec<u8>, &str, f64)> = vec![
            (2, 8, vec![200u8], "uint16", 200.0), // uint8 → uint16
            (4, 16, (-1234i16).to_le_bytes().to_vec(), "int16", -1234.0),
            (512, 16, 60000u16.to_le_bytes().to_vec(), "uint16", 60000.0),
            (8, 32, (-70000i32).to_le_bytes().to_vec(), "int32", -70000.0),
            (16, 32, 0.5f32.to_le_bytes().to_vec(), "float32", 0.5),
            (64, 64, (-0.25f64).to_le_bytes().to_vec(), "float64", -0.25),
        ];
        for (code, bitpix, payload, dtype, value) in cases {
            let p = dir.path().join(format!("dt-{code}.nii"));
            Synth::new(&[1, 1, 1])
                .datatype(code, bitpix)
                .write_raw(&p, &payload);
            let img = read_nifti(&p).unwrap_or_else(|e| panic!("datatype {code}: {e}"));
            assert_eq!(img.data.dtype(), dtype, "datatype {code}");
            assert_eq!(img.data.len(), 1, "datatype {code}");
            assert_eq!(img.data.as_f64(), vec![value], "datatype {code}");
        }

        // an unsupported code is rejected before any size arithmetic runs on it.
        let p = dir.path().join("dt-rgb.nii");
        Synth::new(&[1, 1, 1])
            .datatype(128, 24)
            .write_raw(&p, &[0; 3]); // NIFTI_TYPE_RGB24
        assert!(matches!(read_nifti(&p), Err(Error::Invalid(_))));
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

        // The bomb guard: the gunzip is capped at the size the *header* declares, and a header
        // declaring more than the in-memory ceiling is refused before a byte of payload is expanded.
        let bomb = dir.path().join("bomb.nii.gz");
        let huge = Synth::new(&[32767, 32767, 8]); // 8.6e9 int16 voxels ≈ 17 GiB
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&huge.header()).unwrap();
        std::fs::write(&bomb, enc.finish().unwrap()).unwrap();
        match read_nifti(&bomb).map(|img| img.shape) {
            Err(Error::Invalid(msg)) => assert!(msg.contains("ceiling"), "{msg}"),
            other => panic!("a gz declaring 17 GiB must be refused, got {other:?}"),
        }
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
        // ADR-0056 §11 asks for *which* transform the geometry came from to be recorded.
        assert_eq!(img.geometry_source, Some(GeometrySource::Qform));

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
        let img = read("both.nii", Synth::new(&[2, 2, 2]).qform(1, rot_x90, 1.0));
        assert_eq!(img.geometry_source, Some(GeometrySource::Sform));
        approx(
            img.world_frame.unwrap().voxel_to_world([1.0, 1.0, 1.0]),
            [-12.0, -23.0, 34.0],
        );

        // a degenerate sform is not a usable frame — fall through to the qform rather than sealing a
        // singular affine (which `ArraySpec::validate` would reject far downstream).
        let mut degenerate = Synth::new(&[2, 2, 2]).qform(1, rot_x90, 1.0);
        degenerate.srows = [[0.; 4]; 3];
        let img = read("degenerate-sform.nii", degenerate);
        assert_eq!(img.geometry_source, Some(GeometrySource::Qform));
        approx(
            img.world_frame.unwrap().voxel_to_world([1.0, 1.0, 1.0]),
            [-12.0, -16.0, 33.0],
        );
        // …and with nothing to fall through to it is a typed error, not a singular affine that
        // `ArraySpec::validate` would only reject at seal time.
        let mut orphan = Synth::new(&[2, 2, 2]);
        orphan.srows = [[0.; 4]; 3];
        let alone = dir.path().join("degenerate-only.nii");
        orphan.write(&alone, &voxels);
        assert!(matches!(read_nifti(&alone), Err(Error::Invalid(_))));

        // neither code set → index space, unchanged (ADR-0029 feature-by-presence).
        let img = read("none.nii", Synth::new(&[2, 2, 2]).no_sform());
        assert!(img.world_frame.is_none());
        assert_eq!(img.geometry_source, None);
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
        assert_eq!(img.axes, vec!["t", "z", "y", "x"]);
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

        // the t axis carries its ADR-0032 referencing, reusing `time_regular` verbatim: TR 2.5 s from
        // pixdim[4], start 1 s from toffset, both scaled by the xyzt_units time code. The spatial axes
        // stay bare storage indices.
        let axref = img
            .axis_referencing
            .clone()
            .expect("a timed 4-D series carries a referenced time axis");
        assert_eq!(axref.len(), 4);
        assert_eq!(axref[0], Some(Referenced::time_regular(1.0, 2.5)));
        assert!(axref[1..].iter().all(Option::is_none));

        // …and a rank-4 volume still seals into a verifying recon product that carries both.
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
        let spec: ArraySpec = serde_json::from_value(m.blocks[0].spec.clone()).unwrap();
        assert_eq!(spec.axes, vec!["t", "z", "y", "x"]);
        assert_eq!(spec.axis_referencing, Some(axref));
        spec.validate().unwrap();

        // A DWI direction stack is the same shape with a different meaning: dim[4] counts gradient
        // directions, and such files leave pixdim[4] / the time unit unset. That axis then stays a bare
        // index rather than carrying a guessed cadence — this decoder invents no axis convention.
        let dwi = dir.path().join("dwi.nii");
        Synth::new(&[nx, ny, nz, 6]).write(&dwi, &voxels[..per_volume * 6]);
        let img_dwi = read_nifti(&dwi).unwrap();
        assert_eq!(img_dwi.shape, vec![6, 2, 2, 2]);
        assert_eq!(img_dwi.axis_referencing, None);

        // 5-D reads all of it too, named with NIfTI's own axis letters reversed.
        let five = dir.path().join("five.nii");
        Synth::new(&[nx, ny, nz, 3, 2]).write(&five, &voxels[..per_volume * 6]);
        let img5 = read_nifti(&five).unwrap();
        assert_eq!(img5.shape, vec![2, 3, 2, 2, 2]);
        assert_eq!(img5.axes, vec!["u", "t", "z", "y", "x"]);
        assert!(matches!(img5.data, ArrayData::I16(ref v) if v == &voxels[..per_volume * 6]));

        // `ndim = 4, dim[4] = 1` is the 3-D volume it says it is — no gratuitous rank bump (which
        // would move the `content_hash` of every single-frame 4-D file in existence).
        let flat = dir.path().join("single-frame.nii");
        Synth::new(&[2, 2, 2, 1]).write(&flat, &voxels[..per_volume]);
        assert_eq!(read_nifti(&flat).unwrap().shape, vec![2, 2, 2]);
    }
}
