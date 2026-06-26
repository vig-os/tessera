//! #221-A — sparse representation crossover: **dense** (Zarr v3 + pcodec) vs **COO** (Vortex table).
//!
//! ADR-0031 sends *scatter*-sparse grids to a COO `(coords…, value)` table and *block*-sparse grids to
//! a dense chunked array (all-zero chunks prune + pcodec crushes zero-runs). The boundary is empirical:
//! pcodec compresses zeros so well that COO only wins below some occupancy, and that crossover depends
//! on *structure* as much as occupancy. This measures it — on-disk size + materialize — to set the
//! ADR-0031 §5 default threshold. Storage/interchange metrics only (no in-mem sparse math — that's
//! scipy's layer, per the ADR).
//!
//! Run: `cargo run -p tessera-io --example spike_sparse --release`
//! Deterministic (splitmix64 masks); numbers land in SPIKE-RESULTS.md.

use std::time::Instant;

use tessera_core::block::array::ArraySpec;
use tessera_core::block::table::{Column, TableSpec};
use tessera_io::array::{self, ArrayData};
use tessera_io::table::{self, ColumnData, TableData};

/// Best (min) wall-clock over `iters` runs of `f`, in microseconds.
fn best_us(iters: u32, mut f: impl FnMut()) -> f64 {
    let mut best = f64::INFINITY;
    for _ in 0..iters {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64() * 1e6);
    }
    best
}

/// splitmix64 — deterministic, well-distributed; drives reproducible occupancy masks.
fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A nonzero, CT-like sample value for a present voxel (never 0, so "absent == 0" is unambiguous).
fn val(k: usize) -> i16 {
    let v = ((k % 1700) as i32 - 850) as i16;
    if v == 0 {
        1
    } else {
        v
    }
}

#[derive(Clone, Copy)]
enum Structure {
    /// Nonzeros spread uniformly — the genuine scatter regime (no whole-chunk skips).
    Scatter,
    /// Nonzeros confined to the first `occ·n` z-slices — clustered along one axis.
    Banded,
    /// Nonzeros confined to one contiguous corner sub-cube — fully block-clustered.
    Block,
}

impl Structure {
    fn name(self) -> &'static str {
        match self {
            Structure::Scatter => "scatter",
            Structure::Banded => "banded",
            Structure::Block => "block",
        }
    }
}

/// Build a dense `n³` int16 volume with ~`occ` fraction of present (nonzero) voxels in `structure`.
fn gen(n: usize, occ: f64, structure: Structure) -> Vec<i16> {
    let total = n * n * n;
    let mut v = vec![0i16; total];
    match structure {
        Structure::Scatter => {
            // include voxel k iff its hashed value falls in the lowest `occ` of the u64 range.
            let thresh = (occ * (u64::MAX as f64)) as u64;
            for (k, slot) in v.iter_mut().enumerate() {
                if mix(k as u64) < thresh {
                    *slot = val(k);
                }
            }
        }
        Structure::Banded => {
            let nz_slices = ((occ * n as f64).round() as usize).clamp(1, n);
            for (k, slot) in v.iter_mut().enumerate().take(nz_slices * n * n) {
                *slot = val(k);
            }
        }
        Structure::Block => {
            // contiguous corner cube of side ≈ cbrt(occ·total).
            let side = (((occ * total as f64).cbrt()).round() as usize).clamp(1, n);
            for z in 0..side {
                for y in 0..side {
                    for x in 0..side {
                        let k = (z * n + y) * n + x;
                        v[k] = val(k);
                    }
                }
            }
        }
    }
    v
}

fn col(name: &str, dt: &str) -> Column {
    Column {
        name: name.into(),
        dtype: dt.into(),
        codec: None,
    }
}

/// COO form **3c**: one column per axis `(z, y, x, value)` — the naïve layout (8 B/nnz).
fn coo_3col(dense: &[i16], n: usize) -> (TableSpec, TableData) {
    let (mut zc, mut yc, mut xc, mut vc) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (k, &d) in dense.iter().enumerate() {
        if d != 0 {
            zc.push((k / (n * n)) as u16);
            yc.push(((k / n) % n) as u16);
            xc.push((k % n) as u16);
            vc.push(d);
        }
    }
    let spec = TableSpec {
        columns: vec![
            col("z", "u2"),
            col("y", "u2"),
            col("x", "u2"),
            col("v", "i2"),
        ],
        rows: vc.len() as u64,
        row_index: None,
    };
    let data: TableData = vec![
        ("z".into(), ColumnData::U16(zc)),
        ("y".into(), ColumnData::U16(yc)),
        ("x".into(), ColumnData::U16(xc)),
        ("v".into(), ColumnData::I16(vc)),
    ];
    (spec, data)
}

