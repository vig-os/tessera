//! `tessera bench write` — operator's "size your system" tool. Drives the **real** write engine
//! ([`tessera_io::TableStreamWriter`] for synthetic listmode; [`tessera_io::TableMultiBlockSink`]
//! over [`tessera_io::StreamWriter`] for real `.h5` listmode and for blocks) against MC-sampled
//! synthetic data (or a real `.h5`), reports wall seconds, throughput (events/s plus MB/s of raw
//! input), and peak RSS (`/proc/self/status: VmHWM`).
//!
//! - **Synthetic listmode is single-thread.** The `TableStreamWriter` path produces one canonical
//!   Vortex block over a sequential stream (ADR-0026 — single-block, in-RAM), so a synthetic
//!   listmode sweep is expected ~flat. We report it honestly: the sweep table makes that visible.
//! - **`--input` listmode parallelizes.** Post-#203 the multi-block ingest dispatches per-block
//!   encode across `workers` std threads (ADR-0034 — never tokio); the sweep scales until the
//!   read/encode knee.
//! - **Block encode parallelizes.** [`StreamWriter`] dispatches per-block encode across `workers`
//!   std threads, so the `blocks` sweep scales until the box saturates.
//! - **`--auto`** (the adaptive thread allocator): warmup-measure the producer's read+transpose
//!   rate AND one encode worker's per-core rate, ask [`tessera_io::WriteConfig::balanced`] for the
//!   worker knee, then run ONCE at that recommendation. Picks the read/encode knee, not max cores.
//!
//! SSoT: reuses the production encode path and the [`tessera_io::benches`]-style helpers (DRY).

/// Bench results go to stdout — they ARE this command's product, not debug logging. Funnel every
/// line through one annotated sink so the guardrails `no-debug-leftovers` gate is satisfied once,
/// rather than escaping each call site.
macro_rules! bench_out {
    ($($a:tt)*) => {{ println!($($a)*) }}; // guardrails-ok: bench command output, not a debug leftover
}

use std::path::PathBuf;
use std::time::Instant;

use tessera_core::block::array::ArraySpec;
use tessera_core::block::table::{Column, TableSpec};
use tessera_ingest::ge_hdf5;
use tessera_io::array::ArrayData;
use tessera_io::{
    array_job, parse_byte_size, table, ColumnData, StreamWriter, TableData, TableStreamWriter,
    WriteConfig, WriteSession, BLOCK_ROWS,
};

/// One MiB in bytes — output formatting constant.
const MIB_F: f64 = 1024.0 * 1024.0;

/// What schema to synthesize for the bench.
#[derive(Clone, Copy, Debug)]
pub enum BenchSchema {
    /// Listmode 6-column table (`ms·en0·en1·tx0·tx1·ax0`). Synthetic = single-thread encode via
    /// `TableStreamWriter`; `--input` = parallel multi-block encode via `TableMultiBlockSink`
    /// (#203). The bench's per-run note line picks the right description based on which path runs.
    Listmode,
    /// 64³ int16 array blocks — parallelizable encode.
    Blocks,
}

impl BenchSchema {
    fn parse(s: &str) -> tessera_core::Result<Self> {
        match s {
            "listmode" => Ok(BenchSchema::Listmode),
            "blocks" => Ok(BenchSchema::Blocks),
            other => Err(tessera_core::Error::Invalid(format!(
                "--schema '{other}' (expected listmode | blocks)"
            ))),
        }
    }

    /// Short label for headers. The "is this single-thread or parallel?" honesty note is
    /// **path-dependent** (synthetic listmode vs --input listmode behave differently after #203),
    /// so it lives on the per-run note line — not baked into the schema label.
    fn label(self) -> &'static str {
        match self {
            BenchSchema::Listmode => "listmode",
            BenchSchema::Blocks => "blocks",
        }
    }
}

/// Options as parsed from the CLI — kept as a struct so the bench function stays testable.
pub struct BenchOpts {
    pub schema: String,
    pub rows: usize,
    pub ram_budget: Option<String>,
    pub workers: Option<usize>,
    pub sweep: bool,
    pub auto: bool,
    pub input: Option<PathBuf>,
    pub dataset: String,
    pub seed: u64,
}

