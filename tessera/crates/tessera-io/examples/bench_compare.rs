//! Cross-substrate comparison report (#143): Tessera `.tsra` I/O vs the **bare backends it wraps**
//! (vanilla Zarr+pcodec for arrays, vanilla Vortex for tables), across a file-size ladder and the
//! access patterns that matter for imaging: write, full sequential read, 3-D ROI sub-cube (chunked
//! access), a single Z-slice (slicing), and a single-voxel random read.
//!
//! Run: `cargo run -p tessera-io --example bench_compare --release`
//! This is a *measurement tool*, not a CI gate — wall-clock is machine-dependent (see FEATURE-MATRIX
//! §D). It prints an ALOCA table; the numbers below it land in SPIKE-RESULTS.md.

use std::io::Cursor;
use std::time::Instant;

use tessera_core::block::array::ArraySpec;
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::ProductBuilder;
use tessera_io::array::{self, ArrayData};
use tessera_io::table::{self, ColumnData, TableData};
use tessera_io::{pack, Reader};

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

fn mbps(bytes: u64, us: f64) -> f64 {
    (bytes as f64 / 1e6) / (us / 1e6)
}

/// A smooth CT-like int16 volume (gradient in z+y) — realistic pcodec ratio, not a pure-ramp ~100×.
fn ct_like(n: usize) -> ArrayData {
    ArrayData::I16(
        (0..(n * n * n))
            .map(|k| {
                let z = (k / (n * n)) as i64;
                let y = ((k / n) % n) as i64;
                (z * 8 + y * 2 - 1024) as i16
            })
            .collect(),
    )
}

