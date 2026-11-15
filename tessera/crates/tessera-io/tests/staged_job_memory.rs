//! Real-memory tests of the streaming writer, with a counting global allocator: what the heap
//! holds, not what the window's accounting ([`StreamStats`]) counts.
//!
//! - **#677 test (a):** a staged table job holds nothing on top of the encoder's own working set,
//!   in particular not the encoded block ([`encoder_reference`]).
//! - **ADR-0064 §8, the per-block heap model of `push_table` (#714):** the input it stages on disk
//!   is not also held in RAM ([`held_heap`]), and its peak heap stays under
//!   `min(W, n) · p_block + s` ([`ceiling`]).
//!
//! **One `#[test]`, on purpose.** The allocator counts every thread of the process, so a
//! measurement is only meaningful while nothing else in the process allocates. The lib's unit tests
//! run in parallel, which is why this is its own test binary; and the measurements share its one
//! `#[test]` and run in sequence, rather than one `#[test]` each, because `cargo test` runs a
//! binary's tests on parallel threads of one process by default, and this binary can't make its
//! callers pass `--test-threads=1`. nextest runs each `#[test]` in a process of its own, so with
//! one `#[test]` either runner has exactly one test in the process. Add a measurement to
//! [`staged_job_memory`], never as a second `#[test]`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use tessera_core::block::table::{Column, TableSpec};
// The deprecated single-block job keeps its heap contract at ≤ BLOCK_ROWS rows, measured here until
// it is removed; `push_table`'s heap model (#714) is measured below.
#[allow(deprecated)]
use tessera_io::stream::table_job_streaming;
use tessera_io::stream::{StagedEncodeJob, StreamWindow, StreamWriter};
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