/// Read `VmHWM:` (resident-set high-water mark, KiB) from `/proc/self/status` and return bytes.
/// On non-Linux or any read error, returns 0 (the bench still reports throughput).
fn peak_rss_bytes() -> u64 {
    let status = match std::fs::read_to_string("/proc/self/status") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            // VmHWM:  123456 kB
            let kib: u64 = rest
                .split_whitespace()
                .next()
                .and_then(|t| t.parse().ok())
                .unwrap_or(0);
            return kib.saturating_mul(1024);
        }
    }
    0
}

/// Pretty-print a byte count as `123 B` / `1.5 KiB` / `42.0 MiB` / `1.2 GiB`.
fn fmt_bytes(b: u64) -> String {
    let f = b as f64;
    if f >= GIB_F {
        format!("{:.2} GiB", f / GIB_F)
    } else if f >= MIB_F {
        format!("{:.1} MiB", f / MIB_F)
    } else if f >= 1024.0 {
        format!("{:.1} KiB", f / 1024.0)
    } else {
        format!("{b} B")
    }
}

/// One GiB in bytes (float).
const GIB_F: f64 = 1024.0 * 1024.0 * 1024.0;

/// Deterministic xorshift64* — seeded MC sampler. Fast, no allocation, byte-stable across runs.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        // splitmix64 once so seed=0 doesn't degenerate the stream.
        let mut s = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        s = (s ^ (s >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        s = (s ^ (s >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Rng(s ^ (s >> 31))
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn u32_in(&mut self, hi_exclusive: u32) -> u32 {
        (self.next() as u32) % hi_exclusive.max(1)
    }
}

/// The listmode 6-column schema (matches `tessera-io/benches/throughput.rs`).
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

/// MC-sample a listmode batch of `n` events starting at row `start` — deterministic per (seed, start),
/// representative ranges (511 keV energy peak, monotonic `ms` timestamps, crystal/axial ids). Mirrors
/// the throughput bench's batch shape so the bench numbers are comparable across machines.
fn synthetic_listmode_batch(seed: u64, start: usize, n: usize) -> TableData {
    let mut rng = Rng::new(seed ^ (start as u64));
    let mut ms = Vec::with_capacity(n);
    let mut en0 = Vec::with_capacity(n);
    let mut en1 = Vec::with_capacity(n);
    let mut tx0 = Vec::with_capacity(n);
    let mut tx1 = Vec::with_capacity(n);
    let mut ax0 = Vec::with_capacity(n);
    for i in 0..n {
        // monotonic timestamp (rows are pushed in start..start+n order)
        ms.push((start + i) as u32);
        // energy peak at 511 keV with ~±10 keV jitter (representative for PET coincidence)
        en0.push(511.0 + (rng.u32_in(20) as f32) - 10.0);
        en1.push(511.0 + (rng.u32_in(20) as f32) - 10.0);
        // crystal id within a small ring
        tx0.push(rng.u32_in(4096) as u16);
        tx1.push(rng.u32_in(4096) as u16);
        // axial block id
        ax0.push(rng.u32_in(64) as u8);
    }
    vec![
        ("ms".into(), ColumnData::U32(ms)),
        ("en0".into(), ColumnData::F32(en0)),
        ("en1".into(), ColumnData::F32(en1)),
        ("tx0".into(), ColumnData::U16(tx0)),
        ("tx1".into(), ColumnData::U16(tx1)),
        ("ax0".into(), ColumnData::U8(ax0)),
    ]
}

/// Bytes-per-row for the synthetic listmode schema (4+4+4+2+2+1 = 17).
const LISTMODE_ROW_BYTES: usize = 4 + 4 + 4 + 2 + 2 + 1;
/// Edge length of a synthetic int16 block (64³ matches `examples/stream_write.rs`).
const BLOCK_EDGE: usize = 64;
/// Raw bytes per int16 64³ block.
const BLOCK_RAW_BYTES: u64 = (BLOCK_EDGE * BLOCK_EDGE * BLOCK_EDGE * 2) as u64;

/// One run's measured result.
#[derive(Clone, Copy, Debug)]
struct Measurement {
    workers: usize,
    wall_s: f64,
    raw_bytes: u64,
    units: u64, // events for listmode, blocks for blocks-mode
    peak_rss: u64,
}

impl Measurement {
    fn units_per_s(&self) -> f64 {
        self.units as f64 / self.wall_s.max(1e-9)
    }
    fn mb_per_s(&self) -> f64 {
        (self.raw_bytes as f64 / 1e6) / self.wall_s.max(1e-9)
    }
}

/// Run the listmode bench once with the given config — drives the **real**
/// [`TableStreamWriter`] (SSoT — same encoder as the production ingest path).
fn run_listmode_synthetic(
    rows: usize,
    cfg: &WriteConfig,
    seed: u64,
) -> tessera_core::Result<Measurement> {
    let dir = tempfile::tempdir().map_err(tessera_core::Error::from)?;
    let spec = TableSpec {
        columns: listmode_columns(),
        rows: rows as u64,
        row_index: Some("ms".into()),
    };
    // Bound the producer's in-memory batch slab to one ring-depth equivalent of rows. Conservative
    // (the row-group accumulator stages durable fragments anyway), but it keeps the producer side
    // honest to the configured RAM ceiling on huge runs.
    let target_batch =
        cfg.ring_depth(u64::try_from(LISTMODE_ROW_BYTES).unwrap_or(LISTMODE_ROW_BYTES as u64));
    let batch_rows = target_batch.clamp(9999, 1 << 17);
    let mut w = TableStreamWriter::new(spec, &dir.path().join("stage"))?;
    let t = Instant::now();
    let mut pushed = 0usize;
    while pushed < rows {
        let n = batch_rows.min(rows - pushed);
        w.push(synthetic_listmode_batch(seed, pushed, n))?;
        pushed += n;
    }
    // Encode + drop the bytes inside the timed window — VmHWM is the *high-water* mark, so it
    // captures the encode-time peak even after we drop the encoded payload.
    let _bytes = w.finish()?;
    let wall_s = t.elapsed().as_secs_f64();
    Ok(Measurement {
        workers: cfg.worker_count(),
        wall_s,
        raw_bytes: (rows as u64) * (LISTMODE_ROW_BYTES as u64),
        units: rows as u64,
        peak_rss: peak_rss_bytes(),
    })
}

/// Run the listmode bench once against a real `.h5` (the `--input` path) — drives the **real**
/// production multi-block streaming ingest (`ge_hdf5::stream_to_listmode_product_2p_to_file` →
/// `TableMultiBlockSink` + `StreamWriter`), so the bench measures the same encode path users hit.
fn run_listmode_real(
    input: &std::path::Path,
    dataset: &str,
    cfg: &WriteConfig,
) -> tessera_core::Result<Measurement> {
    let dir = tempfile::tempdir().map_err(tessera_core::Error::from)?;
    // The 2p stream path takes a `slab_rows` (the HDF5 hyperslab unit). Derive it from the RAM
    // budget: roughly one ring-depth-worth of rows at the schema's row width.
    let slab_rows = cfg
        .ring_depth(u64::try_from(LISTMODE_ROW_BYTES).unwrap_or(LISTMODE_ROW_BYTES as u64))
        .clamp(1 << 14, 1 << 18);
    let out = dir.path().join("bench.tsra");
    let t = Instant::now();
    // The STREAMING multi-block path: seals each block to disk as it's encoded (constant memory,
    // bounded by ram_budget) and parallel-encodes across `cfg.workers` — the path whose peak RAM the
    // bench is here to measure. (Not the old payload-returning `stream_to_listmode_product_2p`.)
    let m = ge_hdf5::stream_to_listmode_product_2p_to_file(
        input,
        dataset,
        "bench",
        "2024-01-01T00:00:00Z",
        slab_rows,
        &dir.path().join("stage"),
        &out,
        cfg,
    )?;
    let wall_s = t.elapsed().as_secs_f64();
    // Total rows = sum over all event blocks (multi-block products split at BLOCK_ROWS).
    let rows: u64 = m
        .blocks
        .iter()
        .filter_map(|b| b.spec.get("rows").and_then(|r| r.as_u64()))
        .sum();
    Ok(Measurement {
        workers: cfg.worker_count(),
        wall_s,
        raw_bytes: rows * (LISTMODE_ROW_BYTES as u64),
        units: rows,
        peak_rss: peak_rss_bytes(),
    })
}

/// MC-sample one 64³ int16 block deterministically (PET/CT-shaped gradient + per-block offset).
fn synthetic_block(seed: u64, i: usize) -> (ArraySpec, ArrayData) {
    let mut spec = ArraySpec::new(vec![BLOCK_EDGE as u64; 3], "int16");
    spec.codec = "pcodec".into();
    let mut rng = Rng::new(seed ^ (i as u64));
    let data = ArrayData::I16(
        (0..BLOCK_EDGE * BLOCK_EDGE * BLOCK_EDGE)
            .map(|k| {
                let z = (k / (BLOCK_EDGE * BLOCK_EDGE)) as i64;
                let y = ((k / BLOCK_EDGE) % BLOCK_EDGE) as i64;
                // structured gradient + small MC jitter so blocks aren't degenerate-compressible
                let jitter = (rng.u32_in(64) as i64) - 32;
                (z * 8 + y * 2 - 1024 + i as i64 + jitter) as i16
            })
            .collect(),
    );
    (spec, data)
}

/// Run the blocks bench once with the given config — drives the **real** [`StreamWriter`]
/// (parallel-encode across `workers`).
fn run_blocks_synthetic(
    rows: usize,
    cfg: &WriteConfig,
    seed: u64,
) -> tessera_core::Result<Measurement> {
    // `--rows` here counts blocks (each = 64³ int16 = 512 KiB raw).
    let n_blocks = rows.max(1);
    let dir = tempfile::tempdir().map_err(tessera_core::Error::from)?;
    let ws = WriteSession::create(
        &dir.path().join("stage"),
        "recon",
        "p",
        "d",
        "2024-01-01T00:00:00Z",
    )?;
    let mut sw = StreamWriter::with_config(ws, cfg, BLOCK_RAW_BYTES);
    let workers = sw.worker_count();
    let t = Instant::now();
    for i in 0..n_blocks {
        let (spec, data) = synthetic_block(seed, i);
        sw.push(array_job(format!("b{i:06}"), spec, data))?;
    }
    sw.finish(&dir.path().join("out.tsra"))?;
    let wall_s = t.elapsed().as_secs_f64();
    Ok(Measurement {
        workers,
        wall_s,
        raw_bytes: BLOCK_RAW_BYTES * (n_blocks as u64),
        units: n_blocks as u64,
        peak_rss: peak_rss_bytes(),
    })
}

/// Print one measurement as a single sweep-table row.
fn print_row(schema: BenchSchema, m: &Measurement) {
    let (unit_name, unit_per_s) = match schema {
        BenchSchema::Listmode => ("events/s", m.units_per_s()),
        BenchSchema::Blocks => ("blocks/s", m.units_per_s()),
    };
    bench_out!(
        "  {:>7} | {:>12.0} {:<9} | {:>9.1} MB/s | {:>10}", // guardrails-ok
        m.workers,
        unit_per_s,
        unit_name,
        m.mb_per_s(),
        fmt_bytes(m.peak_rss),
    );
}

/// The sweep-table header (printed once).
fn print_header() {
    bench_out!(
        "  {:>7} | {:>22} | {:>14} | {:>10}", // guardrails-ok
        "workers",
        "throughput",
        "raw MB/s",
        "peak RAM",
    );
    bench_out!("  {}", "-".repeat(64)); // guardrails-ok
}

/// Warmup sample size for the read+encode rate measurement (rows). One full canonical block is
/// ideal — it puts the encode through the same Vortex chunking pipeline a production run hits.
/// For synthetic, capped by `--rows` so a tiny bench (`--rows 200000`) still runs fast.
const WARMUP_BLOCK_ROWS: usize = BLOCK_ROWS;
/// Warmup floor — even on tiny `--rows`, measure at least one ROWS_PER_GROUP-worth so the encode
/// path exercises a real Vortex chunk (matches the production row-group grid).
const WARMUP_FLOOR_ROWS: usize = 1 << 16; // = ROWS_PER_GROUP

/// One warmup result — the per-stage rates the `balanced` heuristic consumes.
#[derive(Debug, Clone, Copy)]
struct Warmup {
    /// Raw input bytes per second of the **read+transpose** stage (the producer's bytes/s).
    read_bps: f64,
    /// Raw input bytes per second of one encode worker (single-thread). Multiplied by `workers`
    /// gives the encode pipeline's bytes/s — the right-hand side of the read-vs-encode min().
    per_core_encode_bps: f64,
    /// Rows sampled in this warmup — surfaced so the operator can sanity-check the measurement.
    sample_rows: usize,
}

/// Measure the **synthetic** producer's read+transpose rate (bytes/s of raw input). Generates a
/// `n_rows`-row batch through the same `synthetic_listmode_batch` the real bench uses — what we
/// time is the cost of producing the rows the encoder consumes, which is exactly the "read+
/// transpose rate" position in the pipeline.
fn warmup_read_synthetic(n_rows: usize, seed: u64) -> (TableData, f64) {
    let t = Instant::now();
    let data = synthetic_listmode_batch(seed, 0, n_rows);
    let elapsed = t.elapsed().as_secs_f64().max(1e-9);
    let raw = u64::try_from(n_rows)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::try_from(LISTMODE_ROW_BYTES).unwrap_or(LISTMODE_ROW_BYTES as u64));
    let bps = (raw as f64) / elapsed;
    (data, bps)
}

/// Measure the **real** read+transpose rate from a `.h5` (`--input` mode). Pulls one warmup slab
/// (up to `n_rows`) through the production [`ge_hdf5::stream_compound`] reader and times it. We
/// stop after one slab via a sentinel error from the sink — bubbling Err out of `stream_compound`
/// short-circuits the loop instead of reading the whole file, so the warmup cost is bounded.
fn warmup_read_real(
    input: &std::path::Path,
    dataset: &str,
    n_rows: usize,
    columns: &[Column],
) -> tessera_core::Result<(TableData, f64)> {
    // The sentinel string the warmup sink raises after capturing the first slab. We compare it
    // by value to distinguish "warmup done" from a real HDF5 error.
    const WARMUP_SENTINEL: &str = "__tessera_bench_warmup_one_slab_done__";
    let mut captured: Option<TableData> = None;
    let t = Instant::now();
    let result = ge_hdf5::stream_compound(input, dataset, n_rows.max(1), |slab| {
        captured = Some(slab);
        Err(tessera_core::Error::Invalid(WARMUP_SENTINEL.into()))
    });
    let elapsed = t.elapsed().as_secs_f64().max(1e-9);
    match result {
        Ok(()) => {} // dataset had ≤ 0 rows — fall through to the empty-data handler below
        Err(tessera_core::Error::Invalid(msg)) if msg == WARMUP_SENTINEL => {}
        Err(e) => return Err(e),
    }
    let data = match captured {
        Some(d) => d,
        None => {
            return Err(tessera_core::Error::Invalid(format!(
                "bench --auto: dataset '{dataset}' in '{}' is empty (no warmup sample)",
                input.display()
            )))
        }
    };
    let sample_rows = data.first().map(|(_, c)| c.len()).unwrap_or(0);
    let row_bytes: usize = columns
        .iter()
        .map(|c| ColumnData::dtype_size(&c.dtype).unwrap_or(0))
        .sum();
    let raw = u64::try_from(sample_rows)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::try_from(row_bytes).unwrap_or(row_bytes as u64));
    let bps = (raw as f64) / elapsed;
    Ok((data, bps))
}

/// Measure single-thread encode rate (bytes/s/core) by running the **real** production encoder
/// ([`tessera_io::table::encode`]) on the warmup sample. Same code path as the multi-block sink's
/// per-block encode jobs — the rate this returns is the per-core saturation the parallel pipeline
/// will see, so the heuristic feeds the encoder its own self-measured ceiling.
fn warmup_encode_per_core(columns: &[Column], data: &TableData) -> tessera_core::Result<f64> {
    let n_rows = data.first().map(|(_, c)| c.len()).unwrap_or(0);
    let spec = TableSpec {
        columns: columns.to_vec(),
        rows: u64::try_from(n_rows).unwrap_or(u64::MAX),
        row_index: None,
    };
    let row_bytes: usize = columns
        .iter()
        .map(|c| ColumnData::dtype_size(&c.dtype).unwrap_or(0))
        .sum();
    let raw = u64::try_from(n_rows)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::try_from(row_bytes).unwrap_or(row_bytes as u64));
    let t = Instant::now();
    let _bytes = table::encode(&spec, data)?;
    let elapsed = t.elapsed().as_secs_f64().max(1e-9);
    Ok((raw as f64) / elapsed)
}

