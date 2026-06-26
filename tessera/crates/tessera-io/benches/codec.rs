//! Perf-SLA benches (ROADMAP P4 / #206) — Rust-measured throughput + size for the storage codecs,
//! against the FEATURE-MATRIX §D floors. Run: `cargo bench -p tessera-io`. Wall-clock floors are
//! machine-dependent (the §D numbers are the 88-core bench box), so these benches are *measured, not
//! gated in CI*; the machine-independent compression-ratio floor is enforced as a unit test
//! (`array::tests`/`table::tests`). Compression ratios are recorded in FEATURE-MATRIX §D.

use std::io::Cursor;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use tessera_core::block::array::ArraySpec;
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::{hash, ProductBuilder};
use tessera_io::array::{self, ArrayData};
use tessera_io::table::{self, ColumnData, TableData};
use tessera_io::{pack, Reader};

/// A smooth, CT-like int16 volume (gradient along z) — representative of real recon data so the
/// pcodec ratio is meaningful, not the ~100× of a pure ramp.
fn ct_like_volume() -> (ArraySpec, ArrayData, u64) {
    let mut spec = ArraySpec::new(vec![64, 64, 64], "int16");
    spec.codec = "pcodec".into();
    let data = ArrayData::I16(
        (0..64 * 64 * 64)
            .map(|k| {
                let z = k / (64 * 64);
                let y = (k / 64) % 64;
                (z * 16 + y * 2 - 1024) as i16
            })
            .collect(),
    );
    (spec, data, (64 * 64 * 64 * 2) as u64)
}

fn bench_array(c: &mut Criterion) {
    let (spec, data, raw) = ct_like_volume();
    let blob = array::encode(&spec, &data).unwrap();
    let mut g = c.benchmark_group("array_pcodec_int16_64cubed");
    g.throughput(Throughput::Bytes(raw));
    g.bench_function("encode", |b| {
        b.iter(|| array::encode(&spec, &data).unwrap())
    });
    g.bench_function("decode", |b| {
        b.iter(|| array::decode(&spec, &blob).unwrap())
    });
    g.finish();
}

fn bench_hash(c: &mut Criterion) {
    let buf = vec![0xA5u8; 8 * 1024 * 1024]; // 8 MiB
    let mut g = c.benchmark_group("blake3");
    g.throughput(Throughput::Bytes(buf.len() as u64));
    g.bench_function("digest_8MiB", |b| b.iter(|| hash::digest(&buf)));
    g.finish();
}

fn bench_table(c: &mut Criterion) {
    let n = 100_000usize;
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
        rows: n as u64,
        row_index: None,
    };
    let data: TableData = vec![
        ("t".into(), ColumnData::U64((0..n as u64).collect())),
        (
            "e0".into(),
            ColumnData::F32((0..n).map(|k| k as f32 * 0.01).collect()),
        ),
        (
            "e1".into(),
            ColumnData::F32((0..n).map(|k| (k as f32 * 0.7).sin()).collect()),
        ),
    ];
    let raw = (n * (8 + 4 + 4)) as u64;
    let blob = table::encode(&spec, &data).unwrap();
    let mut g = c.benchmark_group("table_vortex_100k");
    g.throughput(Throughput::Bytes(raw));
    g.bench_function("encode", |b| {
        b.iter(|| table::encode(&spec, &data).unwrap())
    });
    g.bench_function("decode", |b| {
        b.iter(|| table::decode(&spec, &blob).unwrap())
    });
    g.finish();
}

/// `.tsra` container cost vs the bare codec store, isolated: same 128³ int16 volume (8× 64³ chunks),
/// same machine/run. "bare" = the zarrs+pcodec store blob with no container; ".tsra" = that blob
/// packed into the sealed zip64 (manifest + blake3 seal). The delta is the pure container tax.
/// Also times a 3-D ROI sub-cube (chunked access — only the intersecting chunk is decoded).
fn bench_tsra_vs_bare(c: &mut Criterion) {
    let mut spec = ArraySpec::new(vec![128, 128, 128], "int16"); // chunks default to 64³ → 8 chunks
    spec.codec = "pcodec".into();
    let data = ArrayData::I16(
        (0..128 * 128 * 128)
            .map(|k| {
                let z = k / (128 * 128);
                let y = (k / 128) % 128;
                (z * 8 + y * 2 - 1024) as i16
            })
            .collect(),
    );
    let raw = (128u64 * 128 * 128) * 2;

    // Pre-build the bare blob and a packed .tsra (read side reads these; write side rebuilds).
    let blob = array::encode(&spec, &data).unwrap();
    let build_tsra = || {
        let (r, p) = array::array_block("volume", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("recon", "bench", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(r);
        (b.seal().unwrap(), p)
    };
    let (sealed, payload) = build_tsra();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.tsra");
    pack(&sealed, std::slice::from_ref(&payload), &path).unwrap();
    let tsra_bytes = std::fs::read(&path).unwrap();

    let mut g = c.benchmark_group("tsra_vs_bare_int16_128cubed");
    g.throughput(Throughput::Bytes(raw));
    // WRITE
    g.bench_function("write_bare_codec", |b| {
        b.iter(|| array::encode(&spec, &data).unwrap())
    });
    g.bench_function("write_tsra_full", |b| {
        b.iter(|| {
            let (sealed, payload) = build_tsra();
            let p = dir.path().join("w.tsra");
            pack(&sealed, std::slice::from_ref(&payload), &p).unwrap();
        })
    });
    // READ (full)
    g.bench_function("read_bare_codec", |b| {
        b.iter(|| array::decode(&spec, &blob).unwrap())
    });
    g.bench_function("read_tsra_full", |b| {
        b.iter(|| {
            let mut rd = Reader::from_reader(Cursor::new(tsra_bytes.clone())).unwrap();
            let bytes = rd.read_block("volume").unwrap();
            array::decode(&spec, &bytes).unwrap()
        })
    });
    // 3-D ROI sub-cube (chunked access): a 32³ corner touches 1 of 8 chunks
    g.bench_function("roi_subcube_32_of_128", |b| {
        b.iter(|| array::decode_subset(&spec, &blob, &[0, 0, 0], &[32, 32, 32]).unwrap())
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_array,
    bench_hash,
    bench_table,
    bench_tsra_vs_bare
);
criterion_main!(benches);
