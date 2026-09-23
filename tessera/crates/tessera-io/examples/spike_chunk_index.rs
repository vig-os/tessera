//! #221-B — chunk-index leaf granularity: index overhead vs pruning selectivity.
//!
//! ADR-0028 §3's `{hash, stats}` chunk-index (`tessera_core::chunk_index`) trades **index size** (one
//! `{digest, min, max, count, sum}` entry per sub-block — grows as chunks shrink) against **pruning
//! power** (finer chunks → tighter per-chunk ranges → more chunks provably skipped on a ranged read).
//! The knee of that trade is the default leaf granularity. Pruning only pays when the data has
//! **locality** (chunk-local value clustering); fully random columns can't be pruned at any granularity
//! — so we sweep three locality regimes. Storage/IO metrics only (ADR-0031 principle).
//!
//! Run: `cargo run -p tessera-io --example spike_chunk_index --release`. Deterministic.

use tessera_core::chunk_index::ChunkIndex;

/// Compact on-disk size of one index entry, in bytes: a raw 32-byte BLAKE3 digest + the stat tuple
/// (count u64 + min i64 + max i64 + sum i128 = 40 B). The real index block is a Vortex column that
/// compresses this further, so this is a conservative (high) estimate of the overhead.
const ENTRY_BYTES: usize = 32 + 40;

fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[derive(Clone, Copy)]
enum Pattern {
    /// Monotonic — a linear index / timestamp column. Best case: per-chunk ranges are disjoint.
    Sorted,
    /// Locally coherent within a fixed window (spatial/temporal locality), windows in order.
    Clustered,
    /// Uniform random over the full range. Worst case: every chunk spans ~everything.
    Random,
}

impl Pattern {
    fn name(self) -> &'static str {
        match self {
            Pattern::Sorted => "sorted",
            Pattern::Clustered => "clustered",
            Pattern::Random => "random",
        }
    }
    fn gen(self, n: usize) -> Vec<i64> {
        let window = 4096i64; // clustered locality width
        (0..n)
            .map(|i| match self {
                Pattern::Sorted => i as i64,
                Pattern::Clustered => {
                    let base = (i as i64 / window) * window;
                    base + (mix(i as u64) % window as u64) as i64
                }
                Pattern::Random => (mix(i as u64) % n as u64) as i64,
            })
            .collect()
    }
}

/// One chunk's BLAKE3 digest over its little-endian bytes (the real per-sub-block content hash).
fn chunk_digest(chunk: &[i64]) -> String {
    let mut bytes = Vec::with_capacity(chunk.len() * 8);
    for &v in chunk {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    tessera_core::hash::digest(&bytes)
}

fn main() {
    let log2n = 20u32;
    let n = 1usize << log2n; // 1,048,576 i64 = 8 MiB payload
    let payload = (n * 8) as f64;
    // query window = 1% of the value range [0, n), centred — what a typical ranged read selects.
    let (qlo, qhi) = ((n as i64 * 40) / 100, (n as i64 * 41) / 100);

    println!("# #221-B — chunk-index granularity: index overhead vs pruning");
    println!(
        "# {n} i64 ({} MiB), query [{qlo},{qhi}] (~1% of value range)\n",
        n * 8 / (1 << 20)
    );

    for pattern in [Pattern::Sorted, Pattern::Clustered, Pattern::Random] {
        let data = pattern.gen(n);
        println!("## {}", pattern.name());
        println!(
            "{:>10} {:>9} {:>11} {:>10} {:>11} {:>10}",
            "chunk", "nchunks", "index B", "index %", "kept chnk", "scan %"
        );
        for log2cs in [10u32, 12, 14, 16, 18, 20] {
            let cs = 1usize << log2cs;
            let mut idx = ChunkIndex::new();
            for chunk in data.chunks(cs) {
                idx.push(chunk_digest(chunk), chunk);
            }
            let nchunks = idx.len();
            let index_bytes = nchunks * ENTRY_BYTES;
            let index_pct = 100.0 * index_bytes as f64 / payload;
            let kept = idx.prune(qlo, qhi).len();
            // rows that survive pruning (a scan touches whole kept chunks) as a fraction of all rows.
            let scan_pct = 100.0 * (kept * cs).min(n) as f64 / n as f64;
            println!(
                "{:>10} {:>9} {:>11} {:>9.3}% {:>11} {:>9.2}%",
                cs, nchunks, index_bytes, index_pct, kept, scan_pct
            );
        }
        println!();
    }
    println!("# index % = ENTRY_BYTES·nchunks ÷ payload (conservative; real index is a compressed Vortex col).");
    println!("# scan % = rows in kept chunks ÷ total (lower = better pruning). Knee = default leaf size.");
}
