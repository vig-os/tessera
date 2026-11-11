//! Deterministic ordering of batches pushed from many producer threads (#802).
//!
//! The sealed bytes of a table are a function of its rows **in order**, so a table fed by several
//! threads is deterministic only if the order does not come from the scheduler. Every batch carries
//! an explicit, dense sequence number (`0, 1, 2, …`, derived by the producer from its data, e.g. the
//! Geant4 event-block index), and [`Reorder`] hands batches to the writer strictly in that order,
//! whatever order the threads deliver them in. A key with no rows is pushed as an empty batch or
//! marked done (`tsra_table_mark_done`), so the sequence stays dense.
//!
//! # Why this is not a second scheduler
//!
//! `StreamWriter` already commits in order (`commit_loop` reorders encode completions), but it
//! numbers jobs itself, in submission order (`StreamWriter::submit`, `self.seq`), through `&mut self`,
//! and a job is a whole leaf of `BLOCK_ROWS` rows. A producer's batch is far smaller and many of them
//! make one leaf, so batches must be in order **before** rows are cut into leaves. This buffer is
//! that sequencer: it turns N producer threads into the single ordered row stream `push_table` reads
//! on one thread, and everything downstream (parallel encode, in-order commit, the credit window) is
//! `StreamWriter`'s, unchanged.
//!
//! # Back-pressure, and what is an error
//!
//! The buffer admits keys `next .. next + window` (`next` being the next key due), so at most
//! `window` batches wait in it ([`Reorder::peak_pending`] records the high-water mark). A push of a
//! key further ahead **blocks** until the window moves: that is the back-pressure on a thread that
//! runs ahead, e.g. while one slow event holds `next`. The release of in-order batches into the
//! writer blocks too while the writer's credit window is full, so producers wait on the encoders.
//!
//! **The blocking wait is safe when keys are claimed in increasing order and each thread pushes its
//! own keys in increasing order** (a shared counter, or `G4TaskRunManager`'s consecutive event
//! blocks): the thread that holds `next` has nothing smaller left to push, so it is never the one
//! waiting, and its push moves the window. Under any other assignment every producer can end up
//! waiting for a key none of them will push. That is a deadlock, and it is turned into an error,
//! not a hang:
//!
//! - **Stall** (every declared producer thread waits on the window): with `producers` declared,
//!   the push that makes the count complete marks the table stalled, every waiting push returns
//!   `TSRA_STATUS_ORDER`, and so does every later push and the end. It's exact, with no timer: while
//!   any producer thread is outside a push (building its next batch) the count isn't complete, so a
//!   slow event is never mistaken for a stall. Without `producers` (0) there is no detection, and a
//!   producer that breaks the precondition hangs; the ABI should make `producers` required.
//! - **Duplicate** (a key already pushed or released): refused at once.
//! - **Missing at the end** (a gap below a pushed key): [`Reorder::close`] refuses, and the table
//!   doesn't seal.

use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex};

use crate::error::{CapiError, CapiResult, TsraStatus};

/// Where in-order batches go: the writer's input channel, or a test's collector. An `Err` means the
/// writer has stopped; the reorder buffer then refuses every later push.
pub trait Downstream<T> {
    fn send(&mut self, item: T) -> Result<(), String>;
}

impl<T> Downstream<T> for std::sync::mpsc::SyncSender<T> {
    fn send(&mut self, item: T) -> Result<(), String> {
        std::sync::mpsc::SyncSender::send(self, item)
            .map_err(|_| "the writer stopped; tsra_table_end reports why".into())
    }
}

struct State<T, D> {
    next: u64,
    pending: BTreeMap<u64, T>,
    /// The most items ever waiting at once.
    peak_pending: usize,
    /// The keys of the pushes blocked on the window right now (key → how many), and the count of
    /// pushes that ever blocked.
    waiting: BTreeMap<u64, u64>,
    waits: u64,
    /// Every declared producer was blocked at once: the message every later push returns.
    stalled: Option<String>,
    /// `None` once the downstream failed or the buffer was closed.
    downstream: Option<D>,
}

