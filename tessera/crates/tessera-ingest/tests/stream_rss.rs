//! #458's load-bearing measurement: streaming a table ingest is bounded by a **block**, not the file.
//!
//! # Why this is its own test file
//!
//! Peak RSS is read from `/proc/self/status: VmHWM`, which is a **process-wide high-water mark** and
//! never falls. In a shared test binary any other test's allocation would raise it, so the assertion
//! would either go flaky or — worse — pass for a reason unrelated to the code. One test per file means
//! cargo gives it its own process, so the mark is this measurement's alone. (`cargo nextest` isolates
//! per test anyway; this keeps the guarantee under plain `cargo test` too.)
//!
//! # Why the assertion is "growth does not scale", not "growth < some fraction of the file"
//!
//! The obvious form — peak growth below a fraction of the decoded size — was tried first and **failed
//! honestly**: streaming a 16 MiB input grew the mark by ~40 MiB. Not because anything materialised the
//! file, but because the one-off costs do not scale with it — the Vortex writer's tokio runtime and
//! compression buffers, the sink's full row-group buffer, the encode pool. A budget expressed as a
//! fraction of the input measures those fixed costs, so loosening it until it passed would have made the
//! test assert nothing about streaming at all.
//!
//! The real claim is that memory is bounded by the **batch and the block**, so a larger input must not
//! need more. That is what is asserted: ingest a small input, then one **4x larger in the same process**,
//! and require the second to leave the high-water mark essentially untouched. `VmHWM` never falls, so the
//! second run's additional growth is exactly "what the bigger file cost beyond the smaller one" — zero if
//! bounded, and necessarily large if the path materialises its input. Fixed overheads are paid by the
//! first run and cancel out, which is what makes this robust across allocators and arches rather than a
//! ratchet.
//!
//! # Why `block_rows` is lowered, and why the first run is a discarded warm-up
//!
//! Two earlier versions of this test failed, and the reasons are worth recording rather than tuning away.
//!
//! `TableMultiBlockSink` dispatches at `finish()`, so a **block** is the encode unit: a product small
//! enough to be ONE block must hold that block to encode it. Both fixtures were far under `BLOCK_ROWS`
//! (4.19 M rows), so both were single-block and peak tracked the file by construction. Bounded memory is
//! a property of the multi-block partition — exactly what ADR-0026 claims ("peak bounded by workers, not
//! dataset") — so `block_rows` is lowered to `ROWS_PER_GROUP` through the seam the sink exposes for this.
//! Production keeps `BLOCK_ROWS`; what is under test is the partition's effect on peak.
//!
//! That left ~6 MiB of growth, which measurement then explained. Streaming four sizes in one process:
//!
//! | rows | blocks | added to peak |
//! | --- | --- | --- |
//! | 250 k | 4 | 41.6 MiB — every fixed cost |
//! | 1 M | 16 | 4.9 MiB |
//! | 4 M | 62 | **0 MiB** |
//! | 4 M again | 62 | 1.9 MiB |
//!
//! Sixteen times the data added **nothing**, while repeating the *same* size added 1.9 MiB. So the
//! residual is allocator warm-up and retention, not accumulation — the path is bounded. The test
//! therefore discards a first run as warm-up and measures the step from a 1x to a 4x input after it.
//!
//! The 1 M and 4 M rows of that table are the sizes the test now uses, and that is deliberate — see
//! [`EXTRA_BUDGET`]. An earlier version measured the 250 k step against a flat 4 MiB budget and went
//! **intermittent** under `cargo nextest`, where sibling test processes compete for memory: a budget
//! the same order of magnitude as its own noise floor is not a measurement. The budget is now a
//! fraction of what a file-proportional path would need, which both scales with the fixture and keeps
//! the failure mode it exists to catch an order of magnitude away from the noise.

#![cfg(feature = "parquet")]

use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use tessera_ingest::column_meta::ColumnMeta;
use tessera_ingest::decoder;

