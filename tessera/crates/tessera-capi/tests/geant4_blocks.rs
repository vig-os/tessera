//! #802 under Geant4's real scheduling: worker threads take **event blocks** dynamically, and the
//! producer pushes each block as one batch whose `seq` is the block index.
//!
//! Geant4's task run manager sizes those blocks as
//! `eventModulo = min(sqrt(N), N / grainSize)`, with `grainSize` the thread-pool size
//! (`G4TaskRunManager.cc:292-323`). So once there are more threads than `sqrt(N)`, the block size
//! depends on the thread count, and so does what a "block index" means. `seq` is deterministic only
//! when the producer pins the block size `B` (`SetEventModulo(B)`, as Stage 2's `g4ring.cpp:616-628`
//! does) and declares it (`TsraSessionOptions::seq_block_events`): batch `seq` then covers the events
//! `[seq·B, (seq+1)·B)` whatever the thread count.
//!
//! The model producer sorts each block's hits by time before it pushes them, as a listmode producer
//! sorting each flush does. That is what makes the block cut visible in the bytes: with a different
//! `B` the same hits are sorted in different windows, and seal in a different order.

mod common;

use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use arrow_array::ffi::FFI_ArrowSchema;
use arrow_array::{Array, ArrayRef, Float64Array, StructArray, UInt32Array, UInt64Array};
use arrow_schema::{DataType, Field, Fields};
use common::*;
use tessera_capi::*;

const EVENTS: u64 = 200;

/// Geant4's default event modulo for `n` events on `threads` threads (`G4TaskRunManager.cc`).
fn g4_event_modulo(n: u64, threads: u64) -> u64 {
    let sqrt = (n as f64).sqrt() as u64;
    sqrt.min(n / threads).max(1)
}

/// Hits per event: 0, 1 or 2, so some blocks are empty.
fn hits_of(event: u64) -> u64 {
    event % 3
}

/// A hit's time: scattered across events, so sorting within a block reorders hits across events.
fn time_of(event: u64, hit: u64) -> f64 {
    (((event * 2_654_435_761 + hit * 40_503) % 1_000_003) as f64) * 0.25
}

fn hit_fields() -> Fields {
    Fields::from(vec![
        Field::new("event_id", DataType::UInt64, false),
        Field::new("time_ps", DataType::Float64, false),
        Field::new("crystal", DataType::UInt32, false),
    ])
}

/// The hits of events `[first, end)`, sorted by time: one producer flush.
fn block_batch(first: u64, end: u64) -> StructArray {
    let mut hits: Vec<(u64, f64, u32)> = (first..end)
        .flat_map(|e| {
            (0..hits_of(e)).map(move |h| (e, time_of(e, h), ((e * 31 + h) % 20_160) as u32))
        })
        .collect();
    hits.sort_by(|a, b| a.1.total_cmp(&b.1));
    let cols: Vec<ArrayRef> = vec![
        Arc::new(UInt64Array::from_iter_values(hits.iter().map(|h| h.0))),
        Arc::new(Float64Array::from_iter_values(hits.iter().map(|h| h.1))),
        Arc::new(UInt32Array::from_iter_values(hits.iter().map(|h| h.2))),
    ];
    StructArray::new(hit_fields(), cols, None)
}

