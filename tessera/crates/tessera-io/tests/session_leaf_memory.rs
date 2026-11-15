//! Real-memory test of what a [`WriteSession`] holds per committed leaf until the seal (#759), with
//! a counting global allocator.
//!
//! The session must keep each committed block's manifest entry until the seal writes the manifest,
//! and nothing more per block. A session that kept every leaf's `BlockRef`, its spec a
//! `serde_json::Value` tree of small allocations, held 13,522 B per leaf here for a 2,404 B entry
//! (heaptrack measured 12.8 KB live per leaf for the consumer's 12-column schema, whose entry is
//! 3.1 KB: `docs/spikes/715-rss-proof-diagnosis.md`, #715), an O(leaves) term in the writer's
//! memory that the format doesn't need.
//!
//! **One `#[test]`, on purpose**, for the reason `tests/staged_job_memory.rs` gives: the allocator
//! counts every thread of the process, so nothing else may allocate while it measures. Add a
//! measurement to [`session_leaf_memory`], never as a second `#[test]`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use tessera_core::block::table::{Column, TableSpec};
use tessera_core::block::{BlockKind, BlockName, BlockRef};
use tessera_io::{block_name, block_partition, WriteSession, BLOCK_ROWS};

/// What the session may hold per committed leaf besides that leaf's serialized manifest entry: its
/// slot in the session's list of committed blocks (40 B, up to twice that while the list's capacity
/// runs ahead of its length) and its name's own copy (11 B here), with room to spare.
const LEAF_OVERHEAD: usize = 128;

/// Counts the live heap.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to `System` unchanged; only counts sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc`'s contract, which is `System.alloc`'s.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
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

#[test]
fn session_leaf_memory() {
    // A throwaway session first, so no measurement pays the one-time setup (lazy statics, the
    // tracing callsites of a commit).
    held_by_leaves(4);

    let failed: Vec<String> = [64, 256]
        .into_iter()
        .filter_map(|n| {
            let (held, entries) = held_by_leaves(n);
            let bound = entries + n * LEAF_OVERHEAD;
            eprintln!(
                "{n} leaves: session holds {held} B ({} B per leaf), manifest entries {entries} B \
                 ({} B per leaf), bound {bound} B",
                held / n,
                entries / n,
            );
            (held > bound).then(|| {
                format!(
                    "{n} leaves: the session holds {held} B ({} B per leaf) for manifest entries of \
                     {entries} B ({} B per leaf); bound {bound} B (entries + {LEAF_OVERHEAD} B per leaf)",
                    held / n,
                    entries / n
                )
            })
        })
        .collect();
    assert!(failed.is_empty(), "\n{}", failed.join("\n"));
}

/// The consumer's shape (#715): 12 columns, each with a description and a unit, 3,053 B of spec
/// JSON per leaf there.
fn spec() -> TableSpec {
    let columns = (0..12)
        .map(|i| Column {
            name: format!("column_{i:02}"),
            dtype: ["u8", "u4", "f4", "i2"][i % 4].into(),
            description: Some(format!(
                "Column {i} of the listmode event table: a detector quantity recorded per \
                 coincidence event, as the acquisition reported it."
            )),
            unit: Some(["ns", "keV", "mm", "1"][i % 4].into()),
            ..Default::default()
        })
        .collect();
    TableSpec {
        columns,
        rows: 0,
        row_index: None,
        partition: None,
    }
}

/// Commit the `n` leaves of a partitioned table to a fresh session, the caller dropping each
/// returned reference as `StreamWriter`'s committer does. Returns the heap the session holds after
/// the commits above what it held before them, and the total size of the leaves' entries in the
/// manifest its seal writes, each serialized as the JSON the manifest stores.
fn held_by_leaves(n: usize) -> (usize, usize) {
    let dir = tempfile::tempdir().unwrap();
    let mut ws = WriteSession::create(
        &dir.path().join("session"),
        "recon",
        "p",
        "d",
        "2024-01-01T00:00:00Z",
    )
    .unwrap();
    let template = spec();
    let total = n as u64;
    // Every leaf's reference is serialized before the measurement starts and parsed inside it, so
    // the heap the measurement counts is the session's: a reference built outside would be freed
    // inside, as `append_block_from` consumes it, and hide what the session keeps.
    let refs: Vec<String> = (0..total)
        .map(|i| {
            let leaf = TableSpec {
                rows: BLOCK_ROWS as u64,
                partition: block_partition(i, total),
                ..template.clone()
            };
            let r = BlockRef {
                name: BlockName::new(block_name("events", i, total).unwrap()).unwrap(),
                kind: BlockKind::Table,
                digest: None,
                spec: serde_json::to_value(&leaf).unwrap(),
            };
            serde_json::to_string(&r).unwrap()
        })
        .collect();

    let before = LIVE.load(Ordering::Relaxed);
    for r in &refs {
        let r: BlockRef = serde_json::from_str(r).unwrap();
        drop(ws.append_block_from(r, &b"leaf"[..]).unwrap());
    }
    let held = LIVE
        .load(Ordering::Relaxed)
        .checked_sub(before)
        .expect("the session freed heap it didn't allocate: the measurement counts something else");

    let manifest = ws.seal(&dir.path().join("p.tsra")).unwrap();
    assert_eq!(manifest.blocks.len(), n);
    let entries = manifest
        .blocks
        .iter()
        .map(|b| serde_json::to_vec(b).unwrap().len())
        .sum();
    (held, entries)
}
