//! Perf-SLA benches (ROADMAP P4 / #206) — Rust-measured throughput + size for the storage codecs,
//! against the FEATURE-MATRIX §D floors. Run: `cargo bench -p tessera-io`. Wall-clock floors are
//! machine-dependent (the §D numbers are the 88-core bench box), so these benches are *measured, not
//! gated in CI*; the machine-independent compression-ratio floor is enforced as a unit test
//! (`array::tests`/`table::tests`). Compression ratios are recorded in FEATURE-MATRIX §D.

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use tessera_core::block::array::ArraySpec;
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::hash;
use tessera_io::array::{self, ArrayData};
use tessera_io::table::{self, ColumnData, TableData};

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

criterion_group!(benches, bench_array, bench_hash, bench_table);
criterion_main!(benches);
