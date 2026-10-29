//! #677 test (a), real memory: a staged table job holds nothing on top of the encoder's own working
//! set — in particular not the encoded block.
//!
//! `stream::tests::single_table_block_many_times_the_budget_stays_within_the_window` checks the
//! window's accounting (`StreamStats`), which counts what a job HANDS the window. A job that buffered
//! its whole encode in RAM and wrote it to the staged file at the end would pass every one of those
//! checks. This test counts the heap instead, with a counting global allocator.
//!
//! **Why a reference run, not an absolute bound.** The pinned Vortex writer holds encoded segments in
//! flight that grow with the table (#681), so no size-independent bound holds today. Instead each job
//! is measured against the same encode into [`std::io::sink`] — the encoder's own working set, #681
//! included — and may exceed it only by [`ALLOWANCE`] (its staged file's `BufWriter`, the session and
//! the pipeline's bookkeeping; measured within ±0.1 MB of the reference). A job that buffers the block
//! grows a buffer the size of the block while the encoder's segments drain into it, so it exceeds the
//! reference by most of the block, which the test makes ≥ 8 × [`ALLOWANCE`]; and the reference must
//! itself be a real encode, so the check can't pass vacuously. Measured with `table_job_streaming`
//! buffering into a `Vec` (the regression this guards): job peak +6.0 MB over the reference for this
//! 9.6 MB block, failing the check.
//!
//! **This lives in its own test binary on purpose**: the allocator counts every thread of the
//! process, and the lib's unit tests run in parallel, so they'd count each other's heap. Here one
//! `#[test]` runs alone.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use tessera_core::block::table::{Column, TableSpec};
use tessera_io::stream::{table_job_streaming, StagedEncodeJob, StreamWindow, StreamWriter};
use tessera_io::table::ROWS_PER_GROUP;
use tessera_io::{ColumnData, TableData, TableStreamWriter, WriteSession};

/// What a staged job may hold on top of the encoder's working set.
const ALLOWANCE: usize = 1 << 20;

/// Counts the live heap and its high-water mark.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to `System` unchanged; only counts sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc`'s contract, which is `System.alloc`'s.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // SAFETY: `p` came from `alloc` above, i.e. from `System`, with this layout.
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// The heap `f` holds at its peak, above what was live when it started.
fn peak_of(f: impl FnOnce()) -> usize {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    f();
    PEAK.load(Ordering::Relaxed) - base
}

fn col(name: &str, dtype: &str) -> Column {
    Column {
        name: name.into(),
        dtype: dtype.into(),
        ..Default::default()
    }
}

/// The listmode shape of test (a): a row index, a nullable timestamp whose first 1024 rows are
/// NULL, an incompressible 64-bit column and a small one.
fn spec(rows: u64) -> TableSpec {
    TableSpec {
        columns: vec![
            col("t", "u8"),
            col("ms", "u4").nullable(),
            col("r", "u8"),
            col("e", "u2"),
        ],
        rows,
        row_index: Some("t".into()),
    }
}

/// SplitMix64 of the absolute row.
fn mix(row: u64) -> u64 {
    let mut x = row.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Rows `start..start + n`.
fn rows(start: u64, n: usize) -> TableData {
    let rows = start..start + n as u64;
    vec![
        ("t".into(), ColumnData::U64(rows.clone().collect())),
        (
            "ms".into(),
            ColumnData::Nullable {
                values: Box::new(ColumnData::U32(
                    rows.clone().map(|i| (i / 1024) as u32).collect(),
                )),
                validity: rows.clone().map(|i| i >= 1024).collect(),
            },
        ),
        ("r".into(), ColumnData::U64(rows.clone().map(mix).collect())),
        (
            "e".into(),
            ColumnData::U16(rows.map(|i| (mix(i ^ 0x55) & 0x1ff) as u16).collect()),
        ),
    ]
}

/// The table as lazy row-groups.
fn groups(total: u64) -> impl Iterator<Item = TableData> + Send + 'static {
    (0..total)
        .step_by(ROWS_PER_GROUP)
        .map(move |start| rows(start, (total - start).min(ROWS_PER_GROUP as u64) as usize))
}