const TS: &str = "2024-03-01T12:00:00Z";
/// Bytes per row: two `i64` columns.
const ROW_BYTES: u64 = 16;
/// The small run. 1 M rows x 16 B = 16 MiB decoded, 16 blocks at `ROWS_PER_GROUP` per block.
const SMALL_ROWS: i64 = 1_000_000;
/// The large run: 4x the rows — 64 MiB decoded, ~62 blocks.
const LARGE_ROWS: i64 = SMALL_ROWS * 4;

/// What a file-proportional path would have to add, going from the small input to the large one: the
/// extra rows, decoded. ~48 MiB. The number the budget below is a fraction *of*, rather than an
/// absolute chosen by hand.
const EXTRA_DECODED: u64 = (LARGE_ROWS - SMALL_ROWS) as u64 * ROW_BYTES;

/// How much the 4x input may add to the high-water mark: a **quarter** of what holding the extra rows
/// would cost.
///
/// Expressed as a fraction, and these sizes chosen, because the first version of this test used a flat
/// 4 MiB against a 4 MiB small fixture and went **intermittent** — it passed alone and failed under
/// `cargo test --workspace` / `cargo nextest`, where ~20 sibling test processes compete for memory and
/// the allocator stops returning pages. A budget the same order of magnitude as its own noise floor is
/// not a measurement, and an intermittent gate is worse than a failing one: it teaches people to re-run.
///
/// The fix is signal, not slack. At these sizes the measured growth is ~0 (see the table above: 1 M to
/// 4 M added **nothing**), a file-proportional path must add the full 48 MiB, and 12 MiB sits an order
/// of magnitude above the few-MiB allocator noise while still failing such a path by 4x. The claim is
/// unchanged and the margin is now justified by both bounds rather than by one.
const EXTRA_BUDGET: u64 = EXTRA_DECODED / 4;

/// Both runs must produce several blocks, or the comparison is about one block growing rather than about
/// the partition bounding peak — which is what tripped the first two versions of this test.
const MIN_BLOCKS: usize = 8;

/// The encode pipeline's RAM budget, fixed so the bound under test is the declared one. At
/// `ROWS_PER_GROUP` x 2 x i64 = 1 MiB per block, this permits ~8 blocks in flight for both runs.
const RAM_BUDGET: u64 = 8 * 1024 * 1024;

/// `VmHWM:` (resident high-water mark, KiB) from `/proc/self/status`, in bytes.
fn peak_rss_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|line| {
                line.strip_prefix("VmHWM:")
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|kib| kib.parse::<u64>().ok())
                    .map(|kib| kib * 1024)
            })
        })
        .unwrap_or(0)
}

/// Write `rows` rows of two varied i64 columns, in modest row groups so the reader sees many batches.
fn write_fixture(path: &std::path::Path, rows: i64) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("v", DataType::Int64, false),
    ]));
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(64 * 1024))
        .build();
    let file = std::fs::File::create(path).unwrap();
    let mut w = ArrowWriter::try_new(file, schema.clone(), Some(props)).unwrap();
    let step = 64 * 1024i64;
    let mut start = 0i64;
    while start < rows {
        let n = step.min(rows - start);
        let id: ArrayRef = Arc::new(Int64Array::from_iter_values(start..start + n));
        let v: ArrayRef = Arc::new(Int64Array::from_iter_values(
            (start..start + n).map(|x| x * 7 % 1013),
        ));
        w.write(&RecordBatch::try_new(schema.clone(), vec![id, v]).unwrap())
            .unwrap();
        start += n;
    }
    w.close().unwrap();
}

