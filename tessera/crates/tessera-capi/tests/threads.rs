//! #802 evidence: N producer threads push one table's batches in a scheduler-chosen order, and the
//! sealed content hash depends only on the `seq → batch` mapping, for every N and every schedule.
//! Without the key (a sequence number taken in arrival order) the hash moves.
//!
//! Two assignment shapes:
//! - **shuffled**: each thread takes batch indices from a shuffled list, so keys arrive in any order.
//!   An arbitrary order can need the whole table buffered, so these runs open the table with a
//!   window of the batch count, and assert the bound: the batches waiting out of order never exceed
//!   it.
//! - **claimed**: threads claim keys from a shared counter, in increasing order (the shape of
//!   `G4TaskRunManager`'s block scheduling), through a window of 2. A thread that runs ahead of a
//!   slow one blocks on the window (back-pressure), and since the thread holding the key due is never
//!   the one waiting, it can't deadlock.
//!
//! Every run declares its thread count as `producers`, so an assignment that could deadlock the
//! window fails with a stall error instead (`a_stall_is_an_error_not_a_hang`).

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

const CLAIMED_BATCHES: u64 = 200;
const CLAIMED_BATCH: u64 = 997;

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

struct Run {
    hash: String,
    /// The most batches that waited out of order at once.
    peak: usize,
    /// Pushes that blocked on the window.
    waits: u64,
}

/// `threads` threads take batch indices from `plan.queue` in order and push them, with a random
/// yield between building a batch and pushing it.
fn run(threads: usize, seed: u64, key: Key, plan: &Plan) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let session = open(&dir.path().join("stage"), 2);
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(fields())).unwrap();
    let rows = plan.queue.len() as u64 * plan.batch;
    let opts = table_opts_producers(rows, plan.window, threads as u32);
    let (st, table) = begin_with(session, &schema, &opts);
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
    let (peak, waits) = {
        let t = unsafe { &*table };
        (t.reorder_peak(), t.reorder_waits())
    };
    assert!(
        peak <= plan.window as usize,
        "{peak} batches waited out of order, past the window of {}",
        plan.window
    );
    expect(end(table), TsraStatus::Ok);
    let (st, hash) = seal(session, &dir.path().join("p.tsra"));
    expect(st, TsraStatus::Ok);
    Run { hash, peak, waits }
}