/// The table staged as row-group fragments, ready to compact.
fn fragments(stage: &std::path::Path, total: u64) -> TableStreamWriter {
    let mut w = TableStreamWriter::new(spec(total), stage).unwrap();
    for group in groups(total) {
        w.push(group).unwrap();
    }
    w
}

/// Run `job` through a one-worker `StreamWriter` and seal it into `dir/p.tsra`.
fn run(dir: &std::path::Path, job: StagedEncodeJob) {
    let ws = WriteSession::create(
        &dir.join("session"),
        "recon",
        "p",
        "d",
        "2024-01-01T00:00:00Z",
    )
    .unwrap();
    let mut sw = StreamWriter::with_window(ws, 1, StreamWindow::new(4, 64 * 1024));
    sw.push_staged(job).unwrap();
    let manifest = sw.finish(&dir.join("p.tsra")).unwrap();
    assert_eq!(manifest.blocks.len(), 1);
}

/// The size of the block `run` sealed into `dir` (read back outside any measurement).
fn sealed_block(dir: &std::path::Path) -> u64 {
    let mut rdr = tessera_io::Reader::open(&dir.join("p.tsra")).unwrap();
    rdr.read_block("events").unwrap().len() as u64
}

#[test]
fn staged_table_jobs_hold_no_more_than_the_encoder_itself() {
    let total = 16 * ROWS_PER_GROUP as u64 + 4321; // 17 row-groups, as test (a)
    let dir = tempfile::tempdir().unwrap();
    // A throwaway encode first, so neither measurement pays the one-time setup (lazy statics, the
    // Vortex registries) and the reference isn't inflated by it.
    tessera_io::table::encode_streaming_to(&spec(total), groups(total), std::io::sink()).unwrap();

    // table_job_streaming, against the same encode into a sink that keeps nothing.
    let reference = peak_of(|| {
        tessera_io::table::encode_streaming_to(&spec(total), groups(total), std::io::sink())
            .unwrap();
    });
    let job = peak_of(|| {
        run(
            &dir.path().join("streaming"),
            table_job_streaming("events", spec(total), groups(total)),
        );
    });
    check(
        "table_job_streaming",
        reference,
        job,
        sealed_block(&dir.path().join("streaming")),
    );

    // TableStreamWriter::into_job (table_job_from_fragments), against finish_to into a sink. The
    // fragments are staged before either measurement starts.
    let reference = {
        let w = fragments(&dir.path().join("frags-ref"), total);
        peak_of(|| {
            w.finish_to(std::io::sink()).unwrap();
        })
    };
    let w = fragments(&dir.path().join("frags-job"), total);
    let job = peak_of(|| {
        run(&dir.path().join("fragments"), w.into_job("events").unwrap());
    });
    check(
        "into_job",
        reference,
        job,
        sealed_block(&dir.path().join("fragments")),
    );
}

/// The job's peak heap is the encoder's, give or take [`ALLOWANCE`], for a block that a buffering
/// job would push far past it.
fn check(what: &str, reference: usize, job: usize, block: u64) {
    eprintln!("{what}: encoder peak {reference} B, job peak {job} B, block {block} B");
    assert!(
        block >= 8 * ALLOWANCE as u64,
        "{what}: the block ({block} B) must dwarf the allowance"
    );
    assert!(
        reference >= ALLOWANCE,
        "{what}: the reference measured no encode ({reference} B)"
    );
    assert!(
        job <= reference + ALLOWANCE,
        "{what}: the job's peak heap is {job} B, {} B above the encoder's own {reference} B — \
         a staged job must not hold its encoded block ({block} B) in RAM",
        job - reference
    );
}