/// A reorder buffer releasing items to a [`Downstream`] in sequence-number order.
pub struct Reorder<T, D> {
    window: u64,
    /// The declared number of producer threads; `None` disables stall detection.
    producers: Option<u64>,
    state: Mutex<State<T, D>>,
    advanced: Condvar,
}

fn order_err(message: impl Into<String>) -> CapiError {
    CapiError::new(TsraStatus::Order, message)
}

impl<T, D: Downstream<T>> Reorder<T, D> {
    /// A buffer releasing to `downstream`, admitting the `window` (≥ 1) keys from the next one due,
    /// fed by `producers` threads (0: unknown, no stall detection).
    pub fn new(downstream: D, window: u64, producers: u64) -> Self {
        Reorder {
            window: window.max(1),
            producers: (producers > 0).then_some(producers),
            state: Mutex::new(State {
                next: 0,
                pending: BTreeMap::new(),
                peak_pending: 0,
                waiting: BTreeMap::new(),
                waits: 0,
                stalled: None,
                downstream: Some(downstream),
            }),
            advanced: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State<T, D>> {
        // A poisoned lock means a push panicked while holding it; the guard reported that panic and
        // the state it left is still consistent (every mutation is a single field update).
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wait<'a>(
        &self,
        st: std::sync::MutexGuard<'a, State<T, D>>,
    ) -> std::sync::MutexGuard<'a, State<T, D>> {
        self.advanced.wait(st).unwrap_or_else(|e| e.into_inner())
    }

    /// Accept item `seq`; release it, and every item after it that is already waiting, if it is the
    /// next one due. Refuses a duplicate; blocks while `seq >= next + window`; fails on a stall.
    pub fn push(&self, seq: u64, item: T) -> CapiResult<()> {
        let mut st = self.lock();
        let mut counted = false;
        let admitted = loop {
            if let Some(why) = &st.stalled {
                break Err(order_err(why.clone()));
            }
            if st.downstream.is_none() {
                break Err(CapiError::new(
                    TsraStatus::Writer,
                    "the table is closed or its writer stopped; tsra_table_end reports why",
                ));
            }
            if seq < st.next || st.pending.contains_key(&seq) {
                break Err(order_err(format!("batch {seq} was already pushed")));
            }
            if seq - st.next < self.window {
                break Ok(());
            }
            if !counted {
                counted = true;
                *st.waiting.entry(seq).or_default() += 1;
                st.waits += 1;
            }
            // A stall: every declared producer waits on a key still past the window. Count only
            // those: a waiter that was woken because the window moved, and hasn't run yet, is
            // already admitted and must not count.
            let bound = st.next + self.window;
            let stuck: u64 = st.waiting.range(bound..).map(|(_, n)| n).sum();
            if self.producers.is_some_and(|p| stuck >= p) {
                st.stalled = Some(format!(
                    "the table stalled: all {stuck} producer threads wait on the reorder window \
                     for batch {}, which none of them will push (keys must be claimed in \
                     increasing order and each thread must push its keys in increasing order)",
                    st.next
                ));
                self.advanced.notify_all();
                continue;
            }
            st = self.wait(st);
        };
        if counted {
            if let Some(n) = st.waiting.get_mut(&seq) {
                *n -= 1;
                if *n == 0 {
                    st.waiting.remove(&seq);
                }
            }
        }
        admitted?;
        st.pending.insert(seq, item);
        st.peak_pending = st.peak_pending.max(st.pending.len());
        let mut released = false;
        loop {
            let next = st.next;
            let Some(item) = st.pending.remove(&next) else {
                break;
            };
            let downstream = st.downstream.as_mut().expect("checked above");
            if let Err(e) = downstream.send(item) {
                st.downstream = None;
                st.pending.clear();
                self.advanced.notify_all();
                return Err(CapiError::new(TsraStatus::Writer, e));
            }
            st.next += 1;
            released = true;
        }
        if released {
            self.advanced.notify_all();
        }
        Ok(())
    }

    /// The most items ever waiting at once (never more than the window).
    pub fn peak_pending(&self) -> usize {
        self.lock().peak_pending
    }

    /// How many pushes blocked on the window.
    pub fn waits(&self) -> u64 {
        self.lock().waits
    }

    /// Close the buffer and hand back the downstream (dropping it is what tells the writer the input
    /// ended), with the count of keys released. `Err` if the table stalled, or if a batch is still
    /// waiting for an earlier one that never came. Every later push fails.
    pub fn close(&self) -> CapiResult<(u64, Option<D>)> {
        let mut st = self.lock();
        let downstream = st.downstream.take();
        self.advanced.notify_all();
        if let Some(why) = &st.stalled {
            return Err(order_err(why.clone()));
        }
        if let Some((&first, _)) = st.pending.iter().next() {
            let waiting = st.pending.len();
            let next = st.next;
            st.pending.clear();
            return Err(order_err(format!(
                "{waiting} batch(es) from seq {first} on wait for seq {next}, which was never pushed"
            )));
        }
        Ok((st.next, downstream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Default, Debug)]
    struct Collect(Arc<Mutex<Vec<u64>>>);
    impl Downstream<u64> for Collect {
        fn send(&mut self, item: u64) -> Result<(), String> {
            self.0.lock().unwrap().push(item);
            Ok(())
        }
    }

    #[test]
    fn releases_in_sequence_order_whatever_the_arrival_order() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let r = Reorder::new(Collect(Arc::clone(&out)), 8, 0);
        for seq in [3, 1, 0, 2, 5, 4] {
            r.push(seq, seq).unwrap();
        }
        assert_eq!(
            r.peak_pending(),
            3,
            "3, 1 and 0 waited together before 0 released them"
        );
        assert_eq!(r.close().unwrap().0, 6);
        assert_eq!(*out.lock().unwrap(), vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn duplicate_and_gap_are_errors() {
        let r = Reorder::new(Collect::default(), 8, 0);
        r.push(0, 0).unwrap();
        assert_eq!(r.push(0, 0).unwrap_err().status, TsraStatus::Order);
        r.push(2, 2).unwrap();
        assert_eq!(r.push(2, 2).unwrap_err().status, TsraStatus::Order);
        let gap = r.close().unwrap_err();
        assert_eq!(gap.status, TsraStatus::Order);
        assert!(gap.message.contains("wait for seq 1"), "{}", gap.message);
    }

    #[test]
    fn a_push_past_the_window_waits_for_the_next_one_due() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let r = Arc::new(Reorder::new(Collect(Arc::clone(&out)), 2, 2));
        r.push(1, 1).unwrap();
        // With next = 0 and a window of 2 the buffer admits keys 0 and 1: key 2 is the first to wait.
        let ahead = {
            let r = Arc::clone(&r);
            std::thread::spawn(move || r.push(2, 2))
        };
        while r.waits() == 0 {
            std::thread::yield_now();
        }
        assert!(out.lock().unwrap().is_empty(), "nothing goes before seq 0");
        // One of the 2 producers waits; the other pushes 0, which moves the window to 2..4.
        r.push(0, 0).unwrap();
        ahead.join().unwrap().unwrap();
        assert_eq!(*out.lock().unwrap(), vec![0, 1, 2]);
        assert!(r.peak_pending() <= 2);
    }

    #[test]
    fn a_stall_is_an_error_not_a_hang() {
        // One producer pushes key 2 before key 0, through a window of 2: it would wait for itself.
        let r = Reorder::new(Collect::default(), 2, 1);
        let stall = r.push(2, 2).unwrap_err();
        assert_eq!(stall.status, TsraStatus::Order);
        assert!(stall.message.contains("stalled"), "{}", stall.message);
        assert_eq!(
            r.push(0, 0).unwrap_err().status,
            TsraStatus::Order,
            "a stall is final"
        );
        assert!(r.close().unwrap_err().message.contains("stalled"));
    }
}