fn array_ladder() {
    println!("\n## Arrays — .tsra vs vanilla Zarr+pcodec (int16, CT-like, 64³ chunks)\n");
    println!(
        "{:>5} {:>9} {:>7} {:>11} {:>11} {:>11} {:>11} {:>10} {:>9} {:>9}",
        "n³",
        "raw",
        "ratio",
        "wr_bare",
        "wr_tsra",
        "rd_bare",
        "rd_tsra",
        "tsra+%",
        "roi×",
        "slice×"
    );
    println!(
        "{:>5} {:>9} {:>7} {:>11} {:>11} {:>11} {:>11} {:>10} {:>9} {:>9}",
        "", "MiB", "x", "MB/s", "MB/s", "MB/s", "MB/s", "size", "vs full", "vs full"
    );
    for &n in &[32usize, 64, 128, 256] {
        let mut spec = ArraySpec::new(vec![n as u64, n as u64, n as u64], "int16");
        spec.codec = "pcodec".into();
        let data = ct_like(n);
        let raw = (n * n * n * 2) as u64;
        let iters = if n >= 256 { 3 } else { 8 };

        let blob = array::encode(&spec, &data).unwrap();
        let ratio = raw as f64 / blob.len() as f64;

        // build a packed .tsra once for the read side / size measurement
        let (r, p) = array::array_block("volume", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("recon", "bench", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(r);
        let sealed = b.seal().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.tsra");
        pack(&sealed, std::slice::from_ref(&p), &path).unwrap();
        let tsra = std::fs::read(&path).unwrap();
        let tsra_over = (tsra.len() as f64 / blob.len() as f64 - 1.0) * 100.0;

        let wr_bare = best_us(iters, || {
            array::encode(&spec, &data).unwrap();
        });
        let wr_tsra = best_us(iters, || {
            let (r, p) = array::array_block("volume", &spec, &data).unwrap();
            let mut b = ProductBuilder::new("recon", "bench", "d", "2024-01-01T00:00:00Z");
            b.add_block_ref(r);
            let s = b.seal().unwrap();
            let pth = dir.path().join("w.tsra");
            pack(&s, std::slice::from_ref(&p), &pth).unwrap();
        });
        let rd_bare = best_us(iters, || {
            array::decode(&spec, &blob).unwrap();
        });
        let rd_tsra = best_us(iters, || {
            let mut rd = Reader::from_reader(Cursor::new(tsra.clone())).unwrap();
            let bytes = rd.read_block("volume").unwrap();
            array::decode(&spec, &bytes).unwrap();
        });
        // 3-D ROI sub-cube (half-extent corner) — chunked access
        let h = (n / 2) as u64;
        let roi = best_us(iters, || {
            array::decode_subset(&spec, &blob, &[0, 0, 0], &[h, h, h]).unwrap();
        });
        // single Z-slice [1, n, n] — slicing
        let slice = best_us(iters, || {
            array::decode_subset(&spec, &blob, &[0, 0, 0], &[1, n as u64, n as u64]).unwrap();
        });

        println!(
            "{:>5} {:>9.2} {:>7.1} {:>11.0} {:>11.0} {:>11.0} {:>11.0} {:>9.1}% {:>8.1}× {:>8.1}×",
            n,
            raw as f64 / (1024.0 * 1024.0),
            ratio,
            mbps(raw, wr_bare),
            mbps(raw, wr_tsra),
            mbps(raw, rd_bare),
            mbps(raw, rd_tsra),
            tsra_over,
            rd_bare / roi,
            rd_bare / slice,
        );
    }
}

/// A listmode-like table: u64 timestamp + 2 f32 energies (realistic Vortex ratio).
fn listmode(rows: usize) -> (TableSpec, TableData) {
    let spec = TableSpec {
        columns: vec![
            Column {
                name: "t".into(),
                dtype: "u8".into(),
                codec: None,
            },
            Column {
                name: "e0".into(),
                dtype: "f4".into(),
                codec: None,
            },
            Column {
                name: "e1".into(),
                dtype: "f4".into(),
                codec: None,
            },
        ],
        rows: rows as u64,
        row_index: Some("t".into()),
    };
    let data: TableData = vec![
        ("t".into(), ColumnData::U64((0..rows as u64).collect())),
        (
            "e0".into(),
            ColumnData::F32((0..rows).map(|k| 511.0 + (k % 7) as f32).collect()),
        ),
        (
            "e1".into(),
            ColumnData::F32((0..rows).map(|k| 510.0 - (k % 5) as f32).collect()),
        ),
    ];
    (spec, data)
}

fn table_ladder() {
    println!("\n## Tables — .tsra vs vanilla Vortex (u64 + 2×f32 listmode)\n");
    println!(
        "{:>10} {:>9} {:>7} {:>11} {:>11} {:>11} {:>11} {:>10}",
        "rows", "raw", "ratio", "wr_bare", "wr_tsra", "rd_bare", "rd_tsra", "tsra+%"
    );
    println!(
        "{:>10} {:>9} {:>7} {:>11} {:>11} {:>11} {:>11} {:>10}",
        "", "MiB", "x", "MB/s", "MB/s", "MB/s", "MB/s", "size"
    );
    for &rows in &[10_000usize, 100_000, 1_000_000] {
        let (spec, data) = listmode(rows);
        let raw = (rows * 16) as u64; // 8 + 4 + 4 bytes/row
        let iters = if rows >= 1_000_000 { 3 } else { 8 };

        let blob = table::encode(&spec, &data).unwrap();
        let ratio = raw as f64 / blob.len() as f64;

        let (r, p) = table::table_block("events", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("listmode", "bench", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(r);
        let sealed = b.seal().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.tsra");
        pack(&sealed, std::slice::from_ref(&p), &path).unwrap();
        let tsra = std::fs::read(&path).unwrap();
        let tsra_over = (tsra.len() as f64 / blob.len() as f64 - 1.0) * 100.0;

        let wr_bare = best_us(iters, || {
            table::encode(&spec, &data).unwrap();
        });
        let wr_tsra = best_us(iters, || {
            let (r, p) = table::table_block("events", &spec, &data).unwrap();
            let mut b = ProductBuilder::new("listmode", "bench", "d", "2024-01-01T00:00:00Z");
            b.add_block_ref(r);
            let s = b.seal().unwrap();
            let pth = dir.path().join("w.tsra");
            pack(&s, std::slice::from_ref(&p), &pth).unwrap();
        });
        let rd_bare = best_us(iters, || {
            table::decode(&spec, &blob).unwrap();
        });
        let rd_tsra = best_us(iters, || {
            let mut rd = Reader::from_reader(Cursor::new(tsra.clone())).unwrap();
            let bytes = rd.read_block("events").unwrap();
            table::decode(&spec, &bytes).unwrap();
        });

        println!(
            "{:>10} {:>9.2} {:>7.1} {:>11.0} {:>11.0} {:>11.0} {:>11.0} {:>9.1}%",
            rows,
            raw as f64 / (1024.0 * 1024.0),
            ratio,
            mbps(raw, wr_bare),
            mbps(raw, wr_tsra),
            mbps(raw, rd_bare),
            mbps(raw, rd_tsra),
            tsra_over,
        );
    }
}

fn main() {
    println!("# Tessera I/O cross-substrate comparison (#143)");
    println!("# .tsra = sealed zip64 (manifest + blake3) over the SAME bare codec blob.");
    println!(
        "# 'bare' = vanilla Zarr+pcodec (arrays) / Vortex (tables) — the substrate Tessera wraps."
    );
    array_ladder();
    table_ladder();
    println!(
        "\n(ratio = raw/compressed; roi×/slice× = speedup of the partial read vs full decode.)"
    );
}
