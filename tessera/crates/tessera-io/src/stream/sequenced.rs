//! The sequenced table input (ADR-0065 §7.2–§7.3, #812): a table lane that many producer threads
//! push numbered batches into, sealed from the batches in sequence order whatever order they
//! arrive in. See [`SequencedTable`].

use std::collections::BTreeMap;
#[cfg(test)]
use std::path::Path;
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::thread;
use std::time::Duration;

use tessera_core::block::table::TableSpec;
use tessera_core::block::StreamedTableError;
use tessera_core::{Error, Result};

use super::{group_rows, rows_of, CreditWindow, Held, OpenTable, StreamWriter};
#[cfg(test)]
use super::{Manifest, StreamWindow, WriteSession};
use crate::table::{ColumnData, TableData, BLOCK_ROWS, ROWS_PER_GROUP};
use crate::write::Lane;

/// The longest [`SequencedOptions::stall_timeout`]: 24 hours (ADR-0065 §7.2).
pub const MAX_STALL_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

/// How a [`SequencedTable`] numbers, admits and bounds its batches (ADR-0065 §7.2). Start from
/// [`Default`] and set the fields to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SequencedOptions {
    /// The lane's first sequence number: its batches are `first_seq, first_seq + 1, …`, each
    /// pushed exactly once. Default `0`.
    pub first_seq: u64,
    /// The most batches admitted at once, counted from the next one due (`next`): a push of
    /// `seq >= next + window` waits. At least 1. Default 64.
    pub window: usize,
    /// The hard horizon: a push of `seq >= next + max_ahead` is refused at once. Under the
    /// claiming contract `N` threads can hold sequence numbers up to `next + window - 1 + N`, so it
    /// must be at least `window + N`, or a well-formed run is refused. At least `window`.
    /// `None` (the default) is `16 × window`.
    pub max_ahead: Option<usize>,
    /// Refuse, with [`StreamedTableError::WouldBlock`], every push that would wait at the gate,
    /// instead of blocking it: a fail-fast mode for producers that can't keep the claiming
    /// contract. Default `false`.
    pub refuse_instead_of_block: bool,
    /// How long the lane's `next` may stay where it is while a push on the lane waits at the gate
    /// before the input fails with [`StreamedTableError::Stalled`]. It measures the absence of
    /// progress, not one push's wait: every advance of `next` restarts it. Finite by design (no
    /// value disables it): more than zero and at most [`MAX_STALL_TIMEOUT`]. Default 10 minutes.
    pub stall_timeout: Duration,
    /// The leaf size: a test seam, like [`StreamWriter::open_table_with_block_rows`]. Anything
    /// but [`BLOCK_ROWS`] writes a non-canonical partition.
    #[doc(hidden)]
    pub block_rows: u64,
}

impl Default for SequencedOptions {
    fn default() -> Self {
        SequencedOptions {
            first_seq: 0,
            window: 64,
            max_ahead: None,
            refuse_instead_of_block: false,
            stall_timeout: Duration::from_secs(10 * 60),
            block_rows: BLOCK_ROWS as u64,
        }
    }
}

impl SequencedOptions {
    /// The horizon `max_ahead` resolves to.
    pub(super) fn horizon(&self) -> usize {
        self.max_ahead
            .unwrap_or_else(|| self.window.saturating_mul(16))
    }

    fn check(&self, name: &str) -> Result<()> {
        let invalid = |why: &str| Err(Error::Invalid(format!("sequenced table '{name}': {why}")));
        if self.window == 0 {
            return invalid("window must be at least 1");
        }
        if self.horizon() < self.window {
            return invalid("max_ahead must be at least window");
        }
        if self.stall_timeout.is_zero() || self.stall_timeout > MAX_STALL_TIMEOUT {
            return invalid("stall_timeout must be more than zero and at most 24 hours");
        }
        Ok(())
    }
}

/// High-water marks of one [`SequencedTable`]'s gate, read with [`SequencedTable::stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SequencedStats {
    /// The most batches of this lane admitted and not yet released at once. At most `window`.
    pub peak_admitted: usize,
    /// The most bytes of admitted, unreleased batches held at once by all the writer's
    /// sequenced inputs together (the byte credit is shared).
    pub peak_admitted_bytes: u64,
    /// How many pushes of this lane waited at the gate.
    pub blocked_pushes: u64,
}

/// What the releasing side of one lane holds, behind the lock that orders its appends.
struct Release {
    table: OpenTable,
    /// Admitted batches waiting for an earlier one, by their `seq` (`None` for
    /// [`SequencedTable::mark_done`]).
    waiting: BTreeMap<u64, Option<TableData>>,
    /// The next `seq` due; `None` once `u64::MAX` was released.
    next: Option<u64>,
    /// The batches released so far.
    released: u64,
}

