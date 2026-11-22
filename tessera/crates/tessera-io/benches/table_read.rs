//! Table read benches at list-mode scale (#828, design note #915 §4/§5 A4–A5): random take,
//! projection and full-scan decode of a table block written by this build's codec profile.
//!
//! Run: `cargo bench -p tessera-io --bench table_read` (pin it, e.g. `taskset -c 3`, for a
//! per-core number). Wall-clock floors are machine-dependent, so these are measured, not gated in
//! CI; the floors they are read against are FEATURE-MATRIX §D (take ×500 ≤ 30 ms and projection
//! ≤ 15 ms on a 100k-row table) and the #828 acceptance (take ×500 on a full 2^22-row block
//! ≤ 100 ms, full-scan decode ≥ 500 MiB/s per core). The numbers on real list-mode come from
//! `examples/table_profile.rs` on the #822 fixtures; this bench uses a synthetic table of the GE
//! list-mode shape (5 × `i2` + `t_ms` `u4`, 14 B/event) so it runs anywhere.

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use tessera_core::block::table::{Column, TableSpec};
use tessera_io::table::{self, ColumnData, TableData, BLOCK_ROWS};

/// A GE-list-mode-shaped table of `rows` events: crystal indices with a realistic range, a signed
/// timing difference, and a millisecond clock that rises slowly. Deterministic.
fn listmode(rows: usize) -> (TableSpec, TableData) {
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move |m: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % m
    };
    let mut col = |m: u64, off: i64| -> ColumnData {
        ColumnData::I16((0..rows).map(|_| (next(m) as i64 + off) as i16).collect())
    };
    let data: TableData = vec![
        ("hi_ax".into(), col(54, 0)),
        ("hi_tx".into(), col(544, 0)),
        ("lo_ax".into(), col(54, 0)),
        ("lo_tx".into(), col(544, 0)),
        ("delta_t".into(), col(301, -150)),
        (
            "t_ms".into(),
            ColumnData::U32((0..rows).map(|k| 12_656 + (k / 1_600) as u32).collect()),
        ),
    ];
    let spec = TableSpec {
        columns: vec![
            Column::new("hi_ax", "i2"),
            Column::new("hi_tx", "i2"),
            Column::new("lo_ax", "i2"),
            Column::new("lo_tx", "i2"),
            Column::new("delta_t", "i2"),
            Column::new("t_ms", "u4"),
        ],
        rows: rows as u64,
        row_index: None,
        partition: None,
        edition: None,
        codec_profile: None,
    };
    (spec, data)
}

/// `k` sorted, distinct, uniformly spread row indices of `0..n` (a fixed sequence).
fn uniform_rows(n: u64, k: usize) -> Vec<u64> {
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    let mut v: Vec<u64> = (0..k)
        .map(|_| {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (x >> 11) % n
        })
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn bench_full_block(c: &mut Criterion) {
    let (spec, data) = listmode(BLOCK_ROWS);
    let blob = table::encode(&spec, &data).unwrap();
    let mut g = c.benchmark_group("table_listmode_block_2pow22");
    g.sample_size(10);
    let take = uniform_rows(spec.rows, 500);
    g.bench_function("take_x500_all_columns", |b| {
        b.iter(|| table::decode_rows(&spec, &blob, &take).unwrap())
    });
    g.bench_function("project_t_ms", |b| {
        b.iter(|| table::decode_projected(&spec, &blob, &["t_ms"]).unwrap())
    });
    g.throughput(Throughput::Bytes(spec.rows * 14));
    g.bench_function("decode_full_scan", |b| {
        b.iter(|| table::decode(&spec, &blob).unwrap())
    });
    g.finish();
}

fn bench_100k(c: &mut Criterion) {
    let (spec, data) = listmode(100_000);
    let blob = table::encode(&spec, &data).unwrap();
    let take = uniform_rows(spec.rows, 500);
    let mut g = c.benchmark_group("table_listmode_100k");
    g.bench_function("take_x500_all_columns", |b| {
        b.iter(|| table::decode_rows(&spec, &blob, &take).unwrap())
    });
    g.bench_function("project_hi_tx", |b| {
        b.iter(|| table::decode_projected(&spec, &blob, &["hi_tx"]).unwrap())
    });
    g.finish();
}

criterion_group!(benches, bench_full_block, bench_100k);
criterion_main!(benches);