#[test]
fn staged_job_memory() {
    // A throwaway encode first, so no measurement pays the one-time setup (lazy statics, the Vortex
    // registries) and no reference is inflated by it.
    let total = 16 * ROWS_PER_GROUP as u64;
    tessera_io::table::encode_streaming_to(&spec(total), groups(total), std::io::sink()).unwrap();

    encoder_reference();
    let failed: Vec<String> = [held_heap(), ceiling()]
        .into_iter()
        .filter_map(Result::err)
        .collect();
    assert!(failed.is_empty(), "\n{}", failed.join("\n"));
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
        partition: None,
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

/// #677 test (a). `stream::tests::single_table_block_many_times_the_budget_stays_within_the_window`
/// checks the window's accounting, which counts what a job HANDS the window. A job that buffered
/// its whole encode in RAM and wrote it to the staged file at the end would pass every one of those
/// checks; this counts the heap instead.
///
/// **Why a reference run, not an absolute bound.** The pinned Vortex writer holds encoded segments
/// in flight that grow with the block (#681), so no size-independent bound holds for one block.
/// Instead each job is measured against the same encode into [`std::io::sink`] — the encoder's own
/// working set, #681 included — and may exceed it only by [`ALLOWANCE`] (its staged file's
/// `BufWriter`, the session and the pipeline's bookkeeping; measured within ±0.1 MB of the
/// reference). A job that buffers the block grows a buffer the size of the block while the
/// encoder's segments drain into it, so it exceeds the reference by most of the block, which the
/// test makes ≥ 8 × [`ALLOWANCE`]; and the reference must itself be a real encode, so the check
/// can't pass vacuously. Measured with `table_job_streaming` buffering into a `Vec` (the
/// regression this guards): job peak +6.0 MB over the reference for this 9.6 MB block, failing
/// the check.
#[allow(deprecated)] // measures the deprecated job at ≤ BLOCK_ROWS rows, see its import
fn encoder_reference() {
    let total = 16 * ROWS_PER_GROUP as u64 + 4321; // 17 row-groups, as test (a)
    let dir = tempfile::tempdir().unwrap();

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

// ---- ADR-0064 §8: the per-block heap model of `push_table` (#714) ----

/// `s_fixed` of ADR-0064 §8: the 8 KiB `BufWriter`s, the commit's 64 KiB verify buffer, the job and
/// window bookkeeping.
const S_FIXED: usize = 4 << 20;

/// The consumer's decoder schema (the #681 note's `pairs` shape, `ms` nullable): a sequential
/// `u64`, a nullable `u32`, two `bool`s, seven masked `u8`s and an `i16`, 24 B per row in memory.
fn consumer_spec(rows: u64) -> TableSpec {
    let mut columns = vec![
        col("pkt", "u8"),
        col("ms", "u4").nullable(),
        col("flag0", "b1"),
        col("flag1", "b1"),
    ];
    columns.extend((0..7).map(|k| col(&format!("m{k}"), "u1")));
    columns.push(col("tof", "i2"));
    TableSpec {
        columns,
        rows,
        row_index: Some("pkt".into()),
        partition: None,
    }
}

/// Rows `start..start + n` of [`consumer_spec`], every column freshly allocated.
fn consumer_rows(start: u64, n: usize) -> TableData {
    let rows = start..start + n as u64;
    let mut group: TableData = vec![
        ("pkt".into(), ColumnData::U64(rows.clone().collect())),
        (
            "ms".into(),
            ColumnData::Nullable {
                values: Box::new(ColumnData::U32(
                    rows.clone().map(|i| (i / 1024) as u32).collect(),
                )),
                validity: rows.clone().map(|i| i % 61 != 0).collect(),
            },
        ),
        (
            "flag0".into(),
            ColumnData::Bool(rows.clone().map(|i| mix(i) & 1 == 1).collect()),
        ),
        (
            "flag1".into(),
            ColumnData::Bool(rows.clone().map(|i| mix(i) & 2 == 2).collect()),
        ),
    ];
    group.extend((0..7u32).map(|k| {
        let masked = rows
            .clone()
            .map(|i| (mix(i) >> (8 * k)) as u8 & 0x3f)
            .collect();
        (format!("m{k}"), ColumnData::U8(masked))
    }));
    group.push((
        "tof".into(),
        ColumnData::I16(rows.map(|i| ((mix(i) >> 56) as i16) - 128).collect()),
    ));
    group
}

/// The table as lazy row-groups, one freshly allocated group per pull, so the generator holds no
/// more than the group the reader is serializing.
fn consumer_groups(total: u64) -> impl Iterator<Item = TableData> + Send + 'static {
    (0..total)
        .step_by(ROWS_PER_GROUP)
        .map(move |start| consumer_rows(start, (total - start).min(ROWS_PER_GROUP as u64) as usize))
}

/// `r_mem` of ADR-0064 §8: the sum of the column widths, plus one validity bit per nullable column
/// rounded up to a whole byte.
fn r_mem(spec: &TableSpec) -> usize {
    spec.columns
        .iter()
        .map(|c| ColumnData::dtype_size(&c.dtype).unwrap() + usize::from(c.nullable))
        .sum()
}

/// `s = s_stage + s_fixed`, `s_stage = 2 · ROWS_PER_GROUP · r_mem` (ADR-0064 §8).
fn slack(spec: &TableSpec) -> usize {
    2 * ROWS_PER_GROUP * r_mem(spec) + S_FIXED
}

/// A `W`-worker [`StreamWriter`] on a fresh session in `dir`.
fn writer(dir: &std::path::Path, workers: usize) -> StreamWriter {
    let ws = WriteSession::create(
        &dir.join("session"),
        "recon",
        "p",
        "d",
        "2024-01-01T00:00:00Z",
    )
    .unwrap();
    StreamWriter::new(ws, workers, 4)
}

const MIB: f64 = (1 << 20) as f64;

/// ADR-0064 §8 part 1, the held heap. All `W = 2` workers are held before they read their input and
/// rows are offered until the reader stalls at the hand-over of block `W`. No encode has run, so
/// the heap is the input the writer keeps in RAM: at most the row-group it is serializing, plus
/// bookkeeping, `≤ s`. A writer that filled blocks in memory would hold the `W` dispatched blocks
/// and the full one waiting, `(W + 1) · A` (18 MiB against `s` = 7 MiB at `ROWS_PER_GROUP` =
/// 65 536).
fn held_heap() -> Result<(), String> {
    const W: usize = 2;
    let block_rows = 4 * ROWS_PER_GROUP as u64;
    let blocks = 2 * W as u64 + 1;
    let total = blocks * block_rows;
    let spec = consumer_spec(total);
    assert_eq!(r_mem(&spec), 24, "the consumer schema is 24 B per row");
    let s = slack(&spec);
    let a = block_rows as usize * r_mem(&spec);
    assert!(
        (W + 1) * a > s,
        "an in-memory fill ({W} + 1) × {a} B must exceed s = {s} B"
    );

    let dir = tempfile::tempdir().unwrap();
    let mut sw = writer(dir.path(), W);
    let hold = sw.hold_table_reads();
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    // `push_table` runs off this thread: it blocks at the hand-over while the hold is ours.
    let reader = std::thread::spawn(move || {
        let leaves =
            sw.push_table_with_block_rows("pairs", spec, consumer_groups(total), block_rows);
        (sw, leaves)
    });
    while hold.stats().producer_stalls == 0 && !reader.is_finished() {
        std::thread::yield_now();
    }
    let held = PEAK.load(Ordering::Relaxed) - base;
    let stats = hold.stats();
    drop(hold);
    let (sw, leaves) = reader.join().unwrap();
    assert_eq!(leaves.unwrap(), blocks);
    // Not vacuous: the reader handed a block to each of the `W` workers and stalled on the next.
    assert_eq!(stats.producer_stalls, 1, "the reader never stalled");
    assert_eq!(
        stats.jobs_pushed, W,
        "{} blocks were handed over",
        stats.jobs_pushed
    );
    let sealed = sw.finish(&dir.path().join("p.tsra")).unwrap();
    assert_eq!(sealed.blocks.len() as u64, blocks);

    eprintln!(
        "held heap (W = {W}, A = {:.1} MiB): {:.2} MiB against s = {:.2} MiB",
        a as f64 / MIB,
        held as f64 / MIB,
        s as f64 / MIB
    );
    if held > s {
        return Err(format!(
            "held heap: {held} B ({:.2} MiB) with all {W} workers held, over s = {s} B ({:.2} MiB) \
             — push_table holds staged input in RAM ({W} + 1 blocks of {a} B would be {} B)",
            held as f64 / MIB,
            s as f64 / MIB,
            (W + 1) * a
        ));
    }
    Ok(())
}

/// ADR-0064 §8 part 2, the ceiling: `peak(n, W) ≤ min(W, n) · p_block + s` at `W = 2`,
/// `n ∈ {4, 8}` and `W = 1`, `n = 4`, with `p_block` the peak of the `W = 1`, `n = 1` write in this
/// binary (it depends on `P`, Vortex's queue depths). That run is the calibration, not a pass/fail
/// point. At `block_rows = 16 · ROWS_PER_GROUP` a block of input is `A` = 24 MiB, so a writer that
/// fills blocks in memory, at `min(W, n) · (p_enc + A) + A`, exceeds the ceiling by `A − s` = 17 MiB.
///
/// The calibration is itself checked from the read-back side: `p_block ≤ p_enc + s`, with `p_enc`
/// the same block encoded from the same lazy row-groups into [`std::io::sink`]. A worker that read
/// its staged block back whole, rather than a row-group at a time, would raise `p_block` by `A`
/// and pass the ceiling with it; it fails here.
fn ceiling() -> Result<(), String> {
    let block_rows = 16 * ROWS_PER_GROUP as u64;
    let s = slack(&consumer_spec(0));
    let a = block_rows as usize * r_mem(&consumer_spec(0));
    assert!(
        a > s,
        "an in-memory fill's excess A − s ({a} − {s} B) must be positive"
    );
    let peak = |workers: usize, n: u64| {
        let total = n * block_rows;
        let dir = tempfile::tempdir().unwrap();
        peak_of(|| {
            let mut sw = writer(dir.path(), workers);
            let leaves = sw
                .push_table_with_block_rows(
                    "pairs",
                    consumer_spec(total),
                    consumer_groups(total),
                    block_rows,
                )
                .unwrap();
            assert_eq!(leaves, n);
            let sealed = sw.finish(&dir.path().join("p.tsra")).unwrap();
            assert_eq!(sealed.blocks.len() as u64, n);
        })
    };
    // A first concurrent write, so no measurement pays the encoder's one-time setup at W = 2.
    peak(2, 2);
    let p_block = peak(1, 1);
    assert!(
        p_block >= a / 4,
        "the calibration measured no encode (p_block = {p_block} B)"
    );
    eprintln!(
        "ceiling (A = {:.1} MiB): p_block = {:.2} MiB, s = {:.2} MiB",
        a as f64 / MIB,
        p_block as f64 / MIB,
        s as f64 / MIB
    );
    let mut over = Vec::new();
    let p_enc = peak_of(|| {
        tessera_io::table::encode_streaming_to(
            &consumer_spec(block_rows),
            consumer_groups(block_rows),
            std::io::sink(),
        )
        .unwrap();
    });
    eprintln!(
        "  read-back: p_block {:.2} MiB, p_enc + s {:.2} MiB",
        p_block as f64 / MIB,
        (p_enc + s) as f64 / MIB
    );
    if p_block > p_enc + s {
        over.push(format!(
            "read-back: p_block = {p_block} B ({:.2} MiB) over p_enc + s = {} B ({:.2} MiB; the \
             encoder alone peaks at {p_enc} B) — a worker holds its staged input in RAM",
            p_block as f64 / MIB,
            p_enc + s,
            (p_enc + s) as f64 / MIB
        ));
    }
    for (workers, n) in [(2usize, 4u64), (2, 8), (1, 4)] {
        let got = peak(workers, n);
        let bound = workers.min(n as usize) * p_block + s;
        eprintln!(
            "  W = {workers}, n = {n}: peak {:.2} MiB, bound {:.2} MiB",
            got as f64 / MIB,
            bound as f64 / MIB
        );
        if got > bound {
            over.push(format!(
                "ceiling: W = {workers}, n = {n}: peak {got} B ({:.2} MiB) over \
                 min(W, n) · p_block + s = {bound} B ({:.2} MiB; p_block = {p_block} B, s = {s} B)",
                got as f64 / MIB,
                bound as f64 / MIB
            ));
        }
    }
    if over.is_empty() {
        Ok(())
    } else {
        Err(over.join("\n"))
    }
}