/// A [`SequencedTable`]'s place in its writer's ordering gate. Dropped before a successful
/// [`SequencedTable::close`] marked the lane closed, it poisons the writer with
/// [`StreamedTableError::NotClosed`].
struct LaneHandle {
    gate: Arc<CreditWindow>,
    lane: usize,
}

impl Drop for LaneHandle {
    fn drop(&mut self) {
        if self.gate.unclosed_lane(self.lane) {
            self.gate.poison(self.lane, StreamedTableError::NotClosed);
        }
    }
}

/// A table lane fed by many threads with numbered batches (ADR-0065 §7.2–§7.3, #812), opened
/// with [`StreamWriter::open_sequenced_table_on`]. Every call takes `&self`, so any number of
/// threads may push at once; the sealed bytes depend only on the `seq → batch` mapping, never on
/// which thread pushed what, when, or how many threads there were.
///
/// **Dense numbering.** The batches are `first_seq, first_seq + 1, …`, each pushed exactly once
/// with [`Self::push`], or with [`Self::mark_done`] for a `seq` with no rows. The rows inside a
/// batch are sealed in the order given.
///
/// **The gate.** A push of the next due `seq` (`next`) is never held. Any other push is admitted
/// while `seq < next + window` and the batches waiting ahead on all the writer's sequenced
/// inputs hold less than the byte credit, and waits otherwise until `next` advances (or is
/// refused at once in [`SequencedOptions::refuse_instead_of_block`] mode). So a lane holds at
/// most `window` batches ahead of a slow thread, and all lanes together less than the credit plus
/// the last batch admitted, plus each lane's due batch, which the credit never holds. The gate is
/// a second credit window of the writer, the type that bounds its encodes, with that rule keyed
/// to each lane's `next`.
///
/// **The byte credit** is the writer's [`crate::StreamWindow::max_in_flight_bytes`], the same
/// budget its encode window holds encoded payloads to: one value bounds two pools of equal size,
/// the batches waiting at this gate (their in-memory column bytes) and the encoded blocks waiting
/// for their commit. ADR-0065 §7.2 gives each pool half of a session's `ram_budget_bytes`, so a
/// caller holding that budget sets `max_in_flight_bytes = ram_budget_bytes / 2`. Both are soft
/// bounds: each pool can pass its half by what it admits last (a batch here, the encodes in
/// progress there) and by the batch or job it never holds back (each lane's due batch, the job
/// the committer waits on). An unequal split can't be expressed.
///
/// **Release.** The push that makes a contiguous run due releases it on its own thread, under
/// one lock per lane that orders the appends, into an [`OpenTable`] on the lane: the rows are
/// re-chunked to the row-group grid, so the leaves don't depend on how many rows each batch
/// held, and each full leaf becomes an ordinary staged job of the writer's encode workers. That
/// push can also wait for the encode window while it hands a leaf over (as
/// [`OpenTable::push`] does); that wait depends on the encoders alone.
///
/// **The claiming contract.** A blocking gate can't hang a producer whose threads claim
/// sequence numbers in increasing order (a shared counter, consecutive event blocks) and push
/// their own in increasing order, on every lane in the same order: the thread holding `next`
/// then has nothing smaller left to push, so it isn't waiting. A producer that can't promise
/// this uses refuse mode. A lost `seq` (a thread that died) is ended by [`Self::abort`] or by
/// the stall deadline.
///
/// **Failure.** Every refusal is terminal and **poisons the writer** (ADR-0065 §7.2: every
/// refusal poisons the session): it wakes every waiting push, every later call on any of the
/// writer's sequenced inputs and every later push on the writer (an [`OpenTable`]'s, a job's)
/// returns the same [`Error::StreamedTable`], naming the table where it happened, the writer's
/// pipeline is closed, and [`StreamWriter::finish`] returns it and seals nothing. The reasons
/// ([`StreamedTableError`]): `BeforeFirst` (a `seq` below `first_seq`), `Duplicate` (a `seq`
/// below `next` or already entered: a push reserves its `seq` on entry, so two pushes of one
/// `seq` are caught even while one waits; once `u64::MAX` was released, every `seq`),
/// `TooFarAhead`, `WouldBlock`, `Missing` and `CountMismatch` (at close), `Aborted`, `Stalled`,
/// `Failed` (releasing a batch into the table failed), and `NotClosed` (the input was dropped
/// before a successful [`Self::close`]; a still-open input fails `finish` with it too). A dead
/// pipeline wakes every waiter with the pipeline's own error. A batch whose column names, order
/// or lengths don't match the spec is refused before its `seq` is entered, and poisons nothing.
///
/// The poison lives in the writer, not on disk: the session directory still journals the leaves
/// committed before it, which [`crate::WriteSession::recover`] replays as it replays any
/// interrupted run (a prefix of whole leaves, not the run; ADR-0065 §5 keeps recovery out of the
/// bridge for that reason).
///
/// One sequenced input per lane, and nothing else pushed on its lane: other jobs on the lane
/// would interleave with its leaves by timing.
#[must_use = "a sequenced table is written only by SequencedTable::close"]
pub struct SequencedTable {
    name: String,
    /// The writer's ordering gate, and this input's lane in it.
    handle: LaneHandle,
    spec: TableSpec,
    release: Mutex<Release>,
}

