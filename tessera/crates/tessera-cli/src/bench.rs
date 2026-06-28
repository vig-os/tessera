//! `tessera bench write` — operator's "size your system" tool. Drives the **real** write engine
//! ([`tessera_io::TableStreamWriter`] for listmode, [`tessera_io::StreamWriter`] for blocks)
//! against MC-sampled synthetic data (or a real `.h5`), reports wall seconds, throughput
//! (events/s + MB/s of raw input), and peak RSS (`/proc/self/status: VmHWM`).
//!
//! - **Listmode encode is single-thread.** The streaming table path produces one canonical Vortex
//!   block over a sequential stream (ADR-0026 — multi-block table core-scaling deferred), so a
//!   listmode sweep is expected ~flat. We report it honestly: the sweep table makes that visible.
//! - **Block encode parallelizes.** [`StreamWriter`] dispatches per-block encode across `workers`
//!   std threads (ADR-0034 — never tokio), so the `blocks` sweep scales until the box saturates.
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
    array_job, parse_byte_size, ColumnData, StreamWriter, TableData, TableStreamWriter,
    WriteConfig, WriteSession,
};

/// One MiB in bytes — output formatting constant.
const MIB_F: f64 = 1024.0 * 1024.0;

/// What schema to synthesize for the bench.
#[derive(Clone, Copy, Debug)]
pub enum BenchSchema {
    /// Listmode 6-column table (`ms·en0·en1·tx0·tx1·ax0`) — single-thread encode.
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

    fn label(self) -> &'static str {
        match self {
            BenchSchema::Listmode => "listmode (single-thread encode)",
            BenchSchema::Blocks => "blocks (parallel encode)",
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
/// [`ge_hdf5::stream_to_listmode_product_2p`] (which uses the production [`TableStreamWriter`]).
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
    let t = Instant::now();
    let (m, _payloads) = ge_hdf5::stream_to_listmode_product_2p(
        input,
        dataset,
        "bench",
        "2024-01-01T00:00:00Z",
        slab_rows,
        &dir.path().join("stage"),
    )?;
    let wall_s = t.elapsed().as_secs_f64();
    // Approximate raw bytes: row count × LISTMODE_ROW_BYTES (the 2p schema is similar in width).
    let rows = m
        .blocks
        .iter()
        .find_map(|b| b.spec.get("rows").and_then(|r| r.as_u64()))
        .unwrap_or(0);
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

/// Entry point invoked by `tessera bench write …`. Drives the real engine, reports honestly.
pub fn run(opts: BenchOpts) -> tessera_core::Result<()> {
    let schema = BenchSchema::parse(&opts.schema)?;
    let ram_budget = match opts.ram_budget.as_deref() {
        Some(s) => parse_byte_size(s)?,
        None => tessera_io::DEFAULT_RAM_BUDGET,
    };

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
    if matches!(schema, BenchSchema::Listmode) {
        bench_out!(
            // guardrails-ok
            "  note: listmode encode is single-thread (ADR-0026) — sweep is expected ~flat for listmode."
        );
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
}
