//! Bounded-memory streaming write pipeline (ROADMAP P3 / #203) — the DAQ-facing front of
//! [`WriteSession`].
//!
//! The DAQ rule (S17): never encode on the producer's hot path. [`StreamWriter`] decouples the
//! producer from encoding with a bounded pipeline:
//!
//! ```text
//!   push(job) ──▶ job queue ──▶ N encode workers ──▶ ordered committer ──▶ WriteSession
//!   (producer)                  (parallel encode)    (reorder by seq,      (durable frag +
//!                                                     append in push order) journal + Merkle)
//!      └──────────── one credit window over the whole span (#677) ────────────┘
//! ```
//!
//! - **Bounded RAM (the credit window, #677 / ADR-0026 "StreamWriter memory contract"):** a job takes
//!   a credit when it is **pushed** and gives it back only when it is **committed in order**, so one
//!   [`StreamWindow`] bounds every job the pipeline holds — queued, encoding, *and* finished but
//!   waiting in the reorder buffer for an earlier job. `push` blocks while the window is full. The
//!   bound, over [`StreamWindow::max_in_flight`] = `J`, [`StreamWindow::max_in_flight_bytes`] = `B`,
//!   the worker count `W` and the largest single job's encoded payload `P` (data block plus its
//!   companions):
//!   - jobs pushed but not yet committed `≤ J`, always;
//!   - encoded payload bytes held in RAM `< B + min(W, J) × P` (and trivially `≤ J × P`).
//! - **Parallel encode:** `workers` threads encode concurrently (the expensive pcodec/Vortex step).
//! - **Deterministic output:** the committer commits blocks in **push order** (a reorder buffer holds
//!   out-of-order completions), so the running Merkle — and therefore the sealed `content_hash` /
//!   `id` / `manifest_hash` — is **byte-identical to a batch writer** that added the same blocks in
//!   the same order. Streaming changes *how* the bytes are produced, never *which* bytes; neither
//!   the worker count nor the window changes them.
//!
//! The encoded payload of ONE job is still materialised in RAM (`P` above): a single table block
//! larger than RAM needs the file-backed encode sink, the second half of #677 (out of scope here).
//!
//! # How the window is enforced
//!
//! Two gates, both on one mutex + condvar:
//! 1. **Submit** ([`StreamWriter::push`]): a job is admitted while fewer than `J` jobs are in flight
//!    *and* the encoded bytes held are below `B`. With nothing in flight the bytes held are `0`, so a
//!    job is always admitted into an empty window — including one whose payload alone exceeds `B`.
//! 2. **Encode start** (a worker, after dequeuing job `seq`): it starts encoding only while the
//!    encoded bytes held are below `B` — **unless `seq` is the job the committer waits on**, which
//!    always runs. This is what makes `B` bind: once `B` is reached, only the in-progress encodes
//!    (at most `min(W, J)`) can still add bytes.
//!
//! **Deadlock freedom.** Credits are released only by an in-order commit, so the window must never
//! stall the job the committer waits on (`next`). It can't: `next` was pushed (every job below the
//! newest push was), the job queue is FIFO so `next` is dequeued before any later job, and gate 2
//! exempts it. So `next` always encodes, commits, and frees its credit; a job too large for `B` simply
//! runs alone once it is `next`. Gate 1 never waits on an empty window. A failed encode, a failed
//! commit or a panicking worker **closes** the window, which wakes every waiter: `push` then returns
//! an error and the workers waiting at gate 2 exit, so a dead pipeline never hangs the producer
//! (idle workers exit once [`StreamWriter::finish`] or a drop closes the job queue).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use tessera_core::block::BlockRef;
use tessera_core::manifest::Manifest;
use tessera_core::{Error, Result};

use crate::write::WriteSession;
use crate::BlockPayload;

/// A committed block: its manifest reference paired with its payload bytes.
type EncodedBlock = (BlockRef, BlockPayload);

/// A unit of work the pipeline encodes off the hot path. The producer hands the *raw* data captured
/// in a closure; the encode (pcodec/Vortex) runs on a worker thread. Returns the data block plus any
/// **additive companion blocks** (e.g. an ADR-0028 §5 chunk-index sidecar) to commit right after it, in
/// order. Use [`array_job`]/[`table_job`] (no companions) or the `*_indexed` variants (with a sidecar).
pub type EncodeJob =
    Box<dyn FnOnce() -> Result<(EncodedBlock, Vec<EncodedBlock>)> + Send + 'static>;

/// Wrap an array block as an [`EncodeJob`] (encoding deferred to a worker). No companion blocks — the
/// streamed bytes stay byte-identical to a batch write.
pub fn array_job(
    name: impl Into<String>,
    spec: tessera_core::block::array::ArraySpec,
    data: crate::array::ArrayData,
) -> EncodeJob {
    let name = name.into();
    Box::new(move || Ok((crate::array::array_block(&name, &spec, &data)?, Vec::new())))
}

/// Like [`array_job`] but also emits the block's `{hash, stats}` chunk-index **sidecar** (ADR-0028 §5)
/// as an additive companion, committed right after the data block. Opt-in: only `*_indexed` jobs add a
/// sidecar, so plain streaming stays byte-identical to batch. Float arrays emit no sidecar.
pub fn array_job_indexed(
    name: impl Into<String>,
    spec: tessera_core::block::array::ArraySpec,
    data: crate::array::ArrayData,
) -> EncodeJob {
    let name = name.into();
    Box::new(move || {
        let (block, sidecar) = crate::array::array_block_with_index(&name, &spec, &data)?;
        Ok((block, sidecar.into_iter().collect()))
    })
}

/// Wrap a table block as an [`EncodeJob`] (no companions).
pub fn table_job(
    name: impl Into<String>,
    spec: tessera_core::block::table::TableSpec,
    data: crate::table::TableData,
) -> EncodeJob {
    let name = name.into();
    Box::new(move || Ok((crate::table::table_block(&name, &spec, &data)?, Vec::new())))
}