/// A Geant4-like run: `threads` workers take blocks of `block` events dynamically and push block
/// `k` as `seq = k`, an empty block as `mark_done`. `declared` is the session's `seq_block_events`
/// and turns on the bridge's check of `event_id` against it. Returns the content hash, or the
/// first refused push's status and message.
fn run(
    threads: u64,
    block: u64,
    declared: Option<u64>,
    seed: u64,
) -> Result<String, (TsraStatus, String)> {
    let dir = tempfile::tempdir().unwrap();
    let strings = Strings::new();
    let mut so = strings.opts(2);
    so.seq_block_events = declared.unwrap_or(0);
    let stage = CString::new(dir.path().join("stage").to_str().unwrap()).unwrap();
    let mut session = std::ptr::null_mut();
    // SAFETY: valid strings, options and out pointer.
    expect(
        unsafe { tsra_session_open(stage.as_ptr(), &so, &mut session) },
        TsraStatus::Ok,
    );

    let rows: u64 = (0..EVENTS).map(hits_of).sum();
    let blocks = EVENTS.div_ceil(block);
    let key = CString::new("event_id").unwrap();
    let mut to = table_opts_producers(rows, blocks as u32, threads as u32);
    if declared.is_some() {
        to.seq_key_column = key.as_ptr();
    }
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(hit_fields())).unwrap();
    let (st, table) = begin_with(session, &schema, &to);
    expect(st, TsraStatus::Ok);

    let next_block = Arc::new(AtomicU64::new(0));
    let refused: Arc<Mutex<Option<(TsraStatus, String)>>> = Arc::default();
    let table_addr = table as usize;
    let workers: Vec<_> = (0..threads)
        .map(|w| {
            let (next_block, refused) = (next_block.clone(), refused.clone());
            std::thread::spawn(move || {
                let mut jitter = seed.wrapping_mul(w + 1) | 1;
                loop {
                    let k = next_block.fetch_add(1, Ordering::Relaxed);
                    if k >= blocks {
                        break;
                    }
                    let (first, end) = (k * block, ((k + 1) * block).min(EVENTS));
                    let data = block_batch(first, end);
                    jitter ^= jitter << 13;
                    jitter ^= jitter >> 7;
                    for _ in 0..jitter % 4 {
                        std::thread::yield_now();
                    }
                    let t = table_addr as *mut TsraTable;
                    let st = if data.is_empty() {
                        // SAFETY: a live table, shared with the other workers.
                        unsafe { tsra_table_mark_done(t, k) }
                    } else {
                        push(t, k, &data)
                    };
                    if st != TsraStatus::Ok {
                        let mut slot = refused.lock().unwrap();
                        slot.get_or_insert((st, last_error().unwrap_or_default()));
                        break;
                    }
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    let refused = refused.lock().unwrap().take();
    if let Some(r) = refused {
        let _ = end(table);
        // SAFETY: a live session, never sealed.
        unsafe { tsra_session_free(session) };
        return Err(r);
    }
    expect(end(table), TsraStatus::Ok);
    let (st, hash) = seal(session, &dir.path().join("p.tsra"));
    expect(st, TsraStatus::Ok);
    Ok(hash)
}

#[test]
fn geant4s_default_block_size_depends_on_the_thread_count() {
    // The premise of the other tests, from G4TaskRunManager's formula: 200 events give blocks of
    // 14 on 1 and 4 threads (sqrt bound) and of 12 on 16 threads (N / grainSize bound).
    assert_eq!(g4_event_modulo(EVENTS, 1), 14);
    assert_eq!(g4_event_modulo(EVENTS, 4), 14);
    assert_eq!(g4_event_modulo(EVENTS, 16), 12);
}

/// A pinned, declared block size: one hash for 1, 4 and 16 threads taking blocks dynamically,
/// with the bridge checking every batch's event ids against its block.
#[test]
fn a_pinned_block_size_seals_one_product_for_every_thread_count() {
    const B: u64 = 10;
    let reference = run(1, B, Some(B), 1).unwrap();
    for threads in [1, 4, 16] {
        for seed in 1..=3 {
            assert_eq!(
                run(threads, B, Some(B), seed).unwrap(),
                reference,
                "N = {threads}, seed {seed}"
            );
        }
    }
}

/// Geant4's default block size, not pinned and not declared: the bytes change with the thread
/// count once it changes the block size (16 threads). A producer error the bridge can't see
/// without a declared `B`.
#[test]
fn an_unpinned_geant4_block_size_moves_the_bytes_with_the_thread_count() {
    let cut = |threads| run(threads, g4_event_modulo(EVENTS, threads), None, 1).unwrap();
    let one = cut(1);
    assert_eq!(cut(4), one, "4 threads still cut blocks of 14");
    assert_ne!(
        cut(16),
        one,
        "16 threads cut blocks of 12, so the per-block sort moved the hits"
    );
}

/// The same producer error with `B` declared: the bridge rejects the first batch whose event ids
/// fall outside `[seq·B, (seq+1)·B)`, instead of sealing different bytes.
#[test]
fn a_declared_block_size_rejects_a_mismatched_cut() {
    let declared = g4_event_modulo(EVENTS, 1); // 14: what a 1-thread run would pin
    let cut = g4_event_modulo(EVENTS, 16); // 12: what 16 threads cut unpinned
    let (st, msg) = run(16, cut, Some(declared), 1).unwrap_err();
    assert_eq!(st, TsraStatus::Order, "{msg}");
    assert!(msg.contains("outside its block"), "{msg}");
    assert!(msg.contains("SetEventModulo(14)"), "{msg}");
    // And the declared size, when the producer honours it, passes at 16 threads.
    run(16, declared, Some(declared), 1).unwrap();
}

/// The declared block size travels with the product.
#[test]
fn the_declared_block_size_is_sealed() {
    let dir = tempfile::tempdir().unwrap();
    let strings = Strings::new();
    let mut so = strings.opts(1);
    so.seq_block_events = 10;
    let stage = CString::new(dir.path().join("stage").to_str().unwrap()).unwrap();
    let mut session = std::ptr::null_mut();
    // SAFETY: valid strings, options and out pointer.
    expect(
        unsafe { tsra_session_open(stage.as_ptr(), &so, &mut session) },
        TsraStatus::Ok,
    );
    let out = dir.path().join("p.tsra");
    expect(seal(session, &out).0, TsraStatus::Ok);
    let manifest = tessera_io::container::Reader::open(&out)
        .unwrap()
        .manifest()
        .clone();
    assert_eq!(manifest.extra["tessera_capi"]["seq_block_events"], 10);
}
