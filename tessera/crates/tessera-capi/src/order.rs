//! Deterministic ordering of batches pushed from many producer threads (#802).
//!
//! The sealed bytes of a table are a function of its rows **in order**, so a table fed by several
//! threads is deterministic only if the order does not come from the scheduler. Every batch carries
//! an explicit sequence number (`0, 1, 2, …`, chosen by the producer from its data, e.g. the event
//! block index), and [`Reorder`] hands batches to the writer strictly in that order, whatever order
//! the threads deliver them in.
//!
//! **Bounded memory, and the precondition it costs.** The buffer admits the keys
//! `next .. next + window` (`window` of them, `next` being the next key due); a push of a key at or
//! past `next + window` blocks until the window moves. That bound holds the buffer to `window`
//! batches, and it is safe only if **the key `next` is never held by a thread that is itself blocked**.
//! That holds when keys are *claimed in increasing order and each thread pushes its own keys in
//! increasing order*, the shape of `G4TaskRunManager`'s block scheduling and of a shared counter: the
//! thread that claimed `next` claimed nothing below it that is still unpushed, so it is not waiting.
//! It does **not** hold for an arbitrary assignment. If every producer thread holds a key at least
//! `window` past `next` while `next` itself is still unclaimed (or held by one of them), every push
//! waits and nothing ever releases them: a deadlock, which a single thread pushing key `window`
//! before key 0 reaches too. The window can't tell the two cases apart, because it can't know how many
//! producers exist. ADR-0065 §7.2 removes the precondition by spilling early batches to disk instead of
//! blocking; this prototype measures the ordering, not that.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// `None` once the downstream failed or the buffer was closed.
    downstream: Option<D>,
}

/// A reorder buffer releasing items to a [`Downstream`] in sequence-number order.
pub struct Reorder<T, D> {
    window: u64,
    state: Mutex<State<T, D>>,
    advanced: Condvar,
    /// Pushes that had to wait for the window (observability; tests prove the path ran).
    waits: AtomicU64,
}

fn order_err(message: impl Into<String>) -> CapiError {
    CapiError::new(TsraStatus::Order, message)
}

impl<T, D: Downstream<T>> Reorder<T, D> {
    /// A buffer releasing to `downstream`, admitting the `window` (≥ 1) keys from the next one due.
    pub fn new(downstream: D, window: u64) -> Self {
        Reorder {
            window: window.max(1),
            state: Mutex::new(State {
                next: 0,
                pending: BTreeMap::new(),
                downstream: Some(downstream),
            }),
            advanced: Condvar::new(),
            waits: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State<T, D>> {
        // A poisoned lock means a push panicked while holding it; the guard reported that panic and
        // the state it left is still consistent (every mutation is a single insert/remove).
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Accept item `seq`; release it, and every item after it that is already waiting, if it is the
    /// next one due. Blocks while `seq >= next + window` (see the module docs for when that is safe).
    pub fn push(&self, seq: u64, item: T) -> CapiResult<()> {
        let mut st = self.lock();
        loop {
            if st.downstream.is_none() {
                return Err(CapiError::new(
                    TsraStatus::Writer,
                    "the table is closed or its writer stopped; tsra_table_end reports why",
                ));
            }
            if seq < st.next || st.pending.contains_key(&seq) {
                return Err(order_err(format!("batch {seq} was already pushed")));
            }
            if seq - st.next < self.window {
                break;
            }
            self.waits.fetch_add(1, Ordering::Relaxed);
            st = self.advanced.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.pending.insert(seq, item);
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

    /// How many times a push waited for the window to move.
    pub fn waits(&self) -> u64 {
        self.waits.load(Ordering::Relaxed)
    }

    /// Close the buffer and hand back the downstream (dropping it is what tells the writer the input
    /// ended). `Err` if a batch is still waiting for an earlier one that never came. Every push still
    /// blocked in the window, and every later push, fails.
    pub fn close(&self) -> CapiResult<(u64, Option<D>)> {
        let mut st = self.lock();
        let downstream = st.downstream.take();
        self.advanced.notify_all();
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
        let r = Reorder::new(Collect(Arc::clone(&out)), 8);
        for seq in [3, 1, 0, 2, 5, 4] {
            r.push(seq, seq).unwrap();
        }
        assert_eq!(r.close().unwrap().0, 6);
        assert_eq!(*out.lock().unwrap(), vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn duplicate_and_gap_are_errors() {
        let r = Reorder::new(Collect::default(), 8);
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
        let r = Arc::new(Reorder::new(Collect(Arc::clone(&out)), 2));
        r.push(1, 1).unwrap();
        // With next = 0 and a window of 2 the buffer admits keys 0 and 1: key 2 is the first to wait.
        let ahead = {
            let r = Arc::clone(&r);
            std::thread::spawn(move || r.push(2, 2))
        };
        assert!(out.lock().unwrap().is_empty(), "nothing goes before seq 0");
        // Releasing 0 and 1 moves the window to 2..4, which admits the waiting push.
        r.push(0, 0).unwrap();
        ahead.join().unwrap().unwrap();
        assert_eq!(*out.lock().unwrap(), vec![0, 1, 2]);
    }
}