/// Like [`table_job`] but also emits the table's chunk-index sidecar over `stat_column` (ADR-0028 §5),
/// when that names an integer column. Opt-in, additive companion.
pub fn table_job_indexed(
    name: impl Into<String>,
    spec: tessera_core::block::table::TableSpec,
    data: crate::table::TableData,
    stat_column: Option<String>,
) -> EncodeJob {
    let name = name.into();
    Box::new(move || {
        let (block, sidecar) =
            crate::table::table_block_with_index(&name, &spec, &data, stat_column.as_deref())?;
        Ok((block, sidecar.into_iter().collect()))
    })
}

/// Wrap a table block as an [`EncodeJob`] that **lazily reads fragments from disk** — the
/// multi-block listmode peer of [`table_job`]. The job closure pulls one row-group fragment at a
/// time via [`crate::accumulate::read_fragment`] and feeds [`crate::table::encode_streaming`], so the
/// per-block encode peak ≈ one row-group's RAM (plus Vortex working set), never the whole block. This
/// is the key piece that lets the multi-block sink dispatch one parallel encode job per block to
/// [`StreamWriter`] without ever holding a full block payload off-disk.
///
/// Inputs:
/// - `name`: the manifest block name (already chosen via [`crate::table::block_name`]).
/// - `spec`: the per-block [`TableSpec`] — its `rows` MUST equal the sum of the fragments' rows.
/// - `frag_paths`: row-group fragment file paths in append order (the order [`encode_streaming`]
///   consumes them).
pub fn table_job_from_fragments(
    name: impl Into<String>,
    spec: tessera_core::block::table::TableSpec,
    frag_paths: Vec<std::path::PathBuf>,
) -> EncodeJob {
    let name = name.into();
    Box::new(move || {
        // The lazy iterator pulled by encode_streaming: one fragment at a time, decoded just before
        // Vortex consumes it. A read error must surface as an Err, but the iterator side is infallible
        // (TableData) — stash the first error and feed empties after it, then return Err if set.
        let columns = spec.columns.clone();
        let err: std::sync::Arc<std::sync::Mutex<Option<tessera_core::Error>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let slot = std::sync::Arc::clone(&err);
        let empty_for = |columns: &[tessera_core::block::table::Column]| {
            columns
                .iter()
                .map(|c| {
                    (
                        c.name.clone(),
                        crate::table::ColumnData::empty_for(c).expect("validated dtype"),
                    )
                })
                .collect::<crate::table::TableData>()
        };
        let iter = frag_paths.into_iter().map(move |p| {
            if slot.lock().expect("err mutex").is_some() {
                return empty_for(&columns);
            }
            match crate::accumulate::read_fragment(&p, &columns) {
                Ok(td) => td,
                Err(e) => {
                    *slot.lock().expect("err mutex") = Some(e);
                    empty_for(&columns)
                }
            }
        });
        let bytes = crate::table::encode_streaming(&spec, iter)?;
        if let Some(e) = err.lock().expect("err mutex").take() {
            return Err(e);
        }
        let digest = tessera_core::hash::digest(&bytes);
        let block_ref = tessera_core::block::BlockRef {
            name: name.clone(),
            kind: tessera_core::block::BlockKind::Table,
            digest: Some(digest),
            spec: serde_json::to_value(&spec).map_err(tessera_core::Error::from)?,
        };
        Ok((
            (block_ref, crate::BlockPayload::new(name.clone(), bytes)),
            Vec::new(),
        ))
    })
}

/// The encoded result of one job: the data block plus its companions.
type JobOutput = (EncodedBlock, Vec<EncodedBlock>);

/// What a worker hands the committer: the job's `seq`, the encoded payload bytes it charged to the
/// window (so the commit can give exactly that back), and the encode result.
type Encoded = (usize, u64, Result<JobOutput>);

/// The credit window bounding everything a [`StreamWriter`] holds between [`StreamWriter::push`]
/// and the in-order commit of that job (#677). See the [module docs](self) for the bound it gives
/// and how it is enforced.
///
/// Build it with [`StreamWindow::new`]; the fields are public to read. Both limits are clamped to
/// at least 1, so a window can always admit a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct StreamWindow {
    /// The most jobs pushed but not yet committed (queued + encoding + waiting in the reorder
    /// buffer). [`StreamWriter::push`] blocks while this many are in flight. Always ≥ 1.
    pub max_in_flight: usize,
    /// The encoded-payload byte budget (data blocks + companions) the window aims to hold in RAM.
    /// While the bytes of encoded-but-uncommitted jobs are at or above it, `push` blocks and
    /// workers start no new encode other than the one the committer waits on. It is a soft
    /// ceiling: the in-progress encodes can overshoot it by at most `min(workers, max_in_flight)`
    /// payloads, and a single job larger than the budget still runs (alone). Always ≥ 1.
    pub max_in_flight_bytes: u64,
}

impl StreamWindow {
    /// A window of `max_in_flight` jobs and `max_in_flight_bytes` encoded bytes, each clamped to ≥ 1.
    pub fn new(max_in_flight: usize, max_in_flight_bytes: u64) -> Self {
        StreamWindow {
            max_in_flight: max_in_flight.max(1),
            max_in_flight_bytes: max_in_flight_bytes.max(1),
        }
    }
}