impl StreamWriter {
    /// Open the table `name` on `lane` as a [`SequencedTable`] with `options`. `spec` is the
    /// leaf template, as for [`Self::open_table`] (`rows` 0, `partition` `None`), and the sealed
    /// bytes are those of an [`OpenTable`] fed the batches in sequence order.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for options outside the ranges of [`SequencedOptions`], or a lane that
    /// already had a sequenced input on this writer; the errors of [`Self::open_table`]; the
    /// writer's refusal once a sequenced input poisoned it.
    pub fn open_sequenced_table_on(
        &self,
        lane: Lane,
        name: impl Into<String>,
        spec: TableSpec,
        options: SequencedOptions,
    ) -> Result<SequencedTable> {
        let name = name.into();
        options.check(&name)?;
        let table =
            self.open_table_in(lane.clone(), name.clone(), spec.clone(), options.block_rows)?;
        let id = self.order.add_lane(lane, name.clone(), options)?;
        Ok(SequencedTable {
            name,
            handle: LaneHandle {
                gate: Arc::clone(&self.order),
                lane: id,
            },
            spec,
            release: Mutex::new(Release {
                table,
                waiting: BTreeMap::new(),
                next: Some(options.first_seq),
                released: 0,
            }),
        })
    }
}

impl SequencedTable {
    /// Enter batch `seq` with the spec's columns (any number of rows), on `writer`, the writer the
    /// table was opened on. Waits at the gate while `seq` isn't due and the window or the byte
    /// credit is full; releases the contiguous run it completes.
    ///
    /// # Errors
    ///
    /// The refusals of [`SequencedTable`] (each poisons the writer's sequenced inputs); the
    /// column errors of [`OpenTable::push`] (before `seq` is entered); [`Error::Invalid`] for
    /// another writer; the pipeline's errors.
    pub fn push(&self, writer: &StreamWriter, seq: u64, batch: TableData) -> Result<()> {
        self.check(writer)?;
        group_rows(&self.spec, &batch)?;
        let bytes = batch.iter().map(|(_, c)| column_bytes(c)).sum();
        self.enter(writer, seq, bytes, Some(batch))
    }

    /// Enter `seq` with no rows: the same as [`Self::push`] of an empty batch.
    ///
    /// # Errors
    ///
    /// As [`Self::push`].
    pub fn mark_done(&self, writer: &StreamWriter, seq: u64) -> Result<()> {
        self.check(writer)?;
        self.enter(writer, seq, 0, None)
    }

    /// Poison the writer with [`StreamedTableError::Aborted`], from any thread, waking every
    /// waiting push: the way out for a producer that lost a `seq`.
    pub fn abort(&self, reason: impl Into<String>) {
        let reason = reason.into();
        self.handle
            .gate
            .poison(self.handle.lane, StreamedTableError::Aborted { reason });
    }

    /// The gate's high-water marks for this input.
    pub fn stats(&self) -> SequencedStats {
        self.handle.gate.sequenced_stats(self.handle.lane)
    }

    /// End the input once every push has returned: check that no `seq` below the largest one
    /// pushed is missing and, with `expected_count`, that exactly that many were pushed, then
    /// close the table and return its leaves, as [`OpenTable::close`]. The count is an argument
    /// here, not an option, because a producer may learn it only at the end.
    ///
    /// # Errors
    ///
    /// [`StreamedTableError::Missing`] naming the first missing `seq`,
    /// [`StreamedTableError::CountMismatch`], an earlier refusal, and the errors of
    /// [`OpenTable::close`]; [`Error::Invalid`] for another writer. Every error poisons the
    /// writer, since the input is consumed unclosed (another writer's included:
    /// [`StreamedTableError::NotClosed`]).
    pub fn close(self, writer: &StreamWriter, expected_count: Option<u64>) -> Result<u64> {
        self.check(writer)?;
        let SequencedTable {
            handle, release, ..
        } = self;
        let (gate, lane) = (&handle.gate, handle.lane);
        gate.refusal().map_or(Ok(()), Err)?;
        let release = release
            .into_inner()
            .map_err(|_| fail(gate, lane, released_panic()))?;
        let refused = match release.next {
            Some(seq) if !release.waiting.is_empty() => Some(StreamedTableError::Missing { seq }),
            _ => expected_count
                .filter(|&expected| expected != release.released)
                .map(|expected| StreamedTableError::CountMismatch {
                    expected,
                    pushed: release.released,
                }),
        };
        if let Some(reason) = refused {
            return Err(gate.poison(lane, reason));
        }
        let leaves = release
            .table
            .close_in(writer)
            .map_err(|e| fail(gate, lane, e))?;
        gate.close_lane(lane);
        Ok(leaves)
    }