/// Run the listmode warmup → (`Warmup`, columns), so the auto path can both apply the
/// recommendation and pretty-print the inputs. Routes between the synthetic and `--input` paths
/// without duplicating the encode-measure code.
fn run_warmup_listmode(opts: &BenchOpts) -> tessera_core::Result<(Warmup, Vec<Column>)> {
    match opts.input.as_deref() {
        Some(input) => {
            let columns = ge_hdf5::compound_columns(input, &opts.dataset)?;
            // Cap the warmup at one canonical block — large enough to hit a real Vortex chunking
            // workload, small enough to bound the warmup cost on a multi-TB acquisition.
            let target = WARMUP_BLOCK_ROWS;
            let (data, read_bps) = warmup_read_real(input, &opts.dataset, target, &columns)?;
            let per_core_encode_bps = warmup_encode_per_core(&columns, &data)?;
            let sample_rows = data.first().map(|(_, c)| c.len()).unwrap_or(0);
            Ok((
                Warmup {
                    read_bps,
                    per_core_encode_bps,
                    sample_rows,
                },
                columns,
            ))
        }
        None => {
            let columns = listmode_columns();
            // For synthetic we cap by --rows so `--rows 200000 --auto` runs in well under a second,
            // but keep ≥ one row-group so the encoder exercises a real Vortex chunk.
            let target = opts
                .rows
                .clamp(WARMUP_FLOOR_ROWS.min(opts.rows.max(1)), WARMUP_BLOCK_ROWS);
            let (data, read_bps) = warmup_read_synthetic(target, opts.seed);
            let per_core_encode_bps = warmup_encode_per_core(&columns, &data)?;
            Ok((
                Warmup {
                    read_bps,
                    per_core_encode_bps,
                    sample_rows: target,
                },
                columns,
            ))
        }
    }
}