/// High-water marks and counters of a [`StreamWriter`]'s credit window, so a caller can assert the
/// memory contract (#677). Read it live with [`StreamWriter::stats`] or as the final account from
/// [`StreamWriter::finish_with_stats`].
///
/// Against the writer's [`StreamWindow`] (`J` jobs, `B` bytes), `W` workers and the largest single
/// job's encoded payload `P`: `peak_in_flight_jobs ≤ J` and `peak_in_flight_bytes < B + min(W, J) × P`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct StreamStats {
    /// The most jobs that were pushed but not yet committed at any one time.
    pub peak_in_flight_jobs: usize,
    /// The most encoded payload bytes (data blocks + companions) held at any one time by jobs that
    /// had finished encoding but were not yet committed.
    pub peak_in_flight_bytes: u64,
    /// The largest encoded payload of a single job (data block + companions) — `P` in the bound.
    pub max_job_bytes: u64,
    /// Jobs pushed so far.
    pub jobs_pushed: usize,
    /// Jobs committed to the session so far, in push order.
    pub jobs_committed: usize,
    /// How many [`StreamWriter::push`] calls had to wait for a credit (backpressure events).
    pub producer_stalls: u64,
}

/// The window's mutable state, behind [`CreditWindow::state`].
struct WindowState {
    /// Jobs pushed and not yet committed.
    in_flight_jobs: usize,
    /// Encoded payload bytes of jobs that finished encoding and are not yet committed.
    in_flight_bytes: u64,
    /// The `seq` the committer commits next — the job the window must never stall.
    next_commit: usize,
    /// Set when the pipeline can no longer make progress (encode/commit error, worker panic,
    /// committer exit); every waiter gives up.
    closed: bool,
    /// Workers currently waiting at gate 2 (tests observe it; it is not part of [`StreamStats`]).
    gated_encodes: usize,
    stats: StreamStats,
}

/// The shared credit window: the limits, the state, and one condvar every gate waits on.
struct CreditWindow {
    limits: StreamWindow,
    state: Mutex<WindowState>,
    changed: Condvar,
}

impl CreditWindow {
    fn new(limits: StreamWindow) -> Self {
        CreditWindow {
            limits,
            state: Mutex::new(WindowState {
                in_flight_jobs: 0,
                in_flight_bytes: 0,
                next_commit: 0,
                closed: false,
                gated_encodes: 0,
                stats: StreamStats::default(),
            }),
            changed: Condvar::new(),
        }
    }

    /// The state is plain counters updated under the lock without any panicking code, so a
    /// poisoned lock (a panic elsewhere on that thread) still holds consistent state — take it,
    /// so [`Self::close`] works while a worker unwinds.
    fn lock(&self) -> MutexGuard<'_, WindowState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(&self, guard: MutexGuard<'a, WindowState>) -> MutexGuard<'a, WindowState> {
        self.changed
            .wait(guard)
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Gate 1 (producer): wait for a job credit, then take it. `Err` once the window is closed.
    fn acquire(&self) -> Result<()> {
        let mut st = self.lock();
        let mut stalled = false;
        loop {
            if st.closed {
                return Err(closed_error());
            }
            // With no job in flight no bytes are held, so an empty window always admits.
            if st.in_flight_jobs < self.limits.max_in_flight
                && st.in_flight_bytes < self.limits.max_in_flight_bytes
            {
                break;
            }
            if !stalled {
                stalled = true;
                st.stats.producer_stalls += 1;
            }
            st = self.wait(st);
        }
        st.in_flight_jobs += 1;
        st.stats.jobs_pushed += 1;
        st.stats.peak_in_flight_jobs = st.stats.peak_in_flight_jobs.max(st.in_flight_jobs);
        Ok(())
    }

    /// Gate 2 (worker): wait until job `seq` may start encoding — always if it is the job the
    /// committer waits on, otherwise while the held bytes are below the budget. `false` once the
    /// window is closed (the worker should exit).
    fn wait_encode_turn(&self, seq: usize) -> bool {
        let mut st = self.lock();
        let mut gated = false;
        let turn = loop {
            if st.closed {
                break false;
            }
            if seq == st.next_commit || st.in_flight_bytes < self.limits.max_in_flight_bytes {
                break true;
            }
            if !gated {
                gated = true;
                st.gated_encodes += 1;
            }
            st = self.wait(st);
        };
        if gated {
            st.gated_encodes -= 1;
        }
        turn
    }

    /// A worker finished encoding a job whose payload is `bytes`: hold them until its commit.
    fn charge(&self, bytes: u64) {
        let mut st = self.lock();
        st.in_flight_bytes += bytes;
        st.stats.peak_in_flight_bytes = st.stats.peak_in_flight_bytes.max(st.in_flight_bytes);
        st.stats.max_job_bytes = st.stats.max_job_bytes.max(bytes);
    }

    /// The committer appended job `next_commit` (whose payload was `bytes`): give its credit back.
    fn release(&self, bytes: u64) {
        let mut st = self.lock();
        st.in_flight_jobs -= 1;
        st.in_flight_bytes -= bytes;
        st.next_commit += 1;
        st.stats.jobs_committed += 1;
        drop(st);
        self.changed.notify_all();
    }

    /// The pipeline can make no more progress: wake every waiter so none hangs.
    fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }

    fn stats(&self) -> StreamStats {
        self.lock().stats
    }
}

fn closed_error() -> Error {
    Error::Container("stream pipeline closed (a worker/committer died)".into())
}

/// Closes the window when dropped — always (the committer: once it returns, by error or not, no
/// credit is released again) or only while unwinding (a worker: a panic loses the result of the
/// job it holds, whose credit then never returns). Either way no producer or worker is left
/// waiting on a pipeline that can't progress. A worker's *normal* exit must not close: it means
/// the input drained, and a sibling may still hold a job waiting at the encode gate.
struct CloseOnDrop {
    window: Arc<CreditWindow>,
    only_on_panic: bool,
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        if !self.only_on_panic || thread::panicking() {
            self.window.close();
        }
    }
}

/// The encoded payload size of one job: its data block plus every companion.
fn payload_bytes((data, companions): &JobOutput) -> u64 {
    std::iter::once(data)
        .chain(companions)
        .map(|(_, payload)| payload.bytes.len() as u64)
        .sum()
}