/// Stream one fixture and return its sealed manifest.
fn stream(dir: &std::path::Path, input: &std::path::Path, tag: &str) -> tessera_core::Manifest {
    let stage = dir.join(format!("stage-{tag}"));
    std::fs::create_dir_all(&stage).unwrap();
    let out = dir.join(format!("{tag}.tsra"));
    let column_meta = ColumnMeta::default();
    // An EXPLICIT ram budget, not the machine-derived default. `ring_depth = ram_budget / unit_bytes`,
    // so with the default budget on a large host the encode queue may hold dozens of blocks and peak
    // grows with the block COUNT until the budget caps it — which is bounded, but bounded by a number
    // this machine chose rather than by anything the test states. ADR-0026's claim is "peak bounded by
    // workers and budget, not dataset", so the test fixes both and measures against them.
    let cfg = tessera_io::WriteConfig::default()
        .workers(1)
        .ram_budget(RAM_BUDGET);
    tessera_ingest::stream_table::stream_to_table_product(
        || tessera_ingest::parquet_table::parquet_chunks(input, 64 * 1024, &[]),
        &tessera_ingest::canonical::GenericIngest {
            name: tag,
            timestamp: TS,
            description: "generically-ingested table",
            source_format: "parquet",
            source_path: input,
            source_label: None,
            extra_sources: &[],
            decoder: decoder::Decoder::PARQUET,
            generation: None,
            column_meta: &column_meta,
        },
        &tessera_ingest::stream_table::StreamOpts {
            stage: &stage,
            out: &out,
            cfg: &cfg,
            batch_rows: 64 * 1024,
            // One block per row group, so 4x the rows is 4x the BLOCKS and not a bigger one. Must be a
            // positive multiple of ROWS_PER_GROUP, which the sink asserts.
            block_rows: tessera_io::table::ROWS_PER_GROUP as u64,
            tiers: Default::default(),
        },
    )
    .unwrap_or_else(|e| panic!("stream {tag}: {e}"))
}

#[test]
fn streaming_a_table_is_bounded_by_a_block_not_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let warm = dir.path().join("warm.parquet");
    let small = dir.path().join("small.parquet");
    let large = dir.path().join("large.parquet");
    write_fixture(&warm, SMALL_ROWS);
    write_fixture(&small, SMALL_ROWS);
    write_fixture(&large, LARGE_ROWS);

    // The comparison is only meaningful if the large file really is much larger.
    let (se, le) = (
        tessera_ingest::parquet_table::parquet_size_estimate(&small).unwrap(),
        tessera_ingest::parquet_table::parquet_size_estimate(&large).unwrap(),
    );
    assert!(
        le >= se * 3,
        "the large fixture must dwarf the small one for the bound to mean anything: {se} vs {le} bytes"
    );
    assert!(
        peak_rss_bytes() > 0,
        "/proc/self/status: VmHWM must be readable"
    );

    // Warm-up: pays the tokio runtime, the Vortex compressor's buffers, the encode pool and the
    // allocator's initial growth. Discarded — it is ~40 MiB and has nothing to do with the input size.
    let _ = stream(dir.path(), &warm, "warm");

    let a = stream(dir.path(), &small, "small");
    let after_small = peak_rss_bytes();

    // 4x the data through the same warmed machinery. Bounded memory means it fits under the mark already
    // set, because what it holds is a batch and a few blocks — not the file.
    let b = stream(dir.path(), &large, "large");
    let extra = peak_rss_bytes().saturating_sub(after_small);

    assert!(
        a.content_hash.is_some() && b.content_hash.is_some(),
        "both runs must have sealed a real product, not an empty one that trivially used no memory"
    );
    assert_ne!(
        a.content_hash, b.content_hash,
        "the two runs must have ingested different data, or the comparison is vacuous"
    );
    assert!(
        a.blocks.len() >= MIN_BLOCKS && b.blocks.len() > a.blocks.len(),
        "the large run must be MORE blocks, not one bigger block ({} then {}) — otherwise this measures \
         a single block growing and says nothing about bounded streaming",
        a.blocks.len(),
        b.blocks.len()
    );
    assert!(
        extra < EXTRA_BUDGET,
        "a 4x larger input raised peak RSS by a further {extra} bytes. Budget is {EXTRA_BUDGET} — a \
         quarter of the {EXTRA_DECODED} bytes the extra rows occupy decoded. Memory is tracking the \
         FILE, not the batch and the block, which is the bug #458 exists to fix."
    );
}
