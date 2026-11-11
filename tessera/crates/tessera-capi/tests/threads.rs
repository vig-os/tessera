//! #802 evidence: N producer threads push one table's batches in a scheduler-chosen order, and the
//! sealed content hash is the same for every N, because each batch carries its sequence number.
//! Without the key (a sequence number taken in arrival order) the hash moves.
//!
//! Two assignment shapes, because the reorder window's blocking is safe for one and not the other
//! (`src/order.rs`):
//! - **shuffled**: each thread takes batch indices from a shuffled list, so keys arrive in any order.
//!   The window is set to the batch count, so nothing blocks; this isolates the ordering.
//! - **claimed**: threads claim keys from a shared counter, in increasing order (the shape of
//!   `G4TaskRunManager`'s block scheduling), with a window far smaller than the batch count, so the
//!   backpressure path runs on most pushes.

mod common;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use arrow_array::ffi::FFI_ArrowSchema;
use arrow_schema::DataType;
use common::*;
use tessera_capi::*;

const BATCHES: u64 = 48;
const BATCH: u64 = 3_001;
const ROWS: u64 = BATCHES * BATCH;

/// The claimed runs: many more batches than the window admits.
const CLAIMED_BATCHES: u64 = 200;
const CLAIMED_BATCH: u64 = 997;
const CLAIMED_WINDOW: u32 = 4;

/// xorshift64*: a seeded shuffle and jitter without a dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn shuffled(seed: u64) -> Vec<u64> {
    let mut rng = Rng(seed | 1);
    let mut v: Vec<u64> = (0..BATCHES).collect();
    for i in (1..v.len()).rev() {
        v.swap(i, (rng.next() % (i as u64 + 1)) as usize);
    }
    v
}

#[derive(Clone, Copy)]
enum Key {
    /// The batch's own index: the producer's data decides the order.
    Explicit,
    /// A counter taken at push time: the scheduler decides the order.
    Arrival,
}

/// The batches a run pushes: `queue` lists batch indices in the order threads take them.
struct Plan {
    queue: Vec<u64>,
    batch: u64,
    window: u32,
}

/// `threads` threads take batch indices from `plan.queue` in order and push them, with a random
/// yield between building a batch and pushing it; returns the content hash and the window waits.
fn run(threads: usize, seed: u64, key: Key, plan: &Plan) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let session = open(&dir.path().join("stage"), 2);
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(fields())).unwrap();
    let rows = plan.queue.len() as u64 * plan.batch;
    let (st, table) = begin_with(session, &schema, &table_opts_window(rows, plan.window));
    expect(st, TsraStatus::Ok);
    let queue = Arc::new(plan.queue.clone());
    let batch_rows = plan.batch;
    let taken = Arc::new(AtomicUsize::new(0));
    let arrival = Arc::new(AtomicU64::new(0));
    // The table is shared across threads exactly as a C++ producer shares the pointer.
    let table_addr = table as usize;
    let workers: Vec<_> = (0..threads)
        .map(|w| {
            let (queue, taken, arrival) = (queue.clone(), taken.clone(), arrival.clone());
            std::thread::spawn(move || {
                let mut rng = Rng(seed ^ (w as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                loop {
                    let k = taken.fetch_add(1, Ordering::Relaxed);
                    let Some(&b) = queue.get(k) else { break };
                    let data = batch(b * batch_rows, batch_rows);
                    for _ in 0..rng.next() % 4 {
                        std::thread::yield_now();
                    }
                    let seq = match key {
                        Key::Explicit => b,
                        Key::Arrival => arrival.fetch_add(1, Ordering::SeqCst),
                    };
                    expect(
                        push(table_addr as *mut TsraTable, seq, &data),
                        TsraStatus::Ok,
                    );
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    // SAFETY: every push has returned, so nothing else uses the table right now.
    let waits = unsafe { &*table }.reorder_waits();
    expect(end(table), TsraStatus::Ok);
    let (st, hash) = seal(session, &dir.path().join("p.tsra"));
    expect(st, TsraStatus::Ok);
    Run { hash, waits }
}

struct Run {
    hash: String,
    waits: u64,
}

fn shuffled_plan(seed: u64) -> Plan {
    Plan {
        queue: shuffled(seed),
        batch: BATCH,
        // Admit every key: an arbitrary assignment may hold any key back (src/order.rs).
        window: BATCHES as u32,
    }
}

#[test]
fn the_hash_does_not_depend_on_producer_threads_or_their_scheduling() {
    let reference = run(1, 0, Key::Explicit, &shuffled_plan(0)).hash;
    let in_order = {
        let dir = tempfile::tempdir().unwrap();
        write_through_capi(dir.path(), ROWS, BATCH, ROWS)
    };
    assert_eq!(
        reference, in_order,
        "a shuffled single thread equals an in-order write"
    );
    for threads in [1, 4, 16] {
        for seed in 1..=3 {
            assert_eq!(
                run(threads, seed, Key::Explicit, &shuffled_plan(seed)).hash,
                reference,
                "N = {threads}, seed {seed}"
            );
        }
    }
}

#[test]
fn without_the_key_the_hash_follows_the_scheduler() {
    let reference = run(1, 0, Key::Explicit, &shuffled_plan(0)).hash;
    let mut hashes = std::collections::BTreeSet::new();
    for threads in [1, 4, 16] {
        for seed in 1..=3 {
            let h = run(threads, seed, Key::Arrival, &shuffled_plan(seed)).hash;
            assert_ne!(
                h, reference,
                "N = {threads}, seed {seed}: arrival order sealed the data order"
            );
            hashes.insert(h);
        }
    }
    eprintln!(
        "arrival order: {} distinct hashes over 9 runs",
        hashes.len()
    );
    assert!(
        hashes.len() > 1,
        "arrival order is not even stable across runs"
    );
}

/// Keys claimed in increasing order through a window of 4 over 200 batches: the backpressure path
/// runs, nothing deadlocks, and the hash is still the in-order one for every N.
#[test]
fn claimed_keys_pass_a_small_window_without_deadlock() {
    let plan = Plan {
        queue: (0..CLAIMED_BATCHES).collect(),
        batch: CLAIMED_BATCH,
        window: CLAIMED_WINDOW,
    };
    let rows = CLAIMED_BATCHES * CLAIMED_BATCH;
    let in_order = {
        let dir = tempfile::tempdir().unwrap();
        write_through_capi(dir.path(), rows, CLAIMED_BATCH, rows)
    };
    let mut waited = 0;
    for threads in [1, 4, 16] {
        for seed in 1..=2 {
            let r = run(threads, seed, Key::Explicit, &plan);
            assert_eq!(r.hash, in_order, "N = {threads}, seed {seed}");
            waited += r.waits;
        }
    }
    // Not vacuous: with 16 threads claiming through a window of 4, pushes do wait.
    eprintln!("claimed runs: {waited} pushes waited for the window");
    assert!(
        waited > 0,
        "no push ever waited, so the window path did not run"
    );
}
