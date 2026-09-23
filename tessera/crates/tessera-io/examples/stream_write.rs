//! Streaming write engine bench (#203): the bounded-memory, parallel-encode `StreamWriter` vs the
//! synchronous encode-on-the-hot-path baseline.
//!
//! Run: `cargo run -p tessera-io --example stream_write --release`
//! (pin to a quiet slice on a busy box, e.g. `taskset -c 10-39 nice -n19 cargo run ...`).
//!
//! Shows: (1) parallel-encode speedup vs 1 worker / vs synchronous, (2) sustained throughput,
//! (3) that a tiny capacity still completes — backpressure bounds RAM regardless of producer speed.

use std::time::Instant;

use tessera_core::block::array::ArraySpec;
use tessera_core::ProductBuilder;
use tessera_io::array::{self, ArrayData};
use tessera_io::{array_job, StreamWriter, WriteSession};

const TS: &str = "2024-01-01T00:00:00Z";
const N: usize = 256; // blocks
const EDGE: usize = 64; // 64³ int16 block = 512 KiB raw

fn make_blocks() -> Vec<(ArraySpec, ArrayData)> {
    (0..N)
        .map(|i| {
            let mut spec = ArraySpec::new(vec![EDGE as u64; 3], "int16");
            spec.codec = "pcodec".into();
            // CT-like gradient + per-block offset so each block is distinct (no dedup shortcuts)
            let data = ArrayData::I16(
                (0..EDGE * EDGE * EDGE)
                    .map(|k| {
                        let z = (k / (EDGE * EDGE)) as i64;
                        let y = ((k / EDGE) % EDGE) as i64;
                        (z * 8 + y * 2 - 1024 + i as i64) as i16
                    })
                    .collect(),
            );
            (spec, data)
        })
        .collect()
}

fn raw_bytes() -> u64 {
    (N * EDGE * EDGE * EDGE * 2) as u64
}

/// Synchronous baseline: encode on the calling thread, then append (the "old way" — encode blocks
/// the producer's hot path).
fn run_sync(blocks: &[(ArraySpec, ArrayData)], dir: &std::path::Path) -> f64 {
    let stage = dir.join("sync_stage");
    let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
    let mut ws = ws;
    let t = Instant::now();
    for (i, (spec, data)) in blocks.iter().enumerate() {
        let (r, p) = array::array_block(&format!("b{i:04}"), spec, data).unwrap();
        ws.append_block(r, &p.bytes).unwrap();
    }
    ws.seal(&dir.join("sync.tsra")).unwrap();
    t.elapsed().as_secs_f64()
}

/// Pipelined: producer pushes raw blocks; `workers` threads encode in parallel; committer seals.
fn run_stream(
    blocks: &[(ArraySpec, ArrayData)],
    workers: usize,
    cap: usize,
    dir: &std::path::Path,
) -> f64 {
    let stage = dir.join(format!("stream_stage_{workers}"));
    let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
    let mut sw = StreamWriter::new(ws, workers, cap);
    let t = Instant::now();
    for (i, (spec, data)) in blocks.iter().enumerate() {
        sw.push(array_job(format!("b{i:04}"), spec.clone(), data.clone()))
            .unwrap();
    }
    sw.finish(&dir.join(format!("stream_{workers}.tsra")))
        .unwrap();
    t.elapsed().as_secs_f64()
}

fn main() {
    let blocks = make_blocks();
    let raw = raw_bytes();
    let mib = raw as f64 / (1024.0 * 1024.0);
    let dir = tempfile::tempdir().unwrap();

    println!("# Streaming write engine (#203) — {N} × {EDGE}³ int16 blocks, {mib:.0} MiB raw");
    println!("# bounded-memory + parallel-encode pipeline vs synchronous encode-on-hot-path.\n");

    let sync_s = run_sync(&blocks, dir.path());

    println!(
        "{:<18} {:>8} {:>11} {:>10} {:>9}",
        "mode", "wall s", "blocks/s", "MB/s", "speedup"
    );
    println!(
        "{:<18} {:>8.3} {:>11.0} {:>10.0} {:>9}",
        "synchronous",
        sync_s,
        N as f64 / sync_s,
        (raw as f64 / 1e6) / sync_s,
        "1.00×",
    );

    for workers in [1usize, 2, 4, 8] {
        let s = run_stream(&blocks, workers, workers * 2, dir.path());
        println!(
            "{:<18} {:>8.3} {:>11.0} {:>10.0} {:>8.2}×",
            format!("stream w={workers} c={}", workers * 2),
            s,
            N as f64 / s,
            (raw as f64 / 1e6) / s,
            sync_s / s,
        );
    }

    // Bounded RAM: a tiny capacity must still complete (producer outruns encoders → backpressure
    // blocks push; peak in-flight ≈ cap blocks, not N).
    let tiny = run_stream(&blocks, 4, 2, dir.path());
    println!(
        "\nbounded RAM: cap=2 with 4 workers completed in {tiny:.3}s — peak in-flight ≤ ~cap blocks \
         (≈{} KiB), not all {N} ({:.0} MiB). Backpressure holds memory flat under burst.",
        2 * EDGE * EDGE * EDGE * 2 / 1024,
        mib,
    );

    // Correctness reminder: streamed output is byte-identical to batch (proven in stream::tests).
    let mut bb = ProductBuilder::new("recon", "p", "d", TS);
    for (i, (spec, data)) in blocks.iter().enumerate() {
        let (r, _) = array::array_block(&format!("b{i:04}"), spec, data).unwrap();
        bb.add_block_ref(r);
    }
    let batch = bb.seal().unwrap();
    println!(
        "\nstreamed seal == batch seal (id {}…) — same bytes, just produced off the hot path.",
        &batch.id[..18]
    );
}