fn shuffled_plan(seed: u64) -> Plan {
    Plan {
        queue: shuffled(seed),
        batch: BATCH,
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
            let r = run(threads, seed, Key::Explicit, &shuffled_plan(seed));
            eprintln!(
                "shuffled N = {threads}, seed {seed}: peak {} of window {BATCHES}",
                r.peak
            );
            assert!(
                r.peak > 1,
                "N = {threads}, seed {seed}: nothing ever waited out of order"
            );
            assert_eq!(r.hash, reference, "N = {threads}, seed {seed}");
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

/// Run `f` on a thread and fail if it hasn't returned in `secs` seconds: a hang is a failure.
fn within<R: Send + 'static>(secs: u64, f: impl FnOnce() -> R + Send + 'static) -> R {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(std::time::Duration::from_secs(secs))
        .unwrap_or_else(|_| panic!("did not finish in {secs} s: a hang"))
}

/// Keys claimed in increasing order through a window of 2 from 1, 4 and 16 threads: threads that
/// run ahead block on the window, nothing deadlocks, at most 2 batches wait, and the hash is the
/// in-order one.
#[test]
fn claimed_keys_are_back_pressured_by_the_window_without_deadlock() {
    let (in_order, runs) = within(120, || {
        let rows = CLAIMED_BATCHES * CLAIMED_BATCH;
        let dir = tempfile::tempdir().unwrap();
        let in_order = write_through_capi(dir.path(), rows, CLAIMED_BATCH, rows);
        let plan = Plan {
            queue: (0..CLAIMED_BATCHES).collect(),
            batch: CLAIMED_BATCH,
            window: 2,
        };
        let runs: Vec<_> = [1usize, 4, 16]
            .into_iter()
            .flat_map(|n| (1..=2).map(move |seed| (n, seed)))
            .map(|(n, seed)| (n, seed, run(n, seed, Key::Explicit, &plan)))
            .collect();
        (in_order, runs)
    });
    let mut waited = 0;
    for (n, seed, r) in runs {
        eprintln!(
            "claimed N = {n}, seed {seed}: peak {} of window 2, {} waits",
            r.peak, r.waits
        );
        assert_eq!(r.hash, in_order, "N = {n}, seed {seed}");
        waited += r.waits;
    }
    assert!(
        waited > 0,
        "no push ever blocked, so the back-pressure path did not run"
    );
}

/// An assignment that breaks the claimed-order precondition, through a small window: instead of a
/// hang, the producers get `TSRA_STATUS_ORDER` (a stall), the end fails, and nothing seals.
#[test]
fn a_stall_is_an_error_not_a_hang() {
    let outcome = within(120, || {
        let dir = tempfile::tempdir().unwrap();
        let session = open(&dir.path().join("stage"), 1);
        let schema = FFI_ArrowSchema::try_from(DataType::Struct(fields())).unwrap();
        let (_, table) = begin_with(session, &schema, &table_opts_producers(ROWS, 2, 4));
        let table_addr = table as usize;
        // Keys in descending order: every producer's first key is far past key 0.
        let queue: Arc<Vec<u64>> = Arc::new((0..BATCHES).rev().collect());
        let taken = Arc::new(AtomicUsize::new(0));
        let refused: Vec<_> = (0..4)
            .map(|_| {
                let (queue, taken) = (queue.clone(), taken.clone());
                std::thread::spawn(move || {
                    while let Some(&b) = queue.get(taken.fetch_add(1, Ordering::Relaxed)) {
                        let st = push(table_addr as *mut TsraTable, b, &batch(b * BATCH, BATCH));
                        if st != TsraStatus::Ok {
                            return Some((st, last_error().unwrap_or_default()));
                        }
                    }
                    None
                })
            })
            .collect::<Vec<_>>() // spawn every producer before joining any
            .into_iter()
            .map(|w| w.join().unwrap())
            .collect();
        let end_status = end(table);
        let end_error = last_error().unwrap_or_default();
        let sealed = seal(session, &dir.path().join("p.tsra")).0;
        (refused, end_status, end_error, sealed)
    });
    let (refused, end_status, end_error, sealed) = outcome;
    for (i, r) in refused.iter().enumerate() {
        let (st, msg) = r.as_ref().expect("every producer stops on the stall");
        assert_eq!(*st, TsraStatus::Order, "producer {i}: {msg}");
        assert!(msg.contains("stalled"), "producer {i}: {msg}");
    }
    assert_eq!(end_status, TsraStatus::Order, "{end_error}");
    assert!(end_error.contains("stalled"), "{end_error}");
    assert_eq!(sealed, TsraStatus::State);
}

/// A missing key fails the table at the end (an error, never a hang), and the session then refuses
/// to seal.
#[test]
fn a_gap_from_many_threads_fails_the_end() {
    let dir = tempfile::tempdir().unwrap();
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(fields())).unwrap();
    // Every key but 17 is pushed, in shuffled order, from 16 threads.
    let session = open(&dir.path().join("b"), 1);
    let (_, table) = begin_with(
        session,
        &schema,
        &table_opts_producers(ROWS, BATCHES as u32, 16),
    );
    let table_addr = table as usize;
    let queue = Arc::new(shuffled(7));
    let taken = Arc::new(AtomicUsize::new(0));
    let workers: Vec<_> = (0..16)
        .map(|_| {
            let (queue, taken) = (queue.clone(), taken.clone());
            std::thread::spawn(move || {
                while let Some(&b) = queue.get(taken.fetch_add(1, Ordering::Relaxed)) {
                    if b != 17 {
                        let st = push(table_addr as *mut TsraTable, b, &batch(b * BATCH, BATCH));
                        expect(st, TsraStatus::Ok);
                    }
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    expect(end(table), TsraStatus::Order);
    assert!(
        last_error().unwrap().contains("wait for seq 17"),
        "{:?}",
        last_error()
    );
    expect(
        seal(session, &dir.path().join("b.tsra")).0,
        TsraStatus::State,
    );
    assert!(!dir.path().join("b.tsra").exists());
}
/// A key with no rows holds its place, as an empty batch or marked done: both seal the product of
/// the rows alone.
#[test]
fn an_empty_batch_and_mark_done_keep_the_keys_dense() {
    let dir = tempfile::tempdir().unwrap();
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(fields())).unwrap();
    let session = open(&dir.path().join("stage"), 1);
    let (_, table) = begin(session, &schema, 20);
    expect(push(table, 3, &batch(10, 10)), TsraStatus::Ok);
    // SAFETY: a live table.
    expect(unsafe { tsra_table_mark_done(table, 1) }, TsraStatus::Ok);
    expect(push(table, 2, &batch(10, 0)), TsraStatus::Ok);
    expect(push(table, 0, &batch(0, 10)), TsraStatus::Ok);
    // SAFETY: a live table; key 1 is already done.
    expect(unsafe { tsra_table_mark_done(table, 1) }, TsraStatus::Order);
    expect(end(table), TsraStatus::Ok);
    let (st, hash) = seal(session, &dir.path().join("p.tsra"));
    expect(st, TsraStatus::Ok);
    let plain = tempfile::tempdir().unwrap();
    assert_eq!(hash, write_through_capi(plain.path(), 20, 10, 20));
}