/// A bounded-memory, parallel-encode streaming writer over a [`WriteSession`].
///
/// Every job holds a credit of the writer's [`StreamWindow`] from [`Self::push`] until it is
/// committed in order, so the jobs and encoded bytes held stay within the bound in the
/// [module docs](self), whatever the producer's pace or the encode order.
pub struct StreamWriter {
    input: Option<Sender<(usize, EncodeJob)>>,
    workers: Vec<JoinHandle<()>>,
    committer: Option<JoinHandle<Result<WriteSession>>>,
    window: Arc<CreditWindow>,
    seq: usize,
}

impl StreamWriter {
    /// Build a pipeline over `session` with `workers` encode threads and a queue of `capacity` jobs
    /// waiting for a worker. Both are clamped to ≥ 1. The window is
    /// `StreamWindow::new(capacity + workers, DEFAULT_RAM_BUDGET)`: room for `capacity` queued jobs
    /// plus one encoding on every worker, so the pool is never starved by the window, and the
    /// default 1 GiB ([`crate::DEFAULT_RAM_BUDGET`]) of encoded bytes. Use [`Self::with_window`] to
    /// set the window directly.
    pub fn new(session: WriteSession, workers: usize, capacity: usize) -> Self {
        let workers = workers.max(1);
        let window = StreamWindow::new(
            capacity.max(1).saturating_add(workers),
            crate::config::DEFAULT_RAM_BUDGET,
        );
        Self::with_window(session, workers, window)
    }

    /// Build a pipeline over `session` with `workers` encode threads (clamped to ≥ 1) and the given
    /// credit `window`.
    pub fn with_window(session: WriteSession, workers: usize, window: StreamWindow) -> Self {
        let workers = workers.max(1);
        let window = Arc::new(CreditWindow::new(window));
        // The channels only carry jobs; the window is what bounds them (at most `max_in_flight`
        // jobs sit in both together). Unbounded channels allocate nothing up front, so a window of
        // `usize::MAX` jobs (bound by bytes only) is fine.
        let (in_tx, in_rx) = channel::<(usize, EncodeJob)>();
        let in_rx = Arc::new(Mutex::new(in_rx));
        let (out_tx, out_rx) = channel::<Encoded>();

        let worker_handles = (0..workers)
            .map(|_| {
                let rx = Arc::clone(&in_rx);
                let tx = out_tx.clone();
                let window = Arc::clone(&window);
                thread::spawn(move || {
                    let _close = CloseOnDrop {
                        window: Arc::clone(&window),
                        only_on_panic: true,
                    };
                    loop {
                        // Hold the lock only to dequeue, then encode without it (true parallelism).
                        let job = {
                            let guard = rx.lock().expect("input mutex");
                            guard.recv()
                        };
                        let Ok((seq, job)) = job else {
                            break; // input closed + drained
                        };
                        if !window.wait_encode_turn(seq) {
                            break; // pipeline closed
                        }
                        let res = job();
                        let bytes = res.as_ref().map_or(0, payload_bytes);
                        window.charge(bytes);
                        if tx.send((seq, bytes, res)).is_err() {
                            break; // committer gone (it closed the window on its way out)
                        }
                    }
                })
            })
            .collect();
        drop(out_tx); // only workers hold senders now → out_rx closes when all workers exit

        let committer = {
            let window = Arc::clone(&window);
            thread::spawn(move || {
                let _close = CloseOnDrop {
                    window: Arc::clone(&window),
                    only_on_panic: false,
                };
                commit_loop(session, out_rx, &window)
            })
        };

        StreamWriter {
            input: Some(in_tx),
            workers: worker_handles,
            committer: Some(committer),
            window,
            seq: 0,
        }
    }

    /// The encode-thread pool size this writer was built with — for observability (bench output,
    /// tracing). The committer and the producer thread run outside this count.
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// The credit window this writer enforces.
    pub fn window(&self) -> StreamWindow {
        self.window.limits
    }

    /// A live snapshot of the window's high-water marks and counters.
    pub fn stats(&self) -> StreamStats {
        self.window.stats()
    }

    /// Enqueue a block for encode+commit. Blocks (backpressure) while the [`StreamWindow`] is full:
    /// `max_in_flight` jobs pushed and not yet committed, or `max_in_flight_bytes` of encoded
    /// payloads held. Blocks are committed in call order, so push in the order you want the
    /// manifest to record them. Errors once the pipeline died (a failed encode or commit, or a
    /// panicking worker); [`Self::finish`] then reports the cause.
    pub fn push(&mut self, job: EncodeJob) -> Result<()> {
        let tx = self
            .input
            .as_ref()
            .ok_or_else(|| Error::Container("stream writer already finished".into()))?;
        self.window.acquire()?;
        // The window admitted the job; the send never blocks, and fails only if every worker is gone.
        tx.send((self.seq, job)).map_err(|_| closed_error())?;
        self.seq += 1;
        Ok(())
    }

    /// Drain the pipeline, then compact + seal to `out`. Returns the sealed manifest (identical to a
    /// batch seal of the same blocks in push order).
    pub fn finish(self, out: &Path) -> Result<Manifest> {
        self.finish_with_stats(out).map(|(manifest, _)| manifest)
    }

    /// [`Self::finish`], also returning the window's final [`StreamStats`] — the high-water marks to
    /// assert the memory contract against.
    pub fn finish_with_stats(mut self, out: &Path) -> Result<(Manifest, StreamStats)> {
        drop(self.input.take()); // close input → workers drain + exit → out_rx closes → committer returns
        for w in self.workers.drain(..) {
            w.join()
                .map_err(|_| Error::Container("encode worker panicked".into()))?;
        }
        let session = self
            .committer
            .take()
            .expect("committer present")
            .join()
            .map_err(|_| Error::Container("committer thread panicked".into()))??;
        let stats = self.window.stats();
        Ok((session.seal(out)?, stats))
    }
}