/// COO form **lin**: a single **linear index** column (`idx`, strictly ascending in raster order →
/// Vortex delta-compresses it) + value. The *fair* COO — 6 B/nnz, best-compressible coordinate.
fn coo_lin(dense: &[i16], _n: usize) -> (TableSpec, TableData) {
    let (mut ic, mut vc) = (Vec::new(), Vec::new());
    for (k, &d) in dense.iter().enumerate() {
        if d != 0 {
            ic.push(k as u32);
            vc.push(d);
        }
    }
    let spec = TableSpec {
        columns: vec![col("idx", "u4"), col("v", "i2")],
        rows: vc.len() as u64,
        row_index: None,
    };
    let data: TableData = vec![
        ("idx".into(), ColumnData::U32(ic)),
        ("v".into(), ColumnData::I16(vc)),
    ];
    (spec, data)
}

fn main() {
    let n = 128usize;
    let total = n * n * n;
    let occs = [0.0001, 0.001, 0.01, 0.05, 0.10, 0.25, 0.50];

    println!("# #221-A — sparse crossover: dense (zarr+pcodec) vs COO (Vortex table)");
    println!(
        "# volume {n}³ = {total} int16 voxels ({} MiB raw)\n",
        total * 2 / (1 << 20)
    );

    let mut aspec = ArraySpec::new(vec![n as u64; 3], "int16");
    aspec.codec = "pcodec".into();

    for structure in [Structure::Scatter, Structure::Banded, Structure::Block] {
        println!("## {}", structure.name());
        println!(
            "{:>8} {:>10} {:>7} {:>11} {:>11} {:>11} {:>8} {:>9} {:>9}",
            "occ%",
            "nnz",
            "nnz%",
            "dense KiB",
            "coo3c KiB",
            "cooLin KiB",
            "winner",
            "lin/den",
            "mat ×"
        );
        for &occ in &occs {
            let dense = gen(n, occ, structure);
            let nnz = dense.iter().filter(|&&d| d != 0).count();
            let nnz_pct = 100.0 * nnz as f64 / total as f64;

            let dense_blob = array::encode(&aspec, &ArrayData::I16(dense.clone())).unwrap();
            let (s3, d3) = coo_3col(&dense, n);
            let coo3_blob = table::encode(&s3, &d3).unwrap();
            let (sl, dl) = coo_lin(&dense, n);
            let coolin_blob = table::encode(&sl, &dl).unwrap();

            let best_coo = coo3_blob.len().min(coolin_blob.len());
            let ratio = coolin_blob.len() as f64 / dense_blob.len() as f64;
            let winner = if best_coo < dense_blob.len() {
                "COO"
            } else {
                "dense"
            };

            // materialize cost: dense decode vs best-COO (lin) decode — relative working-set proxy.
            // 15 iters (not 3) — decode timing is noisy; the low-occupancy regime is the robust signal.
            let mat_dense = best_us(15, || {
                array::decode(&aspec, &dense_blob).unwrap();
            });
            let mat_coo = best_us(15, || {
                table::decode(&sl, &coolin_blob).unwrap();
            });
            let mat_ratio = mat_coo / mat_dense;

            println!(
                "{:>8.3} {:>10} {:>6.2}% {:>11.1} {:>11.1} {:>11.1} {:>8} {:>9.3} {:>8.2}×",
                occ * 100.0,
                nnz,
                nnz_pct,
                dense_blob.len() as f64 / 1024.0,
                coo3_blob.len() as f64 / 1024.0,
                coolin_blob.len() as f64 / 1024.0,
                winner,
                ratio,
                mat_ratio
            );
        }
        println!();
    }
    println!("# winner = smaller on-disk (best of the two COO forms vs dense);");
    println!(
        "# lin/den = linear-COO bytes ÷ dense bytes (<1 → COO wins); mat × = COO ÷ dense decode."
    );
    println!("# Crossover (scatter) sets ADR-0031 §5.");
}