    fn check(&self, writer: &StreamWriter) -> Result<()> {
        if Arc::ptr_eq(&self.handle.gate, &writer.order) {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "sequenced table '{}' was opened on another StreamWriter",
                self.name
            )))
        }
    }

    fn fail(&self, e: Error) -> Error {
        fail(&self.handle.gate, self.handle.lane, e)
    }

    fn enter(
        &self,
        writer: &StreamWriter,
        seq: u64,
        bytes: u64,
        batch: Option<TableData>,
    ) -> Result<()> {
        let (gate, lane) = (&self.handle.gate, self.handle.lane);
        gate.admit(lane, seq, bytes)?;
        let mut release = match self.release.lock() {
            Ok(release) => release,
            Err(_) => return Err(self.fail(released_panic())),
        };
        release.waiting.insert(seq, batch);
        loop {
            let Some(next) = release.next else {
                return Ok(());
            };
            let Some(batch) = release.waiting.remove(&next) else {
                return Ok(());
            };
            release.next = next.checked_add(1);
            release.released += 1;
            gate.release(
                lane,
                Held {
                    ram: bytes_of(&batch),
                    staged: 0,
                },
            );
            if let Some(batch) = batch {
                push_rows(&mut release.table, writer, batch).map_err(|e| self.fail(e))?;
            }
        }
    }
}

/// Poison the writer with [`StreamedTableError::Failed`] on `lane` and return `e` itself.
fn fail(gate: &CreditWindow, lane: usize, e: Error) -> Error {
    let cause = e.to_string();
    gate.poison(lane, StreamedTableError::Failed { cause });
    e
}

fn released_panic() -> Error {
    Error::Container("a push panicked while releasing batches".into())
}

fn bytes_of(batch: &Option<TableData>) -> u64 {
    batch.iter().flatten().map(|(_, c)| column_bytes(c)).sum()
}

/// Append `batch` to `table` in row-groups of at most [`ROWS_PER_GROUP`] rows.
fn push_rows(table: &mut OpenTable, writer: &StreamWriter, batch: TableData) -> Result<()> {
    let rows = rows_of(&batch);
    if rows <= ROWS_PER_GROUP {
        return table.push_in(writer, batch);
    }
    for start in (0..rows).step_by(ROWS_PER_GROUP) {
        let end = (start + ROWS_PER_GROUP).min(rows);
        let group = batch
            .iter()
            .map(|(n, c)| (n.clone(), c.slice(start, end)))
            .collect();
        table.push_in(writer, group)?;
    }
    Ok(())
}