/// The single committer: receives encoded blocks, commits them to the session in `seq` order (a
/// reorder buffer holds completions that arrive early), releases each job's credit once it is
/// committed, and returns the session for sealing. The window bounds the reorder buffer to
/// `max_in_flight` entries.
fn commit_loop(
    mut session: WriteSession,
    out_rx: Receiver<Encoded>,
    window: &CreditWindow,
) -> Result<WriteSession> {
    let mut pending: BTreeMap<usize, (u64, JobOutput)> = BTreeMap::new();
    let mut next = 0usize;
    // Commit the data block then its additive companions (ADR-0028 §5 sidecars), in order, then
    // give the job's credit back.
    let mut commit_ready = |session: &mut WriteSession,
                            pending: &mut BTreeMap<usize, (u64, JobOutput)>|
     -> Result<()> {
        while let Some((bytes, ((bref, payload), companions))) = pending.remove(&next) {
            session.append_block(bref, &payload.bytes)?;
            for (cref, cpayload) in companions {
                session.append_block(cref, &cpayload.bytes)?;
            }
            window.release(bytes);
            next += 1;
        }
        Ok(())
    };
    while let Ok((seq, bytes, res)) = out_rx.recv() {
        pending.insert(seq, (bytes, res?));
        commit_ready(&mut session, &mut pending)?;
    }
    // Channel closed: all workers exited → every pushed block has arrived → drain the contiguous rest.
    commit_ready(&mut session, &mut pending)?;
    // Every pushed job must have committed. Workers exit only after the queue drained or on a
    // panic (which `finish` reports first), so a shortfall here is a pipeline bug: fail loudly
    // rather than seal a truncated product.
    let pushed = window.stats().jobs_pushed;
    if !pending.is_empty() || next != pushed {
        return Err(Error::Container(format!(
            "stream pipeline lost jobs: {next} of {pushed} committed, {} stranded",
            pending.len()
        )));
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{self, ArrayData};
    use crate::Reader;
    use tessera_core::block::array::ArraySpec;
    use tessera_core::ProductBuilder;

    const TS: &str = "2024-01-01T00:00:00Z";

    fn ablock(name: &str, n: i16) -> (BlockRef, ArraySpec, ArrayData) {
        let mut spec = ArraySpec::new(vec![8, 8, 8], "int16");
        spec.codec = "pcodec".into();
        let data = ArrayData::I16((0..512).map(|k| (k as i16).wrapping_mul(3) + n).collect());
        let (r, _p) = array::array_block(name, &spec, &data).unwrap();
        (r, spec, data)
    }

    #[test]
    fn streamed_equals_batch_for_many_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let n = 50;
        let blocks: Vec<_> = (0..n)
            .map(|i| ablock(&format!("b{i:03}"), i as i16))
            .collect();

        // streamed: small capacity + several workers → forces reorder + backpressure
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 4, 3);
        for (i, (_, spec, data)) in blocks.iter().enumerate() {
            sw.push(array_job(format!("b{i:03}"), spec.clone(), data.clone()))
                .unwrap();
        }
        let out = dir.path().join("stream.tsra");
        let sealed = sw.finish(&out).unwrap();

        // batch equivalent in the same (push) order
        let mut bb = ProductBuilder::new("recon", "p", "d", TS);
        for (r, _, _) in &blocks {
            bb.add_block_ref(r.clone());
        }
        let batch = bb.seal().unwrap();
        assert_eq!(sealed.id, batch.id);
        assert_eq!(sealed.content_hash, batch.content_hash);
        assert_eq!(
            sealed.manifest_hash, batch.manifest_hash,
            "streamed seal != batch"
        );

        let mut rdr = Reader::open(&out).unwrap();
        assert_eq!(rdr.block_names().len(), n);
        for nm in rdr.block_names() {
            rdr.read_block(&nm).unwrap();
        }
    }

    #[test]
    fn streamed_product_carries_session_metadata() {
        // metadata-first durability through the streaming path: fields set on the WriteSession (persisted
        // to the durable header) flow through StreamWriter to the sealed product — so a DAQ writer can set
        // modality/study up front and they survive the stream → seal.
        let dir = tempfile::tempdir().unwrap();
        let mut ws =
            WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        ws.with_study("study-9").unwrap();
        ws.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "PT"}),
        )
        .unwrap();
        let mut sw = StreamWriter::new(ws, 2, 2);
        let (_, spec, data) = ablock("b000", 0);
        sw.push(array_job("b000", spec, data)).unwrap();
        let sealed = sw.finish(&dir.path().join("s.tsra")).unwrap();
        assert_eq!(sealed.study.as_deref(), Some("study-9"));
        assert_eq!(
            sealed.metadata.get("modality"),
            Some(&serde_json::json!({"_vocabulary": "DICOM", "_code": "PT"}))
        );
    }

    #[test]
    fn indexed_jobs_emit_chunk_index_sidecars_matching_batch() {
        // ADR-0028 §5 — an indexed job emits each block's chunk-index sidecar during streaming, so the
        // sealed result must equal a batch that adds block then sidecar interleaved in push order.
        let dir = tempfile::tempdir().unwrap();
        let n = 6;
        let blocks: Vec<_> = (0..n)
            .map(|i| ablock(&format!("b{i:03}"), i as i16))
            .collect();

        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 4, 3); // small capacity → reorder/backpressure
        for (i, (_, spec, data)) in blocks.iter().enumerate() {
            sw.push(array_job_indexed(
                format!("b{i:03}"),
                spec.clone(),
                data.clone(),
            ))
            .unwrap();
        }
        let out = dir.path().join("stream.tsra");
        let sealed = sw.finish(&out).unwrap();

        // batch equivalent: data block then its sidecar, interleaved in the same push order.
        let mut bb = ProductBuilder::new("recon", "p", "d", TS);
        for (i, (r, spec, data)) in blocks.iter().enumerate() {
            bb.add_block_ref(r.clone());
            let (_, sidecar) =
                array::array_block_with_index(&format!("b{i:03}"), spec, data).unwrap();
            bb.add_block_ref(sidecar.expect("integer array → sidecar").0);
        }
        let batch = bb.seal().unwrap();
        assert_eq!(
            sealed.content_hash, batch.content_hash,
            "indexed stream != batch + interleaved sidecars"
        );
        assert_eq!(sealed.manifest_hash, batch.manifest_hash);

        // both the data blocks and their `.cidx` sidecars are present + readable.
        let rdr = Reader::open(&out).unwrap();
        let names = rdr.block_names();
        assert_eq!(names.len(), n * 2);
        assert_eq!(names.iter().filter(|nm| nm.ends_with(".cidx")).count(), n);
    }

    // ---- #677: the credit window -------------------------------------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The `n` distinct array blocks the window tests stream.
    fn blocks(n: usize) -> Vec<(BlockRef, ArraySpec, ArrayData)> {
        (0..n)
            .map(|i| ablock(&format!("b{i:03}"), i as i16))
            .collect()
    }

    /// A job that encodes block `i` only after `gate` fires — the barrier that holds job 0.
    fn held_job(i: usize, spec: ArraySpec, data: ArrayData, gate: Receiver<()>) -> EncodeJob {
        Box::new(move || {
            gate.recv().expect("gate sender dropped");
            array_job(format!("b{i:03}"), spec, data)()
        })
    }

    /// A job that encodes block `i` and then reports `i` on `done`.
    fn reporting_job(i: usize, spec: ArraySpec, data: ArrayData, done: Sender<usize>) -> EncodeJob {
        Box::new(move || {
            let out = array_job(format!("b{i:03}"), spec, data)();
            done.send(i).expect("done receiver dropped");
            out
        })
    }

    /// Spin until `cond` holds on the window's live state. Timing-free: it waits for a state, never
    /// for a duration, and every caller waits only for a state the window must reach.
    fn spin_until(window: &CreditWindow, cond: impl Fn(&WindowState) -> bool) {
        while !cond(&window.lock()) {
            thread::yield_now();
        }
    }

    /// The unthrottled reference: every block streamed through a wide window with plenty of workers.
    fn unthrottled(dir: &Path, blocks: &[(BlockRef, ArraySpec, ArrayData)]) -> (Manifest, Vec<u8>) {
        let ws = WriteSession::create(&dir.join("ref_stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 4, blocks.len());
        for (i, (_, spec, data)) in blocks.iter().enumerate() {
            sw.push(array_job(format!("b{i:03}"), spec.clone(), data.clone()))
                .unwrap();
        }
        let out = dir.join("ref.tsra");
        let m = sw.finish(&out).unwrap();
        (m, std::fs::read(&out).unwrap())
    }

    #[test]
    fn held_first_job_cannot_grow_in_flight_past_the_window() {
        // #677 test (b): job 0 is held at a barrier while later jobs finish. Before the window, the
        // committer drained every later completion into its reorder buffer and the producer never
        // blocked, so all N-1 payloads piled up behind job 0. Now a job holds its credit until its
        // in-order commit: exactly `J` jobs are admitted, the producer blocks on job `J`, and nothing
        // past the window is ever encoded while job 0 is held.
        const J: usize = 4;
        const N: usize = 32;
        let dir = tempfile::tempdir().unwrap();
        let blocks = blocks(N);
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        // Byte budget out of reach: only the job credits bind here.
        let sw = StreamWriter::with_window(ws, 2, StreamWindow::new(J, u64::MAX));
        let window = Arc::clone(&sw.window);

        let (gate_tx, gate_rx) = channel();
        let (done_tx, done_rx) = channel();
        let pushed = Arc::new(AtomicUsize::new(0));
        let producer = {
            let pushed = Arc::clone(&pushed);
            let blocks = blocks.clone();
            let out = dir.path().join("held.tsra");
            thread::spawn(move || {
                let mut sw = sw;
                let mut gate = Some(gate_rx);
                for (i, (_, spec, data)) in blocks.into_iter().enumerate() {
                    let job = match gate.take() {
                        Some(g) => held_job(i, spec, data, g),
                        None => reporting_job(i, spec, data, done_tx.clone()),
                    };
                    sw.push(job).unwrap();
                    pushed.fetch_add(1, Ordering::SeqCst);
                }
                let res = sw.finish_with_stats(&out);
                (res, out)
            })
        };

        // The free worker finishes every admitted job behind job 0: jobs 1..J.
        let mut done: Vec<usize> = (1..J).map(|_| done_rx.recv().unwrap()).collect();
        done.sort_unstable();
        assert_eq!(done, (1..J).collect::<Vec<_>>());
        // … and the producer then blocks on job J: the window is full and nothing commits.
        spin_until(&window, |st| st.stats.producer_stalls == 1);
        {
            let st = window.lock();
            assert_eq!(st.in_flight_jobs, J, "the held window is exactly full");
            assert_eq!(st.stats.jobs_pushed, J);
            assert_eq!(st.stats.jobs_committed, 0, "nothing commits behind job 0");
            assert_eq!(st.stats.peak_in_flight_jobs, J);
        }
        assert_eq!(pushed.load(Ordering::SeqCst), J, "push J blocks");
        assert!(
            done_rx.try_recv().is_err(),
            "no job past the window is encoded while job 0 is held"
        );

        // Release job 0: everything drains in order and seals to the unthrottled bytes.
        gate_tx.send(()).unwrap();
        let ((sealed, stats), out) = {
            let (res, out) = producer.join().unwrap();
            (res.unwrap(), out)
        };
        assert_eq!(stats.jobs_pushed, N);
        assert_eq!(stats.jobs_committed, N);
        assert!(stats.peak_in_flight_jobs <= J, "{stats:?}");
        assert!(stats.producer_stalls >= 1);
        let (reference, ref_bytes) = unthrottled(dir.path(), &blocks);
        assert_eq!(sealed.content_hash, reference.content_hash);
        assert_eq!(sealed.manifest_hash, reference.manifest_hash);
        assert_eq!(
            std::fs::read(&out).unwrap(),
            ref_bytes,
            "sealed bytes differ"
        );
        let names = Reader::open(&out).unwrap().block_names();
        let expected: Vec<String> = (0..N).map(|i| format!("b{i:03}")).collect();
        assert_eq!(names, expected, "commit order is push order");
    }

    #[test]
    fn byte_budget_bounds_encoded_bytes_while_the_first_job_is_held() {
        // The byte credit: with job 0 held and a job window wider than the stream, only the byte
        // budget can stop the producer — and it must, and the encoded bytes held must stay within
        // `B + min(W, J) × P`, not grow to (N-1) × P.
        const N: usize = 24;
        const W: usize = 4;
        let dir = tempfile::tempdir().unwrap();
        let blocks = blocks(N);
        let p_est = array::array_block("x", &blocks[1].1, &blocks[1].2)
            .unwrap()
            .1
            .bytes
            .len() as u64;
        let budget = 2 * p_est; // about two payloads
        let window_limits = StreamWindow::new(N + 8, budget);
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let sw = StreamWriter::with_window(ws, W, window_limits);
        let window = Arc::clone(&sw.window);

        let (gate_tx, gate_rx) = channel();
        let producer = {
            let blocks = blocks.clone();
            let out = dir.path().join("bytes.tsra");
            let window = Arc::clone(&window);
            thread::spawn(move || {
                let mut sw = sw;
                let mut gate = Some(gate_rx);
                for (i, (_, spec, data)) in blocks.into_iter().enumerate() {
                    let job = match gate.take() {
                        Some(g) => held_job(i, spec, data, g),
                        None => array_job(format!("b{i:03}"), spec, data),
                    };
                    let before = window.lock().in_flight_bytes;
                    sw.push(job).unwrap();
                    // Until the first stall, pace the producer on the encodes: wait for job i's
                    // payload to be charged, so each push sees every earlier payload. (Nothing
                    // commits while job 0 is held, so the held bytes only grow.)
                    if i > 0 && window.lock().stats.producer_stalls == 0 {
                        spin_until(&window, |st| st.in_flight_bytes > before);
                    }
                }
                (sw.finish_with_stats(&out), out)
            })
        };

        // The job window can't fill (N < J), so a stall here is the byte budget blocking the producer.
        spin_until(&window, |st| st.stats.producer_stalls == 1);
        {
            let st = window.lock();
            assert!(
                st.in_flight_bytes >= budget,
                "stalled below the byte budget"
            );
            assert_eq!(st.stats.jobs_committed, 0);
            assert!(
                st.stats.jobs_pushed < N,
                "the producer was stopped mid-stream"
            );
        }
        gate_tx.send(()).unwrap();
        let (res, out) = producer.join().unwrap();
        let (sealed, stats) = res.unwrap();
        let p = stats.max_job_bytes;
        assert!(p > 0);
        let in_progress = W.min(window_limits.max_in_flight) as u64;
        assert!(
            stats.peak_in_flight_bytes < budget + in_progress * p,
            "peak {} ≥ B {budget} + {in_progress} × P {p}",
            stats.peak_in_flight_bytes
        );
        assert!(stats.peak_in_flight_bytes < (N as u64 - 1) * p);
        assert_eq!(stats.jobs_committed, N);
        let (reference, ref_bytes) = unthrottled(dir.path(), &blocks);
        assert_eq!(sealed.content_hash, reference.content_hash);
        assert_eq!(std::fs::read(&out).unwrap(), ref_bytes);
    }

    #[test]
    fn encode_gate_stops_new_encodes_once_the_byte_budget_is_held() {
        // Gate 2 on its own: jobs 2..5 are admitted while no bytes are held (so gate 1 lets them
        // in), then job 1 finishes and holds the 1-byte budget while job 0 is still held. The free
        // worker must now wait at the encode gate instead of encoding 2..5 behind job 0 — without
        // gate 2 it would hold ~5 payloads; with it the peak stays `< B + min(W, J) × P = 1 + 2P`.
        const N: usize = 6;
        let dir = tempfile::tempdir().unwrap();
        let blocks = blocks(N);
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::with_window(ws, 2, StreamWindow::new(N, 1));
        let window = Arc::clone(&sw.window);
        let (gate0_tx, gate0_rx) = channel();
        let (gate1_tx, gate1_rx) = channel();
        let (done_tx, done_rx) = channel();
        let mut gates = vec![gate0_rx, gate1_rx].into_iter();
        for (i, (_, spec, data)) in blocks.iter().cloned().enumerate() {
            let job = match gates.next() {
                Some(g) => held_job(i, spec, data, g),
                None => reporting_job(i, spec, data, done_tx.clone()),
            };
            // Nothing has finished encoding yet (jobs 0 and 1 are held, and occupy both
            // workers), so every push is admitted without waiting.
            sw.push(job).unwrap();
        }
        assert_eq!(window.lock().stats.producer_stalls, 0);
        gate1_tx.send(()).unwrap();
        // Job 1's worker comes back for job 2 and must wait at the gate (with no gate it would
        // encode 2..5 and report all four).
        let done = std::cell::Cell::new(0);
        spin_until(&window, |st| {
            while done_rx.try_recv().is_ok() {
                done.set(done.get() + 1);
            }
            st.gated_encodes == 1 || done.get() == N - 2
        });
        assert_eq!(done.get(), 0, "jobs encoded past a held byte budget");
        assert_eq!(window.lock().gated_encodes, 1);
        gate0_tx.send(()).unwrap();
        let (_, stats) = sw.finish_with_stats(&dir.path().join("gate.tsra")).unwrap();
        assert_eq!(stats.jobs_committed, N);
        assert!(
            stats.peak_in_flight_bytes < 1 + 2 * stats.max_job_bytes,
            "{stats:?}"
        );
    }

    #[test]
    fn job_larger_than_the_byte_budget_completes_alone() {
        // Deadlock rule: a job whose payload alone exceeds the byte budget is still admitted into an
        // empty window and always runs once it is the job the committer waits on — so a 1-byte budget
        // degrades to one encoded job at a time, never to a hang. The peak is reported.
        let dir = tempfile::tempdir().unwrap();
        let blocks = blocks(8);
        for workers in [1, 4] {
            let stage = dir.path().join(format!("stage{workers}"));
            let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            let mut sw = StreamWriter::with_window(ws, workers, StreamWindow::new(4, 1));
            for (i, (_, spec, data)) in blocks.iter().enumerate() {
                sw.push(array_job_indexed(
                    format!("b{i:03}"),
                    spec.clone(),
                    data.clone(),
                ))
                .unwrap();
            }
            let out = dir.path().join(format!("big{workers}.tsra"));
            let (_, stats) = sw.finish_with_stats(&out).unwrap();
            assert_eq!(stats.jobs_committed, 8);
            assert!(stats.max_job_bytes > 1, "every payload exceeds the budget");
            assert!(stats.peak_in_flight_bytes >= stats.max_job_bytes);
            // B = 1: once one payload is held no other encode may start, so at most the encodes
            // already in progress add theirs: `< B + min(W, J) × P`.
            let in_progress = workers.min(4) as u64;
            assert!(stats.peak_in_flight_bytes < 1 + in_progress * stats.max_job_bytes);
        }
    }

    #[test]
    fn content_hash_is_identical_across_workers_and_windows() {
        // #677 test (c): the window throttles, it never reorders — bytes and content_hash are the same
        // for every worker count and window size, including windows smaller than the pool.
        let dir = tempfile::tempdir().unwrap();
        let blocks = blocks(20);
        let mut runs: Vec<(String, Manifest, Vec<u8>)> = Vec::new();
        for workers in [1usize, 4, 16] {
            let windows = [
                ("1", Some(StreamWindow::new(1, u64::MAX))),
                ("2", Some(StreamWindow::new(2, u64::MAX))),
                ("2/1B", Some(StreamWindow::new(2, 1))),
                ("default", None),
            ];
            for (label, window) in windows {
                let tag = format!("w{workers}-{}", label.replace('/', "_"));
                let ws =
                    WriteSession::create(&dir.path().join(&tag), "recon", "p", "d", TS).unwrap();
                let mut sw = match window {
                    Some(w) => StreamWriter::with_window(ws, workers, w),
                    None => StreamWriter::new(ws, workers, 3),
                };
                for (i, (_, spec, data)) in blocks.iter().enumerate() {
                    sw.push(array_job_indexed(
                        format!("b{i:03}"),
                        spec.clone(),
                        data.clone(),
                    ))
                    .unwrap();
                }
                let out = dir.path().join(format!("{tag}.tsra"));
                let (m, stats) = sw.finish_with_stats(&out).unwrap();
                assert!(stats.peak_in_flight_jobs <= sw_window(window, workers).max_in_flight);
                runs.push((tag, m, std::fs::read(&out).unwrap()));
            }
        }
        let (first_tag, first, first_bytes) = &runs[0];
        for (tag, m, bytes) in &runs[1..] {
            assert_eq!(m.content_hash, first.content_hash, "{tag} vs {first_tag}");
            assert_eq!(m.manifest_hash, first.manifest_hash, "{tag} vs {first_tag}");
            assert_eq!(
                bytes, first_bytes,
                "{tag} vs {first_tag}: sealed bytes differ"
            );
        }
    }

    /// The window a test run used: the explicit one, or [`StreamWriter::new`]'s default for a
    /// capacity-3 queue.
    fn sw_window(window: Option<StreamWindow>, workers: usize) -> StreamWindow {
        window.unwrap_or(StreamWindow::new(3 + workers, crate::DEFAULT_RAM_BUDGET))
    }

    #[test]
    fn new_keeps_one_job_per_worker_beyond_the_queue() {
        // The default window: the queue capacity plus one encoding job per worker, so the window
        // never starves the pool (today's throughput), and the default RAM budget in bytes.
        let dir = tempfile::tempdir().unwrap();
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let sw = StreamWriter::new(ws, 4, 2);
        assert_eq!(sw.window(), StreamWindow::new(6, crate::DEFAULT_RAM_BUDGET));
        assert_eq!(
            StreamWindow::new(0, 0),
            StreamWindow::new(1, 1),
            "clamped to ≥ 1"
        );
        sw.finish(&dir.path().join("s.tsra")).unwrap();
    }

    #[test]
    fn failed_encode_closes_the_window_instead_of_hanging_the_producer() {
        // A job whose encode fails never commits, so its credit never returns. The committer's exit
        // closes the window: the producer blocked on a full window gets an error, and finish
        // reports the encode failure.
        let dir = tempfile::tempdir().unwrap();
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::with_window(ws, 2, StreamWindow::new(1, u64::MAX));
        sw.push(Box::new(|| Err(Error::Codec("boom".into()))))
            .unwrap();
        let (_, spec, data) = ablock("b001", 1);
        let err = sw.push(array_job("b001", spec, data)).unwrap_err();
        assert!(err.to_string().contains("pipeline closed"), "{err}");
        let err = sw.finish(&dir.path().join("s.tsra")).unwrap_err();
        assert!(err.to_string().contains("boom"), "{err}");
    }

    #[test]
    fn panicking_worker_closes_the_window_instead_of_hanging_the_producer() {
        let dir = tempfile::tempdir().unwrap();
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::with_window(ws, 2, StreamWindow::new(1, u64::MAX));
        sw.push(Box::new(|| panic!("encoder bug"))).unwrap();
        let (_, spec, data) = ablock("b001", 1);
        let err = sw.push(array_job("b001", spec, data)).unwrap_err();
        assert!(err.to_string().contains("pipeline closed"), "{err}");
        let err = sw.finish(&dir.path().join("s.tsra")).unwrap_err();
        assert!(err.to_string().contains("worker panicked"), "{err}");
    }
}