/// Entry point invoked by `tessera bench write …`. Drives the real engine, reports honestly.
pub fn run(opts: BenchOpts) -> tessera_core::Result<()> {
    let schema = BenchSchema::parse(&opts.schema)?;
    let ram_budget = match opts.ram_budget.as_deref() {
        Some(s) => parse_byte_size(s)?,
        None => tessera_io::DEFAULT_RAM_BUDGET,
    };

    // --auto = warmup measure → balanced heuristic → single run at the recommended workers. Only
    // wired up for the listmode schema (the path users hit on a production ingest). For the blocks
    // schema we'd need a separate read/encode warmup; not blocking, but out of scope here.
    if opts.auto {
        if !matches!(schema, BenchSchema::Listmode) {
            return Err(tessera_core::Error::Invalid(
                "--auto currently only supports --schema listmode (blocks-schema heuristic TBD)"
                    .into(),
            ));
        }
        return run_auto(&opts, ram_budget);
    }

    // Pick the worker plan: explicit --workers, --sweep (1,2,4,… up to available_parallelism), or
    // single run at WriteConfig::for_system() defaults.
    let max_parallel = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let workers_plan: Vec<usize> = if opts.sweep {
        let mut v = Vec::new();
        let mut w = 1usize;
        while w <= max_parallel {
            v.push(w);
            w = w.saturating_mul(2);
        }
        // ensure the actual max is included (e.g. 12 on a 12-core box rather than just 1,2,4,8)
        if *v.last().unwrap_or(&1) < max_parallel {
            v.push(max_parallel);
        }
        v
    } else {
        vec![opts.workers.unwrap_or(max_parallel)]
    };

    // Header — the operator should read this top to bottom.
    bench_out!(
        // guardrails-ok
        "tessera bench write — schema={} · rows={} · ram_budget={} · seed={}",
        schema.label(),
        opts.rows,
        fmt_bytes(ram_budget),
        opts.seed,
    );
    // Honesty note — narrowly scoped. The listmode TableStreamWriter path (synthetic, no --input)
    // is single-thread encode (one canonical Vortex block, ADR-0026); the multi-block real-h5 path
    // (--input → TableMultiBlockSink + StreamWriter) parallelizes per-block encode, so the sweep
    // scales until the read/encode knee. The note picks one or the other based on what we're about
    // to run, so it stays true post-#203 (multi-block ingest).
    if matches!(schema, BenchSchema::Listmode) {
        if opts.input.is_some() {
            bench_out!(
                // guardrails-ok
                "  note: listmode --input parallelizes per-block encode (#203) — sweep scales to the read/encode knee."
            );
        } else {
            bench_out!(
                // guardrails-ok
                "  note: synthetic listmode is single-block (TableStreamWriter, ADR-0026) — sweep is expected ~flat."
            );
        }
    }
    bench_out!(); // guardrails-ok
    print_header();

    let mut measurements: Vec<Measurement> = Vec::with_capacity(workers_plan.len());
    for w in workers_plan {
        let cfg = WriteConfig::for_system().workers(w).ram_budget(ram_budget);
        let m = match (schema, opts.input.as_deref()) {
            (BenchSchema::Listmode, Some(input)) => run_listmode_real(input, &opts.dataset, &cfg)?,
            (BenchSchema::Listmode, None) => run_listmode_synthetic(opts.rows, &cfg, opts.seed)?,
            (BenchSchema::Blocks, _) => run_blocks_synthetic(opts.rows, &cfg, opts.seed)?,
        };
        print_row(schema, &m);
        measurements.push(m);
    }

    // Verdict — the operator's TL;DR line. partial_cmp is `None` only for NaN, which `units_per_s`
    // never produces (it divides by a non-zero clamp); fall back to `Equal` so the bench can't panic.
    if let Some(best) = measurements.iter().max_by(|a, b| {
        a.units_per_s()
            .partial_cmp(&b.units_per_s())
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        let label = match schema {
            BenchSchema::Listmode => "events/s",
            BenchSchema::Blocks => "blocks/s",
        };
        bench_out!(); // guardrails-ok
        bench_out!(
            // guardrails-ok
            "  saturation: ~{:.0} {} @ {} worker{} ({:.1} MB/s)",
            best.units_per_s(),
            label,
            best.workers,
            if best.workers == 1 { "" } else { "s" },
            best.mb_per_s(),
        );
    }
    Ok(())
}

/// `--auto`: warmup-measure the producer's read+transpose rate AND one encode worker's per-core
/// rate, ask [`WriteConfig::balanced`] for the worker knee, then run the full bench ONCE at that
/// worker count and report. SSoT — the run leg goes through the same `run_listmode_real` /
/// `run_listmode_synthetic` the manual path uses (no second encoder).
fn run_auto(opts: &BenchOpts, ram_budget: u64) -> tessera_core::Result<()> {
    let (warmup, _columns) = run_warmup_listmode(opts)?;
    let cfg =
        WriteConfig::balanced(warmup.read_bps, warmup.per_core_encode_bps).ram_budget(ram_budget);
    let workers = cfg.worker_count();
    // "read-bound" = the recommendation matched ceil(read/per_core), so adding workers won't help
    // (the encode pipeline already meets the read floor). "encode-bound" = the recommendation
    // clamped at available_parallelism — read could feed more, but we're out of cores.
    let max_parallel = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let storage_label = if workers >= max_parallel {
        "encode-bound"
    } else {
        "read-bound"
    };
    bench_out!(
        // guardrails-ok
        "tessera bench write --auto — schema={} · sample={} rows · ram_budget={} · seed={}",
        BenchSchema::Listmode.label(),
        warmup.sample_rows,
        fmt_bytes(ram_budget),
        opts.seed,
    );
    bench_out!(
        // guardrails-ok
        "  measured read ≈ {:.1} MB/s, encode ≈ {:.1} MB/s/core → recommend {} worker{} (storage is {})",
        warmup.read_bps / 1e6,
        warmup.per_core_encode_bps / 1e6,
        workers,
        if workers == 1 { "" } else { "s" },
        storage_label,
    );
    bench_out!(
        // guardrails-ok
        "  (a starting recommendation — assumes ~linear encode scaling; run --sweep to confirm the knee on your box)"
    );
    bench_out!(); // guardrails-ok
    print_header();
    let m = match opts.input.as_deref() {
        Some(input) => run_listmode_real(input, &opts.dataset, &cfg)?,
        None => run_listmode_synthetic(opts.rows, &cfg, opts.seed)?,
    };
    print_row(BenchSchema::Listmode, &m);
    bench_out!(); // guardrails-ok
    bench_out!(
        // guardrails-ok
        "  result: ~{:.0} events/s @ {} worker{} ({:.1} MB/s, peak {})",
        m.units_per_s(),
        m.workers,
        if m.workers == 1 { "" } else { "s" },
        m.mb_per_s(),
        fmt_bytes(m.peak_rss),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_listmode_batch_is_deterministic() {
        // same (seed, start, n) → byte-identical columns (the bench's MC sampler must be reproducible).
        let a = synthetic_listmode_batch(42, 0, 1000);
        let b = synthetic_listmode_batch(42, 0, 1000);
        assert_eq!(a, b);
        let c = synthetic_listmode_batch(7, 0, 1000);
        assert_ne!(a, c, "different seed must produce different stream");
    }

    #[test]
    fn run_listmode_synthetic_completes_and_reports_throughput() {
        // Smoke: the real engine runs through the synthetic data + finish() under a single worker.
        let cfg = WriteConfig::for_system()
            .workers(1)
            .ram_budget(64 * 1024 * 1024);
        let m = run_listmode_synthetic(2000, &cfg, 1).unwrap();
        assert_eq!(m.units, 2000);
        assert!(m.units_per_s() > 0.0);
    }

    #[test]
    fn warmup_synthetic_measures_positive_rates() {
        // The warmup must return finite, strictly positive rates for the heuristic to consume —
        // anything else collapses to the `balanced` degenerate-input fallback, which would mask
        // bugs in the measurement pipeline rather than expose them.
        let opts = BenchOpts {
            schema: "listmode".into(),
            rows: 200_000,
            ram_budget: None,
            workers: None,
            sweep: false,
            auto: true,
            input: None,
            dataset: "events_2p".into(),
            seed: 1,
        };
        let (w, cols) = run_warmup_listmode(&opts).unwrap();
        assert!(w.read_bps.is_finite() && w.read_bps > 0.0);
        assert!(w.per_core_encode_bps.is_finite() && w.per_core_encode_bps > 0.0);
        assert_eq!(cols.len(), 6, "synthetic listmode has 6 columns");
        assert!(w.sample_rows >= 1);
    }

    #[test]
    fn auto_synthetic_recommends_and_runs_end_to_end() {
        // The `--auto` driver, end-to-end: warmup → balanced → run. Uses a small synthetic
        // workload so the test stays fast + hermetic (no real .h5 needed). Verifies the path
        // returns Ok and the recommendation lands inside [1, available_parallelism].
        let opts = BenchOpts {
            schema: "listmode".into(),
            rows: 200_000,
            ram_budget: Some("64M".into()),
            workers: None,
            sweep: false,
            auto: true,
            input: None,
            dataset: "events_2p".into(),
            seed: 1,
        };
        run(opts).expect("auto-mode bench must complete on a tiny synthetic workload");
    }
}