/// A column's in-memory value bytes, what a waiting batch charges to the byte credit.
fn column_bytes(c: &ColumnData) -> u64 {
    use std::mem::size_of_val as bytes;
    let n = match c {
        ColumnData::I8(v) => bytes(v.as_slice()),
        ColumnData::I16(v) => bytes(v.as_slice()),
        ColumnData::I32(v) => bytes(v.as_slice()),
        ColumnData::I64(v) => bytes(v.as_slice()),
        ColumnData::U8(v) => bytes(v.as_slice()),
        ColumnData::U16(v) => bytes(v.as_slice()),
        ColumnData::U32(v) => bytes(v.as_slice()),
        ColumnData::U64(v) => bytes(v.as_slice()),
        ColumnData::F32(v) => bytes(v.as_slice()),
        ColumnData::F64(v) => bytes(v.as_slice()),
        ColumnData::Bool(v) => bytes(v.as_slice()),
        ColumnData::Utf8(v) => v.iter().map(String::len).sum(),
        ColumnData::Nullable { values, validity } => {
            return column_bytes(values) + validity.len() as u64
        }
    };
    n as u64
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::tests::{canonical, listmode, RPG, TS};
    use crate::stream::Job;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    /// Run `f` on its own thread and fail if it hasn't returned within a minute. A hang DETECTOR
    /// only: every path under test ends by a wake-up, never by this timeout.
    fn no_hang<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let out = f();
            let _ = tx.send(());
            out
        });
        if let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(Duration::from_secs(60)) {
            panic!("hung: no wake-up within 60 s");
        }
        handle
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p))
    }

    fn writer(dir: &Path, window: StreamWindow) -> StreamWriter {
        let ws = WriteSession::create(&dir.join("stage"), "recon", "p", "d", TS).unwrap();
        StreamWriter::with_window(ws, 2, window)
    }

    fn options(window: usize) -> SequencedOptions {
        SequencedOptions {
            window,
            ..SequencedOptions::default()
        }
    }

    fn open(sw: &StreamWriter, options: SequencedOptions) -> SequencedTable {
        sw.open_sequenced_table_on(Lane::table("events"), "events", listmode::spec(0), options)
            .unwrap()
    }

    fn reason(e: Error) -> StreamedTableError {
        match e {
            Error::StreamedTable { reason, .. } => reason,
            e => panic!("not a StreamedTable error: {e}"),
        }
    }

    /// Wait until `n` pushes of `t` have blocked at the gate (a push of the test's own making, so
    /// this ends as soon as that thread reaches the wait).
    fn until_blocked(t: &SequencedTable, n: u64) {
        while t.stats().blocked_pushes < n {
            thread::yield_now();
        }
    }

    /// With a window of 1 and `next == 0`, a push of `1` blocks; return the error it wakes with
    /// once `wake` has run on the main thread.
    fn blocked_push_wakes_with(
        options: SequencedOptions,
        wake: impl FnOnce(&StreamWriter, &SequencedTable) + Send + 'static,
    ) -> Error {
        no_hang(move || {
            let dir = tempfile::tempdir().unwrap();
            let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
            let t = open(&sw, options);
            thread::scope(|s| {
                let blocked = s.spawn(|| t.push(&sw, 1, listmode::rows(0, 3)));
                until_blocked(&t, 1);
                wake(&sw, &t);
                blocked.join().unwrap().unwrap_err()
            })
        })
    }

    /// Batch `seq` of the smoke run: `SIZES[seq]` rows from the prefix sum, empty every 7th.
    fn batch_rows(batches: u64) -> Vec<(u64, usize)> {
        let mut start = 0;
        (0..batches)
            .map(|seq| {
                let n = if seq % 7 == 3 {
                    0
                } else {
                    1 + (seq * 7919 % 3000) as usize
                };
                start += n as u64;
                (start - n as u64, n)
            })
            .collect()
    }

    /// Shuffled pushes from `threads` threads under the claiming contract (a shared counter, each
    /// thread pushing its claims in order, after a seeded per-seq delay so arrivals interleave),
    /// through a window of 4 that blocks: the product is sealed from the in-order rows.
    fn sequenced_product(dir: &Path, tag: &str, threads: usize) -> (Manifest, SequencedStats) {
        let batches = batch_rows(300);
        let sw = writer(&dir.join(tag), StreamWindow::new(8, u64::MAX));
        let t = sw
            .open_sequenced_table_on(
                Lane::table("events"),
                "events",
                listmode::spec(0),
                SequencedOptions {
                    block_rows: RPG,
                    ..options(4)
                },
            )
            .unwrap();
        let claim = AtomicU64::new(0);
        thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| loop {
                    let seq = claim.fetch_add(1, Ordering::SeqCst);
                    let Some(&(start, n)) = batches.get(seq as usize) else {
                        break;
                    };
                    thread::sleep(Duration::from_micros(seq.wrapping_mul(2654435761) % 300));
                    if n == 0 {
                        t.mark_done(&sw, seq).unwrap();
                    } else {
                        t.push(&sw, seq, listmode::rows(start, n)).unwrap();
                    }
                });
            }
        });
        let stats = t.stats();
        let total: u64 = batches.iter().map(|&(_, n)| n as u64).sum();
        let leaves = t.close(&sw, Some(300)).unwrap();
        assert_eq!(leaves, crate::partition_blocks(total, RPG), "{tag}: leaves");
        assert!(leaves > 2, "{tag}: a multi-leaf table");
        let m = sw.finish(&dir.join(format!("{tag}.tsra"))).unwrap();
        (m, stats)
    }

    #[test]
    fn shuffled_pushes_from_four_threads_seal_the_in_order_bytes() {
        no_hang(|| {
            let dir = tempfile::tempdir().unwrap();
            let total: u64 = batch_rows(300).iter().map(|&(_, n)| n as u64).sum();
            // The reference: the same rows pushed in order on one thread into an open table.
            let ws = WriteSession::create(&dir.path().join("ref"), "recon", "p", "d", TS).unwrap();
            let mut sw = StreamWriter::new(ws, 2, 4);
            let mut table = sw
                .open_table_with_block_rows("events", listmode::spec(0), RPG)
                .unwrap();
            for start in (0..total).step_by(9999) {
                let n = (total - start).min(9999) as usize;
                table.push(&mut sw, listmode::rows(start, n)).unwrap();
            }
            table.close(&mut sw).unwrap();
            let reference = sw.finish(&dir.path().join("ref.tsra")).unwrap();
            for threads in [1, 4] {
                let tag = format!("t{threads}");
                let (m, stats) = sequenced_product(dir.path(), &tag, threads);
                assert_eq!(canonical(&m), canonical(&reference), "{tag}");
                assert_eq!(m.content_hash, reference.content_hash, "{tag}");
                assert!(stats.peak_admitted <= 4, "{tag}: {stats:?}");
            }
        });
    }

    #[test]
    fn a_duplicate_is_refused_and_wakes_the_blocked_push_of_the_same_seq() {
        // Two pushes of one seq: the first blocks with its seq reserved, so the second is caught
        // on entry, refused, and wakes the first.
        let err = blocked_push_wakes_with(options(1), |sw, t| {
            let dup = reason(t.push(sw, 1, listmode::rows(0, 3)).unwrap_err());
            assert_eq!(dup, StreamedTableError::Duplicate { seq: 1 });
        });
        assert_eq!(reason(err), StreamedTableError::Duplicate { seq: 1 });
        // A seq below `next` is a duplicate too, and the refusal is terminal.
        let dir = tempfile::tempdir().unwrap();
        let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
        let t = open(&sw, options(4));
        t.mark_done(&sw, 0).unwrap();
        let dup = reason(t.push(&sw, 0, listmode::rows(0, 1)).unwrap_err());
        assert_eq!(dup, StreamedTableError::Duplicate { seq: 0 });
        assert_eq!(reason(t.mark_done(&sw, 1).unwrap_err()), dup, "poisoned");
    }

    #[test]
    fn a_seq_past_the_horizon_is_refused_and_wakes_the_blocked_push() {
        let opts = SequencedOptions {
            max_ahead: Some(4),
            ..options(1)
        };
        let far = StreamedTableError::TooFarAhead {
            seq: 4,
            next: 0,
            max_ahead: 4,
        };
        let expected = far.clone();
        let err = blocked_push_wakes_with(opts, move |sw, t| {
            assert_eq!(reason(t.mark_done(sw, 4).unwrap_err()), expected);
        });
        assert_eq!(reason(err), far);
    }

    #[test]
    fn a_gap_or_a_wrong_count_at_close_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
        let t = open(&sw, options(4));
        t.push(&sw, 0, listmode::rows(0, 2)).unwrap();
        t.push(&sw, 2, listmode::rows(3, 2)).unwrap();
        let gap = reason(t.close(&sw, None).unwrap_err());
        assert_eq!(gap, StreamedTableError::Missing { seq: 1 });

        let dir = tempfile::tempdir().unwrap();
        let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
        let t = open(
            &sw,
            SequencedOptions {
                first_seq: 10,
                ..options(4)
            },
        );
        t.push(&sw, 10, listmode::rows(0, 2)).unwrap();
        t.mark_done(&sw, 11).unwrap();
        let count = reason(t.close(&sw, Some(3)).unwrap_err());
        assert_eq!(
            count,
            StreamedTableError::CountMismatch {
                expected: 3,
                pushed: 2
            }
        );
    }

    #[test]
    fn abort_wakes_the_blocked_push() {
        let err = blocked_push_wakes_with(options(1), |_, t| t.abort("worker lost seq 0"));
        assert_eq!(
            reason(err),
            StreamedTableError::Aborted {
                reason: "worker lost seq 0".into()
            }
        );
    }

    #[test]
    fn a_stall_past_the_deadline_wakes_the_blocked_push() {
        // Nothing pushes seq 0: the deadline, not the hang detector, ends the wait.
        let opts = SequencedOptions {
            stall_timeout: Duration::from_millis(200),
            ..options(1)
        };
        let err = blocked_push_wakes_with(opts, |_, _| {});
        assert_eq!(reason(err), StreamedTableError::Stalled { next: 0 });
    }

    #[test]
    fn a_dead_pipeline_wakes_the_blocked_push() {
        let err = blocked_push_wakes_with(options(1), |sw, _| {
            sw.submit(
                &Lane::Arrays,
                Job::InMemory(Box::new(|| Err(Error::Codec("boom".into())))),
                false,
            )
            .unwrap();
        });
        assert!(err.to_string().contains("pipeline closed"), "{err}");
    }

    #[test]
    fn refuse_mode_refuses_what_would_block() {
        let dir = tempfile::tempdir().unwrap();
        let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
        let t = open(
            &sw,
            SequencedOptions {
                refuse_instead_of_block: true,
                ..options(1)
            },
        );
        let would = reason(t.push(&sw, 1, listmode::rows(0, 1)).unwrap_err());
        assert_eq!(would, StreamedTableError::WouldBlock { seq: 1, next: 0 });
        assert_eq!(reason(t.mark_done(&sw, 0).unwrap_err()), would, "terminal");
    }

    #[test]
    fn a_held_byte_credit_blocks_until_the_due_push_releases_it() {
        no_hang(|| {
            let dir = tempfile::tempdir().unwrap();
            // A byte credit of 1: one batch waiting ahead holds it.
            let sw = writer(dir.path(), StreamWindow::new(8, 1));
            let t = open(&sw, options(8));
            t.push(&sw, 1, listmode::rows(1, 1)).unwrap();
            thread::scope(|s| {
                let blocked = s.spawn(|| t.push(&sw, 2, listmode::rows(2, 1)));
                until_blocked(&t, 1);
                // The due push is never held by the credit, and its release frees it.
                t.push(&sw, 0, listmode::rows(0, 1)).unwrap();
                blocked.join().unwrap().unwrap();
            });
            assert_eq!(t.close(&sw, Some(3)).unwrap(), 1);
        });
    }

    /// The refusal `finish` returned, and that it wrote no product.
    fn finish_refuses(sw: StreamWriter, dir: &Path) -> StreamedTableError {
        let out = dir.join("out.tsra");
        let err = reason(sw.finish(&out).unwrap_err());
        assert!(!out.exists(), "nothing sealed");
        err
    }

    #[test]
    fn a_refusal_poisons_the_writer_so_finish_seals_nothing() {
        // Seq 0 never fills a leaf, so its rows exist only in the table's staging, which the
        // failed close abandons: a finish that ignored the refusal would seal no events table.
        let dir = tempfile::tempdir().unwrap();
        let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
        let t = open(&sw, options(4));
        t.push(&sw, 0, listmode::rows(0, 3)).unwrap();
        t.abort("lost seq 1");
        let aborted = StreamedTableError::Aborted {
            reason: "lost seq 1".into(),
        };
        assert_eq!(reason(t.close(&sw, None).unwrap_err()), aborted);
        // The writer is poisoned, not just the input: its other lanes refuse too.
        let job = Job::InMemory(Box::new(|| unreachable!("never admitted")));
        let other = sw.submit(&Lane::Arrays, job, false).unwrap_err();
        assert_eq!(reason(other), aborted);
        assert_eq!(finish_refuses(sw, dir.path()), aborted);
    }

    #[test]
    fn a_sequenced_table_not_closed_at_finish_fails_it() {
        // Dropped without close: the drop poisons the writer.
        let dir = tempfile::tempdir().unwrap();
        let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
        let t = open(&sw, options(4));
        t.push(&sw, 0, listmode::rows(0, 3)).unwrap();
        drop(t);
        assert_eq!(
            reason(
                sw.open_sequenced_table_on(
                    Lane::table("hits"),
                    "hits",
                    listmode::spec(0),
                    options(4)
                )
                .err()
                .unwrap()
            ),
            StreamedTableError::NotClosed,
            "poisoned"
        );
        assert_eq!(
            finish_refuses(sw, dir.path()),
            StreamedTableError::NotClosed
        );
        // Still open: `finish` checks every lane it opened.
        let dir = tempfile::tempdir().unwrap();
        let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
        let t = open(&sw, options(4));
        t.push(&sw, 0, listmode::rows(0, 3)).unwrap();
        assert_eq!(
            finish_refuses(sw, dir.path()),
            StreamedTableError::NotClosed
        );
        drop(t);
    }

    #[test]
    fn a_seq_below_first_seq_is_refused_as_before_first() {
        let dir = tempfile::tempdir().unwrap();
        let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
        let t = open(
            &sw,
            SequencedOptions {
                first_seq: 10,
                ..options(4)
            },
        );
        let before = reason(t.mark_done(&sw, 9).unwrap_err());
        assert_eq!(
            before,
            StreamedTableError::BeforeFirst {
                seq: 9,
                first_seq: 10
            }
        );
        assert_eq!(
            reason(t.mark_done(&sw, 10).unwrap_err()),
            before,
            "poisoned"
        );
    }

    #[test]
    fn the_last_sequence_numbers_release_without_overflow() {
        // `u64::MAX - 1` and `u64::MAX`, out of order: both seal, and `next` runs out.
        let dir = tempfile::tempdir().unwrap();
        let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
        let opts = SequencedOptions {
            first_seq: u64::MAX - 1,
            ..options(4)
        };
        let t = open(&sw, opts);
        t.push(&sw, u64::MAX, listmode::rows(2, 3)).unwrap();
        t.push(&sw, u64::MAX - 1, listmode::rows(0, 2)).unwrap();
        assert_eq!(t.close(&sw, Some(2)).unwrap(), 1);
        let m = sw.finish(&dir.path().join("out.tsra")).unwrap();
        // The same rows in order through an open table on the lane.
        let ws = WriteSession::create(&dir.path().join("ref"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 2, 4);
        let mut table = sw
            .open_table_on(Lane::table("events"), "events", listmode::spec(0))
            .unwrap();
        table.push(&mut sw, listmode::rows(0, 5)).unwrap();
        table.close(&mut sw).unwrap();
        let reference = sw.finish(&dir.path().join("ref.tsra")).unwrap();
        assert_eq!(m.content_hash, reference.content_hash);
        // Past the last number every seq was pushed: a duplicate, not an overflow.
        let dir = tempfile::tempdir().unwrap();
        let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
        let t = open(&sw, opts);
        t.mark_done(&sw, u64::MAX - 1).unwrap();
        t.mark_done(&sw, u64::MAX).unwrap();
        let dup = reason(t.mark_done(&sw, u64::MAX).unwrap_err());
        assert_eq!(dup, StreamedTableError::Duplicate { seq: u64::MAX });
    }

    #[test]
    fn every_advance_of_next_restarts_the_stall_deadline() {
        // A push of 4 waits (window 1) for 1.35 deadlines in all, while `next` advances every
        // 0.45 (each sleep has 1.1 s of slack): a deadline counted from the push's entry would
        // fire between the advances.
        const STALL: Duration = Duration::from_millis(2000);
        let opts = SequencedOptions {
            stall_timeout: STALL,
            ..options(1)
        };
        no_hang(move || {
            let dir = tempfile::tempdir().unwrap();
            let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
            let t = open(&sw, opts);
            thread::scope(|s| {
                let blocked = s.spawn(|| t.mark_done(&sw, 4));
                until_blocked(&t, 1);
                let entered = std::time::Instant::now();
                for seq in 0..4 {
                    if seq < 3 {
                        thread::sleep(STALL * 9 / 20);
                    }
                    t.mark_done(&sw, seq).unwrap();
                }
                blocked.join().unwrap().unwrap();
                assert!(entered.elapsed() > STALL, "waited past one deadline");
            });
            assert_eq!(t.close(&sw, Some(5)).unwrap(), 1);
        });
    }

    #[test]
    fn a_failed_release_poisons_the_writer_as_failed() {
        // The table's staging directory is gone: releasing seq 0, a whole row-group, can't stage
        // its rows.
        let dir = tempfile::tempdir().unwrap();
        let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
        let t = open(&sw, options(4));
        std::fs::remove_dir_all(dir.path().join("stage").join("staging")).unwrap();
        let io = t.push(&sw, 0, listmode::rows(0, RPG as usize)).unwrap_err();
        assert!(matches!(io, Error::Io(_)), "the release's own error: {io}");
        let failed = StreamedTableError::Failed {
            cause: io.to_string(),
        };
        assert_eq!(reason(t.mark_done(&sw, 1).unwrap_err()), failed);
        assert_eq!(reason(t.close(&sw, Some(2)).unwrap_err()), failed);
        assert_eq!(finish_refuses(sw, dir.path()), failed);
    }

    #[test]
    fn open_refuses_bad_options_and_a_second_input_on_one_lane() {
        let dir = tempfile::tempdir().unwrap();
        let sw = writer(dir.path(), StreamWindow::new(8, u64::MAX));
        let bad = [
            options(0),
            SequencedOptions {
                max_ahead: Some(3),
                ..options(4)
            },
            SequencedOptions {
                stall_timeout: Duration::ZERO,
                ..options(4)
            },
            SequencedOptions {
                stall_timeout: MAX_STALL_TIMEOUT + Duration::from_millis(1),
                ..options(4)
            },
        ];
        for o in bad {
            let r = sw.open_sequenced_table_on(Lane::table("e"), "e", listmode::spec(0), o);
            assert!(matches!(r, Err(Error::Invalid(_))), "{o:?}");
        }
        let _first = open(&sw, options(4));
        let second = sw.open_sequenced_table_on(
            Lane::table("events"),
            "events2",
            listmode::spec(0),
            options(4),
        );
        assert!(matches!(second, Err(Error::Invalid(_))));
    }
}
