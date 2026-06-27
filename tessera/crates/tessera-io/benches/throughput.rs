//! Streaming-write **throughput** bench (#203 / ADR-0026) — drives the real [`TableStreamWriter`] (the
//! DAQ-facing bounded-memory ingest path) with synthetic listmode batches, so an operator can measure the
//! sustainable rows/s on **their own** hardware: `cargo bench -p tessera-io --bench throughput`.
//!
//! Reuses the production write path (no re-implementation — same SSoT as the format), pushing
//! deliberately non-grid-aligned batches so the spill + lazy-compaction the streaming engine does for a
//! real acquisition is exercised. Wall-clock is machine-dependent (criterion reports elements/s); the
//! point is a number an operator can compare against their acquisition rate to size RAM/throughput.

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use tessera_core::block::table::{Column, TableSpec};
use tessera_io::{ColumnData, TableData, TableStreamWriter};

/// A listmode-like 6-column schema (ms timestamp + 2 energies + 2 crystal ids + 1 axial block).
fn listmode_columns() -> Vec<Column> {
    ["ms", "en0", "en1", "tx0", "tx1", "ax0"]
        .iter()
        .zip(["u4", "f4", "f4", "u2", "u2", "u1"])
        .map(|(n, d)| Column {
            name: (*n).into(),
            dtype: d.into(),
            codec: None,
        })
        .collect()
}

/// A synthetic batch of `n` listmode events starting at row `start` (deterministic, representative
/// value ranges — 511 keV energy peak, monotonic timestamps).
fn batch(start: usize, n: usize) -> TableData {
    let r = start..start + n;
    vec![
        (
            "ms".into(),
            ColumnData::U32(r.clone().map(|k| k as u32).collect()),
        ),
        (
            "en0".into(),
            ColumnData::F32(r.clone().map(|k| 511.0 + (k % 13) as f32).collect()),
        ),
        (
            "en1".into(),
            ColumnData::F32(r.clone().map(|k| 510.0 + (k % 11) as f32).collect()),
        ),
        (
            "tx0".into(),
            ColumnData::U16(r.clone().map(|k| (k % 4096) as u16).collect()),
        ),
        (
            "tx1".into(),
            ColumnData::U16(r.clone().map(|k| (k % 4096) as u16).collect()),
        ),
        (
            "ax0".into(),
            ColumnData::U8(r.map(|k| (k % 64) as u8).collect()),
        ),
    ]
}

fn bench_streaming_write(c: &mut Criterion) {
    const ROWS: usize = 1 << 20; // ~1.05M events
    let spec = TableSpec {
        columns: listmode_columns(),
        rows: ROWS as u64,
        row_index: Some("ms".into()),
    };

    let mut g = c.benchmark_group("streaming_write");
    g.throughput(Throughput::Elements(ROWS as u64)); // criterion reports events/s
    g.sample_size(10);
    g.bench_function("table_stream_writer_1M_listmode_events", |b| {
        b.iter(|| {
            let dir = tempfile::tempdir().unwrap();
            let mut w = TableStreamWriter::new(spec.clone(), &dir.path().join("stage")).unwrap();
            // push in 9999-row transport batches — deliberately NOT aligned to the 65536 row-group grid,
            // so full-group spill + the trailing-remainder + lazy compaction all run (the real DAQ shape).
            let mut pushed = 0usize;
            while pushed < ROWS {
                let n = 9999.min(ROWS - pushed);
                w.push(batch(pushed, n)).unwrap();
                pushed += n;
            }
            w.finish().unwrap()
        })
    });
    g.finish();
}

criterion_group!(benches, bench_streaming_write);
criterion_main!(benches);
